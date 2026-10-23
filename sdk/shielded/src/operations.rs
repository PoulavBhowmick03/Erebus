//! Durable installed wallet operations: deposit and withdrawal.
//!
//! These are explicit wallet flows, never agreement settlement. A deposit creates a fresh
//! random note opening, persists it before proving, and fences submission so a crash cannot
//! silently spend the same nonce lane twice. A withdrawal reserves the note locally before
//! proving and completes only when verified consumption matches its own finalized receipt.
//! An unknown broadcast stays
//! unknown: only an explicit resume may resend the exact retained bytes.

use std::{fmt, path::PathBuf, sync::Arc};

use ark_bn254::Fr;
use ark_std::{rand::rngs::OsRng, UniformRand};
use erebus_core::{
    commitment::{DealCommitment, DealNullifier},
    domain::DeploymentDomain,
    settlement::PreparedSettlement,
    shielded::SHIELDED_GUARANTEES,
    suite::keccak256,
    terms::GuaranteeSet,
};
use erebus_evm::{
    chain::{
        BroadcastOutcome, Eip1559Fees, EvmChain, SignedTransaction, SignerJournal, SigningPlan,
        TransactionKey, TransactionParams,
    },
    error::EvmError,
};
use erebus_journal::{FaultHook, JournalRecord, NoFaults, RecordId, Store};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    artifacts::{ArtifactRelease, CircuitKind, InstallOptions, InstalledProvingError},
    build_deposit_witness, build_withdraw_witness,
    gas_policy::GasPolicy,
    indexer::PoolIndex,
    wallet::{OwnedNote, WalletDomain, WalletError, WalletStore},
    LocalProof, ProverError,
};

const DEPOSIT_SIGNATURE: &[u8] = b"deposit(uint256[2],uint256[2][2],uint256[2],uint256[6])";
const WITHDRAW_SIGNATURE: &[u8] = b"withdraw(uint256[2],uint256[2][2],uint256[2],uint256[8])";
const DEPOSIT_CALL_BYTES: usize = 4 + 14 * 32;
const WITHDRAW_CALL_BYTES: usize = 4 + 16 * 32;

/// Which explicit wallet operation a durable record describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WalletOperationKind {
    /// An externally visible deposit creating one shielded note.
    Deposit,
    /// An externally visible withdrawal consuming one shielded note.
    Withdraw,
    /// An ERC-20 approval allowing the pool to move the signer's tokens.
    Approve,
    /// A native wrap into the configured wrapper, carrying the wrapped value.
    Wrap,
    /// A native unwrap from the configured wrapper, carrying zero value.
    Unwrap,
}

/// Durable progress of one wallet operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WalletOperationState {
    /// The note opening is persisted; nothing has been signed or sent.
    Planned,
    /// Exact signed bytes are retained and may still be sent explicitly.
    Signed,
    /// At least one broadcast attempt has an unknown outcome; only observation or explicit
    /// resume of the retained bytes may change this operation.
    BroadcastUnknown,
    /// Verified onchain evidence completed the operation.
    Completed,
}

