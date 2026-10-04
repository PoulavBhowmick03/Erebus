//! x402 `exact` driven end to end by two agents, each through its own Metropolis MCP server.
//!
//! Negotiation and the initial paid access request go through MCP, not the CLIs. The buyer's
//! server is the operator-selected x402 profile (negotiation and retrieval only). Faults: the
//! response to the first paid request is dropped after the seller acted on it, the seller's
//! service restarts, and the buyer's MCP processes restart. With
//! `EREBUS_TEST_INSTALLED_BIN_DIR` and `EREBUS_TEST_MCP_SERVER` set, every product process is
//! the installed one; the agent driver is the test's own client.

use super::*;
use axum::{body::Bytes, extract::State as RelayState, http::HeaderMap, response::IntoResponse};

/// Relays the seller's access endpoint and drops the first `drop` paid responses after the
/// service has processed them, so the buyer never sees the seller's answer.
#[derive(Clone)]
struct AccessRelay {
    upstream: String,
    client: reqwest::Client,
    drop: Arc<AtomicUsize>,
}

async fn relay_access(
    RelayState(relay): RelayState<AccessRelay>,
    headers: HeaderMap,
    body: Bytes,
) -> axum::response::Response {
    let mut request = relay
        .client
        .post(format!("{}/v1/access", relay.upstream))
        .body(body.to_vec());
    for name in ["content-type", "PAYMENT-SIGNATURE"] {
        if let Some(value) = headers.get(name) {
            request = request.header(name, value.as_bytes());
        }
    }
    // Claimed on arrival: the service may answer, or be restarted mid-request; either way this
    // response never reaches the buyer.
    let dropped = headers.contains_key("PAYMENT-SIGNATURE")
        && relay
            .drop
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok();
    let Ok(response) = request.send().await else {
        return axum::http::StatusCode::BAD_GATEWAY.into_response();
    };
    let status = response.status();
    let mut forwarded = HeaderMap::new();
    for name in ["content-type", "PAYMENT-REQUIRED", "PAYMENT-RESPONSE"] {
        if let Some(value) = response.headers().get(name) {
            forwarded.insert(name, value.clone());
        }
    }
    let body = response.bytes().await.unwrap_or_default();
    if dropped {
        return axum::http::StatusCode::BAD_GATEWAY.into_response();
    }
    (
        axum::http::StatusCode::from_u16(status.as_u16()).unwrap(),
        forwarded,
        body,
    )
        .into_response()
}

/// Durable buyer permits; the journal keeps a zero-byte `.lock` beside each record.
fn permit_records(cache: &Path) -> usize {
    fs::read_dir(cache.join("x402-authorizations"))
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|e| e == "json")
        })
        .count()
}

