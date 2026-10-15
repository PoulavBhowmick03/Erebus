//! Concurrent pool-history reads against a scripted JSON-RPC provider.
//!
//! Concurrency may only change how fast blocks arrive. These tests pin that a windowed,
//! concurrent scan builds the same index as a one-at-a-time scan, applies blocks in height
//! order, and commits nothing when any read in a window fails, times out, or disagrees.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use erebus_core::shielded::{note_tree_parent, NOTE_TREE_DEPTH};
use erebus_shielded_prover::index_store::{IndexDomain, IndexStore};
use erebus_shielded_prover::indexer::{IndexError, PoolIndex};
use erebus_shielded_prover::recovery::{
    sync_public_index_step, sync_public_index_through, RecoveryError,
};
use erebus_shielded_prover::rpc::{PoolRpc, RpcError, MAX_LOG_BLOCK_RANGE};
use serde_json::{json, Value};
use sha3::{Digest, Keccak256};

const CHAIN_ID: u64 = 31_337;
const POOL: [u8; 20] = [3; 20];
const BASE: u64 = 1_000;

fn hex32(bytes: &[u8; 32]) -> String {
    format!("0x{}", hex::encode(bytes))
}

fn topic(signature: &str) -> [u8; 32] {
    Keccak256::digest(signature.as_bytes()).into()
}

fn selector(signature: &str) -> String {
    format!(
        "0x{}",
        hex::encode(&Keccak256::digest(signature.as_bytes())[..4])
    )
}

/// A canonical BN254 field element derived from a label; the top byte stays zero.
fn field(kind: u8, fork: u8, number: u64, item: u8) -> [u8; 32] {
    let mut value = [0u8; 32];
    value[1] = kind;
    value[2] = fork;
    value[16..24].copy_from_slice(&number.to_be_bytes());
    value[31] = item.wrapping_add(1);
    value
}

/// Independent incremental Poseidon tree used only to script the provider's emitted roots.
struct Tree {
    zero: [[u8; 32]; NOTE_TREE_DEPTH],
    filled: [[u8; 32]; NOTE_TREE_DEPTH],
    next: u32,
    root: [u8; 32],
}

impl Tree {
    fn new() -> Self {
        let mut zero = [[0u8; 32]; NOTE_TREE_DEPTH];
        let mut node = [0u8; 32];
        for item in &mut zero {
            *item = node;
            node = note_tree_parent(&node, &node).expect("zero hash");
        }
        Self {
            zero,
            filled: zero,
            next: 0,
            root: node,
        }
    }

    fn insert(&mut self, leaf: [u8; 32]) -> [u8; 32] {
        let mut node = leaf;
        let mut position = self.next;
        for level in 0..NOTE_TREE_DEPTH {
            node = if position & 1 == 0 {
                self.filled[level] = node;
                note_tree_parent(&node, &self.zero[level])
            } else {
                note_tree_parent(&self.filled[level], &node)
            }
            .expect("tree parent");
            position >>= 1;
        }
        self.next += 1;
        self.root = node;
        node
    }
}

#[derive(Clone)]
struct MockBlock {
    hash: [u8; 32],
    parent: [u8; 32],
    logs: Vec<Value>,
    root: [u8; 32],
    leaves: u32,
}

