//! Segmented public history (index version 4): crash safety, corruption, migration and races.
//!
//! Most tests use a small layout (4-block segments, sealed 2 blocks after their end) so that a
//! few dozen blocks cross many segment and seal boundaries.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use erebus_core::shielded::{note_tree_parent, NOTE_TREE_DEPTH};
use erebus_shielded_prover::{
    index_store::{IndexDomain, IndexStore, IndexStoreError},
    indexer::{IndexError, PoolBlock, PoolEvent, PoolIndex, TreeCheckpoint},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

mod support;

const BASE: u64 = 100;
const SEGMENT: u64 = 4;
const MARGIN: u64 = 2;

/// A canonical BN254 field element derived from a label; the top byte stays zero.
fn field(kind: u8, fork: u8, number: u64, item: u8) -> [u8; 32] {
    let mut value = [0u8; 32];
    value[1] = kind;
    value[2] = fork;
    value[16..24].copy_from_slice(&number.to_be_bytes());
    value[31] = item.wrapping_add(1);
    value
}

/// Independent incremental Poseidon tree used only to script emitted roots.
struct Tree {
    zero: [[u8; 32]; NOTE_TREE_DEPTH],
    filled: [[u8; 32]; NOTE_TREE_DEPTH],
    next: u32,
}

impl Tree {
    fn new() -> Self {
        let mut zero = [[0u8; 32]; NOTE_TREE_DEPTH];
        let mut node = [0u8; 32];
        for item in &mut zero {
            *item = node;
            node = note_tree_parent(&node, &node).unwrap();
        }
        Self {
            zero,
            filled: zero,
            next: 0,
        }
    }

    fn insert(&mut self, leaf: [u8; 32]) -> [u8; 32] {
        let mut node = leaf;
        let mut position = self.next;
        for level in 0..NOTE_TREE_DEPTH {
            node = if position & 1 == 0 {
                self.filled[level] = node;
                note_tree_parent(&node, &self.zero[level])
            } else {
                note_tree_parent(&self.filled[level], &node)
            }
            .unwrap();
            position >>= 1;
        }
        self.next += 1;
        node
    }
}

/// Pseudo-random like a real block hash, so encoded sizes are realistic.
fn block_hash(number: u64, fork: u8) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(number.to_be_bytes());
    hasher.update([fork]);
    hasher.finalize().into()
}

/// `len` blocks from `BASE`; heights at or above `fork_at` belong to branch `fork`.
/// Insertions every `every` blocks, transfers every `every + 2`, withdrawals every `2·every + 1`
/// (5, 7 and 11 for [`chain`]).
fn chain_with(len: u64, fork_at: u64, fork: u8, every: u64) -> Vec<PoolBlock> {
    let mut tree = Tree::new();
    let mut blocks: Vec<PoolBlock> = Vec::new();
    let mut parent = [0x99; 32];
    for number in BASE..BASE + len {
        let branch = if number >= fork_at { fork } else { 0 };
        let hash = block_hash(number, branch);
        let mut events = Vec::new();
        let mut log_index = 0;
        let mut tx = [0u8; 32];
        tx[0] = 0x7f;
        tx[1] = branch;
        tx[24..].copy_from_slice(&number.to_be_bytes());
        if number % every == 0 {
            for item in 0..(1 + (number / every) % 2) as u8 {
                let commitment = field(1, branch, number, item);
                let index = tree.next;
                let root = tree.insert(commitment);
                events.push(PoolEvent::Inserted {
                    index,
                    commitment,
                    root,
                    tx_hash: tx,
                    log_index,
                });
                log_index += 1;
            }
        }
        if number % (every + 2) == 0 {
            events.push(PoolEvent::Transferred {
                deal_commitment: field(2, branch, number, 0),
                deal_nullifier: field(3, branch, number, 0),
                input_nullifier: field(4, branch, number, 0),
                tx_hash: tx,
                log_index,
            });
            log_index += 1;
        }
        if number % (2 * every + 1) == 0 {
            events.push(PoolEvent::Consumed {
                nullifier: field(5, branch, number, 0),
                tx_hash: tx,
                log_index,
            });
        }
        blocks.push(PoolBlock {
            number,
            hash,
            parent_hash: parent,
            events,
        });
        parent = hash;
    }
    blocks
}

fn chain(len: u64, fork_at: u64, fork: u8) -> Vec<PoolBlock> {
    chain_with(len, fork_at, fork, 5)
}

fn index_of(blocks: &[PoolBlock]) -> PoolIndex {
    let mut index = PoolIndex::new(BASE).unwrap();
    for block in blocks {
        index.apply_block(block.clone()).unwrap();
    }
    index
}

