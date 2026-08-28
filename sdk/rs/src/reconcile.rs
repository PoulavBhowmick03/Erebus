//! Read-only classification of what each journalled operation actually did.
//!
//! This runs at startup, before anything else touches the chain. It answers one question per
//! operation — did this produce an effect? — and nothing else. It never submits, never
//! resubmits, and never writes to the journal. Acting on its answer is
//! `resume_operation`'s job, and that is explicit and operator-driven.
//!
//! The bias throughout is that "I could not tell" and "it did not happen" are different
//! answers, and only one of them is safe to act on. Anything that cannot be established
//! from a receipt or from the account nonce comes back as [`Outcome::Unknown`].

use starknet_types_core::felt::Felt;

use crate::journal::{Attempt, OperationRecord, OperationStage};
use crate::operation::{OperationId, WriteOperation};
use crate::prover::BlockId;
use crate::rpc::{RpcError, StarknetRpc};
use crate::state::ChannelHandle;

/// What the chain says about one operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Established that no transaction from this operation was included.
    ///
    /// Either nothing was ever signed, or a signed transaction is missing from the chain
    /// while the account has already moved past the nonce it was signed against.
    NoEffect,
    /// A transaction was included and succeeded.
    Effect,
    /// A transaction was included and reverted, so no effect exists.
    Reverted,
    /// A signed transaction is not on the chain yet and could still be included.
    Pending,
    /// Could not be established. Never read this as "no effect".
    Unknown,
}

/// What an operator or caller may do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NextAction {
    /// Finished. Nothing to do.
    None,
    /// Safe to retry under a fresh operation id, or to abandon.
    SafeToRetry,
    /// The chain effect exists but local state does not reflect it yet.
    CommitLocalState,
    /// Local state reflects the effect but the journal/result is not finalized.
    CommitJournal,
    /// Wait: the transaction may still be included.
    Wait,
    /// A person has to look at this.
    OperatorAttention,
}

/// One operation's classification.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Finding {
    /// The operation this describes.
    pub operation_id: OperationId,
    /// Which write it was.
    pub operation: WriteOperation,
    /// Canonical replay request recorded before execution. It contains no key material.
    pub request: Option<serde_json::Value>,
    /// Stage the journal last recorded.
    pub stage: OperationStage,
    /// Channel it belongs to, when it has one.
    pub channel: Option<ChannelHandle>,
    /// Hash of the latest attempt's transaction, when one was signed.
    pub transaction_hash: Option<Felt>,
    /// Unix timestamp of the accepted Starknet block. Present only when an effect and its
    /// acceptance block can be established from authoritative chain data.
    pub accepted_at: Option<u64>,
    /// What the chain says.
    pub outcome: Outcome,
    /// What may be done about it.
    pub next_action: NextAction,
    /// Why this classification, in one sentence an operator can act on.
    pub reason: String,
}

impl Finding {
    /// Whether this operation needs a person.
    pub fn needs_attention(&self) -> bool {
        self.next_action == NextAction::OperatorAttention
    }
}

/// Classifies every journal record against the chain.
///
/// `account_address` is the account that signs submissions; its nonce is what turns a
/// missing receipt into proof that a transaction was never included.
pub async fn reconcile(
    rpc: &StarknetRpc,
    account_address: Felt,
    records: &[OperationRecord],
) -> Result<Vec<Finding>, RpcError> {
    let mut findings = Vec::with_capacity(records.len());
    // Read once, outside the loop: every record is judged against the same view of the
    // account, so a nonce that advances mid-scan cannot make two records disagree.
    let live_nonce = rpc.nonce(account_address, &BlockId::Latest).await?;

    for record in records {
        findings.push(classify(rpc, record, live_nonce).await?);
    }
    Ok(findings)
}

async fn classify(
    rpc: &StarknetRpc,
    record: &OperationRecord,
    live_nonce: Felt,
) -> Result<Finding, RpcError> {
    let attempt = record.attempt();
    let (outcome, next_action, reason, observed_block) = match record.stage() {
        // Nothing was signed, so nothing can be on the chain. This is the only case that is
        // safe purely from the journal, with no chain read at all.
        OperationStage::Claimed | OperationStage::Prepared | OperationStage::Proven => (
            Outcome::NoEffect,
            NextAction::SafeToRetry,
            "no transaction was ever signed, so nothing reached the chain".to_owned(),
            None,
        ),
        OperationStage::Reverted => (
            Outcome::Reverted,
            NextAction::SafeToRetry,
            "the transaction was included and reverted, so no effect exists".to_owned(),
            None,
        ),
        OperationStage::Committed => (
            Outcome::Effect,
            NextAction::None,
            "settled: the chain accepted it and local state records it".to_owned(),
            None,
        ),
        // Signed and Submitted are the same question. Signed means the process died with a
        // transaction on disk and no evidence it was ever handed to the node — which is not
        // evidence that it was not.
        OperationStage::Signed | OperationStage::Submitted => {
            resolve_on_chain(rpc, attempt, live_nonce).await?
        }
        // The chain accepted it. The gap is local: the write returned but the channel
        // record was never updated, so a cursor may be behind what the chain has seen.
        OperationStage::Accepted => (
            Outcome::Effect,
            NextAction::CommitLocalState,
            "the chain accepted this but local state was never committed; the channel \
             cursor may be behind the chain"
                .to_owned(),
            attempt
                .receipt
                .as_ref()
                .and_then(|receipt| receipt.block_number),
        ),
        OperationStage::NeedsAttention => (
            Outcome::Unknown,
            NextAction::OperatorAttention,
            "a previous run could not classify this attempt".to_owned(),
            None,
        ),
    };

    let accepted_at = if outcome == Outcome::Effect {
        acceptance_timestamp(rpc, record, observed_block).await?
    } else {
        None
    };

    Ok(Finding {
        operation_id: record.operation_id.clone(),
        operation: record.operation,
        request: record.request.clone(),
        stage: record.stage(),
        channel: record.channel.clone(),
        transaction_hash: attempt.transaction_hash,
        accepted_at,
        outcome,
        next_action,
        reason,
    })
}

