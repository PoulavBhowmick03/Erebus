use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::{record, Coordinator, Error};
use erebus_core::settlement::PreparedSettlement;

/// A local network response, never independently verified settlement evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BroadcastOutcome {
    /// No conclusive response, including a timeout, disconnect, or interrupted process.
    Unknown,
    /// The backend acknowledged submission, without proving inclusion or finality.
    Submitted,
    /// This request was rejected. Earlier requests may still have taken effect.
    Rejected,
}

/// Redacted attempt history; contains no token, signed payload, or backend response text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttemptDiagnostic {
    /// Zero-based position in the append-only broadcast history.
    pub attempt_index: usize,
    /// Initial transaction (zero) or replacement index submitted by this attempt.
    pub transaction_index: usize,
    /// Local response only, never settlement evidence.
    pub outcome: BroadcastOutcome,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AttemptRecord {
    pub(crate) token: [u8; 32],
    pub(crate) outcome: BroadcastOutcome,
    pub(crate) transaction_index: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Replacement {
    pub(crate) plan: Vec<u8>,
    pub(crate) raw: Option<Vec<u8>>,
}

impl Drop for Replacement {
    fn drop(&mut self) {
        self.plan.zeroize();
        if let Some(raw) = &mut self.raw {
            raw.zeroize();
        }
    }
}

/// Opaque capability for finishing exactly one operation's broadcast attempt.
/// Tokens cannot be reconstructed from diagnostics and must not be logged.
pub struct AttemptToken {
    operation_ref: [u8; 32],
    token: [u8; 32],
}

impl std::fmt::Debug for AttemptToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AttemptToken { [REDACTED] }")
    }
}

impl Drop for AttemptToken {
    fn drop(&mut self) {
        self.token.zeroize();
    }
}

/// Private recovery payload with redacted Debug and explicit borrowed accessors.
/// Contains no lock. Do not log accessor results or serialize them as diagnostics.
pub struct SignedTransaction {
    index: usize,
    raw: Vec<u8>,
    plan: Vec<u8>,
    prepared: PreparedSettlement,
}

impl SignedTransaction {
    /// Zero for the initial transaction, then the append-only replacement index.
    pub fn index(&self) -> usize {
        self.index
    }
    /// Exact immutable signed bytes. Reading does not authorize a broadcast.
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }
    /// Exact immutable backend signing plan.
    pub fn plan(&self) -> &[u8] {
        &self.plan
    }
    /// Bound prepared settlement, including private backend evidence.
    pub fn prepared(&self) -> &PreparedSettlement {
        &self.prepared
    }
}

impl std::fmt::Debug for SignedTransaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SignedTransaction { [REDACTED] }")
    }
}

impl Drop for SignedTransaction {
    fn drop(&mut self) {
        self.raw.zeroize();
        self.plan.zeroize();
        self.prepared.backend_evidence.zeroize();
    }
}

/// A durably recorded attempt and its owned payload, safe to retain across await.
#[derive(Debug)]
pub struct BroadcastAttempt {
    /// Capability accepted only for this attempt and operation.
    pub token: AttemptToken,
    /// Immutable transaction, signing plan, and prepared settlement.
    pub transaction: SignedTransaction,
}

impl record::Intent {
    fn signed_payload_at(&self, index: usize) -> Result<Option<SignedTransaction>, Error> {
        let (raw, plan) = if index == 0 {
            (&self.signed_transaction, self.transaction_plan.as_ref())
        } else {
            let replacement = self.replacements.get(index - 1).ok_or(Error::Conflict)?;
            (&replacement.raw, Some(&replacement.plan))
        };
        let Some(raw) = raw else { return Ok(None) };
        Ok(Some(SignedTransaction {
            index,
            raw: raw.clone(),
            plan: plan.cloned().ok_or(Error::Storage)?,
            prepared: PreparedSettlement {
                backend_evidence: self.prepared.clone().ok_or(Error::Storage)?,
                ..self.prepared_identity()?
            },
        }))
    }
}

