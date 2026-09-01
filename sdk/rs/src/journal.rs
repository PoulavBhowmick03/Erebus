//! Durable record of what each write operation actually did.
//!
//! The journal is the only thing standing between a crash mid-write and a duplicate
//! on-chain effect. Nothing here decides *what* to do about a half-finished operation —
//! that is reconciliation's job. This module records facts and refuses to lose them.
//!
//! Storage mirrors [`crate::state`]: one file per record under a `0700` directory, `0600`
//! on the files, an advisory lock per record, and replacement by atomic rename. It adds one
//! thing `state` does not do: the parent directory is synced after the rename, because a
//! rename that survives only in the page cache is exactly the durability this module exists
//! to provide.

use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starknet_types_core::felt::Felt;

use crate::operation::{OperationId, RequestBinding, WriteOperation};
use crate::rpc::Receipt;
use crate::state::ChannelHandle;

/// Journal record schema version. A record written by a newer SDK fails closed.
pub const JOURNAL_VERSION: u32 = 4;

/// Oldest schema this SDK can classify. Version 1 has no replayable request or completion
/// plan, so it remains readable but cannot be rebuilt automatically.
const MIN_READABLE_JOURNAL_VERSION: u32 = 1;

/// Directory holding operation records, relative to the identity state directory.
const JOURNAL_DIR: &str = "operations";

/// How far one attempt at an operation got.
///
/// The ordering is not decorative. A stage names what is already true on the chain or on
/// disk, so reconciliation reads it to decide whether an effect may still be pending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStage {
    /// The id is reserved and bound to its parameters. Nothing has been read or proven.
    Claimed,
    /// Preflight reads completed and the action set is built. Nothing proven.
    Prepared,
    /// A proof exists, anchored to a specific block.
    Proven,
    /// A transaction is signed and its hash is known. Not yet handed to the RPC.
    Signed,
    /// Handed to the RPC. The chain outcome is unknown and an effect may exist.
    Submitted,
    /// The chain accepted it. The effect exists.
    Accepted,
    /// The requested effect and result are durably reflected locally.
    ///
    /// Most committed operations reached [`Self::Accepted`] first. A request whose effect
    /// already existed can move directly from [`Self::Claimed`] once that no-op result is
    /// recorded; it must not remain an indefinitely claimed write.
    Committed,
    /// The chain rejected it. No effect exists.
    Reverted,
    /// Reconciliation could not classify this attempt. Requires an operator.
    NeedsAttention,
}

impl OperationStage {
    /// Whether the attempt is finished and will not advance on its own.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Committed | Self::Reverted)
    }

    /// Whether an on-chain effect may exist for this attempt.
    ///
    /// True from [`Self::Signed`] onwards. `Signed` is included because a process can die
    /// after the RPC accepted the bytes but before it durably records `Submitted`.
    pub const fn may_have_landed(self) -> bool {
        matches!(
            self,
            Self::Signed
                | Self::Submitted
                | Self::Accepted
                | Self::Committed
                | Self::NeedsAttention
        )
    }

    /// Whether `next` is a legal successor of `self`.
    ///
    /// Written as an explicit table rather than a rank comparison: the interesting edges
    /// are the ones that are not a straight line, and a table makes an illegal edge a
    /// review question rather than an arithmetic accident.
    pub const fn can_advance_to(self, next: Self) -> bool {
        match (self, next) {
            // `Claimed -> Committed` is an idempotent no-chain result: for example, opening
            // a channel that already exists with the requested peer and wire version.
            (Self::Claimed, Self::Prepared | Self::Committed)
            // `Prepared -> Signed` is the no-proof path: the ERC-20 approve that must land
            // before a charged `apply_actions` is an ordinary account call with nothing to
            // prove. Every pool state transition goes the long way round.
            | (Self::Prepared, Self::Proven | Self::Signed)
            | (Self::Proven, Self::Signed)
            | (Self::Signed, Self::Submitted)
            | (Self::Submitted, Self::Accepted | Self::Reverted)
            | (Self::Accepted, Self::Committed) => true,
            // Reconciliation may resolve an unclassified attempt in either direction once
            // it has read the receipt.
            (Self::NeedsAttention, Self::Accepted | Self::Reverted | Self::Committed) => true,
            // Any attempt that has not finished may be escalated to an operator.
            (from, Self::NeedsAttention) => !from.is_terminal(),
            _ => false,
        }
    }
}

