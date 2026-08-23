//! Explicit, operator-driven recovery of a half-finished write.
//!
//! Nothing here runs automatically. Startup only classifies ([`crate::reconcile`]); acting
//! on that classification happens when someone asks for it, one operation at a time.
//!
//! ## Why resubmitting the same bytes is safe
//!
//! The two recovery paths look similar and are not. Resubmitting the *exact* recorded
//! transaction is safe for a reason that has nothing to do with proving it never landed: the
//! transaction hash is a function of the transaction, so a duplicate is the same transaction.
//! A node that already has it rejects or ignores the duplicate, and a node that does not have
//! it includes it once. Either way the chain ends up with one effect.
//!
//! Rebuilding is the dangerous path, because a rebuilt transaction is a *different*
//! transaction that produces the *same* effect. Nothing about its hash prevents it from
//! landing beside the original. So rebuilding is only ever offered once the original has been
//! proven dead, and this module will not do the rebuilding itself — it says the request must
//! be re-issued, and leaves that to the caller that still holds the parameters.

use serde_json::Value;
use starknet_types_core::felt::Felt;

use crate::journal::{OperationLease, OperationStage};
use crate::reconcile::{NextAction, Outcome};

/// What a resume did, or why it did nothing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ResumeOutcome {
    /// The operation already has its effect. Nothing was submitted.
    AlreadyComplete {
        /// Hash of the transaction that produced it, when one is recorded.
        transaction_hash: Option<Felt>,
    },
    /// Local state is behind a chain effect that already exists.
    ///
    /// Nothing is resubmitted: the write happened, and what is missing is the local record
    /// of it. Reissuing the request here would pay twice for one effect.
    LocalStateBehind {
        /// The accepted transaction.
        transaction_hash: Felt,
    },
    /// The recorded transaction was resubmitted unchanged.
    Resubmitted {
        /// Its hash, which resubmission cannot change.
        transaction_hash: Felt,
    },
    /// Proven dead. The original request may be re-issued under the same operation id.
    RebuildRequired {
        /// Why the recorded transaction can no longer land.
        reason: String,
    },
    /// Could not be established. Nothing was submitted and nothing may be rebuilt.
    ReconciliationRequired {
        /// What could not be established.
        reason: String,
    },
}

/// What a resume should do next, decided without touching the chain.
///
/// Split out from the work so the decision can be tested exhaustively: every combination of
/// stage, chain outcome and proof expiry resolves here, with no I/O involved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumePlan {
    /// Return without submitting.
    Report(ResumeOutcome),
    /// Resubmit the stored transaction for this attempt index.
    Resubmit {
        /// Which attempt holds the transaction.
        attempt_index: usize,
        /// Its recorded hash.
        transaction_hash: Felt,
    },
}

/// Decides what to do with one classified operation.
///
/// `head` is the current block. `outcome` and `next_action` come from
/// [`crate::reconcile`], which has already read the receipt and the account nonce.
pub fn plan(
    lease: &OperationLease,
    outcome: Outcome,
    next_action: NextAction,
    reason: &str,
    head: u64,
) -> ResumePlan {
    let record = lease.record();
    let attempt = record.attempt();

    match outcome {
        // The effect exists. Whether anything remains to do is a local question.
        Outcome::Effect => {
            return ResumePlan::Report(match next_action {
                NextAction::CommitLocalState => match attempt.transaction_hash {
                    Some(transaction_hash) => ResumeOutcome::LocalStateBehind { transaction_hash },
                    // Accepted with no hash is a contradiction the journal should not be
                    // able to produce, and guessing which way it goes is exactly what this
                    // module refuses to do.
                    None => ResumeOutcome::ReconciliationRequired {
                        reason: "the chain accepted this but no transaction hash is recorded"
                            .to_owned(),
                    },
                },
                _ => ResumeOutcome::AlreadyComplete {
                    transaction_hash: attempt.transaction_hash,
                },
            });
        }
        // Both mean no effect exists, so the request may be re-issued. Neither means the
        // recorded transaction can be resubmitted: a reverted transaction would revert
        // again, and a nonce-expired one can never be included.
        Outcome::Reverted => {
            return ResumePlan::Report(ResumeOutcome::RebuildRequired {
                reason: reason.to_owned(),
            })
        }
        Outcome::Unknown => {
            return ResumePlan::Report(ResumeOutcome::ReconciliationRequired {
                reason: reason.to_owned(),
            })
        }
        Outcome::NoEffect | Outcome::Pending => {}
    }

    // Nothing was ever signed, so there is nothing to resubmit and nothing to prove dead.
    if matches!(
        record.stage(),
        OperationStage::Claimed | OperationStage::Prepared | OperationStage::Proven
    ) {
        return ResumePlan::Report(ResumeOutcome::RebuildRequired {
            reason: "no transaction was ever signed for this operation".to_owned(),
        });
    }

    let Some(transaction_hash) = attempt.transaction_hash else {
        return ResumePlan::Report(ResumeOutcome::ReconciliationRequired {
            reason: "the record reached a signed stage without recording a transaction hash"
                .to_owned(),
        });
    };
    if !attempt.transaction_stored {
        return ResumePlan::Report(ResumeOutcome::ReconciliationRequired {
            reason: format!(
                "transaction {transaction_hash:#x} was signed but its bytes were not recorded, \
                 so it cannot be resubmitted unchanged"
            ),
        });
    }

    // `NoEffect` here means the account moved past the signed nonce, which kills this exact
    // transaction. `Pending` means it is still includable. Only the second can be resubmitted.
    if outcome == Outcome::NoEffect {
        return ResumePlan::Report(ResumeOutcome::RebuildRequired {
            reason: reason.to_owned(),
        });
    }

    // A proof-carrying transaction stops being includable when its window closes, even
    // though its nonce is untouched. An approve carries no proof and no window.
    if let Some(valid_until) = attempt.valid_until_block {
        if head > valid_until {
            return ResumePlan::Report(ResumeOutcome::RebuildRequired {
                reason: format!(
                    "the proof for transaction {transaction_hash:#x} was valid to block \
                     {valid_until} and the head is {head}, so it can no longer be included"
                ),
            });
        }
    }

    ResumePlan::Resubmit {
        attempt_index: record.attempts.len() - 1,
        transaction_hash,
    }
}

/// Parses a stored transaction back into the value the RPC expects.
pub fn stored_wire_transaction(stored: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(stored)
}
