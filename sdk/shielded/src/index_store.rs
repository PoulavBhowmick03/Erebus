//! Restartable public pool index cache. It contains no note openings.
//!
//! # On-disk form (version 4)
//!
//! `path` is a JSON manifest; the history itself lives in fixed-height segment files under
//! `path.with_extension("segments")/`. Segment `k` holds blocks `[base + k·N, base + (k+1)·N)`,
//! where `base` is the deployment block, or the block after a checkpoint, and `N` is
//! `segment_blocks`. Each block is one record `hash[32] ‖ events_len u32 ‖ canonical-JSON(events)`;
//! the number and parent hash are implied by position and rechecked on load.
//!
//! The manifest names every segment with its length, SHA-256 and end state (root, leaf count and
//! history digest), and carries its own SHA-256. A segment is sealed, and never rewritten again,
//! once the tip is `reorg_margin` blocks past its end. The manifest rename is the only commit point:
//! segment files are written and synced first, and files the manifest does not name are never read.
//!
//! Version 2 and 3 single-file caches still load; the next save migrates them.

use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use ark_std::rand::{rngs::OsRng, RngCore};
use erebus_journal::{Boundary, FaultHook, NoFaults, Step};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::indexer::{
    canonical_events, history_digest_step, IndexError, PoolBlock, PoolEvent, PoolIndex,
    TreeCheckpoint, EMPTY_HISTORY_DIGEST,
};

/// Legacy single-file cache of deployment-era history.
const VERSION: u32 = 2;
/// Legacy single-file cache that starts from a verified checkpoint.
const CHECKPOINT_VERSION: u32 = 3;
/// Manifest plus segment files. Versions 2 and 3 binaries reject it: it has no `blocks` field.
const MANIFEST_VERSION: u32 = 4;
const MANIFEST_KIND: &str = "erebus-pool-history";
/// Read limit for legacy single-file caches only; segmented history has no aggregate cap.
const MAX_LEGACY_CACHE_BYTES: u64 = 128 * 1024 * 1024;
/// A manifest is about 600 bytes per segment; this allows far more segments than any chain needs.
const MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;
/// Largest encoded event list of one block. Writes above it fail instead of producing an
/// unreadable segment.
const MAX_RECORD_EVENT_BYTES: u32 = 4 * 1024 * 1024;
const RECORD_HEADER_BYTES: usize = 36;
/// Blocks per segment: about 5.5 hours of Monad testnet at 3.3 blocks per second.
pub const DEFAULT_SEGMENT_BLOCKS: u64 = 65_536;
/// Blocks past a segment's end before it is sealed. Reorgs deeper than this into a sealed
/// segment are refused, not rewritten.
pub const DEFAULT_REORG_MARGIN: u64 = 1_024;

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
    /// Cache is malformed, incomplete, unsafe on disk, or built for another deployment.
    #[error("public index cache is malformed or belongs to another deployment")]
    Cache,
    /// Deployment anchor is missing or invalid.
    #[error("invalid public index deployment identity")]
    Domain,
    /// Another scanner changed this cache since it was read.
    #[error("public index cache changed concurrently; reload before saving")]
    Stale,
    /// Cached event history failed independent verification, or a save would rewrite sealed
    /// history ([`IndexError::Checkpoint`]).
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

#[derive(Deserialize)]
struct VersionPeek {
    version: u32,
}

/// A 32-byte value stored as lowercase hex.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Hex32([u8; 32]);

impl Serialize for Hex32 {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(self.0))
    }
}

impl<'de> Deserialize<'de> for Hex32 {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let bytes = hex::decode(&text).map_err(serde::de::Error::custom)?;
        bytes
            .try_into()
            .map(Hex32)
            .map_err(|_| serde::de::Error::custom("expected 32 bytes"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestTip {
    number: u64,
    hash: Hex32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SegmentEntry {
    k: u64,
    file: String,
    first: u64,
    last: u64,
    last_hash: Hex32,
    byte_len: u64,
    sha256: Hex32,
    end_root: Hex32,
    end_leaf_count: u32,
    history_digest: Hex32,
    sealed: bool,
}

/// The committed description of one segmented history. Every field is either rechecked against
/// the segment bytes and replay on load, or binds the cache to its deployment and layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    kind: String,
    domain: IndexDomain,
    checkpoint: Option<TreeCheckpoint>,
    segment_blocks: u64,
    reorg_margin: u64,
    generation: u64,
    /// Parent hash of the first block; the only link not implied by an earlier record.
    base_parent_hash: Option<Hex32>,
    tip: Option<ManifestTip>,
    root: Hex32,
    leaf_count: u32,
    sealed_frontier: Option<u64>,
    segments: Vec<SegmentEntry>,
    /// SHA-256 of this manifest serialized without this field. Absent only while hashing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    manifest_sha256: Option<Hex32>,
}

impl Manifest {
    fn body_sha256(&self) -> Result<[u8; 32], IndexStoreError> {
        let mut body = self.clone();
        body.manifest_sha256 = None;
        Ok(sha256(
            &serde_json::to_vec(&body).map_err(|_| IndexStoreError::Cache)?,
        ))
    }

    /// The manifest with every field except its generation and hash, to detect no-op saves.
    fn content(&self) -> Self {
        Self {
            generation: 0,
            manifest_sha256: None,
            ..self.clone()
        }
    }
}

/// Segment size and seal margin, fixed per cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Layout {
    segment_blocks: u64,
    reorg_margin: u64,
}

/// A loaded, verified cache: its manifest (none for empty or legacy files) and rebuilt index.
struct Stored {
    manifest: Option<Manifest>,
    index: PoolIndex,
}

impl Stored {
    fn sealed_frontier(&self) -> Option<u64> {
        self.manifest.as_ref().and_then(|m| m.sealed_frontier)
    }
}

/// Sealed history this process has verified for one cache path, and the index state at its end.
///
/// Sealed segments are immutable, so a later load whose manifest names the same sealed entries
/// (file, length, SHA-256 and end state) starts from this state instead of replaying them.
struct VerifiedPrefix {
    domain: IndexDomain,
    checkpoint: Option<TreeCheckpoint>,
    layout: Layout,
    sealed: Vec<SegmentEntry>,
    /// Index sealed through the last entry of `sealed`, with an empty tail.
    index: PoolIndex,
}

fn verified_prefixes() -> &'static Mutex<HashMap<PathBuf, Arc<VerifiedPrefix>>> {
    static PREFIXES: OnceLock<Mutex<HashMap<PathBuf, Arc<VerifiedPrefix>>>> = OnceLock::new();
    PREFIXES.get_or_init(Default::default)
}

/// One segment file write planned by a save.
enum SegmentWrite {
    /// Keep the first `keep` committed bytes of an unsealed file and append `bytes`.
    Append {
        file: String,
        keep: u64,
        bytes: Vec<u8>,
    },
    /// Create a new file for this generation.
    Create { file: String, bytes: Vec<u8> },
}

/// Atomic, exclusively locked public index cache for one deployment.
pub struct IndexStore {
    path: PathBuf,
    lock_path: PathBuf,
    segments_path: PathBuf,
    domain: IndexDomain,
    // Boxed: the frontier is ~700 bytes and stores are held inline by larger enums.
    checkpoint: Option<Box<TreeCheckpoint>>,
    layout: Layout,
    faults: Arc<dyn FaultHook>,
}

impl IndexStore {
    /// Whether two handles index the same deployment from the same starting state.
    pub fn same_history(&self, other: &Self) -> bool {
        self.domain == other.domain && self.checkpoint == other.checkpoint
    }