/// One attempt to carry an operation through to a chain effect.
///
/// Recovery mode 2 in plan.md rebuilds an expired proof under the same operation id, which
/// starts a new attempt rather than mutating the old one. The old attempt is retained: it
/// holds the transaction hash that reconciliation must still prove never landed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attempt {
    /// How far this attempt got.
    pub stage: OperationStage,
    /// Unix seconds when the attempt started.
    pub started_at: u64,
    /// Unix seconds of the last stage change.
    pub updated_at: u64,
    /// Block the proof was anchored to.
    pub proving_block: Option<u64>,
    /// Last block at which the proof is still accepted, read live from the pool.
    pub valid_until_block: Option<u64>,
    /// SHA-256 commitment to the same-block `compile_actions` output.
    ///
    /// A recovered hosted proof must match this before it can be submitted. Older records do
    /// not have it and therefore fail closed into the ordinary rebuild path.
    #[serde(default)]
    pub simulation_hash: Option<String>,
    /// Hash of the signed transaction, persisted before submission.
    pub transaction_hash: Option<Felt>,
    /// Account nonce the transaction was signed against.
    ///
    /// Reconciliation needs this to tell "not on chain yet" from "can never be on chain".
    /// A missing receipt proves nothing on its own; a missing receipt while the account has
    /// moved past this nonce proves the transaction was never included.
    #[serde(default)]
    pub account_nonce: Option<Felt>,
    /// Whether the exact signed transaction is stored beside this record.
    ///
    /// The transaction itself lives in its own file because it carries the proof blob, and a
    /// record that reconciliation must scan should stay small enough to read cheaply.
    #[serde(default)]
    pub transaction_stored: bool,
    /// Live funding and proof-validity inputs read before proof generation.
    #[serde(default)]
    pub prepared: Option<PreparedSnapshot>,
    /// Result recipe and idempotent local-state mutation fixed before submission.
    #[serde(default)]
    pub completion: Option<Value>,
    /// Accepted or reverted receipt, persisted before the matching terminal stage.
    #[serde(default)]
    pub receipt: Option<Receipt>,
    /// Unix seconds from the accepted Starknet block. Versions before 3 wrote the local
    /// wall clock here, so reconciliation ignores their value and derives it from the
    /// receipt's block number.
    pub accepted_at: Option<u64>,
}

impl Attempt {
    fn new(stage: OperationStage, now: u64) -> Self {
        Self {
            stage,
            started_at: now,
            updated_at: now,
            proving_block: None,
            valid_until_block: None,
            simulation_hash: None,
            transaction_hash: None,
            account_nonce: None,
            transaction_stored: false,
            prepared: None,
            completion: None,
            receipt: None,
            accepted_at: None,
        }
    }
}

/// Live prepared-stage values that explain why proving was allowed to start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedSnapshot {
    /// Deposit pulled in addition to the pool fee.
    pub deposit: String,
    /// Pool-owned proof-validity window read at call time.
    pub proof_validity_blocks: u64,
    /// Pool fee read at call time.
    pub fee_per_write: String,
    /// Allowance read at call time.
    pub allowance: String,
    /// Public token balance read at call time.
    pub public_balance: String,
}

/// The durable record for one operation id.
///
/// The record has no stage field of its own. Its stage is the stage of its latest attempt,
/// so the two can never contradict each other on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationRecord {
    /// Schema version.
    pub version: u32,
    /// The caller-supplied id this record belongs to.
    pub operation_id: OperationId,
    /// Which write was asked for.
    pub operation: WriteOperation,
    /// Fingerprint of the canonical request parameters.
    pub binding: RequestBinding,
    /// Canonical replayable request. Missing only on readable legacy version-1 records.
    #[serde(default)]
    pub request: Option<Value>,
    /// Channel this write belongs to, when it has one.
    ///
    /// `open_channel` has no handle yet when the id is claimed, and the funding writes have
    /// no channel of their own, so this is absent for them. It exists so reconciliation can
    /// name the local record an accepted-but-uncommitted operation left behind.
    #[serde(default)]
    pub channel: Option<ChannelHandle>,
    /// Unix seconds when the id was first claimed.
    pub created_at: u64,
    /// Final method result, persisted before the operation becomes committed.
    #[serde(default)]
    pub result: Option<Value>,
    /// Every attempt, oldest first. Never empty.
    pub attempts: Vec<Attempt>,
}

impl OperationRecord {
    /// The latest attempt.
    pub fn attempt(&self) -> &Attempt {
        self.attempts
            .last()
            .expect("a record is created with one attempt and attempts are only appended")
    }

    /// Stage of the latest attempt.
    pub fn stage(&self) -> OperationStage {
        self.attempt().stage
    }

    /// Whether any attempt may have produced an on-chain effect.
    pub fn may_have_landed(&self) -> bool {
        self.attempts
            .iter()
            .any(|attempt| attempt.stage.may_have_landed())
    }
}

