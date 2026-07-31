//! Differential tests for the salt-lane wire codec, against the TypeScript oracle.
//!
//! Fixture: `fixtures/ts-wire-salts.json`, regenerate with
//! `cd sdk/ts && pnpm vitest run tests/gen-wire-vectors.test.ts`.
//!
//! This format is Erebus's own, so Cairo emits nothing to check it against and there is no
//! written spec. Two independent implementations agreeing on the same salts is the entire
//! correctness argument. A wrong chunk here does not throw — it writes a note whose salt
//! decodes to a different offer, or to nothing the counterparty recognises.

use erebus_sdk::actions::NoteSalt;
use erebus_sdk::wire::{
    decode_message, encode_message, note_index_for_message, truncate_memo_hash, MessageType,
    WireError, WireMessage, CAPACITY_BITS, MESSAGE_BITS, NOTES_PER_MESSAGE, PAYLOAD_BITS_PER_NOTE,
};
use serde::Deserialize;
use starknet_types_core::felt::Felt;

#[derive(Deserialize)]
struct Fixture {
    constants: Constants,
    note_indices: Vec<NoteIndex>,
    vectors: Vec<Vector>,
}

#[derive(Deserialize)]
struct Constants {
    notes_per_message: usize,
    payload_bits_per_note: u32,
    message_bits: u32,
    capacity_bits: u32,
}

#[derive(Deserialize)]
struct NoteIndex {
    message_index: u32,
    first_note_index: u32,
}

#[derive(Deserialize)]
struct Vector {
    name: String,
    message: Message,
    salts: Vec<String>,
}

#[derive(Deserialize)]
struct Message {
    #[serde(rename = "type")]
    message_type: String,
    reply_to: Option<u32>,
    created_at: u64,
    amount: String,
    deadline: u64,
    memo_hash: String,
}

fn load() -> Fixture {
    serde_json::from_str(include_str!("fixtures/ts-wire-salts.json")).expect("fixture parses")
}

fn u128_of(hex: &str) -> u128 {
    u128::from_str_radix(hex.trim_start_matches("0x"), 16).expect("fixture u128 parses")
}

fn salt_of(hex: &str) -> NoteSalt {
    NoteSalt::new(u128_of(hex)).expect("fixture salt is in range")
}

fn build(m: &Message) -> WireMessage {
    let message_type = match m.message_type.as_str() {
        "offer" => MessageType::Offer,
        "counter" => MessageType::Counter,
        "accept" => MessageType::Accept,
        other => panic!("unknown message type in fixture: {other}"),
    };
    WireMessage {
        message_type,
        reply_to: m.reply_to,
        created_at: m.created_at,
        amount: u128_of(&m.amount),
        deadline: m.deadline,
        memo_hash: truncate_memo_hash(
            Felt::from_hex(&m.memo_hash).expect("fixture memo hash parses"),
        ),
    }
}

#[test]
fn constants_match_the_typescript() {
    let c = load().constants;
    assert_eq!(c.notes_per_message, NOTES_PER_MESSAGE);
    assert_eq!(c.payload_bits_per_note, PAYLOAD_BITS_PER_NOTE);
    assert_eq!(c.message_bits, MESSAGE_BITS);
    assert_eq!(c.capacity_bits, CAPACITY_BITS);
}

/// The claim that matters: same message in, same salts out.
#[test]
fn encoded_salts_match_the_typescript_oracle() {
    let fixture = load();
    assert!(!fixture.vectors.is_empty());
    for v in &fixture.vectors {
        let expected: Vec<NoteSalt> = v.salts.iter().map(|s| salt_of(s)).collect();
        let actual = encode_message(&build(&v.message)).expect("encoding succeeds");
        assert_eq!(actual.to_vec(), expected, "salt mismatch for {}", v.name);
    }
}

#[test]
fn decoding_the_oracle_salts_recovers_the_message() {
    for v in &load().vectors {
        let salts: Vec<NoteSalt> = v.salts.iter().map(|s| salt_of(s)).collect();
        let array: [NoteSalt; NOTES_PER_MESSAGE] =
            salts.try_into().expect("fixture has four salts");
        let decoded = decode_message(&array).expect("decoding succeeds");
        assert_eq!(decoded, build(&v.message), "decode mismatch for {}", v.name);
    }
}

