//! The on-disk store: locks, atomic writes, stored transactions, listing, and prune.
//!
//! Storage mirrors the SDK's channel state: one file per record under a `0700` directory,
//! `0600` on the files, an advisory lock per record, and replacement by atomic rename. It adds
//! one thing channel state does not do: the directory is synced after the rename, because a
//! rename that survives only in the page cache is exactly the durability a journal exists to
//! provide.

use core::fmt;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fs2::FileExt;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::Serialize;

use crate::error::{Corruption, StoreError};
use crate::fault::{Boundary, FaultHook, NoFaults, Step};
use crate::record::{JournalRecord, RecordId};

/// File holding the identity-wide write lock.
const IDENTITY_LOCK: &str = ".identity.lock";

/// A journal directory holding records of type `R`.
///
/// Cheap to clone: a path and a shared hook. It holds no lock itself; callers take the
/// [`IdentityLock`] and the [`RecordLock`] they need and keep them alive for as long as what
/// they read must stay true.
pub struct Store<R> {
    root: PathBuf,
    faults: Arc<dyn FaultHook>,
    records: PhantomData<fn() -> R>,
}

impl<R> Clone for Store<R> {
    fn clone(&self) -> Self {
        Self {
            root: self.root.clone(),
            faults: Arc::clone(&self.faults),
            records: PhantomData,
        }
    }
}

impl<R> fmt::Debug for Store<R> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Store")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

/// The identity-wide write lock, held until dropped.
///
/// Every writer takes it before its record lock, and an exclusive reader takes it alone, so a
/// reader that holds it knows no older process is midway through a write.
#[must_use = "the lock is released as soon as this is dropped"]
#[derive(Debug)]
pub struct IdentityLock {
    _file: File,
}

/// One record's lock, held until dropped.
#[must_use = "the lock is released as soon as this is dropped"]
#[derive(Debug)]
pub struct RecordLock {
    _file: File,
}

/// What prune should do with one record. The backend decides; the store only acts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PruneDecision {
    /// Keep it: it has not finished, so it may still need an explicit resume.
    RetainUnfinished,
    /// Keep it: it finished inside the retention window.
    RetainRecent,
    /// Remove it with everything filed under its id, unless another process holds its lock.
    Remove,
}

/// What a prune sweep did, and what it deliberately left alone.
///
/// The retained counts are the useful half. A prune that silently kept things would be
/// indistinguishable from one that had nothing to do, and "the journal is not growing" is
/// exactly the belief an operator should not hold on faith.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct PruneReport {
    /// Records removed, with their lock files and stored transactions.
    pub pruned: usize,
    /// Kept because they are not terminal, so they may still need an explicit resume.
    /// A record waiting for an operator is counted here: it is not terminal precisely because
    /// a person still has to look at it.
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

