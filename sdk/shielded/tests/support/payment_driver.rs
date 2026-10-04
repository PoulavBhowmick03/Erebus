//! Real chain evidence for copied negotiation and payment commands. Public-bound only.

use super::*;
use axum::{extract::State, routing::post, Json, Router};
use base64::{engine::general_purpose::STANDARD, Engine};
use erebus_core::{
    domain::DeploymentDomain,
    ids::{AddressBytes, AssetId, ChainNamespace},
    suite::keccak256,
};
use erebus_evm::x402::{EXACT_PERMIT2_PROXY, PERMIT2};
use erebus_evm::{
    abi,
    chain::{EvmChain, HistoricalObservation, ObservationJournal, ObservationLimits},
    deployment::EvmDeployment,
};
use erebus_shielded_prover::access::x402::payment_signature_header;
use erebus_shielded_prover::access::{issuance_id, request_digest, AccessRequest, X402Payment};
use sha2::{Digest, Sha256};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

struct ServiceProcess(Child);
#[path = "agent_driver.rs"]
mod agent_driver;
#[path = "native_product.rs"]
mod native_product;
#[path = "shielded_driver.rs"]
mod shielded_driver;
#[path = "x402_agents.rs"]
mod x402_agents;
impl Drop for ServiceProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[derive(Clone)]
pub(super) struct Proxy {
    upstream: String,
    client: reqwest::Client,
    lose_broadcast: Arc<AtomicBool>,
    sends: Arc<AtomicUsize>,
    minimum_log_block: Arc<std::sync::Mutex<Option<u64>>>,
    corrupt_code: Arc<AtomicBool>,
    corrupt_consumed: Arc<AtomicBool>,
    broadcast_hashes: Arc<std::sync::Mutex<Vec<[u8; 32]>>>,
}