/// Locked, on-disk store of operation records.
/// What a [`OperationJournal::prune`] sweep did, and what it deliberately left alone.
///
/// The retained counts are the useful half. A prune that silently kept things would be
/// indistinguishable from one that had nothing to do, and "the journal is not growing" is
/// exactly the belief an operator should not hold on faith.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct PruneReport {
    /// Records removed, with their lock files and stored transactions.
    pub pruned: usize,
    /// Kept because they are not terminal, so they may still need an explicit resume.
    /// `NeedsAttention` is counted here: it is not terminal precisely because a person still
    /// has to look at it.
    pub retained_unfinished: usize,
    /// Kept because they finished more recently than the retention window.
    pub retained_recent: usize,
    /// Kept because another process holds the record's lock. Pruning never waits.
    pub retained_locked: usize,
}

impl PruneReport {
    /// Total records the sweep looked at.
    pub fn examined(&self) -> usize {
        self.pruned + self.retained_unfinished + self.retained_recent + self.retained_locked
    }
}

/// Locked, on-disk store of operation records.
#[derive(Debug, Clone)]
pub struct OperationJournal {
    root: PathBuf,
}

impl OperationJournal {
    /// Opens or creates the journal below an identity state directory.
    pub fn new(state_dir: impl AsRef<Path>) -> Result<Self, JournalError> {
        let root = state_dir.as_ref().join(JOURNAL_DIR);
        std::fs::create_dir_all(&root).map_err(|source| JournalError::Io {
            path: root.clone(),
            source,
        })?;
        set_mode(&root, 0o700)?;
        Ok(Self { root })
    }

    /// Claims an operation id for a request, or reopens the record already under it.
    ///
    /// Returns [`JournalError::BindingConflict`] when the id exists under a different
    /// request. That check happens here, under the lock and before any chain work, because
    /// it is the one failure the whole journal exists to make cheap.
    pub fn claim(
        &self,
        operation_id: &OperationId,
        operation: WriteOperation,
        binding: RequestBinding,
        channel: Option<ChannelHandle>,
        now: u64,
    ) -> Result<OperationLease, JournalError> {
        self.claim_inner(operation_id, operation, binding, channel, None, now)
    }

    /// Claims an operation together with the canonical request needed for durable replay.
    pub fn claim_with_request(
        &self,
        operation_id: &OperationId,
        operation: WriteOperation,
        binding: RequestBinding,
        channel: Option<ChannelHandle>,
        request: Value,
        now: u64,
    ) -> Result<OperationLease, JournalError> {
        self.claim_inner(
            operation_id,
            operation,
            binding,
            channel,
            Some(request),
            now,
        )
    }

    fn claim_inner(
        &self,
        operation_id: &OperationId,
        operation: WriteOperation,
        binding: RequestBinding,
        channel: Option<ChannelHandle>,
        request: Option<Value>,
        now: u64,
    ) -> Result<OperationLease, JournalError> {
        let identity_lock = self.identity_lock()?;
        let lock = self.lock_file(operation_id)?;
        let path = self.record_path(operation_id);

        let record = match self.read(&path)? {
            Some(mut record) => {
                if record.binding != binding {
                    return Err(JournalError::BindingConflict {
                        operation_id: operation_id.clone(),
                        recorded: record.binding,
                        received: binding,
                    });
                }
                if record.operation_id != *operation_id {
                    return Err(JournalError::IdMismatch {
                        expected: operation_id.clone(),
                        found: record.operation_id,
                    });
                }
                if record.operation != operation {
                    return Err(JournalError::OperationConflict {
                        operation_id: operation_id.clone(),
                        recorded: record.operation,
                        received: operation,
                    });
                }
                match (&record.request, &request) {
                    (Some(recorded), Some(received)) if recorded != received => {
                        return Err(JournalError::RequestConflict {
                            operation_id: operation_id.clone(),
                        });
                    }
                    (None, Some(received)) if record.stage() == OperationStage::Claimed => {
                        record.version = JOURNAL_VERSION;
                        record.request = Some(received.clone());
                    }
                    _ => {}
                }
                record
            }
            None => OperationRecord {
                version: JOURNAL_VERSION,
                operation_id: operation_id.clone(),
                operation,
                binding,
                request,
                channel,
                created_at: now,
                result: None,
                attempts: vec![Attempt::new(OperationStage::Claimed, now)],
            },
        };
        if record.version == JOURNAL_VERSION {
            write_atomic(&self.root, &path, &record)?;
        }

        Ok(OperationLease {
            root: self.root.clone(),
            path,
            _identity_lock: identity_lock,
            _lock: lock,
            record,
        })
    }

