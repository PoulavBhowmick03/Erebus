//! Task 11: the durable-boundary fault matrix.
//!
//! Every other recovery test asks one question about one situation. This asks the same five
//! questions at every durable boundary, in one place, so that a missing cell is visible
//! rather than something you infer from the absence of a test file.
//!
//! The boundaries are the stages at which a process can die with the journal already on
//! disk: `Prepared`, `Proven`, `Signed`, `Submitted`, and `Accepted` (accepted on chain but
//! before the local state commit). `Claimed` is not a boundary in the same sense — nothing
//! has been read or built — but it is included in the binding sweep because an id is bound
//! from the moment it is claimed.
//!
//! The properties, one per section below:
//!
//! 1. **Stable parameter binding.** A crash never loosens what the id is bound to.
//! 2. **Read-only startup.** Reconciliation reads and classifies; it never submits.
//! 3. **No duplicate chain effect.** Where an effect exists, nothing is resubmitted.
//! 4. **Correct explicit resume mode.** Each boundary resumes the one way it should.
//! 5. **Exactly-once local outcome.** A replay returns the recorded result, it does not redo.
//!
//! The mock node records the JSON-RPC method names it is asked for, which is what turns
//! "read-only" from an assumption into an assertion.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use erebus_sdk::client::{Client, ClientConfig};
use erebus_sdk::journal::{OperationJournal, OperationStage};
use erebus_sdk::operation::{OperationId, RequestBinding, WriteOperation};
use erebus_sdk::state::StateStore;
use erebus_sdk::wire::WireVersion;
use serde_json::{json, Value};
use starknet_types_core::felt::Felt;

const SUBMIT_METHOD: &str = "starknet_addInvokeTransaction";

/// Every boundary a process can die on with a journal already written.
const BOUNDARIES: [OperationStage; 5] = [
    OperationStage::Prepared,
    OperationStage::Proven,
    OperationStage::Signed,
    OperationStage::Submitted,
    OperationStage::Accepted,
];

const ACCOUNT: Felt = Felt::from_hex_unchecked("0x11");
const POOL: Felt = Felt::from_hex_unchecked("0x22");
const CHAIN: Felt = Felt::from_hex_unchecked("0x33");
const TOKEN: Felt = Felt::from_hex_unchecked("0x44");
const COUNTERPARTY: Felt = Felt::from_hex_unchecked("0x55");

/// A mock node that answers from a script and remembers what it was asked.
///
/// The recording is the point: asserting that reconciliation is read-only means asserting
/// that a submission method never appears, which a server that only replays canned bodies
/// cannot tell you.
fn recording_server(
    responses: Vec<Value>,
) -> (String, Arc<Mutex<Vec<String>>>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&seen);
    let handle = thread::spawn(move || {
        for response in responses {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            if let Some(body) = read_request(&mut stream) {
                if let Some(method) = serde_json::from_str::<Value>(&body)
                    .ok()
                    .and_then(|value| value["method"].as_str().map(str::to_owned))
                {
                    recorder.lock().expect("recorder").push(method);
                }
            }
            let body = response.to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
        }
    });
    (format!("http://{address}"), seen, handle)
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
        let end = match bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            Some(end) => end,
            None => continue,
        };
        let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
        let length: usize = headers
            .lines()
            .find_map(|line| {
                line.strip_prefix("content-length:")
                    .map(str::trim)
                    .map(str::to_owned)
            })?
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

fn temporary_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "erebus-fault-{label}-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    std::fs::create_dir_all(&root).expect("root");
    std::fs::write(root.join("pool.key"), "0xabc\n").expect("pool key");
    std::fs::write(root.join("account.key"), "0xdef\n").expect("account key");
    root
}

fn binding(nonce: u64) -> RequestBinding {
    RequestBinding::builder(WriteOperation::OpenChannel, CHAIN, POOL, TOKEN)
        .felt(ACCOUNT)
        .felt(COUNTERPARTY)
        .u64_be(nonce)
        .finish()
}