    /// Replaces this cache with `source` when `source` is the same history and covers more of it.
    ///
    /// Both caches are verified on load, so the copy carries no trust beyond what each already
    /// had; the caller keeps its own anchor and finality checks. Returns whether the cache
    /// changed. A concurrent writer wins: `Stale` leaves this cache as that writer saved it.
    ///
    /// The two caches' locks are never held at the same time. Sealed history is copied as
    /// segment files: they are staged under the source lock, then re-hashed, re-verified and
    /// committed under this cache's lock. This cache's own sealed segments must be identical in
    /// the source, or the seed fails with [`IndexError::Checkpoint`].
    pub fn seed_from(&self, source: &Self) -> Result<bool, IndexStoreError> {
        if !self.same_history(source) || self.layout != source.layout {
            return Err(IndexStoreError::Domain);
        }
        let current = self.load()?;
        let seed = source.load()?;
        if covered(&seed) <= covered(&current) {
            return Ok(false);
        }
        let expected = current.tip().map(|tip| tip.hash);
        let result = if seed.sealed_through().is_none() {
            // Nothing sealed: the whole history is in memory and saves like any other index.
            self.save_if_unchanged(&seed, expected)
        } else {
            self.seed_segments(source, expected)
        };
        match result {
            Ok(()) => Ok(true),
            Err(IndexStoreError::Stale) => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn seed_segments(
        &self,
        source: &Self,
        expected_tip: Option<[u8; 32]>,
    ) -> Result<(), IndexStoreError> {
        create_private_directory(&self.segments_path)?;
        let mut suffix = [0u8; 8];
        OsRng.fill_bytes(&mut suffix);
        let staging = self
            .segments_path
            .join(format!("seed-{}", hex::encode(suffix)));
        create_new_private_directory(&staging)?;
        let result = source.stage_segments(&staging).and_then(|manifest| {
            self.commit_staged(&staging, &manifest, source.path(), expected_tip)
        });
        // Moved files are already gone; anything left is an unused copy.
        let _ = fs::remove_dir_all(&staging);
        result
    }

    /// Copies the committed bytes of every segment this cache names into `staging`, under
    /// this cache's lock, and returns the manifest that names them.
    fn stage_segments(&self, staging: &Path) -> Result<Vec<u8>, IndexStoreError> {
        let lock = self.lock()?;
        let stored = self.load_locked()?;
        let manifest = stored.manifest.ok_or(IndexStoreError::Cache)?;
        for entry in &manifest.segments {
            let bytes = read_segment(&self.segments_path, entry)?;
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            private_create(&mut options);
            let mut file = options.open(staging.join(&entry.file))?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        File::open(staging)?.sync_all()?;
        drop(lock);
        serde_json::to_vec(&manifest).map_err(|_| IndexStoreError::Cache)
    }

    /// Verifies a staged copy and commits it over this cache, keeping identical segments.
    fn commit_staged(
        &self,
        staging: &Path,
        data: &[u8],
        source_key: &Path,
        expected_tip: Option<[u8; 32]>,
    ) -> Result<(), IndexStoreError> {
        let lock = self.lock()?;
        let stored = self.load_locked()?;
        if stored.index.tip().map(|tip| tip.hash) != expected_tip {
            return Err(IndexStoreError::Stale);
        }
        // Re-hash every staged byte, then rebuild: segments this process has not already verified
        // for the source are replayed in full.
        let staged = self.load_manifest(data, staging, Some(source_key))?;
        let manifest = staged.manifest.as_ref().ok_or(IndexStoreError::Cache)?;
        for entry in &manifest.segments {
            read_segment(staging, entry)?;
        }
        if covered(&staged.index) <= covered(&stored.index) {
            return Err(IndexStoreError::Stale);
        }
        let common = shared_prefix(&stored.index, &staged.index)?;
        if let Some(frontier) = stored.sealed_frontier() {
            if common.is_none_or(|common| common < frontier) {
                return Err(IndexError::Checkpoint.into());
            }
        }
        let old = stored.manifest.as_ref();
        let generation = old.map_or(0, |old| old.generation) + 1;
        let mut segments = Vec::new();
        let mut moves = Vec::new();
        for entry in &manifest.segments {
            let kept = old
                .and_then(|old| old.segments.get(entry.k as usize))
                .filter(|kept| same_content(kept, entry));
            segments.push(match kept {
                Some(kept) => SegmentEntry {
                    sealed: kept.sealed || entry.sealed,
                    ..kept.clone()
                },
                None => {
                    let file = segment_name(entry.k, generation);
                    moves.push((entry.file.clone(), file.clone()));
                    SegmentEntry {
                        file,
                        ..entry.clone()
                    }
                }
            });
        }
        if let Some(old) = old {
            for sealed in old.segments.iter().filter(|entry| entry.sealed) {
                if segments
                    .get(sealed.k as usize)
                    .is_none_or(|entry| !same_content(entry, sealed))
                {
                    return Err(IndexError::Checkpoint.into());
                }
            }
        }
        if segments
            .iter()
            .skip_while(|entry| entry.sealed)
            .any(|entry| entry.sealed)
        {
            return Err(IndexStoreError::Cache);
        }
        let mut seeded = Manifest {
            generation,
            sealed_frontier: segments
                .iter()
                .take_while(|entry| entry.sealed)
                .last()
                .map(|entry| entry.last),
            segments,
            manifest_sha256: None,
            ..manifest.clone()
        };
        seeded.manifest_sha256 = Some(Hex32(seeded.body_sha256()?));
        self.remove_unreferenced(old)?;
        let moved = !moves.is_empty();
        for (from, to) in moves {
            let target = self.segments_path.join(to);
            fs::rename(staging.join(from), &target)?;
            self.faults.after(Boundary {
                step: Step::Renamed,
                path: &target,
            })?;
        }
        if moved {
            File::open(&self.segments_path)?.sync_all()?;
            self.faults.after(Boundary {
                step: Step::DirectorySynced,
                path: &self.segments_path,
            })?;
        }
        let data = serde_json::to_vec(&seeded).map_err(|_| IndexStoreError::Cache)?;
        self.write_manifest(&data)?;
        self.remove_unreferenced(Some(&seeded))?;
        drop(lock);
        Ok(())
    }

    /// The cache manifest, which identifies this history within a process.
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

    /// Test-only: the same store with a small segment size and seal margin.
    ///
    /// A cache must always be opened with the layout it was written with; any other layout
    /// refuses to load it.
    #[doc(hidden)]
    pub fn with_layout(
        mut self,
        segment_blocks: u64,
        reorg_margin: u64,
    ) -> Result<Self, IndexStoreError> {
        if segment_blocks == 0 || segment_blocks > 1 << 32 || reorg_margin > 1 << 32 {
            return Err(IndexStoreError::Domain);
        }
        self.layout = Layout {
            segment_blocks,
            reorg_margin,
        };
        Ok(self)
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
        let segments_path = path.with_extension("segments");
        regular_or_absent(&path)?;
        regular_or_absent(&lock_path)?;
        directory_or_absent(&segments_path)?;
        Ok(Self {
            path,
            lock_path,
            segments_path,
            domain,
            checkpoint: None,
            layout: Layout {
                segment_blocks: DEFAULT_SEGMENT_BLOCKS,
                reorg_margin: DEFAULT_REORG_MARGIN,
            },
            faults,
        })
    }

    /// Verifies the cache and rebuilds its index.
    ///
    /// Every segment's hash and every transition is checked, except sealed segments this
    /// process already verified for this path (they are immutable, and their manifest entries,
    /// hashes and end states must match exactly). The index is sealed through the cache's sealed
    /// frontier: earlier empty blocks are not kept in memory.
    pub fn load(&self) -> Result<PoolIndex, IndexStoreError> {
        let lock = self.lock()?;
        let stored = self.load_locked()?;
        drop(lock);
        Ok(stored.index)
    }

    /// Loads the cache and up to `limit` verified blocks after `after`, empty ones included.
    ///
    /// Sealed blocks are decoded from their segment files, whose hashes are checked again;
    /// the rest come from the loaded tail. Both are read under one lock.
    pub fn load_with_blocks(
        &self,
        after: Option<u64>,
        limit: usize,
    ) -> Result<(PoolIndex, Vec<PoolBlock>), IndexStoreError> {
        let lock = self.lock()?;
        let stored = self.load_locked()?;
        let index = &stored.index;
        let mut blocks = Vec::new();
        let start = match after {
            Some(after) => after.checked_add(1),
            None => Some(index.first_block()),
        };
        if let (Some(tip), Some(start)) = (index.tip().map(|tip| tip.number), start) {
            let mut decoded: Option<(u64, Vec<PoolBlock>)> = None;
            let mut height = start.max(index.first_block());
            while blocks.len() < limit && height <= tip {
                if index
                    .sealed_through()
                    .is_some_and(|through| height < through)
                {
                    let manifest = stored.manifest.as_ref().ok_or(IndexStoreError::Cache)?;
                    let k = (height - self.base_block()) / self.layout.segment_blocks;
                    let entry = manifest
                        .segments
                        .get(k as usize)
                        .ok_or(IndexStoreError::Cache)?;
                    if decoded.as_ref().is_none_or(|(held, _)| *held != k) {
                        let parent = match k {
                            0 => manifest.base_parent_hash,
                            _ => manifest
                                .segments
                                .get(k as usize - 1)
                                .map(|previous| previous.last_hash),
                        }
                        .ok_or(IndexStoreError::Cache)?;
                        let bytes = read_segment(&self.segments_path, entry)?;
                        decoded = Some((
                            k,
                            decode_records(&bytes, entry.first, entry.last, parent.0)?,
                        ));
                    }
                    let block = decoded
                        .as_ref()
                        .and_then(|(_, held)| held.get((height - entry.first) as usize))
                        .ok_or(IndexStoreError::Cache)?;
                    blocks.push(block.clone());
                } else {
                    blocks.push(index.block(height).ok_or(IndexStoreError::Cache)?.clone());
                }
                height += 1;
            }
        }
        drop(lock);
        Ok((stored.index, blocks))
    }

    /// How many tail blocks a long-running scan may hold before reloading to seal them.
    pub(crate) fn retained_tail_limit(&self) -> u64 {
        2 * (self.layout.segment_blocks + self.layout.reorg_margin)
    }

    /// Persists one verified index only if the cache has not changed since `load`.
    ///
    /// `expected_tip` is the hash observed when the caller loaded the cache, or `None`
    /// for an empty cache. The save is also `Stale` when the cache holds the same block hashes
    /// as `index` up to their last shared block but different events. A reorg may replace the
    /// old suffix once the RPC has been checked against the trusted deployment anchor, but only
    /// within unsealed segments; replacing sealed history fails with [`IndexError::Checkpoint`]
    /// and leaves the cache unchanged.
    pub fn save_if_unchanged(
        &self,
        index: &PoolIndex,
        expected_tip: Option<[u8; 32]>,
    ) -> Result<(), IndexStoreError> {
        self.check_index(index)?;
        let lock = self.lock()?;
        let stored = self.load_locked()?;
        if stored.index.tip().map(|tip| tip.hash) != expected_tip {
            return Err(IndexStoreError::Stale);
        }
        let common = shared_prefix(&stored.index, index)?;
        if let Some(frontier) = stored.sealed_frontier() {
            if common.is_none_or(|common| common < frontier) {
                return Err(IndexError::Checkpoint.into());
            }
        }
        let (manifest, writes) = self.plan(&stored, index, common)?;
        if writes.is_empty()
            && stored
                .manifest
                .as_ref()
                .is_some_and(|old| old.content() == manifest.content())
        {
            // Nothing to commit; the cache already holds exactly this history. Leftovers of
            // an interrupted cleanup are still removed.
            self.remove_unreferenced(stored.manifest.as_ref())?;
            return Ok(());
        }
        self.commit(stored.manifest.as_ref(), &manifest, writes)?;
        drop(lock);
        Ok(())
    }

    fn empty_index(&self) -> Result<PoolIndex, IndexStoreError> {
        Ok(match self.checkpoint() {
            Some(checkpoint) => PoolIndex::from_checkpoint(checkpoint)?,
            None => PoolIndex::new(self.domain.first_block)?,
        })
    }

    /// First block of segment 0.
    fn base_block(&self) -> u64 {
        match self.checkpoint() {
            Some(checkpoint) => checkpoint.block_number + 1,
            None => self.domain.first_block,
        }
    }

    /// Checks that an index starts at this cache's anchor. Its transitions were already verified
    /// by [`PoolIndex::apply_block`], the only way to extend one.
    fn check_index(&self, index: &PoolIndex) -> Result<(), IndexStoreError> {
        if index.checkpoint().copied() != self.checkpoint() {
            return Err(IndexStoreError::Domain);
        }
        let link = index.first_link();
        match self.checkpoint() {
            // The first scanned block must extend the verified checkpoint block.
            Some(checkpoint) => {
                if index.first_block() != checkpoint.block_number + 1
                    || link.is_some_and(|(_, parent)| parent != checkpoint.block_hash)
                {
                    return Err(IndexStoreError::Domain);
                }
            }
            None => {
                if index.first_block() != self.domain.first_block
                    || link.is_some_and(|(hash, _)| hash != self.domain.first_hash)
                {
                    return Err(IndexStoreError::Domain);
                }
            }
        }
        Ok(())
    }

    fn load_locked(&self) -> Result<Stored, IndexStoreError> {
        regular_or_absent(&self.path)?;
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Stored {
                    manifest: None,
                    index: self.empty_index()?,
                })
            }
            Err(error) => return Err(error.into()),
        };
        same_file(&file, &self.path)?;
        let limit = MAX_LEGACY_CACHE_BYTES.max(MAX_MANIFEST_BYTES);
        if file.metadata()?.len() > limit {
            return Err(IndexStoreError::Cache);
        }
        let mut data = Vec::new();
        file.take(limit + 1).read_to_end(&mut data)?;
        let peek: VersionPeek =
            serde_json::from_slice(&data).map_err(|_| IndexStoreError::Cache)?;
        if peek.version == MANIFEST_VERSION {
            if data.len() as u64 > MAX_MANIFEST_BYTES {
                return Err(IndexStoreError::Cache);
            }
            private_file(&self.path)?;
            return self.load_manifest(&data, &self.segments_path, Some(&self.path));
        }
        if data.len() as u64 > MAX_LEGACY_CACHE_BYTES {
            return Err(IndexStoreError::Cache);
        }
        Ok(Stored {
            manifest: None,
            index: self.load_legacy(&data)?,
        })
    }