    /// Locks and loads an existing record, or `None` if the id was never claimed.
    pub fn lock(&self, operation_id: &OperationId) -> Result<Option<OperationLease>, JournalError> {
        let path = self.record_path(operation_id);
        if !path.exists() {
            return Ok(None);
        }
        let identity_lock = self.identity_lock()?;
        let lock = self.lock_file(operation_id)?;
        let Some(record) = self.read(&path)? else {
            return Ok(None);
        };
        Ok(Some(OperationLease {
            root: self.root.clone(),
            path,
            _identity_lock: identity_lock,
            _lock: lock,
            record,
        }))
    }

    fn identity_lock(&self) -> Result<File, JournalError> {
        let path = self.root.join(".identity.lock");
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|source| JournalError::Io {
                path: path.clone(),
                source,
            })?;
        set_file_mode(&lock, &path, 0o600)?;
        lock.lock_exclusive()
            .map_err(|source| JournalError::Io { path, source })?;
        Ok(lock)
    }

    /// What a prune did, and what it deliberately left alone.
    ///
    /// The retained counts are the useful half. A prune that silently kept things would be
    /// indistinguishable from one that had nothing to do, and "the journal is not growing"
    /// is exactly the belief an operator should not hold on faith.
    ///
    /// Removing a record removes its lock file and every stored transaction with it, so the
    /// counts below describe whole operations rather than files.
    ///
    /// Deletes finished records older than `older_than_seconds`.
    ///
    /// Nothing here is a judgement call about disk space. The rule is narrow on purpose:
    ///
    /// - **Only terminal records.** `Committed` and `Reverted` are the two stages that will
    ///   never advance again. Everything else may still need an explicit resume, and a
    ///   record is the only thing that knows a transaction was signed.
    /// - **`NeedsAttention` is never pruned, at any age.** It is not terminal precisely
    ///   because a person still has to look at it; deleting it destroys the evidence they
    ///   were going to look at, and the operation would then reconcile as though it had
    ///   never happened.
    /// - **Age is measured from the last update**, not from the claim, so a long-running
    ///   operation is not pruned for having started early.
    ///
    /// Holds the identity write lock for the whole sweep, so a prune cannot race a write
    /// that is midway through claiming or advancing. A record whose own lock is held by
    /// another process is skipped rather than waited for: pruning is maintenance and must
    /// never block, or stall, a real operation.
    pub fn prune(&self, older_than_seconds: u64, now: u64) -> Result<PruneReport, JournalError> {
        let _identity_lock = self.identity_lock()?;
        let mut report = PruneReport::default();

        for record in self.records()? {
            if !record.stage().is_terminal() {
                report.retained_unfinished += 1;
                continue;
            }
            // Saturating: a clock that moved backwards must not underflow into "infinitely
            // old" and delete a record that was written seconds ago.
            let age = now.saturating_sub(record.attempt().updated_at);
            if age < older_than_seconds {
                report.retained_recent += 1;
                continue;
            }
            if self.remove(&record)? {
                report.pruned += 1;
            } else {
                report.retained_locked += 1;
            }
        }
        Ok(report)
    }

    /// Removes one record and everything filed under its id. `false` if it is locked.
    ///
    /// The record goes last. If the process dies mid-removal, what is left is a record whose
    /// stored transaction is missing, and the journal already treats that as a distinct,
    /// loud condition rather than as "never submitted". Removing the record first would
    /// instead leave orphan blobs that nothing knows the id of.
    fn remove(&self, record: &OperationRecord) -> Result<bool, JournalError> {
        let lock_path = self
            .root
            .join(format!("{}.lock", record.operation_id.as_str()));
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|source| JournalError::Io {
                path: lock_path.clone(),
                source,
            })?;
        if lock.try_lock_exclusive().is_err() {
            return Ok(false);
        }

        for index in 0..record.attempts.len() {
            let path = self
                .root
                .join(format!("{}.{index}.tx", record.operation_id.as_str()));
            remove_if_present(&path)?;
        }
        remove_if_present(&self.record_path(&record.operation_id))?;
        drop(lock);
        remove_if_present(&lock_path)?;
        sync_dir(&self.root)?;
        Ok(true)
    }

    /// Every record in the journal, in unspecified order.
    ///
    /// A single unreadable record fails the whole listing. Reconciliation that silently
    /// skipped a record it could not parse would report "nothing pending" for an operation
    /// that may have landed, which is the one answer that must never be guessed.
    pub fn records(&self) -> Result<Vec<OperationRecord>, JournalError> {
        let mut records = Vec::new();
        let entries = std::fs::read_dir(&self.root).map_err(|source| JournalError::Io {
            path: self.root.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| JournalError::Io {
                path: self.root.clone(),
                source,
            })?;
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            if let Some(record) = self.read(&path)? {
                records.push(record);
            }
        }
        Ok(records)
    }

    /// Takes an identity-wide read snapshot while excluding every write operation.
    ///
    /// A successful snapshot proves that no older process still holds the identity write
    /// lock. Keep the returned value alive while chain reconciliation uses its records.
    pub fn exclusive_snapshot(&self) -> Result<JournalSnapshot, JournalError> {
        let identity_lock = self.identity_lock()?;
        let records = self.records()?;
        Ok(JournalSnapshot {
            records,
            _identity_lock: identity_lock,
        })
    }

    fn read(&self, path: &Path) -> Result<Option<OperationRecord>, JournalError> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(JournalError::Io {
                    path: path.to_path_buf(),
                    source,
                })
            }
        };
        let record: OperationRecord =
            serde_json::from_reader(BufReader::new(file)).map_err(|source| JournalError::Json {
                path: path.to_path_buf(),
                source,
            })?;
        if !(MIN_READABLE_JOURNAL_VERSION..=JOURNAL_VERSION).contains(&record.version) {
            return Err(JournalError::UnsupportedVersion(record.version));
        }
        if record.attempts.is_empty() {
            return Err(JournalError::Corrupt {
                path: path.to_path_buf(),
                reason: "record has no attempts",
            });
        }
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| JournalError::Corrupt {
                path: path.to_path_buf(),
                reason: "record filename is not a valid operation id",
            })?;
        let expected = OperationId::parse(stem.to_owned()).map_err(|_| JournalError::Corrupt {
            path: path.to_path_buf(),
            reason: "record filename is not a valid operation id",
        })?;
        if record.operation_id != expected {
            return Err(JournalError::IdMismatch {
                expected,
                found: record.operation_id,
            });
        }
        Ok(Some(record))
    }

    fn lock_file(&self, operation_id: &OperationId) -> Result<File, JournalError> {
        let lock_path = self.root.join(format!("{}.lock", operation_id.as_str()));
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|source| JournalError::Io {
                path: lock_path.clone(),
                source,
            })?;
        set_file_mode(&lock, &lock_path, 0o600)?;
        lock.lock_exclusive().map_err(|source| JournalError::Io {
            path: lock_path,
            source,
        })?;
        Ok(lock)
    }

    fn record_path(&self, operation_id: &OperationId) -> PathBuf {
        self.root.join(format!("{}.json", operation_id.as_str()))
    }
}