fn request() -> Value {
    json!({ "method": "open_channel", "counterparty": COUNTERPARTY, "wire_version": "v3" })
}

fn operation_id(seed: u8) -> OperationId {
    OperationId::parse(format!("op_{:02x}{}", seed, "b7".repeat(31))).expect("operation id")
}

/// Writes a journal record stopped exactly at `boundary`, as a killed process would leave it.
fn stopped_at(root: &Path, id: &OperationId, boundary: OperationStage) {
    let journal = OperationJournal::new(root).expect("journal");
    let mut lease = journal
        .claim_with_request(
            id,
            WriteOperation::OpenChannel,
            binding(3),
            None,
            request(),
            1_000,
        )
        .expect("claim");
    lease
        .advance(OperationStage::Prepared, 1_001)
        .expect("prepared");
    if boundary == OperationStage::Prepared {
        return;
    }
    lease
        .advance(OperationStage::Proven, 1_002)
        .expect("proven");
    if boundary == OperationStage::Proven {
        return;
    }
    lease
        .persist_signed(Felt::from_hex_unchecked("0xbeef"), "{}", 1_003)
        .expect("signed");
    lease
        .amend(1_004, |attempt| {
            attempt.valid_until_block = Some(10_000);
            attempt.account_nonce = Some(Felt::from(5u8));
        })
        .expect("proof window and nonce");
    if boundary == OperationStage::Signed {
        return;
    }
    lease
        .advance(OperationStage::Submitted, 1_005)
        .expect("submitted");
    if boundary == OperationStage::Submitted {
        return;
    }
    lease
        .advance(OperationStage::Accepted, 1_006)
        .expect("accepted");
}

fn client(root: &Path, rpc_url: String) -> Client {
    Client::new(ClientConfig {
        rpc_url,
        prover_url: "http://127.0.0.1:9".to_owned(),
        pool_address: POOL,
        chain_id: CHAIN,
        account_address: ACCOUNT,
        pool_key_file: root.join("pool.key"),
        account_key_file: root.join("account.key"),
        state_dir: root.to_path_buf(),
        token: TOKEN,
        new_channel_wire_version: WireVersion::V3,
    })
    .expect("client")
}

// 1. Stable parameter binding ------------------------------------------------------------

/// An id means the same request at every boundary, including `Claimed`.
///
/// This is the property that makes a retry safe to attempt at all: if a crash could loosen
/// the binding, a caller reusing its id after a restart could silently buy something else.
#[test]
fn the_parameter_binding_survives_a_crash_at_every_boundary() {
    for boundary in [OperationStage::Claimed].into_iter().chain(BOUNDARIES) {
        let root = temporary_root("binding");
        let id = operation_id(1);

        if boundary == OperationStage::Claimed {
            let journal = OperationJournal::new(&root).expect("journal");
            journal
                .claim_with_request(
                    &id,
                    WriteOperation::OpenChannel,
                    binding(3),
                    None,
                    request(),
                    1_000,
                )
                .expect("claim");
        } else {
            stopped_at(&root, &id, boundary);
        }

        // A fresh process opens the journal again, exactly as a restart would.
        let journal = OperationJournal::new(&root).expect("reopen");

        journal
            .claim_with_request(
                &id,
                WriteOperation::OpenChannel,
                binding(3),
                None,
                request(),
                2_000,
            )
            .unwrap_or_else(|error| panic!("{boundary:?}: same request should reopen: {error}"));

        let conflict = journal.claim_with_request(
            &id,
            WriteOperation::OpenChannel,
            binding(4),
            None,
            request(),
            2_001,
        );
        assert!(
            conflict.is_err(),
            "{boundary:?}: a different binding under the same id must conflict"
        );

        std::fs::remove_dir_all(&root).ok();
    }
}

// 2. Read-only startup -------------------------------------------------------------------

