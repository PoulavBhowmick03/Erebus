//! Circomlib/Anvil-generated M5 note and tree vectors, checked independently in Rust.

use erebus_core::shielded::{
    note_commitment, note_nullifier, note_root_from_path, note_spend_tag, note_tree_parent,
    NOTE_TREE_DEPTH,
};
use serde_json::Value;

const VECTOR: &str = include_str!("fixtures/m5-note-vector.json");

fn fixed<const N: usize>(hex_value: &str) -> [u8; N] {
    hex::decode(hex_value)
        .expect("pinned hex")
        .try_into()
        .expect("pinned width")
}

fn field<'a>(value: &'a Value, name: &str) -> &'a str {
    value[name].as_str().expect("pinned string")
}

fn zero_nodes() -> [[u8; 32]; NOTE_TREE_DEPTH] {
    let mut nodes = [[0u8; 32]; NOTE_TREE_DEPTH];
    for level in 1..NOTE_TREE_DEPTH {
        nodes[level] = note_tree_parent(&nodes[level - 1], &nodes[level - 1]).expect("zero hash");
    }
    nodes
}

#[test]
fn note_openings_and_paths_match_the_m5_circuit_runner() {
    let vector: Value = serde_json::from_str(VECTOR).expect("pinned JSON");
    let asset = fixed::<20>(field(&vector, "assetHex"));
    let mut opened = Vec::new();

    for name in ["input", "payment"] {
        let note = &vector[name];
        let secret = fixed::<32>(field(note, "spendSecretHex"));
        let tag = note_spend_tag(&secret).expect("tag");
        assert_eq!(hex::encode(tag), field(note, "spendTagHex"));

        let commitment = note_commitment(
            &asset,
            field(note, "amount").parse().expect("amount"),
            &fixed(field(note, "ownerHex")),
            &tag,
            &fixed(field(note, "saltHex")),
        )
        .expect("note");
        assert_eq!(hex::encode(commitment), field(note, "commitmentHex"));
        assert_eq!(
            hex::encode(note_nullifier(&secret, &commitment).expect("nullifier")),
            field(note, "nullifierHex")
        );
        opened.push(commitment);
    }

    let zeros = zero_nodes();
    let input_path = zeros;
    assert_eq!(
        hex::encode(note_root_from_path(&opened[0], 0, &input_path).expect("deposit root")),
        field(&vector["input"], "rootHex")
    );

    let mut payment_path = zeros;
    payment_path[0] = opened[0];
    payment_path[1] = note_tree_parent(&fixed(field(&vector, "changeCommitmentHex")), &[0u8; 32])
        .expect("change subtree");
    assert_eq!(
        hex::encode(note_root_from_path(&opened[1], 1, &payment_path).expect("transfer root")),
        field(&vector["payment"], "rootHex")
    );
    assert_ne!(
        note_root_from_path(&opened[1], 0, &payment_path).expect("wrong index"),
        fixed(field(&vector["payment"], "rootHex"))
    );
    assert!(note_root_from_path(&opened[1], 1 << NOTE_TREE_DEPTH, &payment_path).is_err());
}

#[test]
fn invalid_note_material_is_rejected() {
    let vector: Value = serde_json::from_str(VECTOR).expect("pinned JSON");
    let input = &vector["input"];
    let asset = fixed(field(&vector, "assetHex"));
    let owner = fixed(field(input, "ownerHex"));
    let tag = fixed(field(input, "spendTagHex"));
    let salt = fixed(field(input, "saltHex"));
    let note = fixed(field(input, "commitmentHex"));
    assert!(note_spend_tag(&[0; 32]).is_err());
    assert!(note_nullifier(&[0; 32], &note).is_err());
    assert!(note_commitment(&[0; 20], 150, &owner, &tag, &salt).is_err());
    assert!(note_commitment(&asset, 150, &owner, &[0; 32], &salt).is_err());

    let mut changed_salt = salt;
    changed_salt[31] ^= 1;
    assert_ne!(
        note_commitment(&asset, 150, &owner, &tag, &changed_salt).expect("other note"),
        note
    );
    let mut invalid_field = [0xff; 32];
    assert!(note_nullifier(&fixed(field(input, "spendSecretHex")), &invalid_field).is_err());
    invalid_field[0] = 0;
    assert!(note_tree_parent(&[0xff; 32], &invalid_field).is_err());
}
