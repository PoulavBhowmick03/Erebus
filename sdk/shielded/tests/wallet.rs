//! Local encrypted-wallet storage, restore, and state-transition checks.

use std::fs;

use erebus_shielded_prover::wallet::{
    NoteConsumption, NoteInclusion, OwnedNote, WalletDomain, WalletError, WalletStore,
};
use serde_json::Value;

mod support;

#[test]
fn every_wallet_write_boundary_preserves_a_restartable_note_reservation() {
    let mut boundaries = 0;
    let mut fail = 0;
    loop {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("private/wallet.enc");
        let (_, input) = confirmed_store(&path);
        let hook = std::sync::Arc::new(support::SweepHook::new(if fail == 0 {
            usize::MAX
        } else {
            fail
        }));
        let store = WalletStore::with_faults(&path, domain(), [7; 32], hook.clone()).unwrap();
        let result = store.update(|wallet| wallet.reserve(&input, [4; 32]));
        if fail == 0 {
            result.unwrap();
            boundaries = hook.count();
            assert!(boundaries > 0);
        } else {
            assert!(matches!(result, Err(WalletError::Io(_))));
        }
        drop(store);
        let restored = WalletStore::new(&path, domain(), [7; 32]).unwrap();
        assert_eq!(restored.snapshot().unwrap().notes().len(), 1);
        restored
            .update(|wallet| wallet.reserve(&input, [4; 32]))
            .unwrap();
        assert!(restored.snapshot().unwrap().spendable(&input).is_none());
        assert!(restored
            .update(|wallet| wallet.reserve(&input, [5; 32]))
            .is_err());
        assert!(restored
            .snapshot()
            .unwrap()
            .spendable_for(&input, [4; 32])
            .is_some());
        fail += 1;
        if fail > boundaries {
            break;
        }
    }
}

fn bytes<const N: usize>(hex_value: &str) -> [u8; N] {
    hex::decode(hex_value)
        .expect("fixture hex")
        .try_into()
        .expect("fixture width")
}

fn fixture_note() -> OwnedNote {
    let vector: Value = serde_json::from_str(include_str!(
        "../../core/tests/fixtures/m5-note-vector.json"
    ))
    .expect("pinned note vector");
    let note = &vector["input"];
    let value = |name: &str| note[name].as_str().expect("fixture string");
    let opening = OwnedNote::new(
        bytes(vector["assetHex"].as_str().expect("asset")),
        value("amount").parse().expect("amount"),
        bytes(value("ownerHex")),
        bytes(value("spendSecretHex")),
        bytes(value("saltHex")),
    )
    .expect("valid local note");
    assert_eq!(opening.commitment(), bytes(value("commitmentHex")));
    assert_eq!(opening.nullifier(), bytes(value("nullifierHex")));
    opening
}

fn domain() -> WalletDomain {
    WalletDomain {
        chain_id: 10143,
        pool: [2; 20],
    }
}

#[test]
fn encrypted_backup_restores_note_and_replays_after_reorg() {
    let root = tempfile::tempdir().expect("private temp dir");
    let path = root.path().join("private/wallet.enc");
    let key = [7; 32];
    let store = WalletStore::new(&path, domain(), key).expect("wallet store");
    let note = fixture_note();
    let commitment = note.commitment();
    let nullifier = note.nullifier();
    store
        .update(|wallet| wallet.add(note))
        .expect("persist expected note");
    let ciphertext = fs::read(&path).expect("encrypted wallet");
    assert!(!ciphertext
        .windows(b"spend_secret".len())
        .any(|part| part == b"spend_secret"));
    assert_ne!(&ciphertext[..8], b"{\"notes\"");

    let backup = root.path().join("private/backup.enc");
    fs::copy(&path, &backup).expect("backup encrypted file");
    let restored = WalletStore::new(&backup, domain(), key).expect("restore store");
    assert_eq!(restored.snapshot().expect("restore").notes().len(), 1);
    let inclusion = NoteInclusion {
        index: 0,
        block_number: 12,
        block_hash: [3; 32],
        root: [4; 32],
    };
    restored
        .update(|wallet| {
            assert!(wallet.observe_insertion(&commitment, inclusion)?);
            assert!(wallet.observe_insertion(&commitment, inclusion)?);
            Ok(())
        })
        .expect("discover note");
    let snapshot = restored.snapshot().expect("confirmed note");
    assert_eq!(snapshot.notes()[0].inclusion(), Some(inclusion));
    let asset = bytes::<20>("5fbdb2315678afecb367f032d93f642f64180aa3");
    assert_eq!(
        snapshot.select(&asset, 70).expect("spendable").amount(),
        150
    );

    restored
        .update(|wallet| {
            wallet.reserve(&commitment, [5; 32])?;
            wallet.reserve(&commitment, [5; 32])?;
            assert!(wallet.select(&asset, 70).is_none());
            assert!(wallet.reserve(&commitment, [6; 32]).is_err());
            wallet.release(&commitment, [5; 32])?;
            wallet.observe_consumption(
                &nullifier,
                NoteConsumption {
                    block_number: 15,
                    block_hash: [8; 32],
                    tx_hash: [9; 32],
                },
            )?;
            assert!(wallet.select(&asset, 70).is_none());
            wallet.rewind_from(15);
            assert!(wallet.select(&asset, 70).is_some());
            wallet.rewind_from(12);
            assert!(wallet.select(&asset, 70).is_none());
            Ok(())
        })
        .expect("reorg replay");
    assert_eq!(
        restored.snapshot().expect("reopened").notes()[0].inclusion(),
        None
    );
}

