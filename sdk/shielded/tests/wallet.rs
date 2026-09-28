//! Local encrypted-wallet storage, restore, and state-transition checks.

use std::fs;

use erebus_shielded_prover::wallet::{
    NoteConsumption, NoteInclusion, OwnedNote, WalletDomain, WalletError, WalletStore,
};
use serde_json::Value;

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
