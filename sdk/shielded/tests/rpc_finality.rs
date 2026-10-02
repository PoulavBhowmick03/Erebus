//! Explicit finality reads for the shielded pool do not fall back to block depth.

use axum::{extract::State, routing::post, Json, Router};
use erebus_core::commitment::DealNullifier;
use erebus_core::deal_state::{DealEvidence, DealReads};
use erebus_core::domain::DeploymentDomain;
use erebus_core::ids::{AddressBytes, AssetId, ChainNamespace};
use erebus_core::settlement::SettlementContext;
use erebus_core::shielded::SHIELDED_GUARANTEES;
use erebus_core::suite::SHIELDED_POSEIDON_EDDSA_SUITE_ID;
use erebus_core::terms::{GuaranteeSet, SettlementMode};
use erebus_shielded_prover::index_store::{IndexDomain, IndexStore};
use erebus_shielded_prover::indexer::PoolIndex;
use erebus_shielded_prover::observation::{
    observe_shielded_deal, observe_shielded_deal_agreed, observe_shielded_deal_bounded,
    ObservationError,
};
use erebus_shielded_prover::recovery::{sync_finalized_public_index, RecoveryError};
use erebus_shielded_prover::rpc::{PoolRpc, RpcError};
use serde_json::{json, Value};
use sha3::{Digest, Keccak256};

#[derive(Clone)]
struct Responses {
    chain_id: &'static str,
    head: &'static str,
    finalized: Value,
    numeric: Value,
    consumed: Value,
    root: Value,
    asset: [u8; 20],
    verifier_version: u32,
}

fn selector(signature: &str) -> String {
    format!(
        "0x{}",
        hex::encode(&Keccak256::digest(signature.as_bytes())[..4])
    )
}

#[test]
fn pool_rpc_rejects_a_zero_request_deadline() {
    assert!(matches!(
        PoolRpc::with_timeout(
            "http://127.0.0.1:8545",
            31_337,
            [3; 20],
            std::time::Duration::ZERO
        ),
        Err(RpcError::Configuration)
    ));
}

async fn reply(State(responses): State<Responses>, Json(request): Json<Value>) -> Json<Value> {
    let result = match request["method"].as_str() {
        Some("eth_chainId") => json!(responses.chain_id),
        Some("eth_blockNumber") => json!(responses.head),
        Some("eth_getBlockByNumber") if request["params"][0] == "finalized" => responses.finalized,
        Some("eth_getBlockByNumber") => responses.numeric,
        Some("eth_getLogs") => json!([]),
        Some("eth_call") => {
            let data = request["params"][0]["data"].as_str().expect("calldata");
            if data != selector("currentRoot()") && data != selector("nextLeafIndex()") {
                assert_eq!(
                    request["params"][1]["blockHash"],
                    format!("0x{}", "11".repeat(32))
                );
                assert_eq!(request["params"][1]["requireCanonical"], true);
            }
            if data == selector("currentRoot()") {
                responses.root
            } else if data == selector("nextLeafIndex()") {
                json!(format!("0x{:064x}", 0))
            } else if data == selector("asset()") {
                json!(format!("0x{:0>64}", hex::encode(responses.asset)))
            } else if data == selector("verifierVersion()") {
                json!(format!("0x{:064x}", responses.verifier_version))
            } else {
                responses.consumed
            }
        }
        _ => panic!("unexpected RPC method"),
    };
    Json(json!({"jsonrpc": "2.0", "id": request["id"], "result": result}))
}

fn block(number: &str, hash_byte: &str) -> Value {
    json!({
        "number": number,
        "hash": format!("0x{}", hash_byte.repeat(32)),
        "parentHash": format!("0x{}", "22".repeat(32)),
        "timestamp": "0x64"
    })
}

async fn start(responses: Responses) -> (PoolRpc, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("address"));
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/", post(reply)).with_state(responses),
        )
        .await
        .expect("serve");
    });
    let rpc = PoolRpc::new(&url, 31_337, [3; 20]).expect("rpc");
    (rpc, server)
}

async fn read(
    responses: Responses,
) -> Result<erebus_shielded_prover::rpc::PoolFinalizedBlock, RpcError> {
    let (rpc, server) = start(responses).await;
    let result = rpc.finalized_head().await;
    server.abort();
    result
}