/// A wallet operation failed validation, durability, proving, or submission.
#[derive(Debug, thiserror::Error)]
pub enum OperationError {
    /// The encrypted wallet rejected the transition.
    #[error(transparent)]
    Wallet(#[from] WalletError),
    /// The operation journal is unavailable or inconsistent.
    #[error("wallet operation journal is unavailable or inconsistent")]
    Journal,
    /// The recorded operation does not match the requested one.
    #[error("wallet operation conflicts with the retained record")]
    Conflict,
    /// A pending operation must be observed or explicitly resumed first.
    #[error("wallet operation is retained; observe or resume it first")]
    Retained,
    /// Local proving or public artifact installation failed.
    #[error(transparent)]
    Proving(#[from] InstalledProvingError),
    /// Witness construction failed.
    #[error(transparent)]
    Witness(#[from] ProverError),
    /// The requested transition is not representable or is missing required local evidence.
    #[error("invalid wallet transition: {0}")]
    Shape(&'static str),
    /// The EVM chain rejected the call or transaction.
    #[error(transparent)]
    Evm(#[from] EvmError),
    /// The durable gas-payer nonce lane is busy or inconsistent.
    #[error(transparent)]
    Nonce(#[from] erebus_evm::chain::NonceError),
    /// A local file or encoding is invalid.
    #[error("invalid local wallet input")]
    Input,
    /// A finalized effect did not match the expected wrapper mint or burn.
    #[error("finalized wallet effect could not be verified")]
    Effect,
    /// A legacy record carries no call expectations and cannot be reconstructed safely.
    #[error("legacy wallet operation lacks call expectations; reconcile it manually")]
    Migration,
    /// A retained signed plan or saved claim uses a gas limit above the configured cap. Nothing
    /// was sent; the signed bytes and the nonce claim are kept for an operator decision.
    #[error("retained plan gas limit {saved} exceeds the configured cap {cap}")]
    PlanAboveCap {
        /// Gas limit of the saved plan.
        saved: u64,
        /// The configured legacy cap.
        cap: u64,
    },
    /// The exact call could not be estimated: it would revert at the latest block, or the
    /// provider failed. No nonce claim was written and nothing was signed.
    #[error("gas estimate of the exact call failed")]
    GasEstimate,
    /// The call's estimate plus margin exceeds the method's gas limit. No nonce claim was
    /// written and nothing was signed.
    #[error("gas estimate {estimate} exceeds the method limit {limit} with margin")]
    GasLimit {
        /// The live `eth_estimateGas` of the exact call.
        estimate: u64,
        /// The method's operator gas limit.
        limit: u64,
    },
}

impl From<erebus_journal::StoreError<WalletOperationId>> for OperationError {
    fn from(_: erebus_journal::StoreError<WalletOperationId>) -> Self {
        Self::Journal
    }
}

fn random_field() -> Result<[u8; 32], OperationError> {
    use ark_ff::{BigInteger, PrimeField};
    let bytes = Fr::rand(&mut OsRng).into_bigint().to_bytes_be();
    let mut field = [0u8; 32];
    field[32 - bytes.len()..].copy_from_slice(&bytes);
    Ok(field)
}

fn word<F: ark_ff::PrimeField>(value: F) -> [u8; 32] {
    use ark_ff::BigInteger;
    let bytes = value.into_bigint().to_bytes_be();
    let mut word = [0u8; 32];
    word[32 - bytes.len()..].copy_from_slice(&bytes);
    word
}

fn call_calldata(
    signature: &[u8],
    expected_bytes: usize,
    public_count: usize,
    proof: &LocalProof,
) -> Result<Vec<u8>, OperationError> {
    if proof.public_inputs.len() != public_count {
        return Err(OperationError::Shape("proof public signal count"));
    }
    let mut data = Vec::with_capacity(expected_bytes);
    data.extend_from_slice(&keccak256(&[signature])[..4]);
    for field in [
        proof.proof.a.x,
        proof.proof.a.y,
        proof.proof.b.x.c1,
        proof.proof.b.x.c0,
        proof.proof.b.y.c1,
        proof.proof.b.y.c0,
        proof.proof.c.x,
        proof.proof.c.y,
    ] {
        data.extend_from_slice(&word(field));
    }
    for field in &proof.public_inputs {
        data.extend_from_slice(&word(*field));
    }
    if data.len() != expected_bytes {
        return Err(OperationError::Shape("calldata length"));
    }
    Ok(data)
}

/// Encodes one `deposit(...)` calldata for the fixed six public signals.
pub fn deposit_calldata(proof: &LocalProof) -> Result<Vec<u8>, OperationError> {
    call_calldata(DEPOSIT_SIGNATURE, DEPOSIT_CALL_BYTES, 6, proof)
}

/// Encodes one `withdraw(...)` calldata for the fixed eight public signals.
pub fn withdraw_calldata(proof: &LocalProof) -> Result<Vec<u8>, OperationError> {
    call_calldata(WITHDRAW_SIGNATURE, WITHDRAW_CALL_BYTES, 8, proof)
}

/// Stable journal id for one operation identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WalletOperationId(String);

impl fmt::Display for WalletOperationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl RecordId for WalletOperationId {
    fn as_file_stem(&self) -> &str {
        &self.0
    }
    fn from_file_stem(stem: &str) -> Option<Self> {
        (stem.len() == 64
            && stem
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
        .then(|| Self(stem.into()))
    }
}

fn operation_id(identity: &[u8; 32]) -> WalletOperationId {
    WalletOperationId(hex::encode(identity))
}

/// One retained submission attempt for an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WalletAttemptOutcome {
    /// A broadcast attempt had an unknown outcome.
    Unknown,
    /// The provider rejected the transaction before inclusion.
    Rejected,
    /// A finalized canonical receipt showed the transaction reverted.
    Reverted,
    /// The operation completed.
    Completed,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletAttempt {
    tx_hash: [u8; 32],
    outcome: WalletAttemptOutcome,
    #[serde(default)]
    block: Option<u64>,
}

const MAX_ATTEMPTS: usize = 32;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletOperation {
    version: u32,
    id: WalletOperationId,
    kind: WalletOperationKind,
    identity: [u8; 32],
    commitment: [u8; 32],
    nullifier: [u8; 32],
    state: WalletOperationState,
    plan: Option<String>,
    raw: Option<String>,
    #[serde(default)]
    tx_hash: Option<[u8; 32]>,
    /// Expected call target. Zero on records written before the operation spec existed.
    #[serde(default)]
    target: [u8; 20],
    /// Expected native value in wei. Zero on records written before value support.
    #[serde(default)]
    value: u128,
    /// Operation amount in base units (deposit, approval, wrap, or unwrap amount).
    #[serde(default)]
    amount: u128,
    /// Keccak-256 of the expected calldata. Zero on records written before it was stored.
    #[serde(default)]
    calldata_hash: [u8; 32],
    #[serde(default)]
    history: Vec<WalletAttempt>,
    created_at: u64,
    updated_at: u64,
}

impl WalletOperation {
    fn push_attempt(&mut self, attempt: WalletAttempt) {
        self.history.push(attempt);
        if self.history.len() > MAX_ATTEMPTS {
            let excess = self.history.len() - MAX_ATTEMPTS;
            self.history.drain(..excess);
        }
    }
}

/// One retained attempt without secret material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalletAttemptView {
    /// Transaction hash of the attempt.
    pub tx_hash: [u8; 32],
    /// What the attempt established.
    pub outcome: WalletAttemptOutcome,
    /// Finalized including block, when known.
    pub block: Option<u64>,
}

impl JournalRecord for WalletOperation {
    type Id = WalletOperationId;
    const CURRENT_VERSION: u32 = 1;
    const OLDEST_READABLE_VERSION: u32 = 1;
    fn version(&self) -> u32 {
        self.version
    }
    fn record_id(&self) -> &Self::Id {
        &self.id
    }
    fn attempt_count(&self) -> usize {
        1
    }
}

/// Exact retained bytes of one signed wallet operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedBytes {
    /// Encoded signing plan.
    pub plan: Vec<u8>,
    /// Exact signed transaction bytes.
    pub raw: Vec<u8>,
}

/// A durable wallet-operation record with no secret material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletOperationView {
    kind: WalletOperationKind,
    identity: [u8; 32],
    commitment: [u8; 32],
    nullifier: [u8; 32],
    state: WalletOperationState,
    retained: bool,
    target: [u8; 20],
    value: u128,
    amount: u128,
    calldata_hash: [u8; 32],
    attempts: Vec<WalletAttemptView>,
}

impl WalletOperationView {
    /// Which explicit operation this is.
    #[must_use]
    pub const fn kind(&self) -> WalletOperationKind {
        self.kind
    }
    /// Deposit commitment or withdrawal nullifier, the journal identity.
    #[must_use]
    pub const fn identity(&self) -> [u8; 32] {
        self.identity
    }
    /// Note commitment this operation acts on.
    #[must_use]
    pub const fn commitment(&self) -> [u8; 32] {
        self.commitment
    }
    /// Note nullifier this operation acts on.
    #[must_use]
    pub const fn nullifier(&self) -> [u8; 32] {
        self.nullifier
    }
    /// Current durable state.
    #[must_use]
    pub const fn state(&self) -> WalletOperationState {
        self.state
    }
    /// Whether exact signed bytes are retained for explicit resume.
    #[must_use]
    pub const fn retained(&self) -> bool {
        self.retained
    }
    /// Expected call target, or zero on a legacy record.
    #[must_use]
    pub const fn target(&self) -> [u8; 20] {
        self.target
    }
    /// Expected native value in wei.
    #[must_use]
    pub const fn value(&self) -> u128 {
        self.value
    }
    /// Operation amount in base units.
    #[must_use]
    pub const fn amount(&self) -> u128 {
        self.amount
    }
    /// Keccak-256 of the expected calldata, or zero on a record signed before it was stored.
    #[must_use]
    pub const fn calldata_hash(&self) -> [u8; 32] {
        self.calldata_hash
    }
    /// Retained attempt history, oldest first.
    #[must_use]
    pub fn attempts(&self) -> &[WalletAttemptView] {
        &self.attempts
    }
}

fn view(operation: &WalletOperation) -> WalletOperationView {
    WalletOperationView {
        kind: operation.kind,
        identity: operation.identity,
        commitment: operation.commitment,
        nullifier: operation.nullifier,
        state: operation.state,
        retained: operation.plan.is_some() && operation.raw.is_some(),
        target: operation.target,
        value: operation.value,
        amount: operation.amount,
        calldata_hash: operation.calldata_hash,
        attempts: operation
            .history
            .iter()
            .map(|attempt| WalletAttemptView {
                tx_hash: attempt.tx_hash,
                outcome: attempt.outcome,
                block: attempt.block,
            })
            .collect(),
    }
}

/// Durable journal of explicit wallet operations. It stores signed bytes only, never keys.
pub struct OperationStore {
    store: Store<WalletOperation>,
}

impl OperationStore {
    /// Opens the journal under an operator-controlled private directory.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, OperationError> {
        Self::with_faults(root, Arc::new(NoFaults))
    }

    /// Opens the journal with a deterministic storage fault hook for crash tests.
    pub fn with_faults(
        root: impl Into<PathBuf>,
        faults: Arc<dyn FaultHook>,
    ) -> Result<Self, OperationError> {
        Ok(Self {
            store: Store::open_with_fault_hook(root.into(), faults)?,
        })
    }

    /// Returns one durable operation by identity.
    pub fn get(&self, identity: &[u8; 32]) -> Result<Option<WalletOperationView>, OperationError> {
        let _lock = self.store.lock_identity()?;
        Ok(self.store.read(&operation_id(identity))?.as_ref().map(view))
    }

    /// Lists operations that are not yet completed.
    pub fn pending(&self) -> Result<Vec<WalletOperationView>, OperationError> {
        let _lock = self.store.lock_identity()?;
        let mut records = self.store.records()?;
        records.retain(|record| record.state != WalletOperationState::Completed);
        records.sort_by_key(|record| (record.created_at, record.identity));
        Ok(records.iter().map(view).collect())
    }

    /// Lists every durable operation, newest last.
    pub fn all(&self) -> Result<Vec<WalletOperationView>, OperationError> {
        let _lock = self.store.lock_identity()?;
        let mut records = self.store.records()?;
        records.sort_by_key(|record| (record.created_at, record.identity));
        Ok(records.iter().map(view).collect())
    }

    fn idempotent_begin(
        &self,
        spec: &OperationSpec,
        now: u64,
    ) -> Result<WalletOperationView, OperationError> {
        let id = operation_id(&spec.identity);
        let _lock = self.store.lock_identity()?;
        if let Some(existing) = self.store.read(&id)? {
            if existing.kind == spec.kind
                && existing.identity == spec.identity
                && existing.commitment == spec.commitment
                && existing.nullifier == spec.nullifier
                && existing.target == spec.target
                && existing.value == spec.value
                && existing.amount == spec.amount
            {
                return Ok(view(&existing));
            }
            return Err(OperationError::Conflict);
        }
        let record = WalletOperation {
            version: 1,
            id,
            kind: spec.kind,
            identity: spec.identity,
            commitment: spec.commitment,
            nullifier: spec.nullifier,
            state: WalletOperationState::Planned,
            plan: None,
            raw: None,
            tx_hash: None,
            target: spec.target,
            value: spec.value,
            amount: spec.amount,
            calldata_hash: [0; 32],
            history: Vec::new(),
            created_at: now,
            updated_at: now,
        };
        self.store.write(&record)?;
        Ok(view(&record))
    }

    fn record_signed(
        &self,
        identity: &[u8; 32],
        plan: &[u8],
        raw: &[u8],
        now: u64,
    ) -> Result<(), OperationError> {
        let id = operation_id(identity);
        let _lock = self.store.lock_identity()?;
        let mut record = self.store.read(&id)?.ok_or(OperationError::Conflict)?;
        // Bind the exact signed call: target, value, and calldata hash. Values already stored
        // must match; unset fields (legacy or pre-signing) are filled from the signature.
        let signed = SignedTransaction::from_raw(raw)?;
        if record.target == [0; 20] {
            record.target = signed.to();
        } else if record.target != signed.to() {
            return Err(OperationError::Conflict);
        }
        if record.value != signed.params().value {
            return Err(OperationError::Conflict);
        }
        let calldata_hash = keccak256(&[signed.input()]);
        if record.calldata_hash == [0; 32] {
            record.calldata_hash = calldata_hash;
        } else if record.calldata_hash != calldata_hash {
            return Err(OperationError::Conflict);
        }
        let plan = hex::encode(plan);
        let raw = hex::encode(raw);
        match (&record.plan, &record.raw) {
            (Some(saved_plan), Some(saved_raw)) => {
                if saved_plan == &plan && saved_raw == &raw {
                    record.updated_at = now;
                    self.store.write(&record)?;
                    return Ok(());
                }
                return Err(OperationError::Conflict);
            }
            (None, None) => {}
            _ => return Err(OperationError::Conflict),
        }
        record.plan = Some(plan);
        record.raw = Some(raw);
        record.state = WalletOperationState::Signed;
        record.updated_at = now;
        self.store.write(&record)?;
        Ok(())
    }

    fn record_attempt(
        &self,
        identity: &[u8; 32],
        tx_hash: [u8; 32],
        now: u64,
    ) -> Result<(), OperationError> {
        let id = operation_id(identity);
        let _lock = self.store.lock_identity()?;
        let mut record = self.store.read(&id)?.ok_or(OperationError::Conflict)?;
        if record.raw.is_none() || record.plan.is_none() {
            return Err(OperationError::Conflict);
        }
        if record.tx_hash.is_some_and(|saved| saved != tx_hash) {
            return Err(OperationError::Conflict);
        }
        record.tx_hash = Some(tx_hash);
        if record.state != WalletOperationState::Completed {
            record.state = WalletOperationState::BroadcastUnknown;
        }
        if !record.history.iter().any(|attempt| {
            attempt.tx_hash == tx_hash && attempt.outcome == WalletAttemptOutcome::Unknown
        }) {
            let attempt = WalletAttempt {
                tx_hash,
                outcome: WalletAttemptOutcome::Unknown,
                block: None,
            };
            record.push_attempt(attempt);
        }
        record.updated_at = now;
        self.store.write(&record)?;
        Ok(())
    }

    /// Records a finalized canonical receipt whose transaction reverted.
    ///
    /// Only finalized receipt evidence may clear retained bytes. The operation returns to
    /// `Planned` so an explicit retry can build a new call for the same note; a withdrawal
    /// caller may then release its local reservation.
    fn record_finalized_revert(
        &self,
        identity: &[u8; 32],
        tx_hash: [u8; 32],
        block: u64,
        now: u64,
    ) -> Result<bool, OperationError> {
        let id = operation_id(identity);
        let _lock = self.store.lock_identity()?;
        let mut record = self.store.read(&id)?.ok_or(OperationError::Conflict)?;
        if record.state == WalletOperationState::Completed
            || record.history.iter().any(|attempt| {
                attempt.tx_hash == tx_hash && attempt.outcome == WalletAttemptOutcome::Reverted
            })
        {
            return Ok(false);
        }
        // A reverted hash must be a known attempt (the active transaction or an older
        // replacement) so a foreign receipt can never change this record.
        let known = record.tx_hash == Some(tx_hash)
            || record
                .history
                .iter()
                .any(|attempt| attempt.tx_hash == tx_hash);
        if !known || record.state == WalletOperationState::Planned {
            return Ok(false);
        }
        // Record the specific attempt's outcome. Older replacements may revert while a newer
        // attempt is still pending; only the active transaction clears the active bytes.
        if let Some(attempt) = record
            .history
            .iter_mut()
            .find(|attempt| attempt.tx_hash == tx_hash)
        {
            attempt.outcome = WalletAttemptOutcome::Reverted;
            attempt.block = Some(block);
        } else {
            let attempt = WalletAttempt {
                tx_hash,
                outcome: WalletAttemptOutcome::Reverted,
                block: Some(block),
            };
            record.push_attempt(attempt);
        }
        if record.tx_hash == Some(tx_hash) {
            record.plan = None;
            record.raw = None;
            record.tx_hash = None;
            // A retry re-proves; Groth16 proofs are randomized, so the exact calldata bytes
            // change while the operation identity, target, and value stay bound.
            record.calldata_hash = [0; 32];
            record.state = WalletOperationState::Planned;
        }
        record.updated_at = now;
        self.store.write(&record)?;
        Ok(true)
    }

    /// Persists one same-intent replacement before broadcast.
    ///
    /// The previous signed transaction must exist, and the new one may change only fee fields:
    /// sender, nonce, chain, target, value, calldata, and gas limit are preserved. All older
    /// attempt hashes stay in the history for reconciliation.
    fn record_replacement(
        &self,
        identity: &[u8; 32],
        plan: &[u8],
        raw: &[u8],
        now: u64,
    ) -> Result<(), OperationError> {
        let id = operation_id(identity);
        let _lock = self.store.lock_identity()?;
        let mut record = self.store.read(&id)?.ok_or(OperationError::Conflict)?;
        if record.state == WalletOperationState::Completed {
            return Err(OperationError::Conflict);
        }
        let previous_raw = record.raw.as_deref().ok_or(OperationError::Conflict)?;
        let previous_raw = hex::decode(previous_raw).map_err(|_| OperationError::Journal)?;
        let previous = SignedTransaction::from_raw(&previous_raw)?;
        let next = SignedTransaction::from_raw(raw)?;
        next.validate_replacement(&previous)?;
        if record.target != next.to()
            || record.value != next.params().value
            || keccak256(&[next.input()]) != record.calldata_hash
        {
            return Err(OperationError::Conflict);
        }
        record.plan = Some(hex::encode(plan));
        record.raw = Some(hex::encode(raw));
        record.tx_hash = Some(next.hash());
        record.state = WalletOperationState::Signed;
        record.updated_at = now;
        self.store.write(&record)?;
        Ok(())
    }

    /// Marks an operation complete from a finalized canonical receipt.
    /// This is used for approvals, whose effect is fully described by the receipt.
    fn record_finalized_success(
        &self,
        identity: &[u8; 32],
        tx_hash: [u8; 32],
        block: Option<u64>,
        now: u64,
    ) -> Result<(), OperationError> {
        let id = operation_id(identity);
        let _lock = self.store.lock_identity()?;
        let mut record = self.store.read(&id)?.ok_or(OperationError::Conflict)?;
        if let Some(attempt) = record
            .history
            .iter_mut()
            .find(|attempt| attempt.tx_hash == tx_hash)
        {
            attempt.outcome = WalletAttemptOutcome::Completed;
            attempt.block = block;
        } else {
            let attempt = WalletAttempt {
                tx_hash,
                outcome: WalletAttemptOutcome::Completed,
                block,
            };
            record.push_attempt(attempt);
        }
        record.state = WalletOperationState::Completed;
        record.updated_at = now;
        self.store.write(&record)?;
        Ok(())
    }

    fn record_rejected(&self, identity: &[u8; 32], now: u64) -> Result<(), OperationError> {
        let id = operation_id(identity);
        let _lock = self.store.lock_identity()?;
        let mut record = self.store.read(&id)?.ok_or(OperationError::Conflict)?;
        if record.state == WalletOperationState::BroadcastUnknown {
            record.state = WalletOperationState::Signed;
        }
        if let Some(tx_hash) = record.tx_hash {
            if let Some(attempt) = record
                .history
                .iter_mut()
                .find(|attempt| attempt.tx_hash == tx_hash)
            {
                attempt.outcome = WalletAttemptOutcome::Rejected;
            } else {
                let attempt = WalletAttempt {
                    tx_hash,
                    outcome: WalletAttemptOutcome::Rejected,
                    block: None,
                };
                record.push_attempt(attempt);
            }
        }
        record.updated_at = now;
        self.store.write(&record)?;
        Ok(())
    }

    fn complete(&self, identity: &[u8; 32], now: u64) -> Result<(), OperationError> {
        let id = operation_id(identity);
        let _lock = self.store.lock_identity()?;
        let mut record = self.store.read(&id)?.ok_or(OperationError::Conflict)?;
        if let Some(tx_hash) = record.tx_hash {
            let attempt = WalletAttempt {
                tx_hash,
                outcome: WalletAttemptOutcome::Completed,
                block: None,
            };
            record.push_attempt(attempt);
        }
        record.state = WalletOperationState::Completed;
        record.updated_at = now;
        self.store.write(&record)?;
        Ok(())
    }

    fn retained_bytes(&self, identity: &[u8; 32]) -> Result<Option<RetainedBytes>, OperationError> {
        let id = operation_id(identity);
        let _lock = self.store.lock_identity()?;
        let record = self.store.read(&id)?.ok_or(OperationError::Conflict)?;
        match (record.plan, record.raw) {
            (Some(plan), Some(raw)) => Ok(Some(RetainedBytes {
                plan: hex::decode(plan).map_err(|_| OperationError::Journal)?,
                raw: hex::decode(raw).map_err(|_| OperationError::Journal)?,
            })),
            _ => Ok(None),
        }
    }
}

fn withdraw_binding(identity: &[u8; 32], commitment: &[u8; 32]) -> [u8; 32] {
    keccak256(&[b"erebus/shielded/wallet-withdraw/v1", identity, commitment])
}

/// One explicit wallet operation's durable identity and expected call.
///
/// The calldata hash is bound at signing time from the exact signed transaction, because a
/// deposit's proof-dependent calldata does not exist when the operation is first persisted.
#[derive(Clone)]
pub struct OperationSpec {
    /// Operation kind.
    pub kind: WalletOperationKind,
    /// Deposit commitment, withdrawal nullifier, or a domain-separated approval/wrap identity.
    pub identity: [u8; 32],
    /// Note commitment this operation acts on (identity for note-free operations).
    pub commitment: [u8; 32],
    /// Note nullifier this operation acts on (identity for note-free operations).
    pub nullifier: [u8; 32],
    /// Expected call target.
    pub target: [u8; 20],
    /// Expected native value in wei.
    pub value: u128,
    /// Operation amount in base units (deposit, approval, wrap, or unwrap amount).
    pub amount: u128,
}

/// One locally proved wallet call ready for fenced submission.
pub struct PreparedWalletCall {
    /// Operation kind.
    pub kind: WalletOperationKind,
    /// Deposit commitment or withdrawal nullifier.
    pub identity: [u8; 32],
    /// Note commitment the call acts on.
    pub commitment: [u8; 32],
    /// Note nullifier the call acts on.
    pub nullifier: [u8; 32],
    /// Exact target contract for this call (pool, token, or wrapper).
    pub target: [u8; 20],
    /// Exact native value in wei. Nonzero only for a wrap.
    pub value: u128,
    /// Amount in base units.
    pub amount: u128,
    /// Exact call calldata. Contains no witness.
    pub calldata: Vec<u8>,
    /// Installation wall time for public artifacts.
    pub installation_ms: u128,
    /// Local proving wall time.
    pub proving_ms: u128,
}

fn proving_value(witness: &crate::NoteWitness) -> Result<Value, OperationError> {
    serde_json::to_value(&witness.input).map_err(|_| OperationError::Input)
}

/// Creates or resumes one deposit for `amount` of `asset`.
///
/// The fresh note opening is random, persisted before proving, and reused on retry. The owner
/// key must be the participant's suite-2 agreement key so a later private transfer can spend it.
#[allow(clippy::too_many_arguments)]
pub async fn prepare_deposit(
    release: &ArtifactRelease,
    options: &InstallOptions,
    wallet: &WalletStore,
    operations: &OperationStore,
    domain: WalletDomain,
    asset: [u8; 20],
    owner: [u8; 64],
    verifier_version: u32,
    amount: u128,
    now: u64,
) -> Result<PreparedWalletCall, OperationError> {
    if amount == 0 || asset == [0; 20] {
        return Err(OperationError::Shape("deposit amount or asset"));
    }
    let pending: Vec<_> = operations
        .pending()?
        .into_iter()
        .filter(|operation| operation.kind == WalletOperationKind::Deposit)
        .collect();
    if pending.len() > 1 {
        return Err(OperationError::Retained);
    }
    let note = match pending.first() {
        Some(operation) => {
            let snapshot = wallet.snapshot()?;
            let note = snapshot
                .notes()
                .iter()
                .find(|note| note.commitment() == operation.commitment())
                .cloned()
                .ok_or(OperationError::Conflict)?;
            if note.asset() != asset || note.amount() != amount || note.owner() != owner {
                return Err(OperationError::Conflict);
            }
            note
        }
        None => {
            let note = OwnedNote::new(
                asset,
                amount,
                owner,
                random_field()?,
                random_field()?,
                domain
                    .note_domain()
                    .map_err(|_| OperationError::Shape("deposit note domain"))?,
            )
            .map_err(|_| OperationError::Shape("deposit note opening"))?;
            // Persist the opening before the operation record. A crash between the two leaves
            // an unreferenced note, which is harmless; the reverse order would leave a record
            // whose note is missing and make the operation unrecoverable.
            wallet.update(|snapshot| snapshot.add(note.clone()))?;
            operations.idempotent_begin(
                &OperationSpec {
                    kind: WalletOperationKind::Deposit,
                    identity: note.commitment(),
                    commitment: note.commitment(),
                    nullifier: note.nullifier(),
                    target: domain.pool,
                    value: 0,
                    amount,
                },
                now,
            )?;
            note
        }
    };
    let witness = build_deposit_witness(&note, domain, verifier_version)?;
    let expected = witness.public_inputs.clone();
    let installed = release
        .prove(
            CircuitKind::Deposit,
            options,
            proving_value(&witness)?,
            expected,
        )
        .await?;
    Ok(PreparedWalletCall {
        kind: WalletOperationKind::Deposit,
        identity: note.commitment(),
        commitment: note.commitment(),
        nullifier: note.nullifier(),
        target: domain.pool,
        value: 0,
        amount,
        calldata: deposit_calldata(&installed.proof)?,
        installation_ms: installed.installation_ms,
        proving_ms: installed.proving_ms,
    })
}

/// Creates or resumes one withdrawal of a confirmed local note to a public recipient.
#[allow(clippy::too_many_arguments)]
pub async fn prepare_withdraw(
    release: &ArtifactRelease,
    options: &InstallOptions,
    wallet: &WalletStore,
    operations: &OperationStore,
    index: &PoolIndex,
    domain: WalletDomain,
    verifier_version: u32,
    commitment: [u8; 32],
    recipient: [u8; 20],
    now: u64,
) -> Result<PreparedWalletCall, OperationError> {
    if recipient == [0; 20] {
        return Err(OperationError::Shape("withdrawal recipient"));
    }
    let snapshot = wallet.snapshot()?;
    let note = snapshot
        .notes()
        .iter()
        .find(|note| note.commitment() == commitment)
        .ok_or(OperationError::Shape("unknown withdrawal note"))?;
    let identity = note.nullifier();
    let amount = note.amount();
    let existing = operations.get(&identity)?;
    if existing.is_none() {
        let binding = withdraw_binding(&identity, &commitment);
        let reserved = wallet.update(|wallet| {
            wallet.reserve(&commitment, binding)?;
            Ok(())
        });
        match reserved {
            Ok(()) => {}
            Err(WalletError::Note(_)) => return Err(OperationError::Retained),
            Err(error) => return Err(error.into()),
        }
        operations.idempotent_begin(
            &OperationSpec {
                kind: WalletOperationKind::Withdraw,
                identity,
                commitment,
                nullifier: identity,
                target: domain.pool,
                value: 0,
                amount,
            },
            now,
        )?;
    }
    let witness = build_withdraw_witness(
        &snapshot,
        index,
        &commitment,
        domain,
        verifier_version,
        recipient,
    )?;
    let expected = witness.public_inputs.clone();
    let installed = release
        .prove(
            CircuitKind::Withdraw,
            options,
            proving_value(&witness)?,
            expected,
        )
        .await?;
    Ok(PreparedWalletCall {
        kind: WalletOperationKind::Withdraw,
        identity,
        commitment,
        nullifier: identity,
        target: domain.pool,
        value: 0,
        amount,
        calldata: withdraw_calldata(&installed.proof)?,
        installation_ms: installed.installation_ms,
        proving_ms: installed.proving_ms,
    })
}

/// Creates or resumes one ERC-20 approval for the pool.
///
/// Approval is idempotent and carries no note; the operation identity binds chain, pool,
/// asset, and amount so a later amount needs its own operation. The call is not sent here.
pub fn prepare_approve(
    operations: &OperationStore,
    chain_id: u64,
    pool: [u8; 20],
    asset: [u8; 20],
    amount: u128,
    now: u64,
) -> Result<PreparedWalletCall, OperationError> {
    prepare_approve_request(operations, chain_id, pool, asset, amount, None, now)
}

/// Explicit caller references allow a new approval after an older allowance was consumed.
/// Reusing a reference must also pass the caller's durable policy binding.
pub fn prepare_approve_request(
    operations: &OperationStore,
    chain_id: u64,
    pool: [u8; 20],
    asset: [u8; 20],
    amount: u128,
    operation_ref: Option<[u8; 32]>,
    now: u64,
) -> Result<PreparedWalletCall, OperationError> {
    if chain_id == 0 || pool == [0; 20] || asset == [0; 20] || amount == 0 {
        return Err(OperationError::Shape("approval identity"));
    }
    if operation_ref == Some([0; 32]) {
        return Err(OperationError::Shape("approval reference"));
    }
    let identity = if let Some(reference) = operation_ref {
        keccak256(&[
            b"erebus/shielded/wallet-approve/v2",
            &chain_id.to_be_bytes(),
            &pool,
            &asset,
            &amount.to_be_bytes(),
            &reference,
        ])
    } else {
        keccak256(&[
            b"erebus/shielded/wallet-approve/v1",
            &chain_id.to_be_bytes(),
            &pool,
            &asset,
            &amount.to_be_bytes(),
        ])
    };
    operations.idempotent_begin(
        &OperationSpec {
            kind: WalletOperationKind::Approve,
            identity,
            commitment: identity,
            nullifier: identity,
            target: asset,
            value: 0,
            amount,
        },
        now,
    )?;
    Ok(PreparedWalletCall {
        kind: WalletOperationKind::Approve,
        identity,
        commitment: identity,
        nullifier: identity,
        target: asset,
        value: 0,
        amount,
        calldata: erebus_evm::abi::encode_approve_call(&pool, amount),
        installation_ms: 0,
        proving_ms: 0,
    })
}

#[cfg(test)]
mod approval_reference_tests {
    use super::*;
    #[test]
    fn explicit_approval_reference_can_renew_the_same_amount_without_changing_a_retained_call() {
        let root = tempfile::tempdir().unwrap();
        let store = OperationStore::open(root.path()).unwrap();
        let first = prepare_approve_request(&store, 31337, [1; 20], [2; 20], 100, Some([3; 32]), 1)
            .unwrap();
        let retry = prepare_approve_request(&store, 31337, [1; 20], [2; 20], 100, Some([3; 32]), 2)
            .unwrap();
        let renewed =
            prepare_approve_request(&store, 31337, [1; 20], [2; 20], 100, Some([4; 32]), 3)
                .unwrap();
        assert_eq!(first.identity, retry.identity);
        assert_ne!(first.identity, renewed.identity);
        assert_eq!(first.calldata, renewed.calldata);
        assert!(
            prepare_approve_request(&store, 31337, [1; 20], [2; 20], 100, Some([0; 32]), 4)
                .is_err()
        );
    }
}

/// The exact calldata of one native wrap: `deposit()` on the wrapper.
#[must_use]
pub fn wrap_calldata() -> Vec<u8> {
    vec![0xd0, 0xe3, 0x0d, 0xb0]
}

/// The exact calldata of one native unwrap: `withdraw(amount)` on the wrapper.
#[must_use]
pub fn unwrap_calldata(amount: u128) -> Vec<u8> {
    let mut data = vec![0x2e, 0x1a, 0x7d, 0x4d];
    let mut word = [0u8; 32];
    word[16..].copy_from_slice(&amount.to_be_bytes());
    data.extend_from_slice(&word);
    data
}

fn native_wrapper_identity(
    domain: &[u8],
    chain_id: u64,
    wrapper: [u8; 20],
    amount: u128,
    operation_ref: [u8; 32],
) -> [u8; 32] {
    keccak256(&[
        domain,
        &chain_id.to_be_bytes(),
        &wrapper,
        &amount.to_be_bytes(),
        &operation_ref,
    ])
}

/// Creates or resumes one native wrap of exactly `amount` wei into the configured wrapper.
///
/// The call target is the wrapper and the transaction value equals the amount. It is not sent
/// here; submission and effect verification are separate.
pub fn prepare_wrap(
    operations: &OperationStore,
    chain_id: u64,
    wrapper: [u8; 20],
    amount: u128,
    operation_ref: [u8; 32],
    now: u64,
) -> Result<PreparedWalletCall, OperationError> {
    if chain_id == 0 || wrapper == [0; 20] || amount == 0 || operation_ref == [0; 32] {
        return Err(OperationError::Shape("wrap identity"));
    }
    let identity = native_wrapper_identity(
        b"erebus/shielded/wallet-wrap/v1",
        chain_id,
        wrapper,
        amount,
        operation_ref,
    );
    operations.idempotent_begin(
        &OperationSpec {
            kind: WalletOperationKind::Wrap,
            identity,
            commitment: identity,
            nullifier: identity,
            target: wrapper,
            value: amount,
            amount,
        },
        now,
    )?;
    Ok(PreparedWalletCall {
        kind: WalletOperationKind::Wrap,
        identity,
        commitment: identity,
        nullifier: identity,
        target: wrapper,
        value: amount,
        amount,
        calldata: wrap_calldata(),
        installation_ms: 0,
        proving_ms: 0,
    })
}

/// Creates or resumes one native unwrap of exactly `amount` base units from the wrapper.
///
/// The transaction value is zero and the calldata binds the exact amount. It is not sent here.
pub fn prepare_unwrap(
    operations: &OperationStore,
    chain_id: u64,
    wrapper: [u8; 20],
    amount: u128,
    operation_ref: [u8; 32],
    now: u64,
) -> Result<PreparedWalletCall, OperationError> {
    if chain_id == 0 || wrapper == [0; 20] || amount == 0 || operation_ref == [0; 32] {
        return Err(OperationError::Shape("unwrap identity"));
    }
    let identity = native_wrapper_identity(
        b"erebus/shielded/wallet-unwrap/v1",
        chain_id,
        wrapper,
        amount,
        operation_ref,
    );
    operations.idempotent_begin(
        &OperationSpec {
            kind: WalletOperationKind::Unwrap,
            identity,
            commitment: identity,
            nullifier: identity,
            target: wrapper,
            value: 0,
            amount,
        },
        now,
    )?;
    Ok(PreparedWalletCall {
        kind: WalletOperationKind::Unwrap,
        identity,
        commitment: identity,
        nullifier: identity,
        target: wrapper,
        value: 0,
        amount,
        calldata: unwrap_calldata(amount),
        installation_ms: 0,
        proving_ms: 0,
    })
}

/// What one submission attempt established. None of these states is finality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalletSubmissionState {
    /// The node acknowledged the transaction hash.
    Submitted,
    /// The outcome is unknown; observe or explicitly resume the retained bytes.
    Unknown,
    /// The provider rejected the transaction; the retained bytes may be sent explicitly.
    Rejected,
    /// A retained attempt exists and was not resent.
    Retained,
    /// The operation already completed; nothing was sent.
    Completed,
}

/// Submission result with the exact transaction identity and no witness material.
#[derive(Debug, Clone)]
pub struct WalletSubmission {
    state: WalletSubmissionState,
    transaction_hash: Option<[u8; 32]>,
}

impl WalletSubmission {
    /// What this attempt established.
    #[must_use]
    pub const fn state(&self) -> WalletSubmissionState {
        self.state
    }
    /// Locally computed hash of the exact bytes, once they exist.
    #[must_use]
    pub const fn transaction_hash(&self) -> Option<[u8; 32]> {
        self.transaction_hash
    }
}

fn operation_stub(domain: &DeploymentDomain, prepared: &PreparedWalletCall) -> PreparedSettlement {
    PreparedSettlement {
        operation_ref: prepared.identity,
        deal_commitment: DealCommitment::from_bytes(prepared.commitment),
        deal_nullifier: DealNullifier::from_bytes(prepared.nullifier),
        domain: domain.clone(),
        mode: erebus_core::terms::SettlementMode::Shielded,
        required_guarantees: GuaranteeSet::from_bits(SHIELDED_GUARANTEES)
            .expect("fixed shielded guarantees"),
        backend_evidence: prepared.calldata.clone(),
    }
}

/// Sends a prepared wallet call at most once.
///
/// `resume` is the only path allowed to resend bytes after an unknown outcome; the automatic
/// path never does. A rejected attempt stays retryable with the same retained bytes. The
/// caller persists the nonce claim, plan, and raw bytes before any network I/O.
#[allow(clippy::too_many_arguments)]
pub async fn submit_wallet_call(
    chain: &EvmChain,
    journal: &SignerJournal,
    operations: &OperationStore,
    domain: &DeploymentDomain,
    prepared: &PreparedWalletCall,
    key: &TransactionKey,
    fees: Eip1559Fees,
    gas_limit: u64,
    now: u64,
    resume: bool,
) -> Result<WalletSubmission, OperationError> {
    submit_with_gas(
        chain,
        journal,
        operations,
        domain,
        prepared,
        key,
        fees,
        Gas::Fixed(gas_limit),
        now,
        resume,
    )
    .await
}

/// Sends a prepared wallet call at most once, at its method's gas limit.
///
/// Identical to [`submit_wallet_call`] except for the gas limit of a new claim: an existing
/// claim for this exact call resumes at its saved limit when the policy admits it (a legacy
/// plan within the cap); otherwise the exact call, with its value, is estimated first and
/// refused with [`OperationError::GasLimit`] before any claim is written or byte is signed.
#[allow(clippy::too_many_arguments)]
pub async fn submit_wallet_call_with_policy(
    chain: &EvmChain,
    journal: &SignerJournal,
    operations: &OperationStore,
    domain: &DeploymentDomain,
    prepared: &PreparedWalletCall,
    key: &TransactionKey,
    fees: Eip1559Fees,
    policy: GasPolicy,
    now: u64,
    resume: bool,
) -> Result<WalletSubmission, OperationError> {
    submit_with_gas(
        chain,
        journal,
        operations,
        domain,
        prepared,
        key,
        fees,
        Gas::Policy(policy),
        now,
        resume,
    )
    .await
}

#[derive(Clone, Copy)]
enum Gas {
    Fixed(u64),
    Policy(GasPolicy),
}

/// The gas limit a new or resumed claim for this exact call uses. Reads the journal and, only
/// without a saved claim, one estimate. Writes nothing.
pub async fn planned_gas_limit(
    chain: &EvmChain,
    journal: &SignerJournal,
    stub: &PreparedSettlement,
    target: [u8; 20],
    calldata: &[u8],
    value: u128,
    policy: GasPolicy,
) -> Result<u64, OperationError> {
    if let Some(plan) = journal.resume_call(chain.deployment(), stub, target, calldata)? {
        let saved = plan.params().gas_limit;
        if !policy.resumes(saved) {
            return Err(OperationError::PlanAboveCap {
                saved,
                cap: policy.legacy_cap,
            });
        }
        return Ok(saved);
    }
    let estimate = chain
        .estimate_call_gas_with_value(journal.account_address(), target, calldata, value)
        .await
        .map_err(|_| OperationError::GasEstimate)?;
    if !policy.admits(estimate) {
        return Err(OperationError::GasLimit {
            estimate,
            limit: policy.limit,
        });
    }
    Ok(policy.limit)
}

#[allow(clippy::too_many_arguments)]
async fn submit_with_gas(
    chain: &EvmChain,
    journal: &SignerJournal,
    operations: &OperationStore,
    domain: &DeploymentDomain,
    prepared: &PreparedWalletCall,
    key: &TransactionKey,
    fees: Eip1559Fees,
    gas: Gas,
    now: u64,
    resume: bool,
) -> Result<WalletSubmission, OperationError> {
    chain.deployment().matches_domain(domain)?;
    let Some(operation) = operations.get(&prepared.identity)? else {
        return Err(OperationError::Conflict);
    };
    if operation.kind != prepared.kind {
        return Err(OperationError::Conflict);
    }
    if operation.state == WalletOperationState::Completed {
        return Ok(WalletSubmission {
            state: WalletSubmissionState::Completed,
            transaction_hash: None,
        });
    }
    // Explicit value policy: only a wrap may carry native value, and it must equal the
    // amount; every other explicit operation requires zero value.
    match prepared.kind {
        WalletOperationKind::Wrap => {
            if prepared.value == 0 || prepared.value != prepared.amount {
                return Err(OperationError::Shape("wrap value"));
            }
        }
        WalletOperationKind::Deposit
        | WalletOperationKind::Withdraw
        | WalletOperationKind::Approve
        | WalletOperationKind::Unwrap => {
            if prepared.value != 0 {
                return Err(OperationError::Shape("unexpected native value"));
            }
        }
    }
    if let Some(retained) = operations.retained_bytes(&prepared.identity)? {
        if operation.state == WalletOperationState::BroadcastUnknown && !resume {
            return Ok(WalletSubmission {
                state: WalletSubmissionState::Retained,
                transaction_hash: None,
            });
        }
        let plan = SigningPlan::decode(&retained.plan).map_err(|_| OperationError::Journal)?;
        // Signed bytes saved under an older, larger gas limit are only sent when the policy still
        // admits that limit. Otherwise the bytes and the claim stay exactly as they are.
        if let Gas::Policy(policy) = gas {
            if !policy.resumes(plan.params().gas_limit) {
                return Err(OperationError::PlanAboveCap {
                    saved: plan.params().gas_limit,
                    cap: policy.legacy_cap,
                });
            }
        }
        let target = prepared.target;
        let result = chain
            .broadcast_call(&plan, &retained.raw, target, &prepared.calldata)
            .await?;
        match result.outcome {
            BroadcastOutcome::Acknowledged => {
                return Ok(WalletSubmission {
                    state: WalletSubmissionState::Submitted,
                    transaction_hash: Some(result.hash),
                });
            }
            BroadcastOutcome::Rejected { .. } => {
                operations.record_rejected(&prepared.identity, now)?;
                return Ok(WalletSubmission {
                    state: WalletSubmissionState::Rejected,
                    transaction_hash: Some(result.hash),
                });
            }
            BroadcastOutcome::Unknown => {
                operations.record_attempt(&prepared.identity, result.hash, now)?;
                return Ok(WalletSubmission {
                    state: WalletSubmissionState::Unknown,
                    transaction_hash: Some(result.hash),
                });
            }
        }
    }
    if prepared.target == [0; 20] {
        return Err(OperationError::Shape("wallet call target"));
    }
    let stub = operation_stub(domain, prepared);
    let target = prepared.target;
    let gas_limit = match gas {
        Gas::Fixed(limit) => limit,
        Gas::Policy(policy) => {
            planned_gas_limit(
                chain,
                journal,
                &stub,
                target,
                &prepared.calldata,
                prepared.value,
                policy,
            )
            .await?
        }
    };
    let plan = chain
        .reserve_nonce_for_call_with_value(
            journal,
            &stub,
            target,
            &prepared.calldata,
            prepared.value,
            fees,
            gas_limit,
        )
        .await?;
    let raw = plan
        .sign_call(
            chain.deployment().chain_id,
            target,
            prepared.calldata.clone(),
            key,
        )?
        .raw()
        .to_vec();
    operations.record_signed(&prepared.identity, &plan.encode(), &raw, now)?;
    operations.record_attempt(
        &prepared.identity,
        SignedTransaction::from_raw(&raw)?.hash(),
        now,
    )?;
    let result = chain
        .broadcast_call(&plan, &raw, target, &prepared.calldata)
        .await?;
    match result.outcome {
        BroadcastOutcome::Acknowledged => Ok(WalletSubmission {
            state: WalletSubmissionState::Submitted,
            transaction_hash: Some(result.hash),
        }),
        BroadcastOutcome::Rejected { .. } => {
            operations.record_rejected(&prepared.identity, now)?;
            Ok(WalletSubmission {
                state: WalletSubmissionState::Rejected,
                transaction_hash: Some(result.hash),
            })
        }
        BroadcastOutcome::Unknown => Ok(WalletSubmission {
            state: WalletSubmissionState::Unknown,
            transaction_hash: Some(result.hash),
        }),
    }
}

/// Resends the exact retained bytes of an open operation after an unknown outcome.
///
/// This never re-signs, re-plans, proves, or reads a witness or key. Only the exact persisted
/// transaction may be sent, and only when explicitly requested by an operator.
pub async fn resume_wallet_call(
    chain: &EvmChain,
    operations: &OperationStore,
    identity: &[u8; 32],
    now: u64,
) -> Result<WalletSubmission, OperationError> {
    resume_with_gas(chain, operations, identity, None, now).await
}

/// [`resume_wallet_call`] that first requires the retained plan's gas limit to be the
/// operation's method limit or within `legacy_cap`. An over-cap plan is refused before any
/// attempt is recorded or byte is sent.
pub async fn resume_wallet_call_with_policy(
    chain: &EvmChain,
    operations: &OperationStore,
    identity: &[u8; 32],
    legacy_cap: u64,
    now: u64,
) -> Result<WalletSubmission, OperationError> {
    resume_with_gas(chain, operations, identity, Some(legacy_cap), now).await
}

/// The gas policy of one wallet method: its fixed limit (withdrawals still sign at the
/// configured cap) with `legacy_cap` for plans saved before per-method limits.
#[must_use]
pub fn method_gas_policy(kind: WalletOperationKind, legacy_cap: u64) -> GasPolicy {
    use crate::gas_policy::{
        APPROVE_GAS_LIMIT, DEPOSIT_GAS_LIMIT, UNWRAP_GAS_LIMIT, WRAP_GAS_LIMIT,
    };
    let limit = match kind {
        WalletOperationKind::Approve => APPROVE_GAS_LIMIT,
        WalletOperationKind::Wrap => WRAP_GAS_LIMIT,
        WalletOperationKind::Unwrap => UNWRAP_GAS_LIMIT,
        WalletOperationKind::Deposit => DEPOSIT_GAS_LIMIT,
        WalletOperationKind::Withdraw => legacy_cap,
    };
    GasPolicy { limit, legacy_cap }
}

/// Refuses a saved plan whose gas limit the operation's policy does not admit. Writes nothing.
fn admit_saved_limit(
    legacy_cap: Option<u64>,
    kind: WalletOperationKind,
    saved: u64,
) -> Result<(), OperationError> {
    match legacy_cap.map(|cap| method_gas_policy(kind, cap)) {
        Some(policy) if !policy.resumes(saved) => Err(OperationError::PlanAboveCap {
            saved,
            cap: policy.legacy_cap,
        }),
        _ => Ok(()),
    }
}

async fn resume_with_gas(
    chain: &EvmChain,
    operations: &OperationStore,
    identity: &[u8; 32],
    legacy_cap: Option<u64>,
    now: u64,
) -> Result<WalletSubmission, OperationError> {
    let operation = operations.get(identity)?.ok_or(OperationError::Conflict)?;
    if operation.state == WalletOperationState::Completed {
        return Err(OperationError::Conflict);
    }
    let Some(retained) = operations.retained_bytes(identity)? else {
        return Err(OperationError::Conflict);
    };
    let plan = SigningPlan::decode(&retained.plan)?;
    admit_saved_limit(legacy_cap, operation.kind, plan.params().gas_limit)?;
    let signed = SignedTransaction::from_raw(&retained.raw)?;
    let target = signed.to();
    if signed.sender() != plan.sender() || target == [0; 20] {
        return Err(OperationError::Conflict);
    }
    let calldata = signed.input().to_vec();
    operations.record_attempt(identity, signed.hash(), now)?;
    let result = chain
        .broadcast_call(&plan, &retained.raw, target, &calldata)
        .await?;
    match result.outcome {
        BroadcastOutcome::Acknowledged => Ok(WalletSubmission {
            state: WalletSubmissionState::Submitted,
            transaction_hash: Some(result.hash),
        }),
        BroadcastOutcome::Rejected { .. } => {
            operations.record_rejected(identity, now)?;
            Ok(WalletSubmission {
                state: WalletSubmissionState::Rejected,
                transaction_hash: Some(result.hash),
            })
        }
        BroadcastOutcome::Unknown => Ok(WalletSubmission {
            state: WalletSubmissionState::Unknown,
            transaction_hash: Some(result.hash),
        }),
    }
}

/// Replaces the retained transaction with the same call and higher fee caps.
///
/// Only the fee fields may change: sender, nonce, chain, target, value, calldata, and gas limit
/// are preserved by the replacement validator. The replacement is persisted before broadcast,
/// every older attempt hash remains in the history, and this is never automatic.
pub async fn replace_wallet_call(
    chain: &EvmChain,
    operations: &OperationStore,
    identity: &[u8; 32],
    key: &TransactionKey,
    fees: Option<Eip1559Fees>,
    now: u64,
) -> Result<WalletSubmission, OperationError> {
    replace_with_gas(chain, operations, identity, key, fees, None, now).await
}

/// [`replace_wallet_call`] that first requires the retained plan's gas limit (which a
/// replacement keeps) to be the operation's method limit or within `legacy_cap`; an over-cap
/// plan is refused before any new bytes or attempts are persisted.
pub async fn replace_wallet_call_with_policy(
    chain: &EvmChain,
    operations: &OperationStore,
    identity: &[u8; 32],
    key: &TransactionKey,
    fees: Option<Eip1559Fees>,
    legacy_cap: u64,
    now: u64,
) -> Result<WalletSubmission, OperationError> {
    replace_with_gas(
        chain,
        operations,
        identity,
        key,
        fees,
        Some(legacy_cap),
        now,
    )
    .await
}

async fn replace_with_gas(
    chain: &EvmChain,
    operations: &OperationStore,
    identity: &[u8; 32],
    key: &TransactionKey,
    fees: Option<Eip1559Fees>,
    legacy_cap: Option<u64>,
    now: u64,
) -> Result<WalletSubmission, OperationError> {
    let operation = operations.get(identity)?.ok_or(OperationError::Conflict)?;
    if operation.state == WalletOperationState::Completed {
        return Ok(WalletSubmission {
            state: WalletSubmissionState::Completed,
            transaction_hash: None,
        });
    }
    let Some(retained) = operations.retained_bytes(identity)? else {
        return Err(OperationError::Shape("no signed transaction to replace"));
    };
    let previous = SignedTransaction::from_raw(&retained.raw)?;
    admit_saved_limit(legacy_cap, operation.kind, previous.params().gas_limit)?;
    admit_saved_limit(
        legacy_cap,
        operation.kind,
        SigningPlan::decode(&retained.plan)?.params().gas_limit,
    )?;
    let next_fees = match fees {
        Some(fees) => fees,
        None => previous.params().fees.replacement_floor()?,
    };
    let plan = SigningPlan::new(
        previous.sender(),
        TransactionParams {
            nonce: previous.params().nonce,
            fees: next_fees,
            gas_limit: previous.params().gas_limit,
            value: previous.params().value,
        },
    )?;
    let raw = plan
        .sign_call(
            chain.deployment().chain_id,
            previous.to(),
            previous.input().to_vec(),
            key,
        )?
        .raw()
        .to_vec();
    // Validates same sender/nonce/chain/target/value/calldata/gas and a fee bump before
    // anything is persisted or sent.
    operations.record_replacement(identity, &plan.encode(), &raw, now)?;
    let hash = SignedTransaction::from_raw(&raw)?.hash();
    operations.record_attempt(identity, hash, now)?;
    let result = chain
        .broadcast_call(&plan, &raw, previous.to(), previous.input())
        .await?;
    match result.outcome {
        BroadcastOutcome::Acknowledged => Ok(WalletSubmission {
            state: WalletSubmissionState::Submitted,
            transaction_hash: Some(result.hash),
        }),
        BroadcastOutcome::Rejected { .. } => {
            operations.record_rejected(identity, now)?;
            Ok(WalletSubmission {
                state: WalletSubmissionState::Rejected,
                transaction_hash: Some(result.hash),
            })
        }
        BroadcastOutcome::Unknown => Ok(WalletSubmission {
            state: WalletSubmissionState::Unknown,
            transaction_hash: Some(result.hash),
        }),
    }
}

/// Completes durable operations whose verified chain evidence is present in the wallet.
///
/// Deposit completion needs observed insertion. Withdrawals complete only in finalized
/// receipt reconciliation, because another operation may have consumed the same note.
/// No transaction is created or sent.
pub fn reconcile_operations(
    wallet: &WalletStore,
    operations: &OperationStore,
    now: u64,
) -> Result<Vec<WalletOperationView>, OperationError> {
    let snapshot = wallet.snapshot()?;
    let mut completed = Vec::new();
    for operation in operations.pending()? {
        let note = snapshot
            .notes()
            .iter()
            .find(|note| note.commitment() == operation.commitment());
        let done = match operation.kind {
            WalletOperationKind::Deposit => note.is_some_and(|note| note.inclusion().is_some()),
            WalletOperationKind::Withdraw
            | WalletOperationKind::Approve
            | WalletOperationKind::Wrap
            | WalletOperationKind::Unwrap => false,
        };
        if !done {
            continue;
        }
        operations.complete(&operation.identity(), now)?;
        completed.push(operation);
    }
    Ok(completed)
}

/// Applies finalized canonical receipts to retained wallet operations.
///
/// A finalized reverted transaction clears its retained bytes and returns the operation to
/// `Planned`; a reverted withdrawal also releases its local note reservation. A finalized
/// successful approval completes. Withdrawals also require observed consumption matching
/// this exact successful attempt; deposits still require observed insertion.
/// Nothing changes before the receipt's block is
/// final, and pending or missing receipts are not evidence.
pub async fn reconcile_finalized_operations(
    chain: &EvmChain,
    context: &OperationContext,
    wallet: &WalletStore,
    operations: &OperationStore,
    now: u64,
) -> Result<ReconcileReport, OperationError> {
    use erebus_evm::chain::TxStatus;

    if chain.deployment().chain_id != context.chain_id
        || chain.deployment().settlement_contract != context.pool
    {
        return Err(OperationError::Conflict);
    }
    let finalized = chain.finalized_block().await?;
    let mut report = ReconcileReport::default();
    for operation in operations.pending()? {
        let Some(retained) = operations.retained_bytes(&operation.identity())? else {
            continue;
        };
        let plan = SigningPlan::decode(&retained.plan).map_err(|_| OperationError::Journal)?;
        let signed = SignedTransaction::from_raw(&retained.raw)?;
        // Reconcile every retained attempt, not only the active one: an older replacement may
        // be the transaction that reached the chain while a newer attempt is still pending.
        let mut hashes: Vec<[u8; 32]> = operation
            .attempts()
            .iter()
            .map(|attempt| attempt.tx_hash)
            .collect();
        hashes.push(signed.hash());
        hashes.sort_unstable();
        hashes.dedup();
        let mut operation_changed = false;
        for tx_hash in hashes {
            let TxStatus::Included(inclusion) = chain.observe(tx_hash).await? else {
                continue;
            };
            if inclusion.block.number > finalized.number
                || inclusion.from != plan.sender()
                || inclusion.nonce != plan.params().nonce
            {
                continue;
            }
            let bound = match receipt_binding(&operation, &signed, &inclusion, context) {
                Ok(bound) => bound,
                Err(OperationError::Migration) => {
                    report.migration_required.push(operation.clone());
                    break;
                }
                Err(error) => return Err(error),
            };
            if !bound {
                // A receipt that does not match the durable intent is not evidence for this
                // operation. Leave the record untouched; a human must reconcile it.
                continue;
            }
            if !inclusion.success {
                if operations.record_finalized_revert(
                    &operation.identity(),
                    tx_hash,
                    inclusion.block.number,
                    now,
                )? {
                    // Only the active attempt's finalized revert frees the reservation: if an
                    // older replacement reverted while a newer attempt is pending, the newest
                    // transaction still cannot reuse the consumed nonce, but its own outcome
                    // must decide. The active attempt reverting is terminal for this nonce.
                    let active_reverted =
                        operations
                            .get(&operation.identity())?
                            .is_some_and(|current| {
                                current.state() == WalletOperationState::Planned
                                    && !current.retained()
                            });
                    if active_reverted && operation.kind() == WalletOperationKind::Withdraw {
                        let binding =
                            withdraw_binding(&operation.identity(), &operation.commitment());
                        wallet_release_for(wallet, &operation, binding)?;
                    }
                    operation_changed = true;
                }
                continue;
            }
            let completed = match operation.kind() {
                WalletOperationKind::Approve => true,
                WalletOperationKind::Wrap => wrapper_event_matches(&operation, &inclusion, true),
                WalletOperationKind::Unwrap => wrapper_event_matches(&operation, &inclusion, false),
                WalletOperationKind::Withdraw => {
                    let snapshot = wallet.snapshot()?;
                    let note = snapshot.notes().iter().find(|note| {
                        note.commitment() == operation.commitment()
                            && note.nullifier() == operation.nullifier()
                    });
                    let matches = note.and_then(OwnedNote::consumption).is_some_and(|spent| {
                        spent.tx_hash == tx_hash
                            && spent.block_number == inclusion.block.number
                            && spent.block_hash == inclusion.block.hash
                    });
                    if matches {
                        let binding =
                            withdraw_binding(&operation.identity(), &operation.commitment());
                        match note.and_then(OwnedNote::reserved) {
                            Some(reserved) if reserved == binding => wallet.update(|wallet| {
                                wallet.conclude_reservation(&operation.commitment(), binding)
                            })?,
                            Some(_) => return Err(OperationError::Conflict),
                            // A crash may have cleared the reservation before the operation
                            // completion was persisted. The same receipt still authorizes it.
                            None => {}
                        }
                    }
                    matches
                }
                WalletOperationKind::Deposit => false,
            };
            if completed {
                operations.record_finalized_success(
                    &operation.identity(),
                    tx_hash,
                    Some(inclusion.block.number),
                    now,
                )?;
                operation_changed = true;
                break;
            }
        }
        if operation_changed {
            report.changed.push(operation);
        }
    }
    Ok(report)
}

/// Deployment context used to reconstruct expectations for legacy operation records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationContext {
    /// EVM chain ID.
    pub chain_id: u64,
    /// Shielded pool address.
    pub pool: [u8; 20],
    /// Verifier version bound into deposit and withdrawal public inputs.
    pub verifier_version: u32,
    /// Pool ERC-20 asset address.
    pub asset: [u8; 20],
}

/// Result of applying finalized receipts.
#[derive(Debug, Default)]
pub struct ReconcileReport {
    /// Operations completed or returned to a retryable plan by finalized evidence.
    pub changed: Vec<WalletOperationView>,
    /// Legacy records whose expectations cannot be reconstructed; manual reconciliation needed.
    pub migration_required: Vec<WalletOperationView>,
}

fn wallet_release_for(
    wallet: &WalletStore,
    operation: &WalletOperationView,
    binding: [u8; 32],
) -> Result<(), OperationError> {
    wallet
        .update(|wallet| wallet.release(&operation.commitment(), binding))
        .map_err(OperationError::from)
}

fn word_at(input: &[u8], index: usize) -> Option<&[u8]> {
    let start = index.checked_mul(32)?;
    input.get(start..start + 32)
}

fn word_is_address(word: &[u8], address: [u8; 20]) -> bool {
    word.len() == 32 && word[..12].iter().all(|byte| *byte == 0) && word[12..] == address
}

fn word_is_u64(word: &[u8], value: u64) -> bool {
    word.len() == 32
        && word[..24].iter().all(|byte| *byte == 0)
        && word[24..] == value.to_be_bytes()
}

/// Reconstructs the expected call for a legacy record and checks the exact signed transaction.
///
/// Records written before the operation spec stored no target or calldata hash. Raw
/// self-consistency alone would accept an altered retained transaction, so the decoded call is
/// validated against the durable intent: the pool/asset target, the selector, and the public
/// input that carries the note commitment or nullifier. Anything that cannot be reconstructed
/// fails closed as a migration.
fn receipt_binding(
    operation: &WalletOperationView,
    signed: &SignedTransaction,
    inclusion: &erebus_evm::chain::Inclusion,
    context: &OperationContext,
) -> Result<bool, OperationError> {
    if operation.target() != [0; 20] {
        return Ok(inclusion.to == Some(operation.target())
            && inclusion.value == operation.value()
            && keccak256(&[signed.input()]) == operation.calldata_hash());
    }
    // Legacy record: value must be zero (nothing else could have been signed then), and the
    // transaction must belong to the configured chain.
    if inclusion.value != 0 || signed.chain_id() != context.chain_id {
        return Ok(false);
    }
    let input = signed.input();
    let selector: [u8; 4] = input
        .get(..4)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(OperationError::Migration)?;
    let proof_words = 8;
    match operation.kind() {
        WalletOperationKind::Deposit => {
            if signed.to() != context.pool || selector != DEPOSIT_SELECTOR {
                return Ok(false);
            }
            let Some(public) = input.get(4 + proof_words * 32..) else {
                return Ok(false);
            };
            Ok(
                word_is_u64(word_at(public, 0).unwrap_or(&[]), context.chain_id)
                    && word_at(public, 1).is_some_and(|word| word_is_address(word, context.pool))
                    && word_at(public, 2)
                        .is_some_and(|word| word_is_u64(word, u64::from(context.verifier_version)))
                    && word_at(public, 3).is_some_and(|word| word_is_address(word, context.asset))
                    && word_at(public, 5).is_some_and(|word| word == operation.identity()),
            )
        }
        WalletOperationKind::Withdraw => {
            if signed.to() != context.pool || selector != WITHDRAW_SELECTOR {
                return Ok(false);
            }
            let Some(public) = input.get(4 + proof_words * 32..) else {
                return Ok(false);
            };
            Ok(
                word_is_u64(word_at(public, 0).unwrap_or(&[]), context.chain_id)
                    && word_at(public, 1).is_some_and(|word| word_is_address(word, context.pool))
                    && word_at(public, 2)
                        .is_some_and(|word| word_is_u64(word, u64::from(context.verifier_version)))
                    && word_at(public, 3).is_some_and(|word| word_is_address(word, context.asset))
                    && word_at(public, 5).is_some_and(|word| word == operation.nullifier()),
            )
        }
        WalletOperationKind::Approve => {
            if signed.to() != context.asset || selector != APPROVE_SELECTOR {
                return Ok(false);
            }
            Ok(input
                .get(4..36)
                .is_some_and(|word| word_is_address(word, context.pool)))
        }
        WalletOperationKind::Wrap | WalletOperationKind::Unwrap => Err(OperationError::Migration),
    }
}

const DEPOSIT_SELECTOR: [u8; 4] = [0xb6, 0xb5, 0xf1, 0xa6];
const WITHDRAW_SELECTOR: [u8; 4] = [0x91, 0x03, 0x5a, 0x9e];
const APPROVE_SELECTOR: [u8; 4] = [0x09, 0x5e, 0xa7, 0xb3];

/// Requires exactly one wrapper mint (`Deposit`) or burn (`Withdrawal`) event for the
/// participant, with the exact amount, emitted by the configured wrapper address.
fn wrapper_event_matches(
    operation: &WalletOperationView,
    inclusion: &erebus_evm::chain::Inclusion,
    mint: bool,
) -> bool {
    use erebus_evm::chain::MAX_INCLUSION_LOGS;
    if inclusion.logs_truncated || inclusion.logs.len() > MAX_INCLUSION_LOGS {
        return false;
    }
    let topic = if mint {
        keccak256(&[b"Deposit(address,uint256)"])
    } else {
        keccak256(&[b"Withdrawal(address,uint256)"])
    };
    let mut participant_word = [0u8; 32];
    participant_word[12..].copy_from_slice(&inclusion.from);
    let mut amount_word = [0u8; 32];
    amount_word[16..].copy_from_slice(&operation.amount().to_be_bytes());
    let mut matches = 0usize;
    for log in &inclusion.logs {
        if log.address != operation.target()
            || log.topics.len() != 2
            || log.topics[0] != topic
            || log.topics[1] != participant_word
            || log.data != amount_word
        {
            continue;
        }
        matches += 1;
    }
    matches == 1
}

/// Parses any 32-byte hex digest, with an optional `0x` prefix.
///
/// Block hashes and SHA-256 manifest digests are arbitrary 256-bit strings, not field
/// elements; they must not be reduced or rejected by a field bound.
pub fn parse_digest32(text: &str) -> Result<[u8; 32], OperationError> {
    let digits = text.strip_prefix("0x").unwrap_or(text);
    if digits.len() != 64
        || !digits
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(OperationError::Input);
    }
    hex::decode(digits)
        .map_err(|_| OperationError::Input)?
        .try_into()
        .map_err(|_| OperationError::Input)
}

