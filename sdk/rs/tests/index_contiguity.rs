//! Tests for note-index allocation — P1.3, contiguity and single-use.
//!
//! The pool enforces two index rules and enforces them *after* the proof:
//! `INDEX_NOT_SEQUENTIAL` if a write leaves a gap (`privacy.cairo:737-746`) and
//! `NON_ZERO_VALUE` if it overwrites (`privacy.cairo:932-946`). Both are ~29 s and a proving
//! fee to learn. Everything here is about catching them at the point of the mistake instead.
//!
//! The property that actually matters is at the bottom: across a whole negotiation, the note
//! indices emitted by every action set form one contiguous run from zero with no repeats.
//! Any weaker check passes on layouts the chain rejects.

use erebus_sdk::actions::{ClientAction, RandomSalt};
use erebus_sdk::channel::{Channel, ChannelError, Counterparty, OwnedNote, PoolIdentity};
use erebus_sdk::subchannel::{IndexError, SubchannelCursor};
use erebus_sdk::wire::{MessageType, WireMessage, NOTES_PER_MESSAGE};
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

fn message(kind: MessageType, amount: u128) -> WireMessage {
    WireMessage {
        message_type: kind,
        reply_to: None,
        created_at: 1_753_699_200,
        amount,
        deadline: 1_753_702_800,
        memo_hash: 0xfeed,
    }
}

fn inputs() -> Vec<OwnedNote> {
    vec![OwnedNote {
        channel_key: Felt::from_hex("0xc0ffee").expect("incoming channel"),
        token: token(),
        index: 0,
    }]
}

/// Every note index a set writes, in emission order.
fn created_indices(set: &erebus_sdk::action_set::ActionSet) -> Vec<u32> {
    set.actions()
        .iter()
        .filter_map(|a| match a {
            ClientAction::CreateEncNote(note) => Some(note.index),
            _ => None,
        })
        .collect()
}

// --- The negative tests ---------------------------------------------------------

/// The task's negative test: skipping a message must fail exactly as loudly as the chain
/// would. A gap at the SDK layer is a revert after a paid-for proof.
#[test]
fn skipping_a_message_is_rejected() {
    let cursor = SubchannelCursor::new();
    // Message 0 occupies notes 0..3, so note 4 is next. Asking for note 8 skips 4..7.
    assert_eq!(
        cursor.check(8).unwrap_err(),
        IndexError::NotSequential { index: 8, next: 0 }
    );
}

#[test]
fn rewriting_a_written_index_is_rejected() {
    let mut cursor = SubchannelCursor::new();
    cursor.reserve_message().expect("first message");

    for written in 0..NOTES_PER_MESSAGE as u32 {
        assert_eq!(
            cursor.check(written).unwrap_err(),
            IndexError::AlreadyWritten { index: written },
            "index {written} was already written and must not be reusable"
        );
    }
    cursor.check(NOTES_PER_MESSAGE as u32).expect("the next index is free");
}

/// A rejected message must not consume indices. If it did, the retry would leave a gap and
/// the subchannel would be permanently unusable.
#[test]
fn a_rejected_message_does_not_burn_indices() {
    let channel = Channel::derive(&alice(), bob());
    let mut cursor = SubchannelCursor::new();

    let mut bad = message(MessageType::Offer, 1);
    bad.created_at = u64::MAX; // rejected by the wire encoder — 40-bit field

    channel
        .write_next_message(token(), &mut cursor, &bad)
        .expect_err("an unencodable message must not be written");
    assert_eq!(cursor.next_index(), 0, "a failed write moved the cursor");

    channel
        .write_next_message(token(), &mut cursor, &message(MessageType::Offer, 1))
        .expect("the subchannel is still usable");
    assert_eq!(cursor.next_index(), NOTES_PER_MESSAGE as u32);
}

// --- Allocation through the channel ---------------------------------------------

#[test]
fn consecutive_messages_take_consecutive_grid_slots() {
    let channel = Channel::derive(&alice(), bob());
    let mut cursor = SubchannelCursor::new();

    for expected in 0..3u32 {
        let (index, set) = channel
            .write_next_message(token(), &mut cursor, &message(MessageType::Offer, 1))
            .expect("valid message");
        assert_eq!(index, expected);

        let first = expected * NOTES_PER_MESSAGE as u32;
        let want: Vec<u32> = (first..first + NOTES_PER_MESSAGE as u32).collect();
        assert_eq!(created_indices(&set), want);
    }
}

