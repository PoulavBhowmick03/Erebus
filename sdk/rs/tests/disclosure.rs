//! Tests for viewing-key disclosure: P2.2.
//!
//! Two claims are under test and they pull in opposite directions. The grant must reveal
//! *enough*: an auditor reconstructs the whole negotiation and can check that what was paid
//! matches what was agreed. And it must reveal *only that*: nothing about the granter's
//! other channels, other counterparties, or other tokens.
//!
//! Leakage tests enforce the grant boundary. Over-disclosure invalidates the compliance
//! claim without causing a protocol failure.

use std::collections::HashMap;

use erebus_sdk::actions::{ClientAction, RandomSalt};
use erebus_sdk::channel::{Channel, Counterparty, OwnedNote, PoolIdentity};
use erebus_sdk::disclosure::{reveal, ViewingGrant};
use erebus_sdk::negotiation::{Author, OfferStatus};
use erebus_sdk::read::NoteSource;
use erebus_sdk::subchannel::SubchannelCursor;
use erebus_sdk::wire::{MessageType, WireMessage, NOTES_PER_MESSAGE};
use starknet_types_core::felt::Felt;

const NOON: u64 = 1_753_699_200;

#[derive(Default)]
struct Storage(HashMap<Felt, Felt>);

impl Storage {
    fn apply(&mut self, channel_key: Felt, set: &erebus_sdk::action_set::ActionSet) {
        for action in set.actions() {
            if let ClientAction::CreateEncNote(note) = action {
                let index = u64::from(note.index);
                let note_id = erebus_sdk::hashes::compute_note_id(channel_key, note.token, index);
                let hash = erebus_sdk::hashes::compute_enc_amount_hash(
                    channel_key,
                    note.token,
                    index,
                    note.salt.get(),
                );
                let d = hash.to_le_digits();
                let mask = u128::from(d[0]) | (u128::from(d[1]) << 64);
                let packed = Felt::from(note.salt.get()) * (Felt::from(u128::MAX) + Felt::ONE)
                    + Felt::from(note.amount.wrapping_add(mask));
                self.0.insert(note_id, packed);
            }
        }
    }

    /// Writes a value note without the SDK consistency checks. This simulates data from a
    /// different or hostile client.
    fn put_value_note(
        &mut self,
        channel_key: Felt,
        token: Felt,
        index: u64,
        amount: u128,
        salt: u128,
    ) {
        let note_id = erebus_sdk::hashes::compute_note_id(channel_key, token, index);
        let hash = erebus_sdk::hashes::compute_enc_amount_hash(channel_key, token, index, salt);
        let d = hash.to_le_digits();
        let mask = u128::from(d[0]) | (u128::from(d[1]) << 64);
        let packed = Felt::from(salt) * (Felt::from(u128::MAX) + Felt::ONE)
            + Felt::from(amount.wrapping_add(mask));
        self.0.insert(note_id, packed);
    }

    fn source(&self) -> impl NoteSource + '_ {
        |id: Felt| self.0.get(&id).copied()
    }
}

fn identity(addr: &str, key: &str) -> PoolIdentity {
    PoolIdentity::new(
        Felt::from_hex(addr).expect("addr"),
        Felt::from_hex(key).expect("key"),
    )
}

fn alice() -> PoolIdentity {
    identity("0xa11ce", "0x1234567890abcdef")
}

fn bob() -> PoolIdentity {
    identity("0xb0b", "0xfeedface")
}

fn carol() -> PoolIdentity {
    identity("0xca401", "0xc0ffeeba6e")
}