/// Parses a canonical 32-byte BN254 field from a hex string.
pub fn parse_field(text: &str) -> Result<[u8; 32], OperationError> {
    use ark_ff::{BigInteger, PrimeField};
    let digits = text.strip_prefix("0x").unwrap_or(text);
    if digits.len() != 64
        || !digits
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(OperationError::Input);
    }
    let bytes: [u8; 32] = hex::decode(digits)
        .map_err(|_| OperationError::Input)?
        .try_into()
        .map_err(|_| OperationError::Input)?;
    let value = Fr::from_be_bytes_mod_order(&bytes);
    let raw = value.into_bigint().to_bytes_be();
    let mut round = [0u8; 32];
    round[32 - raw.len()..].copy_from_slice(&raw);
    if round != bytes {
        return Err(OperationError::Input);
    }
    Ok(bytes)
}

/// Reads a 20-byte lowercase address from a hex string.
pub fn parse_address(text: &str) -> Result<[u8; 20], OperationError> {
    let digits = text.strip_prefix("0x").unwrap_or(text);
    if digits.len() != 40
        || !digits
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(OperationError::Input);
    }
    hex::decode(digits)
        .map_err(|_| OperationError::Input)?
        .try_into()
        .map_err(|_| OperationError::Input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use erebus_evm::chain::{
        build_signed_call, BlockRef, Inclusion, InclusionLog, TransactionParams,
    };

    fn identity(seed: u8) -> [u8; 32] {
        [seed; 32]
    }

    #[test]
    fn consumed_note_without_an_own_receipt_does_not_complete_withdrawal() {
        use crate::wallet::{NoteConsumption, NoteInclusion};

        let vector: Value = serde_json::from_str(include_str!(
            "../../core/tests/fixtures/m5-note-vector.json"
        ))
        .unwrap();
        let bytes = |value: &Value| hex::decode(value.as_str().unwrap()).unwrap();
        let domain = WalletDomain {
            chain_id: vector["chainId"].as_str().unwrap().parse().unwrap(),
            pool: bytes(&vector["poolHex"]).try_into().unwrap(),
            verifier_version: 2,
        };
        let note = OwnedNote::new(
            bytes(&vector["assetHex"]).try_into().unwrap(),
            150,
            bytes(&vector["input"]["ownerHex"]).try_into().unwrap(),
            bytes(&vector["input"]["spendSecretHex"])
                .try_into()
                .unwrap(),
            bytes(&vector["input"]["saltHex"]).try_into().unwrap(),
            domain.note_domain().unwrap(),
        )
        .unwrap();
        let commitment = note.commitment();
        let nullifier = note.nullifier();
        let binding = withdraw_binding(&nullifier, &commitment);
        let root = tempfile::tempdir().unwrap();
        let wallet =
            WalletStore::new(root.path().join("private/notes.enc"), domain, [7; 32]).unwrap();
        wallet
            .update(|snapshot| {
                snapshot.add(note)?;
                snapshot.observe_insertion(
                    &commitment,
                    NoteInclusion {
                        index: 0,
                        block_number: 10,
                        block_hash: [1; 32],
                        root: [2; 32],
                    },
                )?;
                snapshot.reserve(&commitment, binding)?;
                snapshot.observe_consumption(
                    &nullifier,
                    NoteConsumption {
                        block_number: 11,
                        block_hash: [3; 32],
                        tx_hash: [4; 32],
                    },
                )?;
                Ok(())
            })
            .unwrap();
        let operations = OperationStore::open(root.path().join("operations")).unwrap();
        operations
            .idempotent_begin(
                &OperationSpec {
                    kind: WalletOperationKind::Withdraw,
                    identity: nullifier,
                    commitment,
                    nullifier,
                    target: domain.pool,
                    value: 0,
                    amount: 150,
                },
                10,
            )
            .unwrap();
        assert!(reconcile_operations(&wallet, &operations, 12)
            .unwrap()
            .is_empty());
        let operation = operations.get(&nullifier).unwrap().unwrap();
        assert_eq!(operation.state(), WalletOperationState::Planned);
        assert!(operation.attempts().is_empty());
        let snapshot = wallet.snapshot().unwrap();
        assert_eq!(snapshot.notes()[0].reserved(), Some(binding));
        assert!(snapshot.spendable(&commitment).is_none());
    }

    #[test]
    fn call_encoders_bind_the_pinned_selectors_and_signal_counts() {
        assert_eq!(
            hex::encode(&keccak256(&[DEPOSIT_SIGNATURE])[..4]),
            "0def57f0"
        );
        assert_eq!(
            hex::encode(&keccak256(&[WITHDRAW_SIGNATURE])[..4]),
            "91035a9e"
        );
        assert_eq!(DEPOSIT_CALL_BYTES, 4 + 14 * 32);
        assert_eq!(WITHDRAW_CALL_BYTES, 4 + 16 * 32);
    }

    #[test]
    fn operation_journal_is_idempotent_and_fences_bytes() {
        let root = tempfile::tempdir().unwrap();
        let store = OperationStore::open(root.path().join("operations")).unwrap();
        let identity = identity(1);
        let spec = OperationSpec {
            kind: WalletOperationKind::Deposit,
            identity,
            commitment: identity,
            nullifier: identity,
            target: [5; 20],
            value: 0,
            amount: 10,
        };
        let first = store.idempotent_begin(&spec, 10).unwrap();
        assert_eq!(first.state(), WalletOperationState::Planned);
        let second = store.idempotent_begin(&spec, 11).unwrap();
        assert_eq!(second.state(), WalletOperationState::Planned);
        assert_eq!(second.identity(), identity);
        let mut conflicting = spec.clone();
        conflicting.amount = 11;
        assert!(store.idempotent_begin(&conflicting, 12).is_err());

        let (plan, raw, _) = signed_fixture(identity);
        let (other_plan, other_raw, _) = signed_fixture([9; 32]);
        store.record_signed(&identity, &plan, &raw, 13).unwrap();
        assert_eq!(
            store.get(&identity).unwrap().unwrap().state(),
            WalletOperationState::Signed
        );
        // A different plan can never replace retained bytes.
        assert!(store
            .record_signed(&identity, &other_plan, &other_raw, 14)
            .is_err());
        // The attempt fence precedes any network I/O and binds one transaction hash.
        store.record_attempt(&identity, [0xaa; 32], 15).unwrap();
        assert_eq!(
            store.get(&identity).unwrap().unwrap().state(),
            WalletOperationState::BroadcastUnknown
        );
        assert!(store.record_attempt(&identity, [0xbb; 32], 16).is_err());
        // A finalized revert clears the retained bytes and allows one explicit retry.
        // An unknown hash is not evidence and changes nothing; only known attempts act.
        assert!(!store
            .record_finalized_revert(&identity, [0xbb; 32], 5, 17)
            .unwrap());
        assert!(store
            .record_finalized_revert(&identity, [0xaa; 32], 5, 18)
            .unwrap());
        let reset = store.get(&identity).unwrap().unwrap();
        assert_eq!(reset.state(), WalletOperationState::Planned);
        assert!(!reset.retained());
        assert!(store.retained_bytes(&identity).unwrap().is_none());
        // Rejection returns to Signed; completion is terminal.
        store.record_signed(&identity, &plan, &raw, 19).unwrap();
        store.record_attempt(&identity, [0xaa; 32], 20).unwrap();
        store.record_rejected(&identity, 21).unwrap();
        assert_eq!(
            store.get(&identity).unwrap().unwrap().state(),
            WalletOperationState::Signed
        );
        store.complete(&identity, 22).unwrap();
        assert!(store.pending().unwrap().is_empty());
    }

    fn word_bytes(value: &[u8]) -> [u8; 32] {
        let mut word = [0u8; 32];
        word[32 - value.len()..].copy_from_slice(value);
        word
    }

    fn signed_call(chain_id: u64, to: [u8; 20], input: Vec<u8>) -> SignedTransaction {
        let key = TransactionKey::from_bytes(&[9; 32]).unwrap();
        let plan = SigningPlan::new(
            key.address(),
            TransactionParams {
                nonce: 0,
                fees: Eip1559Fees::new(1_000_000_000, 1_000_000).unwrap(),
                gas_limit: 100_000,
                value: 0,
            },
        )
        .unwrap();
        build_signed_call(&key, chain_id, to, input, plan.params()).unwrap()
    }

    fn withdraw_input(
        chain_id: u64,
        pool: [u8; 20],
        version: u32,
        asset: [u8; 20],
        nullifier: [u8; 32],
    ) -> Vec<u8> {
        let mut input = WITHDRAW_SELECTOR.to_vec();
        input.extend_from_slice(&[0u8; 8 * 32]);
        input.extend_from_slice(&word_bytes(&chain_id.to_be_bytes()));
        input.extend_from_slice(&word_bytes(&pool));
        input.extend_from_slice(&word_bytes(&u64::from(version).to_be_bytes()));
        input.extend_from_slice(&word_bytes(&asset));
        input.extend_from_slice(&word_bytes(&[4u8; 32]));
        input.extend_from_slice(&word_bytes(&nullifier));
        input.extend_from_slice(&word_bytes(&[6u8; 20]));
        input.extend_from_slice(&word_bytes(&7u64.to_be_bytes()));
        input
    }

    fn legacy_view(identity: [u8; 32], nullifier: [u8; 32]) -> WalletOperationView {
        WalletOperationView {
            kind: WalletOperationKind::Withdraw,
            identity,
            commitment: [1; 32],
            nullifier,
            state: WalletOperationState::BroadcastUnknown,
            retained: true,
            target: [0; 20],
            value: 0,
            amount: 0,
            calldata_hash: [0; 32],
            attempts: Vec::new(),
        }
    }

    fn inclusion_for(
        signed: &SignedTransaction,
        logs: Vec<InclusionLog>,
        value: u128,
    ) -> erebus_evm::chain::Inclusion {
        Inclusion {
            block: BlockRef {
                number: 10,
                hash: [3; 32],
                timestamp: 1,
            },
            success: true,
            from: signed.sender(),
            to: Some(signed.to()),
            nonce: signed.params().nonce,
            value,
            logs,
            logs_truncated: false,
            transaction_index: 0,
        }
    }

    #[test]
    fn legacy_records_are_reconstructed_from_intent_not_raw_self_consistency() {
        let pool = [7; 20];
        let asset = [8; 20];
        let context = OperationContext {
            chain_id: 31337,
            pool,
            verifier_version: 2,
            asset,
        };
        let nullifier = [9; 32];
        let input = withdraw_input(31337, pool, 2, asset, nullifier);
        let signed = signed_call(31337, pool, input.clone());
        let view = legacy_view(nullifier, nullifier);
        assert!(receipt_binding(
            &view,
            &signed,
            &inclusion_for(&signed, Vec::new(), 0),
            &context
        )
        .unwrap());
        // An altered retained transaction with a different public nullifier is refused.
        let altered_input = withdraw_input(31337, pool, 2, asset, [10; 32]);
        let altered = signed_call(31337, pool, altered_input);
        assert!(!receipt_binding(
            &view,
            &altered,
            &inclusion_for(&altered, Vec::new(), 0),
            &context
        )
        .unwrap());
        // A wrong target, version, asset, or chain is refused rather than migrated.
        let wrong_target = signed_call(31337, [11; 20], input.clone());
        assert!(!receipt_binding(
            &view,
            &wrong_target,
            &inclusion_for(&wrong_target, Vec::new(), 0),
            &context
        )
        .unwrap());
        let wrong_chain = signed_call(1, pool, input.clone());
        assert!(!receipt_binding(
            &view,
            &wrong_chain,
            &inclusion_for(&wrong_chain, Vec::new(), 0),
            &context
        )
        .unwrap());
        // Nonzero legacy value is impossible and refused.
        assert!(!receipt_binding(
            &view,
            &signed,
            &inclusion_for(&signed, Vec::new(), 5),
            &context
        )
        .unwrap());
        // A wrapper kind cannot be a legacy record.
        let mut wrap_view = view.clone();
        wrap_view.kind = WalletOperationKind::Wrap;
        assert!(matches!(
            receipt_binding(
                &wrap_view,
                &signed,
                &inclusion_for(&signed, Vec::new(), 0),
                &context
            ),
            Err(OperationError::Migration)
        ));
    }

    #[test]
    fn wrapper_completion_requires_the_exact_mint_or_burn_event() {
        let wrapper = [7; 20];
        let signed = signed_call(31337, wrapper, super::wrap_calldata());
        let mut view = legacy_view([9; 32], [9; 32]);
        view.kind = WalletOperationKind::Wrap;
        view.target = wrapper;
        view.value = 5;
        view.amount = 5;
        let mut participant = [0u8; 32];
        participant[12..].copy_from_slice(&signed.sender());
        let mut amount = [0u8; 32];
        amount[16..].copy_from_slice(&5u128.to_be_bytes());
        let mint = InclusionLog {
            address: wrapper,
            topics: vec![keccak256(&[b"Deposit(address,uint256)"]), participant],
            data: amount.to_vec(),
            log_index: 0,
        };
        assert!(wrapper_event_matches(
            &view,
            &inclusion_for(&signed, vec![mint.clone()], 5),
            true
        ));
        // The burn topic does not authorize a wrap, and a same-block unrelated mint for another
        // participant does not either.
        assert!(!wrapper_event_matches(
            &view,
            &inclusion_for(&signed, vec![mint.clone()], 5),
            false
        ));
        let mut other = mint.clone();
        other.topics[1] = word_bytes(&[1u8; 20]);
        assert!(!wrapper_event_matches(
            &view,
            &inclusion_for(&signed, vec![other], 5),
            true
        ));
        // A wrong emitter, amount, or truncated receipt fails closed.
        let mut wrong_address = mint.clone();
        wrong_address.address = [8; 20];
        assert!(!wrapper_event_matches(
            &view,
            &inclusion_for(&signed, vec![wrong_address], 5),
            true
        ));
        let mut wrong_amount = mint.clone();
        wrong_amount.data = word_bytes(&6u128.to_be_bytes()).to_vec();
        assert!(!wrapper_event_matches(
            &view,
            &inclusion_for(&signed, vec![wrong_amount], 5),
            true
        ));
        let mut truncated = inclusion_for(&signed, vec![mint.clone()], 5);
        truncated.logs_truncated = true;
        assert!(!wrapper_event_matches(&view, &truncated, true));
        // Exactly one matching event is required; two are not evidence.
        assert!(!wrapper_event_matches(
            &view,
            &inclusion_for(&signed, vec![mint.clone(), mint], 5),
            true
        ));
    }

    #[test]
    fn native_wrapper_operations_bind_target_value_and_amount() {
        let root = tempfile::tempdir().unwrap();
        let store = OperationStore::open(root.path().join("operations")).unwrap();
        let wrapper = [7; 20];
        let wrap = prepare_wrap(&store, 31337, wrapper, 5, [1; 32], 10).unwrap();
        assert_eq!(wrap.target, wrapper);
        assert_eq!(wrap.value, 5);
        assert_eq!(wrap.amount, 5);
        assert_eq!(wrap.calldata, super::wrap_calldata());
        assert_eq!(
            prepare_wrap(&store, 31337, wrapper, 5, [1; 32], 11)
                .unwrap()
                .identity,
            wrap.identity
        );
        assert_ne!(
            prepare_wrap(&store, 31337, wrapper, 5, [2; 32], 12)
                .unwrap()
                .identity,
            wrap.identity
        );
        assert_ne!(
            prepare_wrap(&store, 31337, wrapper, 6, [1; 32], 13)
                .unwrap()
                .identity,
            wrap.identity
        );
        let unwrap = prepare_unwrap(&store, 31337, wrapper, 5, [1; 32], 14).unwrap();
        assert_eq!(unwrap.target, wrapper);
        assert_eq!(unwrap.value, 0);
        assert_eq!(unwrap.amount, 5);
        assert_eq!(unwrap.calldata, super::unwrap_calldata(5));
        assert_ne!(unwrap.identity, wrap.identity);
    }

    fn signed_fixture(identity: [u8; 32]) -> (Vec<u8>, Vec<u8>, [u8; 20]) {
        let key = TransactionKey::from_bytes(&[9; 32]).unwrap();
        let plan = SigningPlan::new(
            key.address(),
            TransactionParams {
                nonce: 0,
                fees: Eip1559Fees::new(1_000_000_000, 1_000_000).unwrap(),
                gas_limit: 100_000,
                value: 0,
            },
        )
        .unwrap();
        let raw = build_signed_call(&key, 31337, [5; 20], identity.to_vec(), plan.params())
            .unwrap()
            .raw()
            .to_vec();
        (plan.encode(), raw, key.address())
    }

    #[test]
    fn finalized_revert_retains_attempt_history_across_restart() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("operations");
        let store = OperationStore::open(&path).unwrap();
        let identity = identity(7);
        let (plan, raw, sender) = signed_fixture(identity);
        let spec = OperationSpec {
            kind: WalletOperationKind::Withdraw,
            identity,
            commitment: [1; 32],
            nullifier: identity,
            target: [5; 20],
            value: 0,
            amount: 9,
        };
        store.idempotent_begin(&spec, 1).unwrap();
        store.record_signed(&identity, &plan, &raw, 2).unwrap();
        let tx_hash = SignedTransaction::from_raw(&raw).unwrap().hash();
        store.record_attempt(&identity, tx_hash, 3).unwrap();
        assert!(store
            .record_finalized_revert(&identity, tx_hash, 44, 4)
            .unwrap());
        drop(store);
        let reopened = OperationStore::open(&path).unwrap();
        let operation = reopened.get(&identity).unwrap().unwrap();
        assert_eq!(operation.state(), WalletOperationState::Planned);
        assert!(!operation.retained());
        assert_eq!(operation.target(), [5; 20]);
        // The reverted attempt is updated in place, not duplicated.
        assert_eq!(operation.attempts().len(), 1);
        assert_eq!(
            operation.attempts().last().unwrap().outcome,
            WalletAttemptOutcome::Reverted
        );
        assert_eq!(operation.attempts().last().unwrap().block, Some(44));
        // A stale revert for the same hash cannot fire twice, and the plan can be rebuilt.
        assert!(!reopened
            .record_finalized_revert(&identity, tx_hash, 44, 5)
            .unwrap());
        // The signer recovered from the raw matches the plan sender.
        assert_eq!(SignedTransaction::from_raw(&raw).unwrap().sender(), sender);
    }

    impl FaultHook for TestSweepHook {
        fn after(&self, _boundary: erebus_journal::Boundary<'_>) -> std::io::Result<()> {
            let attempt = self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            if attempt == self.fail_at {
                Err(std::io::Error::other("injected boundary failure"))
            } else {
                Ok(())
            }
        }
    }

    struct TestSweepHook {
        fail_at: usize,
        count: std::sync::atomic::AtomicUsize,
    }

    #[test]
    fn operation_journal_write_faults_preserve_one_restartable_record() {
        let identity = identity(8);
        let (plan, raw, _) = signed_fixture(identity);
        let mut boundaries = 0;
        let mut fail = 0;
        loop {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("operations");
            let hook = Arc::new(TestSweepHook {
                fail_at: if fail == 0 { usize::MAX } else { fail },
                count: std::sync::atomic::AtomicUsize::new(0),
            });
            let store = OperationStore::with_faults(&path, hook.clone()).unwrap();
            let spec = OperationSpec {
                kind: WalletOperationKind::Deposit,
                identity,
                commitment: identity,
                nullifier: identity,
                target: [5; 20],
                value: 0,
                amount: 9,
            };
            let result = store
                .idempotent_begin(&spec, 1)
                .and_then(|_| store.record_signed(&identity, &plan, &raw, 2))
                .and_then(|_| store.record_attempt(&identity, [0x11; 32], 3));
            drop(store);
            let reopened = OperationStore::open(&path).unwrap();
            let operation = reopened.get(&identity).unwrap();
            if fail == 0 {
                result.unwrap();
                boundaries = hook.count.load(std::sync::atomic::Ordering::SeqCst);
                assert!(boundaries >= 2);
                assert!(operation.unwrap().retained());
            } else if let Some(operation) = operation {
                // A partially applied record is never inconsistent: retained bytes exist
                // together with a decodable plan, or the record is still only planned.
                if operation.retained() {
                    let retained = reopened.retained_bytes(&identity).unwrap().unwrap();
                    let plan = SigningPlan::decode(&retained.plan).unwrap();
                    let signed = SignedTransaction::from_raw(&retained.raw).unwrap();
                    assert_eq!(signed.sender(), plan.sender());
                    assert_eq!(operation.calldata_hash(), keccak256(&[signed.input()]));
                } else {
                    assert_eq!(operation.state(), WalletOperationState::Planned);
                }
            }
            fail += 1;
            if fail > boundaries {
                break;
            }
        }
    }

    #[test]
    fn approval_identity_binds_chain_pool_asset_and_amount() {
        let root = tempfile::tempdir().unwrap();
        let store = OperationStore::open(root.path().join("operations")).unwrap();
        let first = prepare_approve(&store, 31337, [1; 20], [2; 20], 100, 10).unwrap();
        assert_eq!(first.target, [2; 20]);
        assert_eq!(&first.calldata[..4], &[0x09, 0x5e, 0xa7, 0xb3]);
        assert_eq!(
            prepare_approve(&store, 31337, [1; 20], [2; 20], 100, 11)
                .unwrap()
                .identity,
            first.identity
        );
        assert_ne!(
            prepare_approve(&store, 31337, [1; 20], [2; 20], 101, 12)
                .unwrap()
                .identity,
            first.identity
        );
        assert_ne!(
            prepare_approve(&store, 31338, [1; 20], [2; 20], 100, 13)
                .unwrap()
                .identity,
            first.identity
        );
    }
}

