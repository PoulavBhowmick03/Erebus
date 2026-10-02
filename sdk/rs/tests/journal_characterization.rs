//! Characterization of the Starknet operation journal (Metropolis M6, slice 1).
//!
//! M6 moves the journal's chain-neutral mechanics into a shared crate. These tests pin what
//! the journal does *today* so that move can be shown to preserve behaviour: the stage table,
//! the on-disk layout and modes, the exact bytes it writes, how it treats records written by
//! every historical schema, and the text of every error.
//!
//! They are not a specification. Where current behaviour looks wrong it is pinned anyway and
//! marked `CHARACTERIZATION: current behavior, flagged for review`.
//!
//! The records under `tests/fixtures/journal/v*-<commit>/` were written by the journal code
//! of those commits; `tests/fixtures/journal/README.md` says how. Goldens live in
//! `tests/fixtures/journal/golden/` and are compared byte for byte. To regenerate them after
//! an intended change, rerun with `EREBUS_BLESS_JOURNAL_GOLDENS=1` and review the diff.

mod journal_support;

use erebus_sdk::journal::{
    JournalError, OperationJournal, OperationStage, PreparedSnapshot, JOURNAL_VERSION,
};
use erebus_sdk::operation::{OperationId, RequestBinding, WriteOperation};
use erebus_sdk::rpc::Receipt;
use erebus_sdk::state::ChannelHandle;
use fs2::FileExt;
use journal_support::*;
use serde_json::{json, Value};
use starknet_types_core::felt::Felt;

const STAGES: [OperationStage; 9] = [
    OperationStage::Claimed,
    OperationStage::Prepared,
    OperationStage::Proven,
    OperationStage::Signed,
    OperationStage::Submitted,
    OperationStage::Accepted,
    OperationStage::Committed,
    OperationStage::Reverted,
    OperationStage::NeedsAttention,
];

/// The v4 fixture whose latest attempt is at each stage, in [`STAGES`] order.
const STAGE_FIXTURES: [u8; 9] = [
    CLAIMED,
    PREPARED,
    PROVEN,
    SIGNED,
    SUBMITTED,
    ACCEPTED,
    COMMITTED,
    REVERTED,
    NEEDS_ATTENTION,
];

// ---- 1. The stage table ------------------------------------------------------------------

/// Every `(from, to)` pair of `can_advance_to`, plus `is_terminal` and `may_have_landed`.
///
/// Columns and rows are in [`STAGES`] order: claimed, prepared, proven, signed, submitted,
/// accepted, committed, reverted, needs_attention. `x` is a legal edge.
#[test]
fn the_stage_table_is_exactly_this() {
    const EDGES: [&str; 9] = [
        // C P V S U A M R N
        ". x . . . . x . x", // claimed
        ". . x x . . . . x", // prepared
        ". . . x . . . . x", // proven
        ". . . . x . . . x", // signed
        ". . . . . x . x x", // submitted
        ". . . . . . x . x", // accepted
        ". . . . . . . . .", // committed
        ". . . . . . . . .", // reverted
        // CHARACTERIZATION: current behavior, flagged for review: needs_attention -> needs_attention
        // is legal (the catch-all `(from, NeedsAttention) => !from.is_terminal()`), and
        // needs_attention -> committed skips accepted. journal.rs:110-112.
        ". . . . . x x x x", // needs_attention
    ];
    for (row, from) in STAGES.into_iter().enumerate() {
        let cells: Vec<&str> = EDGES[row].split(' ').collect();
        for (column, to) in STAGES.into_iter().enumerate() {
            assert_eq!(
                from.can_advance_to(to),
                cells[column] == "x",
                "{from:?} -> {to:?}"
            );
        }
    }

    let terminal: Vec<OperationStage> = STAGES.into_iter().filter(|s| s.is_terminal()).collect();
    assert_eq!(
        terminal,
        [OperationStage::Committed, OperationStage::Reverted]
    );

    let landed: Vec<OperationStage> = STAGES.into_iter().filter(|s| s.may_have_landed()).collect();
    assert_eq!(
        landed,
        [
            OperationStage::Signed,
            OperationStage::Submitted,
            OperationStage::Accepted,
            OperationStage::Committed,
            OperationStage::NeedsAttention,
        ]
    );
}

/// `OperationLease::advance` enforces exactly the table, and an illegal edge writes nothing.
///
/// Driven over the v4 fixtures, one per stage, so every `from` is a real on-disk record.
#[cfg(unix)]
#[test]
fn the_lease_enforces_the_table_on_real_records() {
    for (from, code) in STAGES.into_iter().zip(STAGE_FIXTURES) {
        for to in STAGES {
            let installed = install("edge", &[V4_93593ED]);
            let id = V4_93593ED.id(code);
            let path = installed.record_path(&id);
            let before = (inode(&path), std::fs::read(&path).expect("record"));

            let mut lease = installed.journal.lock(&id).expect("lock").expect("record");
            assert_eq!(lease.record().stage(), from);
            let result = lease.advance(to, 1_900_000_000);

            if from.can_advance_to(to) {
                result.unwrap_or_else(|error| panic!("{from:?} -> {to:?}: {error}"));
                let reread: Value =
                    serde_json::from_slice(&std::fs::read(&path).expect("record")).expect("json");
                assert_eq!(
                    reread["attempts"][0]["stage"],
                    serde_json::to_value(to).expect("stage")
                );
                assert_eq!(reread["attempts"][0]["updated_at"], 1_900_000_000);
            } else {
                assert!(
                    matches!(
                        result,
                        Err(JournalError::IllegalTransition { from: f, to: t }) if f == from && t == to
                    ),
                    "{from:?} -> {to:?} should be illegal"
                );
                assert_eq!(
                    (inode(&path), std::fs::read(&path).expect("record")),
                    before,
                    "{from:?} -> {to:?}: an illegal edge rewrote the record"
                );
            }
        }
    }
}

