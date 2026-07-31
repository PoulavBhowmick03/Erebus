//! Tests for the composed channel operations.
//!
//! The pieces below are each pinned elsewhere — hashes against Cairo vectors, salts against
//! the TypeScript oracle, ordering against the contract's revert conditions. What this file
//! checks is that they are wired together correctly: the right salt on the right note at
//! the right index, addressed to the right party.

use erebus_sdk::actions::ClientAction;
use erebus_sdk::channel::{Channel, Counterparty, PoolIdentity};
use erebus_sdk::hashes;
use erebus_sdk::wire::{encode_message, MessageType, WireMessage, NOTES_PER_MESSAGE};
use starknet_types_core::felt::Felt;

fn alice() -> PoolIdentity {
    PoolIdentity::new(
        Felt::from_hex("0xa11ce").expect("addr"),
        Felt::from_hex("0x1234567890abcdef").expect("key"),
    )
}

fn bob() -> Counterparty {
    Counterparty {
        address: Felt::from_hex("0xb0b").expect("addr"),
        public_key: Felt::from_hex("0x9bcdef").expect("pubkey"),
    }
}

fn offer() -> WireMessage {
    WireMessage {
        message_type: MessageType::Offer,
        reply_to: None,
        created_at: 1_753_699_200,
        amount: 1_000_000,
        deadline: 1_753_702_800,
        memo_hash: 0x1234_5678_9abc_def0,
    }
}

fn token() -> Felt {
    Felt::from_hex("0x7042").expect("token")
}

// --- Key containment ------------------------------------------------------------

/// CLAUDE.md constraint 6, as a test rather than a rule to remember. If a key ever
/// reaches a log line it will be through `Debug`.
#[test]
fn debug_does_not_leak_the_private_key() {
    let identity = alice();
    let rendered = format!("{identity:?}");
    assert!(rendered.contains("redacted"), "key should be redacted");
    assert!(
        !rendered.contains("1234567890abcdef"),
        "private key leaked through Debug: {rendered}"
    );
}

#[test]
fn the_public_key_is_derived_not_stored() {
    // Deriving twice agrees, and matches the curve operation directly.
    let identity = alice();
    assert_eq!(identity.public_key(), identity.public_key());
    assert_eq!(
        identity.public_key(),
        starknet_crypto::get_public_key(&Felt::from_hex("0x1234567890abcdef").expect("key"))
    );
}

// --- Channel derivation ---------------------------------------------------------

#[test]
fn the_channel_key_matches_the_pinned_derivation() {
    let channel = Channel::derive(&alice(), bob());
    let expected = hashes::compute_channel_key(
        Felt::from_hex("0xa11ce").expect("addr"),
        Felt::from_hex("0x1234567890abcdef").expect("key"),
        bob().address,
        bob().public_key,
    );
    assert_eq!(channel.key(), expected);
}

/// Channels are directional: A→B and B→A are different, because the derivation hashes the
/// *sender's* private key. Getting this wrong would put both parties' messages in the same
/// place and break the whole addressing scheme.
#[test]
fn the_reverse_channel_has_a_different_key() {
    let a_to_b = Channel::derive(&alice(), bob());

    let bob_identity = PoolIdentity::new(
        bob().address,
        Felt::from_hex("0xfedcba0987654321").expect("key"),
    );
    let alice_as_counterparty = Counterparty {
        address: alice().address(),
        public_key: alice().public_key(),
    };
    let b_to_a = Channel::derive(&bob_identity, alice_as_counterparty);

    assert_ne!(a_to_b.key(), b_to_a.key());
}

#[test]
fn a_received_channel_key_reconstructs_the_same_channel() {
    let derived = Channel::derive(&alice(), bob());
    let received = Channel::from_key(derived.key(), bob());
    assert_eq!(derived, received);
}

// --- Writing a message ----------------------------------------------------------

#[test]
fn a_message_becomes_four_zero_amount_notes() {
    let channel = Channel::derive(&alice(), bob());
    let set = channel
        .write_message(token(), 0, &offer())
        .expect("valid message");

    assert_eq!(set.actions().len(), NOTES_PER_MESSAGE);
    for action in set.actions() {
        match action {
            ClientAction::CreateEncNote(note) => {
                assert_eq!(note.amount, 0, "data notes must carry no value");
                assert_eq!(note.recipient_addr, bob().address);
                assert_eq!(note.recipient_public_key, bob().public_key);
                assert_eq!(note.token, token());
            }
            other => panic!("expected CreateEncNote, got {other:?}"),
        }
    }
}

#[test]
fn notes_carry_the_wire_salts_in_index_order() {
    let channel = Channel::derive(&alice(), bob());
    let expected = encode_message(&offer()).expect("encodes");
    let set = channel
        .write_message(token(), 3, &offer())
        .expect("valid message");

    for (slot, action) in set.actions().iter().enumerate() {
        let ClientAction::CreateEncNote(note) = action else {
            panic!("expected CreateEncNote");
        };
        assert_eq!(note.salt, expected[slot], "salt mismatch at slot {slot}");
        // Message 3 occupies indices 12..15.
        assert_eq!(
            note.index,
            12 + slot as u32,
            "index mismatch at slot {slot}"
        );
    }
}

#[test]
fn message_indices_do_not_overlap() {
    let channel = Channel::derive(&alice(), bob());
    let first = channel.write_message(token(), 0, &offer()).expect("valid");
    let second = channel.write_message(token(), 1, &offer()).expect("valid");

    let indices = |set: &erebus_sdk::action_set::ActionSet| -> Vec<u32> {
        set.actions()
            .iter()
            .map(|a| match a {
                ClientAction::CreateEncNote(n) => n.index,
                _ => unreachable!(),
            })
            .collect()
    };

    assert_eq!(indices(&first), vec![0, 1, 2, 3]);
    assert_eq!(indices(&second), vec![4, 5, 6, 7]);
}

// --- Keyed reads ----------------------------------------------------------------

#[test]
fn note_ids_match_the_pinned_derivation() {
    let channel = Channel::derive(&alice(), bob());
    let ids = channel.note_ids_for_message(token(), 2);

    for (slot, id) in ids.iter().enumerate() {
        let expected = hashes::compute_note_id(channel.key(), token(), 8 + slot as u64);
        assert_eq!(*id, expected, "note id mismatch at slot {slot}");
    }
}

/// The reader's ids must land on the notes the writer created. If these ever diverge the
/// counterparty finds nothing, with no error anywhere — the silent failure this whole
/// codebase is built to avoid.
#[test]
fn the_reader_and_writer_agree_on_where_notes_live() {
    let channel = Channel::derive(&alice(), bob());
    let message_index = 5;
    let set = channel
        .write_message(token(), message_index, &offer())
        .expect("valid");
    let ids = channel.note_ids_for_message(token(), message_index);

    for (slot, action) in set.actions().iter().enumerate() {
        let ClientAction::CreateEncNote(note) = action else {
            panic!("expected CreateEncNote");
        };
        let written_to = hashes::compute_note_id(channel.key(), note.token, u64::from(note.index));
        assert_eq!(
            written_to, ids[slot],
            "writer and reader disagree at slot {slot}"
        );
    }
}

#[test]
fn different_tokens_give_different_locations() {
    let channel = Channel::derive(&alice(), bob());
    let a = channel.note_ids_for_message(token(), 0);
    let b = channel.note_ids_for_message(Felt::from_hex("0x9999").expect("token"), 0);
    assert_ne!(a, b, "a subchannel is per-token; locations must differ");
}
