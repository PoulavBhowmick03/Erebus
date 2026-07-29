//! Tests for atomic accept-and-settle — P2.1, the operation the design exists for.
//!
//! The property under test is that acceptance and payment cannot be separated. They go
//! into one action set, which becomes one proof, so the chain either applies both or
//! neither. Anything that let them split would reintroduce exactly the failure Erebus
//! claims to remove: a counterparty holding an acceptance and no money.

use erebus_sdk::actions::{ClientAction, RandomSalt};
use erebus_sdk::channel::{
    Acceptance, Channel, ChannelError, Counterparty, OwnedNote, Payment, PoolIdentity,
};
use erebus_sdk::wire::{MessageType, WireMessage};
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

fn token() -> Felt {
    Felt::from_hex("0x7042").expect("token")
}

fn salt() -> RandomSalt {
    RandomSalt::from_entropy([
        0x9a, 0x3f, 0x11, 0x7c, 0x42, 0xd8, 0x05, 0xbe, 0x6e, 0x21, 0xa0, 0x77, 0x13, 0x94,
        0xcc, 0x58,
    ])
}

fn accept_message() -> WireMessage {
    WireMessage {
        message_type: MessageType::Accept,
        reply_to: Some(1),
        created_at: 1_753_699_320,
        amount: 950_000,
        deadline: 1_753_702_800,
        memo_hash: 0xdead_beef,
    }
}

fn inputs() -> Vec<OwnedNote> {
    vec![OwnedNote {
        channel_key: Felt::from_hex("0xc0ffee").expect("incoming channel"),
        token: token(),
        index: 0,
    }]
}

fn settle(channel: &Channel, payment_index: u32, message_index: u32) -> Result<erebus_sdk::action_set::ActionSet, ChannelError> {
    channel.accept_and_settle(
        token(),
        &inputs(),
        Payment { amount: 950_000, index: payment_index, salt: salt() },
        Acceptance { message_index, message: accept_message() },
    )
}

// --- Atomicity ------------------------------------------------------------------

/// The whole point: one action set, so one proof, so both legs or neither.
#[test]
fn acceptance_and_payment_land_in_one_action_set() {
    let channel = Channel::derive(&alice(), bob());
    let set = settle(&channel, 4, 2).expect("valid settlement");

    // 1 spend + 1 payment + 4 acceptance notes.
    assert_eq!(set.actions().len(), 6);

    let spends = set
        .actions()
        .iter()
        .filter(|a| matches!(a, ClientAction::UseNote(_)))
        .count();
    let notes = set
        .actions()
        .iter()
        .filter(|a| matches!(a, ClientAction::CreateEncNote(_)))
        .count();
    assert_eq!(spends, 1, "the input note must be consumed in this set");
    assert_eq!(notes, 5, "payment plus the four-note acceptance record");
}

/// Spends must precede creations, or the contract rejects with ACTIONS_OUT_OF_ORDER after
/// a proof has already been paid for.
#[test]
fn spends_come_before_creations() {
    let channel = Channel::derive(&alice(), bob());
    let set = settle(&channel, 4, 2).expect("valid settlement");

    let first_create = set
        .actions()
        .iter()
        .position(|a| matches!(a, ClientAction::CreateEncNote(_)))
        .expect("a note is created");
    let last_spend = set
        .actions()
        .iter()
        .rposition(|a| matches!(a, ClientAction::UseNote(_)))
        .expect("a note is spent");

    assert!(last_spend < first_create, "a spend followed a creation");
}

#[test]
fn multiple_inputs_are_all_consumed() {
    let channel = Channel::derive(&alice(), bob());
    let many: Vec<OwnedNote> = (0..3)
        .map(|index| OwnedNote {
            channel_key: Felt::from_hex("0xc0ffee").expect("channel"),
            token: token(),
            index,
        })
        .collect();

    let set = channel
        .accept_and_settle(
            token(),
            &many,
            Payment { amount: 1, index: 4, salt: salt() },
            Acceptance { message_index: 2, message: accept_message() },
        )
        .expect("valid");

    assert_eq!(
        set.actions().iter().filter(|a| matches!(a, ClientAction::UseNote(_))).count(),
        3
    );
}

// --- The salt rule --------------------------------------------------------------

