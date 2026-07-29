//! Tests for the read path — P1.3 counterparty read, and the basis of P2.2.
//!
//! The strongest test available offline is the round trip: write a message with the real
//! writer, store it exactly where the writer says, and read it back with the reader. If the
//! writer's slot derivation and the reader's ever diverge, the note is simply "not found"
//! with no error anywhere — which is why `writer_and_reader_agree_on_slots` exists as its
//! own test rather than being implied by the round trip.

use std::collections::HashMap;

use erebus_sdk::actions::{ClientAction, RandomSalt};
use erebus_sdk::channel::{Channel, Counterparty, OwnedNote, PoolIdentity};
use erebus_sdk::negotiation::{Author, OfferId};
use erebus_sdk::read::{reconstruct, ChannelReader, ReadError};
use erebus_sdk::subchannel::SubchannelCursor;
use erebus_sdk::wire::{MessageType, WireMessage, NOTES_PER_MESSAGE};
use starknet_types_core::felt::Felt;

/// A stand-in for chain storage: note id -> packed value.
#[derive(Default)]
struct Storage(HashMap<Felt, Felt>);

impl Storage {
    /// Applies an action set the way the pool would — every `CreateEncNote` becomes a slot.
    fn apply(&mut self, channel: &Channel, set: &erebus_sdk::action_set::ActionSet) {
        for action in set.actions() {
            if let ClientAction::CreateEncNote(note) = action {
                let note_id = erebus_sdk::hashes::compute_note_id(
                    channel.key(),
                    note.token,
                    u64::from(note.index),
                );
                let packed = Felt::from(note.salt.get()) * two_pow_128()
                    + Felt::from(encrypted_amount(
                        note.amount,
                        note.salt.get(),
                        channel.key(),
                        note.token,
                        u64::from(note.index),
                    ));
                self.0.insert(note_id, packed);
            }
        }
    }

    fn source(&self) -> impl erebus_sdk::read::NoteSource + '_ {
        |id: Felt| self.0.get(&id).copied()
    }
}

fn two_pow_128() -> Felt {
    Felt::from(u128::MAX) + Felt::ONE
}

/// The pool's encryption, so the fixture stores what the contract would.
/// `enc = amount + low128(h(ENC_AMOUNT_TAG, channel_key, token, index, salt))`.
fn encrypted_amount(amount: u128, salt: u128, channel_key: Felt, token: Felt, index: u64) -> u128 {
    let hash = erebus_sdk::hashes::compute_enc_amount_hash(channel_key, token, index, salt);
    let digits = hash.to_le_digits();
    let mask = u128::from(digits[0]) | (u128::from(digits[1]) << 64);
    amount.wrapping_add(mask)
}

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

fn bob_identity() -> PoolIdentity {
    PoolIdentity::new(
        Felt::from_hex("0xb0b").expect("addr"),
        Felt::from_hex("0xfeedface").expect("key"),
    )
}

fn alice_as_counterparty() -> Counterparty {
    Counterparty {
        address: alice().address(),
        public_key: alice().public_key(),
    }
}

fn token() -> Felt {
    Felt::from_hex("0x7042").expect("token")
}

fn message(kind: MessageType, reply_to: Option<u32>, amount: u128, at: u64) -> WireMessage {
    WireMessage {
        message_type: kind,
        reply_to,
        created_at: at,
        amount,
        deadline: at + 3_600,
        memo_hash: 0xc0de,
    }
}

// --- The slot agreement ---------------------------------------------------------

/// The failure this guards is invisible. If the writer's note ids and the reader's diverge,
/// nothing errors — the note is written somewhere the reader never looks, and the read
/// returns "no message yet" forever.
#[test]
fn writer_and_reader_agree_on_slots() {
    let channel = Channel::derive(&alice(), bob());
    let reader = ChannelReader::new(channel.key(), token());

    for message_index in 0..4 {
        assert_eq!(
            channel.note_ids_for_message(token(), message_index),
            reader.note_ids(message_index),
            "writer and reader disagree at message {message_index}"
        );
    }
}

// --- Round trips ----------------------------------------------------------------

#[test]
fn a_written_message_reads_back_identically() {
    let channel = Channel::derive(&alice(), bob());
    let mut cursor = SubchannelCursor::new();
    let mut storage = Storage::default();

    let sent = message(MessageType::Offer, None, 1_000_000, 1_753_699_200);
    let (index, set) = channel
        .write_next_message(token(), &mut cursor, &sent)
        .expect("valid message");
    storage.apply(&channel, &set);

    let reader = ChannelReader::new(channel.key(), token());
    let read = reader
        .message(index, &storage.source())
        .expect("read succeeds")
        .expect("the message is there");

    assert_eq!(read, sent);
}

