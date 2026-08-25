//! Durability and lifecycle behaviour of the operation journal.
//!
//! The properties worth testing here are the ones a crash would exercise: a record that
//! outlives the process that wrote it, an id that cannot be silently reused for a different
//! request, and a lifecycle that cannot run backwards.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use erebus_sdk::journal::{
    JournalError, OperationJournal, OperationStage, PreparedSnapshot, JOURNAL_VERSION,
};
use erebus_sdk::operation::{OperationId, RequestBinding, WriteOperation};
use erebus_sdk::rpc::Receipt;
use fs2::FileExt;
use serde_json::json;
use starknet_types_core::felt::Felt;

const CHAIN: Felt = Felt::from_hex_unchecked("0x534e5f5345504f4c4941");
const POOL: Felt = Felt::from_hex_unchecked("0x4e4f");
const TOKEN: Felt = Felt::from_hex_unchecked("0x53545f");

fn id(seed: u8) -> OperationId {
    OperationId::parse(format!("op_{}", format!("{seed:02x}").repeat(32)))
        .expect("constructed id is well formed")
}

fn binding(amount: u128) -> RequestBinding {
    RequestBinding::builder(WriteOperation::Shield, CHAIN, POOL, TOKEN)
        .u128_be(amount)
        .finish()
}

