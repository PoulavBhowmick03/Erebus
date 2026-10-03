//! Real chain evidence for copied negotiation and payment commands. Public-bound only.

use super::*;
use axum::{extract::State, routing::post, Json, Router};
use erebus_core::{
    domain::DeploymentDomain,
    ids::{AddressBytes, AssetId, ChainNamespace},
    suite::keccak256,
};
use erebus_evm::{
    abi,
    chain::{EvmChain, HistoricalObservation, ObservationJournal, ObservationLimits},
    deployment::EvmDeployment,
};
use sha2::{Digest, Sha256};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

struct ServiceProcess(Child);
#[path = "native_product.rs"]
mod native_product;
#[path = "shielded_driver.rs"]
mod shielded_driver;
impl Drop for ServiceProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[derive(Clone)]
struct Proxy {
    upstream: String,
    client: reqwest::Client,
    lose_broadcast: Arc<AtomicBool>,
    sends: Arc<AtomicUsize>,
    minimum_log_block: Arc<std::sync::Mutex<Option<u64>>>,
    corrupt_code: Arc<AtomicBool>,
    corrupt_consumed: Arc<AtomicBool>,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil and built EVM artifacts; copied commands submit public-bound test payments"]
async fn negotiated_payment_recovers_a_lost_broadcast_without_keys_or_a_second_send() {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let _anvil = ServiceProcess(
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
    let (buyer, seller) = fixture.pair(false);
    assert_eq!(buyer["deal_commitment"], seller["deal_commitment"]);
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
    rpc(&client, &rpc_url, "anvil_mine", json!(["0x3"])).await;
    let (_, unfunded) = run_payment(&binary, fixture.root.path(), &config_path, "funding").await;
    assert_eq!(unfunded["status"], "funding_required", "{unfunded}");
    assert_eq!(unfunded["funding"]["allowance_shortfall"], "70");
    assert_eq!(unfunded["funding"]["balance_shortfall"], "70");
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 0);
    rpc(
        &client,
        &rpc_url,
        "anvil_setBalance",
        json!([gas_address, "0xde0b6b3a7640000"]),
    )
    .await;
    send(
        &client,
        &rpc_url,
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
    rpc(&client, &rpc_url, "anvil_mine", json!(["0x3"])).await;
    let (ok, ready) = run_payment(&binary, fixture.root.path(), &config_path, "funding").await;
    assert!(ok, "{ready}");
    assert_eq!(ready["status"], "ready");
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 0);
    let held = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture.root.path().join("buyer/state/.payment-driver.lock"))
        .unwrap();
    fs2::FileExt::try_lock_exclusive(&held).unwrap();
    let (ok, busy) = run_payment(&binary, fixture.root.path(), &config_path, "settle").await;
    assert!(!ok);
    assert_eq!(busy["error"], "another payment command is active");
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 0);
    drop(held);
    // Invalid runtime data fails before signing/broadcast and cannot release policy capacity.
    proxy.corrupt_code.store(true, Ordering::SeqCst);
    let (ok, corrupted) = run_payment(&binary, fixture.root.path(), &config_path, "settle").await;
    assert!(!ok);
    assert_eq!(corrupted["status"], "error");
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 0);
    proxy.corrupt_code.store(false, Ordering::SeqCst);
    // A shifted start at a later canonical block must also be rejected.
    let later = rpc(
        &client,
        &rpc_url,
        "eth_getBlockByNumber",
        json!([format!("0x{:x}", first_block + 1), false]),
    )
    .await;
    let mut invalid = config.clone();
    invalid["first_block"] = json!(first_block + 1);
    invalid["first_hash"] = later["hash"].clone();
    let invalid_path = fixture.root.path().join("buyer/late-deployment.json");
    private(&invalid_path, invalid.to_string().as_bytes());
    let (ok, late) = run_payment(&binary, fixture.root.path(), &invalid_path, "settle").await;
    assert!(!ok);
    assert_eq!(late["status"], "error");
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 0);
    proxy.lose_broadcast.store(true, Ordering::SeqCst);
    let (_, submitted) = run_payment(&binary, fixture.root.path(), &config_path, "settle").await;
    assert_eq!(submitted["submitted_this_call"], true, "{submitted}");
    assert_eq!(submitted["payment_verified"], false);
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 1);
    // Further settle calls are observation only, even while finality is outstanding.
    let (_, pending) = run_payment(&binary, fixture.root.path(), &config_path, "settle").await;
    assert_eq!(pending["submitted_this_call"], false);
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 1);
    fs::remove_file(&key_path).unwrap();
    fs::rename(
        fixture.root.path().join("buyer/state/transcripts"),
        fixture.root.path().join("buyer/retained-transcripts"),
    )
    .unwrap();
    rpc(&client, &rpc_url, "anvil_mine", json!(["0x3"])).await;
    proxy.corrupt_consumed.store(true, Ordering::SeqCst);
    let (ok, disagreement) =
        run_payment(&binary, fixture.root.path(), &config_path, "observe").await;
    assert!(!ok, "{disagreement}");
    assert_eq!(disagreement["payment_verified"], false);
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 1);
    proxy.corrupt_consumed.store(false, Ordering::SeqCst);
    let (ok, recovered) = run_payment(&binary, fixture.root.path(), &config_path, "observe").await;
    assert!(ok, "{recovered}");
    assert_eq!(recovered["payment_verified"], true);
    assert_eq!(recovered["delivery_verified"], false);
    assert_eq!(recovered["submitted_this_call"], false);
    assert_eq!(recovered["nonce_cleanup_pending"], false);
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 1);
    assert_eq!(recovered["deal_commitment"], buyer["deal_commitment"]);
    assert_eq!(*proxy.minimum_log_block.lock().unwrap(), Some(first_block));
    let balance = rpc(&client,&rpc_url,"eth_call",json!([{"to":token,"data":format!("0x{}",hex::encode(abi::encode_balance_of_call(&AuthorizationIdentity::from_bytes(&[22;32]).unwrap().address())))},"latest"])).await;
    assert_eq!(
        u128::from_str_radix(balance.as_str().unwrap().trim_start_matches("0x"), 16).unwrap(),
        70
    );
    // Paired scans yield and resume from the same authenticated bound under a tiny budget.
    rpc(&client, &rpc_url, "anvil_mine", json!(["0x3"])).await;
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
    native_product::finish(&fixture, &buyer, &seller, &config, &client).await;
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 1);
    task.abort();
    let _ = task.await;
}
