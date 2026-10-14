//! Durable, explicit checkpoint record for a fresh participant (`docs/metropolis-evidence/ws1-checkpoint-design.md`).
//!
//! The record lives at `<state_root>/history-checkpoint.json`. Every participant binary that builds a
//! pool index under that state root loads it; without a record, indexes cover deployment-era history
//! exactly as before. Creating a record never signs, sends or scans history.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use ark_std::rand::{rngs::OsRng, RngCore};
use erebus_journal::{Boundary, FaultHook, NoFaults, Step};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::{index_store::IndexDomain, indexer::TreeCheckpoint};

/// Record file name under a participant state root.
pub const RECORD_FILE: &str = "history-checkpoint.json";
const LOCK_FILE: &str = "history-checkpoint.lock";
const RECORD_VERSION: u32 = 1;
const MAX_RECORD_BYTES: u64 = 16 * 1024;

/// A checkpoint record could not be read, created or trusted.
#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    /// Filesystem operation failed.
    #[error("checkpoint filesystem operation failed")]
    Io(#[from] std::io::Error),
    /// The record is malformed, from another version, or bound to another deployment.
    #[error("checkpoint record is malformed or belongs to another deployment")]
    Record,
    /// A different checkpoint is already recorded for this state root.
    #[error("a different checkpoint is already recorded")]
    Conflict,
    /// Another process is initializing this state root.
    #[error("another checkpoint initialization is in progress")]
    Busy,
    /// The state root already holds history-bearing state, or it could not be inspected safely.
    #[error("participant state is not fresh: {0}")]
    NotFresh(&'static str),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRecord {
    version: u32,
    chain_id: u64,
    pool: [u8; 20],
    deployment_block: u64,
    deployment_hash: [u8; 32],
    checkpoint: TreeCheckpoint,
}

/// Whether [`persist`] created the record or found the same one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Persisted {
    /// The record was created by this call.
    Created,
    /// An identical record already existed; nothing was written.
    Existing,
}

fn record_path(state_root: &Path) -> PathBuf {
    state_root.join(RECORD_FILE)
}

/// Loads a recorded checkpoint bound to `domain`, or `None` when this state root has none.
///
/// A record for another chain, pool or deployment anchor, an unknown version, or a frontier that
/// does not recompute its root is an error, never `None`.
pub fn load(
    state_root: &Path,
    domain: IndexDomain,
) -> Result<Option<TreeCheckpoint>, CheckpointError> {
    let path = record_path(state_root);
    let file = match symlink_free(&path)? {
        false => return Ok(None),
        true => File::open(&path)?,
    };
    if file.metadata()?.len() > MAX_RECORD_BYTES {
        return Err(CheckpointError::Record);
    }
    let mut data = Vec::new();
    file.take(MAX_RECORD_BYTES + 1).read_to_end(&mut data)?;
    let stored: StoredRecord =
        serde_json::from_slice(&data).map_err(|_| CheckpointError::Record)?;
    if stored.version != RECORD_VERSION
        || stored.chain_id != domain.chain_id
        || stored.pool != domain.pool
        || stored.deployment_block != domain.first_block
        || stored.deployment_hash != domain.first_hash
        || stored.checkpoint.block_number < domain.first_block
    {
        return Err(CheckpointError::Record);
    }
    stored
        .checkpoint
        .verify()
        .map_err(|_| CheckpointError::Record)?;
    Ok(Some(stored.checkpoint))
}

/// Exclusive, non-blocking initialization lock for one state root.
pub struct InitLock {
    _file: File,
}

impl InitLock {
    /// Acquires the lock or refuses immediately if another initialization holds it.
    pub fn acquire(state_root: &Path) -> Result<Self, CheckpointError> {
        let path = state_root.join(LOCK_FILE);
        symlink_free(&path)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        FileExt::try_lock_exclusive(&file).map_err(|_| CheckpointError::Busy)?;
        Ok(Self { _file: file })
    }
}

/// Creates the record once; an identical existing record is accepted, any other is a conflict.
///
/// The record becomes visible only through an atomic no-replace link after its bytes are synced,
/// so a crash leaves either no record or the complete record (plus at most a stray temporary file).
pub fn persist(
    state_root: &Path,
    domain: IndexDomain,
    checkpoint: TreeCheckpoint,
    _lock: &InitLock,
) -> Result<Persisted, CheckpointError> {
    persist_with_faults(state_root, domain, checkpoint, _lock, Arc::new(NoFaults))
}

