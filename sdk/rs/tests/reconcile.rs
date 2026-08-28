//! Startup classification of journalled operations.
//!
//! The distinction under test throughout is between "this did not happen" and "I cannot
//! tell". Only the first is safe to act on, and every path that cannot establish it has to
//! say so rather than default to the convenient answer.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use erebus_sdk::journal::{Attempt, OperationJournal, OperationRecord, OperationStage};
use erebus_sdk::operation::{OperationId, RequestBinding, WriteOperation};
use erebus_sdk::reconcile::{reconcile, NextAction, Outcome};
use erebus_sdk::rpc::StarknetRpc;
use serde_json::{json, Value};
use starknet_types_core::felt::Felt;

const ACCOUNT: Felt = Felt::from_hex_unchecked("0xacc");
const CHAIN: Felt = Felt::from_hex_unchecked("0x534e5f5345504f4c4941");
const POOL: Felt = Felt::from_hex_unchecked("0x4e4f");
const TOKEN: Felt = Felt::from_hex_unchecked("0x53545f");
const TX: Felt = Felt::from_hex_unchecked("0xbeef");

fn server(responses: Vec<Value>) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    let handle = thread::spawn(move || {
        for response in responses {
            let (mut stream, _) = listener.accept().expect("accept");
            drain_request(&mut stream);
            let body = response.to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("response");
        }
    });
    (format!("http://{address}"), handle)
}

fn drain_request(stream: &mut TcpStream) {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let read = stream.read(&mut buffer).expect("request read");
        assert!(read > 0, "connection closed before headers");
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..end]);
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(str::trim)
                        .map(str::to_owned)
                })
                .expect("content-length")
                .parse()
                .expect("content length");
            let body_start = end + 4;
            while bytes.len() < body_start + length {
                let read = stream.read(&mut buffer).expect("body read");
                assert!(read > 0, "connection closed mid-body");
                bytes.extend_from_slice(&buffer[..read]);
            }
            return;
        }
    }
}

fn nonce_response(nonce: &str) -> Value {
    json!({"jsonrpc":"2.0","id":1,"result":nonce})
}

fn receipt(execution_status: &str) -> Value {
    json!({"jsonrpc":"2.0","id":1,"result":{
        "transaction_hash": format!("{TX:#x}"),
        "block_number": 12,
        "finality_status": "ACCEPTED_ON_L2",
        "execution_status": execution_status
    }})
}

fn block(timestamp: u64) -> Value {
    json!({"jsonrpc":"2.0","id":1,"result":{
        "block_number":12,
        "timestamp":timestamp,
        "transactions":[]
    }})
}

fn not_found() -> Value {
    json!({"jsonrpc":"2.0","id":1,"error":{"code":29,"message":"Transaction hash not found"}})
}