/// Runs the repository's agent driver against two MCP servers. Exit status and the final JSON.
fn run_agents(fixture: &Fixture, access: &Path, max_polls: u32) -> (bool, Value) {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let driver = repository.join("agents/src/erebus_agents/metropolis_loop.py");
    let mut command = match (
        std::env::var_os("EREBUS_TEST_MCP_SERVER"),
        std::env::var_os("EREBUS_TEST_MCP_PYTHON"),
    ) {
        // Installed: the driver (stdlib and `mcp` only) runs isolated on the installed Python,
        // and the servers are the installed entry point with only installed commands on PATH.
        (Some(server), Some(python)) => {
            let mut command = Command::new(python);
            command
                .arg("-I")
                .arg(&driver)
                .arg("--server-command")
                .arg(server);
            command
        }
        _ => {
            let mut command = Command::new("uv");
            command
                .args([
                    "run",
                    "--locked",
                    "python",
                    "-m",
                    "erebus_agents.metropolis_loop",
                ])
                .arg("--negotiation-cli")
                .arg(&fixture.binary)
                .current_dir(&repository)
                .env_remove("VIRTUAL_ENV");
            command
        }
    };
    command
        .args([
            "--profile",
            "x402-exact",
            "--operation",
            &hex::encode(OPERATION),
        ])
        .args(["--max-polls", &max_polls.to_string()])
        .arg("--buyer-negotiation")
        .arg(&fixture.buyer_config)
        .arg("--buyer-access")
        .arg(access)
        .arg("--seller-negotiation")
        .arg(&fixture.seller_config);
    let output = command.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let Some(last) = stdout.trim().lines().last() else {
        panic!(
            "agent driver produced no result (exit {:?}): {stderr}",
            output.status.code()
        );
    };
    let record: Value =
        serde_json::from_str(last).unwrap_or_else(|_| panic!("agents: {stdout}\n{stderr}"));
    (output.status.success(), record)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil, uv, and built EVM artifacts; MCP agents submit one x402 test payment"]
async fn two_mcp_agents_pay_over_x402_through_a_dropped_response_and_restarts() {
    let (client, rpc_url, _anvil, proxy, proxy_url, token, permit2_hash, proxy_hash) =
        deploy_x402_chain().await;
    let buyer_identity = AuthorizationIdentity::from_bytes(&[21; 32]).unwrap();
    let seller_identity = AuthorizationIdentity::from_bytes(&[22; 32]).unwrap();
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
    let root = fixture.root.path();
    // Agents start independently, so each side waits for its peer; the seller's negotiation
    // publishes its agreement straight to the access service's evidence directory.
    let publication = root.join("seller/access-evidence");
    fs::create_dir(&publication).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&publication, fs::Permissions::from_mode(0o700)).unwrap();
    }
    for (path, publish) in [
        (&fixture.buyer_config, false),
        (&fixture.seller_config, true),
    ] {
        let mut config: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        config["timeout_seconds"] = json!(120);
        if publish {
            config["access_evidence_root"] = json!(publication);
        }
        fs::write(path, config.to_string()).unwrap();
    }

    // Buyer onboarding before any deal: a token balance and one Permit2 approval.
    for (method, params) in [
        (
            "anvil_setBalance",
            json!([gas_address, "0xde0b6b3a7640000"]),
        ),
        ("anvil_impersonateAccount", json!([buyer_address])),
        (
            "anvil_setBalance",
            json!([buyer_address, "0xde0b6b3a7640000"]),
        ),
    ] {
        rpc(&client, &rpc_url, method, params).await;
    }
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

    let service_id = [42; 32];
    let service_port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let service_url = format!("http://127.0.0.1:{service_port}");
    private(&root.join("seller/payload"), native_product::PAYLOAD);
    private(&root.join("seller/x402-gas.key"), &gas_seed);
    let service_config = root.join("seller/x402-access.json");
    private(&service_config, json!({"service_id":service_id,"seller_key":seller_identity.address(),"suite_id":1,
        "resource":"dataset.snapshot.v1","payload_file":root.join("seller/payload"),"evidence_root":publication,
        "state_root":root.join("seller/x402-state"),"port":service_port,
        "backend":{"mode":"x402_exact","namespace":"eip155:31337","rpc_url":proxy_url,"peer_rpc_url":rpc_url,
            "permit2_runtime_hash":permit2_hash,"proxy_runtime_hash":proxy_hash,
            "transaction_key_file":root.join("seller/x402-gas.key"),"signer_journal_root":root.join("operator-signer"),
            "gas_limit":500000,"max_fee_per_gas":"3000000000","max_priority_fee_per_gas":"1000000000"}})
        .to_string().as_bytes());
    let service = Arc::new(std::sync::Mutex::new(Some(
        native_product::service(&service_config, &service_url, &client).await,
    )));

    let relay = AccessRelay {
        upstream: service_url.clone(),
        client: client.clone(),
        drop: Arc::new(AtomicUsize::new(1)),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_url = format!("http://{}", listener.local_addr().unwrap());
    let relay_state = relay.clone();
    tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/v1/access", post(relay_access))
                .with_state(relay_state),
        )
        .await
        .unwrap()
    });
    let cache = root.join("buyer/x402-cache");
    let access = root.join("buyer/access.json");
    let mut variables = json!({"EREBUS_ACCESS_PAYMENT_RAIL":"x402-exact",
        "EREBUS_ACCESS_SERVICE_URL":format!("{relay_url}/v1/access"),"EREBUS_ACCESS_SERVICE_ID":hex::encode(service_id),
        "EREBUS_ACCESS_BUYER_KEY_FILE":root.join("buyer/agreement.key"),"EREBUS_ACCESS_CACHE":cache,
        "EREBUS_ALLOW_LOOPBACK_ACCESS_HTTP":"1"});
    if std::env::var_os("EREBUS_TEST_MCP_SERVER").is_none() {
        variables["EREBUS_ACCESS_CLI"] = json!(env!("CARGO_BIN_EXE_erebus-access"));
    }
    private(&access, variables.to_string().as_bytes());

    // The seller restarts as soon as its one broadcast leaves; the buyer sees only failures.
    let watched = proxy.broadcast_hashes.clone();
    let killed = service.clone();
    let watcher = tokio::spawn(async move {
        loop {
            if !watched.lock().unwrap().is_empty() {
                killed.lock().unwrap().take();
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    let first_fixture = &fixture;
    let (ok, interrupted) = tokio::task::block_in_place(|| run_agents(first_fixture, &access, 4));
    assert!(!ok, "{interrupted}");
    assert!(
        interrupted["error"]
            .as_str()
            .unwrap()
            .contains("do not pay again"),
        "{interrupted}"
    );
    watcher.await.unwrap();
    assert_eq!(
        relay.drop.load(Ordering::SeqCst),
        0,
        "the first paid response was dropped"
    );
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 1);
    assert_eq!(
        permit_records(&cache),
        1,
        "one retained buyer authorization"
    );

    // Seller restart, then fresh buyer and seller MCP processes resume by retrieval alone.
    *service.lock().unwrap() =
        Some(native_product::service(&service_config, &service_url, &client).await);
    let miner_client = client.clone();
    let miner_url = rpc_url.clone();
    let miner = tokio::spawn(async move {
        loop {
            rpc(&miner_client, &miner_url, "anvil_mine", json!(["0x1"])).await;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });
    let (ok, record) = tokio::task::block_in_place(|| run_agents(&fixture, &access, 60));
    miner.abort();
    assert!(ok, "{record}");
    eprintln!("x402 MCP agent record (Anvil): {record}");
    assert_eq!(record["profile"], "x402-exact");
    assert_eq!(record["payment_verified"], false);
    assert_eq!(
        record["resource_sha256"],
        hex::encode(Sha256::digest(native_product::PAYLOAD))
    );
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 1);
    assert_eq!(permit_records(&cache), 1);
    let balance = rpc(
        &client,
        &rpc_url,
        "eth_call",
        json!([{"to":token,"data":format!("0x{}",
        hex::encode(abi::encode_balance_of_call(&seller_identity.address())))},"latest"]),
    )
    .await;
    assert_eq!(
        u128::from_str_radix(balance.as_str().unwrap().trim_start_matches("0x"), 16).unwrap(),
        70
    );

    // Payment is verified only by an independent auditor from the encrypted grant.
    let transaction_hash = format!(
        "0x{}",
        hex::encode(proxy.broadcast_hashes.lock().unwrap()[0])
    );
    let disclosure = root.join("erebus-disclosure");
    fs::copy(
        native_binary(
            "erebus-shielded-disclosure",
            env!("CARGO_BIN_EXE_erebus-shielded-disclosure"),
        ),
        &disclosure,
    )
    .unwrap();
    let auditor = root.join("auditor");
    fs::create_dir(&auditor).unwrap();
    let auditor_key = auditor.join("auditor.key");
    let (ok, key) = finish(spawn(
        &disclosure,
        &auditor,
        json!({"method":"keygen","key_file":auditor_key}),
    ));
    assert!(ok, "{key}");
    let grant = auditor.join("deal.grant");
    let seller_evidence = root.join(format!(
        "seller/state/agent/{}.0.tx",
        hex::encode(OPERATION)
    ));
    let (ok, exported) = finish(spawn(
        &disclosure,
        root,
        json!({"method":"export","evidence_file":seller_evidence,
        "issuer_key_file":root.join("seller/agreement.key"),"recipient_public_key":key["recipient_public_key"],
        "grant_file":grant,"expires_at":clock()+300}),
    ));
    assert!(ok, "{exported}");
    let (ok, verified) = finish(spawn(
        &disclosure,
        &auditor,
        json!({"method":"verify_payment","grant_file":grant,
        "key_file":auditor_key,"expected_issuer":format!("0x{}", hex::encode(seller_identity.address())),
        "deployment":{"rail":"x402_exact","namespace":"eip155:31337","rpc_url":proxy_url,"peer_rpc_url":rpc_url,
            "permit2_runtime_hash":format!("0x{}",hex::encode(permit2_hash)),
            "proxy_runtime_hash":format!("0x{}",hex::encode(proxy_hash)),"transaction_hash":transaction_hash}}),
    ));
    assert!(ok, "{verified}");
    assert_eq!(verified["payment_verified"], true);
    assert_eq!(verified["delivery_verified"], false);
    assert_eq!(verified["deal_commitment"], record["deal_commitment"]);
}
