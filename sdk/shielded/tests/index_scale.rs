//! Cold-history index-cache scale baseline (investigation-only, not part of any candidate).
//!
//! Measures synthetic 10k/100k/600k-block caches: build, save (full rewrite), load/restart,
//! serialized bytes, peak RSS, and the 128 MiB cap failure. A bounded mock-RPC phase
//! separates RPC/scan time from the full-index rewrite cost.
//!
//! Run with:
//!   CARGO_TARGET_DIR=<target> cargo test --release --locked --manifest-path sdk/shielded/Cargo.toml \
//!     --test index_scale -- --ignored --nocapture --test-threads=1

use std::{
    io::Write,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use axum::{extract::State, routing::post, Json, Router};
use erebus_shielded_prover::index_store::{IndexDomain, IndexStore};
use erebus_shielded_prover::indexer::{PoolBlock, PoolIndex};
use erebus_shielded_prover::recovery::recover_finalized_wallet_agreed_bounded;
use erebus_shielded_prover::rpc::PoolRpc;
use erebus_shielded_prover::wallet::{WalletDomain, WalletStore};
use serde_json::{json, Value};
use sha3::{Digest, Keccak256};

fn block_hash(number: u64) -> [u8; 32] {
    let mut hash = [0x11; 32];
    hash[24..].copy_from_slice(&number.to_be_bytes());
    hash
}

fn domain() -> IndexDomain {
    IndexDomain {
        chain_id: 31_337,
        pool: [3; 20],
        first_block: 2,
        first_hash: block_hash(2),
    }
}

fn build_index(blocks: u64) -> PoolIndex {
    let mut index = PoolIndex::new(2).unwrap();
    for number in 2..2 + blocks {
        index
            .apply_block(PoolBlock {
                number,
                hash: block_hash(number),
                parent_hash: block_hash(number - 1),
                events: Vec::new(),
            })
            .unwrap();
    }
    index
}

fn rss_bytes() -> u64 {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u64>()
        .unwrap_or(0)
        * 1024
}

struct RssSampler {
    stop: Arc<AtomicBool>,
    peak: Arc<AtomicU64>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl RssSampler {
    fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let peak = Arc::new(AtomicU64::new(rss_bytes()));
        let handle = std::thread::spawn({
            let stop = stop.clone();
            let peak = peak.clone();
            move || {
                while !stop.load(Ordering::SeqCst) {
                    peak.fetch_max(rss_bytes(), Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        });
        Self {
            stop,
            peak,
            handle: Some(handle),
        }
    }

    fn finish(mut self) -> u64 {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.handle.take().unwrap().join();
        self.peak.load(Ordering::SeqCst)
    }
}

fn measure(blocks: u64) -> Value {
    let root = tempfile::tempdir().unwrap();
    let store = IndexStore::new(root.path().join("index.json"), domain()).unwrap();

    let sampler = RssSampler::start();
    let started = Instant::now();
    let mut index = build_index(blocks);
    let build = started.elapsed();

    let loaded_tip = index.tip().map(|block| block.hash);
    let started = Instant::now();
    let saved = store.save_if_unchanged(&index, None);
    let save = started.elapsed();
    let bytes = std::fs::metadata(root.path().join("index.json"))
        .map(|meta| meta.len())
        .unwrap_or(0);

    let started = Instant::now();
    let loaded = store.load();
    let load = started.elapsed();
    let restart = {
        let store = IndexStore::new(root.path().join("index.json"), domain()).unwrap();
        let started = Instant::now();
        let result = store.load();
        (started.elapsed(), result.is_ok())
    };
    let save_load_peak = sampler.finish();
    // Append-only save: only the tail segment and the manifest may be rewritten.
    let sampler = RssSampler::start();
    for number in 2 + blocks..2 + blocks + 10 {
        index
            .apply_block(PoolBlock {
                number,
                hash: block_hash(number),
                parent_hash: block_hash(number - 1),
                events: Vec::new(),
            })
            .unwrap();
    }
    let started = Instant::now();
    let appended = store.save_if_unchanged(&index, loaded_tip);
    let append_save = started.elapsed();
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(root.path().join("index.json")).unwrap())
            .unwrap_or(Value::Null);
    let segment_count = manifest["segments"].as_array().map(Vec::len).unwrap_or(0);
    let total_bytes = std::fs::metadata(root.path().join("index.json"))
        .map(|meta| meta.len())
        .unwrap_or(0)
        + manifest["segments"]
            .as_array()
            .map(|segments| {
                segments
                    .iter()
                    .map(|segment| segment["bytes"].as_u64().unwrap_or(0))
                    .sum::<u64>()
            })
            .unwrap_or(0);
    let max_segment_bytes = manifest["segments"]
        .as_array()
        .map(|segments| {
            segments
                .iter()
                .map(|segment| segment["bytes"].as_u64().unwrap_or(0))
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0);
    let append_peak = sampler.finish();

    json!({
        "append_save_ms": append_save.as_millis(),
        "append_save_ok": appended.is_ok(),
        "segments": segment_count,
        "max_segment_bytes": max_segment_bytes,
        "total_bytes": total_bytes,
        "blocks": blocks,
        "build_ms": build.as_millis(),
        "save_ms": save.as_millis(),
        "save_ok": saved.is_ok(),
        "save_error": saved.err().map(|error| error.to_string()),
        "bytes": bytes,
        "bytes_per_block": bytes.checked_div(blocks).unwrap_or(0),
        "load_ms": load.as_millis(),
        "load_ok": loaded.is_ok(),
        "restart_ms": restart.0.as_millis(),
        "restart_ok": restart.1,
        "save_load_peak_rss_bytes": save_load_peak,
        "append_peak_rss_bytes": append_peak,
    })
}

#[derive(Clone)]
struct HistoryResponses {
    finalized: Arc<AtomicU64>,
    logs: Arc<AtomicU64>,
    headers: Arc<AtomicU64>,
}

fn history_hash(number: u64) -> [u8; 32] {
    block_hash(number)
}

async fn history_reply(
    State(state): State<HistoryResponses>,
    Json(request): Json<Value>,
) -> Json<Value> {
    let result = match request["method"].as_str() {
        Some("eth_chainId") => json!("0x7a69"),
        Some("eth_blockNumber") => json!(format!("0x{:x}", state.finalized.load(Ordering::SeqCst))),
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
                "hash": format!("0x{}", hex::encode(history_hash(number))),
                "parentHash": format!("0x{}", hex::encode(history_hash(number.saturating_sub(1)))),
                "timestamp": format!("0x{:x}", 100 + number)
            })
        }
        Some("eth_getLogs") => {
            state.logs.fetch_add(1, Ordering::SeqCst);
            json!([])
        }
        Some("eth_call") => {
            let data = request["params"][0]["data"].as_str().unwrap();
            let selector = |signature: &str| {
                format!(
                    "0x{}",
                    hex::encode(&Keccak256::digest(signature.as_bytes())[..4])
                )
            };
            if data == selector("currentRoot()") {
                json!(format!(
                    "0x{}",
                    hex::encode(PoolIndex::new(2).unwrap().root())
                ))
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

/// One bounded paired scan over a mock history: separates RPC/scan time from rewrite cost.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "cold-history scale baseline; synthetic and CPU heavy"]
async fn index_scale_baseline() {
    for blocks in [10_000u64, 100_000, 600_000] {
        println!("{}", measure(blocks));
    }

    // The 128 MiB cap: save fails above the boundary, and loading an oversized file fails closed.
    let root = tempfile::tempdir().unwrap();
    let store = IndexStore::new(root.path().join("oversized.json"), domain()).unwrap();
    let mut file = std::fs::File::create(root.path().join("oversized.json")).unwrap();
    let chunk = vec![b' '; 8 * 1024 * 1024];
    for _ in 0..17 {
        file.write_all(&chunk).unwrap();
    }
    drop(file);
    let oversized = match store.load() {
        Ok(_) => "unexpectedly loaded".to_owned(),
        Err(error) => error.to_string(),
    };
    println!(
        "{}",
        json!({
            "oversized_load_error": oversized,
            "oversized_bytes": std::fs::metadata(root.path().join("oversized.json")).unwrap().len(),
        })
    );

    if std::env::var_os("INDEX_SCALE_SKIP_MOCK").is_some() {
        return;
    }
    // Bounded paired scan over a mock 10k history with an empty cache: total time vs the
    // rewrite share measured above.
    let spawn_history = || async {
        let state = HistoryResponses {
            finalized: Arc::new(AtomicU64::new(10_001)),
            logs: Arc::new(AtomicU64::new(0)),
            headers: Arc::new(AtomicU64::new(0)),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn({
            let state = state.clone();
            async move {
                axum::serve(
                    listener,
                    Router::new()
                        .route("/", post(history_reply))
                        .with_state(state),
                )
                .await
                .unwrap();
            }
        });
        (PoolRpc::new(&url, 31_337, [3; 20]).unwrap(), state, server)
    };
    let (primary, primary_state, server) = spawn_history().await;
    let (peer, peer_state, peer_server) = spawn_history().await;
    let cache = tempfile::tempdir().unwrap();
    let wallet = WalletStore::new(
        cache.path().join("private/wallet.enc"),
        WalletDomain {
            chain_id: 31_337,
            pool: [3; 20],
            verifier_version: 2,
        },
        [7; 32],
    )
    .unwrap();
    wallet.update(|_| Ok(())).unwrap();
    let started = Instant::now();
    let mut scans = 0u64;
    loop {
        scans += 1;
        match recover_finalized_wallet_agreed_bounded(
            &primary,
            &IndexStore::new(cache.path().join("primary.json"), domain()).unwrap(),
            &peer,
            &IndexStore::new(cache.path().join("peer.json"), domain()).unwrap(),
            &wallet,
            1_000,
        )
        .await
        {
            Ok(_) => break,
            Err(erebus_shielded_prover::recovery::RecoveryError::HistoryPending { .. }) => {}
            Err(error) => panic!("mock scan failed: {error}"),
        }
        if scans > 40 {
            panic!("mock scan did not complete");
        }
    }
    let scan_total = started.elapsed();
    server.abort();
    peer_server.abort();
    println!(
        "{}",
        json!({
            "mock_scan_blocks": 10_000,
            "mock_scan_calls": scans,
            "mock_scan_total_ms": scan_total.as_millis(),
            "mock_log_reads": primary_state.logs.load(Ordering::SeqCst) + peer_state.logs.load(Ordering::SeqCst),
            "mock_header_reads": primary_state.headers.load(Ordering::SeqCst) + peer_state.headers.load(Ordering::SeqCst),
        })
    );
}