    fn load_legacy(&self, data: &[u8]) -> Result<PoolIndex, IndexStoreError> {
        let stored: StoredIndex =
            serde_json::from_slice(data).map_err(|_| IndexStoreError::Cache)?;
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

    /// Verifies a manifest and the segments it names in `dir`, and rebuilds the index.
    ///
    /// Sealed segments that this process already verified for `prefix_key` (the same entries,
    /// hashes and end states) are not read or replayed again; every other segment is. The returned
    /// index is sealed through the manifest's sealed frontier.
    fn load_manifest(
        &self,
        data: &[u8],
        dir: &Path,
        prefix_key: Option<&Path>,
    ) -> Result<Stored, IndexStoreError> {
        let manifest: Manifest =
            serde_json::from_slice(data).map_err(|_| IndexStoreError::Cache)?;
        // Exactly one encoding is accepted, and its hash must cover every other field.
        if serde_json::to_vec(&manifest).map_err(|_| IndexStoreError::Cache)? != data
            || manifest.manifest_sha256.map(|hash| hash.0) != Some(manifest.body_sha256()?)
        {
            return Err(IndexStoreError::Cache);
        }
        if manifest.version != MANIFEST_VERSION
            || manifest.kind != MANIFEST_KIND
            || manifest.domain != self.domain
            || manifest.checkpoint != self.checkpoint()
            || manifest.segment_blocks != self.layout.segment_blocks
            || manifest.reorg_margin != self.layout.reorg_margin
            || manifest.generation == 0
        {
            return Err(IndexStoreError::Cache);
        }
        let sealed_count = manifest
            .segments
            .iter()
            .take_while(|entry| entry.sealed)
            .count();
        let (mut index, verified) = match prefix_key
            .and_then(|key| self.verified_prefix(key, &manifest.segments[..sealed_count]))
        {
            Some(prefix) => (prefix.index.clone(), prefix.sealed.len()),
            None => (self.empty_index()?, 0),
        };
        let mut parent = match verified {
            0 => manifest.base_parent_hash.map(|hash| hash.0),
            _ => index.tip().map(|tip| tip.hash),
        };
        let mut frontier = None;
        let mut unsealed = false;
        for (position, entry) in manifest.segments.iter().enumerate() {
            let (first, end) = self.segment_bounds(entry.k)?;
            let last_segment = position + 1 == manifest.segments.len();
            if entry.k != position as u64
                || entry.first != first
                || entry.last < first
                || entry.last > end
                || (!last_segment && entry.last != end)
                || parse_segment_name(&entry.file)
                    .is_none_or(|(k, generation)| k != entry.k || generation > manifest.generation)
            {
                return Err(IndexStoreError::Cache);
            }
            // Sealed segments are complete and form a prefix of the history.
            if entry.sealed {
                if unsealed || entry.last != end {
                    return Err(IndexStoreError::Cache);
                }
                frontier = Some(entry.last);
            } else {
                unsealed = true;
            }
            if position < verified {
                // Verified earlier in this process; its entry matched exactly.
                continue;
            }
            {
                let bytes = read_segment(dir, entry)?;
                let blocks = decode_records(
                    &bytes,
                    entry.first,
                    entry.last,
                    parent.ok_or(IndexStoreError::Cache)?,
                )?;
                for block in blocks {
                    index.apply_block(block)?;
                }
            }
            let tip = index.tip().ok_or(IndexStoreError::Cache)?;
            if tip.number != entry.last
                || tip.hash != entry.last_hash.0
                || index.root() != entry.end_root.0
                || index.leaf_count() != entry.end_leaf_count
                || index.history_digest() != entry.history_digest.0
            {
                return Err(IndexStoreError::Cache);
            }
            parent = Some(tip.hash);
            if position + 1 == sealed_count {
                // Drop sealed empty blocks from memory and remember the verified prefix.
                index.seal()?;
                if let Some(key) = prefix_key {
                    self.remember_prefix(key, &manifest.segments[..sealed_count], &index);
                }
            }
        }
        let tip = index.tip().map(|tip| ManifestTip {
            number: tip.number,
            hash: Hex32(tip.hash),
        });
        if tip != manifest.tip
            || index.root() != manifest.root.0
            || index.leaf_count() != manifest.leaf_count
            || frontier != manifest.sealed_frontier
            || index.sealed_through() != frontier
            || (manifest.segments.is_empty() && manifest.base_parent_hash.is_some())
            || (!manifest.segments.is_empty()
                && manifest.base_parent_hash.map(|hash| hash.0)
                    != index.first_link().map(|(_, parent)| parent))
        {
            return Err(IndexStoreError::Cache);
        }
        self.check_index(&index)?;
        Ok(Stored {
            manifest: Some(manifest),
            index,
        })
    }

    /// The verified sealed prefix this process holds for `key`, if `sealed` starts with it.
    fn verified_prefix(&self, key: &Path, sealed: &[SegmentEntry]) -> Option<Arc<VerifiedPrefix>> {
        let prefixes = verified_prefixes()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let prefix = prefixes.get(key)?;
        (prefix.domain == self.domain
            && prefix.checkpoint == self.checkpoint()
            && prefix.layout == self.layout
            && !prefix.sealed.is_empty()
            && sealed.starts_with(&prefix.sealed))
        .then(|| Arc::clone(prefix))
    }

    fn remember_prefix(&self, key: &Path, sealed: &[SegmentEntry], index: &PoolIndex) {
        let prefix = Arc::new(VerifiedPrefix {
            domain: self.domain,
            checkpoint: self.checkpoint(),
            layout: self.layout,
            sealed: sealed.to_vec(),
            index: index.clone(),
        });
        verified_prefixes()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key.to_path_buf(), prefix);
    }