async fn forward(State(proxy): State<Proxy>, Json(request): Json<Value>) -> Json<Value> {
    if request["method"] == "eth_getLogs" {
        if let Some(from) = request["params"][0]["fromBlock"].as_str() {
            let height = u64::from_str_radix(from.trim_start_matches("0x"), 16).unwrap();
            let mut minimum = proxy.minimum_log_block.lock().unwrap();
            *minimum = Some(minimum.map_or(height, |previous| previous.min(height)));
        }
    }
    let sent = request["method"] == "eth_sendRawTransaction";
    if sent {
        proxy.sends.fetch_add(1, Ordering::SeqCst);
        if let Some(raw) = request["params"][0].as_str() {
            if let Ok(bytes) = hex::decode(raw.trim_start_matches("0x")) {
                proxy
                    .broadcast_hashes
                    .lock()
                    .unwrap()
                    .push(keccak256(&[&bytes]));
            }
        }
    }
    let mut response: Value = proxy
        .client
        .post(&proxy.upstream)
        .json(&request)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    if sent && proxy.lose_broadcast.load(Ordering::SeqCst) {
        return Json(json!({"this_is_not":"a broadcast acknowledgement"}));
    }
    if request["method"] == "eth_getCode" && proxy.corrupt_code.load(Ordering::SeqCst) {
        response["result"] = json!("0x00");
    }
    if request["method"] == "eth_call"
        && proxy.corrupt_consumed.load(Ordering::SeqCst)
        && request["params"][0]["data"].as_str().is_some_and(|data| {
            data.starts_with(&format!(
                "0x{}",
                hex::encode(&abi::encode_consumed_deals_call(&[0; 32])[..4])
            )) || data.starts_with(&format!(
                "0x{}",
                hex::encode(&keccak256(&[b"consumedDeals(uint256)"])[..4])
            ))
        })
    {
        response["result"] = json!(format!("0x{}", "00".repeat(32)));
    }
    Json(response)
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

async fn send(
    client: &reqwest::Client,
    url: &str,
    from: &str,
    to: Option<&str>,
    data: Vec<u8>,
) -> Value {
    let mut tx = json!({"from":from,"data":format!("0x{}",hex::encode(data)),"gas":"0x7a1200"});
    if let Some(to) = to {
        tx["to"] = json!(to);
    }
    let hash = rpc(client, url, "eth_sendTransaction", json!([tx])).await;
    for _ in 0..100 {
        let receipt = rpc(client, url, "eth_getTransactionReceipt", json!([hash])).await;
        if !receipt.is_null() {
            assert_eq!(receipt["status"], "0x1", "test transaction: {receipt}");
            return receipt;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("receipt missing");
}

async fn deploy(
    client: &reqwest::Client,
    url: &str,
    from: &str,
    name: &str,
    args: Vec<u8>,
) -> Value {
    let artifact: Value = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../../contracts/evm/out/{name}.sol/{name}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    let mut code = hex::decode(
        artifact["bytecode"]["object"]
            .as_str()
            .unwrap()
            .trim_start_matches("0x"),
    )
    .unwrap();
    code.extend(args);
    send(client, url, from, None, code).await
}

async fn run_payment(binary: &Path, cwd: &Path, config: &Path, method: &str) -> (bool, Value) {
    finish_with_timeout(
        spawn(
            binary,
            cwd,
            json!({"method":method,"config_file":config,"operation_ref":hex::encode(OPERATION)}),
        ),
        Duration::from_secs(180),
    )
}

/// A deployed public-bound settlement and token, negotiated configs for both participants, and
/// the buyer's payment configuration behind an RPC proxy. Nothing is negotiated or funded yet.
pub(super) struct PublicDeployed {
    pub(super) fixture: Fixture,
    pub(super) client: reqwest::Client,
    pub(super) rpc_url: String,
    pub(super) proxy: Proxy,
    pub(super) proxy_url: String,
    pub(super) contract: String,
    pub(super) token: String,
    pub(super) deployer: String,
    pub(super) first_block: u64,
    pub(super) binary: PathBuf,
    pub(super) key_path: PathBuf,
    pub(super) config: Value,
    pub(super) config_path: PathBuf,
    pub(super) gas_address: String,
    pub(super) buyer_address: String,
    pub(super) task: tokio::task::JoinHandle<()>,
    _anvil: ServiceProcess,
}

pub(super) async fn deploy_public(block_time: Option<u64>) -> PublicDeployed {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let anvil = ServiceProcess(
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
            .args(
                block_time
                    .map(|seconds| ["--block-time".to_string(), seconds.to_string()])
                    .into_iter()
                    .flatten(),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let rpc_url = format!("http://127.0.0.1:{port}");
    for _ in 0..100 {
        if client
            .post(&rpc_url)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}))
            .send()
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    let accounts = rpc(&client, &rpc_url, "eth_accounts", json!([])).await;
    let deployer = accounts[0].as_str().unwrap();
    // A large unrelated prefix makes a mistaken genesis scan visible in the proxy.
    rpc(&client, &rpc_url, "anvil_mine", json!(["0x80"])).await;
    let deployed = deploy(
        &client,
        &rpc_url,
        deployer,
        "ErebusSettlement",
        abi::encode_settlement_constructor(31337, 1),
    )
    .await;
    let contract = deployed["contractAddress"].as_str().unwrap();
    let first_block = u64::from_str_radix(
        deployed["blockNumber"]
            .as_str()
            .unwrap()
            .trim_start_matches("0x"),
        16,
    )
    .unwrap();
    let first_hash = deployed["blockHash"].as_str().unwrap();
    let token_receipt = deploy(
        &client,
        &rpc_url,
        deployer,
        "MockERC20",
        abi::encode_token_constructor("Test", "TEST"),
    )
    .await;
    let token = token_receipt["contractAddress"].as_str().unwrap();
    let code = rpc(
        &client,
        &rpc_url,
        "eth_getCode",
        json!([contract, "latest"]),
    )
    .await;
    let runtime =
        keccak256(&[&hex::decode(code.as_str().unwrap().trim_start_matches("0x")).unwrap()]);
    let mut template = terms(1, 60);
    template.domain = DeploymentDomain {
        namespace: ChainNamespace::parse("eip155:31337").unwrap(),
        settlement_contract: Some(
            AddressBytes::new(hex::decode(contract.trim_start_matches("0x")).unwrap()).unwrap(),
        ),
        pool: None,
        verifier_version: 1,
    };
    template.asset = AssetId::parse(&format!("eip155:31337/erc20:{token}")).unwrap();
    template.service.resource = "dataset.snapshot.v1".into();
    template.service.unit = "snapshot".into();
    template.service.quantity = BaseUnits::new(1);
    template.service.fulfillment_method = "http-access-v1".into();
    template.service.fulfillment_digest = Sha256::digest(native_product::PAYLOAD).into();
    let fixture = Fixture::with_terms(template);
    let binary = fixture.root.path().join("erebus-payment");
    fs::copy(env!("CARGO_BIN_EXE_erebus-payment"), &binary).unwrap();
    let gas_seed = [24; 32];
    let gas = AuthorizationIdentity::from_bytes(&gas_seed).unwrap();
    let gas_address = format!("0x{}", hex::encode(gas.address()));
    let buyer_address = format!(
        "0x{}",
        hex::encode(
            AuthorizationIdentity::from_bytes(&[21; 32])
                .unwrap()
                .address()
        )
    );
    let key_path = fixture.root.path().join("buyer/gas.key");
    private(&key_path, &gas_seed);
    let proxy = Proxy {
        upstream: rpc_url.clone(),
        client: client.clone(),
        lose_broadcast: Arc::new(AtomicBool::new(false)),
        sends: Arc::new(AtomicUsize::new(0)),
        minimum_log_block: Arc::new(std::sync::Mutex::new(None)),
        corrupt_code: Arc::new(AtomicBool::new(false)),
        corrupt_consumed: Arc::new(AtomicBool::new(false)),
        broadcast_hashes: Arc::new(std::sync::Mutex::new(Vec::new())),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_url = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/", post(forward))
        .with_state(proxy.clone());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let config_path = fixture.root.path().join("buyer/payment.json");
    let config = json!({"version":1,"state_root":fixture.root.path().join("buyer/state"),"namespace":"eip155:31337",
        "settlement_contract":contract,"verifier_version":1,"runtime_keccak256":hex::encode(runtime),"first_block":first_block,"first_hash":first_hash,
        "rpc_url":proxy_url,"peer_rpc_url":rpc_url,"buyer_address":buyer_address,"asset":fixture.template.asset.to_string(),
        "signer_address":gas_address,"signer_journal_root":fixture.root.path().join("operator-signer"),"transaction_key_file":key_path,
        "maximum_price":"75","gas_limit":500_000,"max_fee_per_gas":"3000000000","max_priority_fee_per_gas":"1000000000",
        "timeout_seconds":2,"log_block_range":100,"max_log_queries":1,"max_ancestry":64});
    private(&config_path, config.to_string().as_bytes());
    PublicDeployed {
        fixture,
        client,
        rpc_url,
        proxy,
        proxy_url,
        contract: contract.to_string(),
        token: token.to_string(),
        deployer: deployer.to_string(),
        first_block,
        binary,
        key_path,
        config,
        config_path,
        gas_address,
        buyer_address,
        task,
        _anvil: anvil,
    }
}

/// Gives the gas payer native funds and the buyer 70 tokens approved to the settlement contract.
pub(super) async fn fund_public(deployed: &PublicDeployed) {
    let PublicDeployed {
        client,
        rpc_url,
        contract,
        token,
        deployer,
        gas_address,
        buyer_address,
        ..
    } = deployed;
    let (contract, token, deployer) = (contract.as_str(), token.as_str(), deployer.as_str());
    rpc(
        client,
        rpc_url,
        "anvil_setBalance",
        json!([gas_address, "0xde0b6b3a7640000"]),
    )
    .await;
    send(
        client,
        rpc_url,
        deployer,
        Some(token),
        abi::encode_mint_call(
            &AuthorizationIdentity::from_bytes(&[21; 32])
                .unwrap()
                .address(),
            70,
        ),
    )
    .await;
    rpc(
        client,
        rpc_url,
        "anvil_impersonateAccount",
        json!([buyer_address]),
    )
    .await;
    rpc(
        client,
        rpc_url,
        "anvil_setBalance",
        json!([buyer_address, "0xde0b6b3a7640000"]),
    )
    .await;
    send(
        client,
        rpc_url,
        buyer_address,
        Some(token),
        abi::encode_approve_call(
            &hex::decode(contract.trim_start_matches("0x"))
                .unwrap()
                .try_into()
                .unwrap(),
            70,
        ),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil and built EVM artifacts; copied commands submit public-bound test payments"]
async fn negotiated_payment_recovers_a_lost_broadcast_without_keys_or_a_second_send() {
    let deployed = deploy_public(None).await;
    let (buyer, seller) = deployed.fixture.pair(false);
    assert_eq!(buyer["deal_commitment"], seller["deal_commitment"]);
    let PublicDeployed {
        fixture,
        client,
        rpc_url,
        proxy,
        proxy_url,
        contract,
        token,
        first_block,
        binary,
        key_path,
        config,
        config_path,
        ..
    } = &deployed;
    let (contract, token, first_block) = (contract.as_str(), token.as_str(), *first_block);
    rpc(client, rpc_url, "anvil_mine", json!(["0x3"])).await;
    let (_, unfunded) = run_payment(binary, fixture.root.path(), config_path, "funding").await;
    assert_eq!(unfunded["status"], "funding_required", "{unfunded}");
    assert_eq!(unfunded["funding"]["allowance_shortfall"], "70");
    assert_eq!(unfunded["funding"]["balance_shortfall"], "70");
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 0);
    fund_public(&deployed).await;
    rpc(client, rpc_url, "anvil_mine", json!(["0x3"])).await;
    let (ok, ready) = run_payment(binary, fixture.root.path(), config_path, "funding").await;
    assert!(ok, "{ready}");
    assert_eq!(ready["status"], "ready");
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 0);
    let held = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture.root.path().join("buyer/state/.payment-driver.lock"))
        .unwrap();
    fs2::FileExt::try_lock_exclusive(&held).unwrap();
    let (ok, busy) = run_payment(binary, fixture.root.path(), config_path, "settle").await;
    assert!(!ok);
    assert_eq!(busy["error"], "another payment command is active");
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 0);
    drop(held);
    // Invalid runtime data fails before signing/broadcast and cannot release policy capacity.
    proxy.corrupt_code.store(true, Ordering::SeqCst);
    let (ok, corrupted) = run_payment(binary, fixture.root.path(), config_path, "settle").await;
    assert!(!ok);
    assert_eq!(corrupted["status"], "error");
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 0);
    proxy.corrupt_code.store(false, Ordering::SeqCst);
    // A shifted start at a later canonical block must also be rejected.
    let later = rpc(
        client,
        rpc_url,
        "eth_getBlockByNumber",
        json!([format!("0x{:x}", first_block + 1), false]),
    )
    .await;
    let mut invalid = config.clone();
    invalid["first_block"] = json!(first_block + 1);
    invalid["first_hash"] = later["hash"].clone();
    let invalid_path = fixture.root.path().join("buyer/late-deployment.json");
    private(&invalid_path, invalid.to_string().as_bytes());
    let (ok, late) = run_payment(binary, fixture.root.path(), &invalid_path, "settle").await;
    assert!(!ok);
    assert_eq!(late["status"], "error");
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 0);
    proxy.lose_broadcast.store(true, Ordering::SeqCst);
    let (_, submitted) = run_payment(binary, fixture.root.path(), config_path, "settle").await;
    assert_eq!(submitted["submitted_this_call"], true, "{submitted}");
    assert_eq!(submitted["payment_verified"], false);
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 1);
    // Further settle calls are observation only, even while finality is outstanding.
    let (_, pending) = run_payment(binary, fixture.root.path(), config_path, "settle").await;
    assert_eq!(pending["submitted_this_call"], false);
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 1);
    fs::remove_file(key_path).unwrap();
    fs::rename(
        fixture.root.path().join("buyer/state/transcripts"),
        fixture.root.path().join("buyer/retained-transcripts"),
    )
    .unwrap();
    rpc(client, rpc_url, "anvil_mine", json!(["0x3"])).await;
    proxy.corrupt_consumed.store(true, Ordering::SeqCst);
    let (ok, disagreement) = run_payment(binary, fixture.root.path(), config_path, "observe").await;
    assert!(!ok, "{disagreement}");
    assert_eq!(disagreement["payment_verified"], false);
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 1);
    proxy.corrupt_consumed.store(false, Ordering::SeqCst);
    let (ok, recovered) = run_payment(binary, fixture.root.path(), config_path, "observe").await;
    assert!(ok, "{recovered}");
    assert_eq!(recovered["payment_verified"], true);
    assert_eq!(recovered["delivery_verified"], false);
    assert_eq!(recovered["submitted_this_call"], false);
    assert_eq!(recovered["nonce_cleanup_pending"], false);
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 1);
    assert_eq!(recovered["deal_commitment"], buyer["deal_commitment"]);
    assert_eq!(*proxy.minimum_log_block.lock().unwrap(), Some(first_block));
    let balance = rpc(client,rpc_url,"eth_call",json!([{"to":token,"data":format!("0x{}",hex::encode(abi::encode_balance_of_call(&AuthorizationIdentity::from_bytes(&[22;32]).unwrap().address())))},"latest"])).await;
    assert_eq!(
        u128::from_str_radix(balance.as_str().unwrap().trim_start_matches("0x"), 16).unwrap(),
        70
    );
    // Paired scans yield and resume from the same authenticated bound under a tiny budget.
    rpc(client, rpc_url, "anvil_mine", json!(["0x3"])).await;
    let deployment = EvmDeployment::new(
        ChainNamespace::parse("eip155:31337").unwrap(),
        hex::decode(contract.trim_start_matches("0x"))
            .unwrap()
            .try_into()
            .unwrap(),
        1,
        proxy_url,
    )
    .unwrap();
    let first = EvmChain::connect(deployment.clone(), Duration::from_secs(2))
        .await
        .unwrap();
    let mut deployment_peer = deployment;
    deployment_peer.rpc_url = rpc_url.clone();
    let second = EvmChain::connect(deployment_peer, Duration::from_secs(2))
        .await
        .unwrap();
    let cursor = ObservationJournal::open(fixture.root.path().join("tiny-primary")).unwrap();
    let peer_cursor = ObservationJournal::open(fixture.root.path().join("tiny-peer")).unwrap();
    let selected =
        SelectedAgreement::decode(&fs::read(buyer["evidence_file"].as_str().unwrap()).unwrap())
            .unwrap();
    let nullifier = erebus_core::commitment::deal_nullifier(&selected.terms).unwrap();
    let tiny = ObservationLimits {
        log_block_range: 1,
        max_log_queries: 1,
        max_ancestry: 1,
    };
    let mut pending_count = 0;
    for _ in 0..100 {
        match first
            .finalized_deal_evidence_resumable_agreed_from(
                &cursor,
                &second,
                &peer_cursor,
                &nullifier,
                first_block,
                tiny,
            )
            .await
            .unwrap()
        {
            HistoricalObservation::Pending { .. } => pending_count += 1,
            HistoricalObservation::Complete { .. } => break,
        }
    }
    assert!(pending_count > 0 && pending_count < 99);
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 1);
    for field in [
        "amount",
        "terms",
        "signature",
        "rpc_url",
        "seed",
        "blinding",
    ] {
        assert!(recovered.get(field).is_none());
    }
    native_product::finish(fixture, &buyer, &seller, config, client).await;
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 1);
    deployed.task.abort();
}