async fn acceptance_timestamp(
    rpc: &StarknetRpc,
    record: &OperationRecord,
    observed_block: Option<u64>,
) -> Result<Option<u64>, RpcError> {
    let attempt = record.attempt();
    if record.version >= 3 {
        if let Some(timestamp) = attempt.accepted_at {
            return Ok(Some(timestamp));
        }
    }

    let block_number = if observed_block.is_some() {
        observed_block
    } else if let Some(receipt) = attempt.receipt.as_ref().filter(|r| r.is_accepted()) {
        receipt.block_number
    } else if let Some(transaction_hash) = attempt.transaction_hash {
        match rpc.transaction_receipt(transaction_hash).await {
            Ok(receipt) if receipt.is_accepted() => receipt.block_number,
            Ok(_) => None,
            Err(error) if error.is_transaction_not_found() => None,
            Err(error) => return Err(error),
        }
    } else {
        None
    };

    match block_number {
        Some(number) => rpc.block_timestamp(number).await.map(Some),
        None => Ok(None),
    }
}

/// Decides a signed-or-submitted attempt from the receipt, falling back to the nonce.
async fn resolve_on_chain(
    rpc: &StarknetRpc,
    attempt: &Attempt,
    live_nonce: Felt,
) -> Result<(Outcome, NextAction, String, Option<u64>), RpcError> {
    let Some(transaction_hash) = attempt.transaction_hash else {
        // The stage claims a transaction exists and the record does not name it. That is a
        // contradiction, not an absence, and the safe reading is that something may be out
        // there under a hash we no longer know.
        return Ok((
            Outcome::Unknown,
            NextAction::OperatorAttention,
            "the record reached a signed stage without recording a transaction hash".to_owned(),
            None,
        ));
    };

    if let Some(receipt) = &attempt.receipt {
        if receipt.is_accepted() {
            return Ok((
                Outcome::Effect,
                NextAction::CommitLocalState,
                format!("the durable receipt says transaction {transaction_hash:#x} was accepted"),
                receipt.block_number,
            ));
        }
        if receipt.is_reverted() {
            return Ok((
                Outcome::Reverted,
                NextAction::SafeToRetry,
                format!("the durable receipt says transaction {transaction_hash:#x} reverted"),
                None,
            ));
        }
    }

    match rpc.transaction_receipt(transaction_hash).await {
        Ok(receipt) if receipt.is_accepted() => Ok((
            Outcome::Effect,
            NextAction::CommitLocalState,
            format!(
                "transaction {transaction_hash:#x} was accepted; local state does not record it"
            ),
            receipt.block_number,
        )),
        Ok(receipt) if receipt.is_reverted() => Ok((
            Outcome::Reverted,
            NextAction::SafeToRetry,
            format!("transaction {transaction_hash:#x} reverted, so no effect exists"),
            None,
        )),
        // Known to the node but neither accepted nor reverted yet.
        Ok(_) => Ok((
            Outcome::Pending,
            NextAction::Wait,
            format!(
                "transaction {transaction_hash:#x} is known to the node but has no outcome yet"
            ),
            None,
        )),
        Err(error) if error.is_transaction_not_found() => {
            let (outcome, action, reason) =
                absent_from_chain(attempt, transaction_hash, live_nonce);
            Ok((outcome, action, reason, None))
        }
        Err(error) => Err(error),
    }
}

/// Judges a transaction the node has never heard of.
///
/// A missing receipt is not proof of anything by itself: the node may simply not have seen
/// it. The nonce is what settles it. A transaction is bound to the nonce it was signed
/// against, so once the account has moved past that nonce this transaction can never be
/// included by anyone.
fn absent_from_chain(
    attempt: &Attempt,
    transaction_hash: Felt,
    live_nonce: Felt,
) -> (Outcome, NextAction, String) {
    let Some(signed_nonce) = attempt.account_nonce else {
        return (
            Outcome::Unknown,
            NextAction::OperatorAttention,
            format!(
                "transaction {transaction_hash:#x} is not on the chain and no nonce was \
                 recorded, so it cannot be ruled out"
            ),
        );
    };

    if live_nonce > signed_nonce {
        (
            Outcome::NoEffect,
            NextAction::SafeToRetry,
            format!(
                "transaction {transaction_hash:#x} is absent and the account has moved past \
                 nonce {signed_nonce:#x}, so it can never be included"
            ),
        )
    } else {
        (
            Outcome::Pending,
            NextAction::Wait,
            format!(
                "transaction {transaction_hash:#x} is absent but nonce {signed_nonce:#x} is \
                 still current, so it may yet be included"
            ),
        )
    }
}