fn temporary_journal() -> (PathBuf, OperationJournal) {
    // A counter, not a timestamp: these tests run in parallel and macOS reports coarse
    // nanoseconds, so two of them collided on one directory and saw each other's records.
    static NEXT: AtomicUsize = AtomicUsize::new(0);

    let root = std::env::temp_dir().join(format!(
        "erebus-journal-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let journal = OperationJournal::new(&root).expect("journal opens");
    (root, journal)
}

#[test]
fn a_claim_survives_the_process_that_wrote_it() {
    let (root, journal) = temporary_journal();

    {
        let lease = journal
            .claim(&id(1), WriteOperation::Shield, binding(500), None, 1_000)
            .expect("first claim succeeds");
        assert_eq!(lease.record().stage(), OperationStage::Claimed);
        assert_eq!(lease.record().version, JOURNAL_VERSION);
        assert_eq!(lease.record().created_at, 1_000);
    }

    // A completely fresh handle on the same directory, as a restarted process would open.
    let reopened = OperationJournal::new(&root).expect("journal reopens");
    let lease = reopened
        .lock(&id(1))
        .expect("lock succeeds")
        .expect("the record is still there");

    assert_eq!(lease.record().operation_id, id(1));
    assert_eq!(lease.record().stage(), OperationStage::Claimed);
}

#[test]
fn reclaiming_an_id_with_the_same_request_reopens_the_record() {
    let (_root, journal) = temporary_journal();

    {
        let mut lease = journal
            .claim(&id(2), WriteOperation::Shield, binding(500), None, 1_000)
            .expect("first claim");
        lease
            .advance(OperationStage::Prepared, 1_001)
            .expect("advance");
    }

    let lease = journal
        .claim(&id(2), WriteOperation::Shield, binding(500), None, 2_000)
        .expect("same request reopens rather than conflicting");

    assert_eq!(lease.record().stage(), OperationStage::Prepared);
    assert_eq!(
        lease.record().created_at,
        1_000,
        "reopening must not restamp the record"
    );
}

#[test]
fn reclaiming_an_id_with_a_different_request_conflicts() {
    let (_root, journal) = temporary_journal();

    drop(
        journal
            .claim(&id(3), WriteOperation::Shield, binding(500), None, 1_000)
            .expect("first claim"),
    );

    let error = journal
        .claim(&id(3), WriteOperation::Shield, binding(501), None, 2_000)
        .expect_err("a different amount is a different effect");

    assert!(matches!(error, JournalError::BindingConflict { .. }));
}

#[test]
fn canonical_request_must_match_even_when_the_binding_matches() {
    let (_root, journal) = temporary_journal();
    let operation_id = id(31);
    drop(
        journal
            .claim_with_request(
                &operation_id,
                WriteOperation::Shield,
                binding(500),
                None,
                json!({"method":"shield","amount":"500"}),
                1_000,
            )
            .expect("first claim"),
    );

    let error = journal
        .claim_with_request(
            &operation_id,
            WriteOperation::Shield,
            binding(500),
            None,
            json!({"method":"shield","amount":"501"}),
            2_000,
        )
        .expect_err("canonical parameters cannot change under the same digest");

    assert!(matches!(error, JournalError::RequestConflict { .. }));
}

#[test]
fn every_recovery_fact_survives_a_restart() {
    let (root, journal) = temporary_journal();
    let operation_id = id(32);
    let request = json!({"method":"shield","amount":"500","wire_version":"v3"});
    let completion = json!({
        "result": {"kind":"settlement","offer_id":null,"nullifiers":[],
                   "selected_input":null,"change":null},
        "local_mutation": null
    });
    let result = json!({
        "offer_id": null,
        "tx_hash": "0xbeef",
        "nullifiers": [],
        "proved_at": 10,
    });
    {
        let mut lease = journal
            .claim_with_request(
                &operation_id,
                WriteOperation::Shield,
                binding(500),
                None,
                request.clone(),
                1_000,
            )
            .expect("claim");
        lease
            .record_prepared(
                PreparedSnapshot {
                    deposit: "500".to_owned(),
                    proof_validity_blocks: 450,
                    fee_per_write: "2".to_owned(),
                    allowance: "502".to_owned(),
                    public_balance: "1000".to_owned(),
                },
                1_001,
            )
            .expect("prepared inputs");
        lease
            .record_completion(completion.clone(), 1_002)
            .expect("completion");
        lease
            .advance(OperationStage::Prepared, 1_003)
            .expect("prepared");
        lease
            .advance(OperationStage::Proven, 1_004)
            .expect("proven");
        lease
            .persist_signed(Felt::from_hex_unchecked("0xbeef"), "{}", 1_005)
            .expect("signed");
        lease
            .advance(OperationStage::Submitted, 1_006)
            .expect("submitted");
        lease
            .record_receipt(
                Receipt {
                    transaction_hash: "0xbeef".to_owned(),
                    block_number: Some(11),
                    finality_status: Some("ACCEPTED_ON_L2".to_owned()),
                    execution_status: Some("SUCCEEDED".to_owned()),
                    revert_reason: None,
                },
                1_007,
            )
            .expect("receipt");
        lease
            .advance(OperationStage::Accepted, 1_008)
            .expect("accepted");
        lease.record_result(result.clone(), 1_009).expect("result");
        lease
            .advance(OperationStage::Committed, 1_010)
            .expect("committed");
    }

    let reopened = OperationJournal::new(&root).expect("reopen");
    let lease = reopened
        .lock(&operation_id)
        .expect("lock")
        .expect("record exists");
    assert_eq!(lease.record().request.as_ref(), Some(&request));
    assert_eq!(
        lease.record().attempt().completion.as_ref(),
        Some(&completion)
    );
    assert_eq!(lease.record().result.as_ref(), Some(&result));
    assert_eq!(
        lease
            .record()
            .attempt()
            .prepared
            .as_ref()
            .map(|value| value.deposit.as_str()),
        Some("500")
    );
    assert!(lease.record().attempt().receipt.is_some());
}

#[test]
fn an_operation_lease_also_holds_the_identity_write_lock() {
    let (root, journal) = temporary_journal();
    let _lease = journal
        .claim(&id(33), WriteOperation::Shield, binding(500), None, 1_000)
        .expect("claim");
    let identity_lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join("operations/.identity.lock"))
        .expect("identity lock file");

    assert!(
        identity_lock.try_lock_exclusive().is_err(),
        "a different operation must not pass startup reconciliation while this write runs"
    );
}

#[test]
fn an_unclaimed_id_locks_to_nothing() {
    let (_root, journal) = temporary_journal();

    assert!(journal
        .lock(&id(4))
        .expect("locking an unknown id is not an error")
        .is_none());
}

#[test]
fn the_lifecycle_cannot_run_backwards_or_skip_a_stage() {
    let (_root, journal) = temporary_journal();
    let mut lease = journal
        .claim(&id(5), WriteOperation::Shield, binding(500), None, 1_000)
        .expect("claim");

    for (index, stage) in [
        OperationStage::Prepared,
        OperationStage::Proven,
        OperationStage::Signed,
        OperationStage::Submitted,
        OperationStage::Accepted,
        OperationStage::Committed,
    ]
    .into_iter()
    .enumerate()
    {
        lease
            .advance(stage, 1_001 + index as u64)
            .expect("each forward step is legal");
    }

    // Terminal: nothing follows a commit.
    assert!(matches!(
        lease.advance(OperationStage::Submitted, 2_000),
        Err(JournalError::IllegalTransition { .. })
    ));
    drop(lease);

    let mut skipping = journal
        .claim(&id(6), WriteOperation::Shield, binding(500), None, 1_000)
        .expect("claim");
    assert!(
        matches!(
            skipping.advance(OperationStage::Submitted, 1_001),
            Err(JournalError::IllegalTransition { .. })
        ),
        "claimed must not jump straight to submitted"
    );
}

#[test]
fn every_stage_change_is_on_disk_before_it_returns() {
    let (root, journal) = temporary_journal();
    let mut lease = journal
        .claim(&id(7), WriteOperation::Shield, binding(500), None, 1_000)
        .expect("claim");
    lease
        .advance(OperationStage::Prepared, 1_001)
        .expect("advance");

    // Read through a second handle while the lease is still alive. If `advance` only
    // mutated memory and deferred the write to a later commit, a crash here would lose it.
    let observed = OperationJournal::new(&root)
        .expect("reopen")
        .records()
        .expect("records read");

    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].stage(), OperationStage::Prepared);
}

