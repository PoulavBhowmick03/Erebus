//! Version-3 segmented cache: bounded files, tail-only rewrites, explicit legacy migration,
//! fail-closed corruption and interruption safety.

use std::fs;

use erebus_shielded_prover::{
    index_store::{IndexDomain, IndexStore, IndexStoreError},
    indexer::{PoolBlock, PoolIndex},
};
use serde_json::Value;

mod support;

fn block_hash(number: u64) -> [u8; 32] {
    let mut hash = [0x11; 32];
    hash[24..].copy_from_slice(&number.to_be_bytes());
    hash
}

fn domain() -> IndexDomain {
    IndexDomain {
        chain_id: 10143,
        pool: [9; 20],
        first_block: 100,
        first_hash: block_hash(100),
    }
}

fn block(number: u64) -> PoolBlock {
    PoolBlock {
        number,
        hash: block_hash(number),
        parent_hash: block_hash(number - 1),
        events: vec![],
    }
}

fn build(blocks: u64) -> PoolIndex {
    let mut index = PoolIndex::new(100).unwrap();
    for number in 100..100 + blocks {
        index.apply_block(block(number)).unwrap();
    }
    index
}

fn segment_files(path: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let dir = path.with_extension("segments");
    let mut files: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|entry| {
            (
                entry.file_name().to_string_lossy().into_owned(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect();
    files.sort();
    files
}

#[test]
fn segmented_cache_bounds_files_and_round_trips() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("index.json");
    let store = IndexStore::new(&path, domain()).unwrap();
    let index = build(100_000);
    store.save_if_unchanged(&index, None).unwrap();

    let manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(manifest["version"], 3);
    let segments = manifest["segments"].as_array().unwrap();
    assert!(
        segments.len() >= 2,
        "100k blocks must span several segments"
    );
    assert!(
        fs::metadata(&path).unwrap().len() < 64 * 1024,
        "the manifest must stay small"
    );
    for segment in segments {
        assert!(segment["bytes"].as_u64().unwrap() <= 32 * 1024 * 1024);
    }
    let files = segment_files(&path);
    assert_eq!(files.len(), segments.len());
    assert_eq!(store.load().unwrap().blocks(), index.blocks());
    let reopened = IndexStore::new(&path, domain()).unwrap();
    assert_eq!(reopened.load().unwrap().blocks(), index.blocks());
    assert!(matches!(
        store.save_if_unchanged(&index, None),
        Err(IndexStoreError::Stale)
    ));
}

#[test]
fn bounded_append_reuses_completed_segments() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("index.json");
    let store = IndexStore::new(&path, domain()).unwrap();
    let mut index = build(50_000);
    store.save_if_unchanged(&index, None).unwrap();
    let before = segment_files(&path);
    assert!(!before.is_empty());

    let loaded_tip = index.tip().map(|block| block.hash);
    for number in 50_000..50_010 {
        index.apply_block(block(100 + number)).unwrap();
    }
    store.save_if_unchanged(&index, loaded_tip).unwrap();
    let after = segment_files(&path);

    let completed = after
        .iter()
        .filter(|(name, bytes)| {
            before
                .iter()
                .any(|(old, old_bytes)| old == name && old_bytes == bytes)
        })
        .count();
    assert!(
        completed >= 1,
        "completed segments must be reused byte for byte: before={:?} after={:?}",
        before.iter().map(|(name, _)| name).collect::<Vec<_>>(),
        after.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    assert_eq!(store.load().unwrap().tip().unwrap().number, 100 + 50_009);
}

#[test]
fn legacy_v2_cache_is_read_and_migrated_on_save() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("index.json");
    let domain = domain();
    let mut index = PoolIndex::new(100).unwrap();
    for number in 100..110 {
        index.apply_block(block(number)).unwrap();
    }
    let legacy = serde_json::json!({
        "version": 2,
        "domain": domain,
        "blocks": index.blocks(),
    });
    fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();

    let store = IndexStore::new(&path, domain).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded.blocks(), index.blocks());
    store
        .save_if_unchanged(&loaded, loaded.tip().map(|block| block.hash))
        .unwrap();
    let manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(manifest["version"], 3);
    assert!(manifest.get("blocks").is_none());
    assert_eq!(store.load().unwrap().blocks(), index.blocks());
}

#[test]
fn missing_modified_or_truncated_segments_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("index.json");
    let store = IndexStore::new(&path, domain()).unwrap();
    let index = build(100_000);
    store.save_if_unchanged(&index, None).unwrap();
    let files = segment_files(&path);
    assert!(files.len() >= 2);

    let victim = path.with_extension("segments").join(&files[0].0);
    let original = fs::read(&victim).unwrap();

    fs::remove_file(&victim).unwrap();
    assert!(matches!(store.load(), Err(IndexStoreError::Cache)));

    let mut modified = original.clone();
    let last = modified.len() - 2;
    modified[last] ^= 1;
    fs::write(&victim, &modified).unwrap();
    assert!(matches!(store.load(), Err(IndexStoreError::Cache)));

    fs::write(&victim, &original[..original.len() / 2]).unwrap();
    assert!(matches!(store.load(), Err(IndexStoreError::Cache)));

    fs::write(&victim, &original).unwrap();
    assert_eq!(store.load().unwrap().blocks(), index.blocks());

    // A manifest that references a missing segment is never treated as absence.
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["segments"][0]["file"] =
        serde_json::json!("seg-0000000000000064-0000000000000065.json");
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(matches!(store.load(), Err(IndexStoreError::Cache)));
}

#[test]
fn unreferenced_segment_garbage_is_ignored_then_cleaned() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("index.json");
    let store = IndexStore::new(&path, domain()).unwrap();
    let index = build(10_000);
    store.save_if_unchanged(&index, None).unwrap();
    let stray = path.with_extension("segments").join("seg-deadbeef.json");
    fs::write(&stray, b"orphan from an interrupted save").unwrap();
    assert_eq!(store.load().unwrap().blocks(), index.blocks());
    store
        .save_if_unchanged(&index, index.tip().map(|block| block.hash))
        .unwrap();
    assert!(!stray.exists(), "unreferenced segments are cleaned on save");
}

#[test]
fn segmented_write_interruption_at_every_boundary_restores_one_complete_prefix() {
    let mut boundaries = 0;
    let mut fail = 0;
    loop {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("index.json");
        let domain = domain();
        let hook = std::sync::Arc::new(support::SweepHook::new(if fail == 0 {
            usize::MAX
        } else {
            fail
        }));
        let store = IndexStore::with_faults(&path, domain, hook.clone()).unwrap();
        let index = build(40_000);
        let result = store.save_if_unchanged(&index, None);
        if fail == 0 {
            result.unwrap();
            boundaries = hook.count();
            assert!(
                boundaries >= 6,
                "segments plus manifest must fence every write"
            );
        } else {
            assert!(matches!(result, Err(IndexStoreError::Io(_))));
        }
        drop(store);
        let restored = IndexStore::new(&path, domain).unwrap();
        let old = restored.load().unwrap();
        assert!(old.tip().is_none() || old.tip() == index.tip());
        restored
            .save_if_unchanged(&index, old.tip().map(|block| block.hash))
            .unwrap();
        assert_eq!(restored.load().unwrap().blocks(), index.blocks());
        fail += 1;
        if fail > boundaries {
            break;
        }
    }
}