/// Builds a record on disk at `stage`, then reads it back so the test sees exactly what a
/// restarted process would.
fn record_at(stage: OperationStage, edit: impl FnOnce(&mut Attempt)) -> OperationRecord {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let root: PathBuf = std::env::temp_dir().join(format!(
        "erebus-reconcile-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let journal = OperationJournal::new(&root).expect("journal");
    let id = OperationId::parse(format!("op_{}", "7a".repeat(32))).expect("id");
    let binding = RequestBinding::builder(WriteOperation::Shield, CHAIN, POOL, TOKEN)
        .u128_be(1)
        .finish();
    let mut lease = journal
        .claim(&id, WriteOperation::Shield, binding, None, 1_000)
        .expect("claim");

    // Reverted and NeedsAttention are not on the straight line, so each names the stage it
    // is reached from rather than being appended to the end of the walk.
    let walk: &[OperationStage] = match stage {
        OperationStage::Claimed => &[],
        OperationStage::Prepared => &[OperationStage::Prepared],
        OperationStage::Proven => &[OperationStage::Prepared, OperationStage::Proven],
        OperationStage::Signed | OperationStage::NeedsAttention => &[
            OperationStage::Prepared,
            OperationStage::Proven,
            OperationStage::Signed,
        ],
        OperationStage::Submitted | OperationStage::Reverted => &[
            OperationStage::Prepared,
            OperationStage::Proven,
            OperationStage::Signed,
            OperationStage::Submitted,
        ],
        OperationStage::Accepted => &[
            OperationStage::Prepared,
            OperationStage::Proven,
            OperationStage::Signed,
            OperationStage::Submitted,
            OperationStage::Accepted,
        ],
        OperationStage::Committed => &[
            OperationStage::Prepared,
            OperationStage::Proven,
            OperationStage::Signed,
            OperationStage::Submitted,
            OperationStage::Accepted,
            OperationStage::Committed,
        ],
    };
    for step in walk {
        if *step == OperationStage::Signed {
            lease
                .persist_signed(TX, "{}", 1_002)
                .expect("persist signed");
            continue;
        }
        lease.advance(*step, 1_003).expect("advance");
    }
    if matches!(
        stage,
        OperationStage::Reverted | OperationStage::NeedsAttention
    ) {
        lease.advance(stage, 1_004).expect("terminal stage");
    }
    lease.amend(1_005, edit).expect("amend");

    let record = lease.record().clone();
    assert_eq!(record.stage(), stage, "fixture did not reach {stage:?}");
    record
}

async fn classify_one(
    record: OperationRecord,
    responses: Vec<Value>,
) -> (Outcome, NextAction, Option<u64>) {
    let (url, thread) = server(responses);
    let rpc = StarknetRpc::new(url).expect("rpc");
    let findings = reconcile(&rpc, ACCOUNT, std::slice::from_ref(&record))
        .await
        .expect("reconcile");
    thread.join().expect("server");
    assert_eq!(findings.len(), 1);
    (
        findings[0].outcome,
        findings[0].next_action,
        findings[0].accepted_at,
    )
}

#[tokio::test]
async fn an_unsigned_operation_needs_no_chain_read_at_all() {
    // Only one response is queued: the account nonce. If classification tried to look up a
    // receipt for a stage that never signed anything, the server would run dry and the test
    // would hang or error rather than pass.
    for stage in [
        OperationStage::Claimed,
        OperationStage::Prepared,
        OperationStage::Proven,
    ] {
        let (outcome, action, _accepted_at) =
            classify_one(record_at(stage, |_| {}), vec![nonce_response("0x5")]).await;
        assert_eq!(outcome, Outcome::NoEffect, "{stage:?}");
        assert_eq!(action, NextAction::SafeToRetry, "{stage:?}");
    }
}

#[tokio::test]
async fn a_submitted_transaction_with_a_successful_receipt_has_an_effect() {
    let (outcome, action, accepted_at) = classify_one(
        record_at(OperationStage::Submitted, |_| {}),
        vec![
            nonce_response("0x5"),
            receipt("SUCCEEDED"),
            block(1_700_000_012),
        ],
    )
    .await;

    assert_eq!(outcome, Outcome::Effect);
    assert_eq!(accepted_at, Some(1_700_000_012));
    assert_eq!(
        action,
        NextAction::CommitLocalState,
        "the chain has it and local state does not"
    );
}

#[tokio::test]
async fn a_reverted_receipt_means_no_effect() {
    let (outcome, action, _accepted_at) = classify_one(
        record_at(OperationStage::Submitted, |_| {}),
        vec![nonce_response("0x5"), receipt("REVERTED")],
    )
    .await;

    assert_eq!(outcome, Outcome::Reverted);
    assert_eq!(action, NextAction::SafeToRetry);
}

#[tokio::test]
async fn a_missing_receipt_alone_does_not_prove_the_transaction_never_landed() {
    // The node has not seen it and the nonce it was signed against is still current, so it
    // can still be included. Answering "no effect" here is how a duplicate payment happens.
    let (outcome, action, _accepted_at) = classify_one(
        record_at(OperationStage::Submitted, |attempt| {
            attempt.account_nonce = Some(Felt::from(5u8));
        }),
        vec![nonce_response("0x5"), not_found()],
    )
    .await;

    assert_eq!(outcome, Outcome::Pending);
    assert_eq!(action, NextAction::Wait);
}

#[tokio::test]
async fn a_missing_receipt_past_the_signed_nonce_does_prove_it() {
    // The account has moved on. The transaction is bound to nonce 5 and the account is at 6,
    // so no one can ever include it.
    let (outcome, action, _accepted_at) = classify_one(
        record_at(OperationStage::Submitted, |attempt| {
            attempt.account_nonce = Some(Felt::from(5u8));
        }),
        vec![nonce_response("0x6"), not_found()],
    )
    .await;

    assert_eq!(outcome, Outcome::NoEffect);
    assert_eq!(action, NextAction::SafeToRetry);
}

#[tokio::test]
async fn a_missing_receipt_with_no_recorded_nonce_is_unknown_not_absent() {
    let (outcome, action, _accepted_at) = classify_one(
        record_at(OperationStage::Submitted, |attempt| {
            attempt.account_nonce = None;
        }),
        vec![nonce_response("0x9"), not_found()],
    )
    .await;

    assert_eq!(
        outcome,
        Outcome::Unknown,
        "without the signed nonce there is nothing to compare and no conclusion to draw"
    );
    assert_eq!(action, NextAction::OperatorAttention);
}

#[tokio::test]
async fn a_signed_stage_without_a_hash_is_a_contradiction_and_escalates() {
    let (outcome, action, _accepted_at) = classify_one(
        record_at(OperationStage::Signed, |attempt| {
            attempt.transaction_hash = None;
        }),
        vec![nonce_response("0x5")],
    )
    .await;

    assert_eq!(outcome, Outcome::Unknown);
    assert_eq!(action, NextAction::OperatorAttention);
}

#[tokio::test]
async fn a_committed_operation_is_finished() {
    let (outcome, action, accepted_at) = classify_one(
        record_at(OperationStage::Committed, |_| {}),
        vec![
            nonce_response("0x5"),
            receipt("SUCCEEDED"),
            block(1_700_000_012),
        ],
    )
    .await;

    assert_eq!(outcome, Outcome::Effect);
    assert_eq!(accepted_at, Some(1_700_000_012));
    assert_eq!(action, NextAction::None);
}

#[tokio::test]
async fn an_operation_parked_for_an_operator_stays_parked() {
    let (outcome, action, _accepted_at) = classify_one(
        record_at(OperationStage::NeedsAttention, |_| {}),
        vec![nonce_response("0x5")],
    )
    .await;

    assert_eq!(outcome, Outcome::Unknown);
    assert_eq!(action, NextAction::OperatorAttention);
}