/// Rejected signing cannot create an orphan blob or replace bytes on a terminal/in-flight
/// attempt. The record's stored flag alone cannot establish that no file was written.
#[cfg(unix)]
#[test]
fn rejected_signing_preserves_every_file_across_historical_schemas() {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::path::Path;

    fn disk_snapshot(directory: &Path) -> BTreeMap<OsString, (u64, u32, Vec<u8>)> {
        std::fs::read_dir(directory)
            .expect("operations")
            .map(|entry| {
                let entry = entry.expect("entry");
                let path = entry.path();
                assert!(path.is_file(), "unexpected directory: {}", path.display());
                (
                    entry.file_name(),
                    (
                        inode(&path),
                        mode(&path),
                        std::fs::read(path).expect("file"),
                    ),
                )
            })
            .collect()
    }

    let mut rejected = 0;
    for set in SETS {
        for code in set.codes() {
            let installed = install_codes("rejected-signing", set, &[code]);
            let id = set.id(code);
            let mut lease = installed.journal.lock(&id).expect("lock").expect("record");
            let stage = lease.record().stage();
            if stage.can_advance_to(OperationStage::Signed) {
                continue;
            }
            let record_before = serde_json::to_value(lease.record()).expect("record");
            // Lock files have been created before this snapshot. Rejected signing must
            // leave even unreferenced blobs and temporary files exactly as they were.
            let disk_before = disk_snapshot(&installed.operations());
            for retry in 0..2 {
                assert!(
                    matches!(
                        lease.persist_signed(
                            Felt::from_hex_unchecked("0xdeadbeef"),
                            "forbidden replacement bytes",
                            1_900_000_000 + retry,
                        ),
                        Err(JournalError::IllegalTransition { from, to })
                            if from == stage && to == OperationStage::Signed
                    ),
                    "{} code {code:02x}: rejected signing returned another result",
                    set.dir
                );
                assert_eq!(
                    serde_json::to_value(lease.record()).expect("record"),
                    record_before,
                    "{} code {code:02x}: rejected signing mutated memory",
                    set.dir
                );
                assert_eq!(
                    disk_snapshot(&installed.operations()),
                    disk_before,
                    "{} code {code:02x}: rejected signing changed files",
                    set.dir
                );
            }
            drop(lease);
            let reopened = OperationJournal::new(&installed.root).expect("reopen journal");
            let lease = reopened.lock(&id).expect("lock").expect("record");
            assert_eq!(
                serde_json::to_value(lease.record()).expect("record"),
                record_before
            );
            assert_eq!(disk_snapshot(&installed.operations()), disk_before);
            rejected += 1;
        }
    }
    assert_eq!(
        rejected, 58,
        "all illegal signing fixtures must be exercised"
    );
}

#[test]
fn stage_and_operation_wire_names() {
    let stages: Vec<Value> = STAGES
        .into_iter()
        .map(|s| serde_json::to_value(s).expect("stage"))
        .collect();
    assert_eq!(
        Value::Array(stages),
        json!([
            "claimed",
            "prepared",
            "proven",
            "signed",
            "submitted",
            "accepted",
            "committed",
            "reverted",
            "needs_attention"
        ])
    );

    let operations = [
        WriteOperation::Shield,
        WriteOperation::ApprovePool,
        WriteOperation::OpenChannel,
        WriteOperation::ProposeOffer,
        WriteOperation::CounterOffer,
        WriteOperation::AcceptAndSettle,
    ];
    for operation in operations {
        // The serde name and the binding tag are the same string for every write.
        assert_eq!(
            serde_json::to_value(operation).expect("operation"),
            json!(operation.tag())
        );
    }
    let tags: Vec<&str> = operations.iter().map(|o| o.tag()).collect();
    assert_eq!(
        tags,
        [
            "shield",
            "approve_pool",
            "open_channel",
            "propose_offer",
            "counter_offer",
            "accept_and_settle"
        ]
    );
    assert_eq!(JOURNAL_VERSION, 4);
}

// ---- 2. Historical records load and classify --------------------------------------------

/// `(code, latest stage, attempts, record-level may_have_landed)` for every fixture.
fn expected_shape(set: FixtureSet, code: u8) -> (OperationStage, usize, bool) {
    use OperationStage as S;
    match code {
        CLAIMED | CLAIMED_WITHOUT_REQUEST => (S::Claimed, 1, false),
        PREPARED | PREPARED_WITHOUT_REQUEST => (S::Prepared, 1, false),
        PROVEN => (S::Proven, 1, false),
        SIGNED => (S::Signed, 1, true),
        SUBMITTED => (S::Submitted, 1, true),
        ACCEPTED => (S::Accepted, 1, true),
        COMMITTED => (S::Committed, 1, true),
        REVERTED => (S::Reverted, 1, false),
        NEEDS_ATTENTION => (S::NeedsAttention, 1, true),
        RESTARTED_IN_FLIGHT => (S::Submitted, 2, true),
        // Schema-v1 code before 448146e reopened a restarted operation at Prepared.
        RESTARTED_FRESH if set.era == 0x1a || set.era == 0x1b => (S::Prepared, 2, true),
        // CHARACTERIZATION: current behavior, flagged for review: the record says an effect may
        // exist (attempt 0 was submitted) while its stage says nothing was signed. Everything
        // that reads `stage()` sees only the fresh attempt. journal.rs:248-257.
        RESTARTED_FRESH => (S::Claimed, 2, true),
        // CHARACTERIZATION: current behavior, flagged for review: a Claimed -> Committed no-chain
        // result signed nothing, yet reports may_have_landed because the answer is per stage,
        // not per attempt facts. journal.rs:79-88.
        NOOP_COMMITTED => (S::Committed, 1, true),
        other => panic!("no fixture code {other:#x}"),
    }
}