/// `len` blocks from `BASE`. Heights at or above `fork_at` belong to fork 1, below it to
/// fork 0, so two chains with different `fork_at` share exactly the prefix below it.
fn chain(len: u64, fork_at: u64) -> Vec<MockBlock> {
    let inserted = topic("NoteInserted(uint256,uint256,uint256)");
    let transferred = topic("DealTransferred(uint256,uint256,uint256)");
    let withdrawn = topic("NoteWithdrawn(uint256,address,uint256)");
    let mut tree = Tree::new();
    let mut blocks: Vec<MockBlock> = Vec::new();
    let mut parent = [0x99; 32];
    for number in BASE..BASE + len {
        let fork = u8::from(number >= fork_at);
        let mut hash = [0u8; 32];
        hash[0] = 0xaa;
        hash[1] = fork;
        hash[24..].copy_from_slice(&number.to_be_bytes());
        let mut logs = Vec::new();
        let log = |logs: &mut Vec<Value>, topics: Vec<String>, data: String| {
            let index = logs.len();
            let mut tx = [0u8; 32];
            tx[0] = 0x7f;
            tx[1] = fork;
            tx[24..].copy_from_slice(&number.to_be_bytes());
            logs.push(json!({
                "address": format!("0x{}", hex::encode(POOL)),
                "blockNumber": format!("0x{number:x}"),
                "blockHash": hex32(&hash),
                "transactionHash": hex32(&tx),
                "logIndex": format!("0x{index:x}"),
                "topics": topics,
                "data": data,
                "removed": false,
            }));
        };
        let insertions = match number % 6 {
            0 => 2,
            3 => 1,
            _ => 0,
        };
        for item in 0..insertions {
            let commitment = field(1, fork, number, item);
            let index = tree.next;
            let root = tree.insert(commitment);
            log(
                &mut logs,
                vec![
                    hex32(&inserted),
                    format!("0x{index:064x}"),
                    hex32(&commitment),
                ],
                hex32(&root),
            );
        }
        if number % 7 == 0 {
            log(
                &mut logs,
                vec![
                    hex32(&transferred),
                    hex32(&field(2, fork, number, 0)),
                    hex32(&field(3, fork, number, 0)),
                    hex32(&field(4, fork, number, 0)),
                ],
                "0x".to_owned(),
            );
        }
        if number % 11 == 0 {
            log(
                &mut logs,
                vec![
                    hex32(&withdrawn),
                    hex32(&field(5, fork, number, 0)),
                    format!("0x{:0>64}", "ab".repeat(20)),
                ],
                format!("0x{:064x}", 5),
            );
        }
        blocks.push(MockBlock {
            hash,
            parent,
            logs,
            root: tree.root,
            leaves: tree.next,
        });
        parent = hash;
    }
    blocks
}

#[derive(Clone, Default)]
enum Fault {
    #[default]
    None,
    /// HTTP 500 for one header height.
    FailHeader(u64),
    /// Holds one header response past the client's deadline.
    StallHeader(u64),
    /// Replaces the chain right after an `eth_getLogs` response is built.
    ReorgAfterLogs(Vec<MockBlock>),
    /// Replaces the chain after this many header responses.
    ReorgAfterHeaders(usize, Vec<MockBlock>),
    /// Adds a copy of the window's first log labelled one height past the window.
    LogOutsideRange,
    /// Relabels one log with the hash of a different block in the window.
    LogWrongHash,
    /// Serves a window's logs in reverse order.
    LogsReversed,
    /// The endpoint changes chain identity after answering the log request.
    ChainSwitchAfterLogs,
    /// HTTP 429 for the next `remaining` reads of one header height, with an optional
    /// `Retry-After` header in seconds.
    RateLimited {
        height: u64,
        remaining: usize,
        retry_after: Option<&'static str>,
    },
}

#[derive(Default)]
struct Mock {
    blocks: Vec<MockBlock>,
    fault: Fault,
    jitter: bool,
    counts: HashMap<String, usize>,
    headers_served: usize,
    in_flight: usize,
    max_in_flight: usize,
    log_ranges: Vec<(u64, u64)>,
    started: Vec<Instant>,
    chain_switched: bool,
    /// Extra latency for every header read.
    slow: Option<Duration>,
}

type Shared = Arc<Mutex<Mock>>;

fn quantity(value: &Value) -> u64 {
    u64::from_str_radix(
        value
            .as_str()
            .and_then(|raw| raw.strip_prefix("0x"))
            .expect("hex quantity"),
        16,
    )
    .expect("quantity")
}

fn block_at(mock: &Mock, number: u64) -> Option<MockBlock> {
    number
        .checked_sub(BASE)
        .and_then(|offset| mock.blocks.get(offset as usize))
        .cloned()
}

