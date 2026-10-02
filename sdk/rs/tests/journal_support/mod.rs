//! Shared helpers for the journal characterization tests (Metropolis M6, slice 1).
//!
//! Both `journal_characterization.rs` and `reconcile_characterization.rs` load the historical
//! fixtures under `tests/fixtures/journal/` and compare against the goldens stored beside
//! them. Not every helper is used by both test crates.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use erebus_sdk::journal::{OperationJournal, OperationRecord};
use erebus_sdk::operation::{OperationId, RequestBinding, WriteOperation};
use serde_json::{json, Value};
use starknet_types_core::felt::Felt;

/// Pool-scoped binding prefix every fixture was written with.
pub const CHAIN: Felt = Felt::from_hex_unchecked("0x534e5f5345504f4c4941");
pub const POOL: Felt = Felt::from_hex_unchecked("0x4e4f");
pub const TOKEN: Felt = Felt::from_hex_unchecked("0x53545f");
/// Account whose nonce reconciliation reads. Fake.
pub const ACCOUNT: Felt = Felt::from_hex_unchecked("0xacc");
/// Channel handle carried by the `propose_offer` fixtures. Fake.
pub const HANDLE: &str = "ch_c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4";

/// Set this to rewrite the goldens from the current code instead of comparing against them.
pub const BLESS_ENV: &str = "EREBUS_BLESS_JOURNAL_GOLDENS";

/// One directory of records written by one historical commit.
#[derive(Debug, Clone, Copy)]
pub struct FixtureSet {
    /// Directory under `tests/fixtures/journal/`.
    pub dir: &'static str,
    /// Commit whose journal code wrote the records.
    pub commit: &'static str,
    /// `version` field every record in the set carries.
    pub version: u32,
    /// First id byte of every record in the set.
    pub era: u8,
}

pub const V1_B784F3F: FixtureSet = FixtureSet {
    dir: "v1-b784f3f",
    commit: "b784f3f",
    version: 1,
    era: 0x1a,
};
pub const V1_7B7B4C8: FixtureSet = FixtureSet {
    dir: "v1-7b7b4c8",
    commit: "7b7b4c8",
    version: 1,
    era: 0x1b,
};
pub const V1_448146E: FixtureSet = FixtureSet {
    dir: "v1-448146e",
    commit: "448146e",
    version: 1,
    era: 0x1c,
};
pub const V2_B1FE0E3: FixtureSet = FixtureSet {
    dir: "v2-b1fe0e3",
    commit: "b1fe0e3",
    version: 2,
    era: 0x20,
};
pub const V3_D3DAB59: FixtureSet = FixtureSet {
    dir: "v3-d3dab59",
    commit: "d3dab59",
    version: 3,
    era: 0x30,
};
pub const V4_93593ED: FixtureSet = FixtureSet {
    dir: "v4-93593ed",
    commit: "93593ed",
    version: 4,
    era: 0x40,
};

pub const SETS: [FixtureSet; 6] = [
    V1_B784F3F, V1_7B7B4C8, V1_448146E, V2_B1FE0E3, V3_D3DAB59, V4_93593ED,
];
pub const LEGACY_SETS: [FixtureSet; 5] =
    [V1_B784F3F, V1_7B7B4C8, V1_448146E, V2_B1FE0E3, V3_D3DAB59];

/// Record codes (second id byte). See `tests/fixtures/journal/README.md`.
pub const CLAIMED: u8 = 0x01;
pub const PREPARED: u8 = 0x02;
pub const PROVEN: u8 = 0x03;
pub const SIGNED: u8 = 0x04;
pub const SUBMITTED: u8 = 0x05;
pub const ACCEPTED: u8 = 0x06;
pub const COMMITTED: u8 = 0x07;
pub const REVERTED: u8 = 0x08;
pub const NEEDS_ATTENTION: u8 = 0x09;
pub const RESTARTED_IN_FLIGHT: u8 = 0x0a;
pub const RESTARTED_FRESH: u8 = 0x0b;
pub const NOOP_COMMITTED: u8 = 0x0c;
pub const CLAIMED_WITHOUT_REQUEST: u8 = 0x0d;
pub const PREPARED_WITHOUT_REQUEST: u8 = 0x0e;