#[test]
fn a_record_that_cannot_be_parsed_fails_closed() {
    let (root, journal) = temporary_journal();
    drop(
        journal
            .claim(&id(9), WriteOperation::Shield, binding(500), None, 1_000)
            .expect("claim"),
    );

    let path = root
        .join("operations")
        .join(format!("{}.json", id(9).as_str()));
    std::fs::write(&path, b"{ not json").expect("corrupt the record");

    assert!(
        matches!(journal.records(), Err(JournalError::Json { .. })),
        "an unreadable record must not be skipped as if it were absent"
    );
    assert!(matches!(
        journal.lock(&id(9)),
        Err(JournalError::Json { .. })
    ));
}

#[test]
fn a_record_from_a_newer_schema_is_refused() {
    let (root, journal) = temporary_journal();
    drop(
        journal
            .claim(&id(10), WriteOperation::Shield, binding(500), None, 1_000)
            .expect("claim"),
    );

    let path = root
        .join("operations")
        .join(format!("{}.json", id(10).as_str()));
    let text = std::fs::read_to_string(&path).expect("read record");
    let bumped = text.replace(
        &format!("\"version\":{JOURNAL_VERSION}"),
        &format!("\"version\":{}", JOURNAL_VERSION + 1),
    );
    assert_ne!(
        text, bumped,
        "version field was not where the test expected"
    );
    std::fs::write(&path, bumped).expect("write record");

    assert!(matches!(
        journal.lock(&id(10)),
        Err(JournalError::UnsupportedVersion(_))
    ));
}

#[cfg(unix)]
#[test]
fn records_are_not_readable_by_anyone_else() {
    use std::os::unix::fs::PermissionsExt;

    let (root, journal) = temporary_journal();
    drop(
        journal
            .claim(&id(11), WriteOperation::Shield, binding(500), None, 1_000)
            .expect("claim"),
    );

    let directory = root.join("operations");
    let record = directory.join(format!("{}.json", id(11).as_str()));

    let directory_mode = std::fs::metadata(&directory)
        .expect("stat directory")
        .permissions()
        .mode()
        & 0o777;
    let record_mode = std::fs::metadata(&record)
        .expect("stat record")
        .permissions()
        .mode()
        & 0o777;

    assert_eq!(directory_mode, 0o700);
    assert_eq!(record_mode, 0o600);
}

