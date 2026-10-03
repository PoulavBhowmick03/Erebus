//! Actual negotiated suite-2 payment through copied native commands, with prototype keys.

use super::*;
use axum::{extract::Path as RoutePath, routing::get};
use erebus_core::shielded_auth::derive_key;
use erebus_evm::chain::TransactionKey;
use erebus_shielded_prover::wallet::{OwnedNote, WalletDomain, WalletStore};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil, Node dependencies and existing M5 prototype proving artifacts"]
async fn negotiated_shielded_payment_proves_locally_and_recovers_without_a_second_send() {
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
    let url = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    for _ in 0..100 {
        if client
            .post(&url)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}))
            .send()
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
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
        "fixture: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let deployment: Value = serde_json::from_slice(&output.stdout).unwrap();
    rpc(&client, &url, "anvil_mine", json!(["0x3"])).await;
    let pool = deployment["pool"].as_str().unwrap();
    let token = deployment["token"].as_str().unwrap();
    let pool_bytes: [u8; 20] = hex::decode(pool.trim_start_matches("0x"))
        .unwrap()
        .try_into()
        .unwrap();
    let token_bytes: [u8; 20] = hex::decode(token.trim_start_matches("0x"))
        .unwrap()
        .try_into()
        .unwrap();
    let mut template = terms(2, 60);
    template.domain = DeploymentDomain {
        namespace: ChainNamespace::parse("eip155:31337").unwrap(),
        settlement_contract: Some(AddressBytes::new(pool_bytes.to_vec()).unwrap()),
        pool: Some(AddressBytes::new(pool_bytes.to_vec()).unwrap()),
        verifier_version: 2,
    };
    template.asset = AssetId::parse(&format!("eip155:31337/erc20:{token}")).unwrap();
    template.service.unit = "snapshot".into();
    template.service.quantity = BaseUnits::new(1);
    template.service.fulfillment_method = "http-access-v1".into();
    template.service.fulfillment_digest = Sha256::digest(native_product::PAYLOAD).into();
    let fixture = Fixture::with_terms(template);
    let (buyer, seller) = fixture.pair(false);
    let selected =
        SelectedAgreement::decode(&fs::read(buyer["evidence_file"].as_str().unwrap()).unwrap())
            .unwrap();
    assert_ne!(selected.terms.transcript_root, [0; 32]);
    assert_eq!(selected.terms.amount.get(), 70);
    assert_eq!(selected.messages.len(), 4);
    verify_selected_agreement(&selected).unwrap();
    let buyer_root = fixture.root.path().join("buyer");
    private(&buyer_root.join("wallet.key"), &[9; 32]);
    let wallet_path = buyer_root.join("state/wallet/notes.enc");
    let wallet = WalletStore::new(
        &wallet_path,
        WalletDomain {
            chain_id: 31337,
            pool: pool_bytes,
        },
        [9; 32],
    )
    .unwrap();
    wallet
        .update(|wallet| {
            wallet.add(OwnedNote::new(
                token_bytes,
                150,
                derive_key(&[61; 32]).unwrap(),
                [6; 32],
                [7; 32],
            )?)
        })
        .unwrap();
    let key = TransactionKey::from_bytes(&[24; 32]).unwrap();
    private(&buyer_root.join("transaction.key"), &[24; 32]);
    let signer = format!("0x{}", hex::encode(key.address()));
    rpc(
        &client,
        &url,
        "anvil_setBalance",
        json!([signer, "0x56bc75e2d63100000"]),
    )
    .await;
    rpc(&client, &url, "anvil_mine", json!(["0x3"])).await;
    let sends = Arc::new(AtomicUsize::new(0));
    let corrupt_consumed = Arc::new(AtomicBool::new(false));
    let proxy = Proxy {
        upstream: url.clone(),
        client: client.clone(),
        lose_broadcast: Arc::new(AtomicBool::new(true)),
        sends: sends.clone(),
        minimum_log_block: Arc::new(std::sync::Mutex::new(None)),
        corrupt_code: Arc::new(AtomicBool::new(false)),
        corrupt_consumed: corrupt_consumed.clone(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let peer_url = format!("http://{}", listener.local_addr().unwrap());
    let proxy_task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/", post(forward)).with_state(proxy),
        )
        .await
        .unwrap();
    });
    // The primary sends through the lossy proxy; the peer reads the honest upstream.
    let build = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../circuits/m5/build");
    let bodies: std::collections::HashMap<String, Vec<u8>> = [
        ("transfer.wasm", build.join("transfer_js/transfer.wasm")),
        ("transfer.r1cs", build.join("transfer.r1cs")),
        ("transfer.zkey", build.join("transfer.zkey")),
    ]
    .into_iter()
    .map(|(name, path)| (name.into(), fs::read(path).unwrap()))
    .collect();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let artifact_url = format!("http://{}", listener.local_addr().unwrap());
    let artifact_task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route(
                    "/{name}",
                    get(
                        |State(bodies): State<Arc<std::collections::HashMap<String, Vec<u8>>>>,
                         RoutePath(name): RoutePath<String>| async move {
                            bodies.get(&name).cloned().unwrap_or_default()
                        },
                    ),
                )
                .with_state(Arc::new(bodies)),
        )
        .await
        .unwrap();
    });
    let circuits: Vec<Value> = ["deposit","transfer","withdraw"].into_iter().map(|kind| {
        let mut circuit = json!({"kind":kind});
        for (name,path) in [("wasm",build.join(format!("{kind}_js/{kind}.wasm"))),("r1cs",build.join(format!("{kind}.r1cs"))),("zkey",build.join(format!("{kind}.zkey")))] {
            let body = fs::read(path).unwrap();
            circuit[name] = json!({"url":format!("{artifact_url}/{kind}.{name}"),"bytes":body.len(),"sha256":hex::encode(Sha256::digest(&body))});
        }
        circuit
    }).collect();
    let manifest = serde_json::to_vec(&json!({"version":1,"chain_id":31337,"settlement_contract":pool,"verifier_version":2,"test_only":true,"circuits":circuits})).unwrap();
    private(&buyer_root.join("manifest.json"), &manifest);
    let mut config = deployment.clone();
    config["version"] = json!(1);
    config["mode"] = json!("shielded");
    config.as_object_mut().unwrap().remove("token");
    config["state_root"] = json!(buyer_root.join("state"));
    config["namespace"] = json!("eip155:31337");
    config["verifier_version"] = json!(2);
    config["rpc_url"] = json!(peer_url);
    config["peer_rpc_url"] = json!(url);
    config["buyer_key_hex"] = json!(hex::encode(
        selected.terms.buyer_authorization_key.as_bytes()
    ));
    config["asset"] = json!(selected.terms.asset.to_string());
    config["signer_address"] = json!(signer);
    config["signer_journal_root"] = json!(fixture.root.path().join("shared-signer"));
    config["transaction_key_file"] = json!(buyer_root.join("transaction.key"));
    config["wallet_file"] = json!(wallet_path);
    config["wallet_key_file"] = json!(buyer_root.join("wallet.key"));
    config["manifest_file"] = json!(buyer_root.join("manifest.json"));
    config["manifest_sha256"] = json!(hex::encode(Sha256::digest(&manifest)));
    config["artifact_cache"] = json!(buyer_root.join("artifact-cache"));
    config["allow_test_artifacts"] = json!(true);
    config["allow_loopback_http"] = json!(true);
    config["maximum_price"] = json!("75");
    config["gas_limit"] = json!(3_000_000);
    config["max_fee_per_gas"] = json!("2000000000");
    config["max_priority_fee_per_gas"] = json!("1000000000");
    config["timeout_seconds"] = json!(10);
    config["max_scan_blocks"] = json!(1000);
    let config_path = buyer_root.join("payment.json");
    private(&config_path, &serde_json::to_vec(&config).unwrap());
    let binary = fixture.root.path().join("erebus-payment");
    fs::copy(env!("CARGO_BIN_EXE_erebus-payment"), &binary).unwrap();
    let (_, funding) = run_payment(&binary, fixture.root.path(), &config_path, "funding").await;
    assert_eq!(funding["status"], "ready", "{funding}");
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    assert!(!buyer_root.join("artifact-cache").exists());
    let mut bad = config.clone();
    bad["transfer_verifier_keccak256"] = json!("01".repeat(32));
    let bad_path = buyer_root.join("bad-pins.json");
    private(&bad_path, &serde_json::to_vec(&bad).unwrap());
    let (ok, rejected) = run_payment(&binary, fixture.root.path(), &bad_path, "settle").await;
    assert!(!ok, "{rejected}");
    assert_eq!(rejected["payment_verified"], false);
    bad = config.clone();
    bad["allow_test_artifacts"] = json!(false);
    let bad_path = buyer_root.join("no-test-opt-in.json");
    private(&bad_path, &serde_json::to_vec(&bad).unwrap());
    let (ok, rejected) = run_payment(&binary, fixture.root.path(), &bad_path, "settle").await;
    assert!(!ok, "{rejected}");
    assert!(wallet
        .snapshot()
        .unwrap()
        .select(&token_bytes, 150)
        .is_some());
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    let (_, submitted) = run_payment(&binary, fixture.root.path(), &config_path, "settle").await;
    assert_eq!(submitted["submitted_this_call"], true, "{submitted}");
    assert_eq!(submitted["payment_verified"], false);
    assert!(
        submitted["measurements_ms"]["proof_preparation"]
            .as_u64()
            .unwrap()
            > 0
    );
    eprintln!(
        "native shielded debug-mode timings (Anvil, known-entropy keys): {}",
        submitted["measurements_ms"]
    );
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    for field in [
        "amount",
        "signature",
        "blinding",
        "witness",
        "spend_secret",
        "rpc_url",
    ] {
        assert!(submitted.get(field).is_none());
    }
    fs::remove_file(buyer_root.join("transaction.key")).unwrap();
    fs::remove_file(buyer_root.join("manifest.json")).unwrap();
    fs::rename(
        buyer_root.join("state/transcripts"),
        buyer_root.join("transcripts-offline"),
    )
    .unwrap();
    fs::rename(
        buyer_root.join("artifact-cache"),
        buyer_root.join("artifacts-offline"),
    )
    .unwrap();
    rpc(&client, &url, "anvil_mine", json!(["0x4"])).await;
    corrupt_consumed.store(true, Ordering::SeqCst);
    let (ok, rejected) = run_payment(&binary, fixture.root.path(), &config_path, "settle").await;
    assert!(!ok, "{rejected}");
    assert_eq!(rejected["payment_verified"], false);
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    corrupt_consumed.store(false, Ordering::SeqCst);
    let (ok, recovered) = run_payment(&binary, fixture.root.path(), &config_path, "settle").await;
    assert!(ok, "{recovered}");
    assert_eq!(recovered["payment_verified"], true);
    assert_eq!(recovered["submitted_this_call"], false);
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let notes = wallet.snapshot().unwrap();
    assert!(notes
        .notes()
        .iter()
        .any(|note| note.amount() == 80 && note.inclusion().is_some()));
    assert_eq!(notes.select(&token_bytes, 80).unwrap().amount(), 80);
    native_product::finish(&fixture, &buyer, &seller, &config, &client).await;
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    proxy_task.abort();
    artifact_task.abort();
}
