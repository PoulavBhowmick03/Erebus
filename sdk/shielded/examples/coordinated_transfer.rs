//! Test-only funded pool settlement, launched by the M5 Anvil runner.

#[path = "support/funded_faults.rs"]
mod funded_faults;

use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use axum::{extract::State, routing::post, Json, Router};

use erebus_coordinator::Coordinator;
use erebus_core::{
    auth::{Authorization, Role},
    commitment::{commit_agreement, CommitmentBlinding},
    domain::DeploymentDomain,
    ids::{AddressBytes, AssetId, BaseUnits, ChainNamespace, KeyBytes, SignatureBytes},
    policy::SpendingPolicy,
    service::ServiceRecord,
    settlement::SettlementContext,
    terms::{AgreementTerms, FeePolicy, GuaranteeSet, SettlementMode},
};
use erebus_evm::chain::{Eip1559Fees, SignerJournal, TransactionKey};
use erebus_shielded_prover::{
    chain::ShieldedChain,
    index_store::{IndexDomain, IndexStore},
    preparation::{capabilities, prepare_coordinated_transfer},
    recovery::{recover_wallet, sync_public_index},
    rpc::PoolRpc,
    wallet::{OwnedNote, WalletDomain, WalletStore},
    ChangeNote, ProvingArtifacts, WalletTransferRequest,
};
use erebus_transport::{
    disclosure::SelectedAgreement,
    message::Message,
    store::{FileTranscriptStore, TranscriptStore},
};
use num_bigint::BigUint;
use serde_json::Value;
use sha2::{Digest, Sha256};

fn disclosure_command(
    binary: &std::path::Path,
    directory: &std::path::Path,
    request: Value,
) -> Value {
    let mut child = Command::new(binary)
        .current_dir(directory)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("disclosure command");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&request).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "disclosure failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(output.stderr.is_empty(), "disclosure stderr");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn funded_disclosure(
    root: &std::path::Path,
    terms: &AgreementTerms,
    operation_ref: [u8; 32],
    rpc_url: &str,
    peer_rpc_url: &str,
    first_block: u64,
    first_hash: [u8; 32],
) {
    let binary = PathBuf::from(
        std::env::var_os("EREBUS_M7_DISCLOSURE_BIN").expect("shielded disclosure binary"),
    );
    let auditor = tempfile_auditor(root);
    let key = disclosure_command(
        &binary,
        &auditor,
        serde_json::json!({"method":"keygen", "key_file":"auditor.key"}),
    );
    disclosure_command(
        &binary,
        root,
        serde_json::json!({
            "method":"select", "state_root":root.join("coordinator"), "operation_ref":hex::encode(operation_ref),
            "store_root":root.join("transcript"), "namespace":"m5-funded", "transcript_hash_version":1, "evidence_file":"disclosed.evidence",
        }),
    );
    let seed: [u8; 32] = Sha256::digest(b"EREBUS_M5_BUYER_TEST_ONLY").into();
    let issuer_file = root.join("disclosure-issuer.key");
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(&issuer_file)
        .unwrap()
        .write_all(&seed)
        .unwrap();
    disclosure_command(
        &binary,
        root,
        serde_json::json!({
            "method":"export", "evidence_file":"disclosed.evidence", "issuer_key_file":"disclosure-issuer.key",
            "recipient_public_key":key["recipient_public_key"], "expires_at":4_102_444_800u64, "grant_file":auditor.join("deal.grant"),
        }),
    );
    assert_eq!(fs::read_dir(&auditor).unwrap().count(), 2);
    let issuer = format!(
        "0x{}",
        hex::encode(terms.buyer_authorization_key.as_bytes())
    );
    let agreement_request = serde_json::json!({"method":"verify_agreement", "grant_file":"deal.grant", "key_file":"auditor.key", "expected_issuer":issuer});
    let mut payment_request = agreement_request.clone();
    payment_request["method"] = serde_json::json!("verify_payment");
    payment_request["deployment"] = serde_json::json!({
        "namespace":terms.domain.namespace.to_string(),
        "settlement_contract":format!("0x{}", hex::encode(terms.domain.settlement_contract.as_ref().unwrap().as_bytes())),
        "verifier_version":terms.domain.verifier_version, "rpc_url":rpc_url, "peer_rpc_url":peer_rpc_url,
        "first_block":first_block, "first_hash":format!("0x{}", hex::encode(first_hash)), "cache_root":auditor.join("public-cache"),
    });
    // The auditor has no path to the participant state under its configured name.
    let offline = root.with_extension("participant-offline");
    fs::rename(root, &offline).unwrap();
    let agreement = disclosure_command(&binary, &auditor, agreement_request);
    let payment = disclosure_command(&binary, &auditor, payment_request.clone());
    let repeated = disclosure_command(&binary, &auditor, payment_request.clone());
    assert_eq!(agreement["agreement_verified"], true);
    assert_eq!(agreement["payment_verified"], false);
    assert_eq!(payment["mode"], "shielded");
    assert_eq!(payment["agreement_verified"], true);
    assert_eq!(payment["payment_verified"], true);
    assert_eq!(payment["delivery_verified"], false);
    assert_eq!(payment, repeated);
    assert_eq!(payment["deal_commitment"], agreement["deal_commitment"]);
    if let Some(python) = std::env::var_os("EREBUS_TEST_MCP_PYTHON") {
        let script =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/check-disclosure-mcp.py");
        let output = Command::new(python)
            .env_clear()
            .current_dir(&auditor)
            .arg(script)
            .arg(&binary)
            .arg(&auditor)
            .arg(&issuer)
            .arg(serde_json::to_string(&payment_request["deployment"]).unwrap())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let facts: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(facts["payment_verified"], true);
        assert_eq!(facts["delivery_verified"], false);
    }
    fs::rename(&offline, root).unwrap();
    fs::write(
        root.join("disclosure-report.json"),
        serde_json::to_vec_pretty(&payment).unwrap(),
    )
    .unwrap();
    println!("M7 independent shielded auditor verified agreement and funded payment without participant state or prover");
}