/// Immutable journal records protected by the identity-wide write lock.
pub struct JournalSnapshot {
    records: Vec<OperationRecord>,
    _identity_lock: File,
}

impl JournalSnapshot {
    /// Records captured while no identity writer can run.
    pub fn records(&self) -> &[OperationRecord] {
        &self.records
    }
}

/// An exclusively locked operation record.
///
/// Every mutation writes through to disk immediately. A lease is held across submission, and
/// a stage change that lived only in memory until some later commit would be lost by exactly
/// the crash it is meant to survive.
pub struct OperationLease {
    root: PathBuf,
    path: PathBuf,
    _identity_lock: File,
    _lock: File,
    record: OperationRecord,
}

impl OperationLease {
    /// The current record.
    pub fn record(&self) -> &OperationRecord {
        &self.record
    }

    /// Advances the latest attempt and persists it before returning.
    pub fn advance(&mut self, stage: OperationStage, now: u64) -> Result<(), JournalError> {
        let current = self.record.stage();
        if !current.can_advance_to(stage) {
            return Err(JournalError::IllegalTransition {
                from: current,
                to: stage,
            });
        }
        let attempt = self
            .record
            .attempts
            .last_mut()
            .expect("attempts are never empty");
        attempt.stage = stage;
        attempt.updated_at = now;
        self.flush()
    }

    /// Records details on the latest attempt and persists them before returning.
    pub fn amend(&mut self, now: u64, edit: impl FnOnce(&mut Attempt)) -> Result<(), JournalError> {
        let attempt = self
            .record
            .attempts
            .last_mut()
            .expect("attempts are never empty");
        edit(attempt);
        attempt.updated_at = now;
        self.flush()
    }

    /// Persists the live funding and validity inputs before proof generation starts.
    pub fn record_prepared(
        &mut self,
        prepared: PreparedSnapshot,
        now: u64,
    ) -> Result<(), JournalError> {
        self.amend(now, |attempt| attempt.prepared = Some(prepared))
    }