#[test]
fn every_historical_record_loads_and_classifies() {
    for set in SETS {
        let installed = install("load", &[set]);
        let records = installed.sorted_records();
        let codes = set.codes();
        assert_eq!(records.len(), codes.len(), "{}", set.dir);

        for (record, code) in records.iter().zip(codes) {
            let label = format!("{} {code:#04x}", set.dir);
            let (stage, attempts, landed) = expected_shape(set, code);

            assert_eq!(record.operation_id, set.id(code), "{label}");
            assert_eq!(record.version, set.version, "{label}");
            assert_eq!(record.operation, op_for(code), "{label}");
            assert_eq!(record.binding, binding(code), "{label}: binding drifted");
            assert_eq!(record.stage(), stage, "{label}");
            assert_eq!(record.attempts.len(), attempts, "{label}");
            assert_eq!(record.may_have_landed(), landed, "{label}");
            assert_eq!(
                record.stage().is_terminal(),
                matches!(stage, OperationStage::Committed | OperationStage::Reverted),
                "{label}"
            );

            let expected_request = (set.version >= 2
                && !matches!(code, CLAIMED_WITHOUT_REQUEST | PREPARED_WITHOUT_REQUEST))
            .then(|| request(code));
            assert_eq!(record.request, expected_request, "{label}");

            let expected_channel = (code == ACCEPTED && set.era >= 0x1c)
                .then(|| ChannelHandle::parse(HANDLE).expect("handle"));
            assert_eq!(record.channel, expected_channel, "{label}");

            assert_eq!(
                record.result.is_some(),
                set.version >= 2 && matches!(code, COMMITTED | NOOP_COMMITTED),
                "{label}"
            );

            // Signed attempts name their transaction. Only b784f3f predates stored bytes.
            for (index, attempt) in record.attempts.iter().enumerate() {
                let signed = attempt.transaction_hash.is_some();
                assert_eq!(
                    attempt.transaction_stored,
                    signed && set.era != 0x1a,
                    "{label} attempt {index}"
                );
                if signed {
                    assert_eq!(
                        attempt.transaction_hash,
                        Some(set.tx(code, u8::try_from(index).expect("index"))),
                        "{label} attempt {index}"
                    );
                }
                assert_eq!(
                    attempt.account_nonce.is_some(),
                    signed && set.era >= 0x1c,
                    "{label} attempt {index}"
                );
                assert_eq!(
                    attempt.simulation_hash.is_some(),
                    set.version == 4 && attempt.proving_block.is_some(),
                    "{label} attempt {index}"
                );
                assert_eq!(
                    attempt.receipt.is_some(),
                    set.version >= 2 && matches!(code, ACCEPTED | COMMITTED | REVERTED),
                    "{label} attempt {index}"
                );
            }
        }
    }
}

/// `accepted_at` on disk: wall clock before schema 3, block timestamp from 3 on. The journal
/// does not interpret it; reconciliation does (see `reconcile_characterization.rs`).
#[test]
fn accepted_at_carries_whatever_the_writing_schema_meant() {
    let expected: [(FixtureSet, u64); 6] = [
        (V1_B784F3F, 1_787_443_229),
        (V1_7B7B4C8, 1_787_446_826),
        (V1_448146E, 1_787_450_429),
        (V2_B1FE0E3, 1_787_529_640),
        (V3_D3DAB59, 1_787_000_120),
        (V4_93593ED, 1_787_000_120),
    ];
    for (set, accepted_at) in expected {
        let installed = install("accepted-at", &[set]);
        let lease = installed
            .journal
            .lock(&set.id(ACCEPTED))
            .expect("lock")
            .expect("record");
        assert_eq!(
            lease.record().attempt().accepted_at,
            Some(accepted_at),
            "{}",
            set.dir
        );
    }
}

// ---- 3. Re-serialization -----------------------------------------------------------------

/// A v4 record re-encodes to the exact bytes 93593ed wrote.
#[test]
fn v4_records_reserialize_byte_for_byte() {
    let installed = install("v4-bytes", &[V4_93593ED]);
    for record in installed.sorted_records() {
        let code = u8::from_str_radix(&record.operation_id.as_str()[5..7], 16).expect("code");
        assert_eq!(
            String::from_utf8(serde_json::to_vec(&record).expect("encode")).expect("utf8"),
            String::from_utf8(V4_93593ED.record_bytes(code)).expect("utf8"),
            "{}",
            record.operation_id
        );
    }
}

/// What the current code writes for each legacy record: every current field, the original
/// `version` number unchanged. One line per record, sorted by id.
///
/// CHARACTERIZATION: current behavior, flagged for review: a v1-v3 record touched by current
/// code keeps its old version number but gains every v4 field (`request`, `channel`, `result`,
/// `simulation_hash`, `prepared`, `completion`, `receipt`, ...), so the number no longer
/// describes the shape. docs/metropolis-m6-decisions.md DM6-2 leaves "pin or migrate" open.
#[test]
fn legacy_records_reserialize_with_every_current_field_and_their_old_version() {
    for set in LEGACY_SETS {
        let installed = install("legacy-bytes", &[set]);
        let mut lines = String::new();
        for record in installed.sorted_records() {
            assert_eq!(record.version, set.version);
            lines.push_str(&serde_json::to_string(&record).expect("encode"));
            lines.push('\n');
        }
        assert_golden_bytes(&format!("reserialized/{}.jsonl", set.dir), lines.as_bytes());
    }
}

/// Every write path is `serde_json::to_vec(record)`: a no-op amend (same `updated_at`)
/// rewrites each record to exactly its re-serialization, which for v4 is the original bytes.
#[cfg(unix)]
#[test]
fn a_rewrite_writes_exactly_the_reserialized_record() {
    for set in SETS {
        let installed = install("rewrite", &[set]);
        for record in installed.sorted_records() {
            let path = installed.record_path(&record.operation_id);
            let before = inode(&path);
            let updated_at = record.attempt().updated_at;
            {
                let mut lease = installed
                    .journal
                    .lock(&record.operation_id)
                    .expect("lock")
                    .expect("record");
                lease.amend(updated_at, |_| {}).expect("amend");
            }
            assert_ne!(inode(&path), before, "amend writes by atomic rename");
            assert_eq!(
                std::fs::read(&path).expect("record"),
                serde_json::to_vec(&record).expect("encode"),
                "{}",
                record.operation_id
            );
            if set.version == 4 {
                let code =
                    u8::from_str_radix(&record.operation_id.as_str()[5..7], 16).expect("code");
                assert_eq!(
                    std::fs::read(&path).expect("record"),
                    set.record_bytes(code)
                );
            }
        }
    }
}

// ---- 4. Write-back after an advance ------------------------------------------------------

