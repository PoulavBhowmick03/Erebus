//! Controlled paired-RPC regressions for the shielded payment CLI.
//!
//! Deterministic view pinning models a provider whose head/finalized sample happened just
//! before a block landed while the peer sampled just after (the intermittent installed
//! failure). A real same-height Anvil fork separately models genuine conflicting evidence.
//! Benign skew must stay retryable; conflicts must never release, reserve or duplicate a
//! payment. Nothing here accepts arbitrary exit codes: a hard `exit 1` from a benign skew
//! fails the regression.

use super::super::*;
use super::native_product;
use super::shielded_driver;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use std::process::Child;

const EVIDENCE_PREFIX: &str = "EREBUS_PAIRED";

struct AnvilProcess(Child);

impl Drop for AnvilProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[derive(Clone)]
enum Behavior {
    Pass,
    /// Present a coherent view frozen at `head`: `eth_blockNumber` answers `head` and the
    /// `finalized` tag is clamped to that block. Numeric reads below the pin pass through.
    Pin {
        head: u64,
    },
    /// Report a stale `finalized` anchor at `number` while the head stays live. Models a
    /// provider whose finality tip lags behind an already-observed, retained spend.
    PinFinalized {
        number: u64,
    },
    /// Answer the next `remaining` calls of one read method with HTTP 503 without forwarding
    /// them upstream. Models transient provider failures the production read retry must absorb.
    FailReads {
        method: &'static str,
        remaining: Arc<AtomicUsize>,
    },
    /// Sleep before forwarding the `calls`-th matching read (1-based). Models a slow provider
    /// whose history scan outlives a client timeout. The request is still forwarded afterwards.
    DelayReads {
        method: &'static str,
        calls: Arc<AtomicUsize>,
        delay_at: usize,
        delay_count: usize,
        milliseconds: u64,
    },
    /// Remove every `eth_getLogs` entry whose first topic is `topic` (0x-prefixed lowercase).
    /// Models a provider that omits one event while every header and state read stays honest.
    DropLogs {
        topic: String,
    },
}

#[derive(Clone)]
struct Controlled {
    upstream: Arc<Mutex<String>>,
    behavior: Arc<Mutex<Behavior>>,
    client: reqwest::Client,
    sends: Arc<AtomicUsize>,
    requests: Arc<AtomicUsize>,
    pins: Arc<AtomicUsize>,
    failed: Arc<AtomicUsize>,
    logs: Arc<AtomicUsize>,
}

struct ControlledProxy {
    url: String,
    state: Controlled,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ControlledProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn forward(State(proxy): State<Controlled>, Json(request): Json<Value>) -> Response {
    proxy.requests.fetch_add(1, Ordering::SeqCst);
    if request["method"] == "eth_getLogs" {
        proxy.logs.fetch_add(1, Ordering::SeqCst);
    }
    if request["method"] == "eth_sendRawTransaction" {
        proxy.sends.fetch_add(1, Ordering::SeqCst);
    }
    let behavior = proxy.behavior.lock().unwrap().clone();
    let upstream = proxy.upstream.lock().unwrap().clone();
    if let Behavior::FailReads { method, remaining } = &behavior {
        if request["method"].as_str() == Some(*method) {
            let left = remaining.load(Ordering::SeqCst);
            if left > 0 {
                remaining.store(left - 1, Ordering::SeqCst);
                proxy.failed.fetch_add(1, Ordering::SeqCst);
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "injected transient read failure",
                )
                    .into_response();
            }
        }
    }
    if let Behavior::DelayReads {
        method,
        calls,
        delay_at,
        delay_count,
        milliseconds,
    } = &behavior
    {
        if request["method"].as_str() == Some(*method) {
            let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call >= *delay_at && call - *delay_at < *delay_count {
                tokio::time::sleep(Duration::from_millis(*milliseconds)).await;
            }
        }
    }
    let mut response: Value = proxy
        .client
        .post(&upstream)
        .json(&request)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    if let Behavior::Pin { head } = behavior {
        if request["method"] == "eth_blockNumber" {
            proxy.pins.fetch_add(1, Ordering::SeqCst);
            response["result"] = json!(format!("0x{head:x}"));
        } else if request["method"] == "eth_getBlockByNumber"
            && request["params"][0].as_str() == Some("finalized")
            && response["result"]["number"]
                .as_str()
                .and_then(|text| u64::from_str_radix(text.trim_start_matches("0x"), 16).ok())
                .is_some_and(|number| number > head)
        {
            proxy.pins.fetch_add(1, Ordering::SeqCst);
            response = proxy
                .client
                .post(&upstream)
                .json(&json!({"jsonrpc":"2.0","id":request["id"],
                    "method":"eth_getBlockByNumber","params":[format!("0x{head:x}"), false]}))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
        }
    } else if let Behavior::DropLogs { topic } = &behavior {
        if request["method"] == "eth_getLogs" {
            if let Some(logs) = response["result"].as_array_mut() {
                logs.retain(|log| log["topics"][0].as_str() != Some(topic.as_str()));
            }
        }
    } else if let Behavior::PinFinalized { number } = behavior {
        if request["method"] == "eth_getBlockByNumber"
            && request["params"][0].as_str() == Some("finalized")
            && response["result"]["number"].as_str() != Some(format!("0x{number:x}").as_str())
        {
            proxy.pins.fetch_add(1, Ordering::SeqCst);
            response = proxy
                .client
                .post(&upstream)
                .json(&json!({"jsonrpc":"2.0","id":request["id"],
                    "method":"eth_getBlockByNumber","params":[format!("0x{number:x}"), false]}))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
        }
    }
    Json(response).into_response()
}

async fn start_proxy(upstream: String) -> ControlledProxy {
    let state = Controlled {
        upstream: Arc::new(Mutex::new(upstream)),
        behavior: Arc::new(Mutex::new(Behavior::Pass)),
        client: reqwest::Client::new(),
        sends: Arc::new(AtomicUsize::new(0)),
        requests: Arc::new(AtomicUsize::new(0)),
        pins: Arc::new(AtomicUsize::new(0)),
        failed: Arc::new(AtomicUsize::new(0)),
        logs: Arc::new(AtomicUsize::new(0)),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let served = state.clone();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/", post(forward)).with_state(served),
        )
        .await
        .unwrap();
    });
    ControlledProxy { url, state, task }
}

async fn rpc(client: &reqwest::Client, url: &str, method: &str, params: Value) -> Value {
    let response: Value = client
        .post(url)
        .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(response.get("error").is_none(), "{method}: {response}");
    response["result"].clone()
}

async fn mine(client: &reqwest::Client, url: &str, count: u64) {
    rpc(client, url, "anvil_mine", json!([format!("0x{count:x}")])).await;
}