fn tempfile_auditor(root: &std::path::Path) -> PathBuf {
    let directory = root.with_extension("auditor");
    fs::create_dir(&directory).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    }
    directory
}

#[derive(Clone)]
struct FaultSource {
    upstream: String,
    client: reqwest::Client,
    selected: Arc<Mutex<Option<&'static str>>>,
    sends: Arc<AtomicUsize>,
}

async fn fault_reply(State(state): State<FaultSource>, Json(request): Json<Value>) -> Json<Value> {
    let method = request["method"].as_str().expect("method");
    let selected = *state.selected.lock().expect("fault selection");
    if method == "eth_sendRawTransaction" {
        state.sends.fetch_add(1, Ordering::SeqCst);
    }
    if selected == Some(method) && method != "eth_sendRawTransaction" {
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let response: Value = state
        .client
        .post(&state.upstream)
        .json(&request)
        .send()
        .await
        .expect("upstream request")
        .json()
        .await
        .expect("upstream JSON");
    if selected == Some("eth_sendRawTransaction") && method == "eth_sendRawTransaction" {
        // The transaction reached Anvil, but its response misses the client's deadline.
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    Json(response)
}

struct FaultProxy {
    url: String,
    state: FaultSource,
    task: tokio::task::JoinHandle<()>,
}

impl FaultProxy {
    async fn start(upstream: &str) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("proxy bind");
        let url = format!("http://{}", listener.local_addr().expect("proxy address"));
        let state = FaultSource {
            upstream: upstream.to_owned(),
            client: reqwest::Client::new(),
            selected: Arc::new(Mutex::new(Some("eth_sendRawTransaction"))),
            sends: Arc::new(AtomicUsize::new(0)),
        };
        let app = Router::new()
            .route("/", post(fault_reply))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("proxy serve");
        });
        Self { url, state, task }
    }
}

