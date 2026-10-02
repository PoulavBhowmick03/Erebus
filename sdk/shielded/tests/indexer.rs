//! Public Merkle-event validation and wallet replay without an RPC dependency.

use erebus_core::shielded::{note_root_from_path, note_tree_parent, NOTE_TREE_DEPTH};
use erebus_shielded_prover::{
    indexer::{IndexError, PoolBlock, PoolEvent, PoolIndex},
    wallet::{OwnedNote, WalletSnapshot},
};
use serde_json::Value;

fn bytes<const N: usize>(hex_value: &str) -> [u8; N] {
    hex::decode(hex_value)
        .expect("fixture hex")
        .try_into()
        .expect("width")
}

fn fixture() -> (OwnedNote, [u8; 32]) {
    let vector: Value = serde_json::from_str(include_str!(
        "../../core/tests/fixtures/m5-note-vector.json"
    ))
    .expect("pinned note vector");
    let input = &vector["input"];
    let value = |name: &str| input[name].as_str().expect("fixture field");
    let note = OwnedNote::new(
        bytes(vector["assetHex"].as_str().expect("asset")),
        value("amount").parse().expect("amount"),
        bytes(value("ownerHex")),
        bytes(value("spendSecretHex")),
        bytes(value("saltHex")),
    )
    .expect("note");
    let root = bytes(value("rootHex"));
    (note, root)
}

fn first_block(note: &OwnedNote, root: [u8; 32]) -> PoolBlock {
    PoolBlock {
        number: 100,
        hash: [1; 32],
        parent_hash: [2; 32],
        events: vec![PoolEvent::Inserted {
            index: 0,
            commitment: note.commitment(),
            root,
            tx_hash: [3; 32],
            log_index: 0,
        }],
    }
}

#[test]
fn verifies_roots_and_replays_a_wallet_across_a_reorg() {
    let (note, root) = fixture();
    let nullifier = note.nullifier();
    let mut wallet = WalletSnapshot::default();
    wallet.add(note.clone()).expect("expected note");
    let mut index = PoolIndex::new(100).expect("empty tree");
    index
        .apply_block(first_block(&note, root))
        .expect("verified insertion");
    assert_eq!(index.root(), root);
    assert_eq!(index.leaves(), &[note.commitment()]);
    let path = index.path(0).expect("verified path");
    assert_eq!(path.root, root);
    assert_eq!(
        note_root_from_path(&note.commitment(), path.index, &path.siblings).expect("path root"),
        root
    );
    index.replay_wallet(&mut wallet).expect("discovery");
    assert_eq!(wallet.notes()[0].inclusion().expect("included").index, 0);

    index
        .apply_block(PoolBlock {
            number: 101,
            hash: [4; 32],
            parent_hash: [1; 32],
            events: vec![PoolEvent::Transferred {
                deal_commitment: [7; 32],
                deal_nullifier: [8; 32],
                input_nullifier: nullifier,
                tx_hash: [5; 32],
                log_index: 2,
            }],
        })
        .expect("verified spend block");
    let transfer = index.deal_transfer(&[8; 32]).expect("deal transfer");
    assert_eq!(transfer.deal_commitment, [7; 32]);
    assert_eq!(transfer.input_nullifier, nullifier);
    assert_eq!(transfer.block_hash, [4; 32]);
    index.replay_wallet(&mut wallet).expect("spent state");
    let asset = bytes::<20>("5fbdb2315678afecb367f032d93f642f64180aa3");
    assert!(wallet.select(&asset, 70).is_none());

    index.rewind_from(101).expect("reorg");
    assert!(index.deal_transfer(&[8; 32]).is_none());
    index
        .apply_block(PoolBlock {
            number: 101,
            hash: [6; 32],
            parent_hash: [1; 32],
            events: vec![],
        })
        .expect("new canonical block");
    index.replay_wallet(&mut wallet).expect("reconciled wallet");
    assert!(wallet.select(&asset, 70).is_some());

    let mut alternate_index = PoolIndex::new(100).expect("another endpoint");
    alternate_index
        .apply_block(first_block(&note, root))
        .expect("same insertion");
    alternate_index
        .apply_block(PoolBlock {
            number: 101,
            hash: [6; 32],
            parent_hash: [1; 32],
            events: vec![],
        })
        .expect("same chain");
    alternate_index
        .replay_wallet(&mut wallet)
        .expect("endpoint switch");
    assert!(wallet.select(&asset, 70).is_some());
}

