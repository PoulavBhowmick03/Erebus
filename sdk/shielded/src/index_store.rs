//! Restartable public pool index cache. It contains no note openings.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use ark_std::rand::{rngs::OsRng, RngCore};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::indexer::{IndexError, PoolBlock, PoolIndex};

const VERSION: u32 = 1;
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
    blocks: Vec<PoolBlock>,
}

/// Atomic, exclusively locked public index cache for one deployment.
pub struct IndexStore {
    path: PathBuf,
    lock_path: PathBuf,
    domain: IndexDomain,
}

impl IndexStore {
    /// Deployment identity this cache accepts.
    pub fn domain(&self) -> IndexDomain {
        self.domain
    }

    /// Creates a cache handle. Existing bytes are validated only when `load` is called.
    pub fn new(path: impl Into<PathBuf>, domain: IndexDomain) -> Result<Self, IndexStoreError> {
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
            version: VERSION,
            domain: self.domain,
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

    fn check_index(&self, index: &PoolIndex) -> Result<(), IndexStoreError> {
        if index.first_block() != self.domain.first_block
            || index
                .blocks()
                .first()
                .is_some_and(|block| block.hash != self.domain.first_hash)
        {
            return Err(IndexStoreError::Domain);
        }
        let mut rebuilt = PoolIndex::new(self.domain.first_block)?;
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
                return PoolIndex::new(self.domain.first_block).map_err(Into::into)
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
        if stored.version != VERSION || stored.domain != self.domain {
            return Err(IndexStoreError::Cache);
        }
        let mut index = PoolIndex::new(self.domain.first_block)?;
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
            fs::rename(&temp, &self.path)?;
            File::open(self.path.parent().ok_or(IndexStoreError::Cache)?)?.sync_all()?;
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