impl Drop for FaultProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn bytes<const N: usize>(value: &str) -> [u8; N] {
    hex::decode(value).expect("hex").try_into().expect("width")
}

fn text<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field].as_str().expect("fixture text")
}

fn word(value: &Value) -> [u8; 32] {
    let integer = text_or_decimal(value).parse::<BigUint>().expect("decimal");
    let data = integer.to_bytes_be();
    let mut out = [0; 32];
    out[32 - data.len()..].copy_from_slice(&data);
    out
}

fn test_field(name: &str) -> [u8; 32] {
    let digest = Sha256::digest(format!("EREBUS_M5_TEST_ONLY_{name}").as_bytes());
    let modulus = BigUint::parse_bytes(
        b"21888242871839275222246405745257275088548364400416034343698204186575808495617",
        10,
    )
    .expect("field modulus");
    let data = (BigUint::from_bytes_be(&digest) % modulus).to_bytes_be();
    let mut out = [0; 32];
    out[32 - data.len()..].copy_from_slice(&data);
    out
}

fn text_or_decimal(value: &Value) -> &str {
    value.as_str().expect("decimal string")
}

fn terms(input: &Value) -> AgreementTerms {
    let data = &input["terms"];
    let domain = &data["domain"];
    let service = &data["service"];
    AgreementTerms {
        protocol_version: data["protocolVersion"].as_u64().expect("version") as u16,
        suite_id: data["suiteId"].as_u64().expect("suite") as u16,
        domain: DeploymentDomain {
            namespace: ChainNamespace::parse(text(domain, "namespace")).expect("namespace"),
            settlement_contract: Some(
                AddressBytes::new(
                    hex::decode(text(domain, "settlementContractHex")).expect("contract"),
                )
                .expect("contract"),
            ),
            pool: Some(
                AddressBytes::new(hex::decode(text(domain, "poolHex")).expect("pool"))
                    .expect("pool"),
            ),
            verifier_version: domain["verifierVersion"].as_u64().expect("version") as u32,
        },
        deal_id: bytes(text(data, "dealIdHex")),
        revision: data["revision"].as_u64().expect("revision") as u32,
        transcript_root: bytes(text(data, "transcriptRootHex")),
        buyer_authorization_key: KeyBytes::new(
            hex::decode(text(data, "buyerAuthorizationKeyHex")).expect("buyer key"),
        )
        .expect("buyer key"),
        seller_authorization_key: KeyBytes::new(
            hex::decode(text(data, "sellerAuthorizationKeyHex")).expect("seller key"),
        )
        .expect("seller key"),
        payment_recipient: KeyBytes::new(
            hex::decode(text(data, "paymentRecipientHex")).expect("recipient"),
        )
        .expect("recipient"),
        asset: AssetId::parse(text(data, "asset")).expect("asset"),
        amount: BaseUnits::new(text(data, "amount").parse().expect("amount")),
        expiry: data["expiry"].as_u64().expect("expiry"),
        fee_policy: FeePolicy::none(),
        settlement_mode: SettlementMode::Shielded,
        required_guarantees: GuaranteeSet::from_bits(7).expect("guarantees"),
        settlement_nonce: bytes(text(data, "settlementNonceHex")),
        service: ServiceRecord {
            resource: text(service, "resource").to_owned(),
            quantity: BaseUnits::new(text(service, "quantity").parse().expect("quantity")),
            unit: text(service, "unit").to_owned(),
            access_recipient: KeyBytes::new(
                hex::decode(text(service, "accessRecipientHex")).expect("access"),
            )
            .expect("access"),
            delivery_deadline: service["deliveryDeadline"].as_u64().expect("deadline"),
            fulfillment_method: text(service, "fulfillmentMethod").to_owned(),
            fulfillment_digest: bytes(text(service, "fulfillmentDigestHex")),
        },
    }
}