impl FixtureSet {
    /// Every record code present in this set.
    pub fn codes(&self) -> Vec<u8> {
        let last = if self.version >= 2 {
            PREPARED_WITHOUT_REQUEST
        } else {
            RESTARTED_FRESH
        };
        (CLAIMED..=last).collect()
    }

    pub fn id(&self, code: u8) -> OperationId {
        fixture_id(self.era, code)
    }

    pub fn tx(&self, code: u8, attempt: u8) -> Felt {
        Felt::from_hex(&format!("0x{:02x}{code:02x}{attempt:02x}beef", self.era))
            .expect("fixture hash is a felt")
    }

    pub fn path(&self) -> PathBuf {
        fixtures_root().join(self.dir)
    }

    pub fn record_bytes(&self, code: u8) -> Vec<u8> {
        std::fs::read(self.path().join(format!("{}.json", self.id(code).as_str())))
            .expect("fixture record")
    }
}

pub fn fixture_id(era: u8, code: u8) -> OperationId {
    OperationId::parse(format!("op_{era:02x}{code:02x}{}", "00".repeat(30))).expect("fixture id")
}

/// The write each record code was claimed as.
pub fn op_for(code: u8) -> WriteOperation {
    match code {
        ACCEPTED => WriteOperation::ProposeOffer,
        NOOP_COMMITTED => WriteOperation::OpenChannel,
        _ => WriteOperation::Shield,
    }
}

/// The binding each fixture was claimed with. Unchanged since schema v1.
pub fn binding(code: u8) -> RequestBinding {
    RequestBinding::builder(op_for(code), CHAIN, POOL, TOKEN)
        .u128_be(1_000 + u128::from(code))
        .finish()
}

/// The canonical request the v2+ fixtures were claimed with.
pub fn request(code: u8) -> Value {
    match op_for(code) {
        WriteOperation::ProposeOffer => json!({"method":"propose_offer","handle":HANDLE,
            "terms":{"amount":"1006","token":"0x53545f","deadline":1790000000,"memo_hash":"0x0"}}),
        WriteOperation::OpenChannel => {
            json!({"method":"open_channel","counterparty":"0x55","wire_version":"v3"})
        }
        _ => json!({"method":"shield","amount":(1_000 + u128::from(code)).to_string(),
                     "wire_version":"v3"}),
    }
}

pub fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/journal")
}

/// A fresh identity state directory under the system temp dir.
pub fn temporary_root(label: &str) -> PathBuf {
    // A counter, not a timestamp: tests run in parallel (see tests/journal.rs).
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    std::env::temp_dir().join(format!(
        "erebus-journal-char-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// A state directory holding copies of the given fixture sets, laid out as the journal
/// expects: `<root>/operations/<id>.json` and `<id>.<n>.tx`, directory `0700`, files `0600`.
pub struct Installed {
    pub root: PathBuf,
    pub journal: OperationJournal,
}

impl Installed {
    pub fn operations(&self) -> PathBuf {
        self.root.join("operations")
    }

    pub fn record_path(&self, id: &OperationId) -> PathBuf {
        self.operations().join(format!("{}.json", id.as_str()))
    }

    /// Records sorted by operation id, so anything derived from them is deterministic.
    pub fn sorted_records(&self) -> Vec<OperationRecord> {
        let mut records = self.journal.records().expect("fixtures load");
        records.sort_by(|a, b| a.operation_id.as_str().cmp(b.operation_id.as_str()));
        records
    }
}

impl Drop for Installed {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub fn install(label: &str, sets: &[FixtureSet]) -> Installed {
    let root = temporary_root(label);
    let journal = OperationJournal::new(&root).expect("journal opens");
    let operations = root.join("operations");
    for set in sets {
        for entry in std::fs::read_dir(set.path()).expect("fixture dir") {
            let source = entry.expect("fixture entry").path();
            let name = source.file_name().expect("file name").to_owned();
            let destination = operations.join(name);
            std::fs::copy(&source, &destination).expect("copy fixture");
            set_mode(&destination, 0o600);
        }
    }
    Installed { root, journal }
}

/// Like [`install`], with only the records (and their stored transactions) for `codes`.
pub fn install_codes(label: &str, set: FixtureSet, codes: &[u8]) -> Installed {
    let installed = install(label, &[set]);
    let keep: Vec<String> = codes
        .iter()
        .map(|code| set.id(*code).as_str().to_owned())
        .collect();
    for entry in std::fs::read_dir(installed.operations()).expect("operations") {
        let path = entry.expect("entry").path();
        let name = path.file_name().and_then(|n| n.to_str()).expect("name");
        if !keep.iter().any(|id| name.starts_with(id.as_str())) {
            std::fs::remove_file(&path).expect("remove");
        }
    }
    installed
}

#[cfg(unix)]
pub fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
}

#[cfg(not(unix))]
pub fn set_mode(_path: &Path, _mode: u32) {}

#[cfg(unix)]
pub fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).expect("stat").permissions().mode() & 0o777
}

/// Inode of a file: an atomic rename replaces it, so an unchanged inode proves no write.
#[cfg(unix)]
pub fn inode(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).expect("stat").ino()
}

