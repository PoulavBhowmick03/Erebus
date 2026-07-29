//! Known-answer tests for Cairo Serde of `ClientAction`, against the TypeScript SDK.
//!
//! Cairo emits no reference vector for this encoding, so per CLAUDE.md the TS SDK is the
//! oracle and we diff byte-for-byte. Fixture is `fixtures/ts-clientaction-serde.json`,
//! produced by compiling one action of each variant through upstream's own
//! `serializeClientActions` + `CallData.compile("compile_actions", …)` and stripping the
//! two leading scalars and the span length.
//!
//! This is the ratchet for the wire format. A wrong variant index or a field in the wrong
//! order does not error anywhere — it produces a well-formed action that means something
//! else, and the failure surfaces as a note that cannot be found.

use erebus_sdk::actions::{
    ClientAction, ComputeAndInvokeInput, CreateEncNoteInput, CreateOpenNoteInput, DepositInput,
    InvokeExternalInput, NoteSalt, OpenChannelInput, OpenSubchannelInput, SetViewingKeyInput,
    UseNoteInput, WithdrawInput,
};
use serde::Deserialize;
use starknet_types_core::felt::Felt;

#[derive(Deserialize)]
struct Vector {
    variant: String,
    variant_index: u8,
    felts: Vec<String>,
}

fn load() -> Vec<Vector> {
    let raw = include_str!("fixtures/ts-clientaction-serde.json");
    serde_json::from_str(raw).expect("fixture parses")
}

fn felt(hex: &str) -> Felt {
    Felt::from_hex(hex).expect("fixture felt parses")
}

fn salt(hex: &str) -> NoteSalt {
    let v = u128::from_str_radix(hex.trim_start_matches("0x"), 16).expect("salt parses");
    NoteSalt::new(v).expect("fixture salt is in range")
}

/// The values in the fixture, rebuilt as Rust actions. Kept in fixture order.
fn cases() -> Vec<ClientAction> {
    vec![
        ClientAction::SetViewingKey(SetViewingKeyInput { random: felt("0x5a13d") }),
        ClientAction::OpenChannel(OpenChannelInput {
            recipient_addr: felt("0xb0b"),
            index: 3,
            random: felt("0x5a13d"),
            salt: felt("0x5a17"),
        }),
        ClientAction::OpenSubchannel(OpenSubchannelInput {
            recipient_addr: felt("0xb0b"),
            recipient_public_key: felt("0x9bcdef"),
            channel_key: felt("0xc4a11e"),
            index: 7,
            token: felt("0x7042"),
            salt: felt("0x5a17"),
        }),
        ClientAction::CreateEncNote(CreateEncNoteInput {
            recipient_addr: felt("0xb0b"),
            recipient_public_key: felt("0x9bcdef"),
            token: felt("0x7042"),
            amount: 1_000_000,
            index: 12,
            salt: salt("0x800000000000001234567890abcdef"),
        }),
        ClientAction::CreateOpenNote(CreateOpenNoteInput {
            recipient_addr: felt("0xb0b"),
            recipient_public_key: felt("0x9bcdef"),
            token: felt("0x7042"),
            index: 4,
            random: felt("0x5a13d"),
        }),
        ClientAction::Deposit(DepositInput { token: felt("0x7042"), amount: 500 }),
        ClientAction::UseNote(UseNoteInput {
            channel_key: felt("0xc4a11e"),
            token: felt("0x7042"),
            index: 9,
        }),
        ClientAction::Withdraw(WithdrawInput {
            to_addr: felt("0xa11ce"),
            token: felt("0x7042"),
            amount: 250,
            random: felt("0x5a13d"),
        }),
        ClientAction::InvokeExternal(InvokeExternalInput {
            contract_address: felt("0xa11ce"),
            calldata: vec![felt("0x1"), felt("0x2"), felt("0x3")],
        }),
        ClientAction::ComputeAndInvoke(ComputeAndInvokeInput {
            contract_address: felt("0xa11ce"),
            compute_additional_data: vec![felt("0xaa"), felt("0xbb")],
            invoke_additional_data: vec![felt("0xcc")],
        }),
    ]
}