fn honest() -> Responses {
    Responses {
        chain_id: "0x7a69",
        head: "0x2",
        finalized: block("0x2", "11"),
        numeric: block("0x2", "11"),
        consumed: json!(format!("0x{:064x}", 1)),
        root: json!(format!(
            "0x{}",
            hex::encode(PoolIndex::new(2).expect("index").root())
        )),
        asset: [0xaa; 20],
        verifier_version: 2,
    }
}

#[derive(Clone)]
struct HistoryResponses {
    forked: std::sync::Arc<std::sync::atomic::AtomicBool>,
    logs: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    headers: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    finalized: std::sync::Arc<std::sync::atomic::AtomicU64>,
    root: [u8; 32],
}

fn history_hash(number: u64, forked: bool) -> [u8; 32] {
    let mut hash = [0x11; 32];
    hash[0] = if forked && number >= 4 { 0x33 } else { 0x11 };
    hash[24..].copy_from_slice(&number.to_be_bytes());
    hash
}

async fn history_reply(
    State(state): State<HistoryResponses>,
    Json(request): Json<Value>,
) -> Json<Value> {
    use std::sync::atomic::Ordering;
    let forked = state.forked.load(Ordering::SeqCst);
    let result = match request["method"].as_str() {
        Some("eth_chainId") => json!("0x7a69"),
        Some("eth_blockNumber") => json!("0x14"),
        Some("eth_getBlockByNumber") => {
            state.headers.fetch_add(1, Ordering::SeqCst);
            let tag = request["params"][0].as_str().unwrap();
            let number = if tag == "finalized" {
                state.finalized.load(Ordering::SeqCst)
            } else {
                u64::from_str_radix(tag.strip_prefix("0x").unwrap(), 16).unwrap()
            };
            json!({
                "number": format!("0x{number:x}"),
                "hash": format!("0x{}", hex::encode(history_hash(number, forked))),
                "parentHash": format!("0x{}", hex::encode(history_hash(number.saturating_sub(1), forked))),
                "timestamp": format!("0x{:x}", 100 + number)
            })
        }
        Some("eth_getLogs") => {
            state.logs.fetch_add(1, Ordering::SeqCst);
            json!([])
        }
        Some("eth_call") => {
            let data = request["params"][0]["data"].as_str().unwrap();
            if data == selector("currentRoot()") {
                json!(format!("0x{}", hex::encode(state.root)))
            } else if data == selector("asset()") {
                json!(format!("0x{:0>64}", "aa".repeat(20)))
            } else if data == selector("verifierVersion()") {
                json!(format!("0x{:064x}", 2))
            } else {
                json!(format!("0x{:064x}", 0))
            }
        }
        _ => panic!("unexpected historical RPC method"),
    };
    Json(json!({"jsonrpc": "2.0", "id": request["id"], "result": result}))
}

async fn start_history() -> (PoolRpc, HistoryResponses, tokio::task::JoinHandle<()>) {
    let state = HistoryResponses {
        forked: Default::default(),
        logs: Default::default(),
        headers: Default::default(),
        finalized: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(3)),
        root: PoolIndex::new(2).unwrap().root(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let rpc = PoolRpc::new(
        &format!("http://{}", listener.local_addr().unwrap()),
        31_337,
        [3; 20],
    )
    .unwrap();
    let server_state = state.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/", post(history_reply))
                .with_state(server_state),
        )
        .await
        .unwrap();
    });
    (rpc, state, server)
}

fn history_domain() -> IndexDomain {
    IndexDomain {
        chain_id: 31_337,
        pool: [3; 20],
        first_block: 2,
        first_hash: history_hash(2, false),
    }
}

