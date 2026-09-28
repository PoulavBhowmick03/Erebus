//! Restart, deployment binding, and stale-writer checks for the public index cache.

use std::fs;

use erebus_shielded_prover::{
    index_store::{IndexDomain, IndexStore, IndexStoreError},
    indexer::{PoolBlock, PoolEvent, PoolIndex},
};
use serde_json::Value;

fn field(value: &str) -> [u8; 32] {
    hex::decode(value)
        .expect("fixture field")
        .try_into()
        .expect("width")
}

#[test]
fn cache_rebuilds_after_restart_and_rejects_stale_or_wrong_deployment() {
    let vector: Value = serde_json::from_str(include_str!(
        "../../core/tests/fixtures/m5-note-vector.json"
    ))
    .expect("note vector");
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("public/index.json");
    let domain = IndexDomain {
        chain_id: 10143,
        pool: [9; 20],
        first_block: 100,
        first_hash: [1; 32],
    };
    let store = IndexStore::new(&path, domain).expect("cache");
    let mut index = store.load().expect("empty cache");
    assert!(index.tip().is_none());
    let commitment = field(
        vector["input"]["commitmentHex"]
            .as_str()
            .expect("commitment"),
    );
    let root = field(vector["input"]["rootHex"].as_str().expect("root"));
    index
        .apply_block(PoolBlock {
            number: 100,
            hash: domain.first_hash,
            parent_hash: [2; 32],
            events: vec![PoolEvent::Inserted {
                index: 0,
                commitment,
                root,
                tx_hash: [3; 32],
                log_index: 0,
            }],
        })
        .expect("verified insertion");
    store.save_if_unchanged(&index, None).expect("persist");
    let restored = IndexStore::new(&path, domain)
        .expect("reopen")
        .load()
        .expect("rebuild");
    assert_eq!(restored.root(), root);
    assert_eq!(restored.path(0).expect("recovered path").root, root);
    assert!(matches!(
        store.save_if_unchanged(&index, None),
        Err(IndexStoreError::Stale)
    ));
    let other = IndexStore::new(
        &path,
        IndexDomain {
            chain_id: 8453,
            ..domain
        },
    )
    .expect("other handle");
    assert!(matches!(other.load(), Err(IndexStoreError::Cache)));
    let wrong_anchor = IndexStore::new(
        &path,
        IndexDomain {
            first_hash: [8; 32],
            ..domain
        },
    )
    .expect("wrong anchor handle");
    assert!(matches!(wrong_anchor.load(), Err(IndexStoreError::Cache)));

    let mut wrong_first = PoolIndex::new(100).expect("index");
    wrong_first
        .apply_block(PoolBlock {
            number: 100,
            hash: [7; 32],
            parent_hash: [2; 32],
            events: vec![PoolEvent::Inserted {
                index: 0,
                commitment,
                root,
                tx_hash: [3; 32],
                log_index: 0,
            }],
        })
        .expect("locally consistent wrong chain");
    assert!(matches!(
        store.save_if_unchanged(&wrong_first, Some(domain.first_hash)),
        Err(IndexStoreError::Domain)
    ));

    fs::write(&path, b"not a verified index").expect("corrupt cache");
    assert!(matches!(store.load(), Err(IndexStoreError::Cache)));
}
