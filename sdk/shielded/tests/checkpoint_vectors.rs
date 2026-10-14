//! Checkpoint frontier proofs against vectors generated from the real pool Solidity on Anvil.
//!
//! Regenerate with `node circuits/m5/scripts/generate-checkpoint-vectors.mjs` after `forge build`.

use erebus_shielded_prover::indexer::{
    IndexError, PoolBlock, PoolEvent, PoolIndex, TreeCheckpoint,
};
use serde::Deserialize;

const DEPTH: usize = 20;

#[derive(Deserialize)]
struct Vectors {
    poseidon_runtime_keccak256: String,
    tree_depth: usize,
    zero_hashes: Vec<String>,
    leaves: Vec<Leaf>,
    snapshots: Vec<Snapshot>,
    near_full: NearFull,
}

#[derive(Deserialize, Clone)]
struct Leaf {
    index: u32,
    commitment: String,
    root: String,
}

#[derive(Deserialize, Clone)]
struct Snapshot {
    leaf_count: u32,
    root: String,
    filled_subtrees: Vec<String>,
    root_known: bool,
}

#[derive(Deserialize)]
struct NearFull {
    base: Snapshot,
    insertions: Vec<Leaf>,
    full_tree_rejects_deposit: bool,
}

fn vectors() -> Vectors {
    serde_json::from_str(include_str!("fixtures/checkpoint-vectors.json")).expect("vectors")
}

fn word(value: &str) -> [u8; 32] {
    let raw = value.strip_prefix("0x").expect("0x");
    hex::decode(format!("{raw:0>64}"))
        .expect("hex")
        .try_into()
        .expect("32 bytes")
}

fn checkpoint(snapshot: &Snapshot) -> TreeCheckpoint {
    let mut filled = [[0u8; 32]; DEPTH];
    for (slot, value) in filled.iter_mut().zip(&snapshot.filled_subtrees) {
        *slot = word(value);
    }
    TreeCheckpoint {
        block_number: 100,
        block_hash: [7; 32],
        leaf_count: snapshot.leaf_count,
        root: word(&snapshot.root),
        filled_subtrees: filled,
    }
}

fn inserted(leaf: &Leaf, log_index: u32) -> PoolEvent {
    PoolEvent::Inserted {
        index: leaf.index,
        commitment: word(&leaf.commitment),
        root: word(&leaf.root),
        tx_hash: [9; 32],
        log_index,
    }
}

fn block(number: u64, parent: [u8; 32], leaves: &[Leaf]) -> PoolBlock {
    let mut hash = [0u8; 32];
    hash[..8].copy_from_slice(&number.to_be_bytes());
    hash[31] = 0xbb;
    PoolBlock {
        number,
        hash,
        parent_hash: parent,
        events: leaves
            .iter()
            .enumerate()
            .map(|(i, leaf)| inserted(leaf, i as u32))
            .collect(),
    }
}

/// The used frontier levels for `n` leaves: the lowest zero bit of `n - 1` and every one bit above it.
fn used_levels(n: u32) -> Vec<usize> {
    if n == 0 {
        return (0..DEPTH).collect();
    }
    let last = n - 1;
    let lowest = last.trailing_ones() as usize;
    (lowest..DEPTH).collect()
}

#[test]
fn vectors_come_from_the_pinned_production_poseidon() {
    let vectors = vectors();
    assert_eq!(vectors.tree_depth, DEPTH);
    assert_eq!(
        vectors.poseidon_runtime_keccak256,
        "354b26999e434f8256d168395315e1f51fc15bf49bf2e6bb58fc549bd597e7da"
    );
    // The empty-tree frontier is the contract's zero hashes and verifies against the empty root.
    let empty = &vectors.snapshots[0];
    assert_eq!(empty.leaf_count, 0);
    assert_eq!(empty.filled_subtrees, vectors.zero_hashes);
    checkpoint(empty).verify().expect("empty tree");
    assert_eq!(PoolIndex::new(1).unwrap().root(), word(&empty.root));
}