fn domain() -> IndexDomain {
    IndexDomain {
        chain_id: 31_337,
        pool: [3; 20],
        first_block: BASE,
        first_hash: block_hash(BASE, 0),
    }
}

fn small(path: &Path) -> IndexStore {
    IndexStore::new(path, domain())
        .unwrap()
        .with_layout(SEGMENT, MARGIN)
        .unwrap()
}

fn small_with_faults(path: &Path, hook: Arc<support::SweepHook>) -> IndexStore {
    IndexStore::with_faults(path, domain(), hook)
        .unwrap()
        .with_layout(SEGMENT, MARGIN)
        .unwrap()
}

fn same(left: &PoolIndex, right: &PoolIndex) -> bool {
    // Seal-agnostic: a loaded index keeps only blocks with events below its sealed frontier.
    left.tip() == right.tip()
        && left.event_blocks().eq(right.event_blocks())
        && left.root() == right.root()
        && left.leaves() == right.leaves()
        && left.history_digest() == right.history_digest()
}

fn tip(index: &PoolIndex) -> Option<[u8; 32]> {
    index.tip().map(|tip| tip.hash)
}

fn segments_dir(path: &Path) -> PathBuf {
    path.with_extension("segments")
}

fn manifest(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn referenced(path: &Path) -> BTreeSet<String> {
    manifest(path)["segments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["file"].as_str().unwrap().to_owned())
        .collect()
}

fn on_disk(path: &Path) -> BTreeSet<String> {
    match fs::read_dir(segments_dir(path)) {
        Ok(entries) => entries
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect(),
        Err(_) => BTreeSet::new(),
    }
}

/// Path of the file the manifest names for segment `k`.
fn segment_file(path: &Path, k: usize) -> PathBuf {
    segments_dir(path).join(manifest(path)["segments"][k]["file"].as_str().unwrap())
}

fn is_cache_error(result: Result<PoolIndex, IndexStoreError>) -> bool {
    matches!(result, Err(IndexStoreError::Cache))
}

/// Recomputes `manifest_sha256` over the manifest text with that last field removed.
fn rehash_manifest(text: &str) -> String {
    let marker = ",\"manifest_sha256\":\"";
    let start = text.find(marker).expect("hash field");
    let body = format!("{}}}", &text[..start]);
    let digest = hex::encode(Sha256::digest(body.as_bytes()));
    format!("{}{marker}{digest}\"}}", &text[..start])
}

/// Every failure point of `save(new)` over a cache prepared by `setup` leaves exactly the old
/// or the new verified history, and a later save always converges to the new one.
fn sweep(setup: impl Fn(&Path), old: &PoolIndex, new: &PoolIndex, legacy: bool) -> usize {
    let mut boundaries = 0;
    let mut fail = 0;
    loop {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("index.json");
        setup(&path);
        let before = fs::read(&path).ok();
        let hook = Arc::new(support::SweepHook::new(if fail == 0 {
            usize::MAX
        } else {
            fail
        }));
        let store = small_with_faults(&path, hook.clone());
        let result = store.save_if_unchanged(new, tip(old));
        if fail == 0 {
            result.unwrap();
            boundaries = hook.count();
            assert!(boundaries > 0);
        } else {
            assert!(
                matches!(result, Err(IndexStoreError::Io(_))),
                "{fail}: {result:?}"
            );
        }
        let restored = small(&path).load().unwrap();
        if same(&restored, old) {
            if legacy {
                // An interrupted migration leaves the legacy file byte-identical.
                assert_eq!(fs::read(&path).ok(), before, "legacy changed at {fail}");
            }
        } else {
            assert!(same(&restored, new), "neither old nor new at {fail}");
        }
        small(&path).save_if_unchanged(new, tip(&restored)).unwrap();
        assert!(same(&small(&path).load().unwrap(), new));
        assert_eq!(on_disk(&path), referenced(&path), "orphans left at {fail}");
        fail += 1;
        if fail > boundaries {
            return boundaries;
        }
    }
}

#[test]
fn every_boundary_of_an_extension_leaves_the_old_or_the_new_history() {
    let blocks = chain(23, u64::MAX, 0);
    let old = index_of(&blocks[..10]);
    let new = index_of(&blocks);
    let count = sweep(
        |path| {
            small(path).save_if_unchanged(&old, None).unwrap();
            // A leftover of an earlier interrupted save, removed before anything is written.
            fs::write(segments_dir(path).join("00000009-g7.seg"), b"orphan").unwrap();
        },
        &old,
        &new,
        false,
    );
    // Removed + synced orphan, one append, three new segments, segment-directory sync,
    // manifest synced, renamed and its directory synced.
    assert_eq!(count, 2 + 4 + 1 + 3);
}

#[test]
fn every_boundary_of_a_reorg_in_the_active_segment_leaves_the_old_or_the_new_history() {
    let old = index_of(&chain(22, u64::MAX, 0));
    let new = index_of(&chain(24, BASE + 20, 1));
    sweep(
        |path| small(path).save_if_unchanged(&old, None).unwrap(),
        &old,
        &new,
        false,
    );
}

#[derive(Serialize, Deserialize)]
struct OldStoredIndex {
    version: u32,
    domain: IndexDomain,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checkpoint: Option<TreeCheckpoint>,
    blocks: Vec<PoolBlock>,
}

/// The decision a version-2/3 binary makes on a cache file: `Ok` only if it would use it.
fn old_binary_accepts(data: &[u8], domain: IndexDomain) -> bool {
    let Ok(stored) = serde_json::from_slice::<OldStoredIndex>(data) else {
        return false;
    };
    stored.domain == domain && matches!(stored.version, 1..=3)
}

fn legacy_bytes(version: u32, checkpoint: Option<TreeCheckpoint>, index: &PoolIndex) -> Vec<u8> {
    serde_json::to_vec(&OldStoredIndex {
        version,
        domain: domain(),
        checkpoint,
        blocks: index.tail_blocks().to_vec(),
    })
    .unwrap()
}

#[test]
fn a_version_two_cache_migrates_without_rpc_and_an_interrupted_migration_keeps_it() {
    let blocks = chain(15, u64::MAX, 0);
    let old = index_of(&blocks[..9]);
    let new = index_of(&blocks);
    let legacy = legacy_bytes(2, None, &old);
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("index.json");
    fs::write(&path, &legacy).unwrap();
    // Loading needs no network: the legacy history is replayed locally.
    let loaded = small(&path).load().unwrap();
    assert!(same(&loaded, &old));
    assert_eq!(fs::read(&path).unwrap(), legacy);
    // Saving the same history migrates it unchanged.
    small(&path).save_if_unchanged(&loaded, tip(&old)).unwrap();
    assert_eq!(manifest(&path)["version"], 4);
    assert!(same(&small(&path).load().unwrap(), &old));
    assert!(!old_binary_accepts(&fs::read(&path).unwrap(), domain()));
    sweep(|path| fs::write(path, &legacy).unwrap(), &old, &new, true);
}

fn empty_checkpoint(block_number: u64) -> TreeCheckpoint {
    let mut zero = [[0u8; 32]; NOTE_TREE_DEPTH];
    let mut value = [0u8; 32];
    for slot in &mut zero {
        *slot = value;
        value = note_tree_parent(&value, &value).unwrap();
    }
    TreeCheckpoint {
        block_number,
        block_hash: block_hash(block_number, 0),
        leaf_count: 0,
        root: PoolIndex::new(1).unwrap().root(),
        filled_subtrees: zero,
    }
}

#[test]
fn a_version_three_checkpoint_cache_migrates_and_keeps_its_checkpoint() {
    let checkpoint = empty_checkpoint(BASE + 3);
    // A history that starts after the checkpoint, with insertions from an empty tree.
    let mut tree = Tree::new();
    let mut index = PoolIndex::from_checkpoint(checkpoint).unwrap();
    let mut parent = checkpoint.block_hash;
    for number in BASE + 4..BASE + 20 {
        let hash = block_hash(number, 0);
        let mut events = Vec::new();
        if number % 3 == 0 {
            let commitment = field(1, 0, number, 0);
            events.push(PoolEvent::Inserted {
                index: tree.next,
                commitment,
                root: tree.insert(commitment),
                tx_hash: [7; 32],
                log_index: 0,
            });
        }
        index
            .apply_block(PoolBlock {
                number,
                hash,
                parent_hash: parent,
                events,
            })
            .unwrap();
        parent = hash;
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("index.json");
    let open = || {
        IndexStore::with_checkpoint(&path, domain(), Some(checkpoint))
            .unwrap()
            .with_layout(SEGMENT, MARGIN)
            .unwrap()
    };
    let legacy = legacy_bytes(3, Some(checkpoint), &index);
    fs::write(&path, &legacy).unwrap();
    let loaded = open().load().unwrap();
    assert!(same(&loaded, &index));
    open().save_if_unchanged(&loaded, tip(&index)).unwrap();
    let written = manifest(&path);
    assert_eq!(written["version"], 4);
    assert_eq!(written["segments"][0]["first"], BASE + 4);
    assert!(same(&open().load().unwrap(), &index));
    // A full-history handle, or another checkpoint, refuses the migrated cache.
    assert!(is_cache_error(small(&path).load()));
    let other = IndexStore::with_checkpoint(&path, domain(), Some(empty_checkpoint(BASE + 2)))
        .unwrap()
        .with_layout(SEGMENT, MARGIN)
        .unwrap();
    assert!(is_cache_error(other.load()));
    assert!(!old_binary_accepts(&fs::read(&path).unwrap(), domain()));
}

#[test]
fn an_old_binary_refuses_a_segmented_cache() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("index.json");
    small(&path)
        .save_if_unchanged(&index_of(&chain(9, u64::MAX, 0)), None)
        .unwrap();
    let data = fs::read(&path).unwrap();
    assert!(serde_json::from_slice::<OldStoredIndex>(&data).is_err());
    assert!(!old_binary_accepts(&data, domain()));
    // An empty segmented cache is refused too: an old binary must never read it as empty.
    let empty = root.path().join("empty.json");
    small(&empty)
        .save_if_unchanged(&PoolIndex::new(BASE).unwrap(), None)
        .unwrap();
    assert_eq!(manifest(&empty)["version"], 4);
    assert!(!old_binary_accepts(&fs::read(&empty).unwrap(), domain()));
}

#[test]
fn an_interrupted_append_keeps_the_old_history_and_the_next_save_truncates_it() {
    let blocks = chain(30, u64::MAX, 0);
    let old = index_of(&blocks[..10]);
    let new = index_of(&blocks);
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("index.json");
    small(&path).save_if_unchanged(&old, None).unwrap();
    let active = segment_file(&path, 2);
    let committed = manifest(&path)["segments"][2]["byte_len"].as_u64().unwrap();
    // Fail right after the append to the active segment reached disk (the first blob).
    let hook = Arc::new(support::SweepHook::new(1));
    assert!(matches!(
        small_with_faults(&path, hook).save_if_unchanged(&new, tip(&old)),
        Err(IndexStoreError::Io(_))
    ));
    assert!(fs::metadata(&active).unwrap().len() > committed);
    assert!(same(&small(&path).load().unwrap(), &old));
    // A torn half-record past the committed length is also ignored.
    let mut torn = fs::read(&active).unwrap();
    torn.extend_from_slice(&[0xff; 7]);
    fs::write(&active, &torn).unwrap();
    assert!(same(&small(&path).load().unwrap(), &old));
    // Crash after the new segments were written but before the manifest: old history, orphans.
    let hook = Arc::new(support::SweepHook::new(4));
    assert!(small_with_faults(&path, hook)
        .save_if_unchanged(&new, tip(&old))
        .is_err());
    assert!(on_disk(&path).len() > referenced(&path).len());
    assert!(same(&small(&path).load().unwrap(), &old));
    small(&path).save_if_unchanged(&new, tip(&old)).unwrap();
    assert!(same(&small(&path).load().unwrap(), &new));
    assert_eq!(on_disk(&path), referenced(&path));
    let entry = &manifest(&path)["segments"][2];
    assert_eq!(
        fs::metadata(segments_dir(&path).join(entry["file"].as_str().unwrap()))
            .unwrap()
            .len(),
        entry["byte_len"].as_u64().unwrap()
    );
}

/// A cache of `blocks`; for 29 blocks, segments 0-5 are sealed and 6 and 7 are not.
fn saved(root: &Path, name: &str, blocks: &[PoolBlock]) -> PathBuf {
    let path = root.join(name);
    small(&path)
        .save_if_unchanged(&index_of(blocks), None)
        .unwrap();
    path
}

fn flip(path: &Path, offset: usize) {
    let mut bytes = fs::read(path).unwrap();
    bytes[offset] ^= 0x01;
    fs::write(path, bytes).unwrap();
}

#[test]
fn corrupted_segments_and_manifests_are_cache_errors_never_empty_or_stale() {
    let root = tempfile::tempdir().unwrap();
    let blocks = chain(29, u64::MAX, 0);
    let fresh = || {
        let dir = tempfile::tempdir_in(root.path()).unwrap();
        let path = saved(dir.path(), "index.json", &blocks);
        (dir, path)
    };
    {
        let (_dir, path) = fresh();
        assert_eq!(manifest(&path)["sealed_frontier"], BASE + 23);
        assert_eq!(manifest(&path)["segments"][5]["sealed"], true);
        assert_eq!(manifest(&path)["segments"][6]["sealed"], false);
    }
    // A flipped byte in a sealed segment, and in the active one.
    for k in [1, 7] {
        let (_dir, path) = fresh();
        flip(&segment_file(&path, k), 3);
        assert!(is_cache_error(small(&path).load()), "segment {k}");
        assert!(matches!(
            small(&path).save_if_unchanged(&index_of(&blocks), tip(&index_of(&blocks))),
            Err(IndexStoreError::Cache)
        ));
    }
    // Truncation below the committed length.
    {
        let (_dir, path) = fresh();
        let file = segment_file(&path, 7);
        let bytes = fs::read(&file).unwrap();
        fs::write(&file, &bytes[..bytes.len() - 1]).unwrap();
        assert!(is_cache_error(small(&path).load()));
    }
    // A missing referenced segment.
    {
        let (_dir, path) = fresh();
        fs::remove_file(segment_file(&path, 3)).unwrap();
        assert!(is_cache_error(small(&path).load()));
    }
    // A valid segment of another branch under this cache's file name.
    {
        let (_dir, path) = fresh();
        let other_dir = tempfile::tempdir_in(root.path()).unwrap();
        let other = saved(other_dir.path(), "index.json", &chain(29, BASE + 2, 1));
        fs::copy(segment_file(&other, 2), segment_file(&path, 2)).unwrap();
        assert!(is_cache_error(small(&path).load()));
    }
    // A symbolic link to an identical copy of the segment.
    #[cfg(unix)]
    {
        let (dir, path) = fresh();
        let file = segment_file(&path, 4);
        let copy = dir.path().join("copy.seg");
        fs::copy(&file, &copy).unwrap();
        fs::remove_file(&file).unwrap();
        std::os::unix::fs::symlink(&copy, &file).unwrap();
        assert!(is_cache_error(small(&path).load()));
    }
    // Group-readable segment file or directory.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, path) = fresh();
        let file = segment_file(&path, 0);
        fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(is_cache_error(small(&path).load()));
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(small(&path).load().is_ok());
        fs::set_permissions(segments_dir(&path), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(is_cache_error(small(&path).load()));
        fs::set_permissions(segments_dir(&path), fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(is_cache_error(small(&path).load()));
    }
    // An edited manifest, without and with a recomputed manifest hash.
    {
        let (_dir, path) = fresh();
        let text = fs::read_to_string(&path).unwrap();
        let edited = text.replacen("\"end_leaf_count\":", "\"end_leaf_count\":1", 1);
        fs::write(&path, &edited).unwrap();
        assert!(is_cache_error(small(&path).load()));
        fs::write(&path, rehash_manifest(&edited)).unwrap();
        assert!(is_cache_error(small(&path).load()));
        // Re-hashing the untouched manifest reproduces it exactly.
        assert_eq!(rehash_manifest(&text), text);
    }
    {
        let (_dir, path) = fresh();
        let text = fs::read_to_string(&path).unwrap();
        let value = manifest(&path);
        let tip_hash = value["tip"]["hash"].as_str().unwrap();
        let other = hex::encode(block_hash(BASE + 28, 9));
        let edited = text.replace(tip_hash, &other);
        fs::write(&path, rehash_manifest(&edited)).unwrap();
        assert!(is_cache_error(small(&path).load()));
    }
    // A cache opened with another layout is refused, not reinterpreted.
    {
        let (_dir, path) = fresh();
        let other = IndexStore::new(&path, domain()).unwrap();
        assert!(is_cache_error(other.load()));
    }
}

#[test]
fn an_edited_consumed_nullifier_is_rejected_even_when_every_hash_is_recomputed() {
    let root = tempfile::tempdir().unwrap();
    let blocks = chain(29, u64::MAX, 0);
    let path = saved(root.path(), "index.json", &blocks);
    // Block 110 (segment 2) has a withdrawal; its nullifier is not bound by any Merkle root.
    let block = &blocks[10];
    assert!(matches!(
        block.events.last(),
        Some(PoolEvent::Consumed { .. })
    ));
    let file = segment_file(&path, 2);
    let bytes = fs::read(&file).unwrap();
    let mut edited = blocks.clone();
    if let Some(PoolEvent::Consumed { nullifier, .. }) = edited[10].events.last_mut() {
        nullifier[31] ^= 1;
    }
    let encode = |events: &[PoolEvent]| serde_json::to_vec(events).unwrap();
    let old_events = encode(&blocks[10].events);
    let new_events = encode(&edited[10].events);
    assert_eq!(old_events.len(), new_events.len());
    let at = bytes
        .windows(old_events.len())
        .position(|window| window == old_events.as_slice())
        .expect("record");
    let mut tampered = bytes.clone();
    tampered[at..at + new_events.len()].copy_from_slice(&new_events);
    // The edited history is itself valid: a legacy cache would have accepted it.
    let edited_index = index_of(&edited);
    assert_eq!(edited_index.root(), index_of(&blocks).root());
    fs::write(&file, &tampered).unwrap();
    assert!(is_cache_error(small(&path).load()));
    // Re-hash the segment and the manifest: the per-segment history digest still disagrees.
    let text = fs::read_to_string(&path).unwrap();
    let old_sha = hex::encode(Sha256::digest(&bytes));
    let new_sha = hex::encode(Sha256::digest(&tampered));
    fs::write(&path, rehash_manifest(&text.replace(&old_sha, &new_sha))).unwrap();
    assert!(is_cache_error(small(&path).load()));
}

#[test]
fn more_than_two_hundred_segments_reload_identically() {
    let blocks = chain_with(900, u64::MAX, 0, 50);
    let full = index_of(&blocks);
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("index.json");
    let store = small(&path);
    let mut index = store.load().unwrap();
    let mut saves = 0;
    let mut next = 0usize;
    while next < blocks.len() {
        let prior = tip(&index);
        let step = 1 + (saves * 7) % 13;
        for block in blocks.iter().skip(next).take(step) {
            index.apply_block(block.clone()).unwrap();
        }
        next += step;
        store.save_if_unchanged(&index, prior).unwrap();
        saves += 1;
        if saves % 25 == 0 {
            index = small(&path).load().unwrap();
        }
    }
    let reloaded = small(&path).load().unwrap();
    assert!(same(&reloaded, &full));
    let written = manifest(&path);
    assert_eq!(written["segments"].as_array().unwrap().len(), 225);
    assert_eq!(on_disk(&path), referenced(&path));
    // The same history saved at once reloads to the same index and the same end states.
    let other = root.path().join("once.json");
    small(&other).save_if_unchanged(&full, None).unwrap();
    assert!(same(&small(&other).load().unwrap(), &full));
    let strip = |mut value: Value| {
        value["generation"] = Value::Null;
        value["manifest_sha256"] = Value::Null;
        for entry in value["segments"].as_array_mut().unwrap() {
            entry["file"] = Value::Null;
        }
        value
    };
    assert_eq!(strip(written), strip(manifest(&other)));
}

#[test]
fn stale_on_another_tip_and_on_the_same_tip_with_different_events() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("index.json");
    let blocks = chain(14, u64::MAX, 0);
    let history = index_of(&blocks[..12]);
    small(&path).save_if_unchanged(&history, None).unwrap();
    // Writer A loads the cache.
    let mut ours = small(&path).load().unwrap();
    let loaded_tip = tip(&ours);
    // A different tip than the one A loaded.
    let longer = index_of(&blocks[..13]);
    small(&path).save_if_unchanged(&longer, loaded_tip).unwrap();
    ours.apply_block(blocks[12].clone()).unwrap();
    assert!(matches!(
        small(&path).save_if_unchanged(&ours, loaded_tip),
        Err(IndexStoreError::Stale)
    ));
    // Writer B rewinds and re-extends with the same block hashes but an edited event.
    let mut edited = blocks.clone();
    if let Some(PoolEvent::Consumed { nullifier, .. }) = edited[10].events.last_mut() {
        nullifier[31] ^= 1;
    } else {
        panic!("block 110 has a withdrawal");
    }
    let prefix = index_of(&blocks[..10]);
    small(&path)
        .save_if_unchanged(&prefix, tip(&longer))
        .unwrap();
    let replaced = index_of(&edited[..13]);
    assert_eq!(tip(&replaced), tip(&longer));
    small(&path)
        .save_if_unchanged(&replaced, tip(&prefix))
        .unwrap();
    // A's view and the cache share the tip hash but not the history: still stale.
    let mut ours = longer.clone();
    ours.apply_block(blocks[13].clone()).unwrap();
    assert!(matches!(
        small(&path).save_if_unchanged(&ours, tip(&longer)),
        Err(IndexStoreError::Stale)
    ));
    assert!(same(&small(&path).load().unwrap(), &replaced));
}

#[test]
fn a_reorg_inside_the_unsealed_tail_is_saved_and_one_below_the_seal_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("index.json");
    let original = index_of(&chain(29, u64::MAX, 0));
    small(&path).save_if_unchanged(&original, None).unwrap();
    assert_eq!(manifest(&path)["sealed_frontier"], BASE + 23);
    // Fork at 125, inside the unsealed segment 6.
    let forked = index_of(&chain(31, BASE + 25, 1));
    small(&path)
        .save_if_unchanged(&forked, tip(&original))
        .unwrap();
    assert!(same(&small(&path).load().unwrap(), &forked));
    assert_eq!(on_disk(&path), referenced(&path));
    // The new tip (130) sealed segment 6. A fork at 122, inside sealed segment 5, is refused
    // and nothing on disk changes.
    assert_eq!(manifest(&path)["sealed_frontier"], BASE + 27);
    let manifest_before = fs::read(&path).unwrap();
    let files_before = on_disk(&path);
    let deep = index_of(&chain(40, BASE + 22, 2));
    assert!(matches!(
        small(&path).save_if_unchanged(&deep, tip(&forked)),
        Err(IndexStoreError::Index(IndexError::Checkpoint))
    ));
    // A shorter history that would drop sealed blocks is refused the same way.
    assert!(matches!(
        small(&path).save_if_unchanged(&index_of(&chain(20, u64::MAX, 0)), tip(&forked)),
        Err(IndexStoreError::Index(IndexError::Checkpoint))
    ));
    assert_eq!(fs::read(&path).unwrap(), manifest_before);
    assert_eq!(on_disk(&path), files_before);
    assert!(same(&small(&path).load().unwrap(), &forked));
}

/// One writer: repeatedly load, extend by one or two canonical blocks, and save.
fn race_writer(path: &Path, blocks: &[PoolBlock], cycles: usize, seed: usize) -> (usize, usize) {
    let (mut saved, mut stale) = (0, 0);
    for cycle in 0..cycles {
        let store = small(path);
        let mut index = store.load().expect("every load verifies");
        let prior = tip(&index);
        let next = index
            .tip()
            .map_or(0, |tip| (tip.number - BASE + 1) as usize);
        let step = 1 + (cycle + seed) % 2;
        for block in blocks.iter().skip(next).take(step) {
            index.apply_block(block.clone()).unwrap();
        }
        match store.save_if_unchanged(&index, prior) {
            Ok(()) => saved += 1,
            Err(IndexStoreError::Stale) => stale += 1,
            Err(error) => panic!("only Ok or Stale may race: {error:?}"),
        }
    }
    (saved, stale)
}

const RACE_CYCLES: usize = 200;

fn race_chain() -> Vec<PoolBlock> {
    chain_with(2 * 2 * RACE_CYCLES as u64 + 4, u64::MAX, 0, 50)
}

#[test]
fn two_writers_in_threads_only_ever_save_or_go_stale() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("index.json");
    let blocks = race_chain();
    let writers: Vec<_> = (0..2)
        .map(|seed| {
            let path = path.clone();
            let blocks = blocks.clone();
            std::thread::spawn(move || race_writer(&path, &blocks, RACE_CYCLES, seed))
        })
        .collect();
    let results: Vec<_> = writers.into_iter().map(|w| w.join().unwrap()).collect();
    let saved: usize = results.iter().map(|(saved, _)| saved).sum();
    let final_index = small(&path).load().unwrap();
    let through = final_index.tip().unwrap().number;
    assert!(saved > 0, "{results:?}");
    assert!(same(
        &final_index,
        &index_of(&blocks[..(through - BASE + 1) as usize])
    ));
    // The race crossed many seal boundaries.
    assert!(manifest(&path)["sealed_frontier"].as_u64().unwrap() > BASE + 50);
    assert_eq!(on_disk(&path), referenced(&path));
}

