//! Durability and lifecycle behaviour of the operation journal.
//!
//! The properties worth testing here are the ones a crash would exercise: a record that
//! outlives the process that wrote it, an id that cannot be silently reused for a different
//! request, and a lifecycle that cannot run backwards.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use erebus_sdk::journal::{JournalError, OperationJournal, OperationStage, JOURNAL_VERSION};
use erebus_sdk::operation::{OperationId, RequestBinding, WriteOperation};
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
fn a_restart_keeps_the_earlier_attempts_transaction_hash() {
    let (_root, journal) = temporary_journal();
    let mut lease = journal
        .claim(&id(8), WriteOperation::Shield, binding(500), None, 1_000)
        .expect("claim");

    for stage in [
        OperationStage::Prepared,
        OperationStage::Proven,
        OperationStage::Signed,
    ] {
        lease.advance(stage, 1_001).expect("advance");
    }
    lease
        .amend(1_002, |attempt| {
            attempt.transaction_hash = Some(Felt::from_hex_unchecked("0xdeadbeef"));
        })
        .expect("amend");
    lease
        .advance(OperationStage::Submitted, 1_003)
        .expect("advance");

    lease.restart(2_000).expect("an expired proof restarts");

    let record = lease.record();
    assert_eq!(record.attempts.len(), 2);
    assert_eq!(record.stage(), OperationStage::Prepared);
    assert_eq!(
        record.attempts[0].transaction_hash,
        Some(Felt::from_hex_unchecked("0xdeadbeef")),
        "the old hash is the only thing that can prove the old attempt never landed"
    );
    assert!(
        record.may_have_landed(),
        "a submitted earlier attempt still counts as possibly landed"
    );
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
fn each_attempt_keeps_its_own_signed_transaction() {
    let (_root, journal) = temporary_journal();
    let mut lease = journal
        .claim(&id(14), WriteOperation::Shield, binding(500), None, 1_000)
        .expect("claim");
    for stage in [OperationStage::Prepared, OperationStage::Proven] {
        lease.advance(stage, 1_001).expect("advance");
    }
    lease
        .persist_signed(Felt::from_hex_unchecked("0x1111"), "first", 1_002)
        .expect("persist");
    lease
        .advance(OperationStage::Submitted, 1_003)
        .expect("submitted");

    lease.restart(2_000).expect("expired proof restarts");
    lease
        .advance(OperationStage::Proven, 2_001)
        .expect("proven");
    lease
        .persist_signed(Felt::from_hex_unchecked("0x2222"), "second", 2_002)
        .expect("persist");

    assert_eq!(
        lease.stored_transaction(0).expect("read"),
        Some("first".to_owned()),
        "the old attempt's transaction is what proves whether it landed"
    );
    assert_eq!(
        lease.stored_transaction(1).expect("read"),
        Some("second".to_owned())
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