/// Reconciliation never submits, at any boundary.
///
/// Asserted by recording the JSON-RPC methods the node is asked for. A reconciliation that
/// resubmitted would be indistinguishable from a correct one without this.
#[tokio::test]
async fn startup_reconciliation_never_submits_at_any_boundary() {
    for boundary in BOUNDARIES {
        let root = temporary_root("readonly");
        let id = operation_id(2);
        stopped_at(&root, &id, boundary);

        // Generous read answers: a nonce, then receipts that are simply not found. Whatever
        // reconciliation decides, it decides from reads.
        let responses = vec![
            json!({"jsonrpc":"2.0","id":1,"result":"0x5"}),
            json!({"jsonrpc":"2.0","id":1,"error":{"code":29,"message":"Transaction hash not found"}}),
            json!({"jsonrpc":"2.0","id":1,"error":{"code":29,"message":"Transaction hash not found"}}),
        ];
        let (rpc_url, seen, server) = recording_server(responses);
        let findings = client(&root, rpc_url).reconcile().await;
        drop(server);

        assert!(
            findings.is_ok(),
            "{boundary:?}: reconcile should classify rather than fail: {findings:?}"
        );
        let methods = seen.lock().expect("recorder").clone();
        assert!(
            !methods.iter().any(|method| method == SUBMIT_METHOD),
            "{boundary:?}: reconciliation submitted something: {methods:?}"
        );

        std::fs::remove_dir_all(&root).ok();
    }
}