    /// Persists the result recipe and local mutation before submission can happen.
    pub fn record_completion(&mut self, completion: Value, now: u64) -> Result<(), JournalError> {
        self.amend(now, |attempt| attempt.completion = Some(completion))
    }

    /// Persists a chain receipt before the corresponding lifecycle stage is advanced.
    pub fn record_receipt(&mut self, receipt: Receipt, now: u64) -> Result<(), JournalError> {
        let receipt_hash = Felt::from_hex(&receipt.transaction_hash).map_err(|_| {
            JournalError::ReceiptConflict {
                operation_id: self.record.operation_id.clone(),
                reason: "receipt transaction hash is not a felt".to_owned(),
            }
        })?;
        let attempt = self.record.attempt();
        if attempt.transaction_hash != Some(receipt_hash) {
            return Err(JournalError::ReceiptConflict {
                operation_id: self.record.operation_id.clone(),
                reason: "receipt transaction hash does not match the signed transaction".to_owned(),
            });
        }
        if attempt
            .receipt
            .as_ref()
            .is_some_and(|recorded| recorded != &receipt)
        {
            return Err(JournalError::ReceiptConflict {
                operation_id: self.record.operation_id.clone(),
                reason: "a different receipt is already recorded".to_owned(),
            });
        }
        self.amend(now, |attempt| attempt.receipt = Some(receipt))
    }

    /// Persists the final method result before marking local state committed.
    pub fn record_result(&mut self, result: Value, now: u64) -> Result<(), JournalError> {
        if self
            .record
            .result
            .as_ref()
            .is_some_and(|recorded| recorded != &result)
        {
            return Err(JournalError::ResultConflict {
                operation_id: self.record.operation_id.clone(),
            });
        }
        self.record.result = Some(result);
        self.record.version = JOURNAL_VERSION;
        self.record
            .attempts
            .last_mut()
            .expect("attempts are never empty")
            .updated_at = now;
        self.flush()
    }

    /// Starts a fresh attempt, retaining every earlier one.
    ///
    /// Only recovery calls this, and only after establishing that no earlier attempt can
    /// still produce an effect. A new attempt is a licence to build a *different*
    /// transaction, which is why it is refused outright once an effect exists: a rebuilt
    /// transaction has its own hash and nothing would stop it landing beside the first.
    ///
    /// [`OperationStage::Reverted`] is not a bar. A reverted transaction consumed its nonce
    /// and produced nothing, so trying again is exactly right.
    ///
    /// The new attempt starts at [`OperationStage::Claimed`], so the re-issued request walks
    /// the same stages as a first attempt and gets the same checks. Earlier attempts keep
    /// their transaction hashes, which is what can still prove they never landed.
    pub(crate) fn restart(&mut self, now: u64) -> Result<(), JournalError> {
        if self.record.stage() == OperationStage::Committed {
            return Err(JournalError::IllegalTransition {
                from: OperationStage::Committed,
                to: OperationStage::Claimed,
            });
        }
        self.record
            .attempts
            .push(Attempt::new(OperationStage::Claimed, now));
        self.flush()
    }

    /// Persists the exact signed transaction, then records its hash and moves to
    /// [`OperationStage::Signed`].
    ///
    /// This is the boundary the whole journal exists for. After it returns, a crash at any
    /// point can still discover that a transaction with this hash may be on the chain, and
    /// recovery can resubmit these exact bytes without changing the hash.
    ///
    /// The order is deliberate: the transaction file is written and synced first, and only
    /// then does the record claim a hash. A crash between the two leaves an orphan file and
    /// a record that still reads as unsubmitted, which is the safe direction to fail. The
    /// reverse order would leave a hash that nothing can resubmit.
    pub fn persist_signed(
        &mut self,
        transaction_hash: Felt,
        transaction: &str,
        now: u64,
    ) -> Result<(), JournalError> {
        let path = self.transaction_path(self.record.attempts.len() - 1);
        write_bytes_atomic(&self.root, &path, transaction.as_bytes())?;

        let attempt = self
            .record
            .attempts
            .last_mut()
            .expect("attempts are never empty");
        attempt.transaction_hash = Some(transaction_hash);
        attempt.transaction_stored = true;
        self.advance(OperationStage::Signed, now)
    }