    /// Inclusive block range of segment `k`.
    fn segment_bounds(&self, k: u64) -> Result<(u64, u64), IndexStoreError> {
        let first = k
            .checked_mul(self.layout.segment_blocks)
            .and_then(|offset| self.base_block().checked_add(offset))
            .ok_or(IndexStoreError::Cache)?;
        let end = first
            .checked_add(self.layout.segment_blocks - 1)
            .ok_or(IndexStoreError::Cache)?;
        Ok((first, end))
    }

    /// Builds the manifest for `index` and the segment writes it needs.
    ///
    /// `common` is the last height at which the stored history and `index` are identical; every
    /// stored segment that ends at or before it is reused as is, and an unsealed one that `index`
    /// extends is appended to. Anything else is written as a new file of the next generation.
    fn plan(
        &self,
        stored: &Stored,
        index: &PoolIndex,
        common: Option<u64>,
    ) -> Result<(Manifest, Vec<SegmentWrite>), IndexStoreError> {
        let old = stored.manifest.as_ref();
        let generation = old.map_or(0, |old| old.generation) + 1;
        let shared = |height: u64| common.is_some_and(|common| height <= common);
        let block_at = |height: u64| -> Result<&PoolBlock, IndexStoreError> {
            let block = if shared(height) {
                stored.index.block(height).or_else(|| index.block(height))
            } else {
                index.block(height)
            };
            block.ok_or(IndexStoreError::Cache)
        };
        let empty = self.empty_index()?;
        let mut state = EndState {
            root: empty.root(),
            leaf_count: empty.leaf_count(),
            digest: EMPTY_HISTORY_DIGEST,
        };
        let mut segments = Vec::new();
        let mut writes = Vec::new();
        if let Some(tip) = index.tip() {
            let last_k = (tip.number - self.base_block()) / self.layout.segment_blocks;
            for k in 0..=last_k {
                let (first, end) = self.segment_bounds(k)?;
                let last = end.min(tip.number);
                let seals =
                    last == end && tip.number >= end.saturating_add(self.layout.reorg_margin);
                let previous = old.and_then(|old| old.segments.get(k as usize));
                let entry = match previous {
                    // Unchanged: the whole stored segment is shared history.
                    Some(entry) if entry.last == last && shared(entry.last) => {
                        state = EndState::of(entry);
                        SegmentEntry {
                            sealed: entry.sealed || seals,
                            ..entry.clone()
                        }
                    }
                    // Extended: append the new blocks to the unsealed file.
                    Some(entry) if !entry.sealed && entry.last < last && shared(entry.last) => {
                        state = EndState::of(entry);
                        let mut bytes = Vec::new();
                        let mut last_hash = entry.last_hash.0;
                        for height in entry.last + 1..=last {
                            let block = block_at(height)?;
                            encode_record(&mut bytes, block)?;
                            state.apply(block)?;
                            last_hash = block.hash;
                        }
                        let mut content = read_segment(&self.segments_path, entry)?;
                        content.extend_from_slice(&bytes);
                        writes.push(SegmentWrite::Append {
                            file: entry.file.clone(),
                            keep: entry.byte_len,
                            bytes,
                        });
                        state.entry(
                            k,
                            entry.file.clone(),
                            first,
                            last,
                            last_hash,
                            &content,
                            seals,
                        )
                    }
                    Some(entry) if entry.sealed => return Err(IndexError::Checkpoint.into()),
                    // New range, or a reorg inside an unsealed segment: write a fresh file.
                    _ => {
                        let mut content = Vec::new();
                        let mut last_hash = [0; 32];
                        for height in first..=last {
                            let block = block_at(height)?;
                            encode_record(&mut content, block)?;
                            state.apply(block)?;
                            last_hash = block.hash;
                        }
                        let file = segment_name(k, generation);
                        let entry =
                            state.entry(k, file.clone(), first, last, last_hash, &content, seals);
                        writes.push(SegmentWrite::Create {
                            file,
                            bytes: content,
                        });
                        entry
                    }
                };
                segments.push(entry);
            }
        }
        // Dropped segments (a shorter history) must all be unsealed.
        if old.is_some_and(|old| {
            old.segments
                .iter()
                .skip(segments.len())
                .any(|entry| entry.sealed)
        }) {
            return Err(IndexError::Checkpoint.into());
        }
        // The planned segments must describe exactly the verified index.
        if state.digest != index.history_digest()
            || state.root != index.root()
            || state.leaf_count != index.leaf_count()
        {
            return Err(IndexStoreError::Cache);
        }
        let mut manifest = Manifest {
            version: MANIFEST_VERSION,
            kind: MANIFEST_KIND.to_owned(),
            domain: self.domain,
            checkpoint: self.checkpoint(),
            segment_blocks: self.layout.segment_blocks,
            reorg_margin: self.layout.reorg_margin,
            generation,
            base_parent_hash: index.first_link().map(|(_, parent)| Hex32(parent)),
            tip: index.tip().map(|tip| ManifestTip {
                number: tip.number,
                hash: Hex32(tip.hash),
            }),
            root: Hex32(index.root()),
            leaf_count: index.leaf_count(),
            sealed_frontier: segments
                .iter()
                .take_while(|entry| entry.sealed)
                .last()
                .map(|entry| entry.last),
            segments,
            manifest_sha256: None,
        };
        // Sealed history is immutable: every stored sealed segment must survive unchanged.
        if let Some(old) = old {
            for sealed in old.segments.iter().filter(|entry| entry.sealed) {
                if manifest.segments.get(sealed.k as usize) != Some(sealed) {
                    return Err(IndexError::Checkpoint.into());
                }
            }
        }
        manifest.manifest_sha256 = Some(Hex32(manifest.body_sha256()?));
        Ok((manifest, writes))
    }