#[tokio::test]
async fn paired_finalized_wallet_recovery_never_writes_on_disagreement_or_peer_failure() {
    use erebus_shielded_prover::{
        recovery::recover_finalized_wallet_agreed,
        wallet::{WalletDomain, WalletStore},
    };
    use std::sync::atomic::Ordering;
    let dir = tempfile::tempdir().unwrap();
    let first_index = IndexStore::new(dir.path().join("first.json"), history_domain()).unwrap();
    let peer_index = IndexStore::new(dir.path().join("peer.json"), history_domain()).unwrap();
    let wallet_path = dir.path().join("private/wallet.enc");
    let wallet = WalletStore::new(
        &wallet_path,
        WalletDomain {
            chain_id: 31_337,
            pool: [3; 20],
        },
        [7; 32],
    )
    .unwrap();
    wallet.update(|_| Ok(())).unwrap();
    let original = std::fs::read(&wallet_path).unwrap();
    let (first, first_state, first_server) = start_history().await;
    let (peer, peer_state, peer_server) = start_history().await;
    first_state.finalized.store(20, Ordering::SeqCst);
    peer_state.finalized.store(20, Ordering::SeqCst);
    peer_state.forked.store(true, Ordering::SeqCst);
    assert!(
        recover_finalized_wallet_agreed(&first, &first_index, &peer, &peer_index, &wallet)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&wallet_path).unwrap(), original);
    assert!(
        recover_finalized_wallet_agreed(&first, &first_index, &peer, &first_index, &wallet)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&wallet_path).unwrap(), original);
    peer_state.forked.store(false, Ordering::SeqCst);
    let report = recover_finalized_wallet_agreed(&first, &first_index, &peer, &peer_index, &wallet)
        .await
        .unwrap();
    assert_eq!(report.through, 20);
    assert_eq!(report.block_hash, history_hash(20, false));
    let restored = std::fs::read(&wallet_path).unwrap();
    peer_server.abort();
    assert!(
        recover_finalized_wallet_agreed(&first, &first_index, &peer, &peer_index, &wallet)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&wallet_path).unwrap(), restored);
    first_server.abort();
}

#[tokio::test]
async fn shielded_history_yields_and_resumes_before_returning_any_evidence() {
    use std::sync::atomic::Ordering;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("index.json");
    let (rpc, state, server) = start_history().await;
    let nullifier = DealNullifier::from_bytes([5; 32]);
    let mut pending = 0;
    for _ in 0..10 {
        let store = IndexStore::new(&path, history_domain()).unwrap();
        let before = state.logs.load(Ordering::SeqCst);
        let result =
            observe_shielded_deal_bounded(&rpc, &store, &context(), &nullifier, &[], 3).await;
        assert!(state.logs.load(Ordering::SeqCst) - before <= 3);
        match result {
            Err(ObservationError::Recovery(RecoveryError::HistoryPending {
                next_block,
                through,
            })) => {
                pending += 1;
                assert_eq!(through, 20);
                assert_eq!(store.load().unwrap().tip().unwrap().number + 1, next_block);
            }
            Ok(DealEvidence::Observed(reads)) => {
                assert_eq!(pending, 6);
                assert!(!reads.consumed_at_head);
                assert!(!reads.consumed_at_final);
                assert!(reads.winner.is_none());
                assert_eq!(store.load().unwrap().tip().unwrap().number, 20);
                server.abort();
                return;
            }
            other => panic!("unexpected historical result: {other:?}"),
        }
    }
    panic!("bounded scan did not complete");
}

#[tokio::test]
async fn shielded_history_rewinds_a_long_orphaned_suffix_with_bounded_rpc_work() {
    use std::sync::atomic::Ordering;
    let dir = tempfile::tempdir().unwrap();
    let store = IndexStore::new(dir.path().join("index.json"), history_domain()).unwrap();
    let (rpc, state, server) = start_history().await;
    let nullifier = DealNullifier::from_bytes([5; 32]);
    observe_shielded_deal_bounded(&rpc, &store, &context(), &nullifier, &[], 1_000)
        .await
        .unwrap();
    state.forked.store(true, Ordering::SeqCst);
    let headers = state.headers.load(Ordering::SeqCst);
    let logs = state.logs.load(Ordering::SeqCst);
    assert!(matches!(
        observe_shielded_deal_bounded(&rpc, &store, &context(), &nullifier, &[], 3).await,
        Err(ObservationError::Recovery(RecoveryError::HistoryPending {
            next_block: 7,
            through: 20
        }))
    ));
    assert_eq!(state.logs.load(Ordering::SeqCst) - logs, 3);
    assert!(state.headers.load(Ordering::SeqCst) - headers < 20);
    assert_eq!(
        store.load().unwrap().tip().unwrap().hash,
        history_hash(6, true)
    );
    for _ in 0..10 {
        match observe_shielded_deal_bounded(&rpc, &store, &context(), &nullifier, &[], 3).await {
            Err(ObservationError::Recovery(RecoveryError::HistoryPending { .. })) => {}
            Ok(_) => {
                assert_eq!(
                    store.load().unwrap().tip().unwrap().hash,
                    history_hash(20, true)
                );
                server.abort();
                return;
            }
            other => panic!("unexpected historical result: {other:?}"),
        }
    }
    panic!("reorganized scan did not complete");
}

