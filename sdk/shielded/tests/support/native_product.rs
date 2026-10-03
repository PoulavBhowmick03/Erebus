//! Delivery recovery and independent auditing of the actually negotiated test payment.

use super::*;
use erebus_shielded_prover::access::{request_digest, AccessRequest};

pub(super) const PAYLOAD: &[u8] = b"private negotiated data-feed snapshot";

pub(super) async fn service(config: &Path, url: &str, client: &reqwest::Client) -> ServiceProcess {
    let mut process = ServiceProcess(
        Command::new(env!("CARGO_BIN_EXE_erebus-access-service"))
            .env_clear()
            .env("EREBUS_ACCESS_CONFIG", config)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
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
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("access service did not become healthy");
}

pub(super) async fn finish(
    fixture: &Fixture,
    buyer: &Value,
    seller: &Value,
    config: &Value,
    client: &reqwest::Client,
) {
    let selected =
        SelectedAgreement::decode(&fs::read(seller["evidence_file"].as_str().unwrap()).unwrap())
            .unwrap();
    verify_selected_agreement(&selected).unwrap();
    let seller_root = fixture.root.path().join("seller");
    let agreements = seller_root.join("access-evidence");
    fs::create_dir(&agreements).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&agreements, fs::Permissions::from_mode(0o700)).unwrap();
    }
    private(
        &agreements.join(format!(
            "{}.evidence",
            seller["deal_commitment"].as_str().unwrap()
        )),
        &selected.encode().unwrap(),
    );
    let payload = seller_root.join("payload");
    private(&payload, PAYLOAD);
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let url = format!("http://127.0.0.1:{port}");
    let service_config = seller_root.join("access.json");
    let service_id = [42; 32];
    let backend = if selected.terms.suite_id == 2 {
        json!({"mode":"shielded","chain_id":31337,"pool":config["pool"],"rpc_url":config["rpc_url"],
            "peer_rpc_url":config["peer_rpc_url"],"first_block":config["first_block"],
            "first_hash":hex::decode(config["first_hash"].as_str().unwrap().trim_start_matches("0x")).unwrap()})
    } else {
        json!({"mode":"public_bound","namespace":config["namespace"],"settlement_contract":config["settlement_contract"],
            "verifier_version":1,"rpc_url":config["peer_rpc_url"],"from_block":config["first_block"],
            "log_block_range":100,"max_log_queries":8,"max_ancestry":64})
    };
    private(&service_config,json!({"service_id":service_id,"seller_key":selected.terms.seller_authorization_key.as_bytes(),
        "suite_id":selected.terms.suite_id,"resource":selected.terms.service.resource,"payload_file":payload,"evidence_root":agreements,
        "state_root":seller_root.join("issuance"),"port":port,"backend":backend}).to_string().as_bytes());
    let process = service(&service_config, &url, client).await;
    let expiry = clock() + 120;
    let digest = request_digest(&selected, service_id, [43; 32], expiry).unwrap();
    let signature = if selected.terms.suite_id == 2 {
        erebus_core::shielded_auth::sign_message(&[61; 32], &digest)
            .unwrap()
            .1
            .to_vec()
    } else {
        AuthorizationIdentity::from_bytes(&[21; 32])
            .unwrap()
            .sign_digest(&digest)
            .to_vec()
    };
    let request = AccessRequest {
        deal_commitment: buyer["deal_commitment"].as_str().unwrap().into(),
        nonce: [43; 32],
        expires_at: expiry,
        signature,
    };
    let issued = client
        .post(format!("{url}/v1/access"))
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(issued.status(), 200);
    // Issuance persists before the response. A lost body and server restart cannot charge again.
    drop(issued);
    drop(process);
    let restarted = service(&service_config, &url, client).await;
    let retrieved = client
        .post(format!("{url}/v1/access"))
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(retrieved.status(), 200);
    let retrieved: Value = retrieved.json().await.unwrap();
    assert_eq!(retrieved["issuance"]["repeated"], true);
    let access = fixture.root.path().join("erebus-access");
    fs::copy(env!("CARGO_BIN_EXE_erebus-access"), &access).unwrap();
    let retrieval = json!({"method":"retrieve","evidence_file":buyer["evidence_file"],
        "buyer_key_file":fixture.root.path().join("buyer/agreement.key"),"service_url":format!("{url}/v1/access"),
        "service_id":hex::encode(service_id),"cache_root":fixture.root.path().join("buyer/content-cache"),"allow_loopback_http":true});
    let (ok, resource) =
        super::super::finish(spawn(&access, fixture.root.path(), retrieval.clone()));
    assert!(ok, "{resource}");
    assert_eq!(resource["result"]["resource_verified"], true);
    assert_eq!(resource["result"]["payment_verified"], false);
    assert_eq!(resource["result"]["delivery_verified"], false);
    assert_eq!(
        resource["result"]["issuance_id"],
        retrieved["issuance"]["issuance_id"]
    );
    assert_eq!(
        fs::read(resource["result"]["resource_file"].as_str().unwrap()).unwrap(),
        PAYLOAD
    );
    drop(restarted);
    fs::remove_file(fixture.root.path().join("buyer/agreement.key")).unwrap();
    let (ok, cached) = super::super::finish(spawn(&access, fixture.root.path(), retrieval));
    assert!(ok, "{cached}");
    assert_eq!(cached["result"]["cached"], true);
    // The auditor owns only its key, grant, and public history cache; participant paths go offline.
    let disclosure = fixture.root.path().join("erebus-disclosure");
    fs::copy(
        env!("CARGO_BIN_EXE_erebus-shielded-disclosure"),
        &disclosure,
    )
    .unwrap();
    let auditor = fixture.root.path().join("auditor");
    fs::create_dir(&auditor).unwrap();
    let auditor_key = auditor.join("auditor.key");
    let (ok, key) = super::super::finish(spawn(
        &disclosure,
        &auditor,
        json!({"method":"keygen","key_file":auditor_key}),
    ));
    assert!(ok, "{key}");
    let grant = auditor.join("deal.grant");
    let (ok, exported) = super::super::finish(spawn(
        &disclosure,
        &seller_root,
        json!({"method":"export","evidence_file":seller["evidence_file"],
        "issuer_key_file":seller_root.join("agreement.key"),"recipient_public_key":key["recipient_public_key"],
        "grant_file":grant,"expires_at":clock()+300}),
    ));
    assert!(ok, "{exported}");
    fs::rename(
        fixture.root.path().join("buyer"),
        fixture.root.path().join("buyer-offline"),
    )
    .unwrap();
    fs::rename(&seller_root, fixture.root.path().join("seller-offline")).unwrap();
    let deployment = if selected.terms.suite_id == 2 {
        json!({"namespace":config["namespace"],"settlement_contract":config["pool"],"verifier_version":2,
            "rpc_url":config["rpc_url"],"peer_rpc_url":config["peer_rpc_url"],"first_block":config["first_block"],
            "first_hash":config["first_hash"],"cache_root":auditor.join("public-history")})
    } else {
        json!({"namespace":config["namespace"],"settlement_contract":config["settlement_contract"],"verifier_version":1,
            "rpc_url":config["peer_rpc_url"],"from_block":config["first_block"],"log_block_range":100,"max_log_queries":8,
            "max_ancestry":64,"cache_root":auditor.join("public-history")})
    };
    let (ok, verified) = super::super::finish(spawn(
        &disclosure,
        &auditor,
        json!({"method":"verify_payment","grant_file":grant,"key_file":auditor_key,
        "expected_issuer":format!("0x{}",hex::encode(selected.terms.seller_authorization_key.as_bytes())),
        "deployment":deployment}),
    ));
    assert!(ok, "{verified}");
    assert_eq!(verified["agreement_verified"], true);
    assert_eq!(verified["payment_verified"], true);
    assert_eq!(verified["delivery_verified"], false);
    assert_eq!(verified["deal_commitment"], buyer["deal_commitment"]);
}