#[test]
fn a_whole_transcript_reads_back_in_order() {
    let channel = Channel::derive(&alice(), bob());
    let mut cursor = SubchannelCursor::new();
    let mut storage = Storage::default();

    let sent: Vec<WireMessage> = (0..3)
        .map(|round| {
            message(
                if round == 0 {
                    MessageType::Offer
                } else {
                    MessageType::Counter
                },
                None,
                1_000_000 - round as u128,
                1_753_699_200 + round * 60,
            )
        })
        .collect();

    for m in &sent {
        let (_, set) = channel
            .write_next_message(token(), &mut cursor, m)
            .expect("valid");
        storage.apply(&channel, &set);
    }

    let reader = ChannelReader::new(channel.key(), token());
    let transcript = reader.transcript(&storage.source()).expect("reads");

    assert_eq!(transcript.len(), 3);
    for (slot, read) in transcript.iter().enumerate() {
        assert_eq!(read.message_index, slot as u32);
        assert_eq!(read.message, sent[slot]);
    }
}

/// An empty subchannel is "nothing yet", not an error — that is the ordinary answer a
/// polling agent gets between rounds.
#[test]
fn an_empty_subchannel_reads_as_no_messages() {
    let channel = Channel::derive(&alice(), bob());
    let reader = ChannelReader::new(channel.key(), token());
    let storage = Storage::default();

    assert!(reader.transcript(&storage.source()).expect("reads").is_empty());
    assert_eq!(reader.message(0, &storage.source()).expect("reads"), None);
}

// --- Reading with the wrong key -------------------------------------------------

/// Nothing in the pool distinguishes "this channel is empty" from "you have the wrong key".
/// Both look like an empty subchannel, because the wrong key derives slots nobody wrote to.
#[test]
fn the_wrong_channel_key_reads_as_an_empty_channel() {
    let channel = Channel::derive(&alice(), bob());
    let mut cursor = SubchannelCursor::new();
    let mut storage = Storage::default();

    let (_, set) = channel
        .write_next_message(
            token(),
            &mut cursor,
            &message(MessageType::Offer, None, 1, 1_753_699_200),
        )
        .expect("valid");
    storage.apply(&channel, &set);

    let wrong = ChannelReader::new(Felt::from_hex("0xbadbad").expect("felt"), token());
    assert!(
        wrong.transcript(&storage.source()).expect("reads").is_empty(),
        "a wrong key must not find anything — and must not error either"
    );
}

/// A torn message cannot happen on-chain, because contiguity forbids gaps. If the source
/// produces one, it is the source that is wrong, and saying so beats decoding three notes.
#[test]
fn a_partial_message_is_an_error_not_a_silent_truncation() {
    let channel = Channel::derive(&alice(), bob());
    let mut cursor = SubchannelCursor::new();
    let mut storage = Storage::default();

    let (index, set) = channel
        .write_next_message(
            token(),
            &mut cursor,
            &message(MessageType::Offer, None, 1, 1_753_699_200),
        )
        .expect("valid");
    storage.apply(&channel, &set);

    // Drop the last note of the message.
    let ids = channel.note_ids_for_message(token(), index);
    storage.0.remove(&ids[NOTES_PER_MESSAGE - 1]);

    let error = ChannelReader::new(channel.key(), token())
        .message(index, &storage.source())
        .expect_err("a torn message must not decode");
    assert!(matches!(
        error,
        ReadError::PartialMessage {
            found: 3,
            message_index: 0
        }
    ));
}

// --- Settlement -----------------------------------------------------------------