#[test]
fn wrong_key_domain_and_corruption_never_reset_a_wallet() {
    let root = tempfile::tempdir().expect("temp dir");
    let path = root.path().join("private/wallet.enc");
    let store = WalletStore::new(&path, domain(), [7; 32]).expect("store");
    store
        .update(|wallet| wallet.add(fixture_note()))
        .expect("save");
    let before = fs::read(&path).expect("file");
    let wrong_key = WalletStore::new(&path, domain(), [8; 32]).expect("other key");
    assert!(matches!(
        wrong_key.snapshot(),
        Err(WalletError::Authentication)
    ));
    let wrong_domain = WalletStore::new(
        &path,
        WalletDomain {
            chain_id: 10144,
            ..domain()
        },
        [7; 32],
    )
    .expect("other domain");
    assert!(matches!(
        wrong_domain.snapshot(),
        Err(WalletError::Authentication)
    ));
    assert_eq!(fs::read(&path).expect("unchanged"), before);
    assert!(store
        .update(|wallet| {
            wallet.add(fixture_note())?;
            Ok(())
        })
        .is_err());
    assert_eq!(fs::read(&path).expect("still unchanged"), before);
    fs::write(&path, b"truncated").expect("simulate corruption");
    assert!(matches!(store.snapshot(), Err(WalletError::InvalidFile)));
}

fn confirmed_store(path: &std::path::Path) -> (WalletStore, [u8; 32]) {
    let store = WalletStore::new(path, domain(), [7; 32]).unwrap();
    let note = fixture_note();
    let input = note.commitment();
    store
        .update(|wallet| {
            wallet.add(note)?;
            wallet.observe_insertion(
                &input,
                NoteInclusion {
                    index: 0,
                    block_number: 12,
                    block_hash: [3; 32],
                    root: [4; 32],
                },
            )?;
            Ok(())
        })
        .unwrap();
    (store, input)
}

fn change_note() -> OwnedNote {
    let note = fixture_note();
    OwnedNote::new(note.asset(), 80, note.owner(), [1; 32], [2; 32]).unwrap()
}

#[test]
fn competing_stores_reserve_exactly_one_operation() {
    use std::sync::{Arc, Barrier};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("private/wallet.enc");
    let (store, input) = confirmed_store(&path);
    let barrier = Arc::new(Barrier::new(4));
    let workers: Vec<_> = (1..=4)
        .map(|id| {
            let barrier = barrier.clone();
            let path = path.clone();
            std::thread::spawn(move || {
                let store = WalletStore::new(path, domain(), [7; 32]).unwrap();
                barrier.wait();
                (
                    id,
                    store
                        .update(|wallet| {
                            wallet.reserve_transfer([id; 32], input, [9; 32], Some(change_note()))
                        })
                        .is_ok(),
                )
            })
        })
        .collect();
    let winners: Vec<_> = workers
        .into_iter()
        .map(|w| w.join().unwrap())
        .filter(|(_, ok)| *ok)
        .collect();
    assert_eq!(winners.len(), 1);
    let operation = [winners[0].0; 32];
    let saved = store.snapshot().unwrap();
    assert_eq!(saved.notes().len(), 2);
    assert!(saved.spendable(&input).is_none());
    assert!(saved.spendable_for(&input, operation).is_some());
    store
        .update(|wallet| wallet.reserve_transfer(operation, input, [9; 32], Some(change_note())))
        .unwrap();
    assert_eq!(store.snapshot().unwrap().notes().len(), 2);
    let before = fs::read(&path).unwrap();
    assert!(store
        .update(|wallet| wallet.reserve_transfer(operation, input, [8; 32], Some(change_note())))
        .is_err());
    assert!(store
        .update(|wallet| wallet.release(&input, [10; 32]))
        .is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn failed_transition_and_failed_persistence_do_not_report_success() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("private/wallet.enc");
    let (store, input) = confirmed_store(&path);
    let before = fs::read(&path).unwrap();
    let result: Result<(), WalletError> = store.update(|wallet| {
        wallet.reserve_transfer([5; 32], input, [9; 32], Some(change_note()))?;
        Err(WalletError::Note("injected failure before write"))
    });
    assert!(result.is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(store.snapshot().unwrap().spendable(&input).is_some());

    // Force rename failure after the new encrypted snapshot has been written.
    let backup = path.with_extension("backup");
    let result = store.update(|wallet| {
        wallet.reserve_transfer([5; 32], input, [9; 32], Some(change_note()))?;
        fs::rename(&path, &backup)?;
        fs::create_dir(&path)?;
        Ok(())
    });
    assert!(matches!(result, Err(WalletError::Io(_))));
    fs::remove_dir(&path).unwrap();
    fs::rename(&backup, &path).unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(store.snapshot().unwrap().notes().len(), 1);
    assert!(store.snapshot().unwrap().spendable(&input).is_some());
    assert!(!fs::read_dir(path.parent().unwrap())
        .unwrap()
        .any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("tmp-")));
}

#[test]
fn legacy_snapshot_loads_and_released_operation_cannot_be_rebound() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("private/wallet.enc");
    let (store, input) = confirmed_store(&path);
    let mut encoded = serde_json::to_value(store.snapshot().unwrap()).unwrap();
    encoded.as_object_mut().unwrap().remove("transfers");
    let legacy = serde_json::from_value(encoded).unwrap();
    store
        .update(|wallet| {
            *wallet = legacy;
            Ok(())
        })
        .unwrap();
    store
        .update(|wallet| wallet.reserve_transfer([5; 32], input, [9; 32], None))
        .unwrap();
    store
        .update(|wallet| wallet.release(&input, [5; 32]))
        .unwrap();
    assert!(store
        .update(|wallet| wallet.reserve_transfer([5; 32], input, [9; 32], None))
        .is_err());
    store
        .update(|wallet| wallet.reserve_transfer([6; 32], input, [9; 32], None))
        .unwrap();
}