/// [`persist`] with a hook after each durable step, for interruption tests.
pub fn persist_with_faults(
    state_root: &Path,
    domain: IndexDomain,
    checkpoint: TreeCheckpoint,
    _lock: &InitLock,
    faults: Arc<dyn FaultHook>,
) -> Result<Persisted, CheckpointError> {
    if checkpoint.block_number < domain.first_block {
        return Err(CheckpointError::Record);
    }
    checkpoint.verify().map_err(|_| CheckpointError::Record)?;
    match load(state_root, domain)? {
        Some(existing) if existing == checkpoint => return Ok(Persisted::Existing),
        Some(_) => return Err(CheckpointError::Conflict),
        None => {}
    }
    let data = serde_json::to_vec_pretty(&StoredRecord {
        version: RECORD_VERSION,
        chain_id: domain.chain_id,
        pool: domain.pool,
        deployment_block: domain.first_block,
        deployment_hash: domain.first_hash,
        checkpoint,
    })
    .map_err(|_| CheckpointError::Record)?;
    let target = record_path(state_root);
    let mut suffix = [0u8; 8];
    OsRng.fill_bytes(&mut suffix);
    let temp = state_root.join(format!("{RECORD_FILE}.tmp-{}", hex::encode(suffix)));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    let result = (|| {
        file.write_all(&data)?;
        file.sync_all()?;
        faults.after(Boundary {
            step: Step::FileSynced,
            path: &temp,
        })?;
        // Atomic and never replaces: a concurrent or earlier record makes this fail.
        fs::hard_link(&temp, &target)?;
        faults.after(Boundary {
            step: Step::Renamed,
            path: &target,
        })?;
        File::open(state_root)?.sync_all()?;
        faults.after(Boundary {
            step: Step::DirectorySynced,
            path: state_root,
        })?;
        Ok::<(), std::io::Error>(())
    })();
    let _ = fs::remove_file(&temp);
    result?;
    Ok(Persisted::Created)
}

/// Refuses unless every path is absent, or a directory tree holding only `*.lock` files.
///
/// Symlinks, special files and unreadable entries refuse; nothing here interprets file contents.
pub fn require_fresh_paths(paths: &[PathBuf]) -> Result<(), CheckpointError> {
    for path in paths {
        inspect(path)?;
    }
    Ok(())
}

fn inspect(path: &Path) -> Result<(), CheckpointError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(CheckpointError::NotFresh("state could not be inspected")),
    };
    let kind = metadata.file_type();
    if kind.is_symlink() {
        return Err(CheckpointError::NotFresh("state path is a symbolic link"));
    }
    if kind.is_file() {
        let lock = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".lock"));
        return if lock {
            Ok(())
        } else {
            Err(CheckpointError::NotFresh("history-bearing state exists"))
        };
    }
    if !kind.is_dir() {
        return Err(CheckpointError::NotFresh("unexpected file type in state"));
    }
    let entries = fs::read_dir(path)
        .map_err(|_| CheckpointError::NotFresh("state could not be inspected"))?;
    for entry in entries {
        let entry = entry.map_err(|_| CheckpointError::NotFresh("state could not be inspected"))?;
        inspect(&entry.path())?;
    }
    Ok(())
}