/// The reader seeks `4k..4k+3` with no framing search, so a message that starts off the grid
/// silently misframes every message after it. Refusing is the only safe answer — rounding up
/// leaves a gap, rounding down overwrites.
#[test]
fn a_message_cannot_start_off_the_grid() {
    let channel = Channel::derive(&alice(), bob());
    let mut cursor = SubchannelCursor::resume_at(5);

    let error = channel
        .write_next_message(token(), &mut cursor, &message(MessageType::Offer, 1))
        .expect_err("index 5 is not a message boundary");
    assert!(matches!(
        error,
        ChannelError::Index(IndexError::Misaligned { next: 5 })
    ));
}

// --- Settlement layout ----------------------------------------------------------

/// Emission order is load-bearing and it is not obvious why. `compile_actions` runs the set
/// through `compile_and_panic`, and `_client_apply_actions` applies each `WriteOnce` as it
/// walks, so the contiguity check on a note sees notes the *same set* created earlier.
/// Writing the payment before a lower-indexed acceptance record fails against a slot the set
/// was about to fill.
#[test]
fn settlement_emits_its_notes_in_ascending_index_order() {
    let channel = Channel::derive(&alice(), bob());
    let mut cursor = SubchannelCursor::new();
    channel
        .write_next_message(token(), &mut cursor, &message(MessageType::Offer, 1_000))
        .expect("offer");

    let (_, set) = channel
        .settle_next(
            token(),
            &mut cursor,
            &inputs(),
            950,
            salt(),
            &message(MessageType::Accept, 950),
        )
        .expect("valid settlement");

    let indices = created_indices(&set);
    assert!(
        indices.windows(2).all(|w| w[0] < w[1]),
        "creates are not ascending: {indices:?}"
    );
}

#[test]
fn settlement_puts_the_record_on_the_grid_and_the_payment_after_it() {
    let channel = Channel::derive(&alice(), bob());
    let mut cursor = SubchannelCursor::new();
    channel
        .write_next_message(token(), &mut cursor, &message(MessageType::Offer, 1_000))
        .expect("offer");

    let (message_index, set) = channel
        .settle_next(
            token(),
            &mut cursor,
            &inputs(),
            950,
            salt(),
            &message(MessageType::Accept, 950),
        )
        .expect("valid settlement");

    assert_eq!(message_index, 1, "the acceptance is message 1");
    // Record at 4..7 on the grid, payment at 8 directly after.
    assert_eq!(created_indices(&set), vec![4, 5, 6, 7, 8]);
}

/// The payment note is one index wide, so settling leaves the cursor at `4k+1`. A second
/// negotiation in the same subchannel therefore cannot start. Pinned here because it is a
/// real constraint on multi-deal subchannels, not because it is desirable.
#[test]
fn settling_ends_the_subchannel_for_further_messages() {
    let channel = Channel::derive(&alice(), bob());
    let mut cursor = SubchannelCursor::new();
    channel
        .settle_next(
            token(),
            &mut cursor,
            &inputs(),
            950,
            salt(),
            &message(MessageType::Accept, 950),
        )
        .expect("valid settlement");

    assert_eq!(cursor.next_index(), NOTES_PER_MESSAGE as u32 + 1);
    let error = channel
        .write_next_message(token(), &mut cursor, &message(MessageType::Offer, 1))
        .expect_err("the cursor is off the grid after settlement");
    assert!(matches!(
        error,
        ChannelError::Index(IndexError::Misaligned { .. })
    ));
}

// --- The property that matters --------------------------------------------------

/// A full negotiation writes one contiguous run of note indices from zero, no gaps and no
/// repeats. This is the whole contract obligation in one assertion; the tests above are
/// diagnostics for when this one fails.
#[test]
fn a_whole_negotiation_writes_one_contiguous_run() {
    let channel = Channel::derive(&alice(), bob());
    let mut cursor = SubchannelCursor::new();
    let mut written: Vec<u32> = Vec::new();

    for round in 0..3u128 {
        let kind = if round == 0 {
            MessageType::Offer
        } else {
            MessageType::Counter
        };
        let (_, set) = channel
            .write_next_message(token(), &mut cursor, &message(kind, 1_000 - round))
            .expect("valid message");
        written.extend(created_indices(&set));
    }

    let (_, set) = channel
        .settle_next(
            token(),
            &mut cursor,
            &inputs(),
            900,
            salt(),
            &message(MessageType::Accept, 900),
        )
        .expect("valid settlement");
    written.extend(created_indices(&set));

    let expected: Vec<u32> = (0..written.len() as u32).collect();
    assert_eq!(
        written, expected,
        "note indices are not a contiguous run from zero"
    );
    assert_eq!(
        written.len() as u32,
        cursor.next_index(),
        "the cursor disagrees with what was actually written"
    );
}
