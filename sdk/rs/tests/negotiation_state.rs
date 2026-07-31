//! Tests for the offer state machine: P1.3.
//!
//! Everything here is enforced *only* here. The pool has no `deadline`, no `status` and no
//! `replyTo`: a settlement against a week-old offer proves and applies exactly as cleanly as
//! a settlement against a live one. The contract does not enforce these rules.

use erebus_sdk::negotiation::{Author, NegotiationError, OfferBook, OfferId, OfferStatus};
use erebus_sdk::wire::{MessageType, WireMessage};

const NOON: u64 = 1_753_699_200;
const ONE_HOUR: u64 = 3_600;

fn theirs(index: u32) -> OfferId {
    OfferId::new(Author::Counterparty, index)
}

fn ours(index: u32) -> OfferId {
    OfferId::new(Author::Us, index)
}

fn message(kind: MessageType, reply_to: Option<u32>, deadline: u64) -> WireMessage {
    WireMessage {
        message_type: kind,
        reply_to,
        created_at: NOON,
        amount: 1_000,
        deadline,
        memo_hash: 0xabc,
    }
}

/// Builds three offers that remain live for one hour.
fn negotiation() -> OfferBook {
    let mut book = OfferBook::new();
    book.record(
        0,
        Author::Counterparty,
        message(MessageType::Offer, None, NOON + ONE_HOUR),
    )
    .expect("offer");
    book.record(
        1,
        Author::Us,
        message(MessageType::Counter, Some(0), NOON + ONE_HOUR),
    )
    .expect("our counter");
    book.record(
        2,
        Author::Counterparty,
        message(MessageType::Counter, Some(1), NOON + ONE_HOUR),
    )
    .expect("their counter");
    book
}

// --- Expiry ---------------------------------------------------------------------

/// The task's requirement, and the one with no backstop anywhere: an expired offer must not
/// be settleable. Nothing on-chain knows what a deadline is.
#[test]
fn an_expired_offer_cannot_be_accepted() {
    let book = negotiation();
    let after = NOON + ONE_HOUR + 1;

    let error = book
        .check_acceptable(theirs(2), after)
        .expect_err("the deadline has passed");
    assert!(matches!(error, NegotiationError::Expired { index: 2, .. }));
    assert_eq!(book.status(theirs(2), after), Some(OfferStatus::Expired));
}

/// The deadline second remains live because expiry uses `now > deadline`.
#[test]
fn the_deadline_second_is_still_live() {
    let book = negotiation();
    book.check_acceptable(theirs(2), NOON + ONE_HOUR)
        .expect("on the deadline is not past it");
    book.check_acceptable(theirs(2), NOON + ONE_HOUR + 1)
        .expect_err("one second later is past it");
}

// --- Who may accept what --------------------------------------------------------

#[test]
fn our_own_offer_is_not_ours_to_accept() {
    let book = negotiation();
    assert!(matches!(
        book.check_acceptable(ours(1), NOON).unwrap_err(),
        NegotiationError::OwnOffer { index: 1 }
    ));
}

#[test]
fn an_unknown_index_is_rejected() {
    let book = negotiation();
    assert!(matches!(
        book.check_acceptable(theirs(99), NOON).unwrap_err(),
        NegotiationError::UnknownOffer { index: 99 }
    ));
}

/// A counter remains acceptable under §4. A counter proposes new terms but does not revoke
/// the earlier offer. The wire has no `withdrawn` state.
#[test]
fn a_countered_offer_is_still_acceptable() {
    let book = negotiation();
    assert_eq!(book.status(theirs(0), NOON), Some(OfferStatus::Countered));
    book.check_acceptable(theirs(0), NOON)
        .expect("countering is a proposal, not a revocation");
}

// --- Settling once --------------------------------------------------------------

#[test]
fn a_settled_channel_cannot_settle_again() {
    let mut book = negotiation();
    book.record(
        3,
        Author::Us,
        message(MessageType::Accept, Some(2), NOON + ONE_HOUR),
    )
    .expect("acceptance");

    assert!(matches!(
        book.check_acceptable(theirs(2), NOON).unwrap_err(),
        NegotiationError::AlreadySettled { index: 3 }
    ));
    assert_eq!(book.status(theirs(2), NOON), Some(OfferStatus::Settled));
    assert_eq!(book.status(ours(3), NOON), Some(OfferStatus::Settled));
}

/// An acceptance is a record, not an offer. Accepting one would be settling against a
/// settlement.
#[test]
fn an_acceptance_is_not_itself_acceptable() {
    let mut book = OfferBook::new();
    book.record(
        0,
        Author::Counterparty,
        message(MessageType::Accept, None, NOON + ONE_HOUR),
    )
    .expect("acceptance");

    assert!(matches!(
        book.check_acceptable(theirs(0), NOON).unwrap_err(),
        NegotiationError::AlreadySettled { .. }
    ));
}

// --- Reply integrity ------------------------------------------------------------

/// Notes are fetched by computed slot, so a reader that miscounts indices gets a message
/// whose `reply_to` points at nothing. Without this it would negotiate against a phantom.
#[test]
fn a_reply_to_a_message_that_does_not_exist_is_rejected() {
    let mut book = OfferBook::new();
    let error = book
        .record(
            1,
            Author::Counterparty,
            message(MessageType::Counter, Some(7), NOON + ONE_HOUR),
        )
        .expect_err("message 7 was never seen");
    assert!(matches!(
        error,
        NegotiationError::DanglingReply {
            index: 1,
            reply_to: 7
        }
    ));
    assert!(book.is_empty(), "a rejected message was still recorded");
}

// --- What a policy engine asks --------------------------------------------------

#[test]
fn the_latest_live_counterparty_offer_is_what_gets_evaluated() {
    let book = negotiation();
    let (id, message) = book.latest_acceptable(NOON).expect("something is live");
    assert_eq!(
        id,
        theirs(2),
        "the newest counterparty message, not ours at 1"
    );
    assert_eq!(message.message_type, MessageType::Counter);
}

#[test]
fn nothing_is_acceptable_once_everything_has_expired() {
    let book = negotiation();
    assert!(book.latest_acceptable(NOON + ONE_HOUR + 1).is_none());
}