async fn head(client: &reqwest::Client, url: &str) -> u64 {
    let value = rpc(client, url, "eth_blockNumber", json!([])).await;
    u64::from_str_radix(value.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
}

async fn head_timestamp(client: &reqwest::Client, url: &str, number: u64) -> u64 {
    let block = rpc(
        client,
        url,
        "eth_getBlockByNumber",
        json!([format!("0x{number:x}"), false]),
    )
    .await;
    u64::from_str_radix(
        block["timestamp"]
            .as_str()
            .unwrap()
            .trim_start_matches("0x"),
        16,
    )
    .unwrap()
}

fn arm(proxy: &ControlledProxy, behavior: Behavior) {
    *proxy.state.behavior.lock().unwrap() = behavior;
}

fn disarm(proxy: &ControlledProxy) {
    *proxy.state.behavior.lock().unwrap() = Behavior::Pass;
}

fn sends(proxy: &ControlledProxy) -> usize {
    proxy.state.sends.load(Ordering::SeqCst)
}

fn pins(proxy: &ControlledProxy) -> usize {
    proxy.state.pins.load(Ordering::SeqCst)
}

fn failures(proxy: &ControlledProxy) -> usize {
    proxy.state.failed.load(Ordering::SeqCst)
}

fn requests(proxy: &ControlledProxy) -> usize {
    proxy.state.requests.load(Ordering::SeqCst)
}

fn log_reads(proxy: &ControlledProxy) -> usize {
    proxy.state.logs.load(Ordering::SeqCst)
}

/// Runs the actual payment CLI without masking its exit code.
fn run_cli(binary: &Path, cwd: &Path, request: Value, timeout: Duration) -> (i32, Value, String) {
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
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "payment command exceeded {timeout:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().unwrap();
    let response = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
    (
        output.status.code().unwrap_or(-1),
        response,
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// A settle rerun is authorized only by present, explicit diagnostics proving no broadcast
/// was attempted. Absent fields are unknown state, never zero attempts.
fn proves_no_broadcast(response: &Value) -> bool {
    response.get("broadcast_attempts").and_then(Value::as_u64) == Some(0)
        && response.get("submitted_this_call").and_then(Value::as_bool) == Some(false)
        && response
            .get("retry_without_new_payment")
            .and_then(Value::as_bool)
            == Some(true)
}

#[test]
fn absent_broadcast_diagnostics_never_authorize_a_settle_retry() {
    assert!(!proves_no_broadcast(
        &json!({"status": "pending", "retry_without_new_payment": true})
    ));
    assert!(!proves_no_broadcast(&json!({"status": "pending",
        "submitted_this_call": false, "retry_without_new_payment": true})));
    assert!(!proves_no_broadcast(
        &json!({"status": "pending", "broadcast_attempts": 1,
        "submitted_this_call": false, "retry_without_new_payment": true})
    ));
    assert!(!proves_no_broadcast(
        &json!({"status": "pending", "broadcast_attempts": 0,
        "submitted_this_call": true, "retry_without_new_payment": true})
    ));
    assert!(!proves_no_broadcast(
        &json!({"status": "pending", "broadcast_attempts": null,
        "submitted_this_call": false, "retry_without_new_payment": true})
    ));
    assert!(proves_no_broadcast(
        &json!({"status": "pending", "broadcast_attempts": 0,
        "submitted_this_call": false, "retry_without_new_payment": true})
    ));
}

fn payment_request(config: &Path, method: &str) -> Value {
    json!({"method":method,"config_file":config,"operation_ref":hex::encode(OPERATION)})
}

fn write_evidence(name: &str, record: &Value) {
    eprintln!("{EVIDENCE_PREFIX} {name} {record}");
    if let Some(directory) = std::env::var_os("EREBUS_TEST_EVIDENCE_DIR") {
        let directory = PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join(format!("paired-{name}.json")),
            serde_json::to_vec_pretty(record).unwrap(),
        )
        .unwrap();
    }
}

fn redact(code: i32, response: &Value, proxy_sends: usize, note: (bool, bool)) -> Value {
    json!({
        "exit_code": code,
        "status": response["status"],
        "error": response["error"],
        "stage": response["stage"],
        "broadcast_attempts": response["broadcast_attempts"],
        "submitted_this_call": response["submitted_this_call"],
        "payment_verified": response["payment_verified"],
        "retry_without_new_payment": response["retry_without_new_payment"],
        "wallet_release_required": response["wallet_release_required"],
        "send_count": proxy_sends,
        "input_note_present": note.0,
        "input_note_reserved": note.1,
    })
}

struct Environment {
    deployed: shielded_driver::Deployed,
    binary: PathBuf,
    config: PathBuf,
    primary: ControlledProxy,
    peer: ControlledProxy,
    buyer: Value,
    seller: Value,
}

impl Environment {
    /// Deploys the funded fixture, negotiates the standard suite-2 deal, and points the
    /// payment config at two controlled proxies over the same Anvil.
    async fn new() -> Self {
        Self::new_with(|_| {}).await
    }

    /// Same, with one chance to adjust the negotiated service terms (e.g. a short deadline).
    async fn new_with(configure: impl FnOnce(&mut AgreementTerms)) -> Self {
        let deployed = shielded_driver::deploy_profile(false, None, configure).await;
        let (buyer, seller) = deployed.fixture.pair(false);
        let upstream = deployed.url.clone();
        let primary = start_proxy(upstream.clone()).await;
        let peer = start_proxy(upstream).await;
        let mut config = deployed.config.clone();
        config["rpc_url"] = json!(primary.url);
        config["peer_rpc_url"] = json!(peer.url);
        let config_path = deployed.buyer_root.join("paired-payment.json");
        private(&config_path, &serde_json::to_vec(&config).unwrap());
        let binary = native_binary("erebus-payment", env!("CARGO_BIN_EXE_erebus-payment"));
        Environment {
            deployed,
            binary,
            config: config_path,
            primary,
            peer,
            buyer,
            seller,
        }
    }

    /// The negotiated suite-2 evidence both participants hold.
    fn selected(&self) -> SelectedAgreement {
        SelectedAgreement::decode(
            &std::fs::read(self.seller["evidence_file"].as_str().unwrap()).unwrap(),
        )
        .unwrap()
    }

    /// Reservation state of the 150-unit seeded input before it is ever included: present in
    /// the encrypted wallet, and either reserved by the payment or not.
    fn input_note(&self) -> (bool, bool) {
        let snapshot = self.deployed.wallet.snapshot().unwrap();
        let note = snapshot
            .notes()
            .iter()
            .find(|note| note.asset() == self.deployed.token_bytes && note.amount() == 150);
        (
            note.is_some(),
            note.is_some_and(|note| note.reserved().is_some()),
        )
    }

    /// Persisted paired wallet-history prefix tip for `name` ("wallet-primary"/"wallet-peer").
    fn wallet_prefix_tip(&self, name: &str) -> u64 {
        let loaded = self.wallet_index(name).load().unwrap();
        let tip = loaded.tip().unwrap().number;
        tip
    }

    fn config_value(&self) -> Value {
        serde_json::from_slice(&std::fs::read(&self.config).unwrap()).unwrap()
    }

    fn pool(&self) -> [u8; 20] {
        hex::decode(
            self.config_value()["pool"]
                .as_str()
                .unwrap()
                .trim_start_matches("0x"),
        )
        .unwrap()
        .try_into()
        .unwrap()
    }

    fn index_domain(&self) -> erebus_shielded_prover::index_store::IndexDomain {
        let config = self.config_value();
        erebus_shielded_prover::index_store::IndexDomain {
            chain_id: 31337,
            pool: self.pool(),
            first_block: config["first_block"].as_u64().unwrap(),
            first_hash: hex::decode(
                config["first_hash"]
                    .as_str()
                    .unwrap()
                    .trim_start_matches("0x"),
            )
            .unwrap()
            .try_into()
            .unwrap(),
        }
    }

    /// The CLI's encrypted wallet, opened read/write by the test for direct catch-up calls.
    fn wallet_store(&self) -> WalletStore {
        let config = self.config_value();
        let key: [u8; 32] = std::fs::read(config["wallet_key_file"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        WalletStore::new(
            Path::new(config["wallet_file"].as_str().unwrap()),
            WalletDomain {
                chain_id: 31337,
                pool: self.pool(),
                verifier_version: 2,
            },
            key,
        )
        .unwrap()
    }

    /// The seller's encrypted wallet holding the received expected-payment note.
    fn seller_wallet_store(&self) -> WalletStore {
        let config: Value =
            serde_json::from_slice(&std::fs::read(&self.deployed.fixture.seller_config).unwrap())
                .unwrap();
        let key: [u8; 32] = std::fs::read(config["seller_wallet_key_file"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        WalletStore::new(
            Path::new(config["seller_wallet_file"].as_str().unwrap()),
            WalletDomain {
                chain_id: 31337,
                pool: self.pool(),
                verifier_version: 2,
            },
            key,
        )
        .unwrap()
    }

    fn wallet_index(&self, name: &str) -> erebus_shielded_prover::index_store::IndexStore {
        let config = self.config_value();
        erebus_shielded_prover::index_store::IndexStore::new(
            PathBuf::from(config["state_root"].as_str().unwrap()).join(match name {
                // The buyer's payment recovery shares the wallet's finalized-only caches.
                "wallet-primary" => "wallet-index/primary.json".to_owned(),
                "wallet-peer" => "wallet-index/peer.json".to_owned(),
                other => format!("payment-observation/{other}.json"),
            }),
            self.index_domain(),
        )
        .unwrap()
    }

    fn pool_rpc(&self, url: &str) -> erebus_shielded_prover::rpc::PoolRpc {
        erebus_shielded_prover::rpc::PoolRpc::with_timeout(
            url,
            31337,
            self.pool(),
            Duration::from_secs(30),
        )
        .unwrap()
    }

    fn rpc_urls(&self) -> (String, String) {
        let config = self.config_value();
        (
            config["rpc_url"].as_str().unwrap().to_owned(),
            config["peer_rpc_url"].as_str().unwrap().to_owned(),
        )
    }

    /// Persisted service-side public history tip (`pool-history`/`pool-peer-history`), or the
    /// configured first block when the service has not synced yet. Missing progress is never
    /// treated as absence: callers compare tips and require a present file after a scan.
    fn service_history_tip(&self, state_root: &Path, name: &str) -> Option<u64> {
        let path = state_root.join(name);
        if !path.exists() {
            return None;
        }
        let index = erebus_shielded_prover::index_store::IndexStore::new(path, self.index_domain())
            .unwrap();
        index.load().unwrap().tip().map(|tip| tip.number)
    }

    /// Signs one snapshot access request exactly as the funded access fixture does.
    fn access_request(
        &self,
        selected: &SelectedAgreement,
        service_id: [u8; 32],
        nonce: [u8; 32],
        expires_at: u64,
    ) -> erebus_shielded_prover::access::AccessRequest {
        let digest = erebus_shielded_prover::access::request_digest(
            selected, service_id, nonce, expires_at, None,
        )
        .unwrap();
        erebus_shielded_prover::access::AccessRequest {
            deal_commitment: self.buyer["deal_commitment"].as_str().unwrap().to_owned(),
            nonce,
            expires_at,
            signature: erebus_core::shielded_auth::sign_message(&[61; 32], &digest)
                .unwrap()
                .1
                .to_vec(),
            payment: None,
        }
    }

    /// Writes the funded seller's snapshot service configuration and evidence root.
    fn write_access_service_config(
        &self,
        selected: &SelectedAgreement,
        directory: &Path,
        port: u16,
    ) -> PathBuf {
        let agreements = directory.join("access-evidence");
        std::fs::create_dir_all(&agreements).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&agreements, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let agreement = verify_selected_agreement(selected).unwrap();
        private(
            &agreements.join(format!("{}.evidence", agreement.commitment.to_hex())),
            &selected.encode().unwrap(),
        );
        let payload = directory.join("payload");
        private(&payload, native_product::PAYLOAD);
        let config = self.config_value();
        let (rpc_url, peer_rpc_url) = self.rpc_urls();
        let service_id = [42u8; 32];
        let service_config = directory.join("access.json");
        private(
            &service_config,
            &serde_json::to_vec(&json!({
                "service_id": service_id,
                "seller_key": selected.terms.seller_authorization_key.as_bytes(),
                "suite_id": 2,
                "resource": selected.terms.service.resource,
                "payload_file": payload,
                "evidence_root": agreements,
                "state_root": directory.join("access-state"),
                "port": port,
                "backend": {
                    "mode": "shielded",
                    "chain_id": 31337,
                    "pool": config["pool"],
                    "rpc_url": rpc_url,
                    "peer_rpc_url": peer_rpc_url,
                    "first_block": config["first_block"],
                    "first_hash": hex::decode(
                        config["first_hash"]
                            .as_str()
                            .unwrap()
                            .trim_start_matches("0x"),
                    )
                    .unwrap(),
                    "runtime_keccak256": config["runtime_keccak256"],
                    "poseidon_keccak256": config["poseidon_keccak256"],
                    "deposit_verifier_keccak256": config["deposit_verifier_keccak256"],
                    "transfer_verifier_keccak256": config["transfer_verifier_keccak256"],
                    "withdraw_verifier_keccak256": config["withdraw_verifier_keccak256"],
                },
            }))
            .unwrap(),
        );
        service_config
    }

    /// Starts the real access service over the controlled proxies and waits for health.
    async fn start_access_service(
        &self,
        directory: &Path,
    ) -> (super::ServiceProcess, String, PathBuf, u16) {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let config = self.write_access_service_config(&self.selected(), directory, port);
        let url = format!("http://127.0.0.1:{port}");
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let service = native_product::service(&config, &url, &client).await;
        (service, url, config, port)
    }

    fn root(&self) -> &Path {
        self.deployed.fixture.root.path()
    }

    fn disclosure_binary(&self) -> PathBuf {
        native_binary(
            "erebus-shielded-disclosure",
            env!("CARGO_BIN_EXE_erebus-shielded-disclosure"),
        )
    }

    /// Independent auditor deployment configuration with the reviewed runtime pins.
    fn auditor_deployment(&self, cache_root: &Path) -> Value {
        let config = self.config_value();
        let (rpc_url, peer_rpc_url) = self.rpc_urls();
        json!({
            "namespace": config["namespace"],
            "settlement_contract": config["pool"],
            "verifier_version": 2,
            "rpc_url": rpc_url,
            "peer_rpc_url": peer_rpc_url,
            "first_block": config["first_block"],
            "first_hash": config["first_hash"],
            "cache_root": cache_root,
            "runtime_keccak256": config["runtime_keccak256"],
            "poseidon_keccak256": config["poseidon_keccak256"],
            "deposit_verifier_keccak256": config["deposit_verifier_keccak256"],
            "transfer_verifier_keccak256": config["transfer_verifier_keccak256"],
            "withdraw_verifier_keccak256": config["withdraw_verifier_keccak256"],
        })
    }

    /// One auditor keygen; returns the recipient public key.
    fn auditor_keygen(&self, key_file: &Path) -> Value {
        let (ok, response) = finish(spawn(
            &self.disclosure_binary(),
            self.root(),
            json!({"method": "keygen", "key_file": key_file}),
        ));
        assert!(ok, "auditor keygen: {response}");
        response
    }

    /// One participant export; the caller checks refusal cases.
    fn auditor_export(
        &self,
        recipient_public_key: &Value,
        grant_file: &Path,
        expires_at: u64,
    ) -> (bool, Value) {
        finish(spawn(
            &self.disclosure_binary(),
            self.root(),
            json!({"method": "export", "evidence_file": self.seller["evidence_file"],
                "issuer_key_file": self.root().join("seller/agreement.key"),
                "recipient_public_key": recipient_public_key, "grant_file": grant_file,
                "expires_at": expires_at}),
        ))
    }

    /// One auditor verification through a fresh process. `ok` is the exit status; `pending`
    /// responses report their progress in the returned value.
    fn auditor_verify(
        &self,
        grant_file: &Path,
        key_file: &Path,
        deployment: &Value,
    ) -> (bool, Value) {
        finish_with_timeout(
            spawn(
                &self.disclosure_binary(),
                self.root(),
                json!({"method": "verify_payment", "grant_file": grant_file, "key_file": key_file,
                    "expected_issuer": format!("0x{}", hex::encode(
                        self.selected().terms.seller_authorization_key.as_bytes())),
                    "deployment": deployment}),
            ),
            Duration::from_secs(120),
        )
    }

    /// Persisted auditor public-history tip (`primary`/`peer`), absent until a scan saves one.
    fn auditor_cache_tip(&self, cache_root: &Path, name: &str) -> Option<u64> {
        let path = cache_root.join(name).join("index.json");
        if !path.exists() {
            return None;
        }
        let index = erebus_shielded_prover::index_store::IndexStore::new(path, self.index_domain())
            .unwrap();
        index.load().unwrap().tip().map(|tip| tip.number)
    }

    /// One auditor `checkpoint` request through a fresh process, with its exit code.
    fn auditor_checkpoint(&self, deployment: &Value, confirm: bool) -> (i32, Value) {
        let (code, response, stderr) = run_cli(
            &self.disclosure_binary(),
            self.root(),
            json!({"method": "checkpoint", "deployment": deployment,
                "confirm_new_cache": confirm}),
            Duration::from_secs(120),
        );
        assert!(stderr.is_empty(), "{stderr}");
        (code, response)
    }

    /// One auditor verification with its exit code (0 verified, 2 pending, 3 settled before
    /// the cache's checkpoint, 1 error).
    fn auditor_verify_code(
        &self,
        grant_file: &Path,
        key_file: &Path,
        deployment: &Value,
    ) -> (i32, Value) {
        let (code, response, stderr) = run_cli(
            &self.disclosure_binary(),
            self.root(),
            json!({"method": "verify_payment", "grant_file": grant_file, "key_file": key_file,
                "expected_issuer": format!("0x{}", hex::encode(
                    self.selected().terms.seller_authorization_key.as_bytes())),
                "deployment": deployment}),
            Duration::from_secs(120),
        );
        assert!(stderr.is_empty(), "{stderr}");
        (code, response)
    }

    /// Repeats a verification while its bounded history scan is pending; returns the first reply
    /// that is not scan progress. An unpaid deal stays pending without progress fields.
    fn auditor_verify_settled(
        &self,
        grant_file: &Path,
        key_file: &Path,
        deployment: &Value,
    ) -> (i32, Value) {
        for _ in 0..12 {
            let (code, response) = self.auditor_verify_code(grant_file, key_file, deployment);
            if code != 2 || response.get("next_log_block").is_none() {
                return (code, response);
            }
            assert_eq!(response["status"], "pending", "{response}");
            assert_ne!(response["payment_verified"], true, "{response}");
        }
        panic!("auditor verification stayed pending");
    }

    fn runtime_pins(&self) -> erebus_shielded_prover::rpc::PoolRuntimePins {
        let config = self.config_value();
        let digest = |key: &str| -> [u8; 32] {
            hex::decode(config[key].as_str().unwrap().trim_start_matches("0x"))
                .unwrap()
                .try_into()
                .unwrap()
        };
        erebus_shielded_prover::rpc::PoolRuntimePins {
            pool: digest("runtime_keccak256"),
            poseidon: digest("poseidon_keccak256"),
            deposit: digest("deposit_verifier_keccak256"),
            transfer: digest("transfer_verifier_keccak256"),
            withdraw: digest("withdraw_verifier_keccak256"),
        }
    }

    /// The library verification over an auditor cache root exactly as the command opens it
    /// (with its checkpoint record, if any), repeated while the bounded scan is pending.
    async fn auditor_certificate(
        &self,
        cache_root: &Path,
    ) -> Result<
        erebus_shielded_prover::disclosure::PaymentCertificate,
        erebus_shielded_prover::disclosure::ShieldedDisclosureError,
    > {
        use erebus_shielded_prover::{
            disclosure::ShieldedDisclosureError, index_store::IndexStore,
            observation::ObservationError, recovery::RecoveryError,
        };
        let recorded =
            erebus_shielded_prover::checkpoint::load(cache_root, self.index_domain()).unwrap();
        let open = |name: &str| {
            IndexStore::with_checkpoint(
                cache_root.join(name).join("index.json"),
                self.index_domain(),
                recorded,
            )
            .unwrap()
        };
        let (primary_url, peer_url) = self.rpc_urls();
        for _ in 0..12 {
            match erebus_shielded_prover::disclosure::verify_shielded_payment_certified(
                self.selected(),
                &self.pool_rpc(&primary_url),
                &open("primary"),
                &self.pool_rpc(&peer_url),
                &open("peer"),
                self.runtime_pins(),
            )
            .await
            {
                Ok(certified) => return Ok(certified.certificate),
                Err(ShieldedDisclosureError::Observation(ObservationError::Recovery(
                    RecoveryError::HistoryPending { .. },
                ))) => continue,
                Err(error) => return Err(error),
            }
        }
        panic!("auditor library verification stayed pending");
    }

    /// Persisted auditor cache bytes (`primary`/`peer`), for version and first-block checks.
    fn auditor_cache_json(&self, cache_root: &Path, name: &str) -> Value {
        serde_json::from_slice(&fs::read(cache_root.join(name).join("index.json")).unwrap())
            .unwrap()
    }

    async fn settle(&self, timeout: Duration) -> (i32, Value, String) {
        run_cli(
            &self.binary,
            self.root(),
            payment_request(&self.config, "settle"),
            timeout,
        )
    }

    async fn observe(&self, timeout: Duration) -> (i32, Value, String) {
        run_cli(
            &self.binary,
            self.root(),
            payment_request(&self.config, "observe"),
            timeout,
        )
    }

    /// Completes the payment after the controlled condition is cleared. A settle rerun is
    /// allowed only when durable response diagnostics explicitly prove no broadcast attempt.
    async fn complete(&self, first: &Value) -> Value {
        assert_eq!(first["status"], "pending", "{first}");
        assert!(
            proves_no_broadcast(first),
            "absent or nonzero broadcast diagnostics cannot authorize a settle retry: {first}"
        );
        let (present, reserved) = self.input_note();
        assert!(
            present && !reserved,
            "settle rerun requires an unreserved input"
        );
        let (code, response, stderr) = self.settle(Duration::from_secs(240)).await;
        assert!(stderr.is_empty(), "{stderr}");
        assert!(
            code == 0 || (code == 2 && response["status"] == "pending"),
            "unexpected settle result: {response}"
        );
        assert_eq!(response["submitted_this_call"], true, "{response}");
        assert_eq!(sends(&self.primary), 1, "{response}");
        let client = self.deployed.client.clone();
        mine(&client, &self.deployed.url, 4).await;
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let (code, observed, stderr) = self.observe(Duration::from_secs(60)).await;
            assert!(stderr.is_empty(), "{stderr}");
            if code == 0 && observed["payment_verified"] == true {
                let (code, repeat, _) = self.observe(Duration::from_secs(60)).await;
                assert_eq!(code, 0, "{repeat}");
                assert_eq!(repeat["payment_verified"], true, "{repeat}");
                assert_eq!(repeat["submitted_this_call"], false, "{repeat}");
                assert_eq!(sends(&self.primary), 1, "{repeat}");
                return json!({"observe_exit": code,
                    "stage": repeat["stage"],
                    "payment_verified": repeat["payment_verified"],
                    "submitted_this_call": repeat["submitted_this_call"],
                    "total_sends": sends(&self.primary),
                    "transaction_hash": repeat["transaction_hash"]});
            }
            assert_eq!(code, 2, "unexpected observe failure: {observed}");
            assert_eq!(observed["status"], "pending", "{observed}");
            assert!(Instant::now() < deadline, "{observed}");
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

/// A provider whose sample predates the block that landed while its peer sampled after it.
/// The skew is benign; the current candidate turns it into a fatal payment error.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil and generated M5 prototype artifacts; deterministic paired-skew reproducer"]
async fn chain_advance_between_paired_reads_must_not_fail_the_payment() {
    let environment = Environment::new().await;
    let client = environment.deployed.client.clone();
    let pinned = head(&client, &environment.deployed.url).await;
    arm(&environment.primary, Behavior::Pin { head: pinned });
    mine(&client, &environment.deployed.url, 1).await;
    assert_eq!(
        head(&client, &environment.primary.url).await,
        pinned,
        "pinned view must answer the pre-advance head"
    );
    assert_eq!(
        head(&client, &environment.deployed.url).await,
        pinned + 1,
        "the chain must have advanced between the paired samples"
    );
    let (code, response, stderr) = environment.settle(Duration::from_secs(240)).await;
    assert!(stderr.is_empty(), "{stderr}");
    assert!(
        pins(&environment.primary) >= 1,
        "the pinned provider view was never exercised"
    );
    let sends = sends(&environment.primary);
    let note = environment.input_note();
    let record = redact(code, &response, sends, note);
    write_evidence("advance", &record);
    let broadcast = code == 2
        && response["status"] == "pending"
        && response["submitted_this_call"] == true
        && response["broadcast_attempts"].as_u64() == Some(1);
    let retryable = code == 2
        && response["status"] == "pending"
        && response["submitted_this_call"] == false
        && response["retry_without_new_payment"] == true;
    assert!(
        broadcast || retryable,
        "benign anchor skew must stay retryable, not fail closed as an error: {response}"
    );
    if broadcast {
        assert_eq!(sends, 1, "{response}");
        assert!(
            note.0 && note.1,
            "input must be durably reserved after broadcast"
        );
    } else {
        assert_eq!(sends, 0, "{response}");
        assert!(note.0 && !note.1, "no reservation before any broadcast");
    }
    disarm(&environment.primary);
    if retryable {
        let recovery = environment.complete(&response).await;
        write_evidence("advance-recovery", &recovery);
    } else {
        let client = environment.deployed.client.clone();
        mine(&client, &environment.deployed.url, 4).await;
        let recovery = environment.complete_after_broadcast().await;
        write_evidence("advance-recovery", &recovery);
    }
}

/// An honest provider that is one block behind its peer. Nothing conflicts.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil and generated M5 prototype artifacts; deterministic provider-lag reproducer"]
async fn honest_provider_lag_must_not_fail_the_payment() {
    let environment = Environment::new().await;
    let client = environment.deployed.client.clone();
    let pinned = head(&client, &environment.deployed.url).await;
    arm(&environment.peer, Behavior::Pin { head: pinned });
    mine(&client, &environment.deployed.url, 1).await;
    assert_eq!(
        head(&client, &environment.peer.url).await,
        pinned,
        "the lagging view must answer the pre-advance head"
    );
    assert_eq!(
        head(&client, &environment.deployed.url).await,
        pinned + 1,
        "the observed chain must have advanced past the lagging view"
    );
    let (code, response, stderr) = environment.settle(Duration::from_secs(240)).await;
    assert!(stderr.is_empty(), "{stderr}");
    assert!(
        pins(&environment.peer) >= 1,
        "the lagging provider view was never exercised"
    );
    let sends = sends(&environment.primary);
    let note = environment.input_note();
    write_evidence("lag", &redact(code, &response, sends, note));
    let broadcast = code == 2
        && response["status"] == "pending"
        && response["submitted_this_call"] == true
        && response["broadcast_attempts"].as_u64() == Some(1);
    let retryable = code == 2
        && response["status"] == "pending"
        && response["submitted_this_call"] == false
        && response["retry_without_new_payment"] == true;
    assert!(
        broadcast || retryable,
        "an honest one-block lag must stay retryable, not fail closed as an error: {response}"
    );
    if broadcast {
        assert_eq!(sends, 1, "{response}");
        assert!(
            note.0 && note.1,
            "input must be durably reserved after broadcast"
        );
    } else {
        assert_eq!(sends, 0, "{response}");
        assert!(note.0 && !note.1, "no reservation before any broadcast");
    }
    disarm(&environment.peer);
    if retryable {
        let recovery = environment.complete(&response).await;
        write_evidence("lag-recovery", &recovery);
    } else {
        let client = environment.deployed.client.clone();
        mine(&client, &environment.deployed.url, 4).await;
        let recovery = environment.complete_after_broadcast().await;
        write_evidence("lag-recovery", &recovery);
    }
}

impl Environment {
    /// Observes a payment that was already broadcast by the controlled settle.
    async fn complete_after_broadcast(&self) -> Value {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let (code, observed, stderr) = self.observe(Duration::from_secs(60)).await;
            assert!(stderr.is_empty(), "{stderr}");
            if code == 0 && observed["payment_verified"] == true {
                let (code, repeat, _) = self.observe(Duration::from_secs(60)).await;
                assert_eq!(code, 0, "{repeat}");
                assert_eq!(repeat["payment_verified"], true, "{repeat}");
                assert_eq!(repeat["submitted_this_call"], false, "{repeat}");
                assert_eq!(sends(&self.primary), 1, "{repeat}");
                return json!({"observe_exit": code,
                    "stage": repeat["stage"],
                    "payment_verified": repeat["payment_verified"],
                    "submitted_this_call": repeat["submitted_this_call"],
                    "total_sends": sends(&self.primary),
                    "transaction_hash": repeat["transaction_hash"]});
            }
            assert_eq!(code, 2, "unexpected observe failure: {observed}");
            assert_eq!(observed["status"], "pending", "{observed}");
            assert!(Instant::now() < deadline, "{observed}");
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

/// Two chains that share every block through the deployment and then diverge at the same
/// height. This is genuine conflicting evidence, not load: it must never release or
/// duplicate a payment, and recovery must still produce exactly one payment.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil and generated M5 prototype artifacts; real same-height fork conflict"]
async fn genuine_conflicting_paired_evidence_fails_closed_and_recovers_once() {
    let environment = Environment::new().await;
    let client = environment.deployed.client.clone();
    let root_anvil = environment.deployed.url.clone();
    let pinned = head(&client, &root_anvil).await;
    let timestamp = head_timestamp(&client, &root_anvil, pinned).await;
    let state = rpc(&client, &root_anvil, "anvil_dumpState", json!([]))
        .await
        .as_str()
        .unwrap()
        .to_owned();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let fork_url = format!("http://127.0.0.1:{port}");
    let fork = AnvilProcess(
        Command::new("anvil")
            .args([
                "--port",
                &port.to_string(),
                "--chain-id",
                "31337",
                "--slots-in-an-epoch",
                "1",
                "--silent",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    for _ in 0..100 {
        if client
            .post(&fork_url)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}))
            .send()
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert_eq!(
        rpc(&client, &fork_url, "anvil_loadState", json!([state])).await,
        true
    );
    // Both chains advance one height with different timestamps: same history through the
    // deployment block, genuinely divergent canonical hashes at the settlement height.
    rpc(
        &client,
        &root_anvil,
        "evm_setNextBlockTimestamp",
        json!([format!("0x{:x}", timestamp + 10)]),
    )
    .await;
    rpc(
        &client,
        &fork_url,
        "evm_setNextBlockTimestamp",
        json!([format!("0x{:x}", timestamp + 20)]),
    )
    .await;
    mine(&client, &root_anvil, 1).await;
    mine(&client, &fork_url, 1).await;
    *environment.peer.state.upstream.lock().unwrap() = fork_url.clone();
    let primary_hash = rpc(
        &client,
        &root_anvil,
        "eth_getBlockByNumber",
        json!([format!("0x{:x}", pinned + 1), false]),
    )
    .await;
    let peer_hash = rpc(
        &client,
        &fork_url,
        "eth_getBlockByNumber",
        json!([format!("0x{:x}", pinned + 1), false]),
    )
    .await;
    assert_ne!(
        primary_hash["hash"], peer_hash["hash"],
        "fork did not diverge"
    );
    let (code, response, stderr) = environment.settle(Duration::from_secs(240)).await;
    assert!(stderr.is_empty(), "{stderr}");
    assert_ne!(
        code, 0,
        "conflicting chains must not report success: {response}"
    );
    assert_ne!(response["payment_verified"], true, "{response}");
    assert_ne!(response["submitted_this_call"], true, "{response}");
    assert_eq!(response["retry_without_new_payment"], true, "{response}");
    assert_eq!(response["status"], "error", "{response}");
    assert!(
        [
            "paired deployment authentication failed",
            "paired pool evidence unavailable; retain reservations",
        ]
        .contains(&response["error"].as_str().unwrap_or_default()),
        "unexpected conflict outcome: {response}"
    );
    assert_eq!(
        sends(&environment.primary),
        0,
        "no broadcast under conflict"
    );
    let note = environment.input_note();
    assert!(note.0 && !note.1, "no reservation under conflict");
    write_evidence(
        "conflict",
        &redact(code, &response, sends(&environment.primary), note),
    );
    // Recovering onto one consistent chain must still produce exactly one payment.
    *environment.peer.state.upstream.lock().unwrap() = root_anvil.clone();
    drop(fork);
    let (code, response, stderr) = environment.settle(Duration::from_secs(240)).await;
    assert!(stderr.is_empty(), "{stderr}");
    assert!(
        code == 0 || (code == 2 && response["status"] == "pending"),
        "unexpected recovery result: {response}"
    );
    assert_eq!(response["submitted_this_call"], true, "{response}");
    assert_eq!(sends(&environment.primary), 1, "{response}");
    mine(&client, &root_anvil, 4).await;
    let recovery = environment.complete_after_broadcast().await;
    write_evidence("conflict-recovery", &recovery);
}

/// A provider whose finalized tip lags strictly below an already-observed spend must not
/// rewind the retained wallet prefix, revive the spent note, or allow a second payment.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil and generated M5 prototype artifacts; lagging-finality CLI regression"]
async fn lagging_finality_cannot_revive_a_retained_spent_note() {
    let environment = Environment::new().await;
    let client = environment.deployed.client.clone();
    let (code, response, stderr) = environment.settle(Duration::from_secs(240)).await;
    assert!(stderr.is_empty(), "{stderr}");
    assert!(
        code == 2 && response["status"] == "pending",
        "first settle: {response}"
    );
    assert_eq!(response["submitted_this_call"], true, "{response}");
    assert_eq!(sends(&environment.primary), 1, "{response}");
    mine(&client, &environment.deployed.url, 4).await;
    let verified = environment.complete_after_broadcast().await;
    assert_eq!(verified["payment_verified"], true, "{verified}");

    // The seeded 150-unit input is spent and retained in the encrypted wallet.
    let snapshot = environment.deployed.wallet.snapshot().unwrap();
    let spent = snapshot
        .notes()
        .iter()
        .find(|note| note.asset() == environment.deployed.token_bytes && note.amount() == 150)
        .expect("retained spent note")
        .commitment();
    assert!(
        snapshot.spendable(&spent).is_none(),
        "spent note must not be spendable"
    );
    drop(snapshot);

    // One provider's finalized tip now reports a block strictly below the retained prefix.
    let retained_tip = environment.wallet_prefix_tip("wallet-primary");
    assert!(retained_tip > 1, "wallet prefix too low");
    arm(
        &environment.peer,
        Behavior::PinFinalized {
            number: retained_tip - 1,
        },
    );
    let pinned_finalized = rpc(
        &client,
        &environment.peer.url,
        "eth_getBlockByNumber",
        json!(["finalized", false]),
    )
    .await;
    assert_eq!(
        u64::from_str_radix(
            pinned_finalized["number"]
                .as_str()
                .unwrap()
                .trim_start_matches("0x"),
            16
        )
        .unwrap(),
        retained_tip - 1,
        "finalized pin did not engage: {pinned_finalized}"
    );
    assert!(
        pins(&environment.peer) >= 1,
        "the lagging finalized view was never exercised"
    );
    let pinned_before_observe = pins(&environment.peer);
    let (code, response, stderr) = environment.observe(Duration::from_secs(60)).await;
    assert!(stderr.is_empty(), "{stderr}");
    assert_eq!(code, 1, "lagging finality must fail closed: {response}");
    assert_eq!(response["status"], "error", "{response}");
    assert_eq!(
        response["error"], "paired finalized wallet recovery failed; retain reservations",
        "{response}"
    );
    assert_ne!(response["payment_verified"], true, "{response}");
    assert_ne!(response["submitted_this_call"], true, "{response}");
    assert_eq!(
        sends(&environment.primary),
        1,
        "no resend under lagging finality"
    );
    assert!(
        pins(&environment.peer) > pinned_before_observe,
        "the failed observation never read the lagging finalized view"
    );
    let snapshot = environment.deployed.wallet.snapshot().unwrap();
    assert!(
        snapshot.spendable(&spent).is_none(),
        "lagging finality revived the spent note"
    );
    assert!(
        snapshot
            .notes()
            .iter()
            .find(|note| note.commitment() == spent)
            .is_some_and(|note| note.consumption().is_some()),
        "lagging finality dropped the consumption record"
    );
    drop(snapshot);
    write_evidence(
        "lagging-finality",
        &redact(
            code,
            &response,
            sends(&environment.primary),
            environment.input_note(),
        ),
    );

    // Finality catches up: the same spend is re-observed and paid exactly once in total.
    disarm(&environment.peer);
    let verified = environment.complete_after_broadcast().await;
    write_evidence("lagging-finality-recovery", &verified);
    let snapshot = environment.deployed.wallet.snapshot().unwrap();
    assert!(
        snapshot.spendable(&spent).is_none(),
        "recovery revived the spent note"
    );
}

/// After a payment is finalized, a genuine same-height provider conflict must stay
/// observation-only: no resend, no reservation release, no spent-note revival. Recovery
/// finishes by observation once the providers agree again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil and generated M5 prototype artifacts; conflict after finalized payment"]
async fn conflicting_providers_after_finality_never_resend_or_revive_the_spend() {
    let environment = Environment::new().await;
    let client = environment.deployed.client.clone();
    let root_anvil = environment.deployed.url.clone();
    let (code, response, stderr) = environment.settle(Duration::from_secs(240)).await;
    assert!(stderr.is_empty(), "{stderr}");
    assert!(
        code == 2 && response["status"] == "pending",
        "first settle: {response}"
    );
    assert_eq!(response["submitted_this_call"], true, "{response}");
    assert_eq!(sends(&environment.primary), 1, "{response}");
    mine(&client, &root_anvil, 4).await;
    let verified = environment.complete_after_broadcast().await;
    assert_eq!(verified["payment_verified"], true, "{verified}");

    let snapshot = environment.deployed.wallet.snapshot().unwrap();
    let spent = snapshot
        .notes()
        .iter()
        .find(|note| note.asset() == environment.deployed.token_bytes && note.amount() == 150)
        .expect("retained spent note")
        .commitment();
    assert!(
        snapshot.spendable(&spent).is_none(),
        "spent note must not be spendable"
    );
    drop(snapshot);

    // Fork the second provider at the finalized height and make both tips diverge there.
    let pinned = head(&client, &root_anvil).await;
    let timestamp = head_timestamp(&client, &root_anvil, pinned).await;
    let state = rpc(&client, &root_anvil, "anvil_dumpState", json!([]))
        .await
        .as_str()
        .unwrap()
        .to_owned();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let fork_url = format!("http://127.0.0.1:{port}");
    let fork = AnvilProcess(
        Command::new("anvil")
            .args([
                "--port",
                &port.to_string(),
                "--chain-id",
                "31337",
                "--slots-in-an-epoch",
                "1",
                "--silent",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    for _ in 0..100 {
        if client
            .post(&fork_url)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}))
            .send()
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert_eq!(
        rpc(&client, &fork_url, "anvil_loadState", json!([state])).await,
        true
    );
    rpc(
        &client,
        &root_anvil,
        "evm_setNextBlockTimestamp",
        json!([format!("0x{:x}", timestamp + 10)]),
    )
    .await;
    rpc(
        &client,
        &fork_url,
        "evm_setNextBlockTimestamp",
        json!([format!("0x{:x}", timestamp + 20)]),
    )
    .await;
    mine(&client, &root_anvil, 1).await;
    mine(&client, &fork_url, 1).await;
    *environment.peer.state.upstream.lock().unwrap() = fork_url.clone();
    let primary_hash = rpc(
        &client,
        &root_anvil,
        "eth_getBlockByNumber",
        json!([format!("0x{:x}", pinned + 1), false]),
    )
    .await;
    let peer_hash = rpc(
        &client,
        &fork_url,
        "eth_getBlockByNumber",
        json!([format!("0x{:x}", pinned + 1), false]),
    )
    .await;
    assert_ne!(
        primary_hash["hash"], peer_hash["hash"],
        "fork did not diverge after finality"
    );

    // Observation only: the conflict must not resend, release or revive anything.
    let (code, response, stderr) = environment.observe(Duration::from_secs(60)).await;
    assert!(stderr.is_empty(), "{stderr}");
    assert_ne!(
        code, 0,
        "conflicting providers must not report success: {response}"
    );
    assert_eq!(response["status"], "error", "{response}");
    assert!(
        [
            "paired deployment authentication failed",
            "paired pool evidence unavailable; retain reservations",
            "paired finalized wallet recovery failed; retain reservations",
        ]
        .contains(&response["error"].as_str().unwrap_or_default()),
        "unexpected conflict outcome: {response}"
    );
    assert_ne!(response["payment_verified"], true, "{response}");
    assert_ne!(response["submitted_this_call"], true, "{response}");
    assert_ne!(response["wallet_release_required"], true, "{response}");
    assert_eq!(
        sends(&environment.primary),
        1,
        "conflict after finality caused a resend"
    );
    let snapshot = environment.deployed.wallet.snapshot().unwrap();
    assert!(
        snapshot.spendable(&spent).is_none(),
        "conflict revived the spent note"
    );
    assert!(
        snapshot
            .notes()
            .iter()
            .find(|note| note.commitment() == spent)
            .is_some_and(|note| note.consumption().is_some()),
        "conflict dropped the consumption record"
    );
    drop(snapshot);
    write_evidence(
        "conflict-after-finality",
        &redact(
            code,
            &response,
            sends(&environment.primary),
            environment.input_note(),
        ),
    );

    // Observation-only recovery once the providers agree again.
    *environment.peer.state.upstream.lock().unwrap() = root_anvil.clone();
    drop(fork);
    let recovered = environment.complete_after_broadcast().await;
    write_evidence("conflict-after-finality-recovery", &recovered);
    assert_eq!(recovered["total_sends"], 1, "{recovered}");
    let snapshot = environment.deployed.wallet.snapshot().unwrap();
    assert!(
        snapshot.spendable(&spent).is_none(),
        "recovery revived the spent note"
    );
}

/// Bounded paired wallet-history scans are interrupted by `history_pending` and resumed by
/// separate CLI processes. The retained prefix must never rewind, consumption and the
/// one-payment fence must survive, and only observation may finish the recovery.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil and generated M5 prototype artifacts; interrupted paired wallet recovery"]
async fn interrupted_paired_wallet_recovery_resumes_without_rewind_or_second_payment() {
    let environment = Environment::new().await;
    let client = environment.deployed.client.clone();
    let (code, response, stderr) = environment.settle(Duration::from_secs(240)).await;
    assert!(stderr.is_empty(), "{stderr}");
    assert!(
        code == 2 && response["status"] == "pending",
        "first settle: {response}"
    );
    assert_eq!(response["submitted_this_call"], true, "{response}");
    assert_eq!(sends(&environment.primary), 1, "{response}");
    mine(&client, &environment.deployed.url, 4).await;

    // Only the scan bound changes: one bounded step per process forces the documented
    // HistoryPending progress path across restarts. No key, fence or format changes.
    let mut bounded: Value =
        serde_json::from_slice(&std::fs::read(&environment.config).unwrap()).unwrap();
    bounded["max_scan_blocks"] = json!(1);
    let bounded_path = environment.deployed.buyer_root.join("paired-bounded.json");
    private(&bounded_path, &serde_json::to_vec(&bounded).unwrap());

    let mut primary_tips = vec![environment.wallet_prefix_tip("wallet-primary")];
    let mut peer_tips = vec![environment.wallet_prefix_tip("wallet-peer")];
    let mut pending_cycles = 0usize;
    let deadline = Instant::now() + Duration::from_secs(240);
    loop {
        let (code, response, stderr) = run_cli(
            &environment.binary,
            environment.root(),
            payment_request(&bounded_path, "observe"),
            Duration::from_secs(60),
        );
        assert!(stderr.is_empty(), "{stderr}");
        if code == 0 {
            assert_eq!(response["payment_verified"], true, "{response}");
            break;
        }
        assert_eq!(code, 2, "bounded recovery must stay retryable: {response}");
        assert_eq!(response["status"], "pending", "{response}");
        assert_eq!(response["history_pending"], true, "{response}");
        assert_ne!(response["submitted_this_call"], true, "{response}");
        assert_eq!(sends(&environment.primary), 1, "bounded recovery resent");
        pending_cycles += 1;
        let primary = environment.wallet_prefix_tip("wallet-primary");
        let peer = environment.wallet_prefix_tip("wallet-peer");
        assert!(
            primary >= *primary_tips.last().unwrap(),
            "primary wallet prefix rewound: {primary_tips:?} -> {primary}"
        );
        assert!(
            peer >= *peer_tips.last().unwrap(),
            "peer wallet prefix rewound: {peer_tips:?} -> {peer}"
        );
        primary_tips.push(primary);
        peer_tips.push(peer);
        assert!(Instant::now() < deadline, "bounded recovery stalled");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        pending_cycles >= 1,
        "the paired wallet recovery was never interrupted"
    );
    let spent = {
        let snapshot = environment.deployed.wallet.snapshot().unwrap();
        let note = snapshot
            .notes()
            .iter()
            .find(|note| note.asset() == environment.deployed.token_bytes && note.amount() == 150)
            .expect("retained spent note");
        assert!(
            note.consumption().is_some(),
            "resumed recovery did not record the consumption"
        );
        let commitment = note.commitment();
        assert!(
            snapshot.spendable(&commitment).is_none(),
            "resumed recovery left the spent note spendable"
        );
        commitment
    };
    write_evidence(
        "interrupted-recovery",
        &json!({
            "pending_cycles": pending_cycles,
            "primary_prefix_tips": primary_tips,
            "peer_prefix_tips": peer_tips,
            "send_count": sends(&environment.primary),
            "status": "verified",
            "input_note_consumed": true,
        }),
    );

    // The completed fence still holds: observation-only repeat, no further send.
    let verified = environment.complete_after_broadcast().await;
    write_evidence("interrupted-recovery-fence", &verified);
    assert_eq!(verified["total_sends"], 1, "{verified}");
    let snapshot = environment.deployed.wallet.snapshot().unwrap();
    assert!(
        snapshot.spendable(&spent).is_none(),
        "the completed fence revived the spent note"
    );
}

/// Transient HTTP 503s on header and log reads during paired wallet catch-up are absorbed by
/// the bounded read retry: the wallet catches up to the shared anchor, the reserved input is
/// consumed exactly once, progression is retained and nothing is broadcast.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil and generated M5 prototype artifacts; transient wallet catch-up retries"]
async fn transient_wallet_catch_up_reads_are_retried_without_losing_progress() {
    let environment = Environment::new().await;
    let client = environment.deployed.client.clone();
    let (code, response, stderr) = environment.settle(Duration::from_secs(240)).await;
    assert!(stderr.is_empty(), "{stderr}");
    assert!(
        code == 2 && response["status"] == "pending",
        "first settle: {response}"
    );
    assert_eq!(response["submitted_this_call"], true, "{response}");
    assert_eq!(sends(&environment.primary), 1, "{response}");
    mine(&client, &environment.deployed.url, 4).await;
    let chain_head = head(&client, &environment.deployed.url).await;

    arm(
        &environment.peer,
        Behavior::FailReads {
            method: "eth_getBlockByNumber",
            remaining: Arc::new(AtomicUsize::new(2)),
        },
    );
    arm(
        &environment.primary,
        Behavior::FailReads {
            method: "eth_getLogs",
            remaining: Arc::new(AtomicUsize::new(2)),
        },
    );
    let (primary_url, peer_url) = environment.rpc_urls();
    let wallet = environment.wallet_store();
    let report = erebus_shielded_prover::recovery::recover_finalized_wallet_agreed_bounded(
        &environment.pool_rpc(&primary_url),
        &environment.wallet_index("wallet-primary"),
        &environment.pool_rpc(&peer_url),
        &environment.wallet_index("wallet-peer"),
        &wallet,
        1000,
    )
    .await
    .unwrap_or_else(|error| panic!("transient faults must be absorbed: {error}"));
    assert_eq!(failures(&environment.primary), 2, "log fault injections");
    assert_eq!(failures(&environment.peer), 2, "header fault injections");
    assert_eq!(
        sends(&environment.primary),
        1,
        "catch-up must not broadcast"
    );
    assert!(report.through <= chain_head);
    assert_eq!(
        environment.wallet_prefix_tip("wallet-primary"),
        report.through,
        "wallet prefix must reach the scanned anchor"
    );
    assert_eq!(environment.wallet_prefix_tip("wallet-peer"), report.through);
    let snapshot = wallet.snapshot().unwrap();
    let spent = snapshot
        .notes()
        .iter()
        .find(|note| note.asset() == environment.deployed.token_bytes && note.amount() == 150)
        .expect("retained spent note");
    assert!(
        spent.consumption().is_some(),
        "catch-up must record the consumption once"
    );
    assert!(snapshot.spendable(&spent.commitment()).is_none());
    write_evidence(
        "wallet-retry-transient",
        &json!({
            "log_faults": failures(&environment.primary),
            "header_faults": failures(&environment.peer),
            "caught_up_through": report.through,
            "primary_prefix_tip": environment.wallet_prefix_tip("wallet-primary"),
            "peer_prefix_tip": environment.wallet_prefix_tip("wallet-peer"),
            "send_count": sends(&environment.primary),
            "input_note_consumed": true,
            "input_note_spendable": false,
        }),
    );
}

/// Exhausted wallet catch-up retries fail closed: exactly four attempts per read, the
/// persisted prefix is retained, the input reservation stays held and nothing is broadcast.
/// After the provider heals the same catch-up consumes the input exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil and generated M5 prototype artifacts; exhausted wallet catch-up retries"]
async fn exhausted_wallet_catch_up_retries_retain_progress_and_hold_reservations() {
    let environment = Environment::new().await;
    let client = environment.deployed.client.clone();
    let (code, response, stderr) = environment.settle(Duration::from_secs(240)).await;
    assert!(stderr.is_empty(), "{stderr}");
    assert!(
        code == 2 && response["status"] == "pending",
        "first settle: {response}"
    );
    assert_eq!(response["submitted_this_call"], true, "{response}");
    assert_eq!(sends(&environment.primary), 1, "{response}");
    mine(&client, &environment.deployed.url, 4).await;

    let (primary_url, peer_url) = environment.rpc_urls();
    let wallet = environment.wallet_store();
    let (commitment, reserved, consumed) = {
        let snapshot = wallet.snapshot().unwrap();
        let note = snapshot
            .notes()
            .iter()
            .find(|note| note.asset() == environment.deployed.token_bytes && note.amount() == 150)
            .expect("reserved input note");
        (
            note.commitment(),
            note.reserved().is_some(),
            note.consumption().is_some(),
        )
    };
    assert!(reserved, "settle must hold the input reservation");
    assert!(!consumed, "the input cannot be consumed before catch-up");

    let primary_before = environment.wallet_prefix_tip("wallet-primary");
    let peer_before = environment.wallet_prefix_tip("wallet-peer");
    let step = erebus_shielded_prover::recovery::recover_finalized_wallet_agreed_bounded(
        &environment.pool_rpc(&primary_url),
        &environment.wallet_index("wallet-primary"),
        &environment.pool_rpc(&peer_url),
        &environment.wallet_index("wallet-peer"),
        &wallet,
        1,
    )
    .await;
    assert!(
        step.is_err(),
        "a one-block bounded step must yield pending progress, not evidence"
    );
    let primary_after_step = environment.wallet_prefix_tip("wallet-primary");
    let peer_after_step = environment.wallet_prefix_tip("wallet-peer");
    assert!(
        primary_after_step > primary_before && peer_after_step > peer_before,
        "the bounded step must persist progress"
    );

    arm(
        &environment.primary,
        Behavior::FailReads {
            method: "eth_getLogs",
            remaining: Arc::new(AtomicUsize::new(999)),
        },
    );
    arm(
        &environment.peer,
        Behavior::FailReads {
            method: "eth_getLogs",
            remaining: Arc::new(AtomicUsize::new(999)),
        },
    );
    let exhausted = erebus_shielded_prover::recovery::recover_finalized_wallet_agreed_bounded(
        &environment.pool_rpc(&primary_url),
        &environment.wallet_index("wallet-primary"),
        &environment.pool_rpc(&peer_url),
        &environment.wallet_index("wallet-peer"),
        &wallet,
        1,
    )
    .await;
    assert!(
        exhausted.is_err(),
        "exhausted reads must not produce recovery"
    );
    assert_eq!(
        failures(&environment.primary),
        5,
        "the retry budget is exactly five attempts (one try, four backed-off retries)"
    );
    assert_eq!(failures(&environment.peer), 5);
    assert_eq!(
        environment.wallet_prefix_tip("wallet-primary"),
        primary_after_step,
        "exhausted retries must retain the persisted prefix"
    );
    assert_eq!(
        environment.wallet_prefix_tip("wallet-peer"),
        peer_after_step
    );
    let snapshot = wallet.snapshot().unwrap();
    let note = snapshot
        .notes()
        .iter()
        .find(|note| note.commitment() == commitment)
        .expect("retained input note");
    assert!(
        note.reserved().is_some(),
        "the reservation must stay held after exhausted retries"
    );
    assert!(note.consumption().is_none());
    assert!(snapshot.spendable(&commitment).is_none());
    assert_eq!(
        sends(&environment.primary),
        1,
        "no broadcast during catch-up"
    );
    write_evidence(
        "wallet-retry-exhausted",
        &json!({
            "attempts_per_provider": failures(&environment.primary),
            "primary_prefix_tip": environment.wallet_prefix_tip("wallet-primary"),
            "peer_prefix_tip": environment.wallet_prefix_tip("wallet-peer"),
            "send_count": sends(&environment.primary),
            "input_note_reserved": true,
            "input_note_consumed": false,
        }),
    );

    arm(&environment.primary, Behavior::Pass);
    arm(&environment.peer, Behavior::Pass);
    let recovered = erebus_shielded_prover::recovery::recover_finalized_wallet_agreed_bounded(
        &environment.pool_rpc(&primary_url),
        &environment.wallet_index("wallet-primary"),
        &environment.pool_rpc(&peer_url),
        &environment.wallet_index("wallet-peer"),
        &wallet,
        1000,
    )
    .await
    .unwrap_or_else(|error| panic!("healed catch-up must complete: {error}"));
    let snapshot = wallet.snapshot().unwrap();
    let note = snapshot
        .notes()
        .iter()
        .find(|note| note.commitment() == commitment)
        .expect("retained input note");
    assert!(
        note.consumption().is_some(),
        "healed catch-up must record the consumption exactly once"
    );
    assert!(snapshot.spendable(&commitment).is_none());
    assert_eq!(sends(&environment.primary), 1);
    assert_eq!(
        environment.wallet_prefix_tip("wallet-primary"),
        recovered.through
    );
}

/// After the payment is spent and retained, exhausted read retries on a later catch-up fail
/// closed: no resend, no revived spent note, no rewind of the persisted wallet prefix, and
/// observation-only recovery once the provider heals.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil and generated M5 prototype artifacts; exhausted retries after consumption"]
async fn exhausted_read_retries_after_consumption_never_revive_the_spent_note() {
    let environment = Environment::new().await;
    let client = environment.deployed.client.clone();
    let (code, response, stderr) = environment.settle(Duration::from_secs(240)).await;
    assert!(stderr.is_empty(), "{stderr}");
    assert!(
        code == 2 && response["status"] == "pending",
        "first settle: {response}"
    );
    assert_eq!(response["submitted_this_call"], true, "{response}");
    assert_eq!(sends(&environment.primary), 1, "{response}");
    mine(&client, &environment.deployed.url, 4).await;
    let verified = environment.complete_after_broadcast().await;
    assert_eq!(verified["payment_verified"], true, "{verified}");

    let spent = {
        let snapshot = environment.deployed.wallet.snapshot().unwrap();
        let note = snapshot
            .notes()
            .iter()
            .find(|note| note.asset() == environment.deployed.token_bytes && note.amount() == 150)
            .expect("retained spent note");
        assert!(note.consumption().is_some());
        let commitment = note.commitment();
        assert!(snapshot.spendable(&commitment).is_none());
        commitment
    };
    let primary_before = environment.wallet_prefix_tip("wallet-primary");
    let peer_before = environment.wallet_prefix_tip("wallet-peer");
    mine(&client, &environment.deployed.url, 2).await;

    arm(
        &environment.primary,
        Behavior::FailReads {
            method: "eth_getLogs",
            remaining: Arc::new(AtomicUsize::new(999)),
        },
    );
    let (code, response, stderr) = environment.observe(Duration::from_secs(60)).await;
    assert!(stderr.is_empty(), "{stderr}");
    assert_eq!(code, 1, "exhausted retries must fail closed: {response}");
    assert_eq!(response["status"], "error", "{response}");
    // Wallet recovery reads new finalized history before observation (WS6), so it is the read
    // that exhausts its retries; either paired reader must fail closed the same way.
    assert_eq!(
        response["error"], "paired finalized wallet recovery failed; retain reservations",
        "{response}"
    );
    assert_ne!(response["payment_verified"], true, "{response}");
    assert_ne!(response["submitted_this_call"], true, "{response}");
    assert_eq!(
        failures(&environment.primary),
        5,
        "the retry budget is exactly five attempts (one try, four backed-off retries)"
    );
    assert_eq!(sends(&environment.primary), 1, "no resend after exhaustion");
    assert_eq!(
        environment.wallet_prefix_tip("wallet-primary"),
        primary_before,
        "exhausted retries must retain the wallet prefix"
    );
    // The healthy peer's paired scan runs alongside the failing primary and may persist its own
    // verified finalized progress (as a wallet sync always could); it never rewinds, and no wallet
    // state changes without both providers agreeing.
    assert!(environment.wallet_prefix_tip("wallet-peer") >= peer_before);
    let snapshot = environment.deployed.wallet.snapshot().unwrap();
    assert!(
        snapshot.spendable(&spent).is_none(),
        "exhausted retries revived the spent note"
    );
    assert!(
        snapshot
            .notes()
            .iter()
            .find(|note| note.commitment() == spent)
            .is_some_and(|note| note.consumption().is_some()),
        "exhausted retries dropped the consumption record"
    );
    drop(snapshot);
    write_evidence(
        "wallet-retry-after-consumption",
        &json!({
            "attempts": failures(&environment.primary),
            "exit_code": code,
            "error": response["error"],
            "send_count": sends(&environment.primary),
            "primary_prefix_tip": environment.wallet_prefix_tip("wallet-primary"),
            "peer_prefix_tip": environment.wallet_prefix_tip("wallet-peer"),
            "spent_note_consumed": true,
            "spent_note_spendable": false,
        }),
    );

    arm(&environment.primary, Behavior::Pass);
    let recovered = environment.complete_after_broadcast().await;
    assert_eq!(recovered["total_sends"], 1, "{recovered}");
    let snapshot = environment.deployed.wallet.snapshot().unwrap();
    assert!(
        snapshot.spendable(&spent).is_none(),
        "recovery revived the spent note"
    );
    assert_eq!(
        snapshot
            .notes()
            .iter()
            .filter(|note| note.commitment() == spent && note.consumption().is_some())
            .count(),
        1,
        "consumption must stay singular after recovery"
    );
}

/// A fresh recipient-bound grant must not replace an existing one, and the documented grant
/// expiry policy must refuse an expired grant without touching the sealed evidence or any RPC.
#[tokio::test]
async fn grant_reissue_preserves_the_original_and_expiry_is_enforced() {
    let fixture = Fixture::new(2, 60);
    let (_buyer, seller) = fixture.pair(false);
    let root = fixture.root.path();
    let disclosure = native_binary(
        "erebus-shielded-disclosure",
        env!("CARGO_BIN_EXE_erebus-shielded-disclosure"),
    );
    let selected =
        SelectedAgreement::decode(&fs::read(seller["evidence_file"].as_str().unwrap()).unwrap())
            .unwrap();
    let expected_issuer = format!(
        "0x{}",
        hex::encode(selected.terms.seller_authorization_key.as_bytes())
    );
    let key_a = root.join("auditor-a.key");
    let (ok, key_a_response) = finish(spawn(
        &disclosure,
        root,
        json!({"method": "keygen", "key_file": key_a}),
    ));
    assert!(ok, "{key_a_response}");
    let grant_a = root.join("grant-a");
    let issued = clock();
    let (ok, exported) = finish(spawn(
        &disclosure,
        root,
        json!({"method": "export", "evidence_file": seller["evidence_file"],
            "issuer_key_file": root.join("seller/agreement.key"),
            "recipient_public_key": key_a_response["recipient_public_key"],
            "grant_file": grant_a, "expires_at": issued + 30}),
    ));
    assert!(ok, "{exported}");
    assert_eq!(
        exported["issuer"], expected_issuer,
        "the grant issuer differs from the negotiated participant: {exported}"
    );
    let original = fs::read(&grant_a).unwrap();
    let (ok, verified) = finish(spawn(
        &disclosure,
        root,
        json!({"method": "verify_agreement", "grant_file": grant_a, "key_file": key_a,
            "expected_issuer": expected_issuer}),
    ));
    assert!(ok && verified["agreement_verified"] == true, "{verified}");

    // Re-exporting over the same path must refuse and preserve the original bytes.
    let (ok, refused) = finish(spawn(
        &disclosure,
        root,
        json!({"method": "export", "evidence_file": seller["evidence_file"],
            "issuer_key_file": root.join("seller/agreement.key"),
            "recipient_public_key": key_a_response["recipient_public_key"],
            "grant_file": grant_a, "expires_at": issued + 600}),
    ));
    assert!(!ok, "a second export must not replace the grant: {refused}");
    assert_eq!(
        fs::read(&grant_a).unwrap(),
        original,
        "the original grant bytes changed"
    );

    // A fresh recipient gets a fresh grant; the original still verifies.
    let key_b = root.join("auditor-b.key");
    let (ok, key_b_response) = finish(spawn(
        &disclosure,
        root,
        json!({"method": "keygen", "key_file": key_b}),
    ));
    assert!(ok, "{key_b_response}");
    let grant_b = root.join("grant-b");
    let (ok, exported_b) = finish(spawn(
        &disclosure,
        root,
        json!({"method": "export", "evidence_file": seller["evidence_file"],
            "issuer_key_file": root.join("seller/agreement.key"),
            "recipient_public_key": key_b_response["recipient_public_key"],
            "grant_file": grant_b, "expires_at": issued + 600}),
    ));
    assert!(ok, "{exported_b}");
    let (ok, verified_b) = finish(spawn(
        &disclosure,
        root,
        json!({"method": "verify_agreement", "grant_file": grant_b, "key_file": key_b,
            "expected_issuer": expected_issuer}),
    ));
    assert!(
        ok && verified_b["agreement_verified"] == true,
        "{verified_b}"
    );
    let (ok, wrong_recipient) = finish(spawn(
        &disclosure,
        root,
        json!({"method": "verify_agreement", "grant_file": grant_b, "key_file": key_a,
            "expected_issuer": expected_issuer}),
    ));
    assert!(!ok, "another recipient opened the grant: {wrong_recipient}");
    let (ok, again) = finish(spawn(
        &disclosure,
        root,
        json!({"method": "verify_agreement", "grant_file": grant_a, "key_file": key_a,
            "expected_issuer": expected_issuer}),
    ));
    assert!(ok && again["agreement_verified"] == true, "{again}");

    // After expiry the sealed grant is refused before any RPC read and is left unchanged.
    while clock() < issued + 31 {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let (ok, expired) = finish(spawn(
        &disclosure,
        root,
        json!({"method": "verify_agreement", "grant_file": grant_a, "key_file": key_a,
            "expected_issuer": expected_issuer}),
    ));
    assert!(!ok, "an expired grant verified: {expired}");
    assert_eq!(
        expired["error"], "disclosure agreement not verified",
        "{expired}"
    );
    assert_eq!(
        fs::read(&grant_a).unwrap(),
        original,
        "the expiry policy mutated the grant"
    );
    let (ok, still_fresh) = finish(spawn(
        &disclosure,
        root,
        json!({"method": "verify_agreement", "grant_file": grant_b, "key_file": key_b,
            "expected_issuer": expected_issuer}),
    ));
    assert!(
        ok && still_fresh["agreement_verified"] == true,
        "{still_fresh}"
    );
    write_evidence(
        "grant-reissue-expiry",
        &json!({
            "reissue_refused": true,
            "original_preserved": true,
            "wrong_recipient_refused": true,
            "expired_error": expired["error"],
            "fresh_grant_verified": true,
        }),
    );
}

/// The auditor's certified catch-up across fresh processes: bounded scans persist
/// checkpoints, missing history stays pending with progress (never absence), provider lag is
/// tolerated, and exhausted read retries fail closed without rewinding the checkpoints.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil and generated M5 prototype artifacts; auditor catch-up checkpoints"]
async fn auditor_catch_up_keeps_checkpoints_and_pending_is_never_absence() {
    let environment = Environment::new().await;
    let client = environment.deployed.client.clone();
    let (code, response, stderr) = environment.settle(Duration::from_secs(240)).await;
    assert!(stderr.is_empty(), "{stderr}");
    assert!(
        code == 2 && response["status"] == "pending",
        "first settle: {response}"
    );
    assert_eq!(response["submitted_this_call"], true, "{response}");
    assert_eq!(sends(&environment.primary), 1, "{response}");
    mine(&client, &environment.deployed.url, 4).await;
    let verified = environment.complete_after_broadcast().await;
    assert_eq!(verified["payment_verified"], true, "{verified}");

    let auditor = environment.root().join("auditor");
    fs::create_dir(&auditor).unwrap();
    let key_file = auditor.join("auditor.key");
    let key_response = environment.auditor_keygen(&key_file);
    let grant_file = auditor.join("deal.grant");
    let (ok, exported) = environment.auditor_export(
        &key_response["recipient_public_key"],
        &grant_file,
        clock() + 3600,
    );
    assert!(ok, "{exported}");
    let cache = auditor.join("public-history");
    let deployment = environment.auditor_deployment(&cache);

    // More history than one certified scan window: the first process must stay pending with
    // persisted checkpoints, never treating missing history as absence.
    mine(&client, &environment.deployed.url, 1200).await;
    let (ok, first) = environment.auditor_verify(&grant_file, &key_file, &deployment);
    assert!(!ok, "an unfinished scan must not finalize: {first}");
    assert_eq!(first["status"], "pending", "{first}");
    assert_eq!(first["agreement_verified"], true, "{first}");
    assert_ne!(first["payment_verified"], true, "{first}");
    assert!(
        first["next_log_block"].as_u64().is_some(),
        "pending must report progress, not absence: {first}"
    );
    let primary_tip = environment
        .auditor_cache_tip(&cache, "primary")
        .expect("a bounded scan must persist a primary checkpoint");
    let peer_tip = environment
        .auditor_cache_tip(&cache, "peer")
        .expect("a bounded scan must persist a peer checkpoint");

    // Fresh processes resume from the same caches without rewind.
    let mut attempts = 0;
    loop {
        let (ok, response) = environment.auditor_verify(&grant_file, &key_file, &deployment);
        if ok && response["payment_verified"] == true {
            assert_eq!(response["agreement_verified"], true, "{response}");
            assert_eq!(response["delivery_verified"], false, "{response}");
            break;
        }
        assert_eq!(response["status"], "pending", "{response}");
        assert_ne!(response["payment_verified"], true, "{response}");
        let primary = environment.auditor_cache_tip(&cache, "primary").unwrap();
        let peer = environment.auditor_cache_tip(&cache, "peer").unwrap();
        assert!(
            primary >= primary_tip && peer >= peer_tip,
            "auditor checkpoints rewound: {primary}/{peer} from {primary_tip}/{peer_tip}"
        );
        attempts += 1;
        assert!(attempts < 8, "auditor catch-up stalled: {response}");
    }
    let settled_primary = environment.auditor_cache_tip(&cache, "primary").unwrap();
    let settled_peer = environment.auditor_cache_tip(&cache, "peer").unwrap();

    // Provider lag: one honest provider one block behind must not stop certification.
    mine(&client, &environment.deployed.url, 2).await;
    let head = head(&client, &environment.deployed.url).await;
    arm(&environment.peer, Behavior::Pin { head: head - 1 });
    let (ok, lagged) = environment.auditor_verify(&grant_file, &key_file, &deployment);
    assert!(ok && lagged["payment_verified"] == true, "{lagged}");
    disarm(&environment.peer);

    // Exhausted read retries: exactly five attempts, fail closed, checkpoints preserved.
    mine(&client, &environment.deployed.url, 2).await;
    arm(
        &environment.primary,
        Behavior::FailReads {
            method: "eth_getLogs",
            remaining: Arc::new(AtomicUsize::new(999)),
        },
    );
    let (ok, exhausted) = environment.auditor_verify(&grant_file, &key_file, &deployment);
    assert!(!ok && exhausted["status"] == "error", "{exhausted}");
    assert_ne!(exhausted["payment_verified"], true, "{exhausted}");
    assert_eq!(
        failures(&environment.primary),
        5,
        "the retry budget is exactly five attempts (one try, four backed-off retries)"
    );
    assert!(
        environment.auditor_cache_tip(&cache, "primary").unwrap() >= settled_primary
            && environment.auditor_cache_tip(&cache, "peer").unwrap() >= settled_peer,
        "exhausted retries rewound the auditor checkpoints"
    );
    disarm(&environment.primary);
    let (ok, recovered) = environment.auditor_verify(&grant_file, &key_file, &deployment);
    assert!(ok && recovered["payment_verified"] == true, "{recovered}");
    write_evidence(
        "auditor-catch-up",
        &json!({
            "first_status": "pending",
            "next_log_block_present": true,
            "initial_primary_tip": primary_tip,
            "initial_peer_tip": peer_tip,
            "settled_primary_tip": settled_primary,
            "settled_peer_tip": settled_peer,
            "lagged_verified": true,
            "exhausted_attempts": 4,
            "exhausted_checkpoints_retained": true,
            "final_verified": true,
            "send_count": sends(&environment.primary),
        }),
    );
}

fn deal_transferred_topic() -> String {
    format!(
        "0x{}",
        hex::encode(erebus_core::suite::keccak256(&[
            b"DealTransferred(uint256,uint256,uint256)".as_slice()
        ]))
    )
}

/// WS5: an auditor cache started from a paired finalized checkpoint before the deal settles.
/// The command is explicit, idempotent without RPC, never reads logs, and refuses a cache that
/// already holds full history or another deployment's record. An unconsumed deal reads exactly
/// as with full history, and the later payment certifies to the same certificate. A provider
/// that omits the settlement event fails closed as inconsistency, never as unpaid.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil and generated M5 prototype artifacts; auditor history checkpoint"]
async fn auditor_checkpoint_before_settlement_certifies_like_full_history() {
    let environment = Environment::new().await;
    let client = environment.deployed.client.clone();
    let auditor = environment.root().join("auditor-checkpoint");
    fs::create_dir(&auditor).unwrap();
    let key_file = auditor.join("auditor.key");
    let key_response = environment.auditor_keygen(&key_file);
    let grant_file = auditor.join("deal.grant");
    let (ok, exported) = environment.auditor_export(
        &key_response["recipient_public_key"],
        &grant_file,
        clock() + 3600,
    );
    assert!(ok, "{exported}");
    let checkpointed = auditor.join("checkpointed");
    let full = auditor.join("full");
    let with_checkpoint = environment.auditor_deployment(&checkpointed);
    let with_full = environment.auditor_deployment(&full);
    let record = checkpointed.join(erebus_shielded_prover::checkpoint::RECORD_FILE);

    // Explicit opt-in: nothing is acquired or written without it.
    let (code, refused) = environment.auditor_checkpoint(&with_checkpoint, false);
    assert_eq!(code, 1, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("confirm_new_cache"),
        "{refused}"
    );
    assert!(
        !checkpointed.exists(),
        "a refused checkpoint created the cache"
    );

    // Created at a paired finalized block, with zero log reads on either provider.
    mine(&client, &environment.deployed.url, 4).await;
    let logs = (
        log_reads(&environment.primary),
        log_reads(&environment.peer),
    );
    let (code, created) = environment.auditor_checkpoint(&with_checkpoint, true);
    assert_eq!(code, 0, "{created}");
    assert_eq!(created["checkpoint"], "created", "{created}");
    assert_eq!(created["payment_verified"], false, "{created}");
    let block = created["checkpoint_block"].as_u64().unwrap();
    assert_eq!(created["history_from_block"], block + 1, "{created}");
    assert_eq!(
        (
            log_reads(&environment.primary),
            log_reads(&environment.peer)
        ),
        logs,
        "a checkpoint must not read logs"
    );
    let record_bytes = fs::read(&record).unwrap();

    // Idempotent: the same record, no provider request, no byte changed.
    let before = (requests(&environment.primary), requests(&environment.peer));
    let (code, again) = environment.auditor_checkpoint(&with_checkpoint, true);
    assert_eq!(code, 0, "{again}");
    assert_eq!(again["checkpoint"], "existing", "{again}");
    assert_eq!(again["checkpoint_block"], block, "{again}");
    assert_eq!(
        (requests(&environment.primary), requests(&environment.peer)),
        before,
        "an existing record must answer without RPC"
    );
    assert_eq!(fs::read(&record).unwrap(), record_bytes);

    // A concurrent initialization is refused.
    let held = erebus_shielded_prover::checkpoint::InitLock::acquire(&checkpointed).unwrap();
    let (code, busy) = environment.auditor_checkpoint(&with_checkpoint, true);
    assert_eq!(code, 1, "{busy}");
    assert_eq!(
        busy["error"], "another checkpoint initialization is in progress",
        "{busy}"
    );
    drop(held);

    // A record bound to another deployment anchor is refused by both methods, never ignored.
    let foreign = auditor.join("foreign");
    fs::create_dir(&foreign).unwrap();
    let mut foreign_domain = environment.index_domain();
    foreign_domain.first_hash[0] ^= 1;
    let stored =
        erebus_shielded_prover::checkpoint::load(&checkpointed, environment.index_domain())
            .unwrap()
            .unwrap();
    let lock = erebus_shielded_prover::checkpoint::InitLock::acquire(&foreign).unwrap();
    erebus_shielded_prover::checkpoint::persist(&foreign, foreign_domain, stored, &lock).unwrap();
    drop(lock);
    let with_foreign = environment.auditor_deployment(&foreign);
    let (code, refused) = environment.auditor_checkpoint(&with_foreign, true);
    assert_eq!(code, 1, "{refused}");
    assert_eq!(
        refused["error"], "auditor history checkpoint record unavailable",
        "{refused}"
    );
    let (code, refused) = environment.auditor_verify_code(&grant_file, &key_file, &with_foreign);
    assert_eq!(code, 1, "{refused}");
    assert_eq!(
        refused["error"], "auditor history checkpoint record unavailable",
        "{refused}"
    );
    assert!(!foreign.join("primary").exists() && !foreign.join("peer").exists());

    // Unconsumed: the checkpointed cache answers exactly as full history does (pending, no claim).
    let (full_code, full_unpaid) =
        environment.auditor_verify_settled(&grant_file, &key_file, &with_full);
    let (code, unpaid) =
        environment.auditor_verify_settled(&grant_file, &key_file, &with_checkpoint);
    assert_eq!((code, &unpaid), (full_code, &full_unpaid));
    assert_eq!(unpaid["payment_verified"], false, "{unpaid}");

    // A cache that already began full history is strictly stronger: it is never narrowed.
    let (code, refused) = environment.auditor_checkpoint(&with_full, true);
    assert_eq!(code, 1, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .starts_with("auditor cache already holds full history"),
        "{refused}"
    );
    assert!(!full
        .join(erebus_shielded_prover::checkpoint::RECORD_FILE)
        .exists());

    // Settle after the checkpoint.
    let (code, response, stderr) = environment.settle(Duration::from_secs(240)).await;
    assert!(stderr.is_empty(), "{stderr}");
    assert!(code == 2 && response["status"] == "pending", "{response}");
    mine(&client, &environment.deployed.url, 4).await;
    let paid = environment.complete_after_broadcast().await;
    assert_eq!(paid["payment_verified"], true, "{paid}");

    let (full_code, full_verified) =
        environment.auditor_verify_settled(&grant_file, &key_file, &with_full);
    let (code, verified) =
        environment.auditor_verify_settled(&grant_file, &key_file, &with_checkpoint);
    assert_eq!(full_code, 0, "{full_verified}");
    assert_eq!(full_verified["payment_verified"], true, "{full_verified}");
    assert_eq!((code, &verified), (full_code, &full_verified));
    let full_certificate = environment.auditor_certificate(&full).await.unwrap();
    let certificate = environment
        .auditor_certificate(&checkpointed)
        .await
        .unwrap();
    assert!(certificate.settlement.block_number > block);
    assert_eq!(
        certificate, full_certificate,
        "checkpointed certificate differs"
    );
    for name in ["primary", "peer"] {
        // Version 4 manifest: bound to the checkpoint at `block`, first segment starting at
        // `block + 1` and linked to the checkpoint block's hash.
        let cache = environment.auditor_cache_json(&checkpointed, name);
        assert_eq!(cache["version"], 4, "{name}");
        assert_eq!(cache["checkpoint"]["block_number"], block, "{name}");
        assert_eq!(cache["segments"][0]["first"], block + 1, "{name}");
        let checkpoint_hash: Vec<u8> =
            serde_json::from_value(cache["checkpoint"]["block_hash"].clone()).unwrap();
        assert_eq!(
            cache["base_parent_hash"],
            hex::encode(checkpoint_hash),
            "{name}"
        );
        let full_cache = environment.auditor_cache_json(&full, name);
        assert_eq!(full_cache["version"], 4, "{name}");
        assert!(full_cache["checkpoint"].is_null(), "{name}");
    }

    // A provider that omits the settlement event: consumed now, not at the checkpoint, no event
    // after it. That is inconsistency, never unpaid and never settled_before_checkpoint.
    let omitted = auditor.join("omitted");
    fs::create_dir(&omitted).unwrap();
    fs::copy(
        &record,
        omitted.join(erebus_shielded_prover::checkpoint::RECORD_FILE),
    )
    .unwrap();
    let with_omitted = environment.auditor_deployment(&omitted);
    let topic = deal_transferred_topic();
    arm(
        &environment.primary,
        Behavior::DropLogs {
            topic: topic.clone(),
        },
    );
    arm(&environment.peer, Behavior::DropLogs { topic });
    let (code, inconsistent) =
        environment.auditor_verify_settled(&grant_file, &key_file, &with_omitted);
    disarm(&environment.primary);
    disarm(&environment.peer);
    assert_eq!(code, 1, "{inconsistent}");
    assert!(
        inconsistent["error"]
            .as_str()
            .unwrap()
            .starts_with("pool providers are inconsistent"),
        "{inconsistent}"
    );
    assert_ne!(inconsistent["payment_verified"], true, "{inconsistent}");
    write_evidence(
        "auditor-checkpoint-before-settlement",
        &json!({
            "checkpoint_block": block,
            "settlement_block": certificate.settlement.block_number,
            "confirm_required": true,
            "checkpoint_log_reads": 0,
            "idempotent_requests": 0,
            "foreign_record_refused": true,
            "full_history_cache_refused": true,
            "unconsumed_equal_to_full_history": true,
            "certificate_equal_to_full_history": true,
            "omitted_event_inconsistent": true,
            "send_count": sends(&environment.primary),
        }),
    );
}

/// WS5: the checkpoint scope boundary. With the record at X - 1 (settlement at B + 1) the
/// payment certifies exactly as full history; with the record at X or later the deal is
/// `settled_before_checkpoint` (exit 3), never paid and never unpaid.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil and generated M5 prototype artifacts; auditor checkpoint scope"]
async fn auditor_checkpoint_at_or_after_settlement_is_settled_before_checkpoint() {
    let environment = Environment::new().await;
    let client = environment.deployed.client.clone();
    let (code, response, stderr) = environment.settle(Duration::from_secs(240)).await;
    assert!(stderr.is_empty(), "{stderr}");
    assert!(code == 2 && response["status"] == "pending", "{response}");
    mine(&client, &environment.deployed.url, 4).await;
    let paid = environment.complete_after_broadcast().await;
    assert_eq!(paid["payment_verified"], true, "{paid}");

    let auditor = environment.root().join("auditor-scope");
    fs::create_dir(&auditor).unwrap();
    let key_file = auditor.join("auditor.key");
    let key_response = environment.auditor_keygen(&key_file);
    let grant_file = auditor.join("deal.grant");
    let (ok, exported) = environment.auditor_export(
        &key_response["recipient_public_key"],
        &grant_file,
        clock() + 3600,
    );
    assert!(ok, "{exported}");
    let full = auditor.join("full");
    let full_certificate = environment.auditor_certificate(&full).await.unwrap();
    let settled = full_certificate.settlement.block_number;
    let (full_code, full_verified) = environment.auditor_verify_settled(
        &grant_file,
        &key_file,
        &environment.auditor_deployment(&full),
    );
    assert_eq!(full_code, 0, "{full_verified}");

    // A checkpoint at a chosen finalized block, by clamping both providers' finalized tag.
    let checkpoint_at = |name: &str, number: Option<u64>| {
        let cache = auditor.join(name);
        let deployment = environment.auditor_deployment(&cache);
        if let Some(number) = number {
            arm(&environment.primary, Behavior::PinFinalized { number });
            arm(&environment.peer, Behavior::PinFinalized { number });
        }
        let (code, created) = environment.auditor_checkpoint(&deployment, true);
        disarm(&environment.primary);
        disarm(&environment.peer);
        assert_eq!(code, 0, "{created}");
        if let Some(number) = number {
            assert_eq!(created["checkpoint_block"], number, "{created}");
        }
        (
            cache,
            deployment,
            created["checkpoint_block"].as_u64().unwrap(),
        )
    };

    // B = X - 1: the settlement is the first scanned block, and certifies like full history.
    let (before, with_before, _) = checkpoint_at("before", Some(settled - 1));
    let (code, verified) = environment.auditor_verify_settled(&grant_file, &key_file, &with_before);
    assert_eq!((code, &verified), (full_code, &full_verified));
    assert_eq!(
        environment.auditor_certificate(&before).await.unwrap(),
        full_certificate
    );

    // B = X and B > X: settled before the checkpoint, never paid or unpaid.
    mine(&client, &environment.deployed.url, 6).await;
    let mut outcomes = Vec::new();
    for (name, number) in [("at", Some(settled)), ("after", None)] {
        let (_, deployment, block) = checkpoint_at(name, number);
        assert!(block >= settled, "{name}: {block} < {settled}");
        let (code, scoped) =
            environment.auditor_verify_settled(&grant_file, &key_file, &deployment);
        assert_eq!(code, 3, "{name}: {scoped}");
        assert_eq!(scoped["status"], "settled_before_checkpoint", "{scoped}");
        assert_eq!(scoped["checkpoint_block"], block, "{scoped}");
        assert_eq!(scoped["agreement_verified"], true, "{scoped}");
        assert_eq!(scoped["payment_verified"], false, "{scoped}");
        assert_eq!(scoped["delivery_verified"], false, "{scoped}");
        assert_eq!(
            scoped["deal_nullifier"], full_verified["deal_nullifier"],
            "{scoped}"
        );
        assert!(scoped["next_action"]
            .as_str()
            .unwrap()
            .contains("starts earlier"));
        // The library never certifies it either: incomplete history, not a payment.
        let library = environment.auditor_certificate(&auditor.join(name)).await;
        assert!(
            matches!(
                library,
                Err(
                    erebus_shielded_prover::disclosure::ShieldedDisclosureError::Observation(
                        erebus_shielded_prover::observation::ObservationError::HistoryIncomplete
                    )
                )
            ),
            "{name}: {:?}",
            library.err()
        );
        outcomes.push(json!({"case": name, "checkpoint_block": block, "status": scoped["status"]}));
    }
    assert_eq!(
        sends(&environment.primary),
        1,
        "the auditor sent a transaction"
    );
    write_evidence(
        "auditor-checkpoint-scope",
        &json!({
            "settlement_block": settled,
            "checkpoint_before_certifies_like_full_history": true,
            "outcomes": outcomes,
        }),
    );
}

/// Payment finalized before the delivery deadline, retrieval after it: the service marks the
/// issuance late and never reports delivery success or recharges, and the agreement is unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil and generated M5 prototype artifacts; retrieval after the delivery deadline"]
async fn retrieval_after_delivery_deadline_is_late_without_delivery_or_recharge() {
    let environment = Environment::new_with(|terms| {
        terms.service.delivery_deadline = clock() + 60;
    })
    .await;
    let client = environment.deployed.client.clone();
    let (code, response, stderr) = environment.settle(Duration::from_secs(240)).await;
    assert!(stderr.is_empty(), "{stderr}");
    assert!(
        code == 2 && response["status"] == "pending",
        "first settle: {response}"
    );
    assert_eq!(response["submitted_this_call"], true, "{response}");
    assert_eq!(sends(&environment.primary), 1, "{response}");
    mine(&client, &environment.deployed.url, 4).await;
    let verified = environment.complete_after_broadcast().await;
    assert_eq!(verified["payment_verified"], true, "{verified}");
    let selected = environment.selected();
    let deadline = selected.terms.service.delivery_deadline;
    assert!(
        clock() < deadline,
        "the payment must finalize before the deadline; run funded tests in release"
    );
    let evidence_file = environment.seller["evidence_file"]
        .as_str()
        .unwrap()
        .to_owned();
    let evidence_before = fs::read(&evidence_file).unwrap();

    let (_service, url, _config, _port) = environment
        .start_access_service(&environment.root().join("deadline-service"))
        .await;
    while clock() <= deadline + 1 {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let request = environment.access_request(&selected, [42; 32], [44; 32], clock() + 120);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let issued = client
        .post(format!("{url}/v1/access"))
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(issued.status(), 200);
    let body: Value = issued.json().await.unwrap();
    assert_eq!(body["payment_verified"], true, "{body}");
    assert_eq!(
        body["delivery_verified"], false,
        "delivery success must never be reported: {body}"
    );
    assert_eq!(
        body["issuance"]["late"], true,
        "issuance after the deadline must be marked late: {body}"
    );
    let issuance_id = body["issuance"]["issuance_id"].clone();
    let repeated = client
        .post(format!("{url}/v1/access"))
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(repeated.status(), 200);
    let repeated: Value = repeated.json().await.unwrap();
    assert_eq!(repeated["issuance"]["repeated"], true, "{repeated}");
    assert_eq!(
        repeated["issuance"]["issuance_id"], issuance_id,
        "a retry must reuse the same issuance: {repeated}"
    );
    assert_eq!(repeated["issuance"]["late"], true, "{repeated}");
    assert_eq!(
        sends(&environment.primary),
        1,
        "retrieval after the deadline must not recharge"
    );
    assert_eq!(
        fs::read(&evidence_file).unwrap(),
        evidence_before,
        "the agreement evidence changed"
    );
    write_evidence(
        "delivery-deadline",
        &json!({
            "finalized_before_deadline": true,
            "late": true,
            "delivery_verified": false,
            "repeated_same_issuance": true,
            "send_count": sends(&environment.primary),
            "agreement_unchanged": true,
        }),
    );
}

/// Slow service-side history scanning outlives the former thirty-second access-client limit:
/// the timed-out attempt loses no checkpoint, the retry finishes from stored progress, the
/// resource verifies and no second payment is ever made.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil and generated M5 prototype artifacts; slow access history scan"]
async fn slow_access_history_outlives_thirty_seconds_without_recharging() {
    let environment = Environment::new().await;
    let client = environment.deployed.client.clone();
    let (code, response, stderr) = environment.settle(Duration::from_secs(240)).await;
    assert!(stderr.is_empty(), "{stderr}");
    assert!(
        code == 2 && response["status"] == "pending",
        "first settle: {response}"
    );
    assert_eq!(response["submitted_this_call"], true, "{response}");
    assert_eq!(sends(&environment.primary), 1, "{response}");
    mine(&client, &environment.deployed.url, 4).await;
    let verified = environment.complete_after_broadcast().await;
    assert_eq!(verified["payment_verified"], true, "{verified}");

    // More history than one bounded service scan, so the first request stays pending with a
    // stored checkpoint before the slow read. It is mined before the service starts.
    mine(&client, &environment.deployed.url, 2200).await;
    // The service's background catch-up outpaces the chain: its first 100-block steps already
    // hold this early payment, and with both caches holding it the service certifies the cached
    // payment from its finalized prefix and delivers at once. Stall the peer observer's first
    // log read for 35 s, so the first request finds the peer cache without the payment and has
    // to make its own bounded scan, which is what this test exercises.
    arm(
        &environment.peer,
        Behavior::DelayReads {
            method: "eth_getLogs",
            calls: Arc::new(AtomicUsize::new(0)),
            delay_at: 1,
            delay_count: 1,
            milliseconds: 35_000,
        },
    );
    let service_dir = environment.root().join("slow-service");
    let (_service, url, _config, _port) = environment.start_access_service(&service_dir).await;
    let state_root = service_dir.join("access-state");
    let selected = environment.selected();
    let request = environment.access_request(&selected, [42; 32], [45; 32], clock() + 295);
    let patient = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .unwrap();
    let first = patient
        .post(format!("{url}/v1/access"))
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(
        first.status(),
        202,
        "an unfinished scan must stay pending, not absence"
    );
    let first_body: Value = first.json().await.unwrap();
    assert_eq!(first_body["status"], "payment_pending", "{first_body}");
    assert_eq!(first_body["retry_without_payment"], true, "{first_body}");
    let checkpoint = environment
        .service_history_tip(&state_root, "pool-history.json")
        .expect("the bounded scan must persist a checkpoint");
    // A fresh request lifetime starts here, so a slow first scan cannot let the signed
    // request expire before the deliberately slow phase.
    let request = environment.access_request(&selected, [42; 32], [46; 32], clock() + 295);

    // Every service log read now outlives a thirty-second client until the retry phase
    // disarms the delay. The aborted attempt must keep the stored checkpoint, and the retry
    // must resume without another payment.
    arm(
        &environment.primary,
        Behavior::DelayReads {
            method: "eth_getLogs",
            calls: Arc::new(AtomicUsize::new(0)),
            delay_at: 1,
            delay_count: usize::MAX,
            milliseconds: 35_000,
        },
    );
    let impatient = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let timed_out = impatient
        .post(format!("{url}/v1/access"))
        .json(&request)
        .send()
        .await;
    assert!(
        timed_out.is_err(),
        "the slow scan must outlive the thirty-second client: {:?}",
        timed_out
            .as_ref()
            .map(|response| (response.status(), response.headers().clone()))
    );
    disarm(&environment.primary);
    let retained = environment
        .service_history_tip(&state_root, "pool-history.json")
        .expect("the timed-out attempt lost the persisted checkpoint");
    assert!(
        retained >= checkpoint,
        "the timed-out attempt rewound the checkpoint: {retained} < {checkpoint}"
    );

    let mut attempts = 0;
    loop {
        let response = patient
            .post(format!("{url}/v1/access"))
            .json(&request)
            .send()
            .await
            .unwrap();
        if response.status() == reqwest::StatusCode::OK {
            let body: Value = response.json().await.unwrap();
            assert_eq!(body["payment_verified"], true, "{body}");
            assert_eq!(body["delivery_verified"], false, "{body}");
            assert_eq!(
                hex::decode(body["payload_hex"].as_str().unwrap()).unwrap(),
                native_product::PAYLOAD,
                "the resumed retrieval returned the wrong resource"
            );
            break;
        }
        assert_eq!(response.status(), 202, "unexpected retrieval status");
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["status"], "payment_pending", "{body}");
        assert_eq!(body["retry_without_payment"], true, "{body}");
        let tip = environment
            .service_history_tip(&state_root, "pool-history.json")
            .unwrap();
        assert!(tip >= retained, "checkpoint rewound during retry: {tip}");
        attempts += 1;
        assert!(attempts < 8, "slow scan did not finish: {body}");
    }
    assert_eq!(
        sends(&environment.primary),
        1,
        "retrieval retries must never trigger another payment"
    );
    assert_eq!(sends(&environment.peer), 0);
    write_evidence(
        "slow-access-history",
        &json!({
            "first_status": "payment_pending",
            "checkpoint_after_first": checkpoint,
            "checkpoint_after_timeout": retained,
            "thirty_second_client_timed_out": true,
            "retry_attempts": attempts,
            "resource_verified": true,
            "send_count": sends(&environment.primary),
        }),
    );
}

/// A cold seller must not see or spend the received note until the complete paired finalized
/// history is scanned. A failing peer never reveals it, bounded catch-up retains progress
/// across calls, and only a finished paired prefix makes the note spendable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil and generated M5 prototype artifacts; cold seller paired recovery"]
async fn cold_seller_recovery_reveals_the_note_only_after_complete_paired_history() {
    let environment = Environment::new().await;
    let client = environment.deployed.client.clone();
    let (code, response, stderr) = environment.settle(Duration::from_secs(240)).await;
    assert!(stderr.is_empty(), "{stderr}");
    assert!(
        code == 2 && response["status"] == "pending",
        "first settle: {response}"
    );
    assert_eq!(sends(&environment.primary), 1, "{response}");
    mine(&client, &environment.deployed.url, 4).await;
    let verified = environment.complete_after_broadcast().await;
    assert_eq!(verified["payment_verified"], true, "{verified}");

    let seller = environment.seller_wallet_store();
    let received = {
        let snapshot = seller.snapshot().unwrap();
        let note = snapshot
            .notes()
            .iter()
            .find(|note| note.asset() == environment.deployed.token_bytes && note.amount() == 70)
            .expect("received expected-payment note");
        assert!(note.consumption().is_none());
        note.commitment()
    };
    assert!(
        seller.snapshot().unwrap().spendable(&received).is_none(),
        "a cold received note must not be spendable before recovery"
    );

    let (primary_url, peer_url) = environment.rpc_urls();
    let recovery_root = environment.root().join("seller-recovery");
    std::fs::create_dir(&recovery_root).unwrap();
    let primary_index = erebus_shielded_prover::index_store::IndexStore::new(
        recovery_root.join("primary.json"),
        environment.index_domain(),
    )
    .unwrap();
    let peer_index = erebus_shielded_prover::index_store::IndexStore::new(
        recovery_root.join("peer.json"),
        environment.index_domain(),
    )
    .unwrap();
    let primary_rpc = environment.pool_rpc(&primary_url);
    let peer_rpc = environment.pool_rpc(&peer_url);
    let tip = |index: &erebus_shielded_prover::index_store::IndexStore| {
        index
            .load()
            .unwrap()
            .tip()
            .map(|tip| tip.number)
            .unwrap_or(0)
    };

    // A failing peer must not produce a recovery, reveal the note, or lose progress.
    let (primary_before, peer_before) = (tip(&primary_index), tip(&peer_index));
    arm(
        &environment.peer,
        Behavior::FailReads {
            method: "eth_getLogs",
            remaining: Arc::new(AtomicUsize::new(999)),
        },
    );
    let failed = erebus_shielded_prover::recovery::recover_finalized_wallet_agreed_bounded(
        &primary_rpc,
        &primary_index,
        &peer_rpc,
        &peer_index,
        &seller,
        1,
    )
    .await;
    disarm(&environment.peer);
    assert!(failed.is_err(), "a failing peer produced a recovery");
    assert!(
        seller.snapshot().unwrap().spendable(&received).is_none(),
        "a failing peer revealed the received note"
    );
    assert!(
        tip(&primary_index) >= primary_before && tip(&peer_index) >= peer_before,
        "a failing peer rewound the retained prefix"
    );
    assert_eq!(
        sends(&environment.primary),
        1,
        "seller recovery must not broadcast"
    );

    // Bounded catch-up resumes; the note appears only on the complete paired prefix.
    let mut pending = 0usize;
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut primary_tips = vec![tip(&primary_index)];
    let mut peer_tips = vec![tip(&peer_index)];
    loop {
        match erebus_shielded_prover::recovery::recover_finalized_wallet_agreed_bounded(
            &primary_rpc,
            &primary_index,
            &peer_rpc,
            &peer_index,
            &seller,
            1,
        )
        .await
        {
            Ok(report) => {
                let snapshot = seller.snapshot().unwrap();
                assert!(
                    snapshot.spendable(&received).is_some(),
                    "a complete paired history must reveal the received note"
                );
                assert!(
                    snapshot
                        .notes()
                        .iter()
                        .find(|note| note.commitment() == received)
                        .is_some_and(|note| note.consumption().is_none()),
                    "the received note must remain unspent"
                );
                drop(snapshot);
                let primary = tip(&primary_index);
                let peer = tip(&peer_index);
                assert_eq!(
                    primary, report.through,
                    "primary prefix differs from report"
                );
                assert_eq!(peer, report.through, "peer prefix differs from report");
                primary_tips.push(primary);
                peer_tips.push(peer);
                break;
            }
            Err(_) => {
                pending += 1;
                assert!(
                    seller.snapshot().unwrap().spendable(&received).is_none(),
                    "a partial paired history revealed the received note"
                );
                let primary = tip(&primary_index);
                let peer = tip(&peer_index);
                assert!(
                    primary >= *primary_tips.last().unwrap() && peer >= *peer_tips.last().unwrap(),
                    "seller catch-up rewound a prefix: {primary}/{peer}"
                );
                primary_tips.push(primary);
                peer_tips.push(peer);
                assert!(Instant::now() < deadline, "seller catch-up stalled");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
    assert!(
        pending >= 1,
        "the cold recovery was never interrupted by bounded history"
    );
    assert_eq!(
        sends(&environment.primary),
        1,
        "seller recovery must never broadcast"
    );
    write_evidence(
        "cold-seller-recovery",
        &json!({
            "pending_cycles": pending,
            "primary_prefix_tips": primary_tips,
            "peer_prefix_tips": peer_tips,
            "received_note_spendable": true,
            "received_note_consumed": false,
            "send_count": sends(&environment.primary),
        }),
    );
}

/// A positive certified payment proves the deal settled at its anchor; it never proves the
/// consumed wallet note is still unspent and cannot authorize reserving or withdrawing it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil and generated M5 prototype artifacts; certificate is not spend authority"]
async fn cached_positive_certificate_never_authorizes_withdrawal_of_a_consumed_note() {
    let environment = Environment::new().await;
    let client = environment.deployed.client.clone();
    let (code, response, stderr) = environment.settle(Duration::from_secs(240)).await;
    assert!(stderr.is_empty(), "{stderr}");
    assert!(
        code == 2 && response["status"] == "pending",
        "first settle: {response}"
    );
    assert_eq!(sends(&environment.primary), 1, "{response}");
    mine(&client, &environment.deployed.url, 4).await;
    let verified = environment.complete_after_broadcast().await;
    assert_eq!(verified["payment_verified"], true, "{verified}");

    let wallet = environment.wallet_store();
    let deal_commitment: [u8; 32] = hex::decode(
        response["deal_commitment"]
            .as_str()
            .unwrap()
            .trim_start_matches("0x"),
    )
    .unwrap()
    .try_into()
    .unwrap();
    let commitment = {
        let snapshot = wallet.snapshot().unwrap();
        let note = snapshot
            .notes()
            .iter()
            .find(|note| note.asset() == environment.deployed.token_bytes && note.amount() == 150)
            .expect("buyer input note");
        assert!(
            note.consumption().is_some(),
            "the finalized payment must consume the buyer input"
        );
        note.commitment()
    };

    // A positive certificate for exactly the payment that consumed this input.
    let (primary_url, peer_url) = environment.rpc_urls();
    let cache = environment.root().join("certificate-cache");
    std::fs::create_dir(&cache).unwrap();
    let config = environment.config_value();
    let digest = |key: &str| -> [u8; 32] {
        hex::decode(config[key].as_str().unwrap().trim_start_matches("0x"))
            .unwrap()
            .try_into()
            .unwrap()
    };
    let pins = erebus_shielded_prover::rpc::PoolRuntimePins {
        pool: digest("runtime_keccak256"),
        poseidon: digest("poseidon_keccak256"),
        deposit: digest("deposit_verifier_keccak256"),
        transfer: digest("transfer_verifier_keccak256"),
        withdraw: digest("withdraw_verifier_keccak256"),
    };
    let certificate = erebus_shielded_prover::disclosure::verify_shielded_payment_certified(
        environment.selected(),
        &environment.pool_rpc(&primary_url),
        &erebus_shielded_prover::index_store::IndexStore::new(
            cache.join("primary.json"),
            environment.index_domain(),
        )
        .unwrap(),
        &environment.pool_rpc(&peer_url),
        &erebus_shielded_prover::index_store::IndexStore::new(
            cache.join("peer.json"),
            environment.index_domain(),
        )
        .unwrap(),
        pins,
    )
    .await
    .unwrap_or_else(|error| panic!("the finalized payment must certify: {error}"));
    assert_eq!(certificate.certificate.amount, 70);
    assert_eq!(
        certificate.certificate.commitment, deal_commitment,
        "the certificate must name the deal that consumed the wallet input"
    );

    // Holding the certificate does not make the consumed note spendable or reservable.
    let operation = [0xAB; 32];
    let snapshot = wallet.snapshot().unwrap();
    assert!(snapshot.spendable(&commitment).is_none());
    assert!(snapshot.spendable_for(&commitment, operation).is_none());
    drop(snapshot);
    let reserved = wallet.update(|wallet| wallet.reserve(&commitment, operation));
    assert!(
        reserved.is_err(),
        "a positive certificate authorized reserving a consumed note: {reserved:?}"
    );
    assert_eq!(
        sends(&environment.primary),
        1,
        "no withdrawal may be broadcast from a certificate"
    );
    write_evidence(
        "certificate-not-spend-authority",
        &json!({
            "certificate_amount": certificate.certificate.amount,
            "certificate_names_consumed_deal": true,
            "consumed_note_spendable": false,
            "reservation_refused": true,
            "send_count": sends(&environment.primary),
        }),
    );
}

/// While the primary's bounded scan is incomplete it reports HistoryPending before the peer's
/// exhausted reads are observed. The peer failure must eventually surface (never be masked
/// forever), both checkpoints must be preserved, and no failing call may write the wallet.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil and generated M5 prototype artifacts; pending must not mask peer failure"]
async fn primary_pending_does_not_permanently_mask_peer_read_failure() {
    let environment = Environment::new().await;
    let client = environment.deployed.client.clone();
    let (code, response, stderr) = environment.settle(Duration::from_secs(240)).await;
    assert!(stderr.is_empty(), "{stderr}");
    assert!(
        code == 2 && response["status"] == "pending",
        "first settle: {response}"
    );
    assert_eq!(sends(&environment.primary), 1, "{response}");
    mine(&client, &environment.deployed.url, 4).await;
    let verified = environment.complete_after_broadcast().await;
    assert_eq!(verified["payment_verified"], true, "{verified}");

    let seller = environment.seller_wallet_store();
    let received = {
        let snapshot = seller.snapshot().unwrap();
        snapshot
            .notes()
            .iter()
            .find(|note| note.asset() == environment.deployed.token_bytes && note.amount() == 70)
            .expect("received note")
            .commitment()
    };
    let seller_config: Value = serde_json::from_slice(
        &std::fs::read(&environment.deployed.fixture.seller_config).unwrap(),
    )
    .unwrap();
    let wallet_path = PathBuf::from(seller_config["seller_wallet_file"].as_str().unwrap());
    let wallet_before = std::fs::read(&wallet_path).unwrap();

    let (primary_url, peer_url) = environment.rpc_urls();
    let recovery_root = environment.root().join("masking-recovery");
    std::fs::create_dir(&recovery_root).unwrap();
    let primary_index = erebus_shielded_prover::index_store::IndexStore::new(
        recovery_root.join("primary.json"),
        environment.index_domain(),
    )
    .unwrap();
    let peer_index = erebus_shielded_prover::index_store::IndexStore::new(
        recovery_root.join("peer.json"),
        environment.index_domain(),
    )
    .unwrap();
    let primary_rpc = environment.pool_rpc(&primary_url);
    let peer_rpc = environment.pool_rpc(&peer_url);
    let tip = |index: &erebus_shielded_prover::index_store::IndexStore| {
        index
            .load()
            .unwrap()
            .tip()
            .map(|tip| tip.number)
            .unwrap_or(0)
    };
    mine(&client, &environment.deployed.url, 6).await;

    arm(
        &environment.peer,
        Behavior::FailReads {
            method: "eth_getLogs",
            remaining: Arc::new(AtomicUsize::new(999)),
        },
    );
    let mut masked = 0usize;
    let mut surfaced = false;
    let mut primary_tips = Vec::new();
    let mut peer_tips = Vec::new();
    for _ in 0..60 {
        match erebus_shielded_prover::recovery::recover_finalized_wallet_agreed_bounded(
            &primary_rpc,
            &primary_index,
            &peer_rpc,
            &peer_index,
            &seller,
            1,
        )
        .await
        {
            Ok(_) => break,
            Err(error) => {
                primary_tips.push(tip(&primary_index));
                peer_tips.push(tip(&peer_index));
                if matches!(
                    error,
                    erebus_shielded_prover::recovery::RecoveryError::HistoryPending { .. }
                ) {
                    masked += 1;
                } else {
                    surfaced = true;
                    break;
                }
            }
        }
        assert!(
            masked <= 40,
            "the primary pending result kept masking the peer failure: tips={primary_tips:?}"
        );
        assert_eq!(
            std::fs::read(&wallet_path).unwrap(),
            wallet_before,
            "a failing catch-up wrote the wallet"
        );
    }
    assert!(
        masked >= 1,
        "expected the primary pending result to mask at least one peer failure"
    );
    assert!(
        surfaced,
        "the peer failure must surface once the primary prefix is complete: tips={primary_tips:?}"
    );
    assert!(
        peer_tips.iter().all(|tip| *tip == 0),
        "the failing peer checkpoint must be retained, not advanced: {peer_tips:?}"
    );
    assert!(
        primary_tips.windows(2).all(|pair| pair[1] >= pair[0]),
        "the primary checkpoint rewound: {primary_tips:?}"
    );
    disarm(&environment.peer);
    let report = erebus_shielded_prover::recovery::recover_finalized_wallet_agreed_bounded(
        &primary_rpc,
        &primary_index,
        &peer_rpc,
        &peer_index,
        &seller,
        1000,
    )
    .await
    .unwrap_or_else(|error| panic!("healed catch-up must complete: {error}"));
    assert!(seller.snapshot().unwrap().spendable(&received).is_some());
    assert_eq!(
        sends(&environment.primary),
        1,
        "catch-up must not broadcast"
    );
    write_evidence(
        "pending-masks-peer",
        &json!({
            "masked_pending_cycles": masked,
            "peer_failure_surfaced": surfaced,
            "primary_prefix_tips": primary_tips,
            "peer_prefix_tips": peer_tips,
            "final_through": report.through,
            "received_note_spendable": true,
            "send_count": sends(&environment.primary),
        }),
    );
}

/// WS6 review M2: a claim saved under an older, larger gas limit resumes at that limit, so the
/// up-front funding check must use that limit, not the settle limit for new plans. With funds
/// above 3,010,000 x max_fee but below 5,000,000 x max_fee the settlement is refused as
/// `funding_required` before signing or broadcast, the claim stays, and once funded it is sent
/// at its saved limit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil and generated M5 prototype artifacts; resumed larger legacy plan"]
async fn a_resumed_larger_legacy_plan_is_funded_at_its_own_limit_before_any_send() {
    use erebus_evm::chain::{Eip1559Fees, EvmChain, SignerJournal};
    let environment = Environment::new().await;
    let client = environment.deployed.client.clone();
    let url = environment.deployed.url.clone();
    // The operator's legacy cap admits the pre-upgrade 5,000,000 plan.
    let mut config = environment.config_value();
    config["gas_limit"] = json!(5_000_000u64);
    std::fs::write(&environment.config, serde_json::to_vec(&config).unwrap()).unwrap();
    let signer_text = config["signer_address"].as_str().unwrap().to_owned();
    let signer: [u8; 20] = hex::decode(signer_text.trim_start_matches("0x"))
        .unwrap()
        .try_into()
        .unwrap();
    let max_fee: u128 = config["max_fee_per_gas"].as_str().unwrap().parse().unwrap();
    let fees = Eip1559Fees::new(
        max_fee,
        config["max_priority_fee_per_gas"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap(),
    )
    .unwrap();
    let journal = SignerJournal::open(
        config["signer_journal_root"].as_str().unwrap(),
        31337,
        signer,
    )
    .unwrap();
    let selected = environment.selected();
    let deployment = erebus_evm::deployment::EvmDeployment::new(
        selected.terms.domain.namespace.clone(),
        environment.pool(),
        selected.terms.domain.verifier_version,
        url.clone(),
    )
    .unwrap();

    // 1. Another operation holds the lane: settle proves, then refuses before any claim.
    let other = erebus_core::settlement::PreparedSettlement {
        operation_ref: [0x77; 32],
        deal_commitment: erebus_core::commitment::DealCommitment::from_bytes([0x77; 32]),
        deal_nullifier: erebus_core::commitment::DealNullifier::from_bytes([0x77; 32]),
        domain: selected.terms.domain.clone(),
        mode: SettlementMode::Shielded,
        required_guarantees: selected.terms.required_guarantees,
        backend_evidence: vec![1],
    };
    journal
        .reserve_call_with_value(
            &deployment,
            &other,
            environment.pool(),
            &[1],
            0,
            0,
            fees,
            3_000_000,
        )
        .unwrap();
    let (code, response, _) = environment.settle(Duration::from_secs(240)).await;
    assert_eq!(code, 1, "{response}");
    assert_eq!(
        response["error"], "signer lane is busy; sync the wallet and retry",
        "{response}"
    );
    assert_eq!(sends(&environment.primary) + sends(&environment.peer), 0);

    // 2. That operation's nonce is consumed and released; then a pre-upgrade process claims
    // this operation's prepared call at 5,000,000 and stops before signing.
    rpc(&client, &url, "anvil_setNonce", json!([signer_text, "0x1"])).await;
    mine(&client, &url, 4).await;
    let observer = EvmChain::connect(deployment.clone(), Duration::from_secs(10))
        .await
        .unwrap();
    let finalized = observer.verified_finalized_nonce(signer).await.unwrap();
    journal.release(&finalized).unwrap();
    let coordinator = Coordinator::open(
        environment.deployed.buyer_root.join("state/coordinator"),
        selected.terms.buyer_authorization_key.clone(),
        SettlementContext {
            domain: selected.terms.domain.clone(),
            asset: selected.terms.asset.clone(),
            suite_id: 2,
            mode: SettlementMode::Shielded,
            required_guarantees: selected.terms.required_guarantees,
            require_local_proving: true,
        },
        &SettlementContext {
            domain: selected.terms.domain.clone(),
            asset: selected.terms.asset.clone(),
            suite_id: 2,
            mode: SettlementMode::Shielded,
            required_guarantees: selected.terms.required_guarantees,
            require_local_proving: true,
        },
        &erebus_shielded_prover::preparation::capabilities(),
        SpendingPolicy {
            per_deal_max: BaseUnits::new(75),
            allowed_assets: [selected.terms.asset.clone()].into(),
            ..Default::default()
        },
    )
    .unwrap();
    let prepared = coordinator.prepared_settlement(OPERATION).unwrap();
    drop(coordinator);
    journal
        .reserve_call_with_value(
            &deployment,
            &prepared,
            environment.pool(),
            &prepared.backend_evidence,
            0,
            1,
            fees,
            5_000_000,
        )
        .unwrap();

    // 3. Funds cover a new 3,010,000 plan but not this 5,000,000 one: refused before signing.
    let between = 4_000_000u128 * max_fee;
    rpc(
        &client,
        &url,
        "anvil_setBalance",
        json!([signer_text, format!("0x{between:x}")]),
    )
    .await;
    mine(&client, &url, 2).await;
    let (code, response, _) = environment.settle(Duration::from_secs(240)).await;
    assert_eq!(code, 2, "{response}");
    assert_eq!(response["status"], "funding_required", "{response}");
    assert_eq!(
        response["funding"]["signer_shortfall"],
        (5_000_000u128 * max_fee - between).to_string(),
        "{response}"
    );
    assert_eq!(response["submitted_this_call"], false, "{response}");
    assert_eq!(sends(&environment.primary) + sends(&environment.peer), 0);
    assert_eq!(
        journal.claim_nonce().unwrap(),
        Some(1),
        "the claim must be kept"
    );

    // 4. Funded at its own limit, the saved plan is signed and sent at 5,000,000.
    let enough = 6_000_000u128 * max_fee;
    rpc(
        &client,
        &url,
        "anvil_setBalance",
        json!([signer_text, format!("0x{enough:x}")]),
    )
    .await;
    mine(&client, &url, 2).await;
    let (code, response, _) = environment.settle(Duration::from_secs(240)).await;
    assert!(code == 0 || code == 2, "{response}");
    assert_eq!(response["submitted_this_call"], true, "{response}");
    let hash = response["transaction_hash"].as_str().unwrap().to_owned();
    let sent = rpc(&client, &url, "eth_getTransactionByHash", json!([hash])).await;
    assert_eq!(sent["gas"], "0x4c4b40", "{sent}");
}