#[test]
fn every_contract_frontier_recomputes_its_contract_root() {
    let vectors = vectors();
    assert!(vectors.snapshots.len() >= 30);
    for snapshot in &vectors.snapshots {
        assert!(snapshot.root_known, "contract knows its current root");
        checkpoint(snapshot)
            .verify()
            .unwrap_or_else(|_| panic!("frontier for {} leaves", snapshot.leaf_count));
        // Independent cross-check: a full replay of the same leaves reaches the same root.
        let mut full = PoolIndex::new(1).unwrap();
        full.apply_block(block(
            1,
            [1; 32],
            &vectors.leaves[..snapshot.leaf_count as usize],
        ))
        .unwrap();
        assert_eq!(
            full.root(),
            word(&snapshot.root),
            "{} leaves",
            snapshot.leaf_count
        );
    }
}

#[test]
fn appends_after_a_checkpoint_match_every_contract_root_and_full_history_paths() {
    let vectors = vectors();
    let total = vectors.leaves.len();
    for snapshot in &vectors.snapshots {
        let n = snapshot.leaf_count as usize;
        if n == total {
            continue;
        }
        let later = &vectors.leaves[n..total.min(n + 70)];
        let base = checkpoint(snapshot);
        let mut index = PoolIndex::from_checkpoint(base).unwrap();
        assert_eq!(index.first_block(), 101);
        // apply_block recomputes each insertion and requires the contract's emitted root.
        index
            .apply_block(block(101, base.block_hash, later))
            .unwrap();
        assert_eq!(index.root(), word(&later.last().unwrap().root));
        assert_eq!(index.leaf_count() as usize, n + later.len());

        let mut full = PoolIndex::new(1).unwrap();
        full.apply_block(block(1, [1; 32], &vectors.leaves[..n + later.len()]))
            .unwrap();
        for leaf in later {
            let path = index.path(leaf.index).unwrap();
            let reference = full.path(leaf.index).unwrap();
            assert_eq!(
                path.siblings, reference.siblings,
                "leaf {} after {n}",
                leaf.index
            );
            assert_eq!(path.root, reference.root);
        }
        if n > 0 {
            assert!(matches!(
                index.path(n as u32 - 1),
                Err(IndexError::Checkpoint)
            ));
        }
        assert!(matches!(
            index.path(index.leaf_count()),
            Err(IndexError::EventOrder)
        ));
    }
}

#[test]
fn altered_used_frontier_root_or_count_is_rejected() {
    let vectors = vectors();
    for snapshot in vectors.snapshots.iter().filter(|s| s.leaf_count > 0) {
        let base = checkpoint(snapshot);
        for level in used_levels(snapshot.leaf_count) {
            let mut altered = base;
            altered.filled_subtrees[level][31] ^= 1;
            assert!(
                altered.verify().is_err(),
                "level {level} of {}",
                snapshot.leaf_count
            );
            assert!(PoolIndex::from_checkpoint(altered).is_err());
        }
        let mut altered = base;
        altered.root[31] ^= 1;
        assert!(altered.verify().is_err());
        // One fewer leaf would drop a non-zero commitment from the root: always rejected.
        let mut fewer = base;
        fewer.leaf_count -= 1;
        assert!(fewer.verify().is_err(), "count {} - 1", snapshot.leaf_count);
        // One more leaf can be root-indistinguishable: a zero leaf at index n hashes like empty space.
        // The contract never inserts zero, so the count is bound by the paired `nextLeafIndex` read and
        // the first later insertion, which must then be refused rather than misplaced.
        let mut more = base;
        more.leaf_count += 1;
        let n = snapshot.leaf_count as usize;
        if more.verify().is_ok() && n < vectors.leaves.len() {
            let mut index = PoolIndex::from_checkpoint(more).unwrap();
            assert!(matches!(
                index.apply_block(block(101, more.block_hash, &vectors.leaves[n..n + 1])),
                Err(IndexError::EventOrder)
            ));
        }
        let mut altered = base;
        altered.block_hash = [0; 32];
        assert!(altered.verify().is_err());
    }
}