/// Child half of the cross-process race; runs only when spawned by the test below.
#[test]
#[ignore = "spawned by two_writers_in_separate_processes_only_ever_save_or_go_stale"]
fn race_child_writer() {
    let Ok(path) = std::env::var("EREBUS_TEST_RACE_PATH") else {
        return;
    };
    let (saved, stale) = race_writer(Path::new(&path), &race_chain(), RACE_CYCLES, 1);
    println!("child saved {saved} stale {stale}");
}

#[test]
fn two_writers_in_separate_processes_only_ever_save_or_go_stale() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("index.json");
    let blocks = race_chain();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["race_child_writer", "--exact", "--ignored", "--nocapture"])
        .env("EREBUS_TEST_RACE_PATH", &path)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let (saved, _) = race_writer(&path, &blocks, RACE_CYCLES, 0);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "child writer failed");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("child saved"), "{text}");
    let final_index = small(&path).load().unwrap();
    let through = final_index.tip().unwrap().number;
    assert!(saved > 0);
    assert!(same(
        &final_index,
        &index_of(&blocks[..(through - BASE + 1) as usize])
    ));
    assert_eq!(on_disk(&path), referenced(&path));
}

/// Mostly empty blocks, as on a quiet pool: the shape that filled the legacy 128 MiB cap.
#[test]
#[ignore = "large synthetic history; run with --release --ignored --nocapture"]
fn a_history_beyond_the_legacy_cap_saves_and_reloads() {
    let blocks = 600_000u64;
    let history = chain_with(blocks, u64::MAX, 0, 997);
    let index = index_of(&history);
    let legacy = legacy_bytes(2, None, &index).len() as u64;
    assert!(legacy > 128 * 1024 * 1024, "legacy {legacy} bytes");
    drop(history);
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("index.json");
    let store = IndexStore::new(&path, domain()).unwrap();
    let started = std::time::Instant::now();
    store.save_if_unchanged(&index, None).unwrap();
    let saved = started.elapsed();
    let segment_bytes: u64 = fs::read_dir(segments_dir(&path))
        .unwrap()
        .map(|entry| entry.unwrap().metadata().unwrap().len())
        .sum();
    let manifest_bytes = fs::metadata(&path).unwrap().len();
    let started = std::time::Instant::now();
    let loaded = IndexStore::new(&path, domain()).unwrap().load().unwrap();
    let load = started.elapsed();
    assert!(same(&loaded, &index));
    // A second load in this process replays only the unsealed segment.
    let started = std::time::Instant::now();
    let again = IndexStore::new(&path, domain()).unwrap().load().unwrap();
    let cached = started.elapsed();
    assert!(same(&again, &index));
    println!(
        "second load {cached:?}, tail {} blocks",
        again.tail_blocks().len()
    );
    println!(
        "blocks {blocks}: legacy {legacy} B ({:.1} B/block); segmented {} B ({:.1} B/block, \
         manifest {manifest_bytes} B, {} segments); save {saved:?}, load {load:?}",
        legacy as f64 / blocks as f64,
        segment_bytes + manifest_bytes,
        (segment_bytes + manifest_bytes) as f64 / blocks as f64,
        manifest(&path)["segments"].as_array().unwrap().len(),
    );
}