// ---- goldens ---------------------------------------------------------------------------

pub fn golden_path(name: &str) -> PathBuf {
    fixtures_root().join("golden").join(name)
}

/// Compares `actual` to a golden file byte for byte, or rewrites it under [`BLESS_ENV`].
pub fn assert_golden_bytes(name: &str, actual: &[u8]) {
    let path = golden_path(name);
    if std::env::var_os(BLESS_ENV).is_some() {
        std::fs::create_dir_all(path.parent().expect("golden dir")).expect("golden dir");
        std::fs::write(&path, actual).expect("write golden");
        return;
    }
    let expected = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("golden {} is missing: {error}", path.display()));
    if expected != actual {
        panic!(
            "golden {name} differs from current behaviour.\n--- expected\n{}\n--- actual\n{}\n\
             If the change is intended, review it and rerun with {BLESS_ENV}=1.",
            String::from_utf8_lossy(&expected),
            String::from_utf8_lossy(actual)
        );
    }
}

/// Pretty JSON with sorted keys and a trailing newline. Sorted keys match what crosses the
/// CLI seam, which re-encodes every result through `serde_json::Value`.
pub fn assert_golden_json(name: &str, actual: &Value) {
    let text = serde_json::to_string_pretty(actual).expect("encode golden") + "\n";
    assert_golden_bytes(name, text.as_bytes());
}

// ---- mock Starknet node ----------------------------------------------------------------

/// Answers one JSON-RPC call: `Ok(result)` or `Err((code, message))`.
pub type Route = dyn Fn(&str, &Value) -> Result<Value, (i64, String)> + Send + Sync;

/// A local JSON-RPC node that answers by method and parameters rather than from a fixed
/// script, and records every call it served, in order.
///
/// The existing tests (`tests/reconcile.rs`, `tests/fault_matrix.rs`) replay canned bodies
/// in order. Goldens over many records need answers that do not depend on the order the
/// records were read in, so this routes instead. The HTTP handling is the same: one
/// request per connection, `Connection: close`.
pub struct MockNode {
    pub url: String,
    address: SocketAddr,
    calls: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl MockNode {
    pub fn start(route: Box<Route>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let calls: Arc<Mutex<Vec<Value>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (recorder, stopping) = (Arc::clone(&calls), Arc::clone(&stop));
        let handle = thread::spawn(move || loop {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            if stopping.load(Ordering::SeqCst) {
                return;
            }
            let Some(body) = read_request(&mut stream) else {
                continue;
            };
            let request: Value = serde_json::from_str(&body).expect("JSON-RPC request");
            let method = request["method"].as_str().unwrap_or_default().to_owned();
            let params = request["params"].clone();
            recorder
                .lock()
                .expect("recorder")
                .push(json!([method, params]));
            let response = match route(&method, &params) {
                Ok(result) => json!({"jsonrpc":"2.0","id":request["id"],"result":result}),
                Err((code, message)) => json!({"jsonrpc":"2.0","id":request["id"],
                    "error":{"code":code,"message":message}}),
            }
            .to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            );
        });
        Self {
            url: format!("http://{address}"),
            address,
            calls,
            stop,
            handle: Some(handle),
        }
    }