#[test]
fn unused_frontier_entries_are_unverifiable_but_never_read() {
    // Levels below the lowest zero bit of n - 1 are overwritten by the next insertion before use.
    let vectors = vectors();
    for snapshot in vectors.snapshots.iter().filter(|s| s.leaf_count > 1) {
        let n = snapshot.leaf_count as usize;
        let unused: Vec<usize> = (0..DEPTH)
            .filter(|l| !used_levels(n as u32).contains(l))
            .collect();
        if unused.is_empty() || n == vectors.leaves.len() {
            continue;
        }
        let mut altered = checkpoint(snapshot);
        for level in &unused {
            altered.filled_subtrees[*level] = [0x42; 32];
        }
        altered
            .verify()
            .expect("unused levels do not affect the root");
        let later = &vectors.leaves[n..vectors.leaves.len().min(n + 9)];
        let mut index = PoolIndex::from_checkpoint(altered).unwrap();
        index
            .apply_block(block(101, altered.block_hash, later))
            .unwrap();
        for leaf in later {
            index.path(leaf.index).unwrap();
        }
    }
}

#[test]
fn checkpointed_index_requires_the_checkpoint_block_as_parent() {
    let vectors = vectors();
    let snapshot = vectors
        .snapshots
        .iter()
        .find(|s| s.leaf_count == 64)
        .unwrap();
    let base = checkpoint(snapshot);
    let later = &vectors.leaves[64..66];
    let mut index = PoolIndex::from_checkpoint(base).unwrap();
    assert!(matches!(
        index.apply_block(block(101, [3; 32], later)),
        Err(IndexError::BlockLink)
    ));
    assert!(matches!(
        index.apply_block(block(102, base.block_hash, later)),
        Err(IndexError::BlockLink)
    ));
    // A gap, a duplicate or an out-of-order insertion is refused exactly as with full history.
    assert!(index
        .apply_block(block(101, base.block_hash, &vectors.leaves[65..66]))
        .is_err());
    assert!(index
        .apply_block(block(101, base.block_hash, &vectors.leaves[63..64]))
        .is_err());
    index
        .apply_block(block(101, base.block_hash, later))
        .unwrap();
    // A rewind keeps the checkpoint and its coverage boundary.
    index.rewind_from(101).unwrap();
    assert_eq!(index.leaf_count(), 64);
    assert_eq!(index.root(), base.root);
    assert_eq!(index.checkpoint(), Some(&base));
}

#[test]
fn near_full_boundary_matches_the_contract_through_the_last_leaf() {
    let vectors = vectors();
    let near = &vectors.near_full;
    assert!(near.full_tree_rejects_deposit);
    assert_eq!(near.base.leaf_count, (1 << DEPTH) - 3);
    let base = checkpoint(&near.base);
    base.verify().unwrap();
    let mut index = PoolIndex::from_checkpoint(base).unwrap();
    index
        .apply_block(block(101, base.block_hash, &near.insertions))
        .unwrap();
    assert_eq!(index.leaf_count(), 1 << DEPTH);
    assert_eq!(index.root(), word(&near.insertions.last().unwrap().root));
    for leaf in &near.insertions {
        index.path(leaf.index).unwrap();
    }
    // Nothing fits after the last valid index, matching the contract's "tree full".
    let overflow = Leaf {
        index: 1 << DEPTH,
        commitment: "0x1234".into(),
        root: near.insertions[2].root.clone(),
    };
    let tip = index.tip().unwrap().hash;
    assert!(matches!(
        index.apply_block(block(102, tip, &[overflow])),
        Err(IndexError::EventOrder)
    ));
    // A full tree is not an acceptable checkpoint.
    let mut full = base;
    full.leaf_count = 1 << DEPTH;
    assert!(full.verify().is_err());
}