#[tokio::test]
async fn optional_pool_pair_requires_matching_anchors_and_both_successful_reads() {
    let dir = tempfile::tempdir().unwrap();
    let domain = IndexDomain {
        chain_id: 31_337,
        pool: [3; 20],
        first_block: 2,
        first_hash: [0x11; 32],
    };
    let first_index = IndexStore::new(dir.path().join("first/index.json"), domain).unwrap();
    let peer_index = IndexStore::new(dir.path().join("peer/index.json"), domain).unwrap();
    let mut responses = honest();
    responses.consumed = json!(format!("0x{:064x}", 0));
    let (first, first_server) = start(responses.clone()).await;
    let (peer, peer_server) = start(responses.clone()).await;
    let nullifier = DealNullifier::from_bytes([5; 32]);
    assert!(observe_shielded_deal_agreed(
        &first,
        &first_index,
        &peer,
        &peer_index,
        &context(),
        &nullifier,
        &[]
    )
    .await
    .is_ok());
    assert!(matches!(
        observe_shielded_deal_agreed(
            &first,
            &first_index,
            &first,
            &peer_index,
            &context(),
            &nullifier,
            &[]
        )
        .await,
        Err(ObservationError::Context)
    ));
    peer_server.abort();
    assert!(observe_shielded_deal_agreed(
        &first,
        &first_index,
        &peer,
        &peer_index,
        &context(),
        &nullifier,
        &[]
    )
    .await
    .is_err());
    responses.finalized["timestamp"] = json!("0xc8");
    responses.numeric["timestamp"] = json!("0xc8");
    let (different, different_server) = start(responses).await;
    assert!(
        observe_shielded_deal(&different, &peer_index, &context(), &nullifier, &[])
            .await
            .is_ok(),
        "peer is individually consistent"
    );
    assert!(matches!(
        observe_shielded_deal_agreed(
            &first,
            &first_index,
            &different,
            &peer_index,
            &context(),
            &nullifier,
            &[]
        )
        .await,
        Err(ObservationError::Inconsistent)
    ));
    first_server.abort();
    different_server.abort();
}

fn context() -> SettlementContext {
    let namespace = ChainNamespace::new("eip155", "31337").expect("namespace");
    SettlementContext {
        require_local_proving: true,
        mode: SettlementMode::Shielded,
        domain: DeploymentDomain {
            namespace: namespace.clone(),
            settlement_contract: Some(AddressBytes::new(vec![3; 20]).expect("contract")),
            pool: Some(AddressBytes::new(vec![3; 20]).expect("pool")),
            verifier_version: 2,
        },
        suite_id: SHIELDED_POSEIDON_EDDSA_SUITE_ID,
        asset: AssetId::new(namespace, "erc20", &format!("0x{}", "aa".repeat(20))).expect("asset"),
        required_guarantees: GuaranteeSet::from_bits(SHIELDED_GUARANTEES).expect("guarantees"),
    }
}

#[tokio::test]
async fn consumption_read_is_pinned_and_rejects_non_boolean_words() {
    let (rpc, server) = start(honest()).await;
    assert!(rpc
        .consumed_deal_at([5; 32], [0x11; 32])
        .await
        .expect("consumed"));
    server.abort();

    let mut invalid = honest();
    invalid.consumed = json!(format!("0x{:064x}", 2));
    let (rpc, server) = start(invalid).await;
    assert!(matches!(
        rpc.consumed_deal_at([5; 32], [0x11; 32]).await,
        Err(RpcError::Evidence(
            "pool deal consumption is not an ABI bool"
        ))
    ));
    server.abort();
}

#[tokio::test]
async fn finalized_pool_scan_checks_root_and_persists_a_rebuildable_prefix() {
    let dir = tempfile::tempdir().expect("directory");
    let domain = IndexDomain {
        chain_id: 31_337,
        pool: [3; 20],
        first_block: 2,
        first_hash: [0x11; 32],
    };
    let store = IndexStore::new(dir.path().join("pool/index.json"), domain).expect("store");
    let (rpc, server) = start(honest()).await;
    let (index, report) = sync_finalized_public_index(&rpc, &store)
        .await
        .expect("finalized scan");
    assert_eq!(report.through, 2);
    assert_eq!(report.block_hash, domain.first_hash);
    assert_eq!(index.root(), PoolIndex::new(2).expect("index").root());
    assert_eq!(
        store
            .load()
            .expect("cached prefix")
            .tip()
            .expect("tip")
            .number,
        2
    );
    server.abort();

    let mut wrong_root = honest();
    wrong_root.root = json!(format!("0x{}", "ff".repeat(32)));
    let (rpc, server) = start(wrong_root).await;
    assert!(matches!(
        sync_finalized_public_index(&rpc, &store).await,
        Err(RecoveryError::Rpc(RpcError::Evidence(
            "pool state differs from indexed events"
        )))
    ));
    server.abort();
}

