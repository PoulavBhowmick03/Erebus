//! Restartable public pool index cache. It contains no note openings.
//!
//! Version 3 stores bounded segment files beside a small manifest, so a bounded scan
//! never rewrites the whole history and no single file is unbounded. Version 2
//! single-file caches are read explicitly and migrated on the next save; version 1
//! caches are rescanned (existing behavior). Corruption is never treated as absence.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use ark_std::rand::{rngs::OsRng, RngCore};
use erebus_core::suite::keccak256;
use erebus_journal::{Boundary, FaultHook, NoFaults, Step};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::indexer::{IndexError, PoolBlock, PoolIndex};

const VERSION: u32 = 3;
const LEGACY_VERSION: u32 = 2;
const MAX_CACHE_BYTES: u64 = 128 * 1024 * 1024;
const SEGMENT_TARGET_BYTES: usize = 8 * 1024 * 1024;
const MAX_SEGMENT_BYTES: u64 = 32 * 1024 * 1024;

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

#[derive(Deserialize)]
struct Header {
    version: u32,
}

#[derive(Serialize, Deserialize)]
struct StoredIndex {
    version: u32,
    domain: IndexDomain,
    blocks: Vec<PoolBlock>,
}

#[derive(Serialize, Deserialize, Clone)]
struct SegmentDescriptor {
    file: String,
    first_block: u64,
    last_block: u64,
    last_hash: [u8; 32],
    bytes: u64,
    digest: [u8; 32],
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u32,
    domain: IndexDomain,
    tip_number: u64,
    tip_hash: [u8; 32],
    segments: Vec<SegmentDescriptor>,
}

#[derive(Serialize, Deserialize)]
struct SegmentFile {
    version: u32,
    first_block: u64,
    last_block: u64,
    blocks: Vec<PoolBlock>,
}

/// Atomic, exclusively locked public index cache for one deployment.
pub struct IndexStore {
    path: PathBuf,
    segment_dir: PathBuf,
    lock_path: PathBuf,
    domain: IndexDomain,
    faults: Arc<dyn FaultHook>,
}