async fn reply(State(shared): State<Shared>, Json(request): Json<Value>) -> Response {
    let method = request["method"].as_str().expect("method").to_owned();
    let params = request["params"].clone();
    {
        let mut mock = shared.lock().expect("mock");
        let throttled = match &mut mock.fault {
            Fault::RateLimited {
                height,
                remaining,
                retry_after,
            } if method == "eth_getBlockByNumber"
                && quantity(&params[0]) == *height
                && *remaining > 0 =>
            {
                *remaining -= 1;
                Some(*retry_after)
            }
            _ => None,
        };
        if let Some(retry_after) = throttled {
            *mock.counts.entry("throttled".to_owned()).or_default() += 1;
            return match retry_after {
                Some(seconds) => (
                    StatusCode::TOO_MANY_REQUESTS,
                    [(axum::http::header::RETRY_AFTER, seconds)],
                )
                    .into_response(),
                None => StatusCode::TOO_MANY_REQUESTS.into_response(),
            };
        }
    }
    let delay = {
        let mut mock = shared.lock().expect("mock");
        *mock.counts.entry(method.clone()).or_default() += 1;
        mock.started.push(Instant::now());
        mock.in_flight += 1;
        mock.max_in_flight = mock.max_in_flight.max(mock.in_flight);
        let mut delay = Duration::ZERO;
        if method == "eth_getBlockByNumber" {
            let number = quantity(&params[0]);
            if mock.jitter {
                // Deterministic scatter so later heights often finish first.
                delay = Duration::from_millis((number * 7_919) % 23);
            }
            if let Some(slow) = mock.slow {
                delay += slow;
            }
            if matches!(mock.fault, Fault::StallHeader(stalled) if stalled == number) {
                delay = Duration::from_secs(3);
            }
        }
        delay
    };
    tokio::time::sleep(delay).await;
    let result = respond(&shared, &method, &params);
    shared.lock().expect("mock").in_flight -= 1;
    match result {
        Ok(result) => {
            Json(json!({"jsonrpc":"2.0","id":request["id"],"result":result})).into_response()
        }
        Err(status) => status.into_response(),
    }
}

fn respond(shared: &Shared, method: &str, params: &Value) -> Result<Value, StatusCode> {
    let mut mock = shared.lock().expect("mock");
    match method {
        "eth_chainId" => Ok(json!(format!(
            "0x{:x}",
            CHAIN_ID + u64::from(mock.chain_switched)
        ))),
        "eth_blockNumber" => Ok(json!(format!(
            "0x{:x}",
            BASE + mock.blocks.len() as u64 - 1
        ))),
        "eth_getBlockByNumber" => {
            let number = quantity(&params[0]);
            if matches!(mock.fault, Fault::FailHeader(failed) if failed == number) {
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
            let block = block_at(&mock, number);
            mock.headers_served += 1;
            if let Fault::ReorgAfterHeaders(after, replacement) = &mock.fault {
                if mock.headers_served == *after {
                    mock.blocks = replacement.clone();
                    mock.fault = Fault::None;
                }
            }
            Ok(block.map_or(Value::Null, |block| {
                json!({
                    "number": format!("0x{number:x}"),
                    "hash": hex32(&block.hash),
                    "parentHash": hex32(&block.parent),
                    "timestamp": format!("0x{:x}", number * 2),
                })
            }))
        }
        "eth_getLogs" => {
            let filter = &params[0];
            assert_eq!(filter["address"], format!("0x{}", hex::encode(POOL)));
            let from = quantity(&filter["fromBlock"]);
            let to = quantity(&filter["toBlock"]);
            mock.log_ranges.push((from, to));
            if to < from || to - from >= 100 {
                // The public Monad RPC rejects wider ranges.
                return Ok(Value::Null);
            }
            let mut logs: Vec<Value> = (from..=to)
                .filter_map(|number| block_at(&mock, number))
                .flat_map(|block| block.logs)
                .collect();
            match std::mem::take(&mut mock.fault) {
                Fault::ReorgAfterLogs(replacement) => mock.blocks = replacement,
                Fault::LogOutsideRange => {
                    let mut extra = logs.first().cloned().expect("window has a log");
                    extra["blockNumber"] = json!(format!("0x{:x}", to + 1));
                    logs.push(extra);
                }
                Fault::LogWrongHash => {
                    let first = logs.first_mut().expect("window has a log");
                    let number = quantity(&first["blockNumber"]);
                    let other = block_at(&mock, number + 1).expect("next block");
                    first["blockHash"] = json!(hex32(&other.hash));
                }
                Fault::LogsReversed => logs.reverse(),
                Fault::ChainSwitchAfterLogs => mock.chain_switched = true,
                other => mock.fault = other,
            }
            Ok(json!(logs))
        }
        "eth_call" => {
            let data = params[0]["data"].as_str().expect("calldata");
            let block = block_at(&mock, quantity(&params[1])).expect("state block");
            if data == selector("currentRoot()") {
                Ok(json!(hex32(&block.root)))
            } else if data == selector("nextLeafIndex()") {
                Ok(json!(format!("0x{:064x}", block.leaves)))
            } else {
                Err(StatusCode::BAD_REQUEST)
            }
        }
        _ => Err(StatusCode::BAD_REQUEST),
    }
}

async fn serve(blocks: Vec<MockBlock>) -> (SocketAddr, Shared) {
    let shared = Arc::new(Mutex::new(Mock {
        blocks,
        ..Mock::default()
    }));
    let app = Router::new()
        .route("/", post(reply))
        .with_state(Arc::clone(&shared));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move { axum::serve(listener, app).await });
    (address, shared)
}