fn as_counterparty(id: &PoolIdentity) -> Counterparty {
    Counterparty {
        address: id.address(),
        public_key: id.public_key(),
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

fn other_token() -> Felt {
    Felt::from_hex("0x9999").expect("token")
}

fn message(kind: MessageType, reply_to: Option<u32>, amount: u128, at: u64) -> WireMessage {
    WireMessage {
        message_type: kind,
        reply_to,
        created_at: at,
        amount,
        deadline: at + 3_600,
        memo_hash: 0xa0d17,
    }
}

fn salt() -> RandomSalt {
    RandomSalt::from_entropy([
        0x9a, 0x3f, 0x11, 0x7c, 0x42, 0xd8, 0x05, 0xbe, 0x6e, 0x21, 0xa0, 0x77, 0x13, 0x94, 0xcc,
        0x58,
    ])
}

/// A complete negotiation: A offers 1000, B counters 900, A accepts and pays 900.
/// Returns the storage and the grant A would hand an auditor.
fn settled_negotiation() -> (Storage, ViewingGrant) {
    let a_to_b = Channel::derive(chain(), pool(), &alice(), as_counterparty(&bob()));
    let b_to_a = Channel::derive(chain(), pool(), &bob(), as_counterparty(&alice()));
    let mut storage = Storage::default();
    let (mut a_cursor, mut b_cursor) = (SubchannelCursor::new(), SubchannelCursor::new());

    let (_, set) = a_to_b
        .write_next_message(
            token(),
            &mut a_cursor,
            &message(MessageType::Offer, None, 1_000, NOON),
        )
        .expect("offer");
    storage.apply(a_to_b.key(), &set);

    let (_, set) = b_to_a
        .write_next_message(
            token(),
            &mut b_cursor,
            &message(MessageType::Counter, Some(0), 900, NOON + 60),
        )
        .expect("counter");
    storage.apply(b_to_a.key(), &set);

    let inputs = vec![OwnedNote {
        channel_key: b_to_a.key(),
        token: token(),
        index: 0,
    }];
    let (_, set) = a_to_b
        .settle_next(
            token(),
            &mut a_cursor,
            &inputs,
            900,
            salt(),
            &message(MessageType::Accept, Some(0), 900, NOON + 120),
        )
        .expect("settlement");
    storage.apply(a_to_b.key(), &set);

    let grant = a_to_b.grant_viewing_key(&alice(), b_to_a.key(), token());
    (storage, grant)
}

// --- Reveal enough --------------------------------------------------------------

#[test]
fn the_holder_reconstructs_the_whole_negotiation() {
    let (storage, grant) = settled_negotiation();
    let record = reveal(&grant, &storage.source(), NOON + 200).expect("reveals");

    assert_eq!(record.participants, [alice().address(), bob().address()]);
    assert_eq!(record.token, token());
    assert_eq!(record.messages.len(), 3, "offer, counter, acceptance");

    let kinds: Vec<MessageType> = record
        .messages
        .iter()
        .map(|m| m.message.message_type)
        .collect();
    assert_eq!(
        kinds,
        vec![
            MessageType::Offer,
            MessageType::Counter,
            MessageType::Accept
        ]
    );
}

/// The record has to say *who said what*, or it cannot settle a dispute about who offered
/// which price.
#[test]
fn every_message_is_attributed_to_an_address() {
    let (storage, grant) = settled_negotiation();
    let record = reveal(&grant, &storage.source(), NOON + 200).expect("reveals");

    assert_eq!(
        record.messages[0].author_addr,
        alice().address(),
        "A offered"
    );
    assert_eq!(
        record.messages[1].author_addr,
        bob().address(),
        "B countered"
    );
    assert_eq!(
        record.messages[2].author_addr,
        alice().address(),
        "A accepted"
    );
    assert_eq!(record.messages[1].id.author, Author::Counterparty);
}

/// `agreed_amount` comes from the acceptance. `paid_amount` comes from the payment note.
/// Keeping both lets an auditor compare them.
#[test]
fn record_separates_agreed_and_paid_amounts() {
    let (storage, grant) = settled_negotiation();
    let record = reveal(&grant, &storage.source(), NOON + 200).expect("reveals");

    let settlement = record.settlement.expect("this negotiation settled");
    assert_eq!(settlement.agreed_amount, 900);
    assert_eq!(settlement.paid_amount, Some(900));
    assert_eq!(settlement.is_consistent(), Some(true));
    assert_eq!(
        settlement.accepted_offer,
        Some(erebus_sdk::negotiation::OfferId::new(
            Author::Counterparty,
            0
        )),
        "A accepted B's counter"
    );
}

/// The reason `paid_amount` is read from the note rather than copied from the acceptance.
///
/// The SDK rejects a mismatched record and payment. Another client can still write one, so
/// this test builds the mismatch directly and checks that disclosure reports it.
#[test]
fn disclosure_detects_a_payment_that_disagrees_with_its_acceptance() {
    let a_to_b = Channel::derive(chain(), pool(), &alice(), as_counterparty(&bob()));
    let b_to_a = Channel::derive(chain(), pool(), &bob(), as_counterparty(&alice()));
    let mut storage = Storage::default();
    let mut cursor = SubchannelCursor::new();

    // An acceptance claiming 900...
    let (acceptance_index, set) = a_to_b
        .write_next_message(
            token(),
            &mut cursor,
            &message(MessageType::Accept, None, 900, NOON),
        )
        .expect("acceptance");
    storage.apply(a_to_b.key(), &set);

    // ...next to a payment note carrying 100, written directly the way a hostile client
    // would rather than through `settle_next`, which now refuses to build this.
    let payment_index = (acceptance_index + 1) * NOTES_PER_MESSAGE as u32;
    storage.put_value_note(
        a_to_b.key(),
        token(),
        u64::from(payment_index),
        100,
        salt().salt().get(),
    );

    let grant = a_to_b.grant_viewing_key(&alice(), b_to_a.key(), token());
    let record = reveal(&grant, &storage.source(), NOON + 10).expect("reveals");
    let settlement = record.settlement.expect("settled");

    assert_eq!(settlement.agreed_amount, 900, "what the record claims");
    assert_eq!(settlement.paid_amount, Some(100), "what was actually paid");
    assert_eq!(
        settlement.is_consistent(),
        Some(false),
        "an auditor must be able to see the discrepancy"
    );
}

#[test]
fn an_unsettled_negotiation_discloses_as_unsettled() {
    let a_to_b = Channel::derive(chain(), pool(), &alice(), as_counterparty(&bob()));
    let b_to_a = Channel::derive(chain(), pool(), &bob(), as_counterparty(&alice()));
    let mut storage = Storage::default();
    let mut cursor = SubchannelCursor::new();

    let (_, set) = a_to_b
        .write_next_message(
            token(),
            &mut cursor,
            &message(MessageType::Offer, None, 1_000, NOON),
        )
        .expect("offer");
    storage.apply(a_to_b.key(), &set);

    let grant = a_to_b.grant_viewing_key(&alice(), b_to_a.key(), token());
    let record = reveal(&grant, &storage.source(), NOON + 10).expect("reveals");

    assert!(!record.is_settled());
    assert_eq!(record.messages.len(), 1);
    assert_eq!(record.messages[0].status, OfferStatus::Proposed);
}

/// `now` changes message statuses but does not filter old messages.
#[test]
fn reading_long_after_the_fact_still_discloses_everything() {
    let (storage, grant) = settled_negotiation();
    let a_year_later = NOON + 31_536_000;
    let record = reveal(&grant, &storage.source(), a_year_later).expect("reveals");

    assert_eq!(record.messages.len(), 3, "nothing disappears with time");
    assert!(record.is_settled());
}

// --- Reveal only that -----------------------------------------------------------

/// The core scoping claim. A grant for A↔B says nothing about A↔C, even though both
/// channels belong to the same identity and sit in the same pool.
#[test]
fn a_grant_discloses_nothing_about_another_counterparty() {
    let (mut storage, grant) = settled_negotiation();

    // A also negotiates with Carol, in the same pool, on the same token.
    let a_to_c = Channel::derive(chain(), pool(), &alice(), as_counterparty(&carol()));
    let mut cursor = SubchannelCursor::new();
    let (_, set) = a_to_c
        .write_next_message(
            token(),
            &mut cursor,
            &message(MessageType::Offer, None, 555_555, NOON + 5),
        )
        .expect("offer to carol");
    storage.apply(a_to_c.key(), &set);

    let record = reveal(&grant, &storage.source(), NOON + 200).expect("reveals");

    assert_eq!(record.messages.len(), 3, "Carol's channel leaked in");
    assert!(
        !record.messages.iter().any(|m| m.message.amount == 555_555),
        "the Carol negotiation is visible in a grant that does not cover it"
    );
    assert!(!record.participants.contains(&carol().address()));
}

/// A grant is scoped to one token, because a subchannel *is* a token. The same two parties
/// dealing in a second token is a second disclosure decision.
#[test]
fn a_grant_discloses_nothing_about_another_token() {
    let (mut storage, grant) = settled_negotiation();

    let a_to_b = Channel::derive(chain(), pool(), &alice(), as_counterparty(&bob()));
    let mut cursor = SubchannelCursor::new();
    let (_, set) = a_to_b
        .write_next_message(
            other_token(),
            &mut cursor,
            &message(MessageType::Offer, None, 777_777, NOON + 5),
        )
        .expect("offer on another token");
    storage.apply(a_to_b.key(), &set);

    let record = reveal(&grant, &storage.source(), NOON + 200).expect("reveals");

    assert_eq!(record.token, token());
    assert!(
        !record.messages.iter().any(|m| m.message.amount == 777_777),
        "a second token's subchannel leaked into a grant scoped to the first"
    );
}

/// Half a grant is half a conversation, and it must fail loudly rather than produce a
/// plausible-looking partial record.
///
/// Granting only the direction you derived yourself is an easy mistake. The acceptance
/// replies to a counter that is now invisible, so the transcript no longer hangs together
/// and reconstruction returns an error. It must not return a partial record that appears
/// complete.
#[test]
fn a_half_grant_is_rejected_rather_than_disclosing_a_partial_record() {
    let (storage, _) = settled_negotiation();
    let a_to_b = Channel::derive(chain(), pool(), &alice(), as_counterparty(&bob()));

    let half = a_to_b.grant_viewing_key(
        &alice(),
        Felt::from_hex("0xdead").expect("wrong key"),
        token(),
    );

    let error = reveal(&half, &storage.source(), NOON + 200)
        .expect_err("a grant missing the incoming key cannot produce a coherent record");
    assert!(
        matches!(
            error,
            erebus_sdk::read::ReadError::Negotiation(
                erebus_sdk::negotiation::NegotiationError::DanglingReply { .. }
            )
        ),
        "expected a dangling reply, got {error:?}"
    );
}

/// The grant carries channel keys, never a pool private key. Structural: there is no
/// constructor that takes one and no accessor that returns one, so a grant cannot be
/// escalated into spending authority.
#[test]
fn a_grant_never_carries_spending_authority() {
    let (_, grant) = settled_negotiation();
    let rendered = format!("{grant:?}");

    assert!(rendered.contains("<redacted>"), "keys must not print");
    assert!(
        !rendered.contains("1234567890abcdef"),
        "alice's pool private key appeared in a grant's Debug"
    );
    assert!(rendered.contains("token"));
}

/// Granting is a transfer, so the grant has to survive serialization intact.
#[test]
fn a_grant_round_trips_through_serialization() {
    let (storage, grant) = settled_negotiation();
    let json = serde_json::to_string(&grant).expect("serializes");
    let restored: ViewingGrant = serde_json::from_str(&json).expect("deserializes");

    let original = reveal(&grant, &storage.source(), NOON + 200).expect("reveals");
    let after = reveal(&restored, &storage.source(), NOON + 200).expect("reveals");

    assert_eq!(original.messages.len(), after.messages.len());
    assert_eq!(
        original.settlement.map(|s| s.paid_amount),
        after.settlement.map(|s| s.paid_amount)
    );
}

#[test]
fn a_legacy_v1_grant_without_new_scope_fields_still_loads() {
    let outgoing = Felt::from_hex("0xa0").expect("key");
    let incoming = Felt::from_hex("0xb0").expect("key");
    let granter = alice().address();
    let counterparty = bob().address();
    let checksum = erebus_sdk::hashes::hash(&[
        Felt::from_bytes_be_slice(b"EREBUS_VIEW_GRANT_V1"),
        outgoing,
        incoming,
        token(),
        granter,
        counterparty,
    ]);
    let serialized = serde_json::json!({
        "version": 1,
        "outgoing_key": outgoing,
        "incoming_key": incoming,
        "token": token(),
        "granter": granter,
        "counterparty": counterparty,
        "checksum": checksum,
    });
    let grant: ViewingGrant = serde_json::from_value(serialized).expect("legacy grant loads");
    let empty = |_id: Felt| None;

    let record = reveal(&grant, &empty, NOON).expect("legacy grant validates");
    assert!(record.messages.is_empty());
}

#[test]
fn a_corrupted_serialized_grant_fails_integrity() {
    let (storage, grant) = settled_negotiation();
    let mut json = serde_json::to_value(grant).expect("serializes");
    json["token"] = serde_json::json!("0x1234");
    let corrupted: ViewingGrant = serde_json::from_value(json).expect("shape still parses");

    assert_eq!(
        reveal(&corrupted, &storage.source(), NOON).unwrap_err(),
        erebus_sdk::read::ReadError::InvalidViewingGrant
    );
}
