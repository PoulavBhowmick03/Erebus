//! Tests for the read path: P1.3 counterparty read, and the basis of P2.2.
//!
//! The strongest test available offline is the round trip: write a message with the real
//! writer, store it exactly where the writer says, and read it back with the reader. If the
//! writer's slot derivation and the reader's diverge, the note appears absent without an
//! error. `writer_and_reader_agree_on_slots` checks this boundary directly.

use std::collections::HashMap;

use erebus_sdk::actions::{ClientAction, RandomSalt};
use erebus_sdk::channel::{
    ChangeOutput, Channel, ChannelError, Counterparty, OwnedNote, PoolIdentity,
};
use erebus_sdk::negotiation::{Author, OfferId};
use erebus_sdk::read::{reconstruct, ChannelReader, ReadError};
use erebus_sdk::subchannel::SubchannelCursor;
use erebus_sdk::wire::{
    encode_legacy_message, MessageType, WireError, WireMessage, WireVersion, NOTES_PER_MESSAGE,
};
use starknet_types_core::felt::Felt;

/// A stand-in for chain storage: note id -> packed value.
#[derive(Default)]
struct Storage(HashMap<Felt, Felt>);

impl Storage {
    /// Applies each `CreateEncNote` to a pool storage slot.
    fn apply(&mut self, channel: &Channel, set: &erebus_sdk::action_set::ActionSet) {
        for action in set.actions() {
            if let ClientAction::CreateEncNote(note) = action {
                if note.recipient_addr != channel.counterparty().address {
                    continue;
                }
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

fn pool() -> Felt {
    Felt::from_hex("0x9001").expect("pool")
}

fn chain() -> Felt {
    Felt::from_hex("0x534e5f5345504f4c4941").expect("chain")
}

fn message(kind: MessageType, reply_to: Option<u32>, amount: u128, at: u64) -> WireMessage {
    WireMessage {
        deal_id: 0,
        message_type: kind,
        reply_to,
        created_at: at,
        amount,
        deadline: at + 3_600,
        memo_hash: 0xc0de,
    }
}

fn deal_message(
    deal_id: u64,
    kind: MessageType,
    reply_to: Option<u32>,
    amount: u128,
    at: u64,
) -> WireMessage {
    WireMessage {
        deal_id,
        ..message(kind, reply_to, amount, at)
    }
}

// --- The slot agreement ---------------------------------------------------------

/// If writer and reader note ids diverge, the writer uses a slot that the reader never
/// checks. Reads then report no message without an error.
#[test]
fn writer_and_reader_agree_on_slots() {
    let channel = Channel::derive(chain(), pool(), &alice(), bob());
    let reader = ChannelReader::new(chain(), pool(), channel.key(), token());

    for message_index in 0..4 {
        assert_eq!(
            channel
                .note_ids_for_message(token(), message_index)
                .to_vec(),
            reader.note_ids(message_index),
            "writer and reader disagree at message {message_index}"
        );
    }
}

// --- Round trips ----------------------------------------------------------------

#[test]
fn a_written_message_reads_back_identically() {
    let channel = Channel::derive(chain(), pool(), &alice(), bob());
    let mut cursor = SubchannelCursor::new();
    let mut storage = Storage::default();

    let sent = message(MessageType::Offer, None, 1_000_000, 1_753_699_200);
    let (index, set) = channel
        .write_next_message(token(), &mut cursor, &sent)
        .expect("valid message");
    storage.apply(&channel, &set);

    let reader = ChannelReader::new(chain(), pool(), channel.key(), token());
    let read = reader
        .message(index, &storage.source())
        .expect("read succeeds")
        .expect("the message is there");

    assert_eq!(read, sent);
}

#[test]
fn a_legacy_four_note_transcript_is_readable_but_not_writable() {
    let key = Felt::from_hex("0xc4a11e").expect("channel key");
    let channel = Channel::from_key_with_version(chain(), pool(), key, bob(), WireVersion::V1);
    let original = message(MessageType::Offer, None, 42, 1_753_699_200);
    let salts = encode_legacy_message(&original).expect("legacy encoding");
    let mut storage = Storage::default();

    for (index, salt) in salts.iter().enumerate() {
        let index = index as u64;
        let note_id = erebus_sdk::hashes::compute_note_id(key, token(), index);
        let packed = Felt::from(salt.get()) * two_pow_128()
            + Felt::from(encrypted_amount(0, salt.get(), key, token(), index));
        storage.0.insert(note_id, packed);
    }

    let reader = ChannelReader::with_version(chain(), pool(), key, token(), WireVersion::V1);
    assert_eq!(
        reader.message(0, &storage.source()).expect("legacy read"),
        Some(original)
    );
    assert!(matches!(
        channel.write_message(token(), 1, &original),
        Err(ChannelError::Wire(WireError::LegacyReadOnly))
    ));
}

#[test]
fn a_whole_transcript_reads_back_in_order() {
    let channel = Channel::derive(chain(), pool(), &alice(), bob());
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

    let reader = ChannelReader::new(chain(), pool(), channel.key(), token());
    let transcript = reader.transcript(&storage.source()).expect("reads");

    assert_eq!(transcript.len(), 3);
    for (slot, read) in transcript.iter().enumerate() {
        assert_eq!(read.message_index, slot as u32 * NOTES_PER_MESSAGE as u32);
        assert_eq!(read.message, sent[slot]);
    }
}

/// An empty subchannel returns no message instead of an error.
#[test]
fn an_empty_subchannel_reads_as_no_messages() {
    let channel = Channel::derive(chain(), pool(), &alice(), bob());
    let reader = ChannelReader::new(chain(), pool(), channel.key(), token());
    let storage = Storage::default();

    assert!(reader
        .transcript(&storage.source())
        .expect("reads")
        .is_empty());
    assert_eq!(reader.message(0, &storage.source()).expect("reads"), None);
}

// --- Reading with the wrong key -------------------------------------------------

/// Nothing in the pool distinguishes "this channel is empty" from "you have the wrong key".
/// Both look like an empty subchannel, because the wrong key derives slots nobody wrote to.
#[test]
fn the_wrong_channel_key_reads_as_an_empty_channel() {
    let channel = Channel::derive(chain(), pool(), &alice(), bob());
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

    let wrong = ChannelReader::new(
        chain(),
        pool(),
        Felt::from_hex("0xbadbad").expect("felt"),
        token(),
    );
    assert!(
        wrong
            .transcript(&storage.source())
            .expect("reads")
            .is_empty(),
        "a wrong key must not find anything, and must not error either"
    );
}

/// A torn message cannot happen on-chain, because contiguity forbids gaps. If the source
/// produces one, it is the source that is wrong, and saying so beats decoding three notes.
#[test]
fn a_partial_message_is_an_error_not_a_silent_truncation() {
    let channel = Channel::derive(chain(), pool(), &alice(), bob());
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

    let error = ChannelReader::new(chain(), pool(), channel.key(), token())
        .message(index, &storage.source())
        .expect_err("a torn message must not decode");
    assert!(matches!(
        error,
        ReadError::PartialMessage {
            found: 4,
            expected: 5,
            message_index: 0,
        }
    ));
}

// --- Settlement -----------------------------------------------------------------

#[test]
fn the_settlement_payment_note_is_found_and_decrypts() {
    let channel = Channel::derive(chain(), pool(), &alice(), bob());
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
        0x9a, 0x3f, 0x11, 0x7c, 0x42, 0xd8, 0x05, 0xbe, 0x6e, 0x21, 0xa0, 0x77, 0x13, 0x94, 0xcc,
        0x58,
    ]);
    let inputs = vec![OwnedNote {
        channel_key: Felt::from_hex("0xc0ffee").expect("incoming"),
        token: token(),
        index: 0,
    }];
    let (acceptance_index, set) = channel
        .settle_next_with_change(
            token(),
            &mut cursor,
            &inputs,
            950,
            salt,
            &message(MessageType::Accept, Some(0), 950, 1_753_699_320),
            Some(ChangeOutput::existing(
                Channel::derive(chain(), pool(), &alice(), alice_as_counterparty()),
                0,
                0,
                RandomSalt::from_entropy([
                    0x31, 0x7a, 0xc4, 0x0d, 0x91, 0xee, 0x62, 0x58, 0xa3, 0x16, 0xb9, 0x44, 0x73,
                    0x20, 0xd5, 0x8f,
                ]),
            )),
        )
        .expect("valid settlement");
    storage.apply(&channel, &set);

    let reader = ChannelReader::new(chain(), pool(), channel.key(), token());
    let payment = reader
        .settlement_note(acceptance_index, &storage.source())
        .expect("the payment note is right after the record");

    assert_eq!(payment.amount, 950, "the paid amount must decrypt exactly");
    assert!(payment.is_value_note());
}

// --- Both directions ------------------------------------------------------------

#[test]
fn two_deals_settle_through_the_same_directional_channel_pair() {
    let a_to_b = Channel::derive(chain(), pool(), &alice(), bob());
    let b_to_a = Channel::derive(chain(), pool(), &bob_identity(), alice_as_counterparty());
    let self_channel = Channel::derive(chain(), pool(), &alice(), alice_as_counterparty());
    let mut storage = Storage::default();
    let (mut a_cursor, mut b_cursor) = (SubchannelCursor::new(), SubchannelCursor::new());
    let payment_salt = RandomSalt::from_entropy([
        0x9a, 0x3f, 0x11, 0x7c, 0x42, 0xd8, 0x05, 0xbe, 0x6e, 0x21, 0xa0, 0x77, 0x13, 0x94, 0xcc,
        0x58,
    ]);
    let change_salt = RandomSalt::from_entropy([
        0x31, 0x7a, 0xc4, 0x0d, 0x91, 0xee, 0x62, 0x58, 0xa3, 0x16, 0xb9, 0x44, 0x73, 0x20, 0xd5,
        0x8f,
    ]);

    for (round, deal_id) in [7u64, 8].into_iter().enumerate() {
        let at = 1_753_699_200 + round as u64 * 120;
        let (offer_start, offer_set) = b_to_a
            .write_next_message(
                token(),
                &mut b_cursor,
                &deal_message(deal_id, MessageType::Offer, None, 900, at),
            )
            .expect("offer frame");
        storage.apply(&b_to_a, &offer_set);

        let (_, settlement_set) = a_to_b
            .settle_next_with_change(
                token(),
                &mut a_cursor,
                &[OwnedNote {
                    channel_key: self_channel.key(),
                    token: token(),
                    index: round as u32,
                }],
                900,
                payment_salt,
                &deal_message(
                    deal_id,
                    MessageType::Accept,
                    Some(offer_start),
                    900,
                    at + 60,
                ),
                Some(ChangeOutput::existing(
                    self_channel,
                    0,
                    round as u32,
                    change_salt,
                )),
            )
            .expect("settlement frame");
        storage.apply(&a_to_b, &settlement_set);
    }

    let book = reconstruct(
        &ChannelReader::new(chain(), pool(), a_to_b.key(), token()),
        &ChannelReader::new(chain(), pool(), b_to_a.key(), token()),
        &storage.source(),
    )
    .expect("both deals reconstruct");

    assert_eq!(book.len(), 4);
    assert_eq!(
        book.status(OfferId::new(Author::Counterparty, 0), u64::MAX),
        Some(erebus_sdk::negotiation::OfferStatus::Settled)
    );
    assert_eq!(
        book.status(OfferId::new(Author::Counterparty, 5), u64::MAX),
        Some(erebus_sdk::negotiation::OfferStatus::Settled)
    );
}

/// A negotiation is two directional channels. Neither alone is the conversation, and the
/// note indices are per-direction, so ordering has to come from `created_at`.
#[test]
fn both_directions_reconstruct_into_one_ordered_book() {
    let a_to_b = Channel::derive(chain(), pool(), &alice(), bob());
    let b_to_a = Channel::derive(chain(), pool(), &bob_identity(), alice_as_counterparty());
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

    let ours = ChannelReader::new(chain(), pool(), a_to_b.key(), token());
    let theirs = ChannelReader::new(chain(), pool(), b_to_a.key(), token());
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
    let a_to_b = Channel::derive(chain(), pool(), &alice(), bob());
    let b_to_a = Channel::derive(chain(), pool(), &bob_identity(), alice_as_counterparty());
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
        &ChannelReader::new(chain(), pool(), a_to_b.key(), token()),
        &ChannelReader::new(chain(), pool(), b_to_a.key(), token()),
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