#[test]
fn round_trip_is_identity() {
    for v in &load().vectors {
        let original = build(&v.message);
        let salts = encode_message(&original).expect("encoding succeeds");
        assert_eq!(
            decode_message(&salts).expect("decoding succeeds"),
            original,
            "{}",
            v.name
        );
    }
}

#[test]
fn note_indices_match_the_typescript() {
    for entry in &load().note_indices {
        assert_eq!(
            note_index_for_message(entry.message_index),
            entry.first_note_index
        );
    }
}

/// Every salt the encoder emits must satisfy the contract's `2 <= salt < 2^120`. The
/// pinned flag bit is what guarantees this, and `NoteSalt` is what checks it — if the flag
/// were ever dropped, construction would fail rather than a note landing unspendable.
#[test]
fn every_emitted_salt_is_contract_valid() {
    for v in &load().vectors {
        for salt in encode_message(&build(&v.message)).expect("encoding succeeds") {
            assert!(salt.get() >= (1u128 << 119), "salt lost the format flag");
            assert!(salt.get() < NoteSalt::TWO_POW_120);
        }
    }
}

// --- Behaviours the fixture cannot express ---------------------------------------

#[test]
fn the_reserved_reply_to_sentinel_is_rejected() {
    let message = WireMessage {
        message_type: MessageType::Counter,
        reply_to: Some(u32::MAX),
        created_at: 1,
        amount: 1,
        deadline: 1,
        memo_hash: 1,
    };
    assert_eq!(encode_message(&message), Err(WireError::ReservedReplyTo));
}

#[test]
fn an_oversized_created_at_is_rejected() {
    let message = WireMessage {
        message_type: MessageType::Offer,
        reply_to: None,
        created_at: 1u64 << 40, // 40 bits is the budget
        amount: 0,
        deadline: 0,
        memo_hash: 0,
    };
    assert!(matches!(
        encode_message(&message),
        Err(WireError::FieldTooWide {
            field: "createdAt",
            ..
        })
    ));
}

#[test]
fn a_salt_without_the_flag_is_not_an_erebus_note() {
    // A plausible random salt from an ordinary value-bearing note.
    let ordinary = NoteSalt::new(0x1234_5678).expect("in range");
    let message = WireMessage {
        message_type: MessageType::Offer,
        reply_to: None,
        created_at: 1,
        amount: 1,
        deadline: 1,
        memo_hash: 1,
    };
    let mut salts = encode_message(&message).expect("encoding succeeds");
    salts[2] = ordinary;
    assert_eq!(decode_message(&salts), Err(WireError::MissingFlag(2)));
}

#[test]
fn memo_hash_truncation_keeps_the_low_128_bits() {
    // A near-full-width felt (63 hex digits, leading 7 keeps it under the STARK prime);
    // only the bottom 128 bits survive onto the wire.
    let felt = Felt::from_hex("0x7bcdef0123456789abcdef0123456789abcdef0123456789abcdef012345678")
        .expect("parses");
    assert_eq!(
        truncate_memo_hash(felt),
        u128_of("0x9abcdef0123456789abcdef012345678")
    );
}

/// The header lands in the *last* salt, not the first — the TypeScript module's ASCII
/// table says otherwise and is wrong. Pinned so a future reader trusts the code.
#[test]
fn the_header_is_in_the_most_significant_salt() {
    let message = WireMessage {
        message_type: MessageType::Accept, // code 3
        reply_to: None,
        created_at: 0,
        amount: 0,
        deadline: 0,
        memo_hash: 0,
    };
    let salts = encode_message(&message).expect("encoding succeeds");
    // salts[3] = type << 35 | NO_REPLY_TO << 3, plus the pinned flag.
    let expected = (1u128 << 119) | (3u128 << 35) | (u128::from(u32::MAX) << 3);
    assert_eq!(salts[3].get(), expected);
    // and nothing else carries the type
    assert_eq!(salts[0].get(), 1u128 << 119);
}