fn authorization(
    input: &Value,
    role: Role,
    commitment: erebus_core::commitment::DealCommitment,
) -> Authorization {
    let prefix = match role {
        Role::Buyer => "buyer",
        Role::Seller => "seller",
    };
    let mut signature = Vec::with_capacity(96);
    for suffix in ["R8x", "R8y", "S"] {
        signature.extend_from_slice(&word(&input[format!("{prefix}{suffix}")]));
    }
    Authorization {
        role,
        suite_id: 2,
        commitment,
        signature: SignatureBytes::new(signature).expect("signature"),
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    assert_eq!(
        args.len(),
        7,
        "rpc, pool, deployment block/hash, test key, state root"
    );
    let rpc_url = &args[1];
    let pool: [u8; 20] = bytes(args[2].strip_prefix("0x").expect("pool prefix"));
    let first_block: u64 = args[3].parse().expect("block");
    let first_hash: [u8; 32] = bytes(args[4].strip_prefix("0x").expect("hash prefix"));
    let key = TransactionKey::from_bytes(&bytes(args[5].strip_prefix("0x").expect("key prefix")))
        .expect("test key");
    let root = PathBuf::from(&args[6]);
    let build = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../circuits/m5/build");
    let agreement: Value =
        serde_json::from_slice(&fs::read(build.join("agreement-input.json")).expect("agreement"))
            .expect("agreement JSON");
    let proof: Value =
        serde_json::from_slice(&fs::read(build.join("transfer-input.json")).expect("proof input"))
            .expect("proof JSON");
    let terms = terms(&agreement);
    let transcript: Value = serde_json::from_slice(
        &fs::read(build.join("transcript.json")).expect("retained transcript"),
    )
    .expect("transcript JSON");
    let transcript_store =
        FileTranscriptStore::open(root.join("transcript")).expect("transcript store");
    let transcript_hash_version = u16::try_from(
        transcript["transcriptHashVersion"]
            .as_u64()
            .expect("hash version"),
    )
    .expect("hash version width");
    for encoded in transcript["messagesHex"].as_array().expect("messages") {
        let message = Message::decode(
            &hex::decode(encoded.as_str().expect("message hex")).expect("message bytes"),
        )
        .expect("canonical message");
        transcript_store
            .append(
                "m5-funded",
                terms.deal_id,
                transcript_hash_version,
                &message,
            )
            .expect("persist transcript");
    }
    assert_eq!(terms.domain.pool.as_ref().expect("pool").as_bytes(), pool);
    let blinding = CommitmentBlinding::from_bytes(word(&proof["blinding"]));
    let commitment = commit_agreement(&terms, &blinding).expect("commitment");
    let buyer = authorization(&proof, Role::Buyer, commitment);
    let seller = authorization(&proof, Role::Seller, commitment);
    let context = SettlementContext {
        require_local_proving: true,
        mode: SettlementMode::Shielded,
        domain: terms.domain.clone(),
        suite_id: terms.suite_id,
        asset: terms.asset.clone(),
        required_guarantees: terms.required_guarantees,
    };
    let policy = SpendingPolicy {
        per_deal_max: BaseUnits::new(100),
        allowed_assets: [terms.asset.clone()].into_iter().collect(),
        ..SpendingPolicy::default()
    };
    let open = || {
        Coordinator::open(
            root.join("coordinator"),
            terms.buyer_authorization_key.clone(),
            context.clone(),
            &context,
            &capabilities(),
            policy.clone(),
        )
        .expect("coordinator")
    };
    let coordinator = open();
    let operation_ref = [4; 32];
    coordinator
        .record_intent(operation_ref, &terms, &blinding, 1000)
        .expect("intent");
    coordinator
        .authorize_buyer(operation_ref, 1001, |_, _| Ok::<_, ()>(buyer.clone()))
        .expect("buyer auth");
    coordinator
        .accept_seller(operation_ref, &seller)
        .expect("seller auth");
    let (durable_terms, durable_blinding, durable_buyer, durable_seller) =
        erebus_coordinator::read_disclosure_opening(root.join("coordinator"), operation_ref)
            .expect("durable disclosure opening");
    SelectedAgreement::from_store(
        durable_terms,
        durable_blinding,
        durable_buyer,
        durable_seller,
        transcript_hash_version,
        &transcript_store,
        "m5-funded",
    )
    .expect("funded agreement must reproduce the retained transcript before settlement");
    let rpc = PoolRpc::new(rpc_url, 10143, pool).expect("pool RPC");
    let index = IndexStore::new(
        root.join("index.json"),
        IndexDomain {
            chain_id: 10143,
            pool,
            first_block,
            first_hash,
        },
    )
    .expect("index");
    let asset: [u8; 20] = bytes(
        terms
            .asset
            .asset_reference()
            .strip_prefix("0x")
            .expect("asset prefix"),
    );
    let input = OwnedNote::new(
        asset,
        text(&proof, "inputAmount").parse().expect("input amount"),
        terms
            .buyer_authorization_key
            .as_bytes()
            .try_into()
            .expect("buyer key"),
        word(&proof["inputSpendSecret"]),
        word(&proof["inputSalt"]),
    )
    .expect("input note");
    let wallet_path = root.join("private/wallet.enc");
    let wallet_domain = WalletDomain {
        chain_id: 10143,
        pool,
    };
    let wallet =
        WalletStore::new(wallet_path.clone(), wallet_domain, [7; 32]).expect("test wallet");
    wallet
        .update(|state| state.add(input.clone()))
        .expect("save input opening");
    recover_wallet(&rpc, &index, &wallet, 0)
        .await
        .expect("discover funded input");
    let (public_index, _) = sync_public_index(&rpc, &index, 0)
        .await
        .expect("verified input path");
    let snapshot = wallet.snapshot().expect("wallet snapshot");
    let change = ChangeNote {
        spend_secret: test_field("change-spend"),
        salt: word(&proof["changeSalt"]),
    };
    let manifest: Value =
        serde_json::from_slice(&fs::read(build.join("artifact-manifest.json")).expect("manifest"))
            .expect("manifest JSON");
    let artifact = &manifest["circuits"]["transfer"];
    let artifacts = ProvingArtifacts {
        wasm: build.join("transfer_js/transfer.wasm"),
        r1cs: build.join("transfer.r1cs"),
        zkey: build.join("transfer.zkey"),
        wasm_sha256: text(artifact, "wasmSha256").to_owned(),
        r1cs_sha256: text(artifact, "r1csSha256").to_owned(),
        zkey_sha256: text(artifact, "zkeySha256").to_owned(),
    };
    let request = WalletTransferRequest {
        terms: &terms,
        blinding: &blinding,
        buyer: &buyer,
        seller: &seller,
        wallet: &snapshot,
        index: &public_index,
        change: &change,
        now: 1002,
    };
    // The synchronous Wasmer prover owns a runtime and must run in a blocking context.
    let prepared = tokio::task::block_in_place(|| {
        prepare_coordinated_transfer(
            &coordinator,
            &context,
            &artifacts,
            &wallet,
            &request,
            input.commitment(),
            operation_ref,
            1002,
        )
    })
    .expect("durable wallet reservation and local proof");
    let journal = SignerJournal::open(root.join("signer"), 10143, key.address()).expect("journal");
    let chain = ShieldedChain::connect(context.clone(), rpc_url, Duration::from_secs(10))
        .await
        .expect("chain");
    let raw = chain
        .sign(
            &coordinator,
            &journal,
            operation_ref,
            &key,
            1003,
            Eip1559Fees::new(2_000_000_000, 1_000_000_000).expect("fees"),
            3_000_000,
        )
        .await
        .expect("sign");
    drop(coordinator);
    drop(wallet);
    let restarted = open();
    let wallet =
        WalletStore::new(wallet_path, wallet_domain, [7; 32]).expect("restart test wallet");
    assert!(wallet
        .snapshot()
        .expect("restored note reservation")
        .spendable(&input.commitment())
        .is_none());
    let mut missing_artifacts = artifacts.clone();
    missing_artifacts.wasm_sha256 = "0".repeat(64);
    assert_eq!(
        tokio::task::block_in_place(|| prepare_coordinated_transfer(
            &restarted,
            &context,
            &missing_artifacts,
            &wallet,
            &request,
            input.commitment(),
            operation_ref,
            1002,
        ))
        .expect("restart restores the proof without reproving"),
        prepared
    );
    assert_eq!(
        restarted
            .signed_transaction(operation_ref)
            .expect("stored tx")
            .expect("signed")
            .raw(),
        raw
    );
    let repeated = chain
        .sign(
            &restarted,
            &journal,
            operation_ref,
            &key,
            1003,
            Eip1559Fees::new(2_000_000_000, 1_000_000_000).expect("fees"),
            3_000_000,
        )
        .await
        .expect("restart sign");
    assert_eq!(repeated, raw, "restart changed signed transaction");
    if std::env::var("EREBUS_M6_MATRIX").as_deref() == Ok("1") {
        let matrix_root = root.join("funded-matrix");
        funded_faults::Case {
            root: &matrix_root,
            rpc_url,
            context: &context,
            policy: &policy,
            input: &input,
            request: &request,
            prepared: &prepared,
            key: &key,
            index_domain: index.domain(),
        }
        .run()
        .await
        .expect("combined funded persistence matrix");
    }
    let fault_proxy = FaultProxy::start(rpc_url).await;
    let uncertain_chain = ShieldedChain::connect(
        context.clone(),
        &fault_proxy.url,
        Duration::from_millis(200),
    )
    .await
    .expect("fault chain");
    let broadcast = uncertain_chain
        .broadcast(&restarted, operation_ref, 1004)
        .await
        .expect("broadcast");
    assert_eq!(
        broadcast.outcome,
        erebus_evm::chain::BroadcastOutcome::Unknown
    );
    assert_eq!(
        restarted
            .ledger()
            .expect("unknown accounting")
            .reserved_total(&terms.asset)
            .unwrap(),
        terms.amount
    );
    assert!(wallet
        .snapshot()
        .unwrap()
        .spendable(&input.commitment())
        .is_none());
    let mined: Value = reqwest::Client::new()
        .post(rpc_url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "anvil_mine", "params": ["0x50"]
        }))
        .send()
        .await
        .expect("mine request")
        .json()
        .await
        .expect("mine response");
    assert!(mined.get("error").is_none(), "Anvil mining failed");
    let finalized = rpc.finalized_head().await.expect("pool finalized anchor");
    assert!(finalized.number >= first_block, "pool deployment not final");
    rpc.pool_identity_at(finalized.hash)
        .await
        .expect("pool deployment identity");
    let history_path = root.join("bounded-history-index.json");
    let (_, revisions) = restarted
        .recorded_deal(operation_ref)
        .expect("durable revisions");
    let mut history_pending = 0;
    let mut history_complete = false;
    for _ in 0..100 {
        let history_index =
            IndexStore::new(&history_path, index.domain()).expect("reopened history");
        match erebus_shielded_prover::observation::observe_shielded_deal_bounded(
            &rpc,
            &history_index,
            &context,
            &revisions[0].deal_nullifier(),
            &revisions,
            3,
        )
        .await
        {
            Err(erebus_shielded_prover::observation::ObservationError::Recovery(
                erebus_shielded_prover::recovery::RecoveryError::HistoryPending { .. },
            )) => {
                history_pending += 1;
                assert_eq!(
                    restarted
                        .ledger()
                        .unwrap()
                        .reserved_total(&terms.asset)
                        .unwrap(),
                    terms.amount
                );
                assert!(wallet
                    .snapshot()
                    .unwrap()
                    .spendable(&input.commitment())
                    .is_none());
            }
            Ok(erebus_core::deal_state::DealEvidence::Observed(reads)) => {
                assert!(reads.consumed_at_final && reads.consumed_at_head);
                assert!(reads
                    .winner
                    .is_some_and(|winner| winner.commitment == commitment && winner.is_final));
                history_complete = true;
                break;
            }
            other => panic!("bounded funded history failed: {other:?}"),
        }
    }
    assert!(history_complete && history_pending > 0);
    assert_eq!(fault_proxy.state.sends.load(Ordering::SeqCst), 1);
    println!("M6 funded shielded history completed after {history_pending} pending batches");
    erebus_shielded_prover::recovery::sync_public_index_through(
        &rpc,
        &index,
        rpc.head().await.expect("pool head"),
    )
    .await
    .expect("pool event index");
    let faulty_rpc =
        PoolRpc::with_timeout(&fault_proxy.url, 10143, pool, Duration::from_millis(200))
            .expect("fault pool RPC");
    let peer_index =
        IndexStore::new(root.join("peer-index.json"), index.domain()).expect("peer index");
    for method in ["eth_getBlockByNumber", "eth_call", "eth_getLogs"] {
        *fault_proxy.state.selected.lock().unwrap() = Some(method);
        assert!(
            chain
                .reconcile_agreed(
                    &uncertain_chain,
                    &restarted,
                    &journal,
                    &rpc,
                    &index,
                    &faulty_rpc,
                    &peer_index,
                    &wallet,
                    operation_ref,
                    1005
                )
                .await
                .is_err(),
            "timeout at {method}"
        );
        assert_eq!(
            restarted
                .ledger()
                .unwrap()
                .reserved_total(&terms.asset)
                .unwrap(),
            terms.amount
        );
        assert!(wallet
            .snapshot()
            .unwrap()
            .spendable(&input.commitment())
            .is_none());
    }
    *fault_proxy.state.selected.lock().unwrap() = None;
    let mut last_observation = String::new();
    for _ in 0..30 {
        match chain
            .reconcile_agreed(
                &uncertain_chain,
                &restarted,
                &journal,
                &rpc,
                &index,
                &faulty_rpc,
                &peer_index,
                &wallet,
                operation_ref,
                1005,
            )
            .await
        {
            Ok(assessment) => {
                last_observation = format!("state: {:?}", assessment.state);
                if matches!(assessment.state, erebus_core::deal_state::DealState::PaidFinalized { commitment: found } if found == commitment)
                {
                    let restored = wallet.snapshot().expect("finalized wallet");
                    assert!(restored
                        .spendable_for(&input.commitment(), operation_ref)
                        .is_none());
                    assert_eq!(
                        restored
                            .select(&asset, 80)
                            .expect("finalized change")
                            .amount(),
                        80
                    );
                    assert_eq!(
                        restarted
                            .ledger()
                            .expect("finalized accounting")
                            .reserved_total(&terms.asset)
                            .expect("reserved total"),
                        BaseUnits::new(0)
                    );
                    assert_eq!(
                        fault_proxy.state.sends.load(Ordering::SeqCst),
                        1,
                        "recovery must not resubmit"
                    );
                    println!(
                        "M6 shielded coordinator finalized one payment: 0x{}",
                        hex::encode(broadcast.hash)
                    );
                    funded_disclosure(
                        &root,
                        &terms,
                        operation_ref,
                        rpc_url,
                        &fault_proxy.url,
                        first_block,
                        first_hash,
                    );
                    return;
                }
            }
            Err(error) => last_observation = error.to_string(),
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("shielded transfer did not finalize through the coordinator: {last_observation}");
}