/// `advance` on a legacy record keeps its version; `record_result` bumps it to 4.
///
/// CHARACTERIZATION: current behavior, flagged for review: `record_result` sets
/// `version = JOURNAL_VERSION` (journal.rs:770) without touching `accepted_at`, so a v1/v2
/// record's wall-clock `accepted_at` becomes indistinguishable from a v3+ block timestamp.
/// `reconcile_characterization.rs` shows reconciliation then reports it as the block time.
#[test]
fn legacy_write_back_keeps_the_version_until_a_result_is_recorded() {
    for set in LEGACY_SETS {
        let installed = install("write-back", &[set]);

        {
            let mut lease = installed
                .journal
                .lock(&set.id(SUBMITTED))
                .expect("lock")
                .expect("record");
            lease
                .advance(OperationStage::NeedsAttention, 1_900_000_001)
                .expect("advance");
            assert_eq!(lease.record().version, set.version, "{}", set.dir);
        }
        assert_golden_bytes(
            &format!(
                "write-back/{}-submitted-advanced-to-needs_attention.json",
                set.dir
            ),
            &std::fs::read(installed.record_path(&set.id(SUBMITTED))).expect("record"),
        );

        {
            let mut lease = installed
                .journal
                .lock(&set.id(ACCEPTED))
                .expect("lock")
                .expect("record");
            lease
                .record_result(json!({"fixture": "result"}), 1_900_000_002)
                .expect("result");
            assert_eq!(lease.record().version, JOURNAL_VERSION, "{}", set.dir);
            lease
                .advance(OperationStage::Committed, 1_900_000_003)
                .expect("commit");
        }
        assert_golden_bytes(
            &format!(
                "write-back/{}-accepted-record_result-committed.json",
                set.dir
            ),
            &std::fs::read(installed.record_path(&set.id(ACCEPTED))).expect("record"),
        );
    }
}

/// `persist_signed` on a legacy record: the version is kept and the transaction lands at
/// `<id>.<attempt>.tx` like any other.
#[test]
fn persisting_a_signature_on_a_legacy_record_keeps_its_version() {
    let installed = install("legacy-sign", &[V1_B784F3F]);
    {
        let mut lease = installed
            .journal
            .lock(&V1_B784F3F.id(PROVEN))
            .expect("lock")
            .expect("record");
        lease
            .persist_signed(
                Felt::from_hex_unchecked("0xfeed"),
                "{\"fixture\":1}",
                1_900_000_004,
            )
            .expect("persist");
        assert_eq!(lease.record().version, 1);
        assert_eq!(
            lease.stored_transaction(0).expect("stored"),
            Some("{\"fixture\":1}".to_owned())
        );
    }
    assert_golden_bytes(
        "write-back/v1-b784f3f-proven-persist_signed.json",
        &std::fs::read(installed.record_path(&V1_B784F3F.id(PROVEN))).expect("record"),
    );
}

// ---- 5. Claims on existing records -------------------------------------------------------

/// A claim that neither backfills nor changes anything writes nothing on a legacy record,
/// and rewrites a v4 record to identical bytes. `lock` never writes.
#[cfg(unix)]
#[test]
fn a_claim_on_a_legacy_record_writes_nothing() {
    for set in SETS {
        let installed = install("claim", &[set]);
        for code in set.codes() {
            let id = set.id(code);
            let path = installed.record_path(&id);
            let original = std::fs::read(&path).expect("record");

            let before = inode(&path);
            drop(installed.journal.lock(&id).expect("lock").expect("record"));
            assert_eq!(inode(&path), before, "lock wrote {id}");

            let before = inode(&path);
            let lease = installed
                .journal
                .claim(&id, op_for(code), binding(code), None, 1_900_000_000)
                .unwrap_or_else(|error| panic!("{id}: {error}"));
            assert_eq!(lease.record().version, set.version);
            drop(lease);

            assert_eq!(std::fs::read(&path).expect("record"), original, "{id}");
            if set.version == JOURNAL_VERSION {
                // CHARACTERIZATION: current behavior, flagged for review: every reclaim of a
                // current-version record rewrites it, even when nothing changed.
                // journal.rs:402-404.
                assert_ne!(inode(&path), before, "{id}: v4 reclaim rewrites");
            } else {
                assert_eq!(inode(&path), before, "{id}: legacy reclaim wrote");
            }
        }
    }
}

/// The recorded channel is not part of the reclaim check: a different (or absent) channel
/// reopens the record and keeps the recorded one.
#[test]
fn a_reclaim_ignores_the_channel_argument() {
    let installed = install("claim-channel", &[V4_93593ED]);
    let other = ChannelHandle::parse(format!("ch_{}", "d5".repeat(32))).expect("handle");
    let lease = installed
        .journal
        .claim_with_request(
            &V4_93593ED.id(ACCEPTED),
            op_for(ACCEPTED),
            binding(ACCEPTED),
            Some(other),
            request(ACCEPTED),
            1_900_000_000,
        )
        .expect("reclaim");
    assert_eq!(
        lease.record().channel,
        Some(ChannelHandle::parse(HANDLE).expect("handle"))
    );
}

/// A request-less `claim` on a record that has a request does not conflict.
#[test]
fn a_request_less_claim_skips_the_request_check() {
    let installed = install("claim-no-request", &[V2_B1FE0E3]);
    let lease = installed
        .journal
        .claim(
            &V2_B1FE0E3.id(CLAIMED),
            op_for(CLAIMED),
            binding(CLAIMED),
            None,
            1_900_000_000,
        )
        .expect("reclaim without request");
    assert_eq!(lease.record().request, Some(request(CLAIMED)));
}