impl Coordinator {
    /// Reads redacted, append-only attempt history for one operation in this context.
    pub fn broadcast_history(
        &self,
        operation_ref: [u8; 32],
    ) -> Result<Vec<AttemptDiagnostic>, Error> {
        let _lock = self.store.lock_identity().map_err(|_| Error::Storage)?;
        let state = self.load()?;
        let intent = &state.intents[state.index(operation_ref)?];
        self.check_terms(&intent.opening()?.0)?;
        Ok(intent
            .broadcast_attempts
            .iter()
            .enumerate()
            .map(|(attempt_index, a)| AttemptDiagnostic {
                attempt_index,
                transaction_index: a.transaction_index,
                outcome: a.outcome,
            })
            .collect())
    }
    /// Retrieves private signed recovery data, even after expiry or reconciliation.
    /// Re-syncs the snapshot before returning bytes after a possibly uncertain write.
    /// This does not record or authorize a new network attempt; use begin for submission.
    pub fn signed_transaction(
        &self,
        operation_ref: [u8; 32],
    ) -> Result<Option<SignedTransaction>, Error> {
        self.signed_transaction_at(operation_ref, 0)
    }

    /// Retrieves an initial or replacement transaction by its durable index.
    /// A fenced but unsigned plan returns None; an absent index is a conflict.
    pub fn signed_transaction_at(
        &self,
        operation_ref: [u8; 32],
        index: usize,
    ) -> Result<Option<SignedTransaction>, Error> {
        let _lock = self.store.lock_identity().map_err(|_| Error::Storage)?;
        let state = self.load()?;
        let intent = &state.intents[state.index(operation_ref)?];
        self.check_terms(&intent.opening()?.0)?;
        let payload = intent.signed_payload_at(index)?;
        if payload.is_some() {
            self.save(&state)?;
        }
        Ok(payload)
    }

    /// Appends and syncs an Unknown attempt before returning any network payload.
    /// No file lock escapes this synchronous method. Broadcast only after success.
    /// Every retry appends a new attempt; it never erases earlier uncertainty.
    /// Expired or reconciled operations require recovery rather than new submission.
    pub fn begin_broadcast_attempt(
        &self,
        operation_ref: [u8; 32],
        now: u64,
    ) -> Result<BroadcastAttempt, Error> {
        self.begin_attempt(operation_ref, now, false)
    }

    /// Atomically fences only the initial transaction's first broadcast. Any retained
    /// attempt or replacement conflicts, including an Unknown fence after a crash before
    /// network I/O. Use explicit recovery APIs for retries; do not erase that uncertainty.
    pub fn begin_initial_broadcast_attempt(
        &self,
        operation_ref: [u8; 32],
        now: u64,
    ) -> Result<BroadcastAttempt, Error> {
        self.begin_attempt(operation_ref, now, true)
    }

    fn begin_attempt(
        &self,
        operation_ref: [u8; 32],
        now: u64,
        initial_only: bool,
    ) -> Result<BroadcastAttempt, Error> {
        let _lock = self.store.lock_identity().map_err(|_| Error::Storage)?;
        let mut state = self.load()?;
        let index = state.index(operation_ref)?;
        let intent = &state.intents[index];
        if initial_only
            && (!intent.broadcast_attempts.is_empty() || !intent.replacements.is_empty())
        {
            return Err(Error::Conflict);
        }
        let (terms, _) = intent.opening()?;
        self.check_terms(&terms)?;
        if intent.accounting != record::Accounting::Reserved {
            return Err(Error::Stage);
        }
        if now >= terms.expiry {
            return Err(Error::Expired);
        }
        if intent.broadcast_attempts.len() >= record::MAX_BROADCAST_ATTEMPTS {
            return Err(Error::Stage);
        }
        let transaction_index = intent.replacements.len();
        let transaction = intent
            .signed_payload_at(transaction_index)?
            .ok_or(Error::Stage)?;
        let mut token = [0; 32];
        OsRng
            .try_fill_bytes(&mut token)
            .map_err(|_| Error::Backend)?;
        state.intents[index].broadcast_attempts.push(AttemptRecord {
            token,
            outcome: BroadcastOutcome::Unknown,
            transaction_index,
        });
        self.save(&state)?;
        Ok(BroadcastAttempt {
            token: AttemptToken {
                operation_ref,
                token,
            },
            transaction,
        })
    }