    /// Writes segment data, then renames the manifest over `path`: the only commit point.
    fn commit(
        &self,
        old: Option<&Manifest>,
        manifest: &Manifest,
        writes: Vec<SegmentWrite>,
    ) -> Result<(), IndexStoreError> {
        if !writes.is_empty() || !manifest.segments.is_empty() {
            create_private_directory(&self.segments_path)?;
        }
        // Files no committed manifest names are leftovers of an interrupted save.
        self.remove_unreferenced(old)?;
        let wrote = !writes.is_empty();
        for write in writes {
            match write {
                SegmentWrite::Append { file, keep, bytes } => {
                    let path = self.segments_path.join(&file);
                    let mut handle = open_private_file_for_append(&path)?;
                    // Drops the torn tail of any earlier append that never committed.
                    handle.set_len(keep)?;
                    handle.seek(SeekFrom::Start(keep))?;
                    handle.write_all(&bytes)?;
                    handle.sync_all()?;
                    self.faults.after(Boundary {
                        step: Step::BlobWritten,
                        path: &path,
                    })?;
                }
                SegmentWrite::Create { file, bytes } => {
                    let path = self.segments_path.join(&file);
                    let mut options = OpenOptions::new();
                    options.write(true).create_new(true);
                    private_create(&mut options);
                    let mut handle = options.open(&path)?;
                    handle.write_all(&bytes)?;
                    handle.sync_all()?;
                    self.faults.after(Boundary {
                        step: Step::BlobWritten,
                        path: &path,
                    })?;
                }
            }
        }
        if wrote {
            File::open(&self.segments_path)?.sync_all()?;
            self.faults.after(Boundary {
                step: Step::DirectorySynced,
                path: &self.segments_path,
            })?;
        }
        let data = serde_json::to_vec(manifest).map_err(|_| IndexStoreError::Cache)?;
        self.write_manifest(&data)?;
        self.remove_unreferenced(Some(manifest))
    }