    /// Reads back the exact signed transaction stored for one attempt.
    pub fn stored_transaction(&self, attempt_index: usize) -> Result<Option<String>, JournalError> {
        let Some(attempt) = self.record.attempts.get(attempt_index) else {
            return Ok(None);
        };
        if !attempt.transaction_stored {
            return Ok(None);
        }
        let path = self.transaction_path(attempt_index);
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(Some(text)),
            // The record says a transaction was stored and it is not there. Reconciliation
            // must not read that as "nothing was submitted".
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(JournalError::Corrupt {
                    path,
                    reason: "record claims a stored transaction that is missing",
                })
            }
            Err(source) => Err(JournalError::Io { path, source }),
        }
    }

    fn transaction_path(&self, attempt_index: usize) -> PathBuf {
        // Deliberately not `.json`: `records()` parses every `.json` file in the directory
        // as an operation record, and a transaction blob is not one.
        self.root.join(format!(
            "{}.{attempt_index}.tx",
            self.record.operation_id.as_str()
        ))
    }

    fn flush(&mut self) -> Result<(), JournalError> {
        write_atomic(&self.root, &self.path, &self.record)
    }
}

impl core::fmt::Debug for OperationLease {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("OperationLease")
            .field("record", &self.record)
            .finish_non_exhaustive()
    }
}

fn write_atomic(root: &Path, path: &Path, record: &OperationRecord) -> Result<(), JournalError> {
    let encoded = serde_json::to_vec(record).map_err(|source| JournalError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    write_bytes_atomic(root, path, &encoded)
}

fn write_bytes_atomic(root: &Path, path: &Path, bytes: &[u8]) -> Result<(), JournalError> {
    let temporary = root.join(format!(
        ".{}.{:016x}.tmp",
        std::process::id(),
        OsRng.next_u64(),
    ));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|source| JournalError::Io {
            path: temporary.clone(),
            source,
        })?;
    set_file_mode(&file, &temporary, 0o600)?;
    let mut writer = BufWriter::new(file);
    writer.write_all(bytes).map_err(|source| JournalError::Io {
        path: temporary.clone(),
        source,
    })?;
    writer.flush().map_err(|source| JournalError::Io {
        path: temporary.clone(),
        source,
    })?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|source| JournalError::Io {
            path: temporary.clone(),
            source,
        })?;
    std::fs::rename(&temporary, path).map_err(|source| JournalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    sync_dir(root)
}

