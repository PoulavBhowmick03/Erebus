//! Public-bound negotiation, settlement, and delivery driven by two MCP agents.

use super::*;
use erebus_core::commitment::commit_agreement;

const SERVICE_ID: [u8; 32] = [42; 32];

/// Plays the seller's operator. Once negotiation leaves the seller's agreement on disk, it
/// publishes that agreement to the access service under its commitment and starts the service.
/// Nothing in the product performs this handoff; an operator must.
async fn seller_operator(
    root: PathBuf,
    config: Value,
    client: reqwest::Client,
    port: u16,
) -> ServiceProcess {
    let seller_root = root.join("seller");
    let negotiated = seller_root.join(format!("state/agent/{}.0.tx", hex::encode(OPERATION)));
    let deadline = Instant::now() + Duration::from_secs(900);
    while !negotiated.exists() {
        assert!(
            Instant::now() < deadline,
            "seller never reached an agreement"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let selected = SelectedAgreement::decode(&fs::read(&negotiated).unwrap()).unwrap();
    verify_selected_agreement(&selected).unwrap();
    let commitment = commit_agreement(&selected.terms, &selected.blinding).unwrap();
    let agreements = seller_root.join("access-evidence");
    fs::create_dir(&agreements).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&agreements, fs::Permissions::from_mode(0o700)).unwrap();
    }
    private(
        &agreements.join(format!("{}.evidence", commitment.to_hex())),
        &selected.encode().unwrap(),
    );
    let payload = seller_root.join("payload");
    private(&payload, native_product::PAYLOAD);
    let service_config = seller_root.join("access.json");
    private(&service_config, json!({"service_id":SERVICE_ID,"seller_key":selected.terms.seller_authorization_key.as_bytes(),
        "suite_id":selected.terms.suite_id,"resource":selected.terms.service.resource,"payload_file":payload,
        "evidence_root":agreements,"state_root":seller_root.join("issuance"),"port":port,
        "backend":{"mode":"public_bound","namespace":config["namespace"],"settlement_contract":config["settlement_contract"],
            "verifier_version":1,"rpc_url":config["peer_rpc_url"],"from_block":config["first_block"],
            "log_block_range":100,"max_log_queries":8,"max_ancestry":64}}).to_string().as_bytes());
    native_product::service(
        &service_config,
        &format!("http://127.0.0.1:{port}"),
        &client,
    )
    .await
}