#[test]
fn every_variant_matches_the_ts_oracle_byte_for_byte() {
    let vectors = load();
    let actions = cases();
    assert_eq!(
        vectors.len(),
        actions.len(),
        "fixture has {} vectors but the test builds {} actions",
        vectors.len(),
        actions.len()
    );

    for (vector, action) in vectors.iter().zip(actions.iter()) {
        let expected: Vec<Felt> = vector.felts.iter().map(|h| felt(h)).collect();
        assert_eq!(
            action.serialize(),
            expected,
            "Cairo Serde mismatch for {}",
            vector.variant
        );
    }
}

#[test]
fn variant_indices_match_the_cairo_enum_order() {
    for (vector, action) in load().iter().zip(cases().iter()) {
        assert_eq!(
            action.variant_index(),
            vector.variant_index,
            "variant index mismatch for {}",
            vector.variant
        );
    }
}

#[test]
fn action_span_is_length_prefixed() {
    let actions = cases();
    let encoded = erebus_sdk::actions::serialize_actions(&actions);
    assert_eq!(encoded[0], Felt::from(actions.len() as u64));

    let mut flat = vec![Felt::from(actions.len() as u64)];
    for action in &actions {
        flat.extend(action.serialize());
    }
    assert_eq!(encoded, flat);
}

#[test]
fn empty_action_span_encodes_as_a_single_zero() {
    assert_eq!(erebus_sdk::actions::serialize_actions(&[]), vec![Felt::ZERO]);
}

// --- NoteSalt bounds -------------------------------------------------------------
// The contract rejects these (SALT_TOO_SMALL / SALT_EXCEEDS_120_BITS), but by then the
// transaction has already cost a proof. The newtype moves the failure to construction.

#[test]
fn note_salt_rejects_reserved_and_out_of_range_values() {
    assert!(NoteSalt::new(0).is_err(), "0 means the note does not exist");
    assert!(NoteSalt::new(1).is_err(), "1 is reserved for open notes");
    assert!(NoteSalt::new(NoteSalt::TWO_POW_120).is_err(), "salts are 120-bit");
    assert!(NoteSalt::new(u128::MAX).is_err());
}

#[test]
fn note_salt_accepts_the_boundaries_and_the_salt_lane() {
    assert!(NoteSalt::new(2).is_ok());
    assert!(NoteSalt::new(NoteSalt::TWO_POW_120 - 1).is_ok());
    // Erebus pins bit 119, so a payload salt is always in [2^119, 2^120).
    assert!(NoteSalt::new(1u128 << 119).is_ok());
    assert!(NoteSalt::new((1u128 << 120) - 1).is_ok());
}

// --- Phase ordering --------------------------------------------------------------

#[test]
fn phases_match_the_cairo_mapping() {
    use erebus_sdk::actions::phase;
    let expected = [
        phase::ACCOUNT,      // SetViewingKey
        phase::CHANNEL,      // OpenChannel
        phase::SUBCHANNEL,   // OpenSubchannel
        phase::CREATE_NOTES, // CreateEncNote
        phase::CREATE_NOTES, // CreateOpenNote
        phase::DEPOSIT,      // Deposit
        phase::USE_NOTES,    // UseNote
        phase::WITHDRAW,     // Withdraw
        phase::INVOKE,       // InvokeExternal
        phase::INVOKE,       // ComputeAndInvoke
    ];
    for (action, want) in cases().iter().zip(expected.iter()) {
        assert_eq!(action.phase(), *want, "phase mismatch for {action:?}");
    }
}

#[test]
fn phase_order_is_not_variant_order() {
    // UseNote has a higher variant index than CreateEncNote but runs before it. Encoding
    // an action set in variant order would be rejected with ACTIONS_OUT_OF_ORDER.
    let use_note = ClientAction::UseNote(UseNoteInput {
        channel_key: Felt::ONE,
        token: Felt::ONE,
        index: 0,
    });
    let create = ClientAction::CreateEncNote(CreateEncNoteInput {
        recipient_addr: Felt::ONE,
        recipient_public_key: Felt::ONE,
        token: Felt::ONE,
        amount: 0,
        index: 0,
        salt: NoteSalt::new(2).expect("2 is in range"),
    });
    assert!(use_note.variant_index() > create.variant_index());
    assert!(use_note.phase() < create.phase());
}
