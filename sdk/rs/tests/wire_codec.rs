//! Wire-v2 confidentiality/authentication tests plus frozen wire-v1 compatibility vectors.
//!
//! Fixture: `fixtures/ts-wire-salts.json`, regenerate with
//! `cd sdk/ts && pnpm vitest run tests/gen-wire-vectors.test.ts`.
//!
//! This format is Erebus's own, so Cairo emits nothing to check it against. Wire v1 remains
//! checked against the independent TypeScript implementation; wire v2 is pinned by a known
//! answer plus round-trip, tamper, context and migration tests until a second implementation
//! exists. A wrong chunk writes a note that the counterparty cannot authenticate or decode.

use erebus_sdk::actions::NoteSalt;
use erebus_sdk::wire::{
    decode_legacy_message, decode_message, encode_legacy_message, encode_message,
    legacy_note_index_for_message, note_index_for_message, truncate_memo_hash,
    truncate_memo_hash_bytes, MessageType, WireContext, WireError, WireMessage, CAPACITY_BITS,
    LEGACY_CAPACITY_BITS, LEGACY_NOTES_PER_MESSAGE, MESSAGE_BITS, NOTES_PER_MESSAGE,
    PAYLOAD_BITS_PER_NOTE,
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

fn context(message_index: u32) -> WireContext {
    WireContext {
        chain_id: Felt::from_hex("0x534e5f5345504f4c4941").expect("chain"),
        pool_address: Felt::from_hex("0x9001").expect("pool"),
        channel_key: Felt::from_hex("0xc4a11e").expect("channel key"),
        token: Felt::from_hex("0x7042").expect("token"),
        message_index,
    }
}

#[test]
fn legacy_constants_match_the_typescript() {
    let c = load().constants;
    assert_eq!(c.notes_per_message, LEGACY_NOTES_PER_MESSAGE);
    assert_eq!(c.payload_bits_per_note, PAYLOAD_BITS_PER_NOTE);
    assert_eq!(c.message_bits, MESSAGE_BITS);
    assert_eq!(c.capacity_bits, LEGACY_CAPACITY_BITS);
    assert_eq!(NOTES_PER_MESSAGE, 5);
    assert_eq!(CAPACITY_BITS, 595);
}

/// The claim that matters: same message in, same salts out.
#[test]
fn legacy_salts_still_match_the_typescript_oracle() {
    let fixture = load();
    assert!(!fixture.vectors.is_empty());
    for v in &fixture.vectors {
        let expected: Vec<NoteSalt> = v.salts.iter().map(|s| salt_of(s)).collect();
        let actual = encode_legacy_message(&build(&v.message)).expect("encoding succeeds");
        assert_eq!(actual.to_vec(), expected, "salt mismatch for {}", v.name);
    }
}

#[test]
fn decoding_legacy_oracle_salts_recovers_the_message() {
    for v in &load().vectors {
        let salts: Vec<NoteSalt> = v.salts.iter().map(|s| salt_of(s)).collect();
        let array: [NoteSalt; LEGACY_NOTES_PER_MESSAGE] =
            salts.try_into().expect("fixture has four salts");
        let decoded = decode_legacy_message(&array).expect("decoding succeeds");
        assert_eq!(decoded, build(&v.message), "decode mismatch for {}", v.name);
    }
}

#[test]
fn round_trip_is_identity() {
    for (index, v) in load().vectors.iter().enumerate() {
        let original = build(&v.message);
        let context = context(index as u32);
        let salts = encode_message(&context, &original).expect("encoding succeeds");
        assert_eq!(
            decode_message(&context, &salts).expect("decoding succeeds"),
            original,
            "{}",
            v.name
        );
    }
}

#[test]
fn changing_any_ciphertext_bit_fails_authentication() {
    let message = build(&load().vectors[0].message);
    let context = context(0);
    let mut salts = encode_message(&context, &message).expect("encoding succeeds");
    salts[0] = NoteSalt::new(salts[0].get() ^ 1).expect("flag remains set");

    assert_eq!(
        decode_message(&context, &salts),
        Err(WireError::Authentication)
    );
}

#[test]
fn ciphertext_is_bound_to_chain_pool_channel_token_and_index() {
    let message = build(&load().vectors[0].message);
    let context = context(7);
    let salts = encode_message(&context, &message).expect("encoding succeeds");

    let wrong_contexts = [
        WireContext {
            chain_id: context.chain_id + Felt::ONE,
            ..context
        },
        WireContext {
            pool_address: context.pool_address + Felt::ONE,
            ..context
        },
        WireContext {
            channel_key: context.channel_key + Felt::ONE,
            ..context
        },
        WireContext {
            token: context.token + Felt::ONE,
            ..context
        },
        WireContext {
            message_index: context.message_index + 1,
            ..context
        },
    ];

    for wrong in wrong_contexts {
        assert_eq!(
            decode_message(&wrong, &salts),
            Err(WireError::Authentication),
            "wrong context unexpectedly authenticated: {wrong:?}"
        );
    }
}

#[test]
fn wire_v2_has_a_pinned_known_answer_and_changes_with_context() {
    let message = build(&load().vectors[0].message);
    let first = encode_message(&context(0), &message).expect("encoding succeeds");
    let retry = encode_message(&context(0), &message).expect("encoding succeeds");
    let next = encode_message(&context(1), &message).expect("encoding succeeds");

    assert_eq!(
        first, retry,
        "safe retries must reproduce the same action set"
    );
    assert_ne!(first, next, "the derived nonce must change with the index");
    assert_eq!(
        first.map(NoteSalt::get),
        [
            856_248_648_942_901_945_608_550_083_923_183_535,
            1_102_952_624_952_783_045_360_253_090_041_748_709,
            1_095_873_189_828_646_305_366_058_636_551_464_342,
            795_592_087_567_446_721_473_319_733_897_145_163,
            664_613_997_892_457_936_462_562_037_547_047_431,
        ],
        "wire-v2 changes require an explicit format/version decision"
    );
}

#[test]
fn a_failed_attempt_can_change_terms_at_the_same_index_without_breaking_decryption() {
    let first_message = build(&load().vectors[0].message);
    let mut retry_message = first_message;
    retry_message.amount += 1;
    let context = context(9);

    let first = encode_message(&context, &first_message).expect("first attempt");
    let retry = encode_message(&context, &retry_message).expect("changed retry");

    assert_ne!(first, retry);
    assert_eq!(
        decode_message(&context, &first).expect("first authenticates"),
        first_message
    );
    assert_eq!(
        decode_message(&context, &retry).expect("retry authenticates"),
        retry_message
    );
}

#[test]
fn non_canonical_high_padding_is_rejected() {
    let message = build(&load().vectors[0].message);
    let context = context(0);
    let mut salts = encode_message(&context, &message).expect("encoding succeeds");
    salts[4] = NoteSalt::new(salts[4].get() | (1u128 << 60)).expect("salt remains in range");

    assert_eq!(
        decode_message(&context, &salts),
        Err(WireError::InvalidV2Envelope)
    );
}

#[test]
fn debug_never_prints_the_channel_key() {
    let rendered = format!("{:?}", context(0));
    assert!(rendered.contains("redacted"));
    assert!(!rendered.contains("c4a11e"));
}

#[test]
fn legacy_note_indices_match_the_typescript() {
    for entry in &load().note_indices {
        assert_eq!(
            legacy_note_index_for_message(entry.message_index),
            entry.first_note_index
        );
    }
    assert_eq!(note_index_for_message(7), 35);
}

/// Every salt the encoder emits must satisfy the contract's `2 <= salt < 2^120`. The
/// pinned flag bit guarantees this, and `NoteSalt` checks it. Without the flag, construction
/// fails before an unspendable note reaches the pool.
#[test]
fn every_emitted_salt_is_contract_valid() {
    for (index, v) in load().vectors.iter().enumerate() {
        for salt in
            encode_message(&context(index as u32), &build(&v.message)).expect("encoding succeeds")
        {
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
    assert_eq!(
        encode_message(&context(0), &message),
        Err(WireError::ReservedReplyTo)
    );
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
        encode_message(&context(0), &message),
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
    let context = context(0);
    let mut salts = encode_message(&context, &message).expect("encoding succeeds");
    salts[2] = ordinary;
    assert_eq!(
        decode_message(&context, &salts),
        Err(WireError::MissingFlag(2))
    );
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

/// A real digest is 256 bits, above the `felt252` modulus, so it cannot reach the wire
/// through a `Felt`. The byte form is what a caller passing SHA-256 output actually needs.
#[test]
fn a_whole_digest_truncates_to_the_same_low_bits_as_a_felt() {
    let digest: [u8; 32] = [
        0xff, 0xee, 0xdd, 0xcc, 0xbb, 0xaa, 0x99, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11,
        0x00, 0x9a, 0xbc, 0xde, 0xf0, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x12, 0x34,
        0x56, 0x78,
    ];
    assert_eq!(
        truncate_memo_hash_bytes(&digest),
        u128_of("0x9abcdef0123456789abcdef012345678"),
        "the low 16 bytes are kept, the high 16 are dropped"
    );

    // The same 256-bit value cannot survive a Felt round trip, which is why the byte entry
    // point exists rather than being a convenience wrapper.
    let wide = "0xffeeddccbbaa997788776655443322119abcdef0123456789abcdef012345678";
    match Felt::from_hex(wide) {
        Err(_) => {}
        Ok(felt) => assert_ne!(
            felt.to_bytes_be().as_slice(),
            &digest[..],
            "a 256-bit digest does not fit felt252 and must not appear to"
        ),
    }

    // Narrower input is left-padded, so a short memo and the same value inside a wide digest
    // agree rather than shifting.
    assert_eq!(truncate_memo_hash_bytes(&[0x01, 0x02]), 0x0102);
    assert_eq!(truncate_memo_hash_bytes(&[]), 0);
}

/// Both entry points implement one rule. If they ever disagree, the wire silently commits to
/// a different memo than the caller believes it did.
#[test]
fn the_felt_and_byte_truncations_agree_wherever_both_are_defined() {
    for value in ["0x0", "0x1", "0xdeadbeef", "0x1bc16d674ec80000"] {
        let felt = Felt::from_hex(value).expect("parses");
        assert_eq!(
            truncate_memo_hash(felt),
            truncate_memo_hash_bytes(&felt.to_bytes_be()),
            "{value}"
        );
    }
}

/// The header is in the last salt. The TypeScript module's ASCII table incorrectly places it
/// in the first salt. This test pins the implemented order.
#[test]
fn the_legacy_header_is_in_the_most_significant_salt() {
    let message = WireMessage {
        message_type: MessageType::Accept, // code 3
        reply_to: None,
        created_at: 0,
        amount: 0,
        deadline: 0,
        memo_hash: 0,
    };
    let salts = encode_legacy_message(&message).expect("encoding succeeds");
    // salts[3] = type << 35 | NO_REPLY_TO << 3, plus the pinned flag.
    let expected = (1u128 << 119) | (3u128 << 35) | (u128::from(u32::MAX) << 3);
    assert_eq!(salts[3].get(), expected);
    // and nothing else carries the type
    assert_eq!(salts[0].get(), 1u128 << 119);
}