impl IndexStore {
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
        let segment_dir = path.with_extension("segments");
        regular_or_absent(&path)?;
        regular_or_absent(&lock_path)?;
        if segment_dir.exists() && !fs::symlink_metadata(&segment_dir)?.file_type().is_dir() {
            return Err(IndexStoreError::Cache);
        }
        Ok(Self {
            path,
            segment_dir,
            lock_path,
            domain,
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
    /// checked against the trusted deployment anchor. Only the tail segment and new
    /// segments are rewritten; completed segments are reused byte for byte.
    pub fn save_if_unchanged(
        &self,
        index: &PoolIndex,
        expected_tip: Option<[u8; 32]>,
    ) -> Result<(), IndexStoreError> {
        self.check_index(index)?;
        let lock = self.lock()?;
        let existing = self.read_manifest()?;
        let existing_tip = match &existing {
            Some(manifest) => Some(manifest.tip_hash),
            None => self.legacy_tip()?,
        };
        if existing_tip != expected_tip {
            return Err(IndexStoreError::Stale);
        }
        let mut segments = Vec::new();
        let mut start_block = self.domain.first_block;
        if let Some(manifest) = &existing {
            for descriptor in &manifest.segments {
                let Some(block) = index
                    .blocks()
                    .iter()
                    .find(|block| block.number == descriptor.last_block)
                else {
                    break;
                };
                if block.hash != descriptor.last_hash {
                    break;
                }
                segments.push(descriptor.clone());
            }
            if !segments.is_empty() {
                // Rewrite the tail segment so appends extend it instead of splitting it.
                segments.pop();
            }
            start_block = segments
                .last()
                .map(|descriptor| descriptor.last_block + 1)
                .unwrap_or(self.domain.first_block);
        }
        let mut chunk = Vec::new();
        let mut chunk_bytes = 0usize;
        let mut pending: Vec<(PathBuf, Vec<u8>)> = Vec::new();
        for block in index
            .blocks()
            .iter()
            .filter(|block| block.number >= start_block)
        {
            let encoded = serde_json::to_vec(block).map_err(|_| IndexStoreError::Cache)?;
            chunk_bytes += encoded.len();
            chunk.push(block.clone());
            if chunk_bytes >= SEGMENT_TARGET_BYTES {
                pending.push(self.segment_bytes(&mut chunk)?);
                chunk_bytes = 0;
            }
        }
        if !chunk.is_empty() {
            pending.push(self.segment_bytes(&mut chunk)?);
        }
        for (path, bytes) in &pending {
            self.write_atomic(path, bytes)?;
        }
        let mut manifest = Manifest {
            version: VERSION,
            domain: self.domain,
            tip_number: index
                .tip()
                .map(|tip| tip.number)
                .unwrap_or(self.domain.first_block),
            tip_hash: index.tip().map(|tip| tip.hash).unwrap_or([0; 32]),
            segments,
        };
        for (path, bytes) in &pending {
            let file = path.file_name().unwrap().to_string_lossy().into_owned();
            let first = segment_first_block(bytes)?;
            let last = segment_last_block(bytes)?;
            let hash = segment_last_hash(bytes)?;
            manifest.segments.push(SegmentDescriptor {
                file,
                first_block: first,
                last_block: last,
                last_hash: hash,
                bytes: bytes.len() as u64,
                digest: keccak256(&[bytes]),
            });
        }
        let encoded = serde_json::to_vec(&manifest).map_err(|_| IndexStoreError::Cache)?;
        if encoded.len() as u64 > MAX_CACHE_BYTES {
            return Err(IndexStoreError::Cache);
        }
        self.write_atomic(&self.path, &encoded)?;
        // Unreferenced segment files are interrupted-save garbage; they are never read.
        let referenced: std::collections::HashSet<_> = manifest
            .segments
            .iter()
            .map(|segment| segment.file.clone())
            .collect();
        if let Ok(entries) = fs::read_dir(&self.segment_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !referenced.contains(&name) {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
        drop(lock);
        Ok(())
    }

    fn segment_bytes(
        &self,
        chunk: &mut Vec<PoolBlock>,
    ) -> Result<(PathBuf, Vec<u8>), IndexStoreError> {
        let first = chunk.first().ok_or(IndexStoreError::Cache)?.number;
        let last = chunk.last().ok_or(IndexStoreError::Cache)?.number;
        let file = SegmentFile {
            version: VERSION,
            first_block: first,
            last_block: last,
            blocks: std::mem::take(chunk),
        };
        let bytes = serde_json::to_vec(&file).map_err(|_| IndexStoreError::Cache)?;
        if bytes.len() as u64 > MAX_SEGMENT_BYTES {
            return Err(IndexStoreError::Cache);
        }
        Ok((
            self.segment_dir
                .join(format!("seg-{first:016x}-{last:016x}.json")),
            bytes,
        ))
    }

    fn read_manifest(&self) -> Result<Option<Manifest>, IndexStoreError> {
        regular_or_absent(&self.path)?;
        let Some(data) = read_capped(&self.path, MAX_CACHE_BYTES)? else {
            return Ok(None);
        };
        let header: Header = serde_json::from_slice(&data).map_err(|_| IndexStoreError::Cache)?;
        if header.version != VERSION {
            return Ok(None);
        }
        let manifest: Manifest =
            serde_json::from_slice(&data).map_err(|_| IndexStoreError::Cache)?;
        if manifest.domain != self.domain {
            return Err(IndexStoreError::Cache);
        }
        Ok(Some(manifest))
    }

    fn legacy_tip(&self) -> Result<Option<[u8; 32]>, IndexStoreError> {
        regular_or_absent(&self.path)?;
        let Some(data) = read_capped(&self.path, MAX_CACHE_BYTES)? else {
            return Ok(None);
        };
        let header: Header = serde_json::from_slice(&data).map_err(|_| IndexStoreError::Cache)?;
        if header.version == 1 {
            return Ok(None);
        }
        if header.version != LEGACY_VERSION {
            return Ok(None);
        }
        let stored: StoredIndex =
            serde_json::from_slice(&data).map_err(|_| IndexStoreError::Cache)?;
        if stored.domain != self.domain {
            return Err(IndexStoreError::Cache);
        }
        Ok(stored.blocks.last().map(|block| block.hash))
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
        let Some(data) = read_capped(&self.path, MAX_CACHE_BYTES)? else {
            return PoolIndex::new(self.domain.first_block).map_err(Into::into);
        };
        let header: Header = serde_json::from_slice(&data).map_err(|_| IndexStoreError::Cache)?;
        match header.version {
            1 => {
                // Version 1 discarded deal identities from transfer events. This public cache
                // must be rescanned before it can support settlement observation.
                PoolIndex::new(self.domain.first_block).map_err(Into::into)
            }
            LEGACY_VERSION => {
                let stored: StoredIndex =
                    serde_json::from_slice(&data).map_err(|_| IndexStoreError::Cache)?;
                if stored.domain != self.domain {
                    return Err(IndexStoreError::Cache);
                }
                let mut index = PoolIndex::new(self.domain.first_block)?;
                for block in stored.blocks {
                    index.apply_block(block)?;
                }
                self.check_index(&index)?;
                Ok(index)
            }
            VERSION => {
                let manifest: Manifest =
                    serde_json::from_slice(&data).map_err(|_| IndexStoreError::Cache)?;
                if manifest.domain != self.domain {
                    return Err(IndexStoreError::Cache);
                }
                let mut index = PoolIndex::new(self.domain.first_block)?;
                let mut previous_last = None;
                for descriptor in &manifest.segments {
                    if previous_last.is_some_and(|last: u64| descriptor.first_block != last + 1) {
                        return Err(IndexStoreError::Cache);
                    }
                    if previous_last.is_none() && descriptor.first_block != self.domain.first_block
                    {
                        return Err(IndexStoreError::Cache);
                    }
                    let path = self.segment_dir.join(&descriptor.file);
                    regular_or_absent(&path)?;
                    let Some(bytes) = read_capped(&path, MAX_SEGMENT_BYTES)? else {
                        return Err(IndexStoreError::Cache);
                    };
                    if bytes.len() as u64 != descriptor.bytes
                        || keccak256(&[bytes.as_slice()]) != descriptor.digest
                    {
                        return Err(IndexStoreError::Cache);
                    }
                    let segment: SegmentFile =
                        serde_json::from_slice(&bytes).map_err(|_| IndexStoreError::Cache)?;
                    if segment.version != VERSION
                        || segment.first_block != descriptor.first_block
                        || segment.last_block != descriptor.last_block
                        || segment.blocks.first().map(|block| block.number)
                            != Some(descriptor.first_block)
                        || segment.blocks.last().map(|block| block.hash)
                            != Some(descriptor.last_hash)
                    {
                        return Err(IndexStoreError::Cache);
                    }
                    for block in segment.blocks {
                        index.apply_block(block)?;
                    }
                    previous_last = Some(descriptor.last_block);
                }
                if manifest.tip_hash != [0; 32]
                    && index.tip().map(|tip| (tip.number, tip.hash))
                        != Some((manifest.tip_number, manifest.tip_hash))
                {
                    return Err(IndexStoreError::Cache);
                }
                self.check_index(&index)?;
                Ok(index)
            }
            _ => Err(IndexStoreError::Cache),
        }
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

    fn write_atomic(&self, target: &Path, data: &[u8]) -> Result<(), IndexStoreError> {
        let parent = target.parent().ok_or(IndexStoreError::Cache)?;
        fs::create_dir_all(parent)?;
        if !fs::symlink_metadata(parent)?.file_type().is_dir() {
            return Err(IndexStoreError::Cache);
        }
        let mut suffix = [0u8; 8];
        OsRng.fill_bytes(&mut suffix);
        let temp = target.with_extension(format!("tmp-{}", hex::encode(suffix)));
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
            fs::rename(&temp, target)?;
            self.faults.after(Boundary {
                step: Step::Renamed,
                path: target,
            })?;
            File::open(parent)?.sync_all()?;
            self.faults.after(Boundary {
                step: Step::DirectorySynced,
                path: parent,
            })?;
            Ok::<(), IndexStoreError>(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temp);
        }
        result
    }
}

fn read_capped(path: &Path, maximum: u64) -> Result<Option<Vec<u8>>, IndexStoreError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if file.metadata()?.len() > maximum {
        return Err(IndexStoreError::Cache);
    }
    let mut data = Vec::new();
    file.take(maximum + 1).read_to_end(&mut data)?;
    if data.len() as u64 > maximum {
        return Err(IndexStoreError::Cache);
    }
    Ok(Some(data))
}

fn segment_first_block(bytes: &[u8]) -> Result<u64, IndexStoreError> {
    let segment: SegmentFile = serde_json::from_slice(bytes).map_err(|_| IndexStoreError::Cache)?;
    Ok(segment.first_block)
}

fn segment_last_block(bytes: &[u8]) -> Result<u64, IndexStoreError> {
    let segment: SegmentFile = serde_json::from_slice(bytes).map_err(|_| IndexStoreError::Cache)?;
    Ok(segment.last_block)
}

fn segment_last_hash(bytes: &[u8]) -> Result<[u8; 32], IndexStoreError> {
    let segment: SegmentFile = serde_json::from_slice(bytes).map_err(|_| IndexStoreError::Cache)?;
    segment
        .blocks
        .last()
        .map(|block| block.hash)
        .ok_or(IndexStoreError::Cache)
}

fn regular_or_absent(path: &Path) -> Result<(), IndexStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => Err(IndexStoreError::Cache),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