/// A source with the pacer effectively off, so functional tests do not wait on it.
fn rpc(address: SocketAddr, reads: u64) -> PoolRpc {
    PoolRpc::new(&format!("http://{address}"), CHAIN_ID, POOL)
        .expect("rpc")
        .with_max_concurrent_reads(reads)
        .expect("concurrency")
        .with_max_requests_per_second(1_000_000)
        .expect("rate")
}

async fn full_sync(rpc: &PoolRpc, through: u64, limit: u64) -> PoolIndex {
    let mut index = PoolIndex::new(BASE).expect("index");
    while index.tip().is_none_or(|tip| tip.number < through) {
        assert!(rpc.sync(&mut index, through, limit).await.expect("sync") > 0);
    }
    index
}

fn serialized(index: &PoolIndex) -> Vec<u8> {
    let dir = tempfile::tempdir().expect("temporary directory");
    let path = dir.path().join("index.json");
    let store = IndexStore::new(
        &path,
        IndexDomain {
            chain_id: CHAIN_ID,
            pool: POOL,
            first_block: BASE,
            first_hash: index.blocks()[0].hash,
        },
    )
    .expect("store");
    store.save_if_unchanged(index, None).expect("save");
    std::fs::read(path).expect("read")
}

fn assert_same(left: &PoolIndex, right: &PoolIndex) {
    assert_eq!(left.blocks(), right.blocks());
    assert_eq!(left.root(), right.root());
    assert_eq!(left.leaves(), right.leaves());
    assert_eq!(serialized(left), serialized(right));
}

fn unchanged(index: &PoolIndex, before: &PoolIndex) -> bool {
    index.blocks() == before.blocks() && index.root() == before.root()
}

#[tokio::test]
async fn concurrent_sync_matches_sequential_sync_byte_for_byte() {
    let blocks = chain(347, u64::MAX);
    let through = BASE + 346;
    let (address, shared) = serve(blocks.clone()).await;
    shared.lock().expect("mock").jitter = true;
    let sequential = full_sync(&rpc(address, 1), through, 1_000).await;
    assert_eq!(shared.lock().expect("mock").max_in_flight, 1);
    shared.lock().expect("mock").max_in_flight = 0;
    let concurrent = full_sync(&rpc(address, 8), through, 1_000).await;
    let max_in_flight = shared.lock().expect("mock").max_in_flight;
    assert!(
        (2..=8).contains(&max_in_flight),
        "in flight {max_in_flight}"
    );
    assert!(sequential.leaves().len() > 100);
    assert!(sequential
        .blocks()
        .iter()
        .any(|block| block.events.len() >= 3));
    assert_same(&sequential, &concurrent);
    // Small batches cross window boundaries at different heights and must agree too.
    let batched = full_sync(&rpc(address, 32), through, 37).await;
    assert_same(&sequential, &batched);
    // Every window stays within the public 100-block cap, contiguous and non-overlapping.
    let ranges = shared.lock().expect("mock").log_ranges.clone();
    assert!(ranges
        .iter()
        .all(|(from, to)| from <= to && to - from < MAX_LOG_BLOCK_RANGE));
    // The recovery helper reaches the same index through its durable store.
    let dir = tempfile::tempdir().expect("temporary directory");
    let store = IndexStore::new(
        dir.path().join("index.json"),
        IndexDomain {
            chain_id: CHAIN_ID,
            pool: POOL,
            first_block: BASE,
            first_hash: blocks[0].hash,
        },
    )
    .expect("store");
    let (recovered, report) = sync_public_index_through(&rpc(address, 8), &store, through)
        .await
        .expect("recovery sync");
    assert_eq!(report.through, through);
    assert_same(&sequential, &recovered);
}