#[test]
fn the_settlement_payment_note_is_found_and_decrypts() {
    let channel = Channel::derive(&alice(), bob());
    let mut cursor = SubchannelCursor::new();
    let mut storage = Storage::default();

    let (_, offer) = channel
        .write_next_message(
            token(),
            &mut cursor,
            &message(MessageType::Offer, None, 1_000, 1_753_699_200),
        )
        .expect("valid");
    storage.apply(&channel, &offer);

    let salt = RandomSalt::from_entropy([
        0x9a, 0x3f, 0x11, 0x7c, 0x42, 0xd8, 0x05, 0xbe, 0x6e, 0x21, 0xa0, 0x77, 0x13, 0x94,
        0xcc, 0x58,
    ]);
    let inputs = vec![OwnedNote {
        channel_key: Felt::from_hex("0xc0ffee").expect("incoming"),
        token: token(),
        index: 0,
    }];
    let (acceptance_index, set) = channel
        .settle_next(
            token(),
            &mut cursor,
            &inputs,
            950,
            salt,
            &message(MessageType::Accept, Some(0), 950, 1_753_699_320),
        )
        .expect("valid settlement");
    storage.apply(&channel, &set);

    let reader = ChannelReader::new(channel.key(), token());
    let payment = reader
        .settlement_note(acceptance_index, &storage.source())
        .expect("the payment note is right after the record");

    assert_eq!(payment.amount, 950, "the paid amount must decrypt exactly");
    assert!(payment.is_value_note());
}

// --- Both directions ------------------------------------------------------------

/// A negotiation is two directional channels. Neither alone is the conversation, and the
/// note indices are per-direction, so ordering has to come from `created_at`.
#[test]
fn both_directions_reconstruct_into_one_ordered_book() {
    let a_to_b = Channel::derive(&alice(), bob());
    let b_to_a = Channel::derive(&bob_identity(), alice_as_counterparty());
    let mut storage = Storage::default();
    let (mut a_cursor, mut b_cursor) = (SubchannelCursor::new(), SubchannelCursor::new());

    // A offers at t, B counters at t+60, A accepts at t+120.
    let (_, set) = a_to_b
        .write_next_message(
            token(),
            &mut a_cursor,
            &message(MessageType::Offer, None, 1_000, 1_753_699_200),
        )
        .expect("offer");
    storage.apply(&a_to_b, &set);

    let (_, set) = b_to_a
        .write_next_message(
            token(),
            &mut b_cursor,
            &message(MessageType::Counter, Some(0), 900, 1_753_699_260),
        )
        .expect("counter");
    storage.apply(&b_to_a, &set);

    let ours = ChannelReader::new(a_to_b.key(), token());
    let theirs = ChannelReader::new(b_to_a.key(), token());
    let book = reconstruct(&ours, &theirs, &storage.source()).expect("reconstructs");

    assert_eq!(book.len(), 2);
    // B's counter is the live offer from our side of the table.
    let (id, latest) = book
        .latest_acceptable(1_753_699_300)
        .expect("B's counter is live");
    assert_eq!(
        id,
        OfferId::new(Author::Counterparty, 0),
        "B's counter is message 0 of B's own channel, not ours"
    );
    assert_eq!(latest.amount, 900);
    assert_eq!(latest.message_type, MessageType::Counter);
}

/// Only the counterparty's messages are acceptable, and reconstruction has to preserve who
/// wrote what or that check silently stops working.
#[test]
fn reconstruction_preserves_authorship() {
    let a_to_b = Channel::derive(&alice(), bob());
    let b_to_a = Channel::derive(&bob_identity(), alice_as_counterparty());
    let mut storage = Storage::default();
    let (mut a_cursor, mut b_cursor) = (SubchannelCursor::new(), SubchannelCursor::new());

    let (_, set) = a_to_b
        .write_next_message(
            token(),
            &mut a_cursor,
            &message(MessageType::Offer, None, 1_000, 1_753_699_200),
        )
        .expect("offer");
    storage.apply(&a_to_b, &set);
    let (_, set) = b_to_a
        .write_next_message(
            token(),
            &mut b_cursor,
            &message(MessageType::Counter, Some(0), 900, 1_753_699_260),
        )
        .expect("counter");
    storage.apply(&b_to_a, &set);

    let book = reconstruct(
        &ChannelReader::new(a_to_b.key(), token()),
        &ChannelReader::new(b_to_a.key(), token()),
        &storage.source(),
    )
    .expect("reconstructs");

    // Both messages are index 0. Only authorship tells them apart, and getting that wrong
    // turns "accept their counter" into "accept your own offer" with no error anywhere.
    book.check_acceptable(OfferId::new(Author::Counterparty, 0), 1_753_699_300)
        .expect("their counter is acceptable");
    let error = book
        .check_acceptable(OfferId::new(Author::Us, 0), 1_753_699_300)
        .expect_err("our own offer at the same index is not");
    assert!(matches!(
        error,
        erebus_sdk::negotiation::NegotiationError::OwnOffer { index: 0 }
    ));
}