    /// Removes segment files the given committed manifest does not name.
    ///
    /// This listing is used only to delete; history is never discovered from file names.
    fn remove_unreferenced(&self, manifest: Option<&Manifest>) -> Result<(), IndexStoreError> {
        let entries = match fs::read_dir(&self.segments_path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let mut removed = false;
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let referenced = manifest
                .is_some_and(|manifest| manifest.segments.iter().any(|seg| seg.file == name));
            if referenced || parse_segment_name(name).is_none() {
                continue;
            }
            let path = entry.path();
            // `remove_file` never follows a symbolic link, so a planted link is only unlinked.
            if fs::symlink_metadata(&path)?.file_type().is_dir() {
                continue;
            }
            fs::remove_file(&path)?;
            removed = true;
            self.faults.after(Boundary {
                step: Step::Removed,
                path: &path,
            })?;
        }
        if removed {
            File::open(&self.segments_path)?.sync_all()?;
            self.faults.after(Boundary {
                step: Step::DirectorySynced,
                path: &self.segments_path,
            })?;
        }
        Ok(())
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

    fn write_manifest(&self, data: &[u8]) -> Result<(), IndexStoreError> {
        let mut suffix = [0u8; 8];
        OsRng.fill_bytes(&mut suffix);
        let temp = self
            .path
            .with_extension(format!("tmp-{}", hex::encode(suffix)));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        private_create(&mut options);
        let mut file = options.open(&temp)?;
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

/// Root, leaf count and history digest after a block, tracked while planning segments.
struct EndState {
    root: [u8; 32],
    leaf_count: u32,
    digest: [u8; 32],
}

impl EndState {
    fn of(entry: &SegmentEntry) -> Self {
        Self {
            root: entry.end_root.0,
            leaf_count: entry.end_leaf_count,
            digest: entry.history_digest.0,
        }
    }

    /// Advances over one block that [`PoolIndex::apply_block`] has already verified.
    fn apply(&mut self, block: &PoolBlock) -> Result<(), IndexStoreError> {
        for event in &block.events {
            if let PoolEvent::Inserted { index, root, .. } = event {
                self.leaf_count = index.checked_add(1).ok_or(IndexStoreError::Cache)?;
                self.root = *root;
            }
        }
        self.digest = history_digest_step(&self.digest, block)?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn entry(
        &self,
        k: u64,
        file: String,
        first: u64,
        last: u64,
        last_hash: [u8; 32],
        content: &[u8],
        sealed: bool,
    ) -> SegmentEntry {
        SegmentEntry {
            k,
            file,
            first,
            last,
            last_hash: Hex32(last_hash),
            byte_len: content.len() as u64,
            sha256: Hex32(sha256(content)),
            end_root: Hex32(self.root),
            end_leaf_count: self.leaf_count,
            history_digest: Hex32(self.digest),
            sealed,
        }
    }
}

/// The last height at which `stored` and `incoming` hold the same block, or `None`.
///
/// Only heights both indexes retain are compared: their tails and last sealed blocks. Block
/// hashes chain through parent hashes, so equal hashes at a height mean equal hashes at every
/// lower one. Equal hashes with different history digests mean the same blocks were recorded
/// with different events: another writer replaced the cache, so the save is `Stale`. `None`
/// means no shared block is retained; a save then may not keep anything sealed.
fn shared_prefix(stored: &PoolIndex, incoming: &PoolIndex) -> Result<Option<u64>, IndexStoreError> {
    let (Some(stored_tip), Some(incoming_tip)) = (stored.tip(), incoming.tip()) else {
        return Ok(None);
    };
    let lowest = |index: &PoolIndex| index.sealed_through().unwrap_or(index.first_block());
    let floor = lowest(stored).max(lowest(incoming));
    let mut height = stored_tip.number.min(incoming_tip.number);
    while height >= floor {
        match (stored.block(height), incoming.block(height)) {
            (Some(ours), Some(theirs)) if ours.hash == theirs.hash => {
                return if stored.history_digest_at(height) == incoming.history_digest_at(height) {
                    Ok(Some(height))
                } else {
                    Err(IndexStoreError::Stale)
                };
            }
            (Some(_), Some(_)) => {}
            _ => return Err(IndexStoreError::Cache),
        }
        match height.checked_sub(1) {
            Some(lower) => height = lower,
            None => break,
        }
    }
    Ok(None)
}

fn covered(index: &PoolIndex) -> Option<u64> {
    index.covered_through().map(|(number, _)| number)
}

/// Whether two entries describe the same segment bytes and end state, whatever their file name.
fn same_content(left: &SegmentEntry, right: &SegmentEntry) -> bool {
    SegmentEntry {
        file: String::new(),
        sealed: false,
        ..left.clone()
    } == SegmentEntry {
        file: String::new(),
        sealed: false,
        ..right.clone()
    }
}

/// Reads exactly the committed bytes of one referenced segment in `dir` and checks their hash.
///
/// Bytes past `byte_len` are an append that never committed; the next save truncates them.
fn read_segment(dir: &Path, entry: &SegmentEntry) -> Result<Vec<u8>, IndexStoreError> {
    private_directory(dir)?;
    let path = dir.join(&entry.file);
    let file = open_private_file(&path)?.ok_or(IndexStoreError::Cache)?;
    if file.metadata()?.len() < entry.byte_len {
        return Err(IndexStoreError::Cache);
    }
    let mut bytes = Vec::new();
    file.take(entry.byte_len).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != entry.byte_len || sha256(&bytes) != entry.sha256.0 {
        return Err(IndexStoreError::Cache);
    }
    Ok(bytes)
}

fn segment_name(k: u64, generation: u64) -> String {
    format!("{k:08}-g{generation}.seg")
}

/// Parses a segment file name; only the exact form [`segment_name`] writes is accepted.
fn parse_segment_name(name: &str) -> Option<(u64, u64)> {
    let stem = name.strip_suffix(".seg")?;
    let (k, generation) = stem.split_once("-g")?;
    let k: u64 = k.parse().ok()?;
    let generation: u64 = generation.parse().ok()?;
    (segment_name(k, generation) == name).then_some((k, generation))
}

fn encode_record(out: &mut Vec<u8>, block: &PoolBlock) -> Result<(), IndexStoreError> {
    let events = canonical_events(&block.events)?;
    let length = u32::try_from(events.len())
        .ok()
        .filter(|length| *length <= MAX_RECORD_EVENT_BYTES)
        .ok_or(IndexStoreError::Cache)?;
    out.extend_from_slice(&block.hash);
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(&events);
    Ok(())
}

/// Decodes the records of blocks `first..=last`, whose first parent is `parent`.
fn decode_records(
    bytes: &[u8],
    first: u64,
    last: u64,
    mut parent: [u8; 32],
) -> Result<Vec<PoolBlock>, IndexStoreError> {
    let mut blocks = Vec::new();
    let mut rest = bytes;
    for number in first..=last {
        if rest.len() < RECORD_HEADER_BYTES {
            return Err(IndexStoreError::Cache);
        }
        let (header, tail) = rest.split_at(RECORD_HEADER_BYTES);
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&header[..32]);
        let mut length = [0u8; 4];
        length.copy_from_slice(&header[32..]);
        let length = u32::from_be_bytes(length);
        if length > MAX_RECORD_EVENT_BYTES || tail.len() < length as usize {
            return Err(IndexStoreError::Cache);
        }
        let (encoded, tail) = tail.split_at(length as usize);
        let events: Vec<PoolEvent> =
            serde_json::from_slice(encoded).map_err(|_| IndexStoreError::Cache)?;
        // One encoding per event list: anything else is not a record this store wrote.
        if canonical_events(&events)? != encoded {
            return Err(IndexStoreError::Cache);
        }
        blocks.push(PoolBlock {
            number,
            hash,
            parent_hash: parent,
            events,
        });
        parent = hash;
        rest = tail;
    }
    if !rest.is_empty() {
        return Err(IndexStoreError::Cache);
    }
    Ok(blocks)
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn regular_or_absent(path: &Path) -> Result<(), IndexStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => Err(IndexStoreError::Cache),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn directory_or_absent(path: &Path) -> Result<(), IndexStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(IndexStoreError::Cache),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Opens a regular, owner-only file that is not a symbolic link; `None` when absent.
fn open_private_file(path: &Path) -> Result<Option<File>, IndexStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => return Err(IndexStoreError::Cache),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    private_file(path)?;
    let file = File::open(path)?;
    same_file(&file, path)?;
    Ok(Some(file))
}

fn open_private_file_for_append(path: &Path) -> Result<File, IndexStoreError> {
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(IndexStoreError::Cache);
    }
    private_file(path)?;
    let file = OpenOptions::new().write(true).open(path)?;
    same_file(&file, path)?;
    Ok(file)
}

/// Refuses when `path` was swapped (for example for a symbolic link) after it was inspected.
#[cfg(unix)]
fn same_file(file: &File, path: &Path) -> Result<(), IndexStoreError> {
    use std::os::unix::fs::MetadataExt;
    let opened = file.metadata()?;
    let named = fs::symlink_metadata(path)?;
    if !named.file_type().is_file() || opened.dev() != named.dev() || opened.ino() != named.ino() {
        return Err(IndexStoreError::Cache);
    }
    Ok(())
}

#[cfg(not(unix))]
fn same_file(_file: &File, path: &Path) -> Result<(), IndexStoreError> {
    regular_or_absent(path)
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> Result<(), IndexStoreError> {
    use std::os::unix::fs::DirBuilderExt;
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    private_directory(path)
}

#[cfg(not(unix))]
fn create_private_directory(path: &Path) -> Result<(), IndexStoreError> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    private_directory(path)
}

/// Creates an owner-only directory that must not exist yet.
fn create_new_private_directory(path: &Path) -> Result<(), IndexStoreError> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    private_directory(path)
}

#[cfg(unix)]
fn private_directory(path: &Path) -> Result<(), IndexStoreError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            IndexStoreError::Cache
        } else {
            error.into()
        }
    })?;
    if !metadata.file_type().is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(IndexStoreError::Cache);
    }
    Ok(())
}

#[cfg(not(unix))]
fn private_directory(path: &Path) -> Result<(), IndexStoreError> {
    if !fs::symlink_metadata(path)?.file_type().is_dir() {
        return Err(IndexStoreError::Cache);
    }
    Ok(())
}

#[cfg(unix)]
fn private_file(path: &Path) -> Result<(), IndexStoreError> {
    use std::os::unix::fs::PermissionsExt;
    if fs::symlink_metadata(path)?.permissions().mode() & 0o077 != 0 {
        return Err(IndexStoreError::Cache);
    }
    Ok(())
}

#[cfg(not(unix))]
fn private_file(_path: &Path) -> Result<(), IndexStoreError> {
    Ok(())
}

#[cfg(unix)]
fn private_create(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

#[cfg(not(unix))]
fn private_create(_options: &mut OpenOptions) {}