    /// Every call served so far, as `[method, params]`.
    pub fn calls(&self) -> Vec<Value> {
        self.calls.lock().expect("recorder").clone()
    }

    pub fn methods(&self) -> Vec<String> {
        self.calls()
            .iter()
            .map(|call| call[0].as_str().unwrap_or_default().to_owned())
            .collect()
    }
}

impl Drop for MockNode {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.address);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Option<String> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let read = stream.read(&mut buffer).ok()?;
        if read == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..read]);
        let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
        let length: usize = headers
            .lines()
            .find_map(|line| line.strip_prefix("content-length:").map(str::trim))?
            .parse()
            .ok()?;
        let start = end + 4;
        while bytes.len() < start + length {
            let read = stream.read(&mut buffer).ok()?;
            if read == 0 {
                return None;
            }
            bytes.extend_from_slice(&buffer[..read]);
        }
        return Some(String::from_utf8_lossy(&bytes[start..start + length]).into_owned());
    }
}

/// Live account nonce the mock chain reports. Fixtures signed at nonce 5 are therefore
/// provably dead; attempts signed at nonce 6 may still land.
pub const LIVE_NONCE: &str = "0x6";

/// Block timestamps the mock chain serves: `1_700_000_000 + n`. Deliberately different from
/// the `1_787_000_000 + n` the v3/v4 fixtures recorded as `accepted_at`, so a golden shows
/// which source reconciliation used.
pub fn mock_block_timestamp(block: u64) -> u64 {
    1_700_000_000 + block
}

/// The receipt the mock chain holds for a fixture transaction, keyed by the record code and
/// attempt encoded in the hash (`0x<era><code><attempt>beef`). Era-independent.
pub fn mock_receipt(hash: &str) -> Option<Value> {
    let digits = hash.strip_prefix("0x")?.strip_suffix("beef")?;
    if digits.len() != 6 {
        return None;
    }
    let code = u8::from_str_radix(&digits[2..4], 16).ok()?;
    let attempt = u8::from_str_radix(&digits[4..6], 16).ok()?;
    let (block, status) = match (code, attempt) {
        (ACCEPTED, 0) => (120, "SUCCEEDED"),
        (COMMITTED, 0) => (121, "SUCCEEDED"),
        (REVERTED, 0) => (122, "REVERTED"),
        _ => return None,
    };
    Some(json!({
        "transaction_hash": hash,
        "block_number": block,
        "finality_status": "ACCEPTED_ON_L2",
        "execution_status": status,
    }))
}

/// The mock chain every golden runs against, at head `head`. Transactions named in
/// `landed` (after a resubmission) have a successful receipt in block 130.
pub fn mock_chain(head: u64, landed: Arc<Mutex<Vec<String>>>) -> Box<Route> {
    Box::new(move |method, params| match method {
        "starknet_getNonce" => Ok(json!(LIVE_NONCE)),
        "starknet_blockNumber" => Ok(json!(head)),
        "starknet_getBlockWithTxHashes" => {
            let block = params["block_id"]["block_number"]
                .as_u64()
                .ok_or((-32602, "bad block id".to_owned()))?;
            Ok(
                json!({"block_number":block,"timestamp":mock_block_timestamp(block),
                      "transactions":[]}),
            )
        }
        "starknet_getTransactionReceipt" => {
            let hash = params["transaction_hash"].as_str().unwrap_or_default();
            if landed.lock().expect("landed").iter().any(|h| h == hash) {
                return Ok(json!({"transaction_hash":hash,"block_number":130,
                    "finality_status":"ACCEPTED_ON_L2","execution_status":"SUCCEEDED"}));
            }
            mock_receipt(hash).ok_or((29, "Transaction hash not found".to_owned()))
        }
        other => Err((-32601, format!("mock chain does not serve {other}"))),
    })
}