/// Request backfill happens only while the latest attempt is Claimed, and bumps the version.
#[cfg(unix)]
#[test]
fn request_backfill_happens_only_at_claimed() {
    // (set, code, backfilled?)
    let cases = [
        (V1_B784F3F, CLAIMED, true),
        (V1_B784F3F, PREPARED, false),
        (V1_B784F3F, SUBMITTED, false),
        // The fresh attempt of a restarted 448146e record is Claimed, so the whole record,
        // including its earlier submitted attempt, is bumped to v4.
        (V1_448146E, RESTARTED_FRESH, true),
        // b784f3f reopened restarts at Prepared, so no backfill.
        (V1_B784F3F, RESTARTED_FRESH, false),
        (V2_B1FE0E3, CLAIMED_WITHOUT_REQUEST, true),
        (V2_B1FE0E3, PREPARED_WITHOUT_REQUEST, false),
        (V3_D3DAB59, CLAIMED_WITHOUT_REQUEST, true),
        (V3_D3DAB59, PREPARED_WITHOUT_REQUEST, false),
        (V4_93593ED, CLAIMED_WITHOUT_REQUEST, true),
        (V4_93593ED, PREPARED_WITHOUT_REQUEST, false),
    ];
    for (set, code, backfilled) in cases {
        let installed = install("backfill", &[set]);
        let id = set.id(code);
        let path = installed.record_path(&id);
        let (original, before) = (std::fs::read(&path).expect("record"), inode(&path));
        let label = format!("{} {code:#04x}", set.dir);

        let lease = installed
            .journal
            .claim_with_request(
                &id,
                op_for(code),
                binding(code),
                None,
                request(code),
                1_900_000_000,
            )
            .unwrap_or_else(|error| panic!("{label}: {error}"));

        if backfilled {
            assert_eq!(lease.record().version, JOURNAL_VERSION, "{label}");
            assert_eq!(lease.record().request, Some(request(code)), "{label}");
            drop(lease);
            assert_golden_bytes(
                &format!("backfill/{}-{code:02x}.json", set.dir),
                &std::fs::read(&path).expect("record"),
            );
        } else {
            assert_eq!(lease.record().version, set.version, "{label}");
            assert_eq!(lease.record().request, None, "{label}");
            drop(lease);
            if set.version == JOURNAL_VERSION {
                assert_eq!(std::fs::read(&path).expect("record"), original, "{label}");
            } else {
                assert_eq!(
                    (inode(&path), std::fs::read(&path).expect("record")),
                    (before, original),
                    "{label}"
                );
            }
        }
    }
}

/// Reclaim checks run in this order: binding, operation, canonical request.
#[test]
fn reclaim_conflicts_are_checked_in_order() {
    let installed = install("conflicts", &[V2_B1FE0E3]);
    let id = V2_B1FE0E3.id(CLAIMED);

    let error = installed
        .journal
        .claim_with_request(
            &id,
            WriteOperation::ApprovePool,
            binding(CLAIMED + 1),
            None,
            json!({"different": true}),
            1,
        )
        .expect_err("binding");
    assert!(matches!(error, JournalError::BindingConflict { .. }));

    let error = installed
        .journal
        .claim_with_request(
            &id,
            WriteOperation::ApprovePool,
            binding(CLAIMED),
            None,
            json!({"different": true}),
            1,
        )
        .expect_err("operation");
    assert!(matches!(
        error,
        JournalError::OperationConflict {
            recorded: WriteOperation::Shield,
            received: WriteOperation::ApprovePool,
            ..
        }
    ));

    let error = installed
        .journal
        .claim_with_request(
            &id,
            op_for(CLAIMED),
            binding(CLAIMED),
            None,
            json!({"different": true}),
            1,
        )
        .expect_err("request");
    assert!(matches!(error, JournalError::RequestConflict { .. }));
}

// ---- 6. Stored transactions --------------------------------------------------------------

#[test]
fn stored_transactions_are_read_back_per_attempt() {
    for set in [V1_7B7B4C8, V1_448146E, V2_B1FE0E3, V3_D3DAB59, V4_93593ED] {
        let installed = install("stored", &[set]);

        let lease = installed
            .journal
            .lock(&set.id(RESTARTED_IN_FLIGHT))
            .expect("lock")
            .expect("record");
        for attempt in 0..2u8 {
            let expected = format!(
                r#"{{"type":"INVOKE","version":"0x3","sender_address":"0xacc","nonce":"{}","calldata":[],"signature":["0x1","0x2"],"proof":"fixture-{}-0a-{attempt}"}}"#,
                if attempt == 0 { "0x5" } else { "0x6" },
                set.commit
            );
            assert_eq!(
                lease
                    .stored_transaction(usize::from(attempt))
                    .expect("read"),
                Some(expected),
                "{} attempt {attempt}",
                set.dir
            );
        }
        assert_eq!(lease.stored_transaction(2).expect("out of range"), None);
        drop(lease);

        // The fresh attempt has no transaction yet.
        let lease = installed
            .journal
            .lock(&set.id(RESTARTED_FRESH))
            .expect("lock")
            .expect("record");
        assert!(lease.stored_transaction(0).expect("read").is_some());
        assert_eq!(lease.stored_transaction(1).expect("read"), None);
    }

    // b784f3f never stored bytes: a signed attempt with `transaction_stored: false` reads as
    // None, not as an error.
    let installed = install("stored-v1a", &[V1_B784F3F]);
    let lease = installed
        .journal
        .lock(&V1_B784F3F.id(SUBMITTED))
        .expect("lock")
        .expect("record");
    assert!(lease.record().attempt().transaction_hash.is_some());
    assert_eq!(lease.stored_transaction(0).expect("read"), None);
}

// ---- 7. RequestBinding known answers -----------------------------------------------------

