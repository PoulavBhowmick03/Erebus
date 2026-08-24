//! What a resume decides to do, and — mostly — what it refuses to do.
//!
//! Resubmitting the recorded transaction is safe because its hash cannot change, so a
//! duplicate is the same transaction. Rebuilding is not safe in the same way: a rebuilt
//! transaction has its own hash and can land beside the original. Almost every test here
//! pins the line between those two.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use erebus_sdk::journal::{OperationJournal, OperationLease, OperationStage};
use erebus_sdk::operation::{OperationId, RequestBinding, WriteOperation};
use erebus_sdk::reconcile::{NextAction, Outcome};
use erebus_sdk::resume::{plan, ResumeOutcome, ResumePlan};
use starknet_types_core::felt::Felt;

const CHAIN: Felt = Felt::from_hex_unchecked("0x534e5f5345504f4c4941");
const POOL: Felt = Felt::from_hex_unchecked("0x4e4f");
const TOKEN: Felt = Felt::from_hex_unchecked("0x53545f");
const TX: Felt = Felt::from_hex_unchecked("0xbeef");

/// A lease at `stage`, with a stored transaction whose proof is valid to block 500.
fn lease_at(stage: OperationStage) -> (PathBuf, OperationLease) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "erebus-resume-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let journal = OperationJournal::new(&root).expect("journal");
    let id = OperationId::parse(format!("op_{}", "3c".repeat(32))).expect("id");
    let binding = RequestBinding::builder(WriteOperation::Shield, CHAIN, POOL, TOKEN)
        .u128_be(1)
        .finish();
    let mut lease = journal
        .claim(&id, WriteOperation::Shield, binding, None, 1_000)
        .expect("claim");

    let path: &[OperationStage] = match stage {
        OperationStage::Claimed => &[],
        OperationStage::Prepared => &[OperationStage::Prepared],
        OperationStage::Proven => &[OperationStage::Prepared, OperationStage::Proven],
        OperationStage::Signed | OperationStage::NeedsAttention => {
            &[OperationStage::Prepared, OperationStage::Proven]
        }
        _ => &[OperationStage::Prepared, OperationStage::Proven],
    };
    for step in path {
        lease.advance(*step, 1_001).expect("advance");
    }
    if !matches!(
        stage,
        OperationStage::Claimed | OperationStage::Prepared | OperationStage::Proven
    ) {
        lease.persist_signed(TX, "{}", 1_002).expect("persist");
        lease
            .amend(1_002, |attempt| {
                attempt.valid_until_block = Some(500);
                attempt.account_nonce = Some(Felt::from(5u8));
            })
            .expect("amend");
    }
    for step in match stage {
        OperationStage::Submitted => &[OperationStage::Submitted][..],
        OperationStage::Accepted => &[OperationStage::Submitted, OperationStage::Accepted][..],
        OperationStage::Committed => &[
            OperationStage::Submitted,
            OperationStage::Accepted,
            OperationStage::Committed,
        ][..],
        OperationStage::Reverted => &[OperationStage::Submitted, OperationStage::Reverted][..],
        OperationStage::NeedsAttention => &[OperationStage::NeedsAttention][..],
        _ => &[][..],
    } {
        lease.advance(*step, 1_003).expect("advance");
    }
    assert_eq!(lease.record().stage(), stage);
    (root, lease)
}

#[test]
fn a_transaction_that_may_still_land_is_resubmitted_unchanged() {
    let (_root, lease) = lease_at(OperationStage::Submitted);

    let decision = plan(
        &lease,
        Outcome::Pending,
        NextAction::Wait,
        "still pending",
        100,
    );

    assert_eq!(
        decision,
        ResumePlan::Resubmit {
            attempt_index: 0,
            transaction_hash: TX
        },
        "a duplicate of a transaction the chain may already have is the same transaction"
    );
}

#[test]
fn a_transaction_the_nonce_has_passed_is_never_resubmitted() {
    // `NoEffect` here comes from the account moving past the signed nonce, which kills this
    // exact transaction. Resubmitting it would simply fail; the request has to be rebuilt.
    let (_root, lease) = lease_at(OperationStage::Submitted);

    let decision = plan(
        &lease,
        Outcome::NoEffect,
        NextAction::SafeToRetry,
        "nonce moved on",
        100,
    );

    assert!(matches!(
        decision,
        ResumePlan::Report(ResumeOutcome::RebuildRequired { .. })
    ));
}