#[tokio::test]
async fn out_of_order_responses_still_apply_in_height_order() {
    let blocks = chain(120, u64::MAX);
    let (address, shared) = serve(blocks.clone()).await;
    shared.lock().expect("mock").jitter = true;
    let index = full_sync(&rpc(address, 16), BASE + 119, 1_000).await;
    let numbers: Vec<u64> = index.blocks().iter().map(|block| block.number).collect();
    assert_eq!(numbers, (BASE..BASE + 120).collect::<Vec<_>>());
    for (block, expected) in index.blocks().iter().zip(&blocks) {
        assert_eq!(block.hash, expected.hash);
        assert_eq!(block.parent_hash, expected.parent);
        assert_eq!(block.events.len(), expected.logs.len());
    }
}

#[tokio::test]
async fn a_failed_read_in_the_middle_of_a_window_commits_nothing() {
    let (address, shared) = serve(chain(250, u64::MAX)).await;
    let rpc = rpc(address, 8);
    let mut index = PoolIndex::new(BASE).expect("index");
    assert_eq!(
        rpc.sync(&mut index, BASE + 249, 60).await.expect("first"),
        60
    );
    let before = index.clone();
    shared.lock().expect("mock").fault = Fault::FailHeader(BASE + 120);
    assert!(matches!(
        rpc.sync(&mut index, BASE + 249, 1_000).await,
        Err(RpcError::Request("eth_getBlockByNumber"))
    ));
    assert!(unchanged(&index, &before));
    shared.lock().expect("mock").fault = Fault::None;
    assert_eq!(
        rpc.sync(&mut index, BASE + 249, 1_000)
            .await
            .expect("retry"),
        190
    );
}

#[tokio::test]
async fn a_chain_switch_after_logs_does_not_commit_history() {
    let (address, shared) = serve(chain(3, u64::MAX)).await;
    let source = rpc(address, 8);
    let mut index = PoolIndex::new(BASE).unwrap();
    let before = index.clone();
    shared.lock().unwrap().fault = Fault::ChainSwitchAfterLogs;
    assert!(matches!(
        source.sync(&mut index, BASE + 2, 3).await,
        Err(RpcError::Evidence("wrong chain ID"))
    ));
    assert!(unchanged(&index, &before));
    shared.lock().unwrap().chain_switched = false;
    assert_eq!(source.sync(&mut index, BASE + 2, 3).await.unwrap(), 3);
}

#[tokio::test]
async fn a_timed_out_read_commits_nothing() {
    let (address, shared) = serve(chain(150, u64::MAX)).await;
    let rpc = PoolRpc::with_timeout(
        &format!("http://{address}"),
        CHAIN_ID,
        POOL,
        Duration::from_millis(300),
    )
    .expect("rpc")
    .with_max_requests_per_second(1_000_000)
    .expect("rate");
    let mut index = PoolIndex::new(BASE).expect("index");
    shared.lock().expect("mock").fault = Fault::StallHeader(BASE + 104);
    assert!(matches!(
        rpc.sync(&mut index, BASE + 149, 1_000).await,
        Err(RpcError::Request("eth_getBlockByNumber"))
    ));
    assert!(index.tip().is_none());
}

#[tokio::test]
async fn a_reorg_between_the_two_header_reads_fails_closed() {
    let original = chain(150, u64::MAX);
    let replacement = chain(150, BASE + 40);
    let (address, shared) = serve(original).await;
    let rpc = rpc(address, 8);
    let mut index = PoolIndex::new(BASE).expect("index");
    shared.lock().expect("mock").fault = Fault::ReorgAfterLogs(replacement.clone());
    assert!(matches!(
        rpc.sync(&mut index, BASE + 149, 1_000).await,
        Err(RpcError::Evidence("block changed while logs were read"))
    ));
    assert!(index.tip().is_none());
    // The next sync reads only the new branch.
    let recovered = full_sync(&rpc, BASE + 149, 1_000).await;
    let (fresh, _) = serve(replacement).await;
    assert_same(
        &recovered,
        &full_sync(&self::rpc(fresh, 1), BASE + 149, 1_000).await,
    );
}