/// Digests computed independently (Python `hashlib.sha256`) over the documented preimage:
/// `EREBUS_OPERATION_BINDING_V1 || chain(32 BE) || pool(32) || token(32) || u64 BE len(tag)
/// || tag || fields`, fixed-width fields big-endian, text length-prefixed with a u64.
#[test]
fn request_binding_known_answers() {
    let felt_max = Felt::from_hex_unchecked(
        "0x800000000000011000000000000000000000000000000000000000000000000",
    );
    let cases: [(RequestBinding, &str); 8] = [
        (
            RequestBinding::builder(WriteOperation::Shield, CHAIN, POOL, TOKEN).finish(),
            "115becf0984ab6bf9f91777e3714ec9c419733bbd709c44edbaa67099b12126d",
        ),
        (
            RequestBinding::builder(WriteOperation::Shield, CHAIN, POOL, TOKEN)
                .u128_be(1_000)
                .finish(),
            "ad960611a7989130f993a1ae3dff0b5f7f7b931799dbcae2df5327a25ed6ff91",
        ),
        (
            RequestBinding::builder(WriteOperation::ApprovePool, CHAIN, POOL, TOKEN)
                .u128_be(1_000)
                .finish(),
            "19f54b59d97465d81d512ac9027081bf796fb1eaed45fe007a69b4f3c5b9a3a9",
        ),
        (
            RequestBinding::builder(
                WriteOperation::OpenChannel,
                Felt::from(0x33u8),
                Felt::from(0x22u8),
                Felt::from(0x44u8),
            )
            .felt(Felt::from(0x11u8))
            .felt(Felt::from(0x55u8))
            .u64_be(3)
            .finish(),
            "19d005b51099e7c714c034d277a62f9ca52543750cdcedda0016ebd884e063f3",
        ),
        (
            RequestBinding::builder(WriteOperation::CounterOffer, CHAIN, POOL, TOKEN)
                .text("ab")
                .text("c")
                .finish(),
            "cffa690daa6fb3757163b800a94edcf50543084b98e4023f05ea02df81deb939",
        ),
        (
            RequestBinding::builder(WriteOperation::ProposeOffer, CHAIN, POOL, TOKEN)
                .u128_be(7)
                .u64_be(7)
                .finish(),
            "1aa8d6d2d48bf442c03c6f26b5314e91e8217eecf0748005a524162d0aba0c47",
        ),
        (
            RequestBinding::builder(WriteOperation::AcceptAndSettle, CHAIN, POOL, TOKEN)
                .felt(felt_max)
                .u128_be(u128::MAX)
                .u64_be(u64::MAX)
                .text(&format!("{HANDLE}:us:0"))
                .finish(),
            "b3cfc8c2405163c556567f5a15cf0a41e4bd2d28237b98d24e53c4c9736718bf",
        ),
        // Written by b784f3f into tests/fixtures/journal/v1-b784f3f; unchanged since.
        (
            binding(CLAIMED),
            "8940fcd3754b62ab0df6968bdce7f26b161dbd52ba8b848f9c8654b05670af8b",
        ),
    ];
    for (binding, expected) in cases {
        assert_eq!(binding.to_hex(), expected);
        assert_eq!(binding.to_string(), expected);
        assert_eq!(
            serde_json::to_value(binding).expect("binding"),
            json!(expected)
        );
    }
}

// ---- 8. Error text -----------------------------------------------------------------------

