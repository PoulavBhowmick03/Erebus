//! Native buyer and seller processes over the canonical x402 runtime on Anvil.

#[path = "access_http.rs"]
mod support;

use erebus_core::{
    auth::{authorization_digest, Authorization, Role},
    commitment::{commit_agreement, CommitmentBlinding},
    domain::DeploymentDomain,
    ids::{AddressBytes, AssetId, BaseUnits, ChainNamespace, KeyBytes, SignatureBytes},
    terms::{AgreementTerms, FeePolicy},
};
use erebus_evm::{
    abi,
    x402::{EXACT_PERMIT2_PROXY, PERMIT2},
};
use erebus_shielded_prover::access::{
    client::AccessClient, request_digest, AccessPolicy, AccessRequest,
};
use erebus_transport::{disclosure::SelectedAgreement, identity::AuthorizationIdentity};
use serde_json::{json, Value};
use std::{
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

struct Proxy {
    url: String,
    sends: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

struct StopWrite {
    at: usize,
    count: AtomicUsize,
}
impl erebus_journal::FaultHook for StopWrite {
    fn after(&self, _: erebus_journal::Boundary<'_>) -> std::io::Result<()> {
        if self.count.fetch_add(1, Ordering::SeqCst) + 1 == self.at {
            Err(std::io::Error::other("injected durable-write failure"))
        } else {
            Ok(())
        }
    }
}
impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn lossy_proxy(upstream: String) -> Proxy {
    use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
    #[derive(Clone)]
    struct StateData {
        url: String,
        client: reqwest::Client,
        sends: Arc<AtomicUsize>,
    }
    async fn forward(
        State(state): State<StateData>,
        Json(body): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        let reply: Value = state
            .client
            .post(&state.url)
            .json(&body)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if body["method"] == "eth_sendRawTransaction" {
            state.sends.fetch_add(1, Ordering::SeqCst);
            // Forwarded, then response lost: the facilitator must recover without another send.
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error":"lost acknowledgement"})),
            )
        } else {
            (StatusCode::OK, Json(reply))
        }
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let sends = Arc::new(AtomicUsize::new(0));
    let state = StateData {
        url: upstream,
        client: reqwest::Client::new(),
        sends: sends.clone(),
    };
    let router = Router::new().route("/", post(forward)).with_state(state);
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Proxy { url, sends, task }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil and forge-built artifacts; no live network writes"]
async fn x402_http_submits_once_recovers_restart_and_rejects_mutations() {
    let client = reqwest::Client::new();
    let port = support::port();
    let _anvil = support::Process(
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
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let runtime: Value = serde_json::from_str(include_str!(
        "../../evm/tests/fixtures/x402-canonical-runtime.json"
    ))
    .unwrap();
    let hash = |name: &str| -> [u8; 32] {
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
    for name in ["permit2", "x402_exact_permit2_proxy"] {
        support::rpc(
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
    let accounts = support::rpc(&client, &rpc_url, "eth_accounts", json!([])).await;
    let account = accounts[0].as_str().unwrap();
    let gas_account = accounts[1].as_str().unwrap();
    let buyer_seed: [u8; 32] =
        hex::decode("ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80")
            .unwrap()
            .try_into()
            .unwrap();
    let gas_seed =
        hex::decode("59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d").unwrap();
    let buyer = AuthorizationIdentity::from_bytes(&buyer_seed).unwrap();
    let seller = AuthorizationIdentity::from_bytes(&[2; 32]).unwrap();
    let token = support::deploy(
        &client,
        &rpc_url,
        account,
        "MockERC20",
        abi::encode_token_constructor("Test", "TEST"),
    )
    .await;
    let token = token["contractAddress"].as_str().unwrap();
    support::send(
        &client,
        &rpc_url,
        account,
        Some(token),
        abi::encode_mint_call(&buyer.address(), 1000),
    )
    .await;
    support::send(
        &client,
        &rpc_url,
        account,
        Some(token),
        abi::encode_approve_call(&PERMIT2, 1000),
    )
    .await;
    support::rpc(&client, &rpc_url, "anvil_mine", json!(["0x2"])).await;
    let proxy = lossy_proxy(rpc_url.clone()).await;
    let fixture: Value = serde_json::from_str(include_str!(
        "../../core/tests/fixtures/agreement-v1-vectors.json"
    ))
    .unwrap();
    let mut terms = AgreementTerms::decode(
        &hex::decode(
            fixture["vectors"][0]["expected"]["canonicalHex"]
                .as_str()
                .unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    terms.domain = DeploymentDomain {
        namespace: ChainNamespace::parse("eip155:31337").unwrap(),
        settlement_contract: Some(AddressBytes::new(EXACT_PERMIT2_PROXY.to_vec()).unwrap()),
        pool: None,
        verifier_version: 1,
    };
    terms.asset = AssetId::new(terms.domain.namespace.clone(), "erc20", token).unwrap();
    terms.amount = BaseUnits::new(70);
    terms.expiry = now() + 60;
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
        payload: b"x402 immutable feed snapshot".to_vec(),
    };
    terms.service.resource = policy.resource.clone();
    terms.service.unit = "snapshot".into();
    terms.service.quantity = BaseUnits::new(1);
    terms.service.access_recipient = terms.buyer_authorization_key.clone();
    terms.service.fulfillment_method = "http-access-v1".into();
    terms.service.fulfillment_digest = policy.resource_hash();
    terms.service.delivery_deadline = now() + 7200;
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
    support::private_file(
        &evidence_root.join(format!("{}.evidence", commitment.to_hex())),
        &evidence.encode().unwrap(),
    );
    let evidence_file = root.path().join("buyer.evidence");
    support::private_file(&evidence_file, &evidence.encode().unwrap());
    let buyer_key = root.path().join("buyer.key");
    support::private_file(&buyer_key, &buyer_seed);
    let gas_key = root.path().join("gas.key");
    support::private_file(&gas_key, &gas_seed);
    let payload = root.path().join("payload");
    support::private_file(&payload, &policy.payload);
    let service_port = support::port();
    let url = format!("http://127.0.0.1:{service_port}");
    let service_config = root.path().join("service.json");
    support::private_file(&service_config, json!({"service_id":policy.service_id,"seller_key":seller.address(),"suite_id":1,
        "resource":policy.resource,"payload_file":payload,"evidence_root":evidence_root,"state_root":root.path().join("service-state"),"port":service_port,
        "backend":{"mode":"x402_exact","namespace":"eip155:31337","rpc_url":proxy.url,"peer_rpc_url":rpc_url,
            "permit2_runtime_hash":hash("permit2"),"proxy_runtime_hash":hash("x402_exact_permit2_proxy"),
            "transaction_key_file":gas_key,"signer_journal_root":root.path().join("signer"),"gas_limit":500000,
            "max_fee_per_gas":"3000000000","max_priority_fee_per_gas":"1000000000"}}).to_string().as_bytes());
    let process = support::service(&service_config, &url, &client).await;
    let access = AccessClient::open(
        &format!("{url}/v1/access"),
        policy.service_id,
        root.path().join("cache"),
        true,
    )
    .unwrap();
    let payment = access.prepare_x402_payment(&evidence, &buyer_seed).unwrap();
    for at in 1..=4 {
        let cache = root.path().join(format!("permit-fault-{at}"));
        let fault = AccessClient::with_faults(
            &format!("{url}/v1/access"),
            policy.service_id,
            &cache,
            true,
            Arc::new(StopWrite {
                at,
                count: AtomicUsize::new(0),
            }),
        )
        .unwrap();
        assert!(fault.prepare_x402_payment(&evidence, &buyer_seed).is_err());
        drop(fault);
        let restored =
            AccessClient::open(&format!("{url}/v1/access"), policy.service_id, cache, true)
                .unwrap();
        assert_eq!(
            restored
                .prepare_x402_payment(&evidence, &buyer_seed)
                .unwrap()
                .encode(),
            payment.encode()
        );
    }
    assert_eq!(
        payment.encode(),
        access
            .prepare_x402_payment(&evidence, &buyer_seed)
            .unwrap()
            .encode()
    );
    let signed_request = |payment: Option<erebus_shielded_prover::access::X402Payment>| {
        let expires_at = now() + 120;
        let signature = buyer
            .sign_digest(
                &request_digest(
                    &evidence,
                    policy.service_id,
                    [4; 32],
                    expires_at,
                    payment.as_ref(),
                )
                .unwrap(),
            )
            .to_vec();
        AccessRequest {
            deal_commitment: commitment.to_hex(),
            nonce: [4; 32],
            expires_at,
            signature,
            payment,
        }
    };
    let initial_nonce = support::rpc(
        &client,
        &rpc_url,
        "eth_getTransactionCount",
        json!([gas_account, "latest"]),
    )
    .await;
    let ordinary = client
        .post(format!("{url}/v1/access"))
        .json(&signed_request(None))
        .send()
        .await
        .unwrap();
    assert_eq!(ordinary.status(), 402);
    assert!(ordinary.headers().contains_key("PAYMENT-REQUIRED"));
    let missing_header = client
        .post(format!("{url}/v1/access"))
        .json(&signed_request(Some(payment.clone())))
        .send()
        .await
        .unwrap();
    assert_eq!(missing_header.status(), 401);
    let mismatched_header = client
        .post(format!("{url}/v1/access"))
        .header("PAYMENT-SIGNATURE", "e30=")
        .json(&signed_request(Some(payment.clone())))
        .send()
        .await
        .unwrap();
    assert_eq!(mismatched_header.status(), 401);
    for changed in ["amount", "recipient", "deadline", "signature"] {
        let mut bad = payment.clone();
        match changed {
            "amount" => bad.amount += 1,
            "recipient" => bad.to = [99; 20],
            "deadline" => bad.deadline += 1,
            _ => bad.signature[0] ^= 1,
        }
        let header =
            erebus_shielded_prover::access::x402::payment_signature_header(&evidence.terms, &bad)
                .unwrap();
        let response = client
            .post(format!("{url}/v1/access"))
            .header("PAYMENT-SIGNATURE", header)
            .json(&signed_request(Some(bad)))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 401, "{changed}");
    }
    assert_eq!(
        support::rpc(
            &client,
            &rpc_url,
            "eth_getTransactionCount",
            json!([gas_account, "latest"])
        )
        .await,
        initial_nonce
    );
    let request = signed_request(Some(payment.clone()));
    let header =
        erebus_shielded_prover::access::x402::payment_signature_header(&evidence.terms, &payment)
            .unwrap();
    let response = client
        .post(format!("{url}/v1/access"))
        .header("PAYMENT-SIGNATURE", &header)
        .json(&request)
        .send()
        .await
        .unwrap();
    assert!(response.status() == 202 || response.status() == 200);
    drop(response); // Lost response: authorization and broadcast fence already exist on disk.
    drop(process);
    support::rpc(&client, &rpc_url, "anvil_mine", json!(["0x3"])).await;
    let _restarted = support::service(&service_config, &url, &client).await;
    let mut response = client
        .post(format!("{url}/v1/access"))
        .header("PAYMENT-SIGNATURE", &header)
        .json(&request)
        .send()
        .await
        .unwrap();
    for _ in 0..20 {
        if response.status() == 200 {
            break;
        }
        assert_eq!(response.status(), 202);
        support::rpc(&client, &rpc_url, "anvil_mine", json!(["0x1"])).await;
        response = client
            .post(format!("{url}/v1/access"))
            .header("PAYMENT-SIGNATURE", &header)
            .json(&request)
            .send()
            .await
            .unwrap();
    }
    assert_eq!(response.status(), 200);
    assert!(response.headers().contains_key("PAYMENT-RESPONSE"));
    let first: Value = response.json().await.unwrap();
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
        first["issuance"]["issuance_id"],
        repeated["issuance"]["issuance_id"]
    );
    let retrieval = json!({"method":"retrieve","evidence_file":evidence_file,"buyer_key_file":buyer_key,
        "service_url":format!("{url}/v1/access"),"service_id":hex::encode(policy.service_id),"cache_root":root.path().join("cache"),
        "allow_loopback_http":true,"x402_exact":true});
    let retrieved = support::retrieve_command(&retrieval);
    assert_eq!(retrieved["result"]["resource_verified"], true);
    assert_eq!(retrieved["result"]["payment_verified"], false);
    let final_nonce = support::rpc(
        &client,
        &rpc_url,
        "eth_getTransactionCount",
        json!([gas_account, "latest"]),
    )
    .await;
    assert_eq!(
        u64::from_str_radix(final_nonce.as_str().unwrap().trim_start_matches("0x"), 16).unwrap(),
        u64::from_str_radix(initial_nonce.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
            + 1
    );
    let balance = support::rpc(&client, &rpc_url, "eth_call", json!([{"to":token,"data":format!("0x{}",hex::encode(abi::encode_balance_of_call(&seller.address())))},"latest"])).await;
    assert_eq!(
        u128::from_str_radix(balance.as_str().unwrap().trim_start_matches("0x"), 16).unwrap(),
        70
    );
    assert_eq!(proxy.sends.load(Ordering::SeqCst), 1);
}