#[tokio::test]
async fn a_reorg_inside_a_window_aborts_and_the_next_sync_follows_the_new_branch() {
    let original = chain(200, u64::MAX);
    let replacement = chain(200, BASE + 130);
    let (address, shared) = serve(original).await;
    shared.lock().expect("mock").jitter = true;
    let rpc = rpc(address, 8);
    let mut index = PoolIndex::new(BASE).expect("index");
    // Commit the first window, then swap branches partway through the second.
    assert_eq!(
        rpc.sync(&mut index, BASE + 199, 100).await.expect("first"),
        100
    );
    let before = index.clone();
    let served = shared.lock().expect("mock").headers_served;
    shared.lock().expect("mock").fault = Fault::ReorgAfterHeaders(served + 50, replacement.clone());
    let error = rpc
        .sync(&mut index, BASE + 199, 1_000)
        .await
        .expect_err("mixed branches");
    assert!(
        matches!(
            error,
            RpcError::Index(IndexError::BlockLink)
                | RpcError::Evidence(
                    "log block or pool mismatch" | "block changed while logs were read"
                )
        ),
        "{error:?}"
    );
    assert!(unchanged(&index, &before));
    assert_eq!(
        rpc.sync(&mut index, BASE + 199, 1_000).await.expect("next"),
        100
    );
    let (fresh, _) = serve(replacement).await;
    assert_same(
        &index,
        &full_sync(&self::rpc(fresh, 1), BASE + 199, 1_000).await,
    );
}

#[tokio::test]
async fn a_reorg_below_the_committed_tip_rewinds_on_the_next_sync() {
    let original = chain(260, u64::MAX);
    let replacement = chain(260, BASE + 150);
    let (address, shared) = serve(original).await;
    let rpc = rpc(address, 8);
    let mut index = PoolIndex::new(BASE).expect("index");
    assert_eq!(
        rpc.sync(&mut index, BASE + 259, 180).await.expect("first"),
        180
    );
    shared.lock().expect("mock").blocks = replacement.clone();
    // The tip changed: the prefix search finds height 150, rewinds, and re-reads from there.
    assert_eq!(
        rpc.sync(&mut index, BASE + 259, 1_000)
            .await
            .expect("rewind"),
        110
    );
    let (fresh, _) = serve(replacement).await;
    assert_same(
        &index,
        &full_sync(&self::rpc(fresh, 1), BASE + 259, 1_000).await,
    );
}

#[tokio::test]
async fn logs_for_the_wrong_block_or_order_are_rejected() {
    for (fault, expected) in [
        (Fault::LogOutsideRange, "log block or pool mismatch"),
        (Fault::LogWrongHash, "log block or pool mismatch"),
        (Fault::LogsReversed, "logs out of block order"),
    ] {
        let (address, shared) = serve(chain(150, u64::MAX)).await;
        let rpc = rpc(address, 8);
        let mut index = PoolIndex::new(BASE).expect("index");
        shared.lock().expect("mock").fault = fault;
        let error = rpc
            .sync(&mut index, BASE + 149, 1_000)
            .await
            .expect_err("bad logs");
        assert!(
            matches!(error, RpcError::Evidence(message) if message == expected),
            "{error:?}"
        );
        assert!(index.tip().is_none());
    }
}

#[tokio::test]
async fn a_scan_needs_about_two_requests_per_block() {
    let (address, shared) = serve(chain(1_000, u64::MAX)).await;
    let rpc = rpc(address, 8);
    let mut index = PoolIndex::new(BASE).expect("index");
    assert_eq!(
        rpc.sync(&mut index, BASE + 999, 1_000).await.expect("sync"),
        1_000
    );
    let counts = shared.lock().expect("mock").counts.clone();
    let total: usize = counts.values().sum();
    // Before: eth_chainId, header, one-block eth_getLogs, and header again for every block.
    assert!(total * 100 <= 1_000 * 205, "{counts:?}");
    // Check identity before reading and again before committing staged history.
    assert_eq!(counts["eth_chainId"], 2);
    assert_eq!(counts["eth_getLogs"], 10);
    assert_eq!(counts["eth_getBlockByNumber"], 2 * 1_000 + 1);
    // `read_block` stays a complete single-block check for its other callers.
    let block = rpc.read_block(BASE + 42).await.expect("one block");
    assert_eq!(&block, &index.blocks()[42]);
}