/// Every variant's `Display`, constructed directly so the text is platform-independent.
#[test]
fn journal_error_display_strings() {
    let id = fixture_id(0x40, 0x01);
    let other = fixture_id(0x40, 0x02);
    let path = std::path::PathBuf::from("/state/operations/record.json");
    let json_error = serde_json::from_str::<Value>("{ not json").expect_err("invalid");

    let cases: Vec<(JournalError, String)> = vec![
        (
            JournalError::Io {
                path: path.clone(),
                source: std::io::Error::other("disk on fire"),
            },
            "journal io error at /state/operations/record.json: disk on fire".to_owned(),
        ),
        (
            JournalError::Json {
                path: path.clone(),
                source: json_error,
            },
            "journal record at /state/operations/record.json is not readable: \
             key must be a string at line 1 column 3"
                .to_owned(),
        ),
        (
            JournalError::UnsupportedVersion(5),
            "journal record schema version 5 is not supported".to_owned(),
        ),
        (
            JournalError::Corrupt {
                path: path.clone(),
                reason: "record has no attempts",
            },
            "journal record at /state/operations/record.json is corrupt: record has no attempts"
                .to_owned(),
        ),
        (
            JournalError::IdMismatch {
                expected: id.clone(),
                found: other.clone(),
            },
            format!("journal record under {id} claims to be {other}"),
        ),
        (
            JournalError::BindingConflict {
                operation_id: id.clone(),
                recorded: binding(1),
                received: binding(2),
            },
            format!("operation {id} is already bound to a different request"),
        ),
        (
            JournalError::RequestConflict {
                operation_id: id.clone(),
            },
            format!("operation {id} is already bound to different canonical parameters"),
        ),
        (
            JournalError::OperationConflict {
                operation_id: id.clone(),
                recorded: WriteOperation::Shield,
                received: WriteOperation::ApprovePool,
            },
            format!("operation {id} records Shield, not ApprovePool"),
        ),
        (
            JournalError::ReceiptConflict {
                operation_id: id.clone(),
                reason: "a different receipt is already recorded".to_owned(),
            },
            format!(
                "operation {id} has a contradictory receipt: a different receipt is already recorded"
            ),
        ),
        (
            JournalError::ResultConflict {
                operation_id: id.clone(),
            },
            format!("operation {id} already has a different final result"),
        ),
        (
            JournalError::IllegalTransition {
                from: OperationStage::NeedsAttention,
                to: OperationStage::Claimed,
            },
            "operation cannot move from NeedsAttention to Claimed".to_owned(),
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(error.to_string(), expected);
    }
}

/// The same variants reached through real journal behaviour, with the text they carry.
#[test]
fn journal_errors_raised_by_real_records() {
    let installed = install("errors", &[V2_B1FE0E3]);
    let operations = installed.operations();
    let id = V2_B1FE0E3.id(ACCEPTED);

    // Receipts: not a felt, wrong hash, a different receipt already recorded.
    {
        let mut lease = installed.journal.lock(&id).expect("lock").expect("record");
        let recorded = lease.record().attempt().receipt.clone().expect("receipt");
        let reasons: Vec<String> = [
            Receipt {
                transaction_hash: "not-a-felt".to_owned(),
                ..recorded.clone()
            },
            Receipt {
                transaction_hash: "0x1".to_owned(),
                ..recorded.clone()
            },
            Receipt {
                block_number: Some(999),
                ..recorded
            },
        ]
        .into_iter()
        .map(|receipt| {
            lease
                .record_receipt(receipt, 1)
                .expect_err("conflict")
                .to_string()
        })
        .collect();
        assert_eq!(
            reasons,
            [
                format!(
                    "operation {id} has a contradictory receipt: receipt transaction hash is not a felt"
                ),
                format!(
                    "operation {id} has a contradictory receipt: receipt transaction hash does \
                     not match the signed transaction"
                ),
                format!(
                    "operation {id} has a contradictory receipt: a different receipt is already recorded"
                ),
            ]
        );
        // Recording the identical receipt again is accepted.
        let same = lease.record().attempt().receipt.clone().expect("receipt");
        lease.record_receipt(same, 1).expect("idempotent receipt");
    }

    // A different result on a committed record.
    {
        let committed = V2_B1FE0E3.id(COMMITTED);
        let mut lease = installed
            .journal
            .lock(&committed)
            .expect("lock")
            .expect("record");
        assert!(matches!(
            lease.record_result(json!("other"), 1),
            Err(JournalError::ResultConflict { .. })
        ));
        let same = lease.record().result.clone().expect("result");
        lease.record_result(same, 1).expect("idempotent result");
        assert!(matches!(
            lease.advance(OperationStage::Submitted, 1),
            Err(JournalError::IllegalTransition {
                from: OperationStage::Committed,
                to: OperationStage::Submitted
            })
        ));
    }

    // A record claiming a stored transaction whose file is gone.
    {
        let submitted = V2_B1FE0E3.id(SUBMITTED);
        let tx = operations.join(format!("{}.0.tx", submitted.as_str()));
        std::fs::remove_file(&tx).expect("remove tx");
        let lease = installed
            .journal
            .lock(&submitted)
            .expect("lock")
            .expect("record");
        assert_eq!(
            lease.stored_transaction(0).expect_err("missing").to_string(),
            format!(
                "journal record at {} is corrupt: record claims a stored transaction that is missing",
                tx.display()
            )
        );
    }

    // Unsupported versions, both sides of the readable range.
    for version in [0u32, 5] {
        let installed = install("errors-version", &[V2_B1FE0E3]);
        let id = V2_B1FE0E3.id(CLAIMED);
        let path = installed.record_path(&id);
        let text = String::from_utf8(V2_B1FE0E3.record_bytes(CLAIMED)).expect("utf8");
        std::fs::write(
            &path,
            text.replace("\"version\":2", &format!("\"version\":{version}")),
        )
        .expect("write");
        assert_eq!(
            installed
                .journal
                .lock(&id)
                .expect_err("version")
                .to_string(),
            format!("journal record schema version {version} is not supported")
        );
        assert!(matches!(
            installed.journal.records(),
            Err(JournalError::UnsupportedVersion(v)) if v == version
        ));
    }

    // No attempts.
    {
        let installed = install("errors-attempts", &[V2_B1FE0E3]);
        let id = V2_B1FE0E3.id(CLAIMED);
        let path = installed.record_path(&id);
        let mut value: Value =
            serde_json::from_slice(&V2_B1FE0E3.record_bytes(CLAIMED)).expect("json");
        value["attempts"] = json!([]);
        std::fs::write(&path, serde_json::to_vec(&value).expect("encode")).expect("write");
        assert_eq!(
            installed.journal.lock(&id).expect_err("empty").to_string(),
            format!(
                "journal record at {} is corrupt: record has no attempts",
                path.display()
            )
        );
    }

    // A record filed under another id, and a `.json` file that is not an operation id.
    {
        let installed = install("errors-names", &[V2_B1FE0E3]);
        let (a, b) = (V2_B1FE0E3.id(CLAIMED), V2_B1FE0E3.id(PREPARED));
        std::fs::copy(installed.record_path(&a), installed.record_path(&b)).expect("copy");
        assert_eq!(
            installed
                .journal
                .lock(&b)
                .expect_err("mismatch")
                .to_string(),
            format!("journal record under {b} claims to be {a}")
        );
    }
    {
        let installed = install("errors-stray", &[V2_B1FE0E3]);
        let stray = installed.operations().join("notes.json");
        std::fs::copy(installed.record_path(&V2_B1FE0E3.id(CLAIMED)), &stray).expect("copy");
        // CHARACTERIZATION: current behavior, flagged for review: one stray `.json` file in
        // `operations/` fails every listing, so `reconcile` and every write refuse to run.
        // Intentional fail-closed per journal.rs:544-546, pinned here for the refactor.
        assert_eq!(
            installed.journal.records().expect_err("stray").to_string(),
            format!(
                "journal record at {} is corrupt: record filename is not a valid operation id",
                stray.display()
            )
        );
    }

    // An unparseable record.
    {
        let installed = install("errors-json", &[V2_B1FE0E3]);
        let id = V2_B1FE0E3.id(CLAIMED);
        let path = installed.record_path(&id);
        std::fs::write(&path, b"{ not json").expect("write");
        assert_eq!(
            installed.journal.lock(&id).expect_err("json").to_string(),
            format!(
                "journal record at {} is not readable: key must be a string at line 1 column 3",
                path.display()
            )
        );
    }

    // `operations` exists as a file.
    {
        let root = temporary_root("errors-io");
        std::fs::create_dir_all(&root).expect("root");
        std::fs::write(root.join("operations"), b"").expect("file");
        let error = OperationJournal::new(&root).expect_err("io");
        let JournalError::Io { path, .. } = &error else {
            panic!("expected Io, got {error:?}");
        };
        assert_eq!(path, &root.join("operations"));
        assert!(error.to_string().starts_with(&format!(
            "journal io error at {}: ",
            root.join("operations").display()
        )));
        let _ = std::fs::remove_dir_all(&root);
    }
}

// ---- 9. File layout and modes ------------------------------------------------------------

#[cfg(unix)]
#[test]
fn file_names_and_modes() {
    let root = temporary_root("layout");
    let journal = OperationJournal::new(&root).expect("journal");
    let operations = root.join("operations");
    assert_eq!(mode(&operations), 0o700);

    let id = fixture_id(0x77, 0x01);
    {
        let mut lease = journal
            .claim_with_request(
                &id,
                WriteOperation::Shield,
                binding(CLAIMED),
                None,
                request(CLAIMED),
                1_000,
            )
            .expect("claim");
        lease
            .record_prepared(
                PreparedSnapshot {
                    deposit: "1".to_owned(),
                    proof_validity_blocks: 450,
                    fee_per_write: "2".to_owned(),
                    allowance: "3".to_owned(),
                    public_balance: "4".to_owned(),
                },
                1_001,
            )
            .expect("prepared");
        lease
            .advance(OperationStage::Prepared, 1_002)
            .expect("prepared");
        lease
            .advance(OperationStage::Proven, 1_003)
            .expect("proven");
        lease
            .persist_signed(Felt::from_hex_unchecked("0xbeef"), "tx-bytes", 1_004)
            .expect("signed");
    }

    let mut names: Vec<String> = std::fs::read_dir(&operations)
        .expect("list")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .into_string()
                .expect("utf8")
        })
        .collect();
    names.sort();
    let stem = id.as_str();
    assert_eq!(
        names,
        [
            ".identity.lock".to_owned(),
            format!("{stem}.0.tx"),
            format!("{stem}.json"),
            format!("{stem}.lock"),
        ],
        "no temporary files are left behind"
    );
    for name in &names {
        assert_eq!(mode(&operations.join(name)), 0o600, "{name}");
    }
    assert_eq!(
        std::fs::read_to_string(operations.join(format!("{stem}.0.tx"))).expect("tx"),
        "tx-bytes"
    );

    // The directory is re-tightened on every open, and lock files on every lock.
    set_mode(&operations, 0o755);
    set_mode(&operations.join(format!("{stem}.lock")), 0o644);
    set_mode(&operations.join(".identity.lock"), 0o644);
    let journal = OperationJournal::new(&root).expect("reopen");
    assert_eq!(mode(&operations), 0o700);
    drop(journal.lock(&id).expect("lock").expect("record"));
    assert_eq!(mode(&operations.join(format!("{stem}.lock"))), 0o600);
    assert_eq!(mode(&operations.join(".identity.lock")), 0o600);

    let _ = std::fs::remove_dir_all(&root);
}