#[test]
fn the_signed_transaction_is_on_disk_before_the_stage_says_signed() {
    let (root, journal) = temporary_journal();
    let mut lease = journal
        .claim(&id(12), WriteOperation::Shield, binding(500), None, 1_000)
        .expect("claim");
    lease
        .advance(OperationStage::Prepared, 1_001)
        .expect("prepared");
    lease
        .advance(OperationStage::Proven, 1_002)
        .expect("proven");

    lease
        .persist_signed(
            Felt::from_hex_unchecked("0xfeed"),
            r#"{"type":"INVOKE"}"#,
            1_003,
        )
        .expect("persist");

    // Read the bytes straight off disk while the lease is still held, which is what a
    // process killed at this instant would leave behind. Both the transaction and the hash
    // naming it have to be there; either one alone is a state recovery cannot act on.
    // (Deliberately not `lock()`: re-locking a held id blocks, it does not fail.)
    let directory = root.join("operations");
    let record: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(directory.join(format!("{}.json", id(12).as_str())))
            .expect("record is on disk"),
    )
    .expect("record parses");
    let stored = std::fs::read_to_string(directory.join(format!("{}.0.tx", id(12).as_str())))
        .expect("transaction is on disk");

    let attempt = record["attempts"].as_array().expect("attempts")[0].clone();
    assert_eq!(attempt["stage"], "signed");
    assert_eq!(attempt["transaction_stored"], true);
    assert_eq!(stored, r#"{"type":"INVOKE"}"#);
    assert_eq!(lease.record().stage(), OperationStage::Signed);
}

#[test]
fn a_missing_transaction_file_is_not_read_as_never_submitted() {
    let (root, journal) = temporary_journal();
    let mut lease = journal
        .claim(&id(13), WriteOperation::Shield, binding(500), None, 1_000)
        .expect("claim");
    for stage in [OperationStage::Prepared, OperationStage::Proven] {
        lease.advance(stage, 1_001).expect("advance");
    }
    lease
        .persist_signed(Felt::from_hex_unchecked("0xfeed"), "{}", 1_002)
        .expect("persist");

    std::fs::remove_file(
        root.join("operations")
            .join(format!("{}.0.tx", id(13).as_str())),
    )
    .expect("delete the stored transaction");

    assert!(
        matches!(
            lease.stored_transaction(0),
            Err(JournalError::Corrupt { .. })
        ),
        "a record naming a transaction that is gone must fail closed, not report absence"
    );
}

#[test]
fn a_write_with_no_proof_goes_straight_from_prepared_to_signed() {
    let (_root, journal) = temporary_journal();
    let mut lease = journal
        .claim(
            &id(15),
            WriteOperation::ApprovePool,
            binding(500),
            None,
            1_000,
        )
        .expect("claim");
    lease
        .advance(OperationStage::Prepared, 1_001)
        .expect("prepared");

    lease
        .persist_signed(Felt::from_hex_unchecked("0xabc"), "{}", 1_002)
        .expect("an approve has nothing to prove");

    assert_eq!(lease.record().stage(), OperationStage::Signed);
}

