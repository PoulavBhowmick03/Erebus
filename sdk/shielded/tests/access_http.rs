//! Funded public-bound access through a separate, restartable HTTP service process.

use erebus_core::{
    auth::{authorization_digest, Authorization, Role},
    commitment::{commit_agreement, CommitmentBlinding},
    domain::DeploymentDomain,
    ids::{AddressBytes, AssetId, BaseUnits, ChainNamespace, KeyBytes, SignatureBytes},
    terms::{AgreementTerms, FeePolicy},
};
use erebus_evm::abi;
use erebus_shielded_prover::access::{request_digest, AccessPolicy, AccessRequest};
use erebus_transport::{disclosure::SelectedAgreement, identity::AuthorizationIdentity};
use serde_json::{json, Value};
use std::{
    io::Write,
    net::TcpListener,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub(crate) struct Process(pub(crate) Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub(crate) fn port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

pub(crate) async fn rpc(client: &reqwest::Client, url: &str, method: &str, params: Value) -> Value {
    let body: Value = client
        .post(url)
        .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(body.get("error").is_none(), "{method}: {body}");
    body["result"].clone()
}

pub(crate) async fn send(
    client: &reqwest::Client,
    url: &str,
    from: &str,
    to: Option<&str>,
    data: Vec<u8>,
) -> Value {
    let mut transaction =
        json!({"from":from,"data":format!("0x{}",hex::encode(data)),"gas":"0x7a1200"});
    if let Some(to) = to {
        transaction["to"] = json!(to);
    }
    let hash = rpc(client, url, "eth_sendTransaction", json!([transaction])).await;
    for _ in 0..100 {
        let receipt = rpc(client, url, "eth_getTransactionReceipt", json!([hash])).await;
        if !receipt.is_null() {
            assert_eq!(receipt["status"], "0x1");
            return receipt;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("missing receipt");
}

pub(crate) async fn deploy(
    client: &reqwest::Client,
    url: &str,
    from: &str,
    name: &str,
    args: Vec<u8>,
) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../contracts/evm/out/{name}.sol/{name}.json"));
    let artifact: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
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

pub(crate) fn private_file(path: &Path, bytes: &[u8]) {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

fn access_output(request: &Value) -> std::process::Output {
    let mut child = Command::new(access_binary())
        .env_clear()
        .current_dir(
            Path::new(request["cache_root"].as_str().unwrap())
                .parent()
                .unwrap(),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(request).unwrap())
        .unwrap();
    child.wait_with_output().unwrap()
}

pub(crate) fn retrieve_command(request: &Value) -> Value {
    let output = access_output(request);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).unwrap()
}

fn access_binary() -> std::ffi::OsString {
    std::env::var_os("EREBUS_TEST_ACCESS_BIN")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_erebus-access").into())
}

pub(crate) async fn service(config: &Path, url: &str, client: &reqwest::Client) -> Process {
    let mut process = Process(
        Command::new(env!("CARGO_BIN_EXE_erebus-access-service"))
            .env_clear()
            .env("EREBUS_ACCESS_CONFIG", config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    for _ in 0..100 {
        assert!(
            process.0.try_wait().unwrap().is_none(),
            "access service stopped"
        );
        if client.get(format!("{url}/healthz")).send().await.is_ok() {
            return process;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("access service did not become healthy");
}

#[tokio::test]
#[ignore = "requires Anvil and built EVM contract artifacts"]
async fn funded_http_access_recovers_a_lost_response_after_service_restart_without_a_second_payment(
) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let rpc_port = port();
    let _anvil = Process(
        Command::new("anvil")
            .args([
                "--port",
                &rpc_port.to_string(),
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
    let rpc_url = format!("http://127.0.0.1:{rpc_port}");
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
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let accounts = rpc(&client, &rpc_url, "eth_accounts", json!([])).await;
    let account = accounts[0].as_str().unwrap();
    let settlement = deploy(
        &client,
        &rpc_url,
        account,
        "ErebusSettlement",
        abi::encode_settlement_constructor(31337, 1),
    )
    .await;
    let contract = settlement["contractAddress"].as_str().unwrap();
    let from_block = u64::from_str_radix(
        settlement["blockNumber"]
            .as_str()
            .unwrap()
            .trim_start_matches("0x"),
        16,
    )
    .unwrap();
    let token = deploy(
        &client,
        &rpc_url,
        account,
        "MockERC20",
        abi::encode_token_constructor("Test", "TEST"),
    )
    .await;
    let token = token["contractAddress"].as_str().unwrap();
    let buyer = AuthorizationIdentity::from_bytes(
        &hex::decode("ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80").unwrap(),
    )
    .unwrap();
    let seller = AuthorizationIdentity::from_bytes(&[2; 32]).unwrap();
    assert_eq!(format!("0x{}", hex::encode(buyer.address())), account);
    let address = |text: &str| {
        hex::decode(text.trim_start_matches("0x"))
            .unwrap()
            .try_into()
            .unwrap()
    };
    send(
        &client,
        &rpc_url,
        account,
        Some(token),
        abi::encode_mint_call(&buyer.address(), 1_000_000),
    )
    .await;
    send(
        &client,
        &rpc_url,
        account,
        Some(token),
        abi::encode_approve_call(&address(contract), 1_000_000),
    )
    .await;

    let value: Value = serde_json::from_str(include_str!(
        "../../core/tests/fixtures/agreement-v1-vectors.json"
    ))
    .unwrap();
    let mut terms = AgreementTerms::decode(
        &hex::decode(
            value["vectors"][0]["expected"]["canonicalHex"]
                .as_str()
                .unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    terms.domain = DeploymentDomain {
        namespace: ChainNamespace::parse("eip155:31337").unwrap(),
        settlement_contract: Some(AddressBytes::new(address(contract).to_vec()).unwrap()),
        pool: None,
        verifier_version: 1,
    };
    terms.asset = AssetId::new(terms.domain.namespace.clone(), "erc20", token).unwrap();
    terms.buyer_authorization_key = KeyBytes::new(buyer.address().to_vec()).unwrap();
    terms.seller_authorization_key = KeyBytes::new(seller.address().to_vec()).unwrap();
    terms.payment_recipient = terms.seller_authorization_key.clone();
    terms.transcript_root = [0; 32];
    terms.fee_policy = FeePolicy::none();
    let policy = AccessPolicy {
        service_id: [42; 32],
        seller: terms.seller_authorization_key.clone(),
        suite_id: 1,
        resource: "dataset.snapshot.v1".into(),
        payload: b"immutable data feed snapshot".to_vec(),
    };
    terms.service.resource = policy.resource.clone();
    terms.service.unit = "snapshot".into();
    terms.service.quantity = BaseUnits::new(1);
    terms.service.access_recipient = terms.buyer_authorization_key.clone();
    terms.service.fulfillment_method = "http-access-v1".into();
    terms.service.fulfillment_digest = policy.resource_hash();
    let blinding = CommitmentBlinding::from_bytes([10; 32]);
    let commitment = commit_agreement(&terms, &blinding).unwrap();
    let sign = |role, key: &AuthorizationIdentity| Authorization {
        role,
        suite_id: 1,
        commitment,
        signature: SignatureBytes::new(
            key.sign_digest(&authorization_digest(&terms.domain, role, &commitment, 1).unwrap())
                .to_vec(),
        )
        .unwrap(),
    };
    let evidence = SelectedAgreement {
        buyer: sign(Role::Buyer, &buyer),
        seller: sign(Role::Seller, &seller),
        terms,
        blinding,
        transcript_hash_version: 1,
        messages: Vec::new(),
    };
    let root = tempfile::tempdir().unwrap();
    let evidence_root = root.path().join("agreements");
    std::fs::create_dir(&evidence_root).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&evidence_root, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    private_file(
        &evidence_root.join(format!("{}.evidence", commitment.to_hex())),
        &evidence.encode().unwrap(),
    );
    let payload_file = root.path().join("payload");
    private_file(&payload_file, &policy.payload);
    let http_port = port();
    let http_url = format!("http://127.0.0.1:{http_port}");
    let config_file = root.path().join("service.json");
    private_file(&config_file,&serde_json::to_vec(&json!({"service_id":policy.service_id,"seller_key":seller.address().to_vec(),"suite_id":1,"resource":policy.resource,"payload_file":payload_file,"evidence_root":evidence_root,"state_root":root.path().join("service-state"),"port":http_port,"backend":{"mode":"public_bound","namespace":"eip155:31337","settlement_contract":contract,"verifier_version":1,"rpc_url":rpc_url,"from_block":from_block,"log_block_range":2,"max_log_queries":1,"max_ancestry":2}})).unwrap());
    let process = service(&config_file, &http_url, &client).await;
    let expires_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 120;
    let digest = request_digest(&evidence, policy.service_id, [4; 32], expires_at, None).unwrap();
    let request = AccessRequest {
        deal_commitment: commitment.to_hex(),
        nonce: [4; 32],
        expires_at,
        signature: buyer.sign_digest(&digest).to_vec(),
        payment: None,
    };
    let mut wrong = request.clone();
    wrong.signature = seller.sign_digest(&digest).to_vec();
    let rejected = client
        .post(format!("{http_url}/v1/access"))
        .json(&wrong)
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), 401);
    let pending = client
        .post(format!("{http_url}/v1/access"))
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(pending.status(), 202);
    let pending: Value = pending.json().await.unwrap();
    assert_eq!(pending["payment_verified"], false);
    assert!(pending.get("payload_hex").is_none());
    send(
        &client,
        &rpc_url,
        account,
        Some(contract),
        abi::encode_settle_call(
            &evidence.terms.encode().unwrap(),
            evidence.blinding.as_bytes(),
            evidence.buyer.signature.as_bytes(),
            evidence.seller.signature.as_bytes(),
            &address(token),
        ),
    )
    .await;
    rpc(&client, &rpc_url, "anvil_mine", json!(["0x4"])).await;
    let nonce = rpc(
        &client,
        &rpc_url,
        "eth_getTransactionCount",
        json!([account, "latest"]),
    )
    .await;
    let first = loop {
        let response = client
            .post(format!("{http_url}/v1/access"))
            .json(&request)
            .send()
            .await
            .unwrap();
        if response.status() == 200 {
            break response;
        }
        assert_eq!(response.status(), 202);
        assert!(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
                < expires_at
        );
    };
    // The server persisted issuance; simulate a caller that lost the response body.
    drop(first);
    drop(process);
    let _restarted = service(&config_file, &http_url, &client).await;
    let received = client
        .post(format!("{http_url}/v1/access"))
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(received.status(), 200);
    let received: Value = received.json().await.unwrap();
    assert_eq!(received["issuance"]["repeated"], true);
    assert_eq!(received["payment_verified"], true);
    assert_eq!(received["delivery_verified"], false);
    assert_eq!(
        hex::decode(received["payload_hex"].as_str().unwrap()).unwrap(),
        policy.payload
    );
    let key_file = root.path().join("buyer.key");
    private_file(
        &key_file,
        &hex::decode("ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80").unwrap(),
    );
    let evidence_file = evidence_root.join(format!("{}.evidence", commitment.to_hex()));
    let retrieval = json!({"method":"retrieve","evidence_file":evidence_file,"buyer_key_file":key_file,
        "service_url":format!("{http_url}/v1/access"),"service_id":hex::encode(policy.service_id),
        "cache_root":root.path().join("buyer-cache"),"allow_loopback_http":true});
    let resource = retrieve_command(&retrieval);
    assert_eq!(resource["result"]["resource_verified"], true);
    assert_eq!(resource["result"]["payment_verified"], false);
    assert_eq!(
        resource["result"]["seller_reported_payment_finalized"],
        true
    );
    assert_eq!(resource["result"]["cached"], false);
    assert!(resource["result"].get("payload_hex").is_none());
    assert_eq!(
        std::fs::read(resource["result"]["resource_file"].as_str().unwrap()).unwrap(),
        policy.payload
    );
    if let Ok(python) = std::env::var("EREBUS_TEST_MCP_PYTHON") {
        let script =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../mcp-server/tests/access_probe.py");
        let output = Command::new(python)
            .arg(script)
            .arg(&evidence_file)
            .arg(&key_file)
            .arg(format!("{http_url}/v1/access"))
            .arg(hex::encode(policy.service_id))
            .arg(root.path().join("mcp-cache"))
            .arg(access_binary())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let verified: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(verified["mcp_access_verified"], true);
    }
    let issuance_file = root.path().join("service-state/issuance").join(format!(
        "{}.json",
        received["issuance"]["issuance_id"].as_str().unwrap()
    ));
    let original = std::fs::read(&issuance_file).unwrap();
    std::fs::write(&issuance_file, b"{}").unwrap();
    let unavailable = client
        .post(format!("{http_url}/v1/access"))
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(unavailable.status(), 503);
    let unavailable: Value = unavailable.json().await.unwrap();
    assert_eq!(unavailable["status"], "paid_but_undelivered");
    assert_eq!(unavailable["payment_verified"], true);
    assert_eq!(unavailable["retry_without_payment"], true);
    assert!(unavailable.get("payload_hex").is_none());
    let mut failed_retrieval = retrieval.clone();
    failed_retrieval["cache_root"] = json!(root.path().join("failure-cache"));
    let output = access_output(&failed_retrieval);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "paid_but_undelivered");
    assert_eq!(report["seller_reported_payment_finalized"], true);
    assert_eq!(report["payment_verified"], false);
    assert_eq!(report["resource_verified"], false);
    if let Ok(python) = std::env::var("EREBUS_TEST_MCP_PYTHON") {
        let script =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../mcp-server/tests/access_probe.py");
        let output = Command::new(python)
            .arg(script)
            .arg(&evidence_file)
            .arg(&key_file)
            .arg(format!("{http_url}/v1/access"))
            .arg(hex::encode(policy.service_id))
            .arg(root.path().join("mcp-failure-cache"))
            .arg(access_binary())
            .arg("paid_but_undelivered")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["seller_reported_undelivered"], true);
    }
    std::fs::write(&issuance_file, original).unwrap();
    drop(_restarted);
    std::fs::remove_file(key_file).unwrap();
    let cached = retrieve_command(&retrieval);
    assert_eq!(cached["result"]["cached"], true);
    assert_eq!(
        cached["result"]["issuance_id"],
        resource["result"]["issuance_id"]
    );
    assert_eq!(
        rpc(
            &client,
            &rpc_url,
            "eth_getTransactionCount",
            json!([account, "latest"])
        )
        .await,
        nonce
    );
    let balance=rpc(&client,&rpc_url,"eth_call",json!([{"to":token,"data":format!("0x{}",hex::encode(abi::encode_balance_of_call(&seller.address())))},"latest"])).await;
    assert_eq!(
        u128::from_str_radix(balance.as_str().unwrap().trim_start_matches("0x"), 16).unwrap(),
        1_000_000
    );
}