#[test]
fn an_expired_proof_is_rebuilt_rather_than_resubmitted() {
    // The nonce is untouched, so the transaction is still *includable* — but its proof is
    // past its window, so including it would revert. Resubmitting would burn gas to fail.
    let (_root, lease) = lease_at(OperationStage::Submitted);

    let decision = plan(
        &lease,
        Outcome::Pending,
        NextAction::Wait,
        "still pending",
        501,
    );

    let ResumePlan::Report(ResumeOutcome::RebuildRequired { reason }) = decision else {
        panic!("an expired proof must not be resubmitted");
    };
    assert!(
        reason.contains("500"),
        "the reason names the window: {reason}"
    );
}

#[test]
fn the_block_the_proof_expires_on_is_still_usable() {
    // Boundary: valid *to* block 500 means 500 is fine and 501 is not.
    let (_root, lease) = lease_at(OperationStage::Submitted);

    assert!(matches!(
        plan(&lease, Outcome::Pending, NextAction::Wait, "", 500),
        ResumePlan::Resubmit { .. }
    ));
}

#[test]
fn an_effect_that_already_exists_is_never_resubmitted() {
    let (_root, lease) = lease_at(OperationStage::Committed);

    assert_eq!(
        plan(&lease, Outcome::Effect, NextAction::None, "done", 100),
        ResumePlan::Report(ResumeOutcome::AlreadyComplete {
            transaction_hash: Some(TX)
        })
    );
}

#[test]
fn an_accepted_write_reports_local_state_behind_rather_than_resubmitting() {
    // The chain has the effect and the local record does not. Re-issuing here would pay
    // twice for one effect, which is the exact failure this milestone exists to prevent.
    let (_root, lease) = lease_at(OperationStage::Accepted);

    assert_eq!(
        plan(
            &lease,
            Outcome::Effect,
            NextAction::CommitLocalState,
            "accepted",
            100
        ),
        ResumePlan::Report(ResumeOutcome::LocalStateBehind {
            transaction_hash: TX
        })
    );
}

#[test]
fn an_effect_with_conflicting_local_state_requires_an_operator() {
    let (_root, lease) = lease_at(OperationStage::Accepted);

    assert!(matches!(
        plan(
            &lease,
            Outcome::Effect,
            NextAction::OperatorAttention,
            "the local channel conflicts with the accepted effect",
            100
        ),
        ResumePlan::Report(ResumeOutcome::ReconciliationRequired { .. })
    ));
}

#[test]
fn a_reverted_transaction_is_rebuilt_not_resubmitted() {
    let (_root, lease) = lease_at(OperationStage::Reverted);

    assert!(matches!(
        plan(
            &lease,
            Outcome::Reverted,
            NextAction::SafeToRetry,
            "reverted",
            100
        ),
        ResumePlan::Report(ResumeOutcome::RebuildRequired { .. })
    ));
}

#[test]
fn an_unknown_outcome_does_nothing_at_all() {
    let (_root, lease) = lease_at(OperationStage::Submitted);

    assert!(
        matches!(
            plan(
                &lease,
                Outcome::Unknown,
                NextAction::OperatorAttention,
                "cannot tell",
                100
            ),
            ResumePlan::Report(ResumeOutcome::ReconciliationRequired { .. })
        ),
        "ambiguity must not be resolved by acting"
    );
}

#[test]
fn an_operation_that_never_signed_anything_is_rebuilt() {
    for stage in [
        OperationStage::Claimed,
        OperationStage::Prepared,
        OperationStage::Proven,
    ] {
        let (_root, lease) = lease_at(stage);

        assert!(
            matches!(
                plan(
                    &lease,
                    Outcome::NoEffect,
                    NextAction::SafeToRetry,
                    "nothing signed",
                    100
                ),
                ResumePlan::Report(ResumeOutcome::RebuildRequired { .. })
            ),
            "{stage:?} has nothing to resubmit"
        );
    }
}

#[test]
fn a_write_with_no_proof_window_does_not_expire() {
    // An approve carries no proof, so no block height makes it unusable. Only its nonce can.
    let (_root, mut lease) = lease_at(OperationStage::Submitted);
    lease
        .amend(1_004, |attempt| attempt.valid_until_block = None)
        .expect("amend");

    assert!(matches!(
        plan(&lease, Outcome::Pending, NextAction::Wait, "", 999_999),
        ResumePlan::Resubmit { .. }
    ));
}

#[test]
fn a_signed_transaction_whose_bytes_were_lost_cannot_be_resubmitted() {
    let (_root, mut lease) = lease_at(OperationStage::Submitted);
    lease
        .amend(1_004, |attempt| attempt.transaction_stored = false)
        .expect("amend");

    assert!(
        matches!(
            plan(&lease, Outcome::Pending, NextAction::Wait, "", 100),
            ResumePlan::Report(ResumeOutcome::ReconciliationRequired { .. })
        ),
        "without the exact bytes any resubmission would be a different transaction"
    );
}
