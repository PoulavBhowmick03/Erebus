//! Restartable public pool index cache. It contains no note openings.

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

use crate::indexer::{IndexError, PoolBlock, PoolIndex, TreeCheckpoint};

const VERSION: u32 = 2;
/// Caches that start from a verified checkpoint; older binaries reject them instead of misreading.
const CHECKPOINT_VERSION: u32 = 3;
const MAX_CACHE_BYTES: u64 = 128 * 1024 * 1024;

/// Deployment anchor for one public pool event history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexDomain {
    /// EVM chain ID.
    pub chain_id: u64,
    /// Pool contract address.
    pub pool: [u8; 20],
    /// Block in which the pool was deployed.
    pub first_block: u64,
    /// Trusted hash of the deployment block.
    pub first_hash: [u8; 32],
}

impl IndexDomain {
    fn validate(self) -> Result<(), IndexStoreError> {
        if self.chain_id == 0 || self.pool == [0; 20] || self.first_hash == [0; 32] {
            return Err(IndexStoreError::Domain);
        }
        Ok(())
    }
}

/// A public cache could not be read or safely updated.
#[derive(Debug, thiserror::Error)]
pub enum IndexStoreError {
    /// Filesystem operation failed.
    #[error("public index filesystem operation failed")]
    Io(#[from] std::io::Error),
    /// Cache is too large, malformed, or built for another deployment.
    #[error("public index cache is malformed or belongs to another deployment")]
    Cache,
    /// Deployment anchor is missing or invalid.
    #[error("invalid public index deployment identity")]
    Domain,
    /// Another scanner changed this cache since it was read.
    #[error("public index cache changed concurrently; reload before saving")]
    Stale,
    /// Cached event history failed independent verification.
    #[error(transparent)]
    Index(#[from] IndexError),
}

#[derive(Serialize, Deserialize)]
struct StoredIndex {
    version: u32,
    domain: IndexDomain,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checkpoint: Option<TreeCheckpoint>,
    blocks: Vec<PoolBlock>,
}

/// Atomic, exclusively locked public index cache for one deployment.
pub struct IndexStore {
    path: PathBuf,
    lock_path: PathBuf,
    domain: IndexDomain,
    // Boxed: the frontier is ~700 bytes and stores are held inline by larger enums.
    checkpoint: Option<Box<TreeCheckpoint>>,
    faults: Arc<dyn FaultHook>,
}

impl IndexStore {
    /// Whether two handles index the same deployment from the same starting state.
    pub fn same_history(&self, other: &Self) -> bool {
        self.domain == other.domain && self.checkpoint == other.checkpoint
    }

    /// Replaces this cache with `source` when `source` is the same history and covers more of it.
    ///
    /// Both caches are fully re-verified on load, so the copy carries no trust beyond what each
    /// already had; the caller keeps its own anchor and finality checks. Returns whether the cache
    /// changed. A concurrent writer wins: `Stale` leaves this cache as that writer saved it.
    pub fn seed_from(&self, source: &Self) -> Result<bool, IndexStoreError> {
        if !self.same_history(source) {
            return Err(IndexStoreError::Domain);
        }
        let current = self.load()?;
        let seed = source.load()?;
        let covered = |index: &PoolIndex| index.covered_through().map(|(number, _)| number);
        if covered(&seed) <= covered(&current) {
            return Ok(false);
        }
        match self.save_if_unchanged(&seed, current.tip().map(|tip| tip.hash)) {
            Ok(()) => Ok(true),
            Err(IndexStoreError::Stale) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// The cache file, which identifies this history within a process.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Verified starting checkpoint, or `None` when this cache covers deployment-era history.
    pub fn checkpoint(&self) -> Option<TreeCheckpoint> {
        self.checkpoint.as_deref().copied()
    }

    /// Opens a cache that starts after a verified checkpoint instead of at deployment.
    ///
    /// The deployment anchor is unchanged; the checkpoint must be at or after it. Passing `None`
    /// is identical to [`Self::new`].
    pub fn with_checkpoint(
        path: impl Into<PathBuf>,
        domain: IndexDomain,
        checkpoint: Option<TreeCheckpoint>,
    ) -> Result<Self, IndexStoreError> {
        let mut store = Self::new(path, domain)?;
        if let Some(checkpoint) = checkpoint {
            if checkpoint.block_number < domain.first_block
                || (checkpoint.block_number == domain.first_block
                    && checkpoint.block_hash != domain.first_hash)
            {
                return Err(IndexStoreError::Domain);
            }
            checkpoint.verify()?;
        }
        store.checkpoint = checkpoint.map(Box::new);
        Ok(store)
    }
    pub(crate) fn shares_cache(&self, other: &Self) -> Result<bool, IndexStoreError> {
        let identity = |path: &Path| -> Result<PathBuf, IndexStoreError> {
            let parent = path.parent().ok_or(IndexStoreError::Cache)?;
            let name = path.file_name().ok_or(IndexStoreError::Cache)?;
            Ok(fs::canonicalize(parent)?.join(name))
        };
        Ok(identity(&self.path)? == identity(&other.path)?)
    }

    /// Deployment identity this cache accepts.
    pub fn domain(&self) -> IndexDomain {
        self.domain
    }

    /// Creates a cache handle. Existing bytes are validated only when `load` is called.
    pub fn new(path: impl Into<PathBuf>, domain: IndexDomain) -> Result<Self, IndexStoreError> {
        Self::with_faults(path, domain, Arc::new(NoFaults))
    }

    /// Opens the same public cache with a hook after each durable filesystem step.
    pub fn with_faults(
        path: impl Into<PathBuf>,
        domain: IndexDomain,
        faults: Arc<dyn FaultHook>,
    ) -> Result<Self, IndexStoreError> {
        domain.validate()?;
        let path = path.into();
        let parent = path.parent().ok_or(IndexStoreError::Cache)?;
        fs::create_dir_all(parent)?;
        if !fs::symlink_metadata(parent)?.file_type().is_dir() {
            return Err(IndexStoreError::Cache);
        }
        let lock_path = path.with_extension("lock");
        regular_or_absent(&path)?;
        regular_or_absent(&lock_path)?;
        Ok(Self {
            path,
            lock_path,
            domain,
            checkpoint: None,
            faults,
        })
    }

    /// Rebuilds and verifies every stored transition before returning an index.
    pub fn load(&self) -> Result<PoolIndex, IndexStoreError> {
        let lock = self.lock()?;
        let index = self.load_locked()?;
        drop(lock);
        Ok(index)
    }

    /// Persists one verified index only if the cache has not changed since `load`.
    ///
    /// `expected_tip` is the hash observed when the caller loaded the cache, or `None`
    /// for an empty cache. A reorg may replace the old suffix once the RPC has been
    /// checked against the trusted deployment anchor.
    pub fn save_if_unchanged(
        &self,
        index: &PoolIndex,
        expected_tip: Option<[u8; 32]>,
    ) -> Result<(), IndexStoreError> {
        self.check_index(index)?;
        let lock = self.lock()?;
        let stored = self.load_locked()?;
        if stored.tip().map(|tip| tip.hash) != expected_tip {
            return Err(IndexStoreError::Stale);
        }
        let data = serde_json::to_vec(&StoredIndex {
            version: if self.checkpoint.is_some() {
                CHECKPOINT_VERSION
            } else {
                VERSION
            },
            domain: self.domain,
            checkpoint: self.checkpoint(),
            blocks: index.blocks().to_vec(),
        })
        .map_err(|_| IndexStoreError::Cache)?;
        if data.len() as u64 > MAX_CACHE_BYTES {
            return Err(IndexStoreError::Cache);
        }
        self.write_locked(&data)?;
        drop(lock);
        Ok(())
    }

    fn empty_index(&self) -> Result<PoolIndex, IndexStoreError> {
        Ok(match self.checkpoint() {
            Some(checkpoint) => PoolIndex::from_checkpoint(checkpoint)?,
            None => PoolIndex::new(self.domain.first_block)?,
        })
    }

    fn check_index(&self, index: &PoolIndex) -> Result<(), IndexStoreError> {
        if index.checkpoint().copied() != self.checkpoint() {
            return Err(IndexStoreError::Domain);
        }
        match self.checkpoint() {
            // The first scanned block must extend the verified checkpoint block.
            Some(checkpoint) => {
                if index.first_block() != checkpoint.block_number + 1
                    || index
                        .blocks()
                        .first()
                        .is_some_and(|block| block.parent_hash != checkpoint.block_hash)
                {
                    return Err(IndexStoreError::Domain);
                }
            }
            None => {
                if index.first_block() != self.domain.first_block
                    || index
                        .blocks()
                        .first()
                        .is_some_and(|block| block.hash != self.domain.first_hash)
                {
                    return Err(IndexStoreError::Domain);
                }
            }
        }
        let mut rebuilt = self.empty_index()?;
        for block in index.blocks() {
            rebuilt.apply_block(block.clone())?;
        }
        if rebuilt.root() != index.root() || rebuilt.leaves() != index.leaves() {
            return Err(IndexStoreError::Cache);
        }
        Ok(())
    }

    fn load_locked(&self) -> Result<PoolIndex, IndexStoreError> {
        regular_or_absent(&self.path)?;
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return self.empty_index()
            }
            Err(error) => return Err(error.into()),
        };
        if file.metadata()?.len() > MAX_CACHE_BYTES {
            return Err(IndexStoreError::Cache);
        }
        let mut data = Vec::new();
        file.take(MAX_CACHE_BYTES + 1).read_to_end(&mut data)?;
        if data.len() as u64 > MAX_CACHE_BYTES {
            return Err(IndexStoreError::Cache);
        }
        let stored: StoredIndex =
            serde_json::from_slice(&data).map_err(|_| IndexStoreError::Cache)?;
        if stored.domain != self.domain || stored.checkpoint != self.checkpoint() {
            return Err(IndexStoreError::Cache);
        }
        if stored.version == 1 && self.checkpoint.is_none() {
            // Version 1 discarded deal identities from transfer events. This public cache
            // must be rescanned before it can support settlement observation.
            return PoolIndex::new(self.domain.first_block).map_err(Into::into);
        }
        let expected = if self.checkpoint.is_some() {
            CHECKPOINT_VERSION
        } else {
            VERSION
        };
        if stored.version != expected {
            return Err(IndexStoreError::Cache);
        }
        let mut index = self.empty_index()?;
        for block in stored.blocks {
            index.apply_block(block)?;
        }
        self.check_index(&index)?;
        Ok(index)
    }

    fn lock(&self) -> Result<File, IndexStoreError> {
        regular_or_absent(&self.lock_path)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.lock_path)?;
        FileExt::lock_exclusive(&lock)?;
        Ok(lock)
    }

    fn write_locked(&self, data: &[u8]) -> Result<(), IndexStoreError> {
        let mut suffix = [0u8; 8];
        OsRng.fill_bytes(&mut suffix);
        let temp = self
            .path
            .with_extension(format!("tmp-{}", hex::encode(suffix)));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        let result = (|| {
            file.write_all(data)?;
            file.sync_all()?;
            self.faults.after(Boundary {
                step: Step::FileSynced,
                path: &temp,
            })?;
            fs::rename(&temp, &self.path)?;
            self.faults.after(Boundary {
                step: Step::Renamed,
                path: &self.path,
            })?;
            File::open(self.path.parent().ok_or(IndexStoreError::Cache)?)?.sync_all()?;
            self.faults.after(Boundary {
                step: Step::DirectorySynced,
                path: self.path.parent().ok_or(IndexStoreError::Cache)?,
            })?;
            Ok::<(), IndexStoreError>(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temp);
        }
        result
    }
}

fn regular_or_absent(path: &Path) -> Result<(), IndexStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => Err(IndexStoreError::Cache),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
