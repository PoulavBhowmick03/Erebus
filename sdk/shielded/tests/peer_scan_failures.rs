//! Peer scan failures during paired cold-history sync.
//!
//! A rate-limited or failing peer must surface a distinct error once the bounded primary
//! prefix completes, must retain both checkpoints, must never mutate the wallet and must
//! heal after the peer recovers. Deterministic JSON-RPC mocks with failure knobs.

use axum::{extract::State, routing::post, Json, Router};
use erebus_shielded_prover::index_store::{IndexDomain, IndexStore};
use erebus_shielded_prover::indexer::PoolIndex;
use erebus_shielded_prover::recovery::{recover_finalized_wallet_agreed_bounded, RecoveryError};
use erebus_shielded_prover::rpc::PoolRpc;
use erebus_shielded_prover::wallet::{WalletDomain, WalletStore};
use serde_json::{json, Value};
use sha3::{Digest, Keccak256};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc,
};

fn selector(signature: &str) -> String {
    format!(
        "0x{}",
        hex::encode(&Keccak256::digest(signature.as_bytes())[..4])
    )
}

#[derive(Clone)]
struct HistoryResponses {
    forked: Arc<AtomicBool>,
    fail_logs: Arc<AtomicUsize>,
    logs: Arc<AtomicUsize>,
    headers: Arc<AtomicUsize>,
    finalized: Arc<AtomicU64>,
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
    let forked = state.forked.load(Ordering::SeqCst);
    if request["method"] == "eth_getLogs" {
        let remaining = state.fail_logs.load(Ordering::SeqCst);
        if remaining > 0 {
            state.fail_logs.store(remaining - 1, Ordering::SeqCst);
            return Json(json!({"jsonrpc": "2.0", "id": request["id"],
                "error": {"code": -32005, "message": "rate limit exceeded"}}));
        }
        state.logs.fetch_add(1, Ordering::SeqCst);
    }
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
        Some("eth_getLogs") => json!([]),
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

struct Harness {
    rpc: PoolRpc,
    state: HistoryResponses,
    _server: tokio::task::JoinHandle<()>,
}

async fn start_history() -> Harness {
    let state = HistoryResponses {
        forked: Default::default(),
        fail_logs: Default::default(),
        logs: Default::default(),
        headers: Default::default(),
        finalized: Arc::new(AtomicU64::new(3)),
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
    Harness {
        rpc,
        state,
        _server: server,
    }
}

fn history_domain() -> IndexDomain {
    IndexDomain {
        chain_id: 31_337,
        pool: [3; 20],
        first_block: 2,
        first_hash: history_hash(2, false),
    }
}

struct Stores {
    root: tempfile::TempDir,
    wallet_path: std::path::PathBuf,
    wallet: WalletStore,
}

fn stores() -> Stores {
    let root = tempfile::tempdir().unwrap();
    let wallet_path = root.path().join("private/wallet.enc");
    let wallet = WalletStore::new(
        &wallet_path,
        WalletDomain {
            chain_id: 31_337,
            pool: [3; 20],
            verifier_version: 2,
        },
        [7; 32],
    )
    .unwrap();
    wallet.update(|_| Ok(())).unwrap();
    Stores {
        root,
        wallet_path,
        wallet,
    }
}

impl Stores {
    fn index(&self, name: &str) -> IndexStore {
        IndexStore::new(self.root.path().join(name), history_domain()).unwrap()
    }

    fn tip(&self, index: &IndexStore) -> u64 {
        index
            .load()
            .unwrap()
            .tip()
            .map(|tip| tip.number)
            .unwrap_or(0)
    }

    fn wallet_bytes(&self) -> Vec<u8> {
        std::fs::read(&self.wallet_path).unwrap()
    }
}

/// Primary bounded scan stays pending while the peer is rate-limited. The pending result
/// masks the peer failure for that call only; the rate-limit error must surface once the
/// primary prefix is complete, must never write the wallet, and must heal after the peer
/// recovers. Index stores are reopened every iteration to model repeated restarts.
#[tokio::test]
async fn primary_pending_masks_peer_rate_limit_then_surfaces_across_restarts() {
    let stores = stores();
    let primary = start_history().await;
    let peer = start_history().await;
    primary.state.finalized.store(20, Ordering::SeqCst);
    peer.state.finalized.store(20, Ordering::SeqCst);
    let before = stores.wallet_bytes();

    peer.state.fail_logs.store(100_000, Ordering::SeqCst);
    let mut masked = 0usize;
    let mut surfaced = None;
    let mut primary_tips = Vec::new();
    let mut peer_tips = Vec::new();
    for _ in 0..80 {
        let primary_index = stores.index("primary.json");
        let peer_index = stores.index("peer.json");
        match recover_finalized_wallet_agreed_bounded(
            &primary.rpc,
            &primary_index,
            &peer.rpc,
            &peer_index,
            &stores.wallet,
            3,
        )
        .await
        {
            Ok(_) => panic!("peer reads are failing; recovery cannot complete"),
            Err(RecoveryError::HistoryPending { .. }) => masked += 1,
            Err(error) => {
                surfaced = Some(error);
                break;
            }
        }
        primary_tips.push(stores.tip(&primary_index));
        peer_tips.push(stores.tip(&peer_index));
        assert_eq!(
            stores.wallet_bytes(),
            before,
            "a failing catch-up wrote the wallet"
        );
    }
    assert!(masked >= 1, "expected at least one masked pending cycle");
    let surfaced = surfaced.expect("the peer failure must surface");
    assert!(
        matches!(surfaced, RecoveryError::Rpc(_)),
        "peer rate limiting must classify as an RPC error: {surfaced:?}"
    );
    // PoolRpc redacts provider messages; the method-level RPC error is the distinct,
    // non-absence classification. The rate limiter was actually exercised (counter drained).
    assert!(
        surfaced.to_string().contains("eth_getLogs"),
        "the failing read must stay visible by method: {surfaced}"
    );
    assert!(
        peer.state.fail_logs.load(Ordering::SeqCst) < 100_000,
        "the mock rate limiter must have refused reads"
    );
    assert!(
        peer_tips.iter().all(|tip| *tip == 0),
        "the failing peer checkpoint must be retained: {peer_tips:?}"
    );
    assert!(
        primary_tips.windows(2).all(|pair| pair[1] >= pair[0]),
        "the primary checkpoint rewound across restarts: {primary_tips:?}"
    );
    assert!(primary_tips.iter().any(|tip| *tip > 0), "{primary_tips:?}");

    peer.state.fail_logs.store(0, Ordering::SeqCst);
    let primary_index = stores.index("primary.json");
    let peer_index = stores.index("peer.json");
    let report = recover_finalized_wallet_agreed_bounded(
        &primary.rpc,
        &primary_index,
        &peer.rpc,
        &peer_index,
        &stores.wallet,
        1_000,
    )
    .await
    .unwrap_or_else(|error| panic!("healed catch-up must complete: {error}"));
    assert_eq!(report.through, 20);
    assert_eq!(report.block_hash, history_hash(20, false));
    assert!(stores.tip(&primary_index) >= 20);
    assert!(stores.tip(&peer_index) >= 20);
}

/// A peer whose finalized history conflicts at the shared anchor is rejected before any
/// bounded scan runs: the error surfaces immediately and nothing is written.
#[tokio::test]
async fn peer_conflicting_anchor_surfaces_before_any_masking() {
    let stores = stores();
    let primary = start_history().await;
    let peer = start_history().await;
    primary.state.finalized.store(20, Ordering::SeqCst);
    peer.state.finalized.store(20, Ordering::SeqCst);
    peer.state.forked.store(true, Ordering::SeqCst);
    let before = stores.wallet_bytes();
    let primary_index = stores.index("primary.json");
    let peer_index = stores.index("peer.json");
    let error = recover_finalized_wallet_agreed_bounded(
        &primary.rpc,
        &primary_index,
        &peer.rpc,
        &peer_index,
        &stores.wallet,
        3,
    )
    .await
    .expect_err("a conflicting shared anchor must fail");
    assert!(
        matches!(error, RecoveryError::Rpc(_)),
        "conflicting anchor must classify as an RPC evidence error: {error:?}"
    );
    assert_eq!(stores.wallet_bytes(), before);
    assert_eq!(stores.tip(&primary_index), 0);
    assert_eq!(stores.tip(&peer_index), 0);

    peer.state.forked.store(false, Ordering::SeqCst);
    let report = recover_finalized_wallet_agreed_bounded(
        &primary.rpc,
        &primary_index,
        &peer.rpc,
        &peer_index,
        &stores.wallet,
        1_000,
    )
    .await
    .unwrap_or_else(|error| panic!("healed catch-up must complete: {error}"));
    assert_eq!(report.through, 20);
}

/// A finalized anchor that regresses below an already-advanced checkpoint must fail closed
/// immediately, never be reported as pending, and never write the wallet.
#[tokio::test]
async fn finalized_anchor_regression_is_never_masked_or_written() {
    let stores = stores();
    let primary = start_history().await;
    let peer = start_history().await;
    primary.state.finalized.store(20, Ordering::SeqCst);
    peer.state.finalized.store(20, Ordering::SeqCst);
    let primary_index = stores.index("primary.json");
    let peer_index = stores.index("peer.json");
    let before = stores.wallet_bytes();
    let report = recover_finalized_wallet_agreed_bounded(
        &primary.rpc,
        &primary_index,
        &peer.rpc,
        &peer_index,
        &stores.wallet,
        1_000,
    )
    .await
    .unwrap();
    assert_eq!(report.through, 20);
    let advanced = stores.wallet_bytes();
    assert_ne!(
        advanced, before,
        "a complete paired scan updates the wallet exactly once"
    );

    // The peer regresses to a finalized height below both advanced checkpoints.
    peer.state.finalized.store(3, Ordering::SeqCst);
    let error = recover_finalized_wallet_agreed_bounded(
        &primary.rpc,
        &primary_index,
        &peer.rpc,
        &peer_index,
        &stores.wallet,
        1_000,
    )
    .await
    .expect_err("a lagging finalized anchor must not regress a completed checkpoint");
    assert!(
        matches!(error, RecoveryError::Rpc(_)),
        "anchor regression must classify as an RPC evidence error, not pending: {error:?}"
    );
    assert!(
        !matches!(error, RecoveryError::HistoryPending { .. }),
        "a regression must never be reported as pending progress"
    );
    assert_eq!(
        stores.wallet_bytes(),
        advanced,
        "a rejected regression must not rewrite the wallet"
    );
    assert!(stores.tip(&primary_index) >= 20);
    assert!(stores.tip(&peer_index) >= 20);
}