#[test]
fn a_loaded_index_keeps_only_its_unsealed_tail_and_event_blocks() {
    let root = tempfile::tempdir().unwrap();
    let blocks = chain(29, u64::MAX, 0);
    let full = index_of(&blocks);
    let path = saved(root.path(), "index.json", &blocks);
    let mut loaded = small(&path).load().unwrap();
    // Sealed through 123: only 124..=128 are held as blocks, the rest only if they have events.
    assert_eq!(loaded.sealed_through(), Some(BASE + 23));
    assert_eq!(loaded.tail_blocks().len(), 5);
    assert!(loaded.event_blocks().eq(full.event_blocks()));
    assert_eq!(loaded.leaves(), full.leaves());
    assert_eq!(loaded.history_digest(), full.history_digest());
    for leaf in 0..full.leaf_count() {
        let (ours, theirs) = (loaded.path(leaf).unwrap(), full.path(leaf).unwrap());
        assert_eq!(
            (ours.index, ours.siblings, ours.root),
            (theirs.index, theirs.siblings, theirs.root)
        );
    }
    // Every block, sealed empty ones included, is still served from the segment files.
    let (_, served) = small(&path).load_with_blocks(None, 64).unwrap();
    assert_eq!(served, blocks);
    let (_, page) = small(&path).load_with_blocks(Some(BASE + 20), 6).unwrap();
    assert_eq!(page, blocks[21..27].to_vec());
    // Rewinding is allowed only above the seal.
    let before = loaded.clone();
    assert!(matches!(
        loaded.rewind_from(BASE + 23),
        Err(IndexError::Checkpoint)
    ));
    assert!(same(&loaded, &before));
    loaded.rewind_from(BASE + 26).unwrap();
    assert!(same(&loaded, &index_of(&blocks[..26])));
    for block in &blocks[26..] {
        loaded.apply_block(block.clone()).unwrap();
    }
    assert!(same(&loaded, &full));
    // A rewound-and-extended sealed index saves like any other.
    small(&path).save_if_unchanged(&loaded, tip(&full)).unwrap();
}