/// The payment note must not carry a structured salt. Value notes and data notes take
/// different salt types precisely so this cannot be got wrong by accident.
#[test]
fn the_payment_note_carries_the_random_salt_and_the_record_does_not() {
    let channel = Channel::derive(&alice(), bob());
    let set = settle(&channel, 4, 2).expect("valid settlement");

    let notes: Vec<_> = set
        .actions()
        .iter()
        .filter_map(|a| match a {
            ClientAction::CreateEncNote(n) => Some(n),
            _ => None,
        })
        .collect();

    let payment = notes.iter().find(|n| n.amount > 0).expect("a payment note exists");
    assert_eq!(payment.salt, salt().salt(), "payment must use the supplied random salt");
    assert_eq!(payment.index, 4);

    let records: Vec<_> = notes.iter().filter(|n| n.amount == 0).collect();
    assert_eq!(records.len(), 4, "the acceptance record is four notes");
    for record in records {
        assert_ne!(
            record.salt,
            salt().salt(),
            "a record note must not reuse the payment's salt"
        );
    }
}

#[test]
fn exactly_one_note_carries_value() {
    let channel = Channel::derive(&alice(), bob());
    let set = settle(&channel, 4, 2).expect("valid settlement");
    let valued = set
        .actions()
        .iter()
        .filter(|a| matches!(a, ClientAction::CreateEncNote(n) if n.amount > 0))
        .count();
    assert_eq!(valued, 1);
}

#[test]
fn random_salts_stay_inside_the_contract_bound() {
    // Including the degenerate inputs, which must be nudged rather than rejected.
    for bytes in [[0u8; 16], [0xff; 16], [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]] {
        let salt = RandomSalt::from_entropy(bytes).salt();
        assert!(salt.get() > 1, "salt {} is reserved", salt.get());
        assert!(salt.get() < erebus_sdk::actions::NoteSalt::TWO_POW_120);
    }
}

// --- Rejections -----------------------------------------------------------------

#[test]
fn a_non_acceptance_message_is_rejected() {
    let channel = Channel::derive(&alice(), bob());
    let mut message = accept_message();
    message.message_type = MessageType::Counter;

    let error = channel
        .accept_and_settle(
            token(),
            &inputs(),
            Payment { amount: 1, index: 4, salt: salt() },
            Acceptance { message_index: 2, message },
        )
        .expect_err("a counter is not a settlement record");
    assert!(matches!(error, ChannelError::NotAnAcceptance(MessageType::Counter)));
}

#[test]
fn a_zero_payment_is_rejected() {
    let channel = Channel::derive(&alice(), bob());
    let error = channel
        .accept_and_settle(
            token(),
            &inputs(),
            Payment { amount: 0, index: 4, salt: salt() },
            Acceptance { message_index: 2, message: accept_message() },
        )
        .expect_err("settling nothing is not settling");
    assert!(matches!(error, ChannelError::ZeroPayment));
}

#[test]
fn settling_without_inputs_is_rejected() {
    let channel = Channel::derive(&alice(), bob());
    let error = channel
        .accept_and_settle(
            token(),
            &[],
            Payment { amount: 1, index: 4, salt: salt() },
            Acceptance { message_index: 2, message: accept_message() },
        )
        .expect_err("payment must be funded by a spend");
    assert!(matches!(error, ChannelError::NothingToSpend));
}

/// The payment note and the acceptance record share one subchannel index space, so an
/// overlap would silently overwrite part of the record.
#[test]
fn a_payment_index_inside_the_record_range_is_rejected() {
    let channel = Channel::derive(&alice(), bob());
    // Message 2 occupies 8..11.
    for colliding in 8..12 {
        let error = settle(&channel, colliding, 2)
            .expect_err("payment index {colliding} overlaps the record");
        assert!(
            matches!(error, ChannelError::IndexCollision { .. }),
            "index {colliding} was not caught"
        );
    }
}

#[test]
fn an_index_just_outside_the_record_range_is_allowed() {
    let channel = Channel::derive(&alice(), bob());
    settle(&channel, 7, 2).expect("index 7 is below the 8..11 record");
    settle(&channel, 12, 2).expect("index 12 is above the 8..11 record");
}