    /// Saves a response only for the matching operation and opaque attempt token.
    /// Unknown may advance to a response; conflicting responses fail closed. A later
    /// Unknown cannot downgrade a response. Identical retries re-sync durably.
    /// Late responses are accepted after expiry or reconciliation, without accounting changes.
    pub fn finish_broadcast_attempt(
        &self,
        operation_ref: [u8; 32],
        token: &AttemptToken,
        outcome: BroadcastOutcome,
    ) -> Result<(), Error> {
        if token.operation_ref != operation_ref {
            return Err(Error::Conflict);
        }
        let _lock = self.store.lock_identity().map_err(|_| Error::Storage)?;
        let mut state = self.load()?;
        let index = state.index(operation_ref)?;
        self.check_terms(&state.intents[index].opening()?.0)?;
        let attempt = state.intents[index]
            .broadcast_attempts
            .iter_mut()
            .find(|a| a.token == token.token)
            .ok_or(Error::Conflict)?;
        if attempt.outcome == BroadcastOutcome::Unknown {
            attempt.outcome = outcome;
        } else if outcome != BroadcastOutcome::Unknown && outcome != attempt.outcome {
            return Err(Error::Conflict);
        }
        self.save(&state)
    }

    /// Reads every replacement plan, including a fenced plan awaiting signature.
    /// The returned bytes are private recovery data, not diagnostics.
    pub fn replacement_plans(&self, operation_ref: [u8; 32]) -> Result<Vec<Vec<u8>>, Error> {
        let _lock = self.store.lock_identity().map_err(|_| Error::Storage)?;
        let state = self.load()?;
        let intent = &state.intents[state.index(operation_ref)?];
        self.check_terms(&intent.opening()?.0)?;
        Ok(intent.replacements.iter().map(|r| r.plan.clone()).collect())
    }

    /// Fences an append-only replacement plan before calling a local signer.
    /// Both callbacks must be local and must not broadcast. The validator MUST verify
    /// the same sender, chain, nonce, payment and gas, and a sufficient fee bump against
    /// the previous signed version. The coordinator cannot interpret backend bytes.
    /// Retry the exact fenced plan after failure; other plans conflict until it is signed.
    /// Returns the durable replacement index. Earlier bytes and plans remain immutable.
    pub fn sign_replacement<E>(
        &self,
        operation_ref: [u8; 32],
        now: u64,
        plan: &[u8],
        signer: impl FnOnce(&SignedTransaction, &[u8]) -> Result<Vec<u8>, E>,
        validate: impl FnOnce(&SignedTransaction, &[u8], &[u8]) -> Result<(), E>,
    ) -> Result<usize, Error> {
        let _lock = self.store.lock_identity().map_err(|_| Error::Storage)?;
        let mut state = self.load()?;
        let index = state.index(operation_ref)?;
        let intent = &state.intents[index];
        let (terms, _) = intent.opening()?;
        self.check_terms(&terms)?;
        if intent.accounting != record::Accounting::Reserved {
            return Err(Error::Stage);
        }
        if now >= terms.expiry {
            return Err(Error::Expired);
        }
        if plan.is_empty() || plan.len() > record::MAX_TRANSACTION_PLAN_BYTES {
            return Err(Error::Agreement);
        }
        if intent.transaction_plan.as_deref() == Some(plan) {
            return Err(Error::Conflict);
        }
        let position =
            if let Some(position) = intent.replacements.iter().position(|r| r.plan == plan) {
                position
            } else {
                if intent.replacements.last().is_some_and(|r| r.raw.is_none()) {
                    return Err(Error::Conflict);
                }
                if intent.replacements.len() >= record::MAX_REPLACEMENTS {
                    return Err(Error::Stage);
                }
                intent.replacements.len()
            };
        let previous = intent.signed_payload_at(position)?.ok_or(Error::Stage)?;
        if position == intent.replacements.len() {
            state.intents[index].replacements.push(Replacement {
                plan: plan.to_vec(),
                raw: None,
            });
        }
        self.save(&state)?;
        let replacement = &state.intents[index].replacements[position];
        if let Some(raw) = &replacement.raw {
            validate(&previous, &replacement.plan, raw).map_err(|_| Error::Backend)?;
        } else {
            let raw = signer(&previous, &replacement.plan).map_err(|_| Error::Backend)?;
            if raw.is_empty() || raw.len() > record::MAX_TRANSACTION_BYTES {
                return Err(Error::Agreement);
            }
            validate(&previous, &replacement.plan, &raw).map_err(|_| Error::Backend)?;
            state.intents[index].replacements[position].raw = Some(raw);
            self.save(&state)?;
        }
        Ok(position + 1)
    }
}