#[test]
fn sealed_segments_are_verified_once_per_process_and_path() {
    let root = tempfile::tempdir().unwrap();
    let blocks = chain(29, u64::MAX, 0);
    let path = saved(root.path(), "index.json", &blocks);
    small(&path).load().unwrap();
    // The sealed prefix is now trusted for this path: a later flip in a sealed file is not reread.
    flip(&segment_file(&path, 1), 3);
    assert!(small(&path).load().is_ok());
    // Another path spelling (as after a restart) verifies everything and finds it.
    fs::create_dir(root.path().join("sub")).unwrap();
    let other = root.path().join("sub").join("..").join("index.json");
    assert!(is_cache_error(small(&other).load()));
    // The active segment is always reread.
    flip(&segment_file(&path, 7), 3);
    assert!(is_cache_error(small(&path).load()));
}

#[cfg(unix)]
#[test]
fn every_new_cache_file_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let path = saved(root.path(), "index.json", &chain(12, u64::MAX, 0));
    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(&path.with_extension("lock")), 0o600);
    assert_eq!(mode(&segments_dir(&path)), 0o700);
    for file in referenced(&path) {
        assert_eq!(mode(&segments_dir(&path).join(file)), 0o600);
    }
    // A load alone creates the lock file, also owner-only.
    let fresh = root.path().join("fresh.json");
    small(&fresh).load().unwrap();
    assert_eq!(mode(&fresh.with_extension("lock")), 0o600);
    assert!(!fresh.exists());
}