#[tokio::test]
async fn the_default_pacer_stays_under_the_public_rate_limit() {
    let (address, shared) = serve(chain(120, u64::MAX)).await;
    let rpc = PoolRpc::new(&format!("http://{address}"), CHAIN_ID, POOL)
        .expect("rpc")
        .with_max_concurrent_reads(32)
        .expect("concurrency");
    let started = Instant::now();
    let mut index = PoolIndex::new(BASE).expect("index");
    assert_eq!(
        rpc.sync(&mut index, BASE + 119, 1_000).await.expect("sync"),
        120
    );
    let elapsed = started.elapsed();
    let mut times = shared.lock().expect("mock").started.clone();
    times.sort();
    // 40 requests per second after a burst of 8: never 50 within any one-second span.
    let busiest = (0..times.len())
        .map(|first| {
            times[first..]
                .iter()
                .take_while(|time| time.duration_since(times[first]) < Duration::from_secs(1))
                .count()
        })
        .max()
        .expect("requests");
    assert!(busiest < 50, "busiest second had {busiest} requests");
    assert!(elapsed >= Duration::from_millis(((times.len() - 8) * 25) as u64 - 50));
    assert!(matches!(
        PoolRpc::new("http://127.0.0.1:1", CHAIN_ID, POOL)
            .expect("rpc")
            .with_max_concurrent_reads(33),
        Err(RpcError::Configuration)
    ));
    assert!(matches!(
        PoolRpc::new("http://127.0.0.1:1", CHAIN_ID, POOL)
            .expect("rpc")
            .with_max_concurrent_reads(0),
        Err(RpcError::Configuration)
    ));
}

fn store_at(path: &std::path::Path, first_hash: [u8; 32]) -> IndexStore {
    IndexStore::new(
        path,
        IndexDomain {
            chain_id: CHAIN_ID,
            pool: POOL,
            first_block: BASE,
            first_hash,
        },
    )
    .expect("store")
}

#[tokio::test]
async fn a_throttled_read_backs_off_and_then_succeeds() {
    let blocks = chain(40, u64::MAX);
    let (address, shared) = serve(blocks).await;
    // Four 429s in a row exhausted the old three retries; Retry-After 0 keeps the test fast.
    shared.lock().expect("mock").fault = Fault::RateLimited {
        height: BASE + 17,
        remaining: 4,
        retry_after: Some("0"),
    };
    let index = full_sync(&rpc(address, 8), BASE + 39, 1_000).await;
    assert_eq!(index.tip().expect("tip").number, BASE + 39);
    assert_eq!(shared.lock().expect("mock").counts["throttled"], 4);
    // Without Retry-After the client waits seconds, not milliseconds, before retrying.
    shared.lock().expect("mock").fault = Fault::RateLimited {
        height: BASE + 5,
        remaining: 1,
        retry_after: None,
    };
    let started = Instant::now();
    let rpc = rpc(address, 8);
    rpc.read_block(BASE + 5).await.expect("read after backoff");
    assert!(started.elapsed() >= Duration::from_millis(500));
}

#[tokio::test]
async fn a_persistently_throttled_read_fails_closed_without_committing() {
    let blocks = chain(40, u64::MAX);
    let (address, shared) = serve(blocks).await;
    shared.lock().expect("mock").fault = Fault::RateLimited {
        height: BASE + 17,
        remaining: usize::MAX,
        retry_after: Some("0"),
    };
    let mut index = PoolIndex::new(BASE).expect("index");
    let before = index.clone();
    assert!(matches!(
        rpc(address, 8).sync(&mut index, BASE + 39, 1_000).await,
        Err(RpcError::RateLimited("eth_getBlockByNumber"))
    ));
    assert!(unchanged(&index, &before));
    // One first attempt and four retries, then the error; never an unbounded loop.
    assert_eq!(shared.lock().expect("mock").counts["throttled"], 5);
}