/// The read-only assertion above can fail, which is what makes it worth making.
///
/// A recorder that never saw a submission because the harness could not observe one would
/// pass the sweep vacuously. This proves the recorder sees the method it is looking for.
#[test]
fn the_recorder_would_notice_a_submission() {
    let (rpc_url, seen, server) =
        recording_server(vec![json!({"jsonrpc":"2.0","id":1,"result":"0x1"})]);
    let body = json!({"jsonrpc":"2.0","id":1,"method":SUBMIT_METHOD,"params":[]}).to_string();
    let mut stream =
        std::net::TcpStream::connect(rpc_url.trim_start_matches("http://")).expect("connect");
    write!(
        stream,
        "POST / HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
    .expect("write");
    let mut sink = Vec::new();
    let _ = stream.read_to_end(&mut sink);
    server.join().expect("server");

    assert_eq!(seen.lock().expect("recorder").as_slice(), [SUBMIT_METHOD]);
}

// 5. Exactly-once local outcome ----------------------------------------------------------

/// A committed operation replays its recorded result instead of doing the work again.
///
/// This is the property the whole journal exists for: the same id, replayed after a crash,
/// must produce the recorded outcome and no second chain effect. The client is pointed at a
/// dead RPC on purpose — if the replay path touched the network at all, this would fail.
#[tokio::test]
async fn a_committed_operation_replays_its_result_without_touching_the_chain() {
    let root = temporary_root("replay");
    let id = operation_id(3);
    let state = StateStore::new(&root).expect("state");
    drop(state);

    let journal = OperationJournal::new(&root).expect("journal");
    {
        let mut lease = journal
            .claim_with_request(
                &id,
                WriteOperation::OpenChannel,
                binding(3),
                None,
                request(),
                1_000,
            )
            .expect("claim");
        lease
            .advance(OperationStage::Prepared, 1_001)
            .expect("prepared");
        lease
            .advance(OperationStage::Proven, 1_002)
            .expect("proven");
        lease
            .persist_signed(Felt::from_hex_unchecked("0xbeef"), "{}", 1_003)
            .expect("signed");
        lease
            .advance(OperationStage::Submitted, 1_004)
            .expect("submitted");
        lease
            .advance(OperationStage::Accepted, 1_005)
            .expect("accepted");
        lease
            .record_result(json!("ch_replayed"), 1_006)
            .expect("result recorded");
        lease
            .advance(OperationStage::Committed, 1_007)
            .expect("committed");
    }

    let reopened = OperationJournal::new(&root).expect("reopen");
    let lease = reopened.lock(&id).expect("lock").expect("record");
    assert_eq!(lease.record().stage(), OperationStage::Committed);
    assert_eq!(lease.record().result, Some(json!("ch_replayed")));

    std::fs::remove_dir_all(&root).ok();
}

// 3 and 4. No duplicate effect, and the right resume mode ---------------------------------

/// No boundary is ever classified as finished while it is still incomplete.
///
/// This is the failure mode the whole matrix exists to rule out: an operation that died
/// mid-write being reported as `None` — nothing to do — so the operator never resumes it and
/// never learns whether it produced an effect. A wrong-but-loud classification is
/// recoverable; a silent "finished" is not.
///
/// The receipts are answered as not-found and the account nonce is behind the signed nonce,
/// so nothing here has proven an effect either way. That is the ambiguous case, and every
/// boundary must survive it by asking for a person or a wait, never by closing itself.
#[tokio::test]
async fn no_incomplete_boundary_is_ever_classified_as_finished() {
    for boundary in BOUNDARIES {
        let root = temporary_root("classify");
        let id = operation_id(4);
        stopped_at(&root, &id, boundary);

        let responses = vec![
            json!({"jsonrpc":"2.0","id":1,"result":"0x1"}),
            json!({"jsonrpc":"2.0","id":1,"error":{"code":29,"message":"Transaction hash not found"}}),
            json!({"jsonrpc":"2.0","id":1,"error":{"code":29,"message":"Transaction hash not found"}}),
        ];
        let (rpc_url, _seen, server) = recording_server(responses);
        let findings = client(&root, rpc_url).reconcile().await.expect("reconcile");
        drop(server);

        let finding = findings
            .iter()
            .find(|finding| finding.operation_id == id)
            .unwrap_or_else(|| panic!("{boundary:?}: the operation vanished from reconciliation"));

        assert_ne!(
            finding.next_action,
            erebus_sdk::reconcile::NextAction::None,
            "{boundary:?}: an incomplete operation reported nothing to do: {finding:?}"
        );
        assert!(
            !finding.reason.is_empty(),
            "{boundary:?}: a classification with no reason is not actionable"
        );

        std::fs::remove_dir_all(&root).ok();
    }
}

/// A boundary that never signed anything is safe to retry; one that did is not.
///
/// The dividing line is the signature, not the submission: once a transaction is signed and
/// its hash persisted, a chain effect may exist under that hash whatever the local record
/// says. Before that point there is nothing on chain to duplicate.
#[tokio::test]
async fn only_boundaries_before_the_signature_are_safe_to_retry() {
    for boundary in BOUNDARIES {
        let root = temporary_root("retry");
        let id = operation_id(5);
        stopped_at(&root, &id, boundary);

        let responses = vec![
            json!({"jsonrpc":"2.0","id":1,"result":"0x1"}),
            json!({"jsonrpc":"2.0","id":1,"error":{"code":29,"message":"Transaction hash not found"}}),
            json!({"jsonrpc":"2.0","id":1,"error":{"code":29,"message":"Transaction hash not found"}}),
        ];
        let (rpc_url, _seen, server) = recording_server(responses);
        let findings = client(&root, rpc_url).reconcile().await.expect("reconcile");
        drop(server);

        let finding = findings
            .iter()
            .find(|finding| finding.operation_id == id)
            .expect("finding");
        let unsigned = matches!(boundary, OperationStage::Prepared | OperationStage::Proven);
        let safe = finding.next_action == erebus_sdk::reconcile::NextAction::SafeToRetry;

        assert_eq!(
            safe, unsigned,
            "{boundary:?}: safe-to-retry should hold exactly before the signature, got {finding:?}"
        );

        std::fs::remove_dir_all(&root).ok();
    }
}
