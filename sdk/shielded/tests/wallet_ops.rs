//! Installed `erebus-wallet` onboarding, native wrapping, and backup recovery against Anvil.
//!
//! The wallet generates fresh note material from the operating system; nothing here derives
//! note secrets from the M5 test fixture. Commands run as separate processes and verify
//! finalized effects, retry safety, and stale-backup recovery through fresh RPC endpoints.
//! Set `EREBUS_TEST_INSTALLED_BIN_DIR` to qualify binaries installed outside the checkout;
//! otherwise Cargo-built binaries run. Pool keys and token/wrapper contracts are test fixtures.

use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use axum::{extract::State, Router};
use erebus_core::ids::{AddressBytes, ChainNamespace};
use erebus_core::shielded_auth::derive_key;
use erebus_evm::chain::TransactionKey;
use erebus_shielded_prover::rpc::PoolRpc;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const ANVIL_ACCOUNT: &str = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";

#[path = "support/random_asset.rs"]
mod random_asset;
const MINT_SELECTOR: [u8; 4] = [0x40, 0xc1, 0x0f, 0x19];
const BALANCE_SELECTOR: [u8; 4] = [0x70, 0xa0, 0x82, 0x31];
const ALLOWANCE_SELECTOR: [u8; 4] = [0xdd, 0x62, 0xed, 0x3e];
const FEE_ON_TRANSFER_SELECTOR: [u8; 4] = [0xf1, 0x07, 0x77, 0x9f];
const FAIL_PAYOUT_SELECTOR: [u8; 4] = [0xc6, 0xbe, 0x62, 0xb9];

struct ServiceProcess(Child);

impl Drop for ServiceProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn native_binary(name: &str, fallback: &str) -> PathBuf {
    match std::env::var_os("EREBUS_TEST_INSTALLED_BIN_DIR") {
        Some(directory) => {
            let directory = PathBuf::from(directory).canonicalize().unwrap();
            let checkout = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .canonicalize()
                .unwrap();
            assert!(
                !directory.starts_with(checkout),
                "installed binaries must be outside the checkout"
            );
            let binary = directory.join(name).canonicalize().unwrap();
            assert!(binary.starts_with(&directory) && binary.is_file());
            binary
        }
        None => fallback.into(),
    }
}

fn private(path: &Path, bytes: &[u8]) {
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    std::fs::File::open("/dev/urandom")
        .unwrap()
        .read_exact(&mut bytes)
        .unwrap();
    bytes
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn rpc(client: &reqwest::Client, url: &str, method: &str, params: Value) -> Value {
    client
        .post(url)
        .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

fn word(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[32 - bytes.len()..].copy_from_slice(bytes);
    out
}

fn encode_call(selector: &[u8; 4], words: &[[u8; 32]]) -> String {
    let mut data = selector.to_vec();
    for word in words {
        data.extend_from_slice(word);
    }
    format!("0x{}", hex::encode(data))
}

/// Reads one ERC-20 allowance word.
async fn allowance(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    owner: &str,
    spender: [u8; 20],
) -> u128 {
    let owner: [u8; 20] = hex::decode(owner.trim_start_matches("0x"))
        .unwrap()
        .try_into()
        .unwrap();
    let response = rpc(
        client,
        url,
        "eth_call",
        json!([{
            "to": token,
            "data": encode_call(&ALLOWANCE_SELECTOR, &[word(&owner), word(&spender)])
        }, "latest"]),
    )
    .await;
    let text = response["result"]
        .as_str()
        .unwrap()
        .trim_start_matches("0x");
    let bytes = hex::decode(text).unwrap();
    u128::from_be_bytes(bytes[16..].try_into().unwrap())
}

/// Waits for an allowance value, bounded, so a submitted approval can be included.
async fn wait_for_allowance(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    owner: &str,
    spender: [u8; 20],
    expected: u128,
) -> u128 {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let value = allowance(client, url, token, owner, spender).await;
        if value >= expected || Instant::now() >= deadline {
            return value;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Waits for a token balance, bounded, so a submitted transaction can be included.
async fn wait_for_balance(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    account: &str,
    expected: u128,
) -> u128 {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let balance = token_balance(client, url, token, account).await;
        if balance == expected || Instant::now() >= deadline {
            return balance;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn token_balance(client: &reqwest::Client, url: &str, token: &str, account: &str) -> u128 {
    let address: [u8; 20] = hex::decode(account.trim_start_matches("0x"))
        .unwrap()
        .try_into()
        .unwrap();
    let response = rpc(
        client,
        url,
        "eth_call",
        json!([{"to": token, "data": encode_call(&BALANCE_SELECTOR, &[word(&address)])}, "latest"]),
    )
    .await;
    let text = response["result"]
        .as_str()
        .unwrap()
        .trim_start_matches("0x");
    let bytes = hex::decode(text).unwrap();
    u128::from_be_bytes(bytes[16..].try_into().unwrap())
}

struct Running(Option<Child>);

fn spawn(binary: &Path, cwd: &Path, request: Value) -> Running {
    let mut child = Command::new(binary)
        .current_dir(cwd)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.to_string().as_bytes())
        .unwrap();
    Running(Some(child))
}

/// Runs one wallet request and returns (ok, parsed output, exit code).
fn finish(running: Running) -> (bool, Value, i32) {
    let mut running = running;
    let output = running.0.take().unwrap().wait_with_output().unwrap();
    let code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    let value = serde_json::from_str(&stdout)
        .or_else(|_| serde_json::from_str(&stderr))
        .unwrap_or_else(|_| json!({"status":"missing","stdout":stdout,"stderr":stderr}));
    (output.status.success(), value, code)
}

struct ArtifactServer {
    url: String,
    bodies: Arc<HashMap<String, Vec<u8>>>,
}

async fn artifact_server(files: HashMap<String, Vec<u8>>) -> ArtifactServer {
    use axum::extract::Path as RoutePath;
    async fn serve(
        State(bodies): State<Arc<HashMap<String, Vec<u8>>>>,
        RoutePath(name): RoutePath<String>,
    ) -> Vec<u8> {
        bodies.get(&name).cloned().unwrap_or_default()
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let bodies = Arc::new(files);
    let state = Arc::clone(&bodies);
    tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/{name}", axum::routing::get(serve))
                .with_state(state),
        )
        .await
        .unwrap();
    });
    ArtifactServer {
        url: format!("http://{address}"),
        bodies,
    }
}

async fn fixture_manifest(deployment: &Value) -> Value {
    let build = PathBuf::from(deployment["artifact_build"].as_str().unwrap());
    let mut files = HashMap::new();
    for kind in ["deposit", "transfer", "withdraw"] {
        for suffix in ["wasm", "r1cs", "zkey"] {
            let path = if suffix == "wasm" {
                build.join(format!("{kind}_js/{kind}.wasm"))
            } else {
                build.join(format!("{kind}.{suffix}"))
            };
            files.insert(format!("{kind}.{suffix}"), fs::read(path).unwrap());
        }
    }
    let server = artifact_server(files).await;
    let circuits: Vec<Value> = ["deposit", "transfer", "withdraw"]
        .into_iter()
        .map(|kind| {
            let mut circuit = json!({"kind": kind});
            for suffix in ["wasm", "r1cs", "zkey"] {
                let name = format!("{kind}.{suffix}");
                let body = &server.bodies[&name];
                circuit[suffix] = json!({
                    "url": format!("{}/{name}", server.url),
                    "sha256": hex::encode(Sha256::digest(body)),
                    "bytes": body.len(),
                });
            }
            circuit
        })
        .collect();
    json!({
        "version": 1, "chain_id": 31337, "settlement_contract": deployment["pool"],
        "verifier_version": 2, "test_only": true, "circuits": circuits,
    })
}

/// A loopback JSON-RPC forwarder that can drop the next raw-transaction response once.
#[derive(Clone)]
struct LossyState {
    upstream: String,
    client: reqwest::Client,
    drop_next: Arc<AtomicBool>,
    sends: Arc<AtomicUsize>,
}

async fn lossy_proxy(upstream: String) -> String {
    lossy_proxy_with_send_count(upstream).await.0
}

async fn lossy_proxy_with_send_count(upstream: String) -> (String, Arc<AtomicUsize>) {
    use axum::{
        extract::State as AxumState, http::StatusCode, response::IntoResponse,
        routing::post as axum_post,
    };
    async fn forward(
        AxumState(state): AxumState<LossyState>,
        body: String,
    ) -> axum::response::Response {
        let request: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        let is_send = |request: &Value| {
            matches!(
                request["method"].as_str(),
                Some("eth_sendRawTransaction" | "eth_sendTransaction")
            )
        };
        let sends = match request.as_array() {
            Some(requests) => requests.iter().filter(|request| is_send(request)).count(),
            None => usize::from(is_send(&request)),
        };
        state.sends.fetch_add(sends, Ordering::SeqCst);
        let dropping = sends > 0 && state.drop_next.swap(false, Ordering::SeqCst);
        // Always forward; dropping means only the response is lost, so the transaction can
        // still land while the caller sees an unknown outcome.
        match state
            .client
            .post(&state.upstream)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
        {
            Ok(response) => {
                let body = response.text().await.unwrap_or_default();
                if dropping {
                    return (StatusCode::BAD_GATEWAY, "dropped").into_response();
                }
                (StatusCode::OK, body).into_response()
            }
            Err(_) => (StatusCode::BAD_GATEWAY, "upstream").into_response(),
        }
    }
    async fn mark(AxumState(state): AxumState<LossyState>) -> &'static str {
        state.drop_next.store(true, Ordering::SeqCst);
        "ok"
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = LossyState {
        upstream,
        client: reqwest::Client::new(),
        drop_next: Arc::new(AtomicBool::new(false)),
        sends: Arc::new(AtomicUsize::new(0)),
    };
    let sends = state.sends.clone();
    tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/", axum_post(forward))
                .route("/drop-next", axum_post(mark))
                .with_state(state),
        )
        .await
        .unwrap();
    });
    (format!("http://{address}"), sends)
}

async fn wait_for_chain(client: &reqwest::Client, url: &str) {
    for _ in 0..200 {
        if client
            .post(url)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}))
            .send()
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("anvil did not start");
}

/// A send whose exact call would revert is refused at its estimate, before any claim (WS6).
const ESTIMATE_REFUSED: &str =
    "gas estimate failed; the call would revert or the provider failed; nothing was signed";