#[tokio::test]
async fn a_concurrently_advanced_shared_cache_is_progress_not_failure() {
    let blocks = chain(300, u64::MAX);
    let (address, shared) = serve(blocks.clone()).await;
    let dir = tempfile::tempdir().expect("temporary directory");
    let path = dir.path().join("index.json");
    let theirs = store_at(&path, blocks[0].hash);
    // Commit a 100-block prefix, then build another process's further verified progress.
    match sync_public_index_step(
        &rpc(address, 8),
        &store_at(&path, blocks[0].hash),
        BASE + 299,
        100,
    )
    .await
    {
        Err(RecoveryError::HistoryPending { next_block, .. }) => assert_eq!(next_block, BASE + 100),
        other => panic!(
            "expected progress, got {:?}",
            other.map(|(_, report)| report.through)
        ),
    }
    let prefix = theirs.load().expect("prefix");
    let mut ahead = prefix.clone();
    rpc(address, 8)
        .sync(&mut ahead, BASE + 249, 1_000)
        .await
        .expect("other process scan");
    // This step loads the prefix, then scans slowly while the other process saves.
    shared.lock().expect("mock").slow = Some(Duration::from_millis(30));
    let step = tokio::spawn({
        let ours = store_at(&path, blocks[0].hash);
        let rpc = rpc(address, 8);
        async move { sync_public_index_step(&rpc, &ours, BASE + 299, 100).await }
    });
    tokio::time::sleep(Duration::from_millis(60)).await;
    theirs
        .save_if_unchanged(&ahead, prefix.tip().map(|tip| tip.hash))
        .expect("other process saves first");
    match step.await.expect("step task") {
        Err(RecoveryError::HistoryPending {
            next_block,
            through,
        }) => {
            assert_eq!((next_block, through), (BASE + 100, BASE + 299));
        }
        other => panic!(
            "a lost save race must be pending, got {:?}",
            other.map(|(_, report)| report.through)
        ),
    }
    // The other process's progress stands; the next step continues from it.
    assert_eq!(
        theirs.load().expect("load").tip().expect("tip").number,
        BASE + 249
    );
    shared.lock().expect("mock").slow = None;
    let ours = store_at(&path, blocks[0].hash);
    let report = loop {
        match sync_public_index_step(&rpc(address, 8), &ours, BASE + 299, 100).await {
            Ok((_, report)) => break report,
            Err(RecoveryError::HistoryPending { .. }) => continue,
            Err(other) => panic!("shared cache step failed: {other}"),
        }
    };
    assert_eq!(report.through, BASE + 299);
    assert_same(
        &ours.load().expect("final"),
        &full_sync(&rpc(address, 8), BASE + 299, 1_000).await,
    );
}

#[tokio::test]
async fn seeding_copies_only_a_further_verified_cache_of_the_same_history() {
    let blocks = chain(150, u64::MAX);
    let (address, _shared) = serve(blocks.clone()).await;
    let dir = tempfile::tempdir().expect("temporary directory");
    let wallet = store_at(&dir.path().join("wallet.json"), blocks[0].hash);
    let observation = store_at(&dir.path().join("observation.json"), blocks[0].hash);
    let full = full_sync(&rpc(address, 8), BASE + 149, 1_000).await;
    wallet.save_if_unchanged(&full, None).expect("wallet save");
    assert!(observation.seed_from(&wallet).expect("seed"));
    assert_same(&observation.load().expect("seeded"), &full);
    // Equal coverage: nothing to copy.
    assert!(!observation.seed_from(&wallet).expect("no-op"));
    // A cache that is ahead is never replaced by one that is behind.
    let behind = store_at(&dir.path().join("behind.json"), blocks[0].hash);
    let prefix = full_sync(&rpc(address, 8), BASE + 99, 1_000).await;
    behind
        .save_if_unchanged(&prefix, None)
        .expect("behind save");
    assert!(!observation.seed_from(&behind).expect("ahead stays"));
    assert_eq!(
        observation.load().expect("load").tip().expect("tip").number,
        BASE + 149
    );
    // A different deployment anchor is refused, not merged.
    let other = store_at(&dir.path().join("other.json"), [9; 32]);
    assert!(matches!(
        other.seed_from(&wallet),
        Err(erebus_shielded_prover::index_store::IndexStoreError::Domain)
    ));
}