/// A finished record older than the window is removed, with everything filed under its id.
#[test]
fn pruning_removes_a_finished_record_and_its_files() {
    let (root, journal) = temporary_journal();
    let id = id(0x31);
    {
        let mut lease = journal
            .claim_with_request(
                &id,
                WriteOperation::Shield,
                binding(1),
                None,
                json!({}),
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
            .advance(OperationStage::Committed, 1_006)
            .expect("committed");
    }

    let record_file = root.join(format!("operations/{}.json", id.as_str()));
    let transaction_file = root.join(format!("operations/{}.0.tx", id.as_str()));
    assert!(record_file.exists() && transaction_file.exists());

    let report = journal.prune(3_600, 1_006 + 3_600).expect("prune");

    assert_eq!(report.pruned, 1);
    assert_eq!(report.examined(), 1);
    assert!(!record_file.exists(), "the record survived");
    assert!(
        !transaction_file.exists(),
        "the stored transaction survived"
    );
    assert!(
        !root
            .join(format!("operations/{}.lock", id.as_str()))
            .exists(),
        "the lock file survived"
    );
    assert!(journal.records().expect("records").is_empty());
}

/// Every stage that is not terminal survives a prune, whatever its age.
///
/// This is the property that matters: a record is the only thing that knows a transaction
/// was signed, so pruning an unfinished one turns a recoverable operation into an invisible
/// one. Ages are set absurdly old so that age cannot be what saves them.
#[test]
fn pruning_never_touches_an_unfinished_record() {
    for stage in [
        OperationStage::Claimed,
        OperationStage::Prepared,
        OperationStage::Proven,
        OperationStage::Signed,
        OperationStage::Submitted,
        OperationStage::Accepted,
    ] {
        let (_root, journal) = temporary_journal();
        let id = id(0x32);
        {
            let mut lease = journal
                .claim_with_request(&id, WriteOperation::Shield, binding(1), None, json!({}), 10)
                .expect("claim");
            if stage != OperationStage::Claimed {
                lease
                    .advance(OperationStage::Prepared, 11)
                    .expect("prepared");
            }
            if matches!(
                stage,
                OperationStage::Proven
                    | OperationStage::Signed
                    | OperationStage::Submitted
                    | OperationStage::Accepted
            ) {
                lease.advance(OperationStage::Proven, 12).expect("proven");
            }
            if matches!(
                stage,
                OperationStage::Signed | OperationStage::Submitted | OperationStage::Accepted
            ) {
                lease
                    .persist_signed(Felt::from_hex_unchecked("0xbeef"), "{}", 13)
                    .expect("signed");
            }
            if matches!(stage, OperationStage::Submitted | OperationStage::Accepted) {
                lease
                    .advance(OperationStage::Submitted, 14)
                    .expect("submitted");
            }
            if stage == OperationStage::Accepted {
                lease
                    .advance(OperationStage::Accepted, 15)
                    .expect("accepted");
            }
        }

        let report = journal.prune(1, 10_000_000).expect("prune");

        assert_eq!(report.pruned, 0, "{stage:?} was pruned");
        assert_eq!(report.retained_unfinished, 1, "{stage:?}");
        assert_eq!(journal.records().expect("records").len(), 1, "{stage:?}");
    }
}

/// `NeedsAttention` is never pruned, at any age.
///
/// It is not terminal precisely because a person still has to look at it. Deleting it
/// destroys the evidence they were going to look at, and the operation would afterwards
/// reconcile as though it had never happened.
#[test]
fn pruning_never_touches_a_record_waiting_for_an_operator() {
    let (_root, journal) = temporary_journal();
    let id = id(0x33);
    {
        let mut lease = journal
            .claim_with_request(&id, WriteOperation::Shield, binding(1), None, json!({}), 10)
            .expect("claim");
        lease
            .advance(OperationStage::Prepared, 11)
            .expect("prepared");
        lease.advance(OperationStage::Proven, 12).expect("proven");
        lease
            .persist_signed(Felt::from_hex_unchecked("0xbeef"), "{}", 13)
            .expect("signed");
        lease
            .advance(OperationStage::Submitted, 14)
            .expect("submitted");
        lease
            .advance(OperationStage::NeedsAttention, 15)
            .expect("needs attention");
    }

    let report = journal.prune(1, 10_000_000).expect("prune");

    assert_eq!(report.pruned, 0);
    assert_eq!(report.retained_unfinished, 1);
    assert_eq!(journal.records().expect("records").len(), 1);
}

/// A record that finished inside the window is kept, and reported as kept.
#[test]
fn pruning_keeps_a_recently_finished_record_and_says_so() {
    let (_root, journal) = temporary_journal();
    let id = id(0x34);
    {
        let mut lease = journal
            .claim_with_request(
                &id,
                WriteOperation::Shield,
                binding(1),
                None,
                json!({}),
                1_000,
            )
            .expect("claim");
        // Claimed -> Committed is the legal idempotent no-chain edge (see can_advance_to);
        // Prepared -> Committed is not, which is what a settled effect that needed no write
        // looks like in the journal.
        lease
            .advance(OperationStage::Committed, 1_002)
            .expect("committed");
    }

    let report = journal.prune(3_600, 1_002 + 60).expect("prune");

    assert_eq!(report.pruned, 0);
    assert_eq!(report.retained_recent, 1);
    assert_eq!(journal.records().expect("records").len(), 1);
}

/// A clock that moved backwards must not read as "infinitely old".
///
/// `now` before the record's own timestamp would underflow a subtraction into a huge age and
/// delete something written seconds ago. Saturating arithmetic makes it read as age zero,
/// which keeps the record.
#[test]
fn a_clock_that_went_backwards_does_not_delete_everything() {
    let (_root, journal) = temporary_journal();
    let id = id(0x35);
    {
        let mut lease = journal
            .claim_with_request(
                &id,
                WriteOperation::Shield,
                binding(1),
                None,
                json!({}),
                9_000,
            )
            .expect("claim");
        lease
            .advance(OperationStage::Committed, 9_002)
            .expect("committed");
    }

    let report = journal
        .prune(3_600, 5_000)
        .expect("prune with an earlier clock");

    assert_eq!(report.pruned, 0);
    assert_eq!(report.retained_recent, 1);
}