/// A record file's mode is only fixed when it is rewritten.
#[cfg(unix)]
#[test]
fn a_loose_legacy_record_mode_survives_until_the_record_is_rewritten() {
    let installed = install("modes", &[V1_448146E]);
    let id = V1_448146E.id(SUBMITTED);
    let path = installed.record_path(&id);
    set_mode(&path, 0o644);

    drop(
        installed
            .journal
            .claim(&id, op_for(SUBMITTED), binding(SUBMITTED), None, 1)
            .expect("claim"),
    );
    assert_eq!(
        mode(&path),
        0o644,
        "a claim that writes nothing leaves the mode"
    );
    assert_eq!(installed.journal.records().expect("records").len(), 11);

    installed
        .journal
        .lock(&id)
        .expect("lock")
        .expect("record")
        .advance(OperationStage::NeedsAttention, 2)
        .expect("advance");
    assert_eq!(mode(&path), 0o600, "the atomic rewrite creates a 0600 file");
}

/// Only `*.json` files are records; locks, stored transactions and stray temporaries are not.
#[test]
fn records_lists_only_json_files() {
    let installed = install("listing", &[V4_93593ED]);
    let operations = installed.operations();
    std::fs::write(operations.join(".12345.00000000deadbeef.tmp"), b"{").expect("tmp");
    std::fs::write(operations.join("README"), b"not a record").expect("readme");
    std::fs::write(operations.join("stray.tx"), b"{").expect("tx");
    assert_eq!(installed.sorted_records().len(), V4_93593ED.codes().len());
}

#[test]
fn an_exclusive_snapshot_holds_the_identity_lock_and_sees_every_record() {
    let installed = install("snapshot", &SETS);
    let snapshot = installed.journal.exclusive_snapshot().expect("snapshot");
    assert_eq!(
        snapshot.records().len(),
        SETS.iter().map(|set| set.codes().len()).sum::<usize>()
    );
    let identity = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(installed.operations().join(".identity.lock"))
        .expect("identity lock");
    assert!(identity.try_lock_exclusive().is_err());
    drop(snapshot);
    identity.try_lock_exclusive().expect("released");
}

// ---- 10. Prune over historical records ---------------------------------------------------

/// Prune removes exactly the records whose *latest* attempt is terminal, with every stored
/// transaction for every attempt, and creates no lock file for what it keeps.
#[test]
fn prune_over_every_historical_record() {
    let installed = install("prune", &SETS);
    let report = installed.journal.prune(0, u64::MAX).expect("prune");
    assert_eq!(report.pruned, 15);
    assert_eq!(report.retained_unfinished, 60);
    assert_eq!(report.retained_recent, 0);
    assert_eq!(report.retained_locked, 0);
    assert_eq!(report.examined(), 75);
    assert_eq!(
        serde_json::to_value(report).expect("report"),
        json!({"pruned":15,"retained_unfinished":60,"retained_recent":0,"retained_locked":0})
    );

    let mut remaining: Vec<String> = std::fs::read_dir(installed.operations())
        .expect("list")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .into_string()
                .expect("utf8")
        })
        .collect();
    remaining.sort();
    let mut expected = vec![".identity.lock".to_owned()];
    for set in SETS {
        for code in set.codes() {
            if matches!(code, COMMITTED | REVERTED | NOOP_COMMITTED) {
                continue;
            }
            let id = set.id(code);
            expected.push(format!("{}.json", id.as_str()));
            let attempts = match code {
                RESTARTED_IN_FLIGHT => 2,
                SIGNED | SUBMITTED | ACCEPTED | NEEDS_ATTENTION | RESTARTED_FRESH => 1,
                _ => 0,
            };
            if set.era != 0x1a {
                for index in 0..attempts {
                    expected.push(format!("{}.{index}.tx", id.as_str()));
                }
            }
        }
    }
    expected.sort();
    assert_eq!(remaining, expected);
}

/// Pruning keys on the latest attempt's `updated_at`, not on `created_at`.
#[test]
fn prune_age_is_measured_from_the_latest_attempt() {
    let installed = install("prune-age", &[V4_93593ED]);
    // 0x07 committed: created 1788220842, last updated 1788220855.
    let report = installed
        .journal
        .prune(10, 1_788_220_855 + 9)
        .expect("prune");
    assert_eq!((report.pruned, report.retained_recent), (0, 3));
    let report = installed
        .journal
        .prune(10, 1_788_220_908 + 10)
        .expect("prune");
    assert_eq!((report.pruned, report.retained_recent), (3, 0));
}

#[test]
fn operation_ids_used_by_the_fixtures_round_trip() {
    for set in SETS {
        for code in set.codes() {
            let id = set.id(code);
            assert_eq!(id.as_str().parse::<OperationId>().expect("parse"), id);
        }
    }
}
