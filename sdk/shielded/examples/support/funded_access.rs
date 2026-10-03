//! Test-only separate-process access issuance after a finalized shielded payment.

use std::{
    io::Write,
    net::TcpListener,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use erebus_core::{shielded_auth, terms::AgreementTerms};
use erebus_shielded_prover::access::{request_digest, AccessRequest};
use erebus_transport::disclosure::{verify_selected_agreement, SelectedAgreement};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

struct Service(Child);

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn private_file(path: &Path, bytes: &[u8]) {
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

async fn start(config: &Path, url: &str, client: &reqwest::Client) -> Service {
    let mut service = Service(
        Command::new(std::env::var_os("EREBUS_M8_ACCESS_BIN").expect("access service binary"))
            .env_clear()
            .env("EREBUS_ACCESS_CONFIG", config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    for _ in 0..100 {
        assert!(
            service.0.try_wait().unwrap().is_none(),
            "access service stopped"
        );
        if client.get(format!("{url}/healthz")).send().await.is_ok() {
            return service;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("access service did not become healthy");
}

async fn retrieve(request: Value) -> Value {
    tokio::task::spawn_blocking(move || {
        let mut child = Command::new(
            std::env::var_os("EREBUS_M8_ACCESS_CLIENT_BIN").expect("access client binary"),
        )
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
            .write_all(&serde_json::to_vec(&request).unwrap())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(output.stderr.is_empty());
        serde_json::from_slice(&output.stdout).unwrap()
    })
    .await
    .unwrap()
}

pub async fn run(
    root: &Path,
    terms: &AgreementTerms,
    rpc_url: &str,
    peer_rpc_url: &str,
    first_block: u64,
    first_hash: [u8; 32],
) {
    let evidence =
        SelectedAgreement::decode(&std::fs::read(root.join("disclosed.evidence")).unwrap())
            .unwrap();
    let agreement = verify_selected_agreement(&evidence).unwrap();
    let directory = root.join("access-evidence");
    std::fs::create_dir(&directory).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    private_file(
        &directory.join(format!("{}.evidence", agreement.commitment.to_hex())),
        &evidence.encode().unwrap(),
    );
    let payload = b"EREBUS_M8_TEST_ONLY_DATASET";
    assert_eq!(
        terms.service.fulfillment_digest,
        <[u8; 32]>::from(Sha256::digest(payload))
    );
    let payload_file = root.join("dataset.snapshot");
    private_file(&payload_file, payload);
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let service_id = [0x42; 32];
    let chain_id: u64 = terms
        .domain
        .namespace
        .to_string()
        .strip_prefix("eip155:")
        .unwrap()
        .parse()
        .unwrap();
    let config = root.join("access-config.json");
    private_file(
        &config,
        &serde_json::to_vec(&json!({
            "service_id":service_id, "seller_key":terms.seller_authorization_key.as_bytes(),
            "suite_id":2, "resource":terms.service.resource, "payload_file":payload_file,
            "evidence_root":directory, "state_root":root.join("access-state"), "port":port,
            "backend":{"mode":"shielded", "chain_id":chain_id,
                "pool":format!("0x{}",hex::encode(terms.domain.pool.as_ref().expect("shielded pool").as_bytes())),
                "rpc_url":rpc_url, "peer_rpc_url":peer_rpc_url,
                "first_block":first_block, "first_hash":first_hash}
        }))
        .unwrap(),
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let url = format!("http://127.0.0.1:{port}");
    let service = start(&config, &url, &client).await;
    let expires_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 300;
    let digest = request_digest(&evidence, service_id, [4; 32], expires_at).unwrap();
    let seed: [u8; 32] = Sha256::digest(b"EREBUS_M5_BUYER_TEST_ONLY").into();
    let request = AccessRequest {
        deal_commitment: agreement.commitment.to_hex(),
        nonce: [4; 32],
        expires_at,
        signature: shielded_auth::sign_message(&seed, &digest)
            .unwrap()
            .1
            .to_vec(),
    };
    let mut wrong = request.clone();
    wrong.signature = shielded_auth::sign_message(&[7; 32], &digest)
        .unwrap()
        .1
        .to_vec();
    assert_eq!(
        client
            .post(format!("{url}/v1/access"))
            .json(&wrong)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let mut issued = None;
    for _ in 0..60 {
        let response = client
            .post(format!("{url}/v1/access"))
            .json(&request)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body: Value = response.json().await.unwrap();
        if status == 200 {
            assert_eq!(body["payload_hex"], hex::encode(payload));
            assert_eq!(body["payment_verified"], true);
            assert_eq!(body["delivery_verified"], false);
            assert_eq!(body["issuance"]["repeated"], false);
            issued = Some(body["issuance"]["issuance_id"].clone());
            break;
        }
        assert_eq!(status, 202, "unexpected access failure: {body}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let issuance = issued.expect("funded shielded issuance");
    drop(service);
    let _restarted = start(&config, &url, &client).await;
    let response = client
        .post(format!("{url}/v1/access"))
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let recovered: Value = response.json().await.unwrap();
    assert_eq!(recovered["issuance"]["issuance_id"], issuance);
    assert_eq!(recovered["issuance"]["repeated"], true);
    assert_eq!(recovered["payload_hex"], hex::encode(payload));
    let evidence_file = directory.join(format!("{}.evidence", agreement.commitment.to_hex()));
    let key_file = root.join("disclosure-issuer.key");
    let retrieval = json!({"method":"retrieve", "evidence_file":evidence_file, "buyer_key_file":key_file,
        "service_url":format!("{url}/v1/access"),"service_id":hex::encode(service_id),
        "cache_root":root.join("buyer-access-cache"),"allow_loopback_http":true});
    let receipt = retrieve(retrieval.clone()).await;
    assert_eq!(receipt["result"]["resource_verified"], true);
    assert_eq!(receipt["result"]["payment_verified"], false);
    assert_eq!(receipt["result"]["cached"], false);
    assert_eq!(
        std::fs::read(receipt["result"]["resource_file"].as_str().unwrap()).unwrap(),
        payload
    );
    if let Ok(python) = std::env::var("EREBUS_TEST_MCP_PYTHON") {
        let arguments = [
            evidence_file.into_os_string(),
            key_file.into_os_string(),
            format!("{url}/v1/access").into(),
            hex::encode(service_id).into(),
            root.join("mcp-access-cache").into_os_string(),
            std::env::var_os("EREBUS_M8_ACCESS_CLIENT_BIN").unwrap(),
        ];
        let script = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../mcp-server/tests/access_probe.py");
        let output = tokio::task::spawn_blocking(move || {
            Command::new(python)
                .arg(script)
                .args(arguments)
                .output()
                .unwrap()
        })
        .await
        .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["mcp_access_verified"], true);
    }
    drop(_restarted);
    let cached = retrieve(retrieval).await;
    assert_eq!(cached["result"]["cached"], true);
    private_file(
        &root.join("access-report.json"),
        &serde_json::to_vec(&json!({
            "mode":"shielded", "payment_verified":true, "delivery_recovered":true,
            "wrong_buyer_rejected":true, "issuance_id":issuance, "buyer_resource_verified":true,
            "buyer_cache_recovered":true, "independent_payment_verified_by_buyer_client":false
        }))
        .unwrap(),
    );
    println!("M8 shielded access recovered one persisted issuance after restart");
}
