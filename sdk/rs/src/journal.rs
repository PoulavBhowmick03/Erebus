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
use starknet_types_core::felt::Felt;

use crate::operation::{OperationId, RequestBinding, WriteOperation};
use crate::state::ChannelHandle;

/// Journal record schema version. A record written by a newer SDK fails closed.
pub const JOURNAL_VERSION: u32 = 1;

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
    /// Local state was updated to match the accepted effect.
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
    /// True from [`Self::Submitted`] onwards, and true for [`Self::NeedsAttention`] because
    /// "we do not know" must be treated as "it might have landed".
    pub const fn may_have_landed(self) -> bool {
        matches!(
            self,
            Self::Submitted | Self::Accepted | Self::Committed | Self::NeedsAttention
        )
    }

    /// Whether `next` is a legal successor of `self`.
    ///
    /// Written as an explicit table rather than a rank comparison: the interesting edges
    /// are the ones that are not a straight line, and a table makes an illegal edge a
    /// review question rather than an arithmetic accident.
    pub const fn can_advance_to(self, next: Self) -> bool {
        match (self, next) {
            (Self::Claimed, Self::Prepared)
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
    /// Unix seconds at which the chain accepted the transaction.
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
            transaction_hash: None,
            account_nonce: None,
            transaction_stored: false,
            accepted_at: None,
        }
    }
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
    /// Channel this write belongs to, when it has one.
    ///
    /// `open_channel` has no handle yet when the id is claimed, and the funding writes have
    /// no channel of their own, so this is absent for them. It exists so reconciliation can
    /// name the local record an accepted-but-uncommitted operation left behind.
    #[serde(default)]
    pub channel: Option<ChannelHandle>,
    /// Unix seconds when the id was first claimed.
    pub created_at: u64,
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
        let lock = self.lock_file(operation_id)?;
        let path = self.record_path(operation_id);

        let record = match self.read(&path)? {
            Some(record) => {
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
                record
            }
            None => {
                let record = OperationRecord {
                    version: JOURNAL_VERSION,
                    operation_id: operation_id.clone(),
                    operation,
                    binding,
                    channel,
                    created_at: now,
                    attempts: vec![Attempt::new(OperationStage::Claimed, now)],
                };
                write_atomic(&self.root, &path, &record)?;
                record
            }
        };

        Ok(OperationLease {
            root: self.root.clone(),
            path,
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
        let lock = self.lock_file(operation_id)?;
        let Some(record) = self.read(&path)? else {
            return Ok(None);
        };
        Ok(Some(OperationLease {
            root: self.root.clone(),
            path,
            _lock: lock,
            record,
        }))
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
        if record.version != JOURNAL_VERSION {
            return Err(JournalError::UnsupportedVersion(record.version));
        }
        if record.attempts.is_empty() {
            return Err(JournalError::Corrupt {
                path: path.to_path_buf(),
                reason: "record has no attempts",
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

/// An exclusively locked operation record.
///
/// Every mutation writes through to disk immediately. A lease is held across submission, and
/// a stage change that lived only in memory until some later commit would be lost by exactly
/// the crash it is meant to survive.
pub struct OperationLease {
    root: PathBuf,
    path: PathBuf,
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

    /// Starts a fresh attempt, retaining every earlier one.
    ///
    /// Used by the expired-proof recovery path. The earlier attempt keeps its transaction
    /// hash, which is the only thing that can later prove it never landed.
    pub fn restart(&mut self, now: u64) -> Result<(), JournalError> {
        if self.record.stage().is_terminal() {
            return Err(JournalError::IllegalTransition {
                from: self.record.stage(),
                to: OperationStage::Prepared,
            });
        }
        self.record
            .attempts
            .push(Attempt::new(OperationStage::Prepared, now));
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
    /// A stage change would skip or reverse the lifecycle.
    #[error("operation cannot move from {from:?} to {to:?}")]
    IllegalTransition {
        /// Current stage.
        from: OperationStage,
        /// Requested stage.
        to: OperationStage,
    },
}