/// Funds the deal, starts the seller's operator, and writes the buyer's access variables.
/// Returns the access variable file and the operator task holding the service.
fn prepare(deployed: &PublicDeployed) -> (PathBuf, tokio::task::JoinHandle<ServiceProcess>) {
    let root = deployed.fixture.root.path().to_path_buf();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let operator = tokio::spawn(seller_operator(
        root.clone(),
        deployed.config.clone(),
        deployed.client.clone(),
        port,
    ));
    let access = root.join("erebus-access");
    fs::copy(env!("CARGO_BIN_EXE_erebus-access"), &access).unwrap();
    let variables = root.join("buyer/access.json");
    private(&variables, json!({"EREBUS_ACCESS_BUYER_KEY_FILE":root.join("buyer/agreement.key"),
        "EREBUS_ACCESS_SERVICE_URL":format!("http://127.0.0.1:{port}/v1/access"),
        "EREBUS_ACCESS_SERVICE_ID":hex::encode(SERVICE_ID),"EREBUS_ACCESS_CACHE":root.join("buyer/content-cache"),
        "EREBUS_ACCESS_CLI":access,"EREBUS_ALLOW_LOOPBACK_ACCESS_HTTP":"1"}).to_string().as_bytes());
    (variables, operator)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil, uv, and built EVM artifacts; two MCP agents settle a public-bound test payment"]
async fn two_mcp_agents_negotiate_pay_and_retrieve_public_bound_with_one_send() {
    // Real block production, so the agents reach finality by observation alone.
    let deployed = deploy_public(Some(1)).await;
    fund_public(&deployed).await;
    deployed.proxy.lose_broadcast.store(true, Ordering::SeqCst);
    let (access, operator) = prepare(&deployed);
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut command = Command::new("uv");
    command
        .args([
            "run",
            "--locked",
            "python",
            "-m",
            "erebus_agents.metropolis_loop",
        ])
        .args(["--operation", &hex::encode(OPERATION)])
        .arg("--buyer-negotiation")
        .arg(&deployed.fixture.buyer_config)
        .arg("--buyer-payment")
        .arg(&deployed.config_path)
        .arg("--buyer-access")
        .arg(&access)
        .arg("--seller-negotiation")
        .arg(&deployed.fixture.seller_config)
        .arg("--negotiation-cli")
        .arg(&deployed.fixture.binary)
        .arg("--payment-cli")
        .arg(&deployed.binary)
        .current_dir(&repository)
        .env_remove("VIRTUAL_ENV");
    let output = tokio::task::spawn_blocking(move || command.output().unwrap())
        .await
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "agents: {stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _service = operator.await.unwrap();
    let record: Value = serde_json::from_str(stdout.trim().lines().last().unwrap()).unwrap();
    eprintln!("public-bound two-agent MCP record (Anvil): {record}");
    assert_eq!(record["payment_verified"], true);
    assert_eq!(record["stage"], "Finalized");
    assert_eq!(record["settle_calls"], 1);
    assert_eq!(record["winning_commitment"], record["deal_commitment"]);
    assert_eq!(record["settlement"]["submitted_this_call"], true);
    assert_eq!(record["settlement"]["payment_verified"], false);
    assert!(record["recover_calls"].as_u64().unwrap() >= 1);
    assert_eq!(
        record["delivery"]["resource_sha256"],
        hex::encode(Sha256::digest(native_product::PAYLOAD))
    );
    for stage in [
        "inclusion",
        "finality",
        "payment_verified_after",
        "delivery",
    ] {
        assert!(record["latency_s"][stage].is_number(), "{stage}: {record}");
    }
    assert_eq!(deployed.proxy.sends.load(Ordering::SeqCst), 1);
    let balance = rpc(&deployed.client,&deployed.rpc_url,"eth_call",json!([{"to":deployed.token,"data":format!("0x{}",hex::encode(abi::encode_balance_of_call(&AuthorizationIdentity::from_bytes(&[22;32]).unwrap().address())))},"latest"])).await;
    assert_eq!(
        u128::from_str_radix(balance.as_str().unwrap().trim_start_matches("0x"), 16).unwrap(),
        70
    );
}

/// Keeps a funded public-bound environment and the seller's operator alive for external agents.
/// Run with `EREBUS_AGENT_ENV_DIR=<new dir>`; create `<dir>/stop` to tear down.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "manual: holds Anvil open for external MCP agents"]
async fn hold_public_agent_environment() {
    let Some(directory) = std::env::var_os("EREBUS_AGENT_ENV_DIR").map(PathBuf::from) else {
        return;
    };
    let deployed = deploy_public(Some(1)).await;
    fund_public(&deployed).await;
    deployed.proxy.lose_broadcast.store(true, Ordering::SeqCst);
    // Agents start independently, so give each side time to find its peer.
    for path in [
        &deployed.fixture.buyer_config,
        &deployed.fixture.seller_config,
    ] {
        let mut config: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        config["timeout_seconds"] = json!(300);
        fs::write(path, config.to_string()).unwrap();
    }
    let (access, operator) = prepare(&deployed);
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("environment.json"),
        json!({"mode":"public-bound","operation_ref":hex::encode(OPERATION),"buyer_negotiation":deployed.fixture.buyer_config,
            "buyer_payment":deployed.config_path,"buyer_access":access,"seller_negotiation":deployed.fixture.seller_config,
            "negotiation_cli":deployed.fixture.binary,"payment_cli":deployed.binary})
        .to_string(),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(1800);
    while !directory.join("stop").exists() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let balance = rpc(&deployed.client,&deployed.rpc_url,"eth_call",json!([{"to":deployed.token,"data":format!("0x{}",hex::encode(abi::encode_balance_of_call(&AuthorizationIdentity::from_bytes(&[22;32]).unwrap().address())))},"latest"])).await;
    let paid =
        u128::from_str_radix(balance.as_str().unwrap().trim_start_matches("0x"), 16).unwrap();
    fs::write(
        directory.join("result.json"),
        json!({"raw_transaction_sends":deployed.proxy.sends.load(Ordering::SeqCst),"seller_token_balance":paid.to_string()})
            .to_string(),
    )
    .unwrap();
    operator.abort();
}