#[cfg(test)]
mod gas_policy_resume_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A node that answers chain ID and counts every other request; it must see no send.
    async fn node() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        use axum::{extract::State, routing::post, Json, Router};
        let calls = Arc::new(AtomicUsize::new(0));
        async fn answer(
            State(calls): State<Arc<AtomicUsize>>,
            Json(request): Json<Value>,
        ) -> Json<Value> {
            if request["method"] == "eth_chainId" {
                return Json(
                    serde_json::json!({"jsonrpc":"2.0","id":request["id"],"result":"0x7a69"}),
                );
            }
            calls.fetch_add(1, Ordering::SeqCst);
            Json(serde_json::json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32000}}))
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/", post(answer))
            .with_state(calls.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (url, calls, task)
    }

    struct Retained {
        chain: EvmChain,
        operations: OperationStore,
        journal: SignerJournal,
        domain: DeploymentDomain,
        prepared: PreparedWalletCall,
        key: TransactionKey,
        fees: Eip1559Fees,
        plan: SigningPlan,
        raw: Vec<u8>,
        calls: Arc<AtomicUsize>,
        _task: tokio::task::JoinHandle<()>,
        _root: tempfile::TempDir,
    }

    /// An approval a pre-upgrade process claimed and signed at `gas_limit`, then stopped
    /// before sending: the signed bytes and the claim are retained, no attempt is recorded.
    async fn retained(gas_limit: u64) -> Retained {
        let (url, calls, task) = node().await;
        let pool = [0x11; 20];
        let asset = [0x22; 20];
        let key = TransactionKey::from_bytes(&[7; 32]).unwrap();
        let address = erebus_core::ids::AddressBytes::new(pool.to_vec()).unwrap();
        let namespace = erebus_core::ids::ChainNamespace::parse("eip155:31337").unwrap();
        let domain = DeploymentDomain {
            namespace: namespace.clone(),
            settlement_contract: Some(address.clone()),
            pool: Some(address),
            verifier_version: 2,
        };
        let chain = EvmChain::connect(
            erebus_evm::deployment::EvmDeployment::new(namespace, pool, 2, url).unwrap(),
            std::time::Duration::from_secs(5),
        )
        .await
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        let operations = OperationStore::open(root.path().join("ops")).unwrap();
        let journal =
            SignerJournal::open(root.path().join("signer"), 31337, key.address()).unwrap();
        let prepared =
            prepare_approve_request(&operations, 31337, pool, asset, 10, Some([3; 32]), 1).unwrap();
        let fees = Eip1559Fees::new(300_000_000_000, 2_000_000_000).unwrap();
        let plan = journal
            .reserve_call_with_value(
                chain.deployment(),
                &operation_stub(&domain, &prepared),
                asset,
                &prepared.calldata,
                0,
                4,
                fees,
                gas_limit,
            )
            .unwrap();
        let raw = plan
            .sign_call(31337, asset, prepared.calldata.clone(), &key)
            .unwrap()
            .raw()
            .to_vec();
        operations
            .record_signed(&prepared.identity, &plan.encode(), &raw, 2)
            .unwrap();
        Retained {
            chain,
            operations,
            journal,
            domain,
            prepared,
            key,
            fees,
            plan,
            raw,
            calls,
            _task: task,
            _root: root,
        }
    }

    fn assert_untouched(state: &Retained) {
        assert_eq!(
            state.calls.load(Ordering::SeqCst),
            0,
            "only the chain ID may be read"
        );
        let retained = state
            .operations
            .retained_bytes(&state.prepared.identity)
            .unwrap()
            .unwrap();
        assert_eq!(retained.raw, state.raw);
        assert_eq!(retained.plan, state.plan.encode());
        assert_eq!(state.journal.claim_nonce().unwrap(), Some(4));
        let operation = state
            .operations
            .get(&state.prepared.identity)
            .unwrap()
            .unwrap();
        assert!(operation.attempts().is_empty());
    }

    fn over_cap<T>(result: Result<T, OperationError>) -> bool {
        matches!(
            result,
            Err(OperationError::PlanAboveCap {
                saved: 5_000_000,
                cap: 3_000_000
            })
        )
    }

    #[tokio::test]
    async fn a_retained_signed_plan_above_the_cap_is_never_broadcast_and_stays_retained() {
        let state = retained(5_000_000).await;
        let refused = submit_wallet_call_with_policy(
            &state.chain,
            &state.journal,
            &state.operations,
            &state.domain,
            &state.prepared,
            &state.key,
            state.fees,
            GasPolicy {
                limit: crate::gas_policy::APPROVE_GAS_LIMIT,
                legacy_cap: 3_000_000,
            },
            3,
            false,
        )
        .await;
        assert!(over_cap(refused));
        assert_untouched(&state);
    }

    #[tokio::test]
    async fn an_explicit_resume_of_an_over_cap_plan_records_and_sends_nothing() {
        let state = retained(5_000_000).await;
        let refused = resume_wallet_call_with_policy(
            &state.chain,
            &state.operations,
            &state.prepared.identity,
            3_000_000,
            3,
        )
        .await;
        assert!(over_cap(refused));
        assert_untouched(&state);
    }

    #[tokio::test]
    async fn a_replacement_of_an_over_cap_plan_persists_no_new_bytes_or_attempt() {
        let state = retained(5_000_000).await;
        let refused = replace_wallet_call_with_policy(
            &state.chain,
            &state.operations,
            &state.prepared.identity,
            &state.key,
            None,
            3_000_000,
            3,
        )
        .await;
        assert!(over_cap(refused));
        assert_untouched(&state);
    }

    #[tokio::test]
    async fn a_legacy_plan_within_the_cap_still_resumes() {
        let state = retained(3_000_000).await;
        let resumed = resume_wallet_call_with_policy(
            &state.chain,
            &state.operations,
            &state.prepared.identity,
            3_000_000,
            3,
        )
        .await
        .unwrap();
        // The mock node rejects the send; what matters is that it was attempted.
        assert_eq!(resumed.state(), WalletSubmissionState::Rejected);
        assert!(state.calls.load(Ordering::SeqCst) >= 1);
    }
}