/// Installs the canonical Permit2 and exact-proxy runtimes, deploys the test token, and starts
/// the RPC proxy used for x402 facilitator submission.
async fn deploy_x402_chain() -> (
    reqwest::Client,
    String,
    ServiceProcess,
    Proxy,
    String,
    String,
    [u8; 32],
    [u8; 32],
) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let anvil = ServiceProcess(
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
    let rpc_url = format!("http://127.0.0.1:{port}");
    for _ in 0..100 {
        if client
            .post(&rpc_url)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}))
            .send()
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    let runtime: Value = serde_json::from_str(include_str!(
        "../../../evm/tests/fixtures/x402-canonical-runtime.json"
    ))
    .unwrap();
    for name in ["permit2", "x402_exact_permit2_proxy"] {
        rpc(
            &client,
            &rpc_url,
            "anvil_setCode",
            json!([
                runtime["contracts"][name]["address"],
                runtime["contracts"][name]["runtime"]
            ]),
        )
        .await;
    }
    let runtime_hash = |name: &str| -> [u8; 32] {
        hex::decode(
            runtime["contracts"][name]["runtime_keccak256"]
                .as_str()
                .unwrap()
                .trim_start_matches("0x"),
        )
        .unwrap()
        .try_into()
        .unwrap()
    };
    let accounts = rpc(&client, &rpc_url, "eth_accounts", json!([])).await;
    let deployer = accounts[0].as_str().unwrap().to_string();
    let token_receipt = deploy(
        &client,
        &rpc_url,
        &deployer,
        "MockERC20",
        abi::encode_token_constructor("Test", "TEST"),
    )
    .await;
    let token = token_receipt["contractAddress"]
        .as_str()
        .unwrap()
        .to_string();
    let proxy = Proxy {
        upstream: rpc_url.clone(),
        client: client.clone(),
        lose_broadcast: Arc::new(AtomicBool::new(true)),
        sends: Arc::new(AtomicUsize::new(0)),
        minimum_log_block: Arc::new(std::sync::Mutex::new(None)),
        corrupt_code: Arc::new(AtomicBool::new(false)),
        corrupt_consumed: Arc::new(AtomicBool::new(false)),
        broadcast_hashes: Arc::new(std::sync::Mutex::new(Vec::new())),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_url = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/", post(forward))
        .with_state(proxy.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (
        client,
        rpc_url,
        anvil,
        proxy,
        proxy_url,
        token,
        runtime_hash("permit2"),
        runtime_hash("x402_exact_permit2_proxy"),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil and built EVM artifacts; copied commands submit x402 test payments"]
async fn negotiated_x402_settles_once_recovers_restart_and_audits_from_the_grant() {
    let (client, rpc_url, _anvil, proxy, proxy_url, token, permit2_hash, proxy_hash) =
        deploy_x402_chain().await;
    let buyer_seed = [21; 32];
    let buyer_identity = AuthorizationIdentity::from_bytes(&buyer_seed).unwrap();
    let seller_identity = AuthorizationIdentity::from_bytes(&[22; 32]).unwrap();
    let seller_address = format!("0x{}", hex::encode(seller_identity.address()));
    let buyer_address = format!("0x{}", hex::encode(buyer_identity.address()));
    let gas_seed = [24; 32];
    let gas_address = format!(
        "0x{}",
        hex::encode(
            AuthorizationIdentity::from_bytes(&gas_seed)
                .unwrap()
                .address()
        )
    );

    // Real authenticated discovery and encrypted negotiation between separate processes.
    let mut template = terms(1, 60);
    template.domain = DeploymentDomain {
        namespace: ChainNamespace::parse("eip155:31337").unwrap(),
        settlement_contract: Some(AddressBytes::new(EXACT_PERMIT2_PROXY.to_vec()).unwrap()),
        pool: None,
        verifier_version: 1,
    };
    template.asset = AssetId::parse(&format!("eip155:31337/erc20:{token}")).unwrap();
    template.service.resource = "dataset.snapshot.v1".into();
    template.service.unit = "snapshot".into();
    template.service.quantity = BaseUnits::new(1);
    template.service.fulfillment_method = "http-access-v1".into();
    template.service.fulfillment_digest = Sha256::digest(native_product::PAYLOAD).into();
    let fixture = Fixture::with_terms(template);
    let publication = fixture.root.path().join("seller/access-evidence");
    fs::create_dir(&publication).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&publication, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut seller_config: Value =
        serde_json::from_slice(&fs::read(&fixture.seller_config).unwrap()).unwrap();
    seller_config["access_evidence_root"] = json!(publication);
    fs::write(&fixture.seller_config, seller_config.to_string()).unwrap();
    let negotiation_start = Instant::now();
    let (buyer, seller) = fixture.pair(false);
    let negotiation = negotiation_start.elapsed();
    assert_eq!(buyer["deal_commitment"], seller["deal_commitment"]);
    let published = publication.join(format!(
        "{}.evidence",
        seller["deal_commitment"].as_str().unwrap()
    ));
    assert_eq!(
        fs::read(&published).unwrap(),
        fs::read(seller["evidence_file"].as_str().unwrap()).unwrap()
    );

    // Buyer onboarding: native funds and one Permit2 approval. No approval is signed here.
    rpc(
        &client,
        &rpc_url,
        "anvil_setBalance",
        json!([gas_address, "0xde0b6b3a7640000"]),
    )
    .await;
    rpc(
        &client,
        &rpc_url,
        "anvil_impersonateAccount",
        json!([buyer_address]),
    )
    .await;
    rpc(
        &client,
        &rpc_url,
        "anvil_setBalance",
        json!([buyer_address, "0xde0b6b3a7640000"]),
    )
    .await;
    send(
        &client,
        &rpc_url,
        &buyer_address,
        Some(&token),
        abi::encode_mint_call(&buyer_identity.address(), 70),
    )
    .await;
    send(
        &client,
        &rpc_url,
        &buyer_address,
        Some(&token),
        abi::encode_approve_call(&PERMIT2, 70),
    )
    .await;

    let selected =
        SelectedAgreement::decode(&fs::read(buyer["evidence_file"].as_str().unwrap()).unwrap())
            .unwrap();
    verify_selected_agreement(&selected).unwrap();
    assert_eq!(
        selected.terms.buyer_authorization_key.as_bytes(),
        buyer_identity.address()
    );
    assert_eq!(
        selected.terms.seller_authorization_key.as_bytes(),
        seller_identity.address()
    );
    assert_eq!(
        selected.terms.payment_recipient.as_bytes(),
        seller_identity.address()
    );

    let service_id = [42; 32];
    let service_port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let url = format!("http://127.0.0.1:{service_port}");
    let payload = fixture.root.path().join("seller/payload");
    private(&payload, native_product::PAYLOAD);
    let gas_key = fixture.root.path().join("seller/x402-gas.key");
    private(&gas_key, &gas_seed);
    let service_config = fixture.root.path().join("seller/x402-access.json");
    private(
        &service_config,
        json!({"service_id":service_id,"seller_key":seller_identity.address(),"suite_id":1,
            "resource":selected.terms.service.resource,"payload_file":payload,"evidence_root":publication,
            "state_root":fixture.root.path().join("seller/x402-state"),"port":service_port,
            "backend":{"mode":"x402_exact","namespace":"eip155:31337","rpc_url":proxy_url,"peer_rpc_url":rpc_url,
                "permit2_runtime_hash":permit2_hash,"proxy_runtime_hash":proxy_hash,
                "transaction_key_file":gas_key,"signer_journal_root":fixture.root.path().join("operator-signer"),
                "gas_limit":500000,"max_fee_per_gas":"3000000000","max_priority_fee_per_gas":"1000000000"}})
            .to_string()
            .as_bytes(),
    );
    let process = native_product::service(&service_config, &url, &client).await;

    let cache = fixture.root.path().join("buyer/x402-cache");
    let access_bin = native_binary("erebus-access", env!("CARGO_BIN_EXE_erebus-access"));
    let preparation_start = Instant::now();
    let (ok, unavailable) = finish(spawn(
        &access_bin,
        fixture.root.path(),
        json!({
            "method":"retrieve", "evidence_file":buyer["evidence_file"],
            "buyer_key_file":fixture.root.path().join("buyer/agreement.key"),
            "service_url":"http://127.0.0.1:1/v1/access", "service_id":hex::encode(service_id),
            "cache_root":cache, "allow_loopback_http":true, "x402_exact":true
        }),
    ));
    assert!(
        !ok && unavailable["payment_verified"] != true
            && unavailable["retry_without_payment"] == true,
        "{unavailable}"
    );
    let commitment: [u8; 32] = hex::decode(buyer["deal_commitment"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let saved: Value = serde_json::from_slice(
        &fs::read(
            cache
                .join("x402-authorizations")
                .join(format!("{}.json", issuance_id(&service_id, &commitment))),
        )
        .unwrap(),
    )
    .unwrap();
    let payment: X402Payment = serde_json::from_value(saved["payment"].clone()).unwrap();
    let permit_preparation = preparation_start.elapsed();
    let expires_at = clock() + 120;
    let signature = buyer_identity
        .sign_digest(
            &request_digest(&selected, service_id, [43; 32], expires_at, Some(&payment)).unwrap(),
        )
        .to_vec();
    let request = AccessRequest {
        deal_commitment: buyer["deal_commitment"].as_str().unwrap().into(),
        nonce: [43; 32],
        expires_at,
        signature,
        payment: Some(payment.clone()),
    };
    let header = payment_signature_header(&selected.terms, &payment).unwrap();
    // A request signed without a payment block is challenged, not charged: the signature is
    // bound to the payment block, so it cannot authorize one.
    let ordinary_expires = clock() + 120;
    let ordinary = client
        .post(format!("{url}/v1/access"))
        .json(&AccessRequest {
            deal_commitment: buyer["deal_commitment"].as_str().unwrap().into(),
            nonce: [43; 32],
            expires_at: ordinary_expires,
            signature: buyer_identity
                .sign_digest(
                    &request_digest(&selected, service_id, [43; 32], ordinary_expires, None)
                        .unwrap(),
                )
                .to_vec(),
            payment: None,
        })
        .send()
        .await
        .unwrap();
    assert_eq!(ordinary.status(), 402);
    assert!(ordinary.headers().contains_key("PAYMENT-REQUIRED"));
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 0);

    let submission_start = Instant::now();
    let first = client
        .post(format!("{url}/v1/access"))
        .header("PAYMENT-SIGNATURE", &header)
        .json(&request)
        .send()
        .await
        .unwrap();
    assert!(first.status() == 202 || first.status() == 200, "{first:?}");
    drop(first);
    let transaction_hash = {
        let hashes = proxy.broadcast_hashes.lock().unwrap();
        assert_eq!(hashes.len(), 1, "exactly one x402 broadcast");
        hashes[0]
    };
    let transaction_hash = format!("0x{}", hex::encode(transaction_hash));
    // First observed inclusion is a local monotonic observation, not a block-timestamp delta.
    let first_inclusion = loop {
        let receipt = rpc(
            &client,
            &rpc_url,
            "eth_getTransactionReceipt",
            json!([transaction_hash]),
        )
        .await;
        if !receipt.is_null() {
            break submission_start.elapsed();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };

    drop(process);
    let _restarted = native_product::service(&service_config, &url, &client).await;
    let mut finalized_verification = None;
    for _ in 0..40 {
        let response = client
            .post(format!("{url}/v1/access"))
            .header("PAYMENT-SIGNATURE", &header)
            .json(&request)
            .send()
            .await
            .unwrap();
        if response.status() == 200 {
            let payment_response = response
                .headers()
                .get("PAYMENT-RESPONSE")
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            let receipt: Value =
                serde_json::from_slice(&STANDARD.decode(payment_response).unwrap()).unwrap();
            assert_eq!(receipt["transaction"], transaction_hash);
            finalized_verification = Some(submission_start.elapsed());
            let issued: Value = response.json().await.unwrap();
            let repeated: Value = client
                .post(format!("{url}/v1/access"))
                .header("PAYMENT-SIGNATURE", &header)
                .json(&request)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(repeated["issuance"]["repeated"], true);
            assert_eq!(
                issued["issuance"]["issuance_id"],
                repeated["issuance"]["issuance_id"]
            );
            break;
        }
        assert_eq!(response.status(), 202, "{response:?}");
        rpc(&client, &rpc_url, "anvil_mine", json!(["0x1"])).await;
    }
    let finalized_verification = finalized_verification.expect("finalized x402 verification");

    let access_bin = fixture.root.path().join("erebus-access");
    fs::copy(
        native_binary("erebus-access", env!("CARGO_BIN_EXE_erebus-access")),
        &access_bin,
    )
    .unwrap();
    let retrieval = json!({"method":"retrieve","evidence_file":buyer["evidence_file"],
        "buyer_key_file":fixture.root.path().join("buyer/agreement.key"),
        "service_url":format!("{url}/v1/access"),"service_id":hex::encode(service_id),
        "cache_root":cache,"allow_loopback_http":true,
        "x402_exact":true});
    let delivery_start = Instant::now();
    let (ok, resource) = finish(spawn(&access_bin, fixture.root.path(), retrieval));
    let delivery = delivery_start.elapsed();
    assert!(ok, "{resource}");
    assert_eq!(resource["result"]["resource_verified"], true);
    assert_eq!(resource["result"]["payment_verified"], false);
    assert_eq!(resource["result"]["delivery_verified"], false);
    assert_eq!(
        fs::read(resource["result"]["resource_file"].as_str().unwrap()).unwrap(),
        native_product::PAYLOAD
    );
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 1);

    if let (Ok(python), Ok(_)) = (
        std::env::var("EREBUS_TEST_MCP_PYTHON"),
        std::env::var("EREBUS_TEST_INSTALLED_BIN_DIR"),
    ) {
        let script =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../mcp-server/tests/access_probe.py");
        let output = Command::new(python)
            .arg("-I")
            .arg(script)
            .arg(buyer["evidence_file"].as_str().unwrap())
            .arg(fixture.root.path().join("buyer/agreement.key"))
            .arg(format!("{url}/v1/access"))
            .arg(hex::encode(service_id))
            .arg(&cache)
            .arg("installed")
            .arg("x402_cached")
            .current_dir(fixture.root.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["mcp_access_verified"], true);
        assert_eq!(result["source_imports"], false);
    }
    let balance = rpc(&client,&rpc_url,"eth_call",json!([{"to":token,"data":format!("0x{}",hex::encode(abi::encode_balance_of_call(&seller_identity.address())))},"latest"])).await;
    assert_eq!(
        u128::from_str_radix(balance.as_str().unwrap().trim_start_matches("0x"), 16).unwrap(),
        selected.terms.amount.get()
    );

    // Independent auditor: encrypted grant, auditor key, and public configuration only.
    let disclosure = fixture.root.path().join("erebus-disclosure");
    fs::copy(
        native_binary(
            "erebus-shielded-disclosure",
            env!("CARGO_BIN_EXE_erebus-shielded-disclosure"),
        ),
        &disclosure,
    )
    .unwrap();
    let auditor = fixture.root.path().join("auditor");
    fs::create_dir(&auditor).unwrap();
    let auditor_key = auditor.join("auditor.key");
    let (ok, key) = finish(spawn(
        &disclosure,
        &auditor,
        json!({"method":"keygen","key_file":auditor_key}),
    ));
    assert!(ok, "{key}");
    let grant = auditor.join("deal.grant");
    let (ok, exported) = finish(spawn(
        &disclosure,
        fixture.root.path(),
        json!({"method":"export","evidence_file":seller["evidence_file"],
            "issuer_key_file":fixture.root.path().join("seller/agreement.key"),
            "recipient_public_key":key["recipient_public_key"],"grant_file":grant,
            "expires_at":clock()+300}),
    ));
    assert!(ok, "{exported}");
    let (ok, verified) = finish(spawn(
        &disclosure,
        &auditor,
        json!({"method":"verify_payment","grant_file":grant,"key_file":auditor_key,
            "expected_issuer":seller_address,
            "deployment":{"rail":"x402_exact","namespace":"eip155:31337","rpc_url":proxy_url,
                "peer_rpc_url":rpc_url,"permit2_runtime_hash":format!("0x{}",hex::encode(permit2_hash)),
                "proxy_runtime_hash":format!("0x{}",hex::encode(proxy_hash)),
                "transaction_hash":transaction_hash}}),
    ));
    assert!(ok, "{verified}");
    assert_eq!(verified["agreement_verified"], true);
    assert_eq!(verified["payment_verified"], true);
    assert_eq!(verified["delivery_verified"], false);
    assert_eq!(verified["deal_commitment"], buyer["deal_commitment"]);
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 1);

    // Stage measurements: local monotonic clocks only; no block-timestamp subtraction and no
    // invented shielded comparison. x402 public-bound has no proving stage.
    let measurements = json!({
        "negotiation_ms": negotiation.as_millis(),
        "permit_preparation_and_failed_http_ms": permit_preparation.as_millis(),
        "submission_to_first_inclusion_ms": first_inclusion.as_millis(),
        "submission_to_finalized_verification_ms": finalized_verification.as_millis(),
        "delivery_ms": delivery.as_millis(),
        "proof_ms": null,
        "proof_note": "x402 exact public-bound has no proving stage",
    });
    println!("X402_MEASUREMENTS {measurements}");
}