#[test]
fn bad_roots_indices_and_parent_hashes_do_not_advance_the_index() {
    let (note, root) = fixture();
    let mut index = PoolIndex::new(100).expect("index");
    let mut block = first_block(&note, root);
    if let PoolEvent::Inserted { root, .. } = &mut block.events[0] {
        root[31] ^= 1;
    }
    assert!(matches!(index.apply_block(block), Err(IndexError::Root)));
    assert!(index.tip().is_none());
    assert!(index.leaves().is_empty());

    let mut block = first_block(&note, root);
    if let PoolEvent::Inserted { index, .. } = &mut block.events[0] {
        *index = 1;
    }
    assert!(matches!(
        index.apply_block(block),
        Err(IndexError::EventOrder)
    ));
    index
        .apply_block(first_block(&note, root))
        .expect("good block");
    assert!(matches!(
        index.apply_block(PoolBlock {
            number: 101,
            hash: [7; 32],
            parent_hash: [8; 32],
            events: vec![],
        }),
        Err(IndexError::BlockLink)
    ));
    assert_eq!(index.tip().expect("old tip").number, 100);
}

#[test]
fn rejects_duplicate_public_identities_and_builds_a_second_leaf_path() {
    let (note, root) = fixture();
    let mut index = PoolIndex::new(100).expect("index");
    index
        .apply_block(first_block(&note, root))
        .expect("first insertion");
    assert!(matches!(index.path(1), Err(IndexError::EventOrder)));

    let duplicate = PoolBlock {
        number: 101,
        hash: [4; 32],
        parent_hash: [1; 32],
        events: vec![PoolEvent::Inserted {
            index: 1,
            commitment: note.commitment(),
            root,
            tx_hash: [5; 32],
            log_index: 0,
        }],
    };
    assert!(matches!(
        index.apply_block(duplicate),
        Err(IndexError::EventOrder)
    ));

    let second = [2u8; 32];
    let mut node = note_tree_parent(&note.commitment(), &second).expect("parent");
    let mut zero = [0u8; 32];
    for _ in 1..NOTE_TREE_DEPTH {
        zero = note_tree_parent(&zero, &zero).expect("zero tree");
        node = note_tree_parent(&node, &zero).expect("root");
    }
    index
        .apply_block(PoolBlock {
            number: 101,
            hash: [4; 32],
            parent_hash: [1; 32],
            events: vec![PoolEvent::Inserted {
                index: 1,
                commitment: second,
                root: node,
                tx_hash: [5; 32],
                log_index: 0,
            }],
        })
        .expect("second insertion");
    let path = index.path(1).expect("second path");
    assert_eq!(path.siblings[0], note.commitment());
    assert_eq!(path.root, node);

    let spend = PoolEvent::Consumed {
        nullifier: note.nullifier(),
        tx_hash: [6; 32],
        log_index: 0,
    };
    assert!(matches!(
        index.apply_block(PoolBlock {
            number: 102,
            hash: [7; 32],
            parent_hash: [4; 32],
            events: vec![
                spend.clone(),
                PoolEvent::Consumed {
                    nullifier: note.nullifier(),
                    tx_hash: [6; 32],
                    log_index: 1,
                },
            ],
        }),
        Err(IndexError::EventOrder)
    ));

    index
        .apply_block(PoolBlock {
            number: 102,
            hash: [7; 32],
            parent_hash: [4; 32],
            events: vec![PoolEvent::Transferred {
                deal_commitment: [8; 32],
                deal_nullifier: [9; 32],
                input_nullifier: note.nullifier(),
                tx_hash: [6; 32],
                log_index: 0,
            }],
        })
        .expect("transfer");
    for (deal_nullifier, input_nullifier) in [([9; 32], [10; 32]), ([11; 32], note.nullifier())] {
        assert!(matches!(
            index.apply_block(PoolBlock {
                number: 103,
                hash: [12; 32],
                parent_hash: [7; 32],
                events: vec![PoolEvent::Transferred {
                    deal_commitment: [13; 32],
                    deal_nullifier,
                    input_nullifier,
                    tx_hash: [14; 32],
                    log_index: 0,
                }],
            }),
            Err(IndexError::EventOrder)
        ));
        assert_eq!(index.tip().expect("unchanged tip").number, 102);
    }
}