#[tokio::test]
async fn observer_requires_consumed_state_to_agree_with_complete_pool_history() {
    let dir = tempfile::tempdir().expect("directory");
    let store = IndexStore::new(
        dir.path().join("pool/index.json"),
        IndexDomain {
            chain_id: 31_337,
            pool: [3; 20],
            first_block: 2,
            first_hash: [0x11; 32],
        },
    )
    .expect("store");
    let nullifier = DealNullifier::from_bytes([5; 32]);
    let mut unpaid = honest();
    unpaid.consumed = json!(format!("0x{:064x}", 0));
    let (rpc, server) = start(unpaid).await;
    assert!(matches!(
        observe_shielded_deal(&rpc, &store, &context(), &nullifier, &[]).await,
        Ok(DealEvidence::Observed(DealReads {
            consumed_at_final: false,
            consumed_at_head: false,
            winner: None,
            final_anchor_timestamp: 100,
            ..
        }))
    ));
    server.abort();

    let (rpc, server) = start(honest()).await;
    assert!(matches!(
        observe_shielded_deal(&rpc, &store, &context(), &nullifier, &[]).await,
        Err(ObservationError::Inconsistent)
    ));
    server.abort();
}

#[tokio::test]
async fn observer_rejects_pool_identity_outside_accepted_context() {
    let dir = tempfile::tempdir().expect("directory");
    let store = IndexStore::new(
        dir.path().join("pool/index.json"),
        IndexDomain {
            chain_id: 31_337,
            pool: [3; 20],
            first_block: 2,
            first_hash: [0x11; 32],
        },
    )
    .expect("store");
    let nullifier = DealNullifier::from_bytes([5; 32]);
    let mut response = honest();
    response.asset = [0xbb; 20];
    let (rpc, server) = start(response).await;
    assert!(matches!(
        observe_shielded_deal(&rpc, &store, &context(), &nullifier, &[]).await,
        Err(ObservationError::Context)
    ));
    server.abort();

    let mut response = honest();
    response.verifier_version = 3;
    let (rpc, server) = start(response).await;
    assert!(matches!(
        observe_shielded_deal(&rpc, &store, &context(), &nullifier, &[]).await,
        Err(ObservationError::Context)
    ));
    server.abort();
}

#[tokio::test]
async fn observer_holds_when_finality_predates_pool_deployment() {
    let dir = tempfile::tempdir().expect("directory");
    let store = IndexStore::new(
        dir.path().join("pool/index.json"),
        IndexDomain {
            chain_id: 31_337,
            pool: [3; 20],
            first_block: 2,
            first_hash: [0x11; 32],
        },
    )
    .expect("store");
    let mut response = honest();
    response.finalized = block("0x1", "11");
    response.numeric = block("0x1", "11");
    let (rpc, server) = start(response).await;
    assert!(matches!(
        observe_shielded_deal(
            &rpc,
            &store,
            &context(),
            &DealNullifier::from_bytes([5; 32]),
            &[]
        )
        .await,
        Err(ObservationError::FinalityPending)
    ));
    server.abort();
}

#[tokio::test]
async fn finalized_anchor_requires_an_explicit_consistent_rpc_response() {
    let anchor = read(honest()).await.expect("finalized anchor");
    assert_eq!(anchor.number, 2);
    assert_eq!(anchor.hash, [0x11; 32]);
    assert_eq!(anchor.timestamp, 100);

    let mut missing = honest();
    missing.finalized = Value::Null;
    assert!(matches!(read(missing).await, Err(RpcError::Request(_))));

    let mut different_hash = honest();
    different_hash.numeric = block("0x2", "33");
    assert!(matches!(
        read(different_hash).await,
        Err(RpcError::Evidence("finalized block is not canonical"))
    ));

    let mut ahead = honest();
    ahead.head = "0x1";
    assert!(matches!(
        read(ahead).await,
        Err(RpcError::Evidence("finalized block is ahead of head"))
    ));

    let mut wrong_chain = honest();
    wrong_chain.chain_id = "0x1";
    assert!(matches!(
        read(wrong_chain).await,
        Err(RpcError::Evidence("wrong chain ID"))
    ));
}