async fn mine(client: &reqwest::Client, url: &str) {
    rpc(client, url, "anvil_mine", json!(["0x2"])).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil and generated M5 prototype artifacts; local prototype setup"]
async fn installed_wallet_deposits_random_notes_and_withdraws_one() {
    let port = free_port();
    let anvil = ServiceProcess(
        Command::new("anvil")
            .args([
                "--port",
                &port.to_string(),
                "--chain-id",
                "31337",
                "--slots-in-an-epoch",
                "1",
                "--block-time",
                "1",
                "--silent",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let url = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    wait_for_chain(&client, &url).await;
    let proxy_url = lossy_proxy(url.clone()).await;
    let peer_url = url.clone();

    // Deploy the pool through the existing native fixture; the fixture's own note is ignored.
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../circuits/m5/scripts/native-fixture.mjs");
    let fixture_url = url.clone();
    let output = tokio::task::spawn_blocking(move || {
        Command::new("node")
            .arg(script)
            .arg(fixture_url)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(
        output.status.success(),
        "fixture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let deployment: Value = serde_json::from_slice(&output.stdout).unwrap();
    let pool = deployment["pool"].as_str().unwrap().to_owned();
    let token = deployment["token"].as_str().unwrap().to_owned();
    let first_block = deployment["first_block"].as_u64().unwrap();
    let first_hash = deployment["first_hash"].as_str().unwrap().to_owned();

    // Fresh participant material from the operating system.
    let transaction_key = random_bytes::<32>();
    let signer = TransactionKey::from_bytes(&transaction_key)
        .unwrap()
        .address();
    let signer_hex = format!("0x{}", hex::encode(signer));
    let agreement_seed = random_bytes::<32>();
    let owner = derive_key(&agreement_seed).unwrap();
    let wallet_key = random_bytes::<32>();
    let recipient: [u8; 20] = random_bytes();

    rpc(
        &client,
        &url,
        "anvil_setBalance",
        json!([signer_hex, "0x56bc75e2d63100000"]),
    )
    .await;
    // Mint the test token to the wallet's own gas signer from the unlocked deployment account.
    let mint = encode_call(
        &MINT_SELECTOR,
        &[word(&signer), word(&100u128.to_be_bytes())],
    );
    let sent = rpc(
        &client,
        &url,
        "eth_sendTransaction",
        json!([{"from": ANVIL_ACCOUNT, "to": token, "data": mint, "gas": "0x100000"}]),
    )
    .await;
    assert!(sent["result"].is_string(), "mint failed: {sent}");
    let pool_address: [u8; 20] = hex::decode(pool.trim_start_matches("0x"))
        .unwrap()
        .try_into()
        .unwrap();
    mine(&client, &url).await;

    // Local fixture manifest only; the caller pins its exact bytes.
    let manifest = fixture_manifest(&deployment).await;
    let root = tempfile::tempdir().unwrap();
    let manifest_path = root.path().join("manifest.json");
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    std::fs::write(&manifest_path, &manifest_bytes).unwrap();
    let manifest_digest = hex::encode(Sha256::digest(&manifest_bytes));
    let fixture_pins = PoolRpc::new(&url, 31337, pool_address)
        .unwrap()
        .runtime_pins_at(
            hex::decode(first_hash.trim_start_matches("0x"))
                .unwrap()
                .try_into()
                .unwrap(),
        )
        .await
        .unwrap();

    let state_root = root.path().join("state");
    std::fs::create_dir(&state_root).unwrap();
    let transaction_key_file = root.path().join("transaction.key");
    let wallet_key_file = root.path().join("wallet.key");
    let agreement_key_file = root.path().join("agreement.key");
    private(&transaction_key_file, &transaction_key);
    private(&wallet_key_file, &wallet_key);
    private(&agreement_key_file, &owner);
    let config_path = root.path().join("wallet.json");
    private(
        &config_path,
        json!({
            "version": 1,
            "state_root": state_root,
            "namespace": "eip155:31337",
            "pool": pool,
            "verifier_version": 2,
            "asset": token,
            "wrapper": deployment["wmon"].as_str().unwrap(),
            "rpc_url": proxy_url,
            "peer_rpc_url": peer_url,
            "first_block": first_block,
            "first_hash": first_hash,
            "wallet_file": state_root.join("wallet/notes.enc"),
            "wallet_key_file": wallet_key_file,
            "agreement_key_file": agreement_key_file,
            "signer_address": signer_hex,
            "signer_journal_root": state_root.join("signer-journal"),
            "transaction_key_file": transaction_key_file,
            "artifact_manifest_file": manifest_path,
            "artifact_manifest_sha256": manifest_digest,
            "artifact_cache_root": state_root.join("artifacts"),
            "runtime_pins": {
                "pool": hex::encode(fixture_pins.pool), "poseidon": hex::encode(fixture_pins.poseidon),
                "deposit": hex::encode(fixture_pins.deposit), "transfer": hex::encode(fixture_pins.transfer),
                "withdraw": hex::encode(fixture_pins.withdraw),
            },
            "allow_test_artifacts": true,
            "allow_loopback_artifact_http": true,
            "gas_limit": 2_000_000u64,
            "max_fee_per_gas": "1000000000",
            "max_priority_fee_per_gas": "100000000",
            "timeout_seconds": 30,
            "max_scan_blocks": 1000,
            "artifact_timeout_seconds": 120,
        })
        .to_string()
        .as_bytes(),
    );

    let wallet_bin = native_binary("erebus-wallet", env!("CARGO_BIN_EXE_erebus-wallet"));
    let frozen_wallet = root.path().join("erebus-wallet");
    fs::copy(&wallet_bin, &frozen_wallet).unwrap();
    let wallet_bin = frozen_wallet;
    let request = |method: &str, extra: Value| {
        let mut value = json!({"method": method, "config_file": config_path});
        if let Some(object) = extra.as_object() {
            for (key, extra_value) in object {
                value_insert(&mut value, key, extra_value.clone());
            }
        }
        value
    };

    // 1. Allowance onboarding through the installed wallet; wait until the chain reflects it.
    // Diagnostics intentionally read finalized state, not the mint/funding at a newer head.
    rpc(&client, &url, "anvil_mine", json!(["0x40"])).await;
    let diagnostic = finish(spawn(
        &wallet_bin,
        root.path(),
        request("status", json!({})),
    ));
    assert!(
        diagnostic.0,
        "paired account diagnostics failed: {diagnostic:?}"
    );
    assert_eq!(diagnostic.1["result"]["account"]["asset_balance"], "100");
    assert_eq!(diagnostic.1["result"]["account"]["allowance"], "0");
    assert_eq!(
        diagnostic.1["result"]["account"]["native_balance"],
        "100000000000000000000"
    );
    assert_eq!(
        diagnostic.1["result"]["checkpoints"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(diagnostic.1["result"]["payment_verified"], false);
    assert!(diagnostic.1["result"]["checkpoints"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["caught_up"] == false));
    let approved = finish(spawn(
        &wallet_bin,
        root.path(),
        request("approve", json!({"amount":"1000"})),
    ));
    assert!(
        approved.0,
        "approve failed: {} (exit {})",
        approved.1, approved.2
    );
    assert_eq!(
        wait_for_allowance(&client, &url, &token, &signer_hex, pool_address, 1000).await,
        1000
    );
    rpc(&client, &url, "anvil_mine", json!(["0x40"])).await;
    let diagnostic = finish(spawn(
        &wallet_bin,
        root.path(),
        request("status", json!({})),
    ));
    assert!(
        diagnostic.0,
        "post-approval diagnostics failed: {diagnostic:?}"
    );
    assert_eq!(diagnostic.1["result"]["account"]["allowance"], "1000");

    // 2. A fee-on-transfer deposit would revert in the pool. The exact call is estimated before
    // any nonce claim (WS6), so it is refused there: nothing is signed or sent and the lane stays
    // free. (Finalized-revert reconciliation stays covered by the wrap and unwrap revert cases.)
    let enable_fee = encode_call(&FEE_ON_TRANSFER_SELECTOR, &[word(&1u128.to_be_bytes())]);
    let fee = rpc(
        &client,
        &url,
        "eth_sendTransaction",
        json!([{"from": ANVIL_ACCOUNT, "to": token, "data": enable_fee, "gas": "0x100000"}]),
    )
    .await;
    assert!(fee["result"].is_string(), "setFeeOnTransfer failed: {fee}");
    mine(&client, &url).await;

    let refused = finish(spawn(
        &wallet_bin,
        root.path(),
        request(
            "deposit",
            json!({"amount":"10", "operation_ref":"55".repeat(32)}),
        ),
    ));
    assert!(
        !refused.0 && refused.1["error"] == ESTIMATE_REFUSED,
        "a reverting deposit must be refused at the estimate: {refused:?}"
    );
    let synced = finish(spawn(&wallet_bin, root.path(), request("sync", json!({}))));
    assert!(synced.0, "{synced:?}");
    assert_eq!(
        synced.1["result"]["nonce_released"], true,
        "no claim was written"
    );
    assert!(synced.1["result"]["pending"]
        .as_array()
        .unwrap()
        .iter()
        .all(|operation| operation["kind"] != "deposit"
            || (operation["state"] == "planned"
                && operation["retained"] == false
                && operation["attempts"].as_array().unwrap().is_empty())));
    // The proved note opening was persisted before the refusal and is reused by the retry.
    let promised = synced.1["result"]["pending"]
        .as_array()
        .unwrap()
        .iter()
        .find(|operation| operation["kind"] == "deposit")
        .expect("the planned deposit stays retryable")["identity"]
        .as_str()
        .unwrap()
        .to_owned();

    // 3. Clear the token restriction and retry; the same note opening is reused.
    let disable_fee = encode_call(&FEE_ON_TRANSFER_SELECTOR, &[word(&0u128.to_be_bytes())]);
    let fee = rpc(
        &client,
        &url,
        "eth_sendTransaction",
        json!([{"from": ANVIL_ACCOUNT, "to": token, "data": disable_fee, "gas": "0x100000"}]),
    )
    .await;
    assert!(fee["result"].is_string(), "setFeeOnTransfer failed: {fee}");
    mine(&client, &url).await;
    let retried = finish(spawn(
        &wallet_bin,
        root.path(),
        request(
            "deposit",
            json!({"amount":"10", "operation_ref":"55".repeat(32)}),
        ),
    ));
    assert!(
        retried.0,
        "retry failed: {} (exit {})",
        retried.1, retried.2
    );
    let first_commitment = retried.1["result"]["commitment"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        first_commitment, promised,
        "a retry must reuse the persisted note opening"
    );
    let first_included = sync_until(&wallet_bin, root.path(), &config_path, |value| {
        value["result"]["notes"]
            .as_array()
            .is_some_and(|notes| notes.iter().filter(|note| note["included"] == true).count() >= 1)
            && value["result"]["pending"]
                .as_array()
                .is_some_and(Vec::is_empty)
    })
    .await;
    assert!(
        first_included.0,
        "first deposit never finalized: {first_included:?}"
    );

    let nonce_before = rpc(
        &client,
        &url,
        "eth_getTransactionCount",
        json!([signer_hex, "pending"]),
    )
    .await;
    let repeated = finish(spawn(
        &wallet_bin,
        root.path(),
        request(
            "deposit",
            json!({"amount":"10", "operation_ref":"55".repeat(32)}),
        ),
    ));
    assert!(repeated.0, "completed deposit retry failed: {repeated:?}");
    assert_eq!(repeated.1["result"]["already_completed"], true);
    assert_eq!(repeated.1["result"]["identity"], first_commitment);
    assert_eq!(
        rpc(
            &client,
            &url,
            "eth_getTransactionCount",
            json!([signer_hex, "pending"])
        )
        .await,
        nonce_before
    );
    let conflict = finish(spawn(
        &wallet_bin,
        root.path(),
        request(
            "deposit",
            json!({"amount":"11", "operation_ref":"55".repeat(32)}),
        ),
    ));
    assert!(!conflict.0, "changed amount reused a deposit reference");
    assert_eq!(
        rpc(
            &client,
            &url,
            "eth_getTransactionCount",
            json!([signer_hex, "pending"])
        )
        .await,
        nonce_before
    );

    // 4. A second deposit of the same amount produces different random material.
    let second = finish(spawn(
        &wallet_bin,
        root.path(),
        request(
            "deposit",
            json!({"amount":"10", "operation_ref":"66".repeat(32)}),
        ),
    ));
    assert!(
        second.0,
        "second deposit failed: {} (exit {})",
        second.1, second.2
    );
    let second_commitment = second.1["result"]["commitment"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(
        first_commitment, second_commitment,
        "note material must be random, not deterministic"
    );
    let two_included = sync_until(&wallet_bin, root.path(), &config_path, |value| {
        value["result"]["notes"]
            .as_array()
            .is_some_and(|notes| notes.iter().filter(|note| note["included"] == true).count() >= 2)
            && value["result"]["pending"]
                .as_array()
                .is_some_and(Vec::is_empty)
    })
    .await;
    assert!(
        two_included.0,
        "second deposit never finalized: {two_included:?}"
    );
    assert_eq!(
        wait_for_balance(&client, &url, &token, &signer_hex, 80).await,
        80
    );

    // Retain a backup before withdrawal, when both notes still appear unspent.
    let wallet_backup = fs::read(state_root.join("wallet/notes.enc")).unwrap();

    // 5. Withdraw the first note and verify consumption plus exact balances.
    let withdrawal = finish(spawn(
        &wallet_bin,
        root.path(),
        request(
            "withdraw",
            json!({"commitment": first_commitment, "recipient": format!("0x{}", hex::encode(recipient))}),
        ),
    ));
    assert!(
        withdrawal.0,
        "withdrawal failed: {} (exit {})",
        withdrawal.1, withdrawal.2
    );
    assert_eq!(
        wait_for_balance(&client, &url, &token, &signer_hex, 80).await,
        80,
        "withdrawal must not touch the signer's token balance"
    );
    let consumed = sync_until(&wallet_bin, root.path(), &config_path, |value| {
        value["result"]["notes"].as_array().is_some_and(|notes| {
            notes
                .iter()
                .any(|note| note["consumed"] == true && note["commitment"] == first_commitment)
        }) && value["result"]["pending"]
            .as_array()
            .is_some_and(Vec::is_empty)
    })
    .await;
    assert!(consumed.0, "sync never observed consumption: {consumed:?}");
    assert_eq!(
        wait_for_balance(
            &client,
            &url,
            &token,
            &format!("0x{}", hex::encode(recipient)),
            10
        )
        .await,
        10
    );

    // 6. Installed native wrap, completed only after the finalized WMON mint is verified.
    // The first broadcast response is dropped: the operation must stay unknown, then be
    // completed by observation with exactly one onchain execution.
    let wmon = deployment["wmon"].as_str().unwrap().to_owned();
    // Live WMON has a nonzero supply (and no totalSupply slot at all); a first-ever wrap into this
    // mock would also pay a zero-to-nonzero totalSupply write and exceed the 65,000 wrap limit.
    // Seed the mock's supply from the Anvil account so the wrap is priced like a live one.
    let seeded = rpc(
        &client,
        &url,
        "eth_sendTransaction",
        json!([{"from": ANVIL_ACCOUNT, "to": wmon, "data": "0xd0e30db0", "value": "0x1", "gas": "0x100000"}]),
    )
    .await;
    assert!(seeded["result"].is_string(), "seed wrap failed: {seeded}");
    mine(&client, &url).await;
    let marked = client
        .post(format!("{proxy_url}/drop-next"))
        .send()
        .await
        .unwrap();
    assert!(marked.status().is_success());
    let lost = finish(spawn(
        &wallet_bin,
        root.path(),
        request(
            "wrap",
            json!({"amount":"5","operation_ref":"a1".repeat(32)}),
        ),
    ));
    assert_eq!(
        lost.2, 2,
        "a lost response must be pending/unknown: {lost:?}"
    );
    assert_eq!(lost.1["status"], "pending");
    let lost_sync = sync_until(&wallet_bin, root.path(), &config_path, |value| {
        operation_completed(value, "wrap")
    })
    .await;
    assert!(
        lost_sync.0,
        "lost-response wrap never completed: {lost_sync:?}"
    );
    assert_eq!(
        wait_for_balance(&client, &url, &wmon, &signer_hex, 5).await,
        5
    );
    let wrapped = finish(spawn(
        &wallet_bin,
        root.path(),
        request(
            "wrap",
            json!({"amount":"5","operation_ref":"a2".repeat(32)}),
        ),
    ));
    assert!(
        wrapped.0,
        "second wrap was refused: {} (exit {})",
        wrapped.1, wrapped.2
    );
    assert_eq!(
        wait_for_balance(&client, &url, &wmon, &signer_hex, 10).await,
        10
    );

    // 6b. A same-block unrelated transfer must not defeat completion: the wrapper mint event
    // authenticates the call, while the parent/including balance delta is only supplementary.
    let wmon_address: [u8; 20] = hex::decode(wmon.trim_start_matches("0x"))
        .unwrap()
        .try_into()
        .unwrap();
    let transfer_recipient: [u8; 20] = [0x5a; 20];
    // Release any finalized signer-lane claim before the mempool-parked wrap.
    let released = sync_until(&wallet_bin, root.path(), &config_path, |value| {
        value["result"]["nonce_released"] == true
    })
    .await;
    assert!(released.0, "signer lane was not released: {released:?}");
    rpc(&client, &url, "anvil_setAutomine", json!([false])).await;
    rpc(&client, &url, "anvil_setIntervalMining", json!([0])).await;
    let same_block = finish(spawn(
        &wallet_bin,
        root.path(),
        request(
            "wrap",
            json!({"amount":"5","operation_ref":"f1".repeat(32)}),
        ),
    ));
    assert!(
        same_block.0,
        "same-block wrap was not submitted: {} (exit {})",
        same_block.1, same_block.2
    );
    // Sign an unrelated WMON transfer with the next nonce while the wrap sits in the mempool.
    let pending_nonce = rpc(
        &client,
        &url,
        "eth_getTransactionCount",
        json!([signer_hex, "pending"]),
    )
    .await["result"]
        .as_str()
        .unwrap()
        .trim_start_matches("0x")
        .to_owned();
    let nonce = u64::from_str_radix(&pending_nonce, 16).unwrap();
    let key = TransactionKey::from_bytes(&transaction_key).unwrap();
    let mut transfer_data = vec![0xa9, 0x05, 0x9c, 0xbb];
    transfer_data.extend_from_slice(&word(&transfer_recipient));
    transfer_data.extend_from_slice(&word(&5u128.to_be_bytes()));
    let plan = erebus_evm::chain::SigningPlan::new(
        signer,
        erebus_evm::chain::TransactionParams {
            nonce,
            fees: erebus_evm::chain::Eip1559Fees::new(1_000_000_000, 100_000_000).unwrap(),
            gas_limit: 200_000,
            value: 0,
        },
    )
    .unwrap();
    let transfer_raw = plan
        .sign_call(31337, wmon_address, transfer_data.clone(), &key)
        .unwrap()
        .raw()
        .to_vec();
    let sent = rpc(
        &client,
        &url,
        "eth_sendRawTransaction",
        json!([format!("0x{}", hex::encode(&transfer_raw))]),
    )
    .await;
    assert!(
        sent["result"].is_string(),
        "unrelated transfer failed: {sent}"
    );
    rpc(&client, &url, "anvil_mine", json!(["0x1"])).await;
    rpc(&client, &url, "anvil_setAutomine", json!([true])).await;
    rpc(&client, &url, "anvil_setIntervalMining", json!([1])).await;
    let same_block_identity = same_block.1["result"]["identity"]
        .as_str()
        .unwrap()
        .to_owned();
    let same_block_sync = sync_until(&wallet_bin, root.path(), &config_path, |value| {
        operation_identity_completed(value, &same_block_identity)
    })
    .await;
    assert!(
        same_block_sync.0,
        "same-block wrap never completed: {same_block_sync:?}"
    );
    assert_eq!(
        wait_for_balance(&client, &url, &wmon, &signer_hex, 10).await,
        10
    );
    assert_eq!(
        wait_for_balance(
            &client,
            &url,
            &wmon,
            &format!("0x{}", hex::encode(transfer_recipient)),
            5
        )
        .await,
        5
    );

    // 7. A wrapper that fails the native payout reverts the whole unwrap; the burn rolls back
    // and only the finalized receipt may return the operation to a retryable plan. A send that
    // would revert at estimate time is refused before signing (WS6), so the failure is switched
    // on after the unwrap's estimate and broadcast: mining pauses, the unwrap enters the mempool,
    // and a higher-tip setFailPayout is ordered ahead of it in the next block.
    rpc(&client, &url, "evm_setIntervalMining", json!([0])).await;
    rpc(&client, &url, "evm_setAutomine", json!([false])).await;
    let failed_unwrap = finish(spawn(
        &wallet_bin,
        root.path(),
        request(
            "unwrap",
            json!({"amount":"5","operation_ref":"b1".repeat(32)}),
        ),
    ));
    assert!(
        failed_unwrap.0,
        "failed-payout unwrap was not submitted: {} (exit {})",
        failed_unwrap.1, failed_unwrap.2
    );
    let enable = encode_call(&FAIL_PAYOUT_SELECTOR, &[word(&1u128.to_be_bytes())]);
    let fee = rpc(
        &client,
        &url,
        "eth_sendTransaction",
        json!([{"from": ANVIL_ACCOUNT, "to": wmon, "data": enable, "gas": "0x100000",
            "maxFeePerGas": "0x2540be400", "maxPriorityFeePerGas": "0x2540be400"}]),
    )
    .await;
    assert!(fee["result"].is_string(), "setFailPayout failed: {fee}");
    rpc(&client, &url, "evm_mine", json!([])).await;
    rpc(&client, &url, "evm_setIntervalMining", json!([1])).await;
    mine(&client, &url).await;
    let failed_sync = sync_until(&wallet_bin, root.path(), &config_path, |value| {
        operation_reverted(value, "unwrap")
    })
    .await;
    assert!(
        failed_sync.0,
        "payout revert never reconciled: {failed_sync:?}"
    );
    assert_eq!(
        token_balance(&client, &url, &wmon, &signer_hex).await,
        10,
        "a reverted payout must leave the WMON balance unchanged"
    );

    // 8. Clear the payout failure; the retry completes the same operation and burns the WMON.
    let disable = encode_call(&FAIL_PAYOUT_SELECTOR, &[word(&0u128.to_be_bytes())]);
    let fee = rpc(
        &client,
        &url,
        "eth_sendTransaction",
        json!([{"from": ANVIL_ACCOUNT, "to": wmon, "data": disable, "gas": "0x100000"}]),
    )
    .await;
    assert!(fee["result"].is_string(), "setFailPayout failed: {fee}");
    mine(&client, &url).await;
    let unwrapped = finish(spawn(
        &wallet_bin,
        root.path(),
        request(
            "unwrap",
            json!({"amount":"5","operation_ref":"b1".repeat(32)}),
        ),
    ));
    assert!(
        unwrapped.0,
        "unwrap retry failed: {} (exit {})",
        unwrapped.1, unwrapped.2
    );
    let unwrapped_done = sync_until(&wallet_bin, root.path(), &config_path, |value| {
        operation_completed(value, "unwrap")
    })
    .await;
    assert!(
        unwrapped_done.0,
        "unwrap never completed: {unwrapped_done:?}"
    );
    assert_eq!(token_balance(&client, &url, &wmon, &signer_hex).await, 5);

    // 9. Unwrapping more than the wrapper balance would revert: refused at the estimate, before
    // any claim or signature, and nothing is sent.
    let too_much = finish(spawn(
        &wallet_bin,
        root.path(),
        request(
            "unwrap",
            json!({"amount":"6","operation_ref":"c1".repeat(32)}),
        ),
    ));
    assert!(
        !too_much.0 && too_much.1["error"] == ESTIMATE_REFUSED,
        "over-unwrap must be refused at the estimate: {too_much:?}"
    );
    assert_eq!(token_balance(&client, &url, &wmon, &signer_hex).await, 5);

    // 10. A wrong wrapper target would revert on unknown calldata: refused at the estimate.
    let bad_config = root.path().join("wallet-wrong-wrapper.json");
    private(
        &bad_config,
        json!({
            "version": 1, "state_root": state_root, "namespace": "eip155:31337",
            "pool": pool, "verifier_version": 2, "asset": token, "wrapper": pool,
            "rpc_url": proxy_url, "peer_rpc_url": peer_url, "first_block": first_block, "first_hash": first_hash,
            "wallet_file": state_root.join("wallet/notes.enc"), "wallet_key_file": wallet_key_file,
            "agreement_key_file": agreement_key_file, "signer_address": signer_hex,
            "signer_journal_root": state_root.join("signer-journal"), "transaction_key_file": transaction_key_file,
            "artifact_manifest_file": manifest_path, "artifact_manifest_sha256": manifest_digest,
            "artifact_cache_root": state_root.join("artifacts"), "allow_test_artifacts": true,
            "allow_loopback_artifact_http": true, "gas_limit": 2_000_000u64,
            "max_fee_per_gas": "1000000000", "max_priority_fee_per_gas": "100000000",
            "timeout_seconds": 30, "max_scan_blocks": 1000, "artifact_timeout_seconds": 120,
        })
        .to_string()
        .as_bytes(),
    );
    let wrong = finish(spawn(
        &wallet_bin,
        root.path(),
        json!({"method":"wrap","config_file":bad_config,"amount":"1","operation_ref":"d1".repeat(32)}),
    ));
    assert!(
        !wrong.0 && wrong.1["error"] == ESTIMATE_REFUSED,
        "a wrong-wrapper wrap must be refused at the estimate: {wrong:?}"
    );

    // 10b. A config pointing at a foreign pool fails closed before any operation is created.
    let foreign_config = root.path().join("wallet-foreign-pool.json");
    private(
        &foreign_config,
        json!({
            "version": 1, "state_root": state_root, "namespace": "eip155:31337",
            "pool": "0x00000000000000000000000000000000000000ff", "verifier_version": 2,
            "asset": token, "wrapper": wmon,
            "rpc_url": proxy_url, "peer_rpc_url": peer_url, "first_block": first_block, "first_hash": first_hash,
            "wallet_file": state_root.join("wallet/notes.enc"), "wallet_key_file": wallet_key_file,
            "agreement_key_file": agreement_key_file, "signer_address": signer_hex,
            "signer_journal_root": state_root.join("signer-journal"), "transaction_key_file": transaction_key_file,
            "artifact_manifest_file": manifest_path, "artifact_manifest_sha256": manifest_digest,
            "artifact_cache_root": state_root.join("artifacts"), "allow_test_artifacts": true,
            "allow_loopback_artifact_http": true, "gas_limit": 2_000_000u64,
            "max_fee_per_gas": "1000000000", "max_priority_fee_per_gas": "100000000",
            "timeout_seconds": 30, "max_scan_blocks": 1000, "artifact_timeout_seconds": 120,
        })
        .to_string()
        .as_bytes(),
    );
    let foreign = finish(spawn(
        &wallet_bin,
        root.path(),
        json!({"method":"wrap","config_file":foreign_config,"amount":"1","operation_ref":"aa".repeat(32)}),
    ));
    assert!(
        !foreign.0 && foreign.2 == 1,
        "foreign pool was not refused: {foreign:?}"
    );

    // 10c. A corrupted public index fails closed and recovers by rebuilding from the chain.
    let primary_index = state_root.join("wallet-index/primary.json");
    let saved_index = fs::read(&primary_index).unwrap();
    fs::write(&primary_index, b"not-json").unwrap();
    let corrupt = finish(spawn(
        &wallet_bin,
        root.path(),
        json!({"method":"sync","config_file":config_path}),
    ));
    assert!(
        !corrupt.0 && corrupt.2 == 1,
        "corrupt index was not refused: {corrupt:?}"
    );
    fs::write(&primary_index, &saved_index).unwrap();
    let recovered = sync_until(&wallet_bin, root.path(), &config_path, |value| {
        value["result"]["notes"].as_array().is_some()
    })
    .await;
    assert!(recovered.0, "index recovery failed: {recovered:?}");

    // 10d. Restore stale encrypted state with new RPC endpoints and no retained cache.
    // Recovery must rediscover consumption, not resurrect the withdrawn note or send funds.
    let retained_wallet = state_root.join("wallet/notes-retained.enc");
    fs::rename(state_root.join("wallet/notes.enc"), &retained_wallet).unwrap();
    private(&state_root.join("wallet/notes.enc"), &wallet_backup);
    fs::rename(
        state_root.join("wallet-index"),
        state_root.join("wallet-index-retained"),
    )
    .unwrap();
    let switched_config = root.path().join("wallet-restored.json");
    let mut recovery_config: Value =
        serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
    let (restored_rpc, primary_sends) = lossy_proxy_with_send_count(url.clone()).await;
    let (restored_peer_rpc, peer_sends) = lossy_proxy_with_send_count(url.clone()).await;
    recovery_config["rpc_url"] = json!(restored_rpc);
    recovery_config["peer_rpc_url"] = json!(restored_peer_rpc);
    recovery_config["transaction_key_file"] = Value::Null;
    recovery_config["artifact_manifest_file"] = json!(root.path().join("absent-manifest.json"));
    recovery_config["artifact_cache_root"] = json!(root.path().join("absent-artifacts"));
    private(
        &switched_config,
        &serde_json::to_vec(&recovery_config).unwrap(),
    );
    let before_recovery = rpc(
        &client,
        &url,
        "eth_getTransactionCount",
        json!([signer_hex, "pending"]),
    )
    .await;
    let restored = sync_until(&wallet_bin, root.path(), &switched_config, |value| {
        value["result"]["notes"].as_array().is_some_and(|notes| {
            notes.len() == 2
                && notes.iter().any(|note| {
                    note["commitment"] == first_commitment
                        && note["included"] == true
                        && note["consumed"] == true
                })
                && notes.iter().any(|note| {
                    note["commitment"] == second_commitment
                        && note["included"] == true
                        && note["consumed"] == false
                        && note["reserved"].is_null()
                })
        })
    })
    .await;
    assert!(restored.0, "stale backup recovery failed: {restored:?}");
    assert!(state_root.join("wallet-index/primary.json").is_file());
    assert!(state_root.join("wallet-index/peer.json").is_file());
    assert!(!root.path().join("absent-artifacts").exists());
    let after_recovery = rpc(
        &client,
        &url,
        "eth_getTransactionCount",
        json!([signer_hex, "pending"]),
    )
    .await;
    assert_eq!(before_recovery["result"], after_recovery["result"]);
    assert_eq!(primary_sends.load(Ordering::SeqCst), 0);
    assert_eq!(peer_sends.load(Ordering::SeqCst), 0);
    assert_eq!(token_balance(&client, &url, &token, &signer_hex).await, 80);
    assert_eq!(
        token_balance(
            &client,
            &url,
            &token,
            &format!("0x{}", hex::encode(recipient))
        )
        .await,
        10
    );

    // 11. Insufficient native funds is refused before any transaction is built.
    let poor = finish(spawn(
        &wallet_bin,
        root.path(),
        request(
            "wrap",
            json!({"amount":"1000000000000000000000000000","operation_ref":"e1".repeat(32)}),
        ),
    ));
    assert!(
        !poor.0 && poor.2 == 1,
        "insufficient funds was not refused: {poor:?}"
    );
    drop(anvil);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires generated M5 prototype artifacts and Anvil; not live WMON qualification"]
async fn installed_wallet_wraps_deposits_withdraws_and_unwraps_native_funds() {
    const WRAPPED: u128 = 20_000_000_000_000_000;
    const PRIVATE_AMOUNT: u128 = WRAPPED / 2;
    const INITIAL_NATIVE: u128 = 100_000_000_000_000_000_000;
    let port = free_port();
    let _anvil = ServiceProcess(
        Command::new("anvil")
            .args([
                "--port",
                &port.to_string(),
                "--chain-id",
                "31337",
                "--slots-in-an-epoch",
                "1",
                "--block-time",
                "1",
                "--silent",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let url = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    wait_for_chain(&client, &url).await;
    let fixture_url = url.clone();
    let deployment: Value = tokio::task::spawn_blocking(move || {
        let script = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../circuits/m5/scripts/native-fixture.mjs");
        let output = Command::new("node")
            .arg(script)
            .arg(fixture_url)
            .arg("wmon")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "WMON fixture failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    })
    .await
    .unwrap();
    assert_eq!(deployment["asset_mode"], "wmon");
    assert_eq!(deployment["token"], deployment["wmon"]);
    let token = deployment["token"].as_str().unwrap();
    let pool = deployment["pool"].as_str().unwrap();
    let transaction_key = random_bytes::<32>();
    let signer = TransactionKey::from_bytes(&transaction_key)
        .unwrap()
        .address();
    let signer_hex = format!("0x{}", hex::encode(signer));
    let funded = rpc(
        &client,
        &url,
        "anvil_setBalance",
        json!([signer_hex, format!("0x{INITIAL_NATIVE:x}")]),
    )
    .await;
    assert!(funded.get("error").is_none());
    let root = tempfile::tempdir().unwrap();
    let state_root = root.path().join("state");
    fs::create_dir(&state_root).unwrap();
    let transaction_key_file = root.path().join("transaction.key");
    let wallet_key_file = root.path().join("wallet.key");
    let agreement_key_file = root.path().join("agreement.key");
    private(&transaction_key_file, &transaction_key);
    private(&wallet_key_file, &random_bytes::<32>());
    private(
        &agreement_key_file,
        &derive_key(&random_bytes::<32>()).unwrap(),
    );
    let manifest = serde_json::to_vec(&fixture_manifest(&deployment).await).unwrap();
    let manifest_path = root.path().join("manifest.json");
    private(&manifest_path, &manifest);
    let (proxy_url, sends) = lossy_proxy_with_send_count(url.clone()).await;
    let config_path = root.path().join("wallet.json");
    private(&config_path, &serde_json::to_vec(&json!({
        "version": 1, "state_root": state_root, "namespace": "eip155:31337",
        "pool": pool, "verifier_version": 2, "asset": token, "wrapper": token,
        "rpc_url": proxy_url, "peer_rpc_url": url,
        "first_block": deployment["first_block"], "first_hash": deployment["first_hash"],
        "wallet_file": state_root.join("wallet/notes.enc"),
        "wallet_key_file": wallet_key_file, "agreement_key_file": agreement_key_file,
        "signer_address": signer_hex, "signer_journal_root": state_root.join("signer-journal"),
        "transaction_key_file": transaction_key_file,
        "artifact_manifest_file": manifest_path, "artifact_manifest_sha256": hex::encode(Sha256::digest(&manifest)),
        "artifact_cache_root": state_root.join("artifacts"), "allow_test_artifacts": true,
        "allow_loopback_artifact_http": true, "gas_limit": 2_000_000u64,
        "max_fee_per_gas": "1000000000", "max_priority_fee_per_gas": "100000000",
        "timeout_seconds": 30, "max_scan_blocks": 1000, "artifact_timeout_seconds": 120,
    })).unwrap());
    let wallet_bin = native_binary("erebus-wallet", env!("CARGO_BIN_EXE_erebus-wallet"));
    let frozen_wallet = root.path().join("erebus-wallet");
    fs::copy(&wallet_bin, &frozen_wallet).unwrap();
    let wallet_bin = frozen_wallet;
    let request = |method: &str, extra: Value| {
        let mut value = json!({"method": method, "config_file": config_path});
        for (key, entry) in extra.as_object().unwrap() {
            value_insert(&mut value, key, entry.clone());
        }
        value
    };
    assert_eq!(token_balance(&client, &url, token, &signer_hex).await, 0);
    assert_eq!(token_balance(&client, &url, token, pool).await, 150);
    let wrapped = finish(spawn(
        &wallet_bin,
        root.path(),
        request(
            "wrap",
            json!({
                "amount": WRAPPED.to_string(), "operation_ref": "71".repeat(32),
            }),
        ),
    ));
    assert!(wrapped.0, "wrap failed: {wrapped:?}");
    let observed = sync_until(&wallet_bin, root.path(), &config_path, |value| {
        operation_completed(value, "wrap")
    })
    .await;
    assert!(observed.0, "wrap did not finalize: {observed:?}");
    assert_eq!(
        token_balance(&client, &url, token, &signer_hex).await,
        WRAPPED
    );

    let approved = finish(spawn(
        &wallet_bin,
        root.path(),
        request("approve", json!({"amount": PRIVATE_AMOUNT.to_string()})),
    ));
    assert!(approved.0, "approve failed: {approved:?}");
    let observed = sync_until(&wallet_bin, root.path(), &config_path, |value| {
        operation_completed(value, "approve")
    })
    .await;
    assert!(observed.0, "approval did not finalize: {observed:?}");
    let deposited = finish(spawn(
        &wallet_bin,
        root.path(),
        request("deposit", json!({"amount": PRIVATE_AMOUNT.to_string()})),
    ));
    assert!(deposited.0, "WMON deposit failed: {deposited:?}");
    let commitment = deposited.1["result"]["commitment"]
        .as_str()
        .unwrap()
        .to_owned();
    let observed = sync_until(&wallet_bin, root.path(), &config_path, |value| {
        value["result"]["notes"].as_array().is_some_and(|notes| {
            notes.len() == 1
                && notes[0]["commitment"] == commitment
                && notes[0]["included"] == true
                && notes[0]["consumed"] == false
        })
    })
    .await;
    assert!(observed.0, "WMON note recovery failed: {observed:?}");
    assert_eq!(
        token_balance(&client, &url, token, &signer_hex).await,
        WRAPPED - PRIVATE_AMOUNT
    );
    assert_eq!(
        token_balance(&client, &url, token, pool).await,
        150 + PRIVATE_AMOUNT
    );

    let withdrawn = finish(spawn(
        &wallet_bin,
        root.path(),
        request(
            "withdraw",
            json!({
                "commitment": commitment, "recipient": signer_hex,
            }),
        ),
    ));
    assert!(withdrawn.0, "WMON withdrawal failed: {withdrawn:?}");
    let observed = sync_until(&wallet_bin, root.path(), &config_path, |value| {
        value["result"]["notes"].as_array().is_some_and(|notes| {
            notes.len() == 1 && notes[0]["commitment"] == commitment && notes[0]["consumed"] == true
        })
    })
    .await;
    assert!(observed.0, "WMON consumption not recovered: {observed:?}");
    assert_eq!(
        token_balance(&client, &url, token, &signer_hex).await,
        WRAPPED
    );
    assert_eq!(token_balance(&client, &url, token, pool).await, 150);
    let unwrapped = finish(spawn(
        &wallet_bin,
        root.path(),
        request(
            "unwrap",
            json!({
                "amount": WRAPPED.to_string(), "operation_ref": "72".repeat(32),
            }),
        ),
    ));
    assert!(unwrapped.0, "unwrap failed: {unwrapped:?}");
    let observed = sync_until(&wallet_bin, root.path(), &config_path, |value| {
        operation_completed(value, "unwrap")
    })
    .await;
    assert!(observed.0, "unwrap did not finalize: {observed:?}");
    assert_eq!(token_balance(&client, &url, token, &signer_hex).await, 0);
    assert_eq!(sends.load(Ordering::SeqCst), 5);

    // The wrapped native amount returns exactly; only the five transaction fees remain spent.
    let operations = observed.1["result"]["operations"].as_array().unwrap();
    assert_eq!(operations.len(), 5);
    let mut fees = 0u128;
    for operation in operations {
        assert_eq!(operation["state"], "completed");
        let attempts = operation["attempts"].as_array().unwrap();
        let hash = attempts[0]["tx_hash"].as_str().unwrap();
        // Some operations retain both unknown and completed observations of one hash.
        assert!(attempts.iter().all(|attempt| attempt["tx_hash"] == hash));
        let tx = format!("0x{hash}");
        let receipt = rpc(&client, &url, "eth_getTransactionReceipt", json!([tx])).await;
        assert_eq!(receipt["result"]["status"], "0x1");
        let parse = |field: &str| {
            u128::from_str_radix(
                receipt["result"][field]
                    .as_str()
                    .unwrap()
                    .trim_start_matches("0x"),
                16,
            )
            .unwrap()
        };
        fees += parse("gasUsed") * parse("effectiveGasPrice");
    }
    let balance = rpc(
        &client,
        &url,
        "eth_getBalance",
        json!([signer_hex, "latest"]),
    )
    .await;
    let balance = u128::from_str_radix(
        balance["result"].as_str().unwrap().trim_start_matches("0x"),
        16,
    )
    .unwrap();
    assert_eq!(balance + fees, INITIAL_NATIVE);
    let retried = finish(spawn(
        &wallet_bin,
        root.path(),
        request(
            "wrap",
            json!({
                "amount": WRAPPED.to_string(), "operation_ref": "71".repeat(32),
            }),
        ),
    ));
    assert!(retried.0, "completed wrap retry failed: {retried:?}");
    let retried = finish(spawn(
        &wallet_bin,
        root.path(),
        request(
            "unwrap",
            json!({
                "amount": WRAPPED.to_string(), "operation_ref": "72".repeat(32),
            }),
        ),
    ));
    assert!(retried.0, "completed unwrap retry failed: {retried:?}");
    assert_eq!(sends.load(Ordering::SeqCst), 5);
    assert_eq!(token_balance(&client, &url, token, &signer_hex).await, 0);
}

fn operation_identity_completed(value: &Value, identity: &str) -> bool {
    value["result"]["operations"]
        .as_array()
        .is_some_and(|operations| {
            operations.iter().any(|operation| {
                operation["identity"] == identity && operation["state"] == "completed"
            })
        })
}

fn operation_completed(value: &Value, kind: &str) -> bool {
    value["result"]["operations"]
        .as_array()
        .is_some_and(|operations| {
            operations
                .iter()
                .any(|operation| operation["kind"] == kind && operation["state"] == "completed")
        })
}

fn operation_reverted(value: &Value, kind: &str) -> bool {
    value["result"]["operations"]
        .as_array()
        .is_some_and(|operations| {
            operations.iter().any(|operation| {
                operation["kind"] == kind
                    && operation["state"] == "planned"
                    && operation["retained"] == false
                    && operation["attempts"].as_array().is_some_and(|attempts| {
                        attempts
                            .last()
                            .is_some_and(|last| last["outcome"] == "reverted")
                    })
            })
        })
}

fn value_insert(target: &mut Value, key: &str, value: Value) {
    if let Some(object) = target.as_object_mut() {
        object.insert(key.to_owned(), value);
    }
}

/// Runs sync until `predicate` holds or the deadline passes; a failing exit is returned.
async fn sync_until(
    binary: &Path,
    cwd: &Path,
    config: &Path,
    predicate: impl Fn(&Value) -> bool,
) -> (bool, Value, i32) {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let binary = binary.to_path_buf();
        let cwd = cwd.to_path_buf();
        let request = json!({"method":"sync","config_file":config});
        let result = tokio::task::spawn_blocking(move || finish(spawn(&binary, &cwd, request)))
            .await
            .unwrap();
        if result.0 && predicate(&result.1) {
            return result;
        }
        if Instant::now() >= deadline {
            // The predicate never held; never report this as a satisfied sync.
            return (false, result.1, result.2);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

// ---- WS1: checkpointed history start ------------------------------------------------------------
//
// These run against a real Anvil pool (production Poseidon, generated M5 verifiers, real proofs).
// Each provider is a counting proxy that can also tamper with exactly one class of response, so
// paired acquisition is exercised against genuine contract state rather than a hand-written mock.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tamper {
    None,
    /// Flip a byte of `filledSubtrees(level)`.
    Filled(u8),
    /// Flip a byte of `currentRoot()`.
    Root,
    /// Report `nextLeafIndex() + 1`.
    CountPlusOne,
    /// Report `knownRoots(currentRoot)` as false.
    UnknownRoot,
    /// Return a JSON-RPC error for every `eth_call`.
    FailCalls,
    /// After the frontier reads, report a different hash for the anchor block.
    AnchorChanges,
    /// After the frontier reads, report block 1 as finalized.
    FinalityRegresses,
}

#[derive(Default, Debug)]
struct Counts {
    methods: std::collections::BTreeMap<String, usize>,
    /// Lowest `fromBlock` of any `eth_getLogs` request.
    min_log_from: Option<u64>,
    /// Lowest explicit block number of any `eth_getBlockByNumber` request.
    min_header: Option<u64>,
    /// Explicit `eth_getBlockByNumber` requests at or below a watched height.
    headers_at_or_below_watch: usize,
    /// Distinct heights at or below the watched height whose headers were requested.
    old_headers: std::collections::BTreeSet<u64>,
}

#[derive(Clone)]
struct CountingState {
    upstream: String,
    client: reqwest::Client,
    tamper: Arc<std::sync::Mutex<Tamper>>,
    counts: Arc<std::sync::Mutex<Counts>>,
    watch: Arc<std::sync::atomic::AtomicU64>,
    frontier_read: Arc<AtomicBool>,
}

struct CountingProxy {
    url: String,
    tamper: Arc<std::sync::Mutex<Tamper>>,
    counts: Arc<std::sync::Mutex<Counts>>,
    watch: Arc<std::sync::atomic::AtomicU64>,
    frontier_read: Arc<AtomicBool>,
}

impl CountingProxy {
    fn set(&self, tamper: Tamper) {
        *self.tamper.lock().unwrap() = tamper;
        self.frontier_read.store(false, Ordering::SeqCst);
    }
    fn reset_counts(&self, watch: u64) {
        *self.counts.lock().unwrap() = Counts::default();
        self.watch.store(watch, Ordering::SeqCst);
    }
    fn counts(&self) -> std::collections::BTreeMap<String, usize> {
        self.counts.lock().unwrap().methods.clone()
    }
    fn snapshot(
        &self,
    ) -> (
        std::collections::BTreeMap<String, usize>,
        Option<u64>,
        std::collections::BTreeSet<u64>,
    ) {
        let counts = self.counts.lock().unwrap();
        (
            counts.methods.clone(),
            counts.min_log_from,
            counts.old_headers.clone(),
        )
    }
    fn total(&self) -> usize {
        self.counts.lock().unwrap().methods.values().sum()
    }
}

fn selector(signature: &str) -> String {
    use sha3::Digest as _;
    hex::encode(&sha3::Keccak256::digest(signature.as_bytes())[..4])
}

fn quantity(value: &Value) -> Option<u64> {
    value
        .as_str()
        .and_then(|text| text.strip_prefix("0x"))
        .and_then(|digits| u64::from_str_radix(digits, 16).ok())
}

fn flip_word(result: &mut Value, delta: impl Fn(&mut [u8; 32])) {
    if let Some(text) = result.as_str() {
        if let Ok(bytes) = hex::decode(text.trim_start_matches("0x")) {
            if let Ok(mut word) = <[u8; 32]>::try_from(bytes) {
                delta(&mut word);
                *result = json!(format!("0x{}", hex::encode(word)));
            }
        }
    }
}

async fn counting_proxy(upstream: String) -> CountingProxy {
    use axum::{
        extract::State as AxumState, http::StatusCode, response::IntoResponse,
        routing::post as axum_post,
    };
    async fn forward(
        AxumState(state): AxumState<CountingState>,
        body: String,
    ) -> axum::response::Response {
        let mut request: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        let tamper = *state.tamper.lock().unwrap();
        let method = request["method"].as_str().unwrap_or("batch").to_owned();
        {
            let mut counts = state.counts.lock().unwrap();
            *counts.methods.entry(method.clone()).or_default() += 1;
            if method == "eth_getLogs" {
                if let Some(from) = quantity(&request["params"][0]["fromBlock"]) {
                    counts.min_log_from =
                        Some(counts.min_log_from.map_or(from, |old| old.min(from)));
                }
            }
            if method == "eth_getBlockByNumber" {
                if let Some(number) = quantity(&request["params"][0]) {
                    counts.min_header =
                        Some(counts.min_header.map_or(number, |old| old.min(number)));
                    if number <= state.watch.load(Ordering::SeqCst) {
                        counts.headers_at_or_below_watch += 1;
                        counts.old_headers.insert(number);
                    }
                }
            }
        }
        let data = request["params"][0]["data"]
            .as_str()
            .unwrap_or("")
            .trim_start_matches("0x")
            .to_owned();
        let call = |signature: &str| method == "eth_call" && data.starts_with(&selector(signature));
        if tamper == Tamper::FailCalls && method == "eth_call" {
            return (StatusCode::OK, json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32000,"message":"injected"}}).to_string()).into_response();
        }
        if tamper == Tamper::FinalityRegresses
            && state.frontier_read.load(Ordering::SeqCst)
            && method == "eth_getBlockByNumber"
            && request["params"][0] == "finalized"
        {
            request["params"][0] = json!("0x1");
        }
        let forwarded = match state
            .client
            .post(&state.upstream)
            .header("content-type", "application/json")
            .body(request.to_string())
            .send()
            .await
        {
            Ok(response) => response.text().await.unwrap_or_default(),
            Err(_) => return (StatusCode::BAD_GATEWAY, "upstream").into_response(),
        };
        let mut reply: Value = match serde_json::from_str(&forwarded) {
            Ok(reply) => reply,
            Err(_) => return (StatusCode::OK, forwarded).into_response(),
        };
        if call("knownRoots(uint256)") {
            state.frontier_read.store(true, Ordering::SeqCst);
        }
        match tamper {
            Tamper::Filled(level)
                if call("filledSubtrees(uint256)") && data.ends_with(&format!("{level:064x}")) =>
            {
                flip_word(&mut reply["result"], |word| word[31] ^= 1);
            }
            Tamper::Root if call("currentRoot()") => {
                flip_word(&mut reply["result"], |word| word[31] ^= 1)
            }
            Tamper::CountPlusOne if call("nextLeafIndex()") => {
                flip_word(&mut reply["result"], |word| {
                    word[31] = word[31].wrapping_add(1)
                });
            }
            Tamper::UnknownRoot if call("knownRoots(uint256)") => {
                flip_word(&mut reply["result"], |word| *word = [0; 32])
            }
            Tamper::AnchorChanges
                if state.frontier_read.load(Ordering::SeqCst)
                    && method == "eth_getBlockByNumber"
                    && quantity(&request["params"][0]).is_some() =>
            {
                if let Some(hash) = reply["result"]["hash"].as_str() {
                    let mut altered = hash.to_owned();
                    altered.replace_range(
                        altered.len() - 1..,
                        if hash.ends_with('0') { "1" } else { "0" },
                    );
                    reply["result"]["hash"] = json!(altered);
                }
            }
            _ => {}
        }
        (StatusCode::OK, reply.to_string()).into_response()
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = CountingState {
        upstream,
        client: reqwest::Client::new(),
        tamper: Arc::new(std::sync::Mutex::new(Tamper::None)),
        counts: Arc::new(std::sync::Mutex::new(Counts::default())),
        watch: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        frontier_read: Arc::new(AtomicBool::new(false)),
    };
    let proxy = CountingProxy {
        url: format!("http://{address}"),
        tamper: state.tamper.clone(),
        counts: state.counts.clone(),
        watch: state.watch.clone(),
        frontier_read: state.frontier_read.clone(),
    };
    tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/", axum_post(forward))
                .with_state(state),
        )
        .await
        .unwrap();
    });
    proxy
}

struct HistoryChain {
    _anvil: ServiceProcess,
    url: String,
    client: reqwest::Client,
    deployment: Value,
    pool: [u8; 20],
    first_block: u64,
    first_hash: [u8; 32],
    pins: erebus_shielded_prover::rpc::PoolRuntimePins,
}

fn bytes32(value: &str) -> [u8; 32] {
    hex::decode(value.trim_start_matches("0x"))
        .unwrap()
        .try_into()
        .unwrap()
}

/// Deploys the native fixture and adds `deposits` real-proof notes plus `empty` blocks of history.
async fn history_chain(asset: &str, deposits: u32, empty: u32) -> HistoryChain {
    let port = free_port();
    let anvil = ServiceProcess(
        Command::new("anvil")
            .args([
                "--port",
                &port.to_string(),
                "--chain-id",
                "31337",
                "--slots-in-an-epoch",
                "1",
                "--block-time",
                "1",
                "--silent",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let url = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    wait_for_chain(&client, &url).await;
    let scripts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../circuits/m5/scripts");
    let (fixture_url, asset_mode) = (url.clone(), asset.to_owned());
    let fixture_scripts = scripts.clone();
    let deployment: Value = tokio::task::spawn_blocking(move || {
        let output = Command::new("node")
            .arg(fixture_scripts.join("native-fixture.mjs"))
            .arg(fixture_url)
            .arg(asset_mode)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "fixture failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    })
    .await
    .unwrap();
    let (history_url, pool_text, token_text) = (
        url.clone(),
        deployment["pool"].as_str().unwrap().to_owned(),
        deployment["token"].as_str().unwrap().to_owned(),
    );
    let asset_mode = asset.to_owned();
    let summary: Value = tokio::task::spawn_blocking(move || {
        let output = Command::new("node")
            .arg(scripts.join("checkpoint-history.mjs"))
            .args([
                history_url,
                pool_text,
                token_text,
                deposits.to_string(),
                empty.to_string(),
                asset_mode,
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "history failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    })
    .await
    .unwrap();
    assert_eq!(summary["leaf_count"], deposits + 1);
    let pool: [u8; 20] = hex::decode(
        deployment["pool"]
            .as_str()
            .unwrap()
            .trim_start_matches("0x"),
    )
    .unwrap()
    .try_into()
    .unwrap();
    let first_block = deployment["first_block"].as_u64().unwrap();
    let first_hash = bytes32(deployment["first_hash"].as_str().unwrap());
    let pins = PoolRpc::new(&url, 31337, pool)
        .unwrap()
        .runtime_pins_at(first_hash)
        .await
        .unwrap();
    HistoryChain {
        _anvil: anvil,
        url,
        client,
        deployment,
        pool,
        first_block,
        first_hash,
        pins,
    }
}

impl HistoryChain {
    fn domain(&self) -> erebus_shielded_prover::index_store::IndexDomain {
        erebus_shielded_prover::index_store::IndexDomain {
            chain_id: 31337,
            pool: self.pool,
            first_block: self.first_block,
            first_hash: self.first_hash,
        }
    }

    async fn wait_finalized_past(&self, height: u64) {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let finalized = rpc(
                &self.client,
                &self.url,
                "eth_getBlockByNumber",
                json!(["finalized", false]),
            )
            .await;
            if quantity(&finalized["result"]["number"]).is_some_and(|number| number > height) {
                return;
            }
            assert!(Instant::now() < deadline, "finality did not advance");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    async fn call_at(&self, data: String, block: u64) -> [u8; 32] {
        let reply = rpc(
            &self.client,
            &self.url,
            "eth_call",
            json!([
            {"to": self.deployment["pool"], "data": data}, format!("0x{block:x}")]),
        )
        .await;
        bytes32(reply["result"].as_str().unwrap())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil, Node and generated M5 prototype artifacts; local WS1 qualification"]
async fn paired_checkpoint_matches_full_history_and_refuses_every_provider_fault() {
    use erebus_shielded_prover::{indexer::PoolIndex, rpc::RpcError};
    let chain = history_chain("erc20", 6, 400).await;
    let primary = counting_proxy(chain.url.clone()).await;
    let peer = counting_proxy(chain.url.clone()).await;
    let rpc_for = |proxy: &CountingProxy| PoolRpc::new(&proxy.url, 31337, chain.pool).unwrap();
    let (first, second) = (rpc_for(&primary), rpc_for(&peer));

    // Honest providers: the checkpoint equals a full deployment-era replay at the same block.
    let started = Instant::now();
    let checkpoint = first
        .acquire_checkpoint_agreed(&second, chain.domain(), chain.pins)
        .await
        .unwrap();
    let acquisition = started.elapsed();
    let acquisition_counts = (primary.counts(), peer.counts());
    checkpoint.verify().unwrap();
    assert_eq!(checkpoint.leaf_count, 7);
    let mut full = PoolIndex::new(chain.first_block).unwrap();
    while full
        .tip()
        .is_none_or(|tip| tip.number < checkpoint.block_number)
    {
        first
            .sync(&mut full, checkpoint.block_number, 1_000)
            .await
            .unwrap();
    }
    assert_eq!(full.tip().unwrap().hash, checkpoint.block_hash);
    assert_eq!(full.root(), checkpoint.root);
    assert_eq!(full.leaf_count(), checkpoint.leaf_count);
    eprintln!(
        "WS1 acquisition {acquisition:?}; requests primary {:?} peer {:?}",
        acquisition_counts.0, acquisition_counts.1
    );
    for counts in [&acquisition_counts.0, &acquisition_counts.1] {
        assert!(
            !counts.contains_key("eth_getLogs"),
            "acquisition never scans logs: {counts:?}"
        );
        assert!(
            counts.keys().all(|method| !method.starts_with("eth_send")),
            "{counts:?}"
        );
    }

    // One provider lying about any frontier field is a disagreement.
    let used = (checkpoint.leaf_count - 1).trailing_ones() as u8;
    for tamper in [
        Tamper::Filled(used),
        Tamper::Root,
        Tamper::CountPlusOne,
        Tamper::UnknownRoot,
        Tamper::FailCalls,
        Tamper::AnchorChanges,
        Tamper::FinalityRegresses,
    ] {
        peer.set(tamper);
        let refused = first
            .acquire_checkpoint_agreed(&second, chain.domain(), chain.pins)
            .await;
        assert!(refused.is_err(), "peer {tamper:?} must be refused");
        peer.set(Tamper::None);
    }
    // Both providers lying identically: internal consistency still refuses frontier/root/root-membership lies.
    for tamper in [Tamper::Filled(used), Tamper::Root, Tamper::UnknownRoot] {
        primary.set(tamper);
        peer.set(tamper);
        let refused = first
            .acquire_checkpoint_agreed(&second, chain.domain(), chain.pins)
            .await;
        assert!(
            matches!(refused, Err(RpcError::Evidence(_))),
            "identical {tamper:?} must be refused"
        );
    }
    // A finality regression or changed anchor on the primary during rechecks is refused too.
    for tamper in [Tamper::AnchorChanges, Tamper::FinalityRegresses] {
        primary.set(tamper);
        peer.set(Tamper::None);
        assert!(first
            .acquire_checkpoint_agreed(&second, chain.domain(), chain.pins)
            .await
            .is_err());
    }
    primary.set(Tamper::None);
    peer.set(Tamper::None);
    // Wrong deployment identity or the same endpoint twice is a configuration error.
    let mut other = chain.domain();
    other.first_hash = [9; 32];
    assert!(first
        .acquire_checkpoint_agreed(&second, other, chain.pins)
        .await
        .is_err());
    let mut wrong_pins = chain.pins;
    wrong_pins.transfer = [7; 32];
    assert!(first
        .acquire_checkpoint_agreed(&second, chain.domain(), wrong_pins)
        .await
        .is_err());
    assert!(matches!(
        first
            .acquire_checkpoint_agreed(&first, chain.domain(), chain.pins)
            .await,
        Err(RpcError::Configuration)
    ));

    // Identical count+1 is root-indistinguishable (a zero leaf hashes like empty space), so it can be
    // acquired; the next real insertion is then refused instead of being misplaced.
    primary.set(Tamper::CountPlusOne);
    peer.set(Tamper::CountPlusOne);
    let inflated = first
        .acquire_checkpoint_agreed(&second, chain.domain(), chain.pins)
        .await;
    primary.set(Tamper::None);
    peer.set(Tamper::None);
    if let Ok(inflated) = inflated {
        let pool_text = chain.deployment["pool"].as_str().unwrap().to_owned();
        let token_text = chain.deployment["token"].as_str().unwrap().to_owned();
        let url = chain.url.clone();
        tokio::task::spawn_blocking(move || {
            let scripts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../circuits/m5/scripts");
            assert!(Command::new("node")
                .arg(scripts.join("checkpoint-history.mjs"))
                .args([
                    url,
                    pool_text,
                    token_text,
                    "1".into(),
                    "0".into(),
                    "erc20".into()
                ])
                .output()
                .unwrap()
                .status
                .success());
        })
        .await
        .unwrap();
        let head = rpc(&chain.client, &chain.url, "eth_blockNumber", json!([])).await;
        let head = quantity(&head["result"]).unwrap();
        chain.wait_finalized_past(head).await;
        let mut index = PoolIndex::from_checkpoint(inflated).unwrap();
        let mut refused = false;
        for _ in 0..20 {
            match first.sync(&mut index, head, 1_000).await {
                Ok(_) if index.tip().is_some_and(|tip| tip.number >= head) => break,
                Ok(_) => continue,
                Err(_) => {
                    refused = true;
                    break;
                }
            }
        }
        assert!(
            refused,
            "an inflated leaf count must not survive the next insertion"
        );
    }

    // Deposits racing acquisition: the checkpoint is exactly the contract state at its block, and a
    // later scan from block + 1 applies each racing insertion once.
    let race = {
        let (url, pool_text, token_text) = (
            chain.url.clone(),
            chain.deployment["pool"].as_str().unwrap().to_owned(),
            chain.deployment["token"].as_str().unwrap().to_owned(),
        );
        tokio::task::spawn_blocking(move || {
            let scripts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../circuits/m5/scripts");
            Command::new("node")
                .arg(scripts.join("checkpoint-history.mjs"))
                .args([
                    url,
                    pool_text,
                    token_text,
                    "3".into(),
                    "0".into(),
                    "erc20".into(),
                ])
                .output()
                .unwrap()
        })
    };
    tokio::time::sleep(Duration::from_secs(2)).await;
    let racing = first
        .acquire_checkpoint_agreed(&second, chain.domain(), chain.pins)
        .await
        .unwrap();
    assert!(race.await.unwrap().status.success());
    let count = chain
        .call_at(
            format!("0x{}", selector("nextLeafIndex()")),
            racing.block_number,
        )
        .await;
    let root = chain
        .call_at(
            format!("0x{}", selector("currentRoot()")),
            racing.block_number,
        )
        .await;
    assert_eq!(
        u32::from_be_bytes(count[28..].try_into().unwrap()),
        racing.leaf_count
    );
    assert_eq!(root, racing.root);
    let head =
        quantity(&rpc(&chain.client, &chain.url, "eth_blockNumber", json!([])).await["result"])
            .unwrap();
    chain.wait_finalized_past(head).await;
    let mut index = PoolIndex::from_checkpoint(racing).unwrap();
    while index.tip().is_none_or(|tip| tip.number < head) {
        first.sync(&mut index, head, 1_000).await.unwrap();
    }
    let chain_count = chain
        .call_at(format!("0x{}", selector("nextLeafIndex()")), head)
        .await;
    assert_eq!(
        index.leaf_count(),
        u32::from_be_bytes(chain_count[28..].try_into().unwrap())
    );
    assert_eq!(
        index.root(),
        chain
            .call_at(format!("0x{}", selector("currentRoot()")), head)
            .await
    );
}

fn checkpoint_config(
    chain: &HistoryChain,
    root: &Path,
    rpc_url: &str,
    peer_url: &str,
    asset: &str,
) -> PathBuf {
    let state_root = root.join("state");
    fs::create_dir_all(&state_root).unwrap();
    let transaction_key = random_bytes::<32>();
    let signer = TransactionKey::from_bytes(&transaction_key)
        .unwrap()
        .address();
    private(&root.join("transaction.key"), &transaction_key);
    private(&root.join("wallet.key"), &random_bytes::<32>());
    private(
        &root.join("agreement.key"),
        &derive_key(&random_bytes::<32>()).unwrap(),
    );
    let manifest =
        serde_json::to_vec(&futures_lite_block(fixture_manifest(&chain.deployment))).unwrap();
    private(&root.join("manifest.json"), &manifest);
    let token = chain.deployment["token"].as_str().unwrap();
    let config = root.join("wallet.json");
    private(&config, &serde_json::to_vec(&json!({
        "version": 1, "state_root": state_root, "namespace": "eip155:31337",
        "pool": chain.deployment["pool"], "verifier_version": 2, "asset": token,
        "wrapper": if asset == "wmon" { json!(token) } else { Value::Null },
        "rpc_url": rpc_url, "peer_rpc_url": peer_url,
        "first_block": chain.first_block, "first_hash": chain.deployment["first_hash"],
        "wallet_file": state_root.join("wallet/notes.enc"), "wallet_key_file": root.join("wallet.key"),
        "agreement_key_file": root.join("agreement.key"), "signer_address": format!("0x{}", hex::encode(signer)),
        "signer_journal_root": state_root.join("signer-journal"), "transaction_key_file": root.join("transaction.key"),
        "artifact_manifest_file": root.join("manifest.json"), "artifact_manifest_sha256": hex::encode(Sha256::digest(&manifest)),
        "artifact_cache_root": state_root.join("artifacts"),
        "runtime_pins": {"pool": hex::encode(chain.pins.pool), "poseidon": hex::encode(chain.pins.poseidon),
            "deposit": hex::encode(chain.pins.deposit), "transfer": hex::encode(chain.pins.transfer),
            "withdraw": hex::encode(chain.pins.withdraw)},
        "allow_test_artifacts": true, "allow_loopback_artifact_http": true, "gas_limit": 2_000_000u64,
        "max_fee_per_gas": "1000000000", "max_priority_fee_per_gas": "100000000",
        "timeout_seconds": 30, "max_scan_blocks": 1000, "artifact_timeout_seconds": 120,
    })).unwrap());
    config
}

/// `fixture_manifest` is async only because it serves artifacts; drive it on this runtime.
fn futures_lite_block<F: std::future::Future>(future: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(future))
}

fn signer_of(config: &Path) -> String {
    let value: Value = serde_json::from_slice(&fs::read(config).unwrap()).unwrap();
    value["signer_address"].as_str().unwrap().to_owned()
}

fn tree_bytes(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut entries = Vec::new();
    if root.is_dir() {
        for entry in fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                entries.extend(tree_bytes(&path));
            } else if path
                .extension()
                .is_some_and(|extension| extension == "lock")
            {
                // Lock files are created by any attempt and carry no state.
                continue;
            } else {
                entries.push((path.clone(), fs::read(&path).unwrap()));
            }
        }
    }
    entries.sort();
    entries
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil, Node and generated M5 prototype artifacts; local WS1 qualification"]
async fn wallet_checkpoint_is_explicit_fresh_only_idempotent_and_never_signs() {
    let chain = history_chain("erc20", 2, 50).await;
    let primary = counting_proxy(chain.url.clone()).await;
    let peer = counting_proxy(chain.url.clone()).await;
    let wallet_bin = native_binary("erebus-wallet", env!("CARGO_BIN_EXE_erebus-wallet"));
    let run = |root: &Path, request: Value| finish(spawn(&wallet_bin, root, request));
    let checkpoint = |config: &Path, confirm: bool| json!({"method":"checkpoint","config_file":config,"confirm_fresh_wallet":confirm});

    // Opt-in only: without confirmation nothing is read, written or created.
    let root = tempfile::tempdir().unwrap();
    let config = checkpoint_config(&chain, root.path(), &primary.url, &peer.url, "erc20");
    let state = root.path().join("state");
    let refused = run(root.path(), checkpoint(&config, false));
    assert!(!refused.0 && refused.2 == 1, "{refused:?}");
    assert!(!state.join("history-checkpoint.json").exists());
    assert_eq!(primary.total() + peer.total(), 0);

    // Any history-bearing state refuses, and refusal leaves every byte unchanged.
    for (path, bytes) in [
        (state.join("wallet-index/primary.json"), b"{}".to_vec()),
        (state.join("wallet-operations/op.json"), b"{}".to_vec()),
        (
            state.join("signer-journal/eip155-31337-x.json"),
            b"{}".to_vec(),
        ),
        (state.join("coordinator/coordinator.json"), b"{}".to_vec()),
        (
            state.join("payment-observation/primary/x.json"),
            b"{}".to_vec(),
        ),
        (state.join("deposit-requests/x.json"), b"{}".to_vec()),
        (
            state.join("wallet/notes.enc"),
            b"not an authenticated wallet".to_vec(),
        ),
    ] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &bytes).unwrap();
        let before = tree_bytes(&state);
        let refused = run(root.path(), checkpoint(&config, true));
        assert!(!refused.0, "{} must refuse: {refused:?}", path.display());
        assert_eq!(
            tree_bytes(&state),
            before,
            "{} refusal changed state",
            path.display()
        );
        assert!(!state.join("history-checkpoint.json").exists());
        // Remove the whole store this case created, so later steps start from genuinely fresh state.
        let top = path
            .strip_prefix(&state)
            .unwrap()
            .components()
            .next()
            .unwrap();
        fs::remove_dir_all(state.join(top)).unwrap();
    }

    // A concurrent initialization is refused while the lock is held.
    let held = erebus_shielded_prover::checkpoint::InitLock::acquire(&state).unwrap();
    let busy = run(root.path(), checkpoint(&config, true));
    assert!(
        !busy.0 && busy.1.to_string().contains("in progress"),
        "{busy:?}"
    );
    drop(held);

    // A fresh participant: one paired acquisition, no signing, nonce claim or broadcast.
    chain.wait_finalized_past(chain.first_block + 1).await;
    primary.reset_counts(0);
    peer.reset_counts(0);
    let created = run(root.path(), checkpoint(&config, true));
    assert!(created.0, "{created:?}");
    let result = &created.1["result"];
    assert_eq!(result["checkpoint"], "created");
    assert_eq!(result["leaf_count"], 3);
    let block = result["checkpoint_block"].as_u64().unwrap();
    assert_eq!(result["history_from_block"], block + 1);
    assert_eq!(result["payment_verified"], false);
    for proxy in [&primary, &peer] {
        let counts = proxy.counts();
        assert!(
            counts.keys().all(
                |method| !method.starts_with("eth_send") && method != "eth_getTransactionCount"
            ),
            "checkpoint must not sign or allocate nonces: {counts:?}"
        );
        assert!(
            !counts.contains_key("eth_getLogs"),
            "checkpoint must not scan history: {counts:?}"
        );
    }
    assert!(
        !state.join("signer-journal").exists()
            || tree_bytes(&state.join("signer-journal")).is_empty()
    );
    let record = fs::read(state.join("history-checkpoint.json")).unwrap();

    // Repeating is idempotent and reads no provider.
    primary.reset_counts(0);
    peer.reset_counts(0);
    let repeated = run(root.path(), checkpoint(&config, true));
    assert!(
        repeated.0 && repeated.1["result"]["checkpoint"] == "existing",
        "{repeated:?}"
    );
    assert_eq!(primary.total() + peer.total(), 0);
    assert_eq!(
        fs::read(state.join("history-checkpoint.json")).unwrap(),
        record
    );

    // Status reports the coverage boundary instead of a deployment-era scan.
    let status = run(root.path(), json!({"method":"status","config_file":config}));
    assert!(status.0, "{status:?}");
    assert_eq!(status.1["result"]["history_scope"], "checkpoint");
    assert_eq!(status.1["result"]["history_from_block"], block + 1);

    // An existing full-history wallet can never be converted: its caches refuse the record.
    let old = tempfile::tempdir().unwrap();
    let old_config = checkpoint_config(&chain, old.path(), &primary.url, &peer.url, "erc20");
    let synced = sync_until(&wallet_bin, old.path(), &old_config, |value| {
        value["result"]["scanned_through"].is_u64()
    })
    .await;
    assert!(synced.0, "{synced:?}");
    let converted = run(old.path(), checkpoint(&old_config, true));
    assert!(!converted.0, "{converted:?}");
    // A record copied in from elsewhere makes the existing caches fail closed, never silently rescan.
    fs::copy(
        state.join("history-checkpoint.json"),
        old.path().join("state/history-checkpoint.json"),
    )
    .unwrap();
    let mismatch = run(
        old.path(),
        json!({"method":"sync","config_file":old_config}),
    );
    assert!(!mismatch.0, "{mismatch:?}");
    let _ = signer_of(&config);
}

fn note_states(value: &Value) -> Vec<(String, bool, bool)> {
    value["result"]["notes"]
        .as_array()
        .map_or_else(Vec::new, |notes| {
            notes
                .iter()
                .map(|note| {
                    (
                        note["commitment"].as_str().unwrap().to_owned(),
                        note["included"] == true,
                        note["consumed"] == true,
                    )
                })
                .collect()
        })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil, Node and generated M5 prototype artifacts; local WS1 end-to-end gate"]
async fn fresh_wallet_from_checkpoint_matches_full_history_through_spends_and_unwrap() {
    use erebus_shielded_prover::{checkpoint as record, index_store::IndexStore};
    const WRAPPED: u128 = 20_000_000_000_000_000;
    const NOTE: u128 = WRAPPED / 4;
    const INITIAL_NATIVE: u128 = 100_000_000_000_000_000_000;
    let history_deposits = std::env::var("WS1_HISTORY_DEPOSITS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10u32);
    let history_blocks = std::env::var("WS1_HISTORY_BLOCKS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3_000u32);
    let setup = Instant::now();
    let chain = history_chain("wmon", history_deposits, history_blocks).await;
    let history_setup = setup.elapsed();
    let primary = counting_proxy(chain.url.clone()).await;
    let peer = counting_proxy(chain.url.clone()).await;
    let wallet_bin = native_binary("erebus-wallet", env!("CARGO_BIN_EXE_erebus-wallet"));
    let root = tempfile::tempdir().unwrap();
    let config = checkpoint_config(&chain, root.path(), &primary.url, &peer.url, "wmon");
    let state = root.path().join("state");
    let signer = signer_of(&config);
    rpc(
        &chain.client,
        &chain.url,
        "anvil_setBalance",
        json!([signer, format!("0x{INITIAL_NATIVE:x}")]),
    )
    .await;
    let token = chain.deployment["token"].as_str().unwrap().to_owned();
    let request = |method: &str, extra: Value| {
        let mut value = json!({"method": method, "config_file": config});
        for (key, entry) in extra.as_object().unwrap() {
            value_insert(&mut value, key, entry.clone());
        }
        value
    };
    let run = |value: Value| finish(spawn(&wallet_bin, root.path(), value));
    let head =
        quantity(&rpc(&chain.client, &chain.url, "eth_blockNumber", json!([])).await["result"])
            .unwrap();
    chain.wait_finalized_past(head).await;

    // 1. Explicit checkpoint after all prior history.
    let started = Instant::now();
    let created = run(request("checkpoint", json!({"confirm_fresh_wallet": true})));
    let acquisition = started.elapsed();
    assert!(created.0, "{created:?}");
    let block = created.1["result"]["checkpoint_block"].as_u64().unwrap();
    assert!(block >= head);
    assert_eq!(created.1["result"]["leaf_count"], history_deposits + 1);
    let checkpoint = record::load(&state, chain.domain()).unwrap().unwrap();
    primary.reset_counts(block);
    peer.reset_counts(block);

    // 2. Fund after the checkpoint: wrap, approve, two deposits.
    let wrapped = run(request(
        "wrap",
        json!({"amount": WRAPPED.to_string(), "operation_ref": "81".repeat(32)}),
    ));
    assert!(wrapped.0, "{wrapped:?}");
    let first_sync = Instant::now();
    let observed = sync_until(&wallet_bin, root.path(), &config, |value| {
        operation_completed(value, "wrap")
    })
    .await;
    let first_sync = first_sync.elapsed();
    assert!(observed.0, "{observed:?}");
    let approved = run(request(
        "approve",
        json!({"amount": (2 * NOTE).to_string()}),
    ));
    assert!(approved.0, "{approved:?}");
    assert!(
        sync_until(&wallet_bin, root.path(), &config, |value| {
            operation_completed(value, "approve")
        })
        .await
        .0
    );
    let mut commitments = Vec::new();
    for _ in 0..2 {
        let deposited = run(request("deposit", json!({"amount": NOTE.to_string()})));
        assert!(deposited.0, "{deposited:?}");
        let commitment = deposited.1["result"]["commitment"]
            .as_str()
            .unwrap()
            .to_owned();
        let observed = sync_until(&wallet_bin, root.path(), &config, |value| {
            note_states(value)
                .iter()
                .any(|(note, included, _)| *note == commitment && *included)
        })
        .await;
        assert!(observed.0, "deposit not recovered: {observed:?}");
        commitments.push(commitment);
    }

    // 3. Spend from the checkpointed tree: a real withdraw proof against a post-checkpoint path.
    let started = Instant::now();
    let withdrawn = run(request(
        "withdraw",
        json!({"commitment": commitments[0], "recipient": signer}),
    ));
    let first_proof = started.elapsed();
    assert!(withdrawn.0, "{withdrawn:?}");
    assert!(
        sync_until(&wallet_bin, root.path(), &config, |value| {
            note_states(value)
                .iter()
                .any(|(note, _, consumed)| *note == commitments[0] && *consumed)
        })
        .await
        .0
    );
    // 4. Repeat spend: the second note, after the tree has grown past the first spend.
    let withdrawn = run(request(
        "withdraw",
        json!({"commitment": commitments[1], "recipient": signer}),
    ));
    assert!(withdrawn.0, "{withdrawn:?}");
    assert!(
        sync_until(&wallet_bin, root.path(), &config, |value| {
            note_states(value).iter().all(|(_, _, consumed)| *consumed)
                && note_states(value).len() == 2
        })
        .await
        .0
    );
    // 5. Exit: unwrap everything back to native MON.
    let unwrapped = run(request(
        "unwrap",
        json!({"amount": WRAPPED.to_string(), "operation_ref": "82".repeat(32)}),
    ));
    assert!(unwrapped.0, "{unwrapped:?}");
    let finished = sync_until(&wallet_bin, root.path(), &config, |value| {
        operation_completed(value, "unwrap")
    })
    .await;
    assert!(finished.0, "{finished:?}");
    assert_eq!(
        token_balance(&chain.client, &chain.url, &token, &signer).await,
        0
    );

    // 6. Old history was never scanned: no log range and no header at or below the checkpoint block,
    // except the deployment anchor (runtime authentication) and the checkpoint block itself.
    let mut checkpoint_counts = Vec::new();
    let mut anchor_headers = std::collections::BTreeSet::new();
    for proxy in [&primary, &peer] {
        let (methods, min_log, old_headers) = proxy.snapshot();
        assert!(
            min_log.is_none_or(|from| from > block),
            "logs scanned from {min_log:?} <= {block}"
        );
        // Runtime authentication proves code at the deployment block and none at its parent (DM8-8).
        let allowed = [chain.first_block - 1, chain.first_block, block];
        assert!(
            old_headers.iter().all(|height| allowed.contains(height)),
            "old headers read: {old_headers:?} (deployment block {})",
            chain.first_block
        );
        anchor_headers.extend(old_headers);
        checkpoint_counts.push(methods);
    }

    // 7. Restart: a fresh process resumes from the durable checkpointed caches.
    let started = Instant::now();
    let restarted = run(request("sync", json!({})));
    let restart = started.elapsed();
    assert!(restarted.0, "{restarted:?}");

    // 8. An equivalent full-history wallet reaches the same root, leaf count and membership paths.
    let full_primary = counting_proxy(chain.url.clone()).await;
    let full_peer = counting_proxy(chain.url.clone()).await;
    let full_root = tempfile::tempdir().unwrap();
    let full_config = checkpoint_config(
        &chain,
        full_root.path(),
        &full_primary.url,
        &full_peer.url,
        "wmon",
    );
    let started = Instant::now();
    let full_sync = sync_until(&wallet_bin, full_root.path(), &full_config, |value| {
        value["result"]["scanned_through"].is_u64()
    })
    .await;
    let full_duration = started.elapsed();
    assert!(full_sync.0, "{full_sync:?}");
    let (full_methods, full_min_log, _) = full_primary.snapshot();
    assert_eq!(full_min_log, Some(chain.first_block));
    let load = |state_root: &Path, checkpoint| {
        IndexStore::with_checkpoint(
            state_root.join("wallet-index/primary.json"),
            chain.domain(),
            checkpoint,
        )
        .unwrap()
        .load()
        .unwrap()
    };
    let mut fast = load(&state, Some(checkpoint));
    let mut full = load(&full_root.path().join("state"), None);
    let common = fast.tip().unwrap().number.min(full.tip().unwrap().number);
    fast.rewind_from(common + 1).unwrap();
    full.rewind_from(common + 1).unwrap();
    assert_eq!(fast.tip().unwrap().hash, full.tip().unwrap().hash);
    assert_eq!(fast.root(), full.root());
    assert_eq!(fast.leaf_count(), full.leaf_count());
    for leaf in fast.leaf_offset()..fast.leaf_count() {
        assert_eq!(
            fast.path(leaf).unwrap().siblings,
            full.path(leaf).unwrap().siblings,
            "leaf {leaf}"
        );
    }
    assert!(
        fast.path(fast.leaf_offset() - 1).is_err(),
        "pre-checkpoint leaves stay unavailable"
    );

    eprintln!(
        "WS1-MEASUREMENTS {}",
        json!({
            "history": {"deposits": history_deposits, "empty_blocks": history_blocks, "checkpoint_block": block,
                        "deployment_block": chain.first_block, "leaf_count_at_checkpoint": checkpoint.leaf_count},
            "setup_ms": history_setup.as_millis(),
            "checkpoint_acquisition_ms": acquisition.as_millis(),
            "first_sync_until_wrap_final_ms": first_sync.as_millis(),
            "first_post_checkpoint_withdraw_proof_ms": first_proof.as_millis(),
            "restart_sync_ms": restart.as_millis(),
            "full_history_first_sync_ms": full_duration.as_millis(),
            "headers_at_or_below_checkpoint": anchor_headers,
            "checkpoint_wallet_requests_primary": checkpoint_counts[0],
            "checkpoint_wallet_requests_peer": checkpoint_counts[1],
            "full_history_wallet_requests_primary_first_sync": full_methods,
        })
    );
}

// ---- WS6: read count and serial depth of a caught-up sync ---------------------------------------

/// One proxied request: its method and when it started and finished, relative to the proxy start.
#[derive(Clone, Debug)]
struct Timed {
    method: String,
    start: Duration,
    end: Duration,
}

/// Forwards to `upstream` after a fixed injected latency and records every request's span.
async fn latency_proxy(
    upstream: String,
    latency: Duration,
) -> (String, Arc<std::sync::Mutex<Vec<Timed>>>) {
    use axum::{extract::State as AxumState, routing::post as axum_post};
    #[derive(Clone)]
    struct LatencyState {
        upstream: String,
        client: reqwest::Client,
        latency: Duration,
        origin: Instant,
        spans: Arc<std::sync::Mutex<Vec<Timed>>>,
    }
    async fn forward(AxumState(state): AxumState<LatencyState>, body: String) -> String {
        let start = state.origin.elapsed();
        let method = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|request| request["method"].as_str().map(str::to_owned))
            .unwrap_or_else(|| "batch".to_owned());
        tokio::time::sleep(state.latency).await;
        let reply = state
            .client
            .post(&state.upstream)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        let end = state.origin.elapsed();
        state
            .spans
            .lock()
            .unwrap()
            .push(Timed { method, start, end });
        reply
    }
    let spans = Arc::new(std::sync::Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let state = LatencyState {
        upstream,
        client: reqwest::Client::new(),
        latency,
        origin: Instant::now(),
        spans: spans.clone(),
    };
    tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/", axum_post(forward))
                .with_state(state),
        )
        .await
        .unwrap();
    });
    (url, spans)
}

/// Longest chain of requests where each starts only after the previous one finished: the number
/// of injected latencies a run pays end to end, whatever its total read count.
fn serial_depth(spans: &[Timed]) -> usize {
    let mut spans = spans.to_vec();
    spans.sort_by_key(|span| span.start);
    let mut depth = vec![1usize; spans.len()];
    for current in 0..spans.len() {
        for earlier in 0..current {
            if spans[earlier].end <= spans[current].start {
                depth[current] = depth[current].max(depth[earlier] + 1);
            }
        }
    }
    depth.into_iter().max().unwrap_or(0)
}

/// A caught-up `erebus-wallet sync` through two providers with 25 ms injected latency each.
/// Every check still runs (deployment and dependency authentication, paired finalized recovery,
/// receipt reconciliation); the reads only overlap. Without a signer claim the paired
/// finalized-nonce reads are skipped; with a claim they run and release a consumed nonce.
///
/// Measured on this fixture (2026-10-22): before WS6 a caught-up sync without a claim made 145
/// reads at a serial depth of 110 (two of them nonce reads); after, 117 reads at depth 48. Set
/// `WS6_SYNC_REPORT=1` to print a run's numbers; `EREBUS_TEST_INSTALLED_BIN_DIR` measures another build.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil, Node and generated M5 prototype artifacts; local WS6 read budget"]
async fn caught_up_sync_has_a_bounded_read_count_and_serial_depth() {
    const LATENCY: Duration = Duration::from_millis(25);
    let chain = history_chain("erc20", 1, 20).await;
    let (primary, primary_spans) = latency_proxy(chain.url.clone(), LATENCY).await;
    let (peer, peer_spans) = latency_proxy(chain.url.clone(), LATENCY).await;
    let wallet_bin = native_binary("erebus-wallet", env!("CARGO_BIN_EXE_erebus-wallet"));
    let root = tempfile::tempdir().unwrap();
    let config = checkpoint_config(&chain, root.path(), &primary, &peer, "erc20");
    let run = |request: Value| finish(spawn(&wallet_bin, root.path(), request));
    let head =
        quantity(&rpc(&chain.client, &chain.url, "eth_blockNumber", json!([])).await["result"])
            .unwrap();
    chain.wait_finalized_past(head).await;
    let created =
        run(json!({"method":"checkpoint","config_file":config,"confirm_fresh_wallet":true}));
    assert!(created.0, "{created:?}");
    let caught_up = sync_until(&wallet_bin, root.path(), &config, |value| {
        value["result"]["nonce_released"] == true
    })
    .await;
    assert!(caught_up.0, "{caught_up:?}");

    let measure = |label: &str| {
        let spans: Vec<Timed> = primary_spans
            .lock()
            .unwrap()
            .drain(..)
            .chain(peer_spans.lock().unwrap().drain(..))
            .collect();
        let nonce_reads = spans
            .iter()
            .filter(|span| span.method == "eth_getTransactionCount")
            .count();
        let (reads, depth) = (spans.len(), serial_depth(&spans));
        if std::env::var_os("WS6_SYNC_REPORT").is_some() {
            let mut methods = std::collections::BTreeMap::<String, usize>::new();
            for span in &spans {
                *methods.entry(span.method.clone()).or_default() += 1;
            }
            eprintln!("WS6 caught-up sync {label}: reads={reads} serial_depth={depth} methods={methods:?}");
        }
        (reads, depth, nonce_reads)
    };
    measure("warm-up");

    // No claim: nothing to release, so no nonce reads at all.
    let started = Instant::now();
    let synced = run(json!({"method":"sync","config_file":config}));
    let elapsed = started.elapsed();
    assert!(
        synced.0 && synced.1["result"]["nonce_released"] == true,
        "{synced:?}"
    );
    let (reads, depth, nonce_reads) = measure("without claim");
    assert_eq!(
        nonce_reads, 0,
        "a sync without a claim must not read nonces"
    );
    assert!(reads <= 130, "caught-up sync made {reads} reads");
    assert!(depth <= 60, "caught-up sync serial depth {depth}");
    if std::env::var_os("WS6_SYNC_REPORT").is_some() {
        eprintln!("WS6 caught-up sync wall time {elapsed:?}");
    }

    // A claim on an already consumed nonce: the paired nonce reads run and release it.
    let signer = signer_of(&config);
    let set = rpc(
        &chain.client,
        &chain.url,
        "anvil_setNonce",
        json!([signer, "0x5"]),
    )
    .await;
    assert!(set.get("error").is_none(), "{set}");
    let latest =
        quantity(&rpc(&chain.client, &chain.url, "eth_blockNumber", json!([])).await["result"])
            .unwrap();
    chain.wait_finalized_past(latest + 1).await;
    let sender: [u8; 20] = hex::decode(signer.trim_start_matches("0x"))
        .unwrap()
        .try_into()
        .unwrap();
    let journal = erebus_evm::chain::SignerJournal::open(
        root.path().join("state/signer-journal"),
        31337,
        sender,
    )
    .unwrap();
    let pool = AddressBytes::new(chain.pool.to_vec()).unwrap();
    let domain = erebus_core::domain::DeploymentDomain {
        namespace: ChainNamespace::parse("eip155:31337").unwrap(),
        settlement_contract: Some(pool.clone()),
        pool: Some(pool),
        verifier_version: 2,
    };
    let deployment = erebus_evm::deployment::EvmDeployment::new(
        ChainNamespace::parse("eip155:31337").unwrap(),
        chain.pool,
        2,
        chain.url.clone(),
    )
    .unwrap();
    let stub = erebus_core::settlement::PreparedSettlement {
        operation_ref: [0x42; 32],
        deal_commitment: erebus_core::commitment::DealCommitment::from_bytes([0x42; 32]),
        deal_nullifier: erebus_core::commitment::DealNullifier::from_bytes([0x42; 32]),
        domain,
        mode: erebus_core::terms::SettlementMode::Shielded,
        required_guarantees: erebus_core::terms::GuaranteeSet::from_bits(
            erebus_core::shielded::SHIELDED_GUARANTEES,
        )
        .unwrap(),
        backend_evidence: vec![1],
    };
    journal
        .reserve_call_with_value(
            &deployment,
            &stub,
            chain.pool,
            &[1],
            0,
            2,
            erebus_evm::chain::Eip1559Fees::new(1_000_000_000, 100_000_000).unwrap(),
            3_000_000,
        )
        .unwrap();
    assert_eq!(journal.claim_nonce().unwrap(), Some(2));
    let released = run(json!({"method":"sync","config_file":config}));
    assert!(
        released.0 && released.1["result"]["nonce_released"] == true,
        "{released:?}"
    );
    let (_, _, nonce_reads) = measure("with claim");
    assert!(
        nonce_reads >= 2,
        "a claim must be checked at both providers"
    );
    assert_eq!(journal.claim_nonce().unwrap(), None);
}

/// Deposit gas across consecutive leaf indices: each insert writes a different set of
/// `filledSubtrees` slots, so the estimate moves with the index. Every deposit's estimate at its
/// parent block, plus the 5% margin, must fit under the deposit method's limit; the spread is
/// printed for the WS6 report.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil, Node and generated M5 prototype artifacts; local WS6 gas measurement"]
async fn deposit_estimates_across_consecutive_leaves_fit_the_method_limit() {
    use erebus_shielded_prover::gas_policy::DEPOSIT_GAS_LIMIT;
    const DEPOSITS: usize = 6;
    const NOTE: u128 = 1_000_000_000_000_000;
    let chain = history_chain("wmon", 1, 0).await;
    let (primary, _) = latency_proxy(chain.url.clone(), Duration::ZERO).await;
    let (peer, _) = latency_proxy(chain.url.clone(), Duration::ZERO).await;
    let wallet_bin = native_binary("erebus-wallet", env!("CARGO_BIN_EXE_erebus-wallet"));
    let root = tempfile::tempdir().unwrap();
    let config = checkpoint_config(&chain, root.path(), &primary, &peer, "wmon");
    let signer = signer_of(&config);
    rpc(
        &chain.client,
        &chain.url,
        "anvil_setBalance",
        json!([signer, "0x56bc75e2d63100000"]),
    )
    .await;
    let run = |request: Value| finish(spawn(&wallet_bin, root.path(), request));
    let total = (NOTE * DEPOSITS as u128).to_string();
    let call = |method: &str, extra: Value| {
        let mut value = json!({"method": method, "config_file": config});
        for (key, entry) in extra.as_object().unwrap() {
            value_insert(&mut value, key, entry.clone());
        }
        value
    };
    for (kind, reference) in [("wrap", "91"), ("approve", "92")] {
        let sent = run(call(
            kind,
            json!({"amount": total, "operation_ref": reference.repeat(32)}),
        ));
        assert!(sent.0, "{kind}: {sent:?}");
        let done = sync_until(&wallet_bin, root.path(), &config, |value| {
            operation_completed(value, kind)
        })
        .await;
        assert!(done.0, "{done:?}");
    }
    let mut estimates = Vec::new();
    for index in 0..DEPOSITS {
        let reference = format!("{:02x}", 0xa0 + index).repeat(32);
        let sent = run(call(
            "deposit",
            json!({"amount": NOTE.to_string(), "operation_ref": reference}),
        ));
        assert!(sent.0, "deposit {index}: {sent:?}");
        let commitment = sent.1["result"]["commitment"].as_str().unwrap().to_owned();
        let done = sync_until(&wallet_bin, root.path(), &config, |value| {
            operation_identity_completed(value, &commitment)
        })
        .await;
        assert!(done.0, "{done:?}");
        let operation = done.1["result"]["operations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|operation| operation["identity"] == commitment)
            .unwrap()
            .clone();
        let hash = format!(
            "0x{}",
            operation["attempts"][0]["tx_hash"].as_str().unwrap()
        );
        let tx = rpc(
            &chain.client,
            &chain.url,
            "eth_getTransactionByHash",
            json!([hash]),
        )
        .await;
        let receipt = rpc(
            &chain.client,
            &chain.url,
            "eth_getTransactionReceipt",
            json!([hash]),
        )
        .await;
        let block = quantity(&receipt["result"]["blockNumber"]).unwrap();
        let limit = quantity(&tx["result"]["gas"]).unwrap();
        assert_eq!(
            limit, DEPOSIT_GAS_LIMIT,
            "deposits sign at the deposit limit"
        );
        let estimate = rpc(
            &chain.client,
            &chain.url,
            "eth_estimateGas",
            json!([{"from": tx["result"]["from"], "to": tx["result"]["to"], "input": tx["result"]["input"]},
                format!("0x{:x}", block - 1)]),
        )
        .await;
        let estimate = quantity(&estimate["result"]).unwrap();
        let used = quantity(&receipt["result"]["gasUsed"]).unwrap();
        eprintln!(
            "WS6 deposit leaf {}: estimate at parent {estimate}, used {used}",
            index + 2
        );
        assert!(estimate * 105 <= DEPOSIT_GAS_LIMIT * 100, "{estimate}");
        estimates.push(estimate);
    }
    let (low, high) = (
        *estimates.iter().min().unwrap(),
        *estimates.iter().max().unwrap(),
    );
    eprintln!(
        "WS6 deposit estimate spread over {DEPOSITS} consecutive leaves: {low}..{high} ({:.2}%)",
        (high - low) as f64 * 100.0 / low as f64
    );
    assert!((high - low) * 10 <= low, "spread exceeds 10%");
}