impl<R: JournalRecord> Store<R> {
    /// Opens or creates the journal directory `root`, with no fault hook.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, StoreError<R::Id>> {
        Self::open_with_fault_hook(root, Arc::new(NoFaults))
    }

    /// Opens or creates the journal directory `root`, reporting every durable step to `faults`.
    ///
    /// The directory is re-tightened to `0700` on every open, so a mode loosened by hand does
    /// not outlive the next process.
    pub fn open_with_fault_hook(
        root: impl Into<PathBuf>,
        faults: Arc<dyn FaultHook>,
    ) -> Result<Self, StoreError<R::Id>> {
        let root = root.into();
        std::fs::create_dir_all(&root).map_err(Self::io(&root))?;
        set_mode(&root, 0o700).map_err(Self::io(&root))?;
        Ok(Self {
            root,
            faults,
            records: PhantomData,
        })
    }

    /// The journal directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where the record for `id` lives.
    pub fn record_path(&self, id: &R::Id) -> PathBuf {
        self.root.join(format!("{}.json", id.as_file_stem()))
    }

    /// Where the transaction stored for attempt `attempt` of `id` lives.
    pub fn blob_path(&self, id: &R::Id, attempt: usize) -> PathBuf {
        // Deliberately not `.json`: `records()` parses every `.json` file in the directory
        // as a record, and a stored transaction is not one.
        self.root
            .join(format!("{}.{attempt}.tx", id.as_file_stem()))
    }

    /// Where the lock for `id` lives.
    pub fn lock_path(&self, id: &R::Id) -> PathBuf {
        self.root.join(format!("{}.lock", id.as_file_stem()))
    }

    /// Whether a record file exists for `id`. Takes no lock, so it is only a hint.
    pub fn contains(&self, id: &R::Id) -> bool {
        self.record_path(id).exists()
    }

    /// Takes the identity-wide write lock, waiting for any other holder.
    pub fn lock_identity(&self) -> Result<IdentityLock, StoreError<R::Id>> {
        self.acquire(self.root.join(IDENTITY_LOCK))
            .map(|file| IdentityLock { _file: file })
    }

    /// Takes the lock for `id`, waiting for any other holder.
    ///
    /// Re-locking an id this process already holds through another lease blocks; it does not
    /// fail.
    pub fn lock_record(&self, id: &R::Id) -> Result<RecordLock, StoreError<R::Id>> {
        self.acquire(self.lock_path(id))
            .map(|file| RecordLock { _file: file })
    }

    /// Reads the record for `id`, or `None` if there is no file.
    ///
    /// Takes no lock. A caller that must see a stable record holds the id's lock first.
    pub fn read(&self, id: &R::Id) -> Result<Option<R>, StoreError<R::Id>> {
        self.read_path(&self.record_path(id))
    }

    /// Every record in the journal, in unspecified order.
    ///
    /// Only `.json` files are records; locks, stored transactions and stray temporary files
    /// are not. A single unreadable record fails the whole listing. Reconciliation that
    /// silently skipped a record it could not parse would report "nothing pending" for an
    /// operation that may have landed, which is the one answer that must never be guessed.
    pub fn records(&self) -> Result<Vec<R>, StoreError<R::Id>> {
        let mut records = Vec::new();
        let entries = std::fs::read_dir(&self.root).map_err(Self::io(&self.root))?;
        for entry in entries {
            let entry = entry.map_err(Self::io(&self.root))?;
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            if let Some(record) = self.read_path(&path)? {
                records.push(record);
            }
        }
        Ok(records)
    }

    /// Writes `record` under its own id by atomic rename, and syncs the directory.
    ///
    /// The caller must hold the identity lock and the record's lock. Every write is the whole
    /// record as `serde_json` encodes it; there is no partial update.
    pub fn write(&self, record: &R) -> Result<(), StoreError<R::Id>> {
        let path = self.record_path(record.record_id());
        let encoded = serde_json::to_vec(record).map_err(|source| StoreError::Json {
            path: path.clone(),
            source,
        })?;
        self.write_bytes_atomic(&path, &encoded)?;
        self.reached(Step::RecordWritten, &path)
    }

    /// Stores `blob` as the transaction of attempt `attempt`, then lets `update` make the
    /// record name it, then writes the record.
    ///
    /// This is the boundary a journal exists for. The order is the whole point: the
    /// transaction is durable under its final name before `update` runs, so a record can
    /// never claim a transaction that is not on disk. A crash, or an error from `update`,
    /// between the two leaves an orphan file and a record that still reads as unsigned, which
    /// is the safe direction to fail. The reverse order would leave a hash that nothing can
    /// resubmit.
    ///
    /// If `update` fails, its error is returned and the record is not written, whatever
    /// `update` changed in memory. `update` must not change the record's id.
    pub fn write_blob_then_record<E>(
        &self,
        record: &mut R,
        attempt: usize,
        blob: &[u8],
        update: impl FnOnce(&mut R) -> Result<(), E>,
    ) -> Result<(), E>
    where
        E: From<StoreError<R::Id>>,
    {
        let path = self.blob_path(record.record_id(), attempt);
        self.write_bytes_atomic(&path, blob)?;
        self.reached(Step::BlobWritten, &path)?;
        update(record)?;
        Ok(self.write(record)?)
    }

    /// Reads back the transaction stored for one attempt.
    ///
    /// Call this only for an attempt whose record says a transaction was stored: a missing
    /// file is then [`Corruption::MissingBlob`], never "nothing stored".
    pub fn read_blob(&self, id: &R::Id, attempt: usize) -> Result<Vec<u8>, StoreError<R::Id>> {
        let path = self.blob_path(id, attempt);
        std::fs::read(&path).map_err(|error| Self::blob_error(path, error))
    }

    /// [`Self::read_blob`] for a transaction stored as text. Bytes that are not UTF-8 are an
    /// I/O error, as `std::fs::read_to_string` reports them.
    pub fn read_blob_to_string(
        &self,
        id: &R::Id,
        attempt: usize,
    ) -> Result<String, StoreError<R::Id>> {
        let path = self.blob_path(id, attempt);
        std::fs::read_to_string(&path).map_err(|error| Self::blob_error(path, error))
    }

    /// Removes every record `decide` marks [`PruneDecision::Remove`], with its stored
    /// transactions and its lock file.
    ///
    /// Holds the identity write lock for the whole sweep, so a prune cannot race a write that
    /// is midway through claiming or advancing. A record whose own lock is held elsewhere is
    /// skipped rather than waited for: pruning is maintenance and must never block, or stall,
    /// a real operation. A lock file is created only for a record being removed, and removed
    /// with it.
    pub fn prune(
        &self,
        mut decide: impl FnMut(&R) -> PruneDecision,
    ) -> Result<PruneReport, StoreError<R::Id>> {
        let _identity_lock = self.lock_identity()?;
        let mut report = PruneReport::default();

        for record in self.records()? {
            match decide(&record) {
                PruneDecision::RetainUnfinished => report.retained_unfinished += 1,
                PruneDecision::RetainRecent => report.retained_recent += 1,
                PruneDecision::Remove => {
                    if self.remove(&record)? {
                        report.pruned += 1;
                    } else {
                        report.retained_locked += 1;
                    }
                }
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
    fn remove(&self, record: &R) -> Result<bool, StoreError<R::Id>> {
        let id = record.record_id();
        let lock_path = self.lock_path(id);
        let lock = open_lock(&lock_path).map_err(Self::io(&lock_path))?;
        if lock.try_lock_exclusive().is_err() {
            return Ok(false);
        }

        for attempt in 0..record.attempt_count() {
            self.remove_file(&self.blob_path(id, attempt))?;
        }
        self.remove_file(&self.record_path(id))?;
        drop(lock);
        self.remove_file(&lock_path)?;
        self.sync_dir()?;
        Ok(true)
    }

    fn read_path(&self, path: &Path) -> Result<Option<R>, StoreError<R::Id>> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(StoreError::Io {
                    path: path.to_path_buf(),
                    source,
                })
            }
        };
        let record: R =
            serde_json::from_reader(BufReader::new(file)).map_err(|source| StoreError::Json {
                path: path.to_path_buf(),
                source,
            })?;
        if !(R::OLDEST_READABLE_VERSION..=R::CURRENT_VERSION).contains(&record.version()) {
            return Err(StoreError::UnsupportedVersion(record.version()));
        }
        if record.attempt_count() == 0 {
            return Err(StoreError::Corrupt {
                path: path.to_path_buf(),
                reason: Corruption::NoAttempts,
            });
        }
        let expected = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(<R::Id as RecordId>::from_file_stem)
            .ok_or_else(|| StoreError::Corrupt {
                path: path.to_path_buf(),
                reason: Corruption::FileName,
            })?;
        if *record.record_id() != expected {
            return Err(StoreError::IdMismatch {
                expected,
                found: record.record_id().clone(),
            });
        }
        Ok(Some(record))
    }

    fn acquire(&self, path: PathBuf) -> Result<File, StoreError<R::Id>> {
        let lock = open_lock(&path).map_err(Self::io(&path))?;
        // Re-tightened on every lock, so a mode loosened by hand does not survive.
        set_file_mode(&lock, 0o600).map_err(Self::io(&path))?;
        lock.lock_exclusive().map_err(Self::io(&path))?;
        Ok(lock)
    }

    fn write_bytes_atomic(&self, path: &Path, bytes: &[u8]) -> Result<(), StoreError<R::Id>> {
        let temporary = self.root.join(format!(
            ".{}.{:016x}.tmp",
            std::process::id(),
            OsRng.next_u64(),
        ));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(Self::io(&temporary))?;
        set_file_mode(&file, 0o600).map_err(Self::io(&temporary))?;
        let mut writer = BufWriter::new(file);
        writer.write_all(bytes).map_err(Self::io(&temporary))?;
        writer.flush().map_err(Self::io(&temporary))?;
        writer.get_ref().sync_all().map_err(Self::io(&temporary))?;
        self.reached(Step::FileSynced, &temporary)?;
        std::fs::rename(&temporary, path).map_err(Self::io(path))?;
        self.reached(Step::Renamed, path)?;
        self.sync_dir()
    }

    fn sync_dir(&self) -> Result<(), StoreError<R::Id>> {
        sync_dir(&self.root).map_err(Self::io(&self.root))?;
        self.reached(Step::DirectorySynced, &self.root)
    }

    fn remove_file(&self, path: &Path) -> Result<(), StoreError<R::Id>> {
        remove_if_present(path).map_err(Self::io(path))?;
        self.reached(Step::Removed, path)
    }

    /// Reports a completed step. An injected failure reads as an I/O error at `path`.
    fn reached(&self, step: Step, path: &Path) -> Result<(), StoreError<R::Id>> {
        self.faults
            .after(Boundary { step, path })
            .map_err(Self::io(path))
    }

    fn io(path: &Path) -> impl FnOnce(std::io::Error) -> StoreError<R::Id> + '_ {
        move |source| StoreError::Io {
            path: path.to_path_buf(),
            source,
        }
    }

    fn blob_error(path: PathBuf, error: std::io::Error) -> StoreError<R::Id> {
        if error.kind() == std::io::ErrorKind::NotFound {
            // The record says a transaction was stored and it is not there. Reconciliation
            // must not read that as "nothing was submitted".
            StoreError::Corrupt {
                path,
                reason: Corruption::MissingBlob,
            }
        } else {
            StoreError::Io {
                path,
                source: error,
            }
        }
    }
}

fn open_lock(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

/// Removes a path, treating "already gone" as success.
///
/// A prune interrupted partway through leaves some of an operation's files removed. Rerunning
/// it must finish the job rather than fail on the ones it already deleted.
fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Flushes the directory entries created by a rename or dropped by a removal.
///
/// Without this the file contents are durable but the name is not, so a crash can leave the
/// record at its previous version while the caller believes it advanced.
#[cfg(unix)]
fn sync_dir(path: &Path) -> std::io::Result<()> {
    File::open(path).and_then(|dir| dir.sync_all())
}

#[cfg(not(unix))]
fn sync_dir(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_file_mode(file: &File, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    file.set_permissions(std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_file_mode(_file: &File, _mode: u32) -> std::io::Result<()> {
    Ok(())
}
