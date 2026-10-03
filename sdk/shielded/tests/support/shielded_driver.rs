//! Actual negotiated suite-2 payment through copied native commands, with prototype keys.

use super::*;
use axum::{extract::Path as RoutePath, routing::get};
use erebus_core::shielded_auth::derive_key;
use erebus_evm::chain::TransactionKey;
use erebus_shielded_prover::wallet::{OwnedNote, WalletDomain, WalletStore};

/// A funded suite-2 buyer, a seller, and the buyer's shielded payment configuration on a fresh
/// Anvil. Nothing is negotiated yet. `block_time` lets finality advance without test-driven mining.
pub(super) struct Deployed {
    pub(super) fixture: Fixture,
    pub(super) config: Value,
    pub(super) config_path: PathBuf,
    pub(super) buyer_root: PathBuf,
    pub(super) url: String,
    pub(super) client: reqwest::Client,
    pub(super) wallet: WalletStore,
    pub(super) token_bytes: [u8; 20],
    pub(super) sends: Arc<AtomicUsize>,
    pub(super) corrupt_consumed: Arc<AtomicBool>,
    _anvil: ServiceProcess,
    tasks: [tokio::task::JoinHandle<()>; 2],
}

impl Drop for Deployed {
    fn drop(&mut self) {
        self.tasks.iter().for_each(|task| task.abort());
    }
}

pub(super) async fn deploy(lose_broadcast: bool, block_time: Option<u64>) -> Deployed {
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
        lose_broadcast: Arc::new(AtomicBool::new(lose_broadcast)),
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
    // Both are fixed before negotiation: the buyer's suite-2 agreement key file holds [61; 32].
    config["buyer_key_hex"] = json!(hex::encode(derive_key(&[61; 32]).unwrap()));
    config["asset"] = json!(fixture.template.asset.to_string());
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
    Deployed {
        fixture,
        config,
        config_path,
        buyer_root,
        url,
        client,
        wallet,
        token_bytes,
        sends,
        corrupt_consumed,
        _anvil: anvil,
        tasks: [proxy_task, artifact_task],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil, Node dependencies and existing M5 prototype proving artifacts"]
async fn negotiated_shielded_payment_proves_locally_and_recovers_without_a_second_send() {
    let deployed = deploy(true, None).await;
    let Deployed {
        fixture,
        config,
        config_path,
        buyer_root,
        url,
        client,
        wallet,
        token_bytes,
        sends,
        corrupt_consumed,
        ..
    } = &deployed;
    let (fixture, config, config_path, buyer_root, url, client, wallet, token_bytes) = (
        fixture,
        config,
        config_path,
        buyer_root,
        url,
        client,
        wallet,
        *token_bytes,
    );
    let (buyer, seller) = fixture.pair(false);
    let selected =
        SelectedAgreement::decode(&fs::read(buyer["evidence_file"].as_str().unwrap()).unwrap())
            .unwrap();
    assert_ne!(selected.terms.transcript_root, [0; 32]);
    assert_eq!(selected.terms.amount.get(), 70);
    assert_eq!(selected.messages.len(), 4);
    verify_selected_agreement(&selected).unwrap();
    let binary = fixture.root.path().join("erebus-payment");
    fs::copy(env!("CARGO_BIN_EXE_erebus-payment"), &binary).unwrap();
    let (_, funding) = run_payment(&binary, fixture.root.path(), config_path, "funding").await;
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
    let (_, submitted) = run_payment(&binary, fixture.root.path(), config_path, "settle").await;
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
    rpc(client, url, "anvil_mine", json!(["0x4"])).await;
    corrupt_consumed.store(true, Ordering::SeqCst);
    let (ok, rejected) = run_payment(&binary, fixture.root.path(), config_path, "settle").await;
    assert!(!ok, "{rejected}");
    assert_eq!(rejected["payment_verified"], false);
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    corrupt_consumed.store(false, Ordering::SeqCst);
    let (ok, recovered) = run_payment(&binary, fixture.root.path(), config_path, "settle").await;
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
    native_product::finish(fixture, &buyer, &seller, config, client).await;
    assert_eq!(sends.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires Anvil, Node, uv, and existing M5 prototype proving artifacts"]
async fn two_mcp_agents_negotiate_and_settle_once_through_a_lost_broadcast() {
    // Real block production, so the agents reach finality by observation alone.
    let deployed = deploy(true, Some(1)).await;
    let root = deployed.fixture.root.path();
    let payment = root.join("erebus-payment");
    fs::copy(env!("CARGO_BIN_EXE_erebus-payment"), &payment).unwrap();
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
        .arg("--seller-negotiation")
        .arg(&deployed.fixture.seller_config)
        .arg("--negotiation-cli")
        .arg(&deployed.fixture.binary)
        .arg("--payment-cli")
        .arg(&payment)
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
    let record: Value = serde_json::from_str(stdout.trim().lines().last().unwrap()).unwrap();
    eprintln!("two-agent MCP record (Anvil, known-entropy keys): {record}");
    assert_eq!(record["payment_verified"], true);
    assert_eq!(record["stage"], "Finalized");
    assert_eq!(record["settle_calls"], 1);
    assert_eq!(record["winning_commitment"], record["deal_commitment"]);
    // The lost acknowledgement made the first result uncertain; recovery was by observation.
    assert_eq!(record["settlement"]["submitted_this_call"], true);
    assert_eq!(record["settlement"]["payment_verified"], false);
    assert!(record["recover_calls"].as_u64().unwrap() >= 1);
    assert_eq!(deployed.sends.load(Ordering::SeqCst), 1);
    let notes = deployed.wallet.snapshot().unwrap();
    assert!(notes
        .notes()
        .iter()
        .any(|note| note.amount() == 80 && note.inclusion().is_some()));
}

/// Keeps a funded two-participant environment alive for externally driven agents.
/// Run with `EREBUS_AGENT_ENV_DIR=<new dir>`; create `<dir>/stop` to tear down.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "manual: holds Anvil and prototype artifacts open for external MCP agents"]
async fn hold_metropolis_agent_environment() {
    let Some(directory) = std::env::var_os("EREBUS_AGENT_ENV_DIR").map(PathBuf::from) else {
        return;
    };
    let deployed = deploy(true, Some(1)).await;
    let root = deployed.fixture.root.path();
    // Agents start independently, so give each side time to find its peer.
    for path in [
        &deployed.fixture.buyer_config,
        &deployed.fixture.seller_config,
    ] {
        let mut config: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        config["timeout_seconds"] = json!(300);
        fs::write(path, config.to_string()).unwrap();
    }
    let payment = root.join("erebus-payment");
    fs::copy(env!("CARGO_BIN_EXE_erebus-payment"), &payment).unwrap();
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("environment.json"),
        json!({"operation_ref":hex::encode(OPERATION),"buyer_negotiation":deployed.fixture.buyer_config,
            "buyer_payment":deployed.config_path,"seller_negotiation":deployed.fixture.seller_config,
            "negotiation_cli":deployed.fixture.binary,"payment_cli":payment})
        .to_string(),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(1800);
    while !directory.join("stop").exists() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let change_included = deployed
        .wallet
        .snapshot()
        .unwrap()
        .notes()
        .iter()
        .any(|note| note.amount() == 80 && note.inclusion().is_some());
    fs::write(
        directory.join("result.json"),
        json!({"raw_transaction_sends":deployed.sends.load(Ordering::SeqCst),"buyer_change_note_included":change_included})
            .to_string(),
    )
    .unwrap();
}