fn symlink_free(path: &Path) -> Result<bool, CheckpointError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(true),
        Ok(_) => Err(CheckpointError::Record),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::PoolIndex;
    use std::sync::Mutex;

    fn domain() -> IndexDomain {
        IndexDomain {
            chain_id: 31337,
            pool: [3; 20],
            first_block: 10,
            first_hash: [10; 32],
        }
    }

    fn empty_checkpoint(block_number: u64) -> TreeCheckpoint {
        let index = PoolIndex::new(1).unwrap();
        let mut zero = [[0u8; 32]; 20];
        let mut value = [0u8; 32];
        for slot in &mut zero {
            *slot = value;
            value = erebus_core::shielded::note_tree_parent(&value, &value).unwrap();
        }
        TreeCheckpoint {
            block_number,
            block_hash: [block_number as u8; 32],
            leaf_count: 0,
            root: index.root(),
            filled_subtrees: zero,
        }
    }

    #[test]
    fn record_round_trips_and_binds_its_deployment() {
        let root = tempfile::tempdir().unwrap();
        let lock = InitLock::acquire(root.path()).unwrap();
        let checkpoint = empty_checkpoint(20);
        assert_eq!(load(root.path(), domain()).unwrap(), None);
        assert_eq!(
            persist(root.path(), domain(), checkpoint, &lock).unwrap(),
            Persisted::Created
        );
        assert_eq!(load(root.path(), domain()).unwrap(), Some(checkpoint));
        // Repeating with the same checkpoint writes nothing; a different one is a conflict.
        let before = fs::read(root.path().join(RECORD_FILE)).unwrap();
        assert_eq!(
            persist(root.path(), domain(), checkpoint, &lock).unwrap(),
            Persisted::Existing
        );
        assert_eq!(fs::read(root.path().join(RECORD_FILE)).unwrap(), before);
        assert!(matches!(
            persist(root.path(), domain(), empty_checkpoint(21), &lock),
            Err(CheckpointError::Conflict)
        ));
        for other in [
            IndexDomain {
                chain_id: 1,
                ..domain()
            },
            IndexDomain {
                pool: [4; 20],
                ..domain()
            },
            IndexDomain {
                first_block: 11,
                ..domain()
            },
            IndexDomain {
                first_hash: [11; 32],
                ..domain()
            },
        ] {
            assert!(matches!(
                load(root.path(), other),
                Err(CheckpointError::Record)
            ));
        }
    }

    #[test]
    fn corrupt_or_tampered_records_refuse() {
        let root = tempfile::tempdir().unwrap();
        let lock = InitLock::acquire(root.path()).unwrap();
        persist(root.path(), domain(), empty_checkpoint(20), &lock).unwrap();
        let path = root.path().join(RECORD_FILE);
        let good = fs::read_to_string(&path).unwrap();
        // A root that the frontier does not recompute is refused. (A changed leaf count alone can be
        // root-indistinguishable when the extra leaves would be zero; the count is bound by the paired
        // `nextLeafIndex` read and by the first later insertion instead.)
        let mut value: serde_json::Value = serde_json::from_str(&good).unwrap();
        let byte = value["checkpoint"]["root"][31].as_u64().unwrap();
        value["checkpoint"]["root"][31] = serde_json::json!(byte ^ 1);
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(
            load(root.path(), domain()),
            Err(CheckpointError::Record)
        ));
        fs::write(&path, b"{").unwrap();
        assert!(matches!(
            load(root.path(), domain()),
            Err(CheckpointError::Record)
        ));
        fs::write(&path, good.replacen("\"version\": 1", "\"version\": 2", 1)).unwrap();
        assert!(matches!(
            load(root.path(), domain()),
            Err(CheckpointError::Record)
        ));
        // A checkpoint before the deployment anchor is never accepted.
        fs::remove_file(&path).unwrap();
        assert!(matches!(
            persist(root.path(), domain(), empty_checkpoint(9), &lock),
            Err(CheckpointError::Record)
        ));
        assert!(!path.exists());
    }

    #[test]
    fn concurrent_initialization_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let held = InitLock::acquire(root.path()).unwrap();
        assert!(matches!(
            InitLock::acquire(root.path()),
            Err(CheckpointError::Busy)
        ));
        drop(held);
        InitLock::acquire(root.path()).unwrap();
    }

    struct FailAt(Step, Mutex<bool>);
    impl FaultHook for FailAt {
        fn after(&self, boundary: Boundary<'_>) -> std::io::Result<()> {
            if boundary.step == self.0 && !*self.1.lock().unwrap() {
                *self.1.lock().unwrap() = true;
                return Err(std::io::Error::other("injected crash"));
            }
            Ok(())
        }
    }

    #[test]
    fn interruption_at_each_boundary_leaves_no_record_or_the_complete_record() {
        for step in [Step::FileSynced, Step::Renamed, Step::DirectorySynced] {
            let root = tempfile::tempdir().unwrap();
            let lock = InitLock::acquire(root.path()).unwrap();
            let checkpoint = empty_checkpoint(20);
            let hook = Arc::new(FailAt(step, Mutex::new(false)));
            assert!(persist_with_faults(root.path(), domain(), checkpoint, &lock, hook).is_err());
            let after = load(root.path(), domain()).unwrap();
            match step {
                Step::FileSynced => assert_eq!(after, None),
                _ => assert_eq!(after, Some(checkpoint)),
            }
            // No temporary file is left behind, and a retry converges on the same record.
            assert!(fs::read_dir(root.path()).unwrap().all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".tmp-")));
            persist(root.path(), domain(), checkpoint, &lock).unwrap();
            assert_eq!(load(root.path(), domain()).unwrap(), Some(checkpoint));
        }
    }

    #[test]
    fn freshness_accepts_only_absent_or_lock_only_state() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("wallet-index");
        let file = root.path().join("pool-history.json");
        require_fresh_paths(&[dir.clone(), file.clone()]).unwrap();
        fs::create_dir_all(dir.join("nested")).unwrap();
        fs::write(dir.join("primary.lock"), b"").unwrap();
        fs::write(dir.join("nested/peer.lock"), b"").unwrap();
        require_fresh_paths(std::slice::from_ref(&dir)).unwrap();
        fs::write(dir.join("nested/peer.json"), b"{}").unwrap();
        assert!(matches!(
            require_fresh_paths(std::slice::from_ref(&dir)),
            Err(CheckpointError::NotFresh(_))
        ));
        fs::write(&file, b"{}").unwrap();
        assert!(require_fresh_paths(&[file]).is_err());
        #[cfg(unix)]
        {
            let link = root.path().join("linked");
            std::os::unix::fs::symlink(root.path().join("elsewhere"), &link).unwrap();
            assert!(matches!(
                require_fresh_paths(&[link]),
                Err(CheckpointError::NotFresh(_))
            ));
        }
    }
}