/// Flushes the directory entry created by the rename.
///
/// Without this the file contents are durable but the name is not, so a crash can leave the
/// record at its previous version while the caller believes it advanced.
#[cfg(unix)]
/// Removes a path, treating "already gone" as success.
///
/// A prune interrupted partway through leaves some of an operation's files removed. Rerunning
/// it must finish the job rather than fail on the ones it already deleted.
fn remove_if_present(path: &Path) -> Result<(), JournalError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(JournalError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(unix)]
fn sync_dir(path: &Path) -> Result<(), JournalError> {
    File::open(path)
        .and_then(|dir| dir.sync_all())
        .map_err(|source| JournalError::Io {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(not(unix))]
fn sync_dir(_path: &Path) -> Result<(), JournalError> {
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), JournalError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(|source| {
        JournalError::Io {
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), JournalError> {
    Ok(())
}

#[cfg(unix)]
fn set_file_mode(file: &File, path: &Path, mode: u32) -> Result<(), JournalError> {
    use std::os::unix::fs::PermissionsExt;

    file.set_permissions(std::fs::Permissions::from_mode(mode))
        .map_err(|source| JournalError::Io {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(not(unix))]
fn set_file_mode(_file: &File, _path: &Path, _mode: u32) -> Result<(), JournalError> {
    Ok(())
}

/// Journal failure. Every variant fails closed: none of them may be read as "no effect".
#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    /// Filesystem failure.
    #[error("journal io error at {}: {source}", path.display())]
    Io {
        /// Path involved.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
    /// A record could not be parsed.
    #[error("journal record at {} is not readable: {source}", path.display())]
    Json {
        /// Path involved.
        path: PathBuf,
        /// Underlying error.
        source: serde_json::Error,
    },
    /// A record was written by an SDK with a different schema.
    #[error("journal record schema version {0} is not supported")]
    UnsupportedVersion(u32),
    /// A record contradicts itself.
    #[error("journal record at {} is corrupt: {reason}", path.display())]
    Corrupt {
        /// Path involved.
        path: PathBuf,
        /// What was wrong.
        reason: &'static str,
    },
    /// A record was found under the wrong id.
    #[error("journal record under {expected} claims to be {found}")]
    IdMismatch {
        /// Id that was asked for.
        expected: OperationId,
        /// Id the record carries.
        found: OperationId,
    },
    /// The id was already used for a different request.
    #[error("operation {operation_id} is already bound to a different request")]
    BindingConflict {
        /// The reused id.
        operation_id: OperationId,
        /// Binding stored against the id.
        recorded: RequestBinding,
        /// Binding of the request that collided with it.
        received: RequestBinding,
    },
    /// The digest matched but the canonical request did not.
    #[error("operation {operation_id} is already bound to different canonical parameters")]
    RequestConflict {
        /// Reused id.
        operation_id: OperationId,
    },
    /// The record names a different method even though its binding was reused.
    #[error("operation {operation_id} records {recorded:?}, not {received:?}")]
    OperationConflict {
        /// Reused id.
        operation_id: OperationId,
        /// Method already stored.
        recorded: WriteOperation,
        /// Method just requested.
        received: WriteOperation,
    },
    /// A receipt contradicts the transaction facts already stored for the attempt.
    #[error("operation {operation_id} has a contradictory receipt: {reason}")]
    ReceiptConflict {
        /// Affected operation.
        operation_id: OperationId,
        /// Contradiction.
        reason: String,
    },
    /// Finalization tried to replace a previously recorded method result.
    #[error("operation {operation_id} already has a different final result")]
    ResultConflict {
        /// Affected operation.
        operation_id: OperationId,
    },
    /// A stage change would skip or reverse the lifecycle.
    #[error("operation cannot move from {from:?} to {to:?}")]
    IllegalTransition {
        /// Current stage.
        from: OperationStage,
        /// Requested stage.
        to: OperationStage,
    },
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    const CHAIN: Felt = Felt::from_hex_unchecked("0x534e5f5345504f4c4941");
    const POOL: Felt = Felt::from_hex_unchecked("0x4e4f");
    const TOKEN: Felt = Felt::from_hex_unchecked("0x53545f");

    fn id(seed: u8) -> OperationId {
        OperationId::parse(format!("op_{}", format!("{seed:02x}").repeat(32)))
            .expect("constructed id is valid")
    }

    fn binding() -> RequestBinding {
        RequestBinding::builder(WriteOperation::Shield, CHAIN, POOL, TOKEN)
            .u128_be(500)
            .finish()
    }

    fn journal() -> OperationJournal {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "erebus-journal-private-restart-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        OperationJournal::new(root).expect("journal opens")
    }

    fn signed_lease(seed: u8) -> OperationLease {
        let journal = journal();
        let mut lease = journal
            .claim(&id(seed), WriteOperation::Shield, binding(), None, 1_000)
            .expect("claim");
        lease
            .advance(OperationStage::Prepared, 1_001)
            .expect("prepare");
        lease.advance(OperationStage::Proven, 1_002).expect("prove");
        lease
            .persist_signed(Felt::from_hex_unchecked("0x1111"), "first", 1_003)
            .expect("persist signed transaction");
        lease
    }

    #[test]
    fn replacement_attempt_retains_every_earlier_hash_and_transaction() {
        let mut lease = signed_lease(1);
        lease
            .advance(OperationStage::Submitted, 1_004)
            .expect("submit");

        lease.restart(2_000).expect("proven-dead attempt restarts");

        assert_eq!(lease.record().attempts.len(), 2);
        assert_eq!(lease.record().stage(), OperationStage::Claimed);
        assert_eq!(
            lease.record().attempts[0].transaction_hash,
            Some(Felt::from_hex_unchecked("0x1111"))
        );
        assert_eq!(
            lease.stored_transaction(0).expect("read first"),
            Some("first".to_owned())
        );

        lease
            .advance(OperationStage::Prepared, 2_001)
            .expect("prepare replacement");
        lease
            .advance(OperationStage::Proven, 2_002)
            .expect("prove replacement");
        lease
            .persist_signed(Felt::from_hex_unchecked("0x2222"), "second", 2_003)
            .expect("persist replacement");

        assert_eq!(
            lease.stored_transaction(1).expect("read replacement"),
            Some("second".to_owned())
        );
    }

    #[test]
    fn committed_operation_cannot_start_a_replacement_attempt() {
        let mut lease = signed_lease(2);
        for stage in [
            OperationStage::Submitted,
            OperationStage::Accepted,
            OperationStage::Committed,
        ] {
            lease.advance(stage, 1_004).expect("advance");
        }

        assert!(matches!(
            lease.restart(2_000),
            Err(JournalError::IllegalTransition { .. })
        ));
    }

    #[test]
    fn reverted_operation_can_start_a_replacement_attempt() {
        let mut lease = signed_lease(3);
        lease
            .advance(OperationStage::Submitted, 1_004)
            .expect("submit");
        lease
            .advance(OperationStage::Reverted, 1_005)
            .expect("revert");

        lease
            .restart(2_000)
            .expect("reverted attempt produced no effect");
        assert_eq!(lease.record().stage(), OperationStage::Claimed);
    }
}
