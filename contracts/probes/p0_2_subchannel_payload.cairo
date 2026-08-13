//! can a subchannel note carry an arbitrary structured payload?
//!
//! Answers ARCHITECTURE.md 8: "Can subchannel writes carry arbitrary structured
//! payloads cleanly, or does the SDK force a payment-shaped envelope?"
//!
//! all three PASS against upstream.

//!
//! 1. `probe_offer_terms_does_not_fit_in_a_note`. The ARCHITECTURE §4 `OfferTerms`
//!    struct is 5 field elements; a note's whole client-writable surface is 120 bits.
//! 2. `probe_note_salt_is_a_120_bit_payload_lane`. The note salt IS a real payload
//!    lane: it round-trips verbatim through storage, and a zero-amount note (a pure
//!    data note, no deposit, nothing to settle) is accepted.
//! 3. `probe_note_salt_rejects_a_full_felt_payload`. One bit over 120 and the write
//!    path rejects it at `assert_valid`.

use core::num::traits::Zero;
use core::poseidon::poseidon_hash_span;
use privacy::actions::ClientAction;
use privacy::errors;
use privacy::hashes::compute_note_id;
use privacy::tests::utils_for_tests::{PrivacyCfgTrait, Test, TestTrait, UserTrait};
use privacy::utils::constants::{OPEN_NOTE_SALT, TWO_POW_120};
use privacy::utils::unpack;
use starknet::ContractAddress;

/// `OfferTerms` from Erebus, transcribed to Cairo.
/// This is the struct we want to put into a subchannel.
#[derive(Serde, Copy, Drop, PartialEq, Debug)]
pub struct OfferTerms {
    pub amount: u128,
    pub token: ContractAddress,
    pub deadline: u64,
    pub memo_hash: felt252,
    pub nonce: u64,
}

fn sample_offer() -> OfferTerms {
    OfferTerms {
        amount: 1_000_000,
        token: 0x49d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7
            .try_into()
            .unwrap(),
        deadline: 1_800_000_000,
        memo_hash: poseidon_hash_span(['erebus:offer:memo'].span()),
        nonce: 1,
    }
}

/// sets up sender -> recipient with an open channel and one token subchannel.
/// returns (sender, recipient, token_addr).
fn setup_channel(ref test: Test) -> (
    privacy::tests::utils_for_tests::User, privacy::tests::utils_for_tests::User, ContractAddress,
) {
    let mut sender = test.new_user();
    let mut recipient = test.new_user();
    sender.set_viewing_key_e2e();
    recipient.set_viewing_key_e2e();
    let token_addr = test.mock_new_token();
    sender.open_channel_with_token_e2e(:recipient, :token_addr, outgoing_channel_index: 0);
    (sender, recipient, token_addr)
}

/// CLaude Code:
/// The offer does not fit. `CreateEncNoteInput` has no payload field at all; the only
/// client-chosen value that survives to storage is `salt`, a u128 constrained to
/// (OPEN_NOTE_SALT, 2^120) by `CreateEncNoteInputValid::assert_valid`.
#[test]
fn probe_offer_terms_does_not_fit_in_a_note() {
    let terms = sample_offer();
    let mut serialized: Array<felt252> = array![];
    terms.serialize(ref serialized);

    // Five field elements of offer.
    assert_eq!(serialized.len(), 5);

    // `memo_hash` alone overflows the single 120-bit lane a note gives us.
    let memo: u256 = terms.memo_hash.into();
    assert!(memo >= TWO_POW_120.into());
}

/// The note salt is a genuine 120-bit lane: the sender picks it, the contract writes it
/// verbatim into the high 120 bits of `packed_value`, and the recipient reads it back
/// from `get_note(note_id)`, a keyed read rather than a scan.
///
/// `amount: 0` is deliberate. `CreateEncNoteInputValid` allows a zero amount, and the
/// token balance ledger nets to zero, so this note moves no value and needs no deposit.
/// It is a pure data note. It is also permanently unspendable: `use_note` rejects a
/// zero-amount note with `ZERO_NOTE_AMOUNT_USAGE`, so it occupies its subchannel index
/// forever and can never be nullified.
#[test]
fn probe_note_salt_is_a_120_bit_payload_lane() {
    let mut test: Test = Default::default();
    let (mut sender, recipient, token_addr) = setup_channel(ref test);

    // All we can carry: the offer digest truncated to 120 bits.
    let digest: u256 = poseidon_hash_span(['erebus:offer:v1', sample_offer().memo_hash].span())
        .into();
    let payload: u128 = (digest % TWO_POW_120.into()).try_into().unwrap();
    assert!(payload > OPEN_NOTE_SALT);

    let input = sender.new_enc_note(:recipient, :token_addr, amount: 0, index: 0, salt: payload);

    // The mandated pipeline: simulate locally, then apply. (`apply_actions` cheats the
    // proof facts under snforge; on a real network this leg needs a real proof.)
    let actions = sender.create_enc_note(create_note_input: input);
    test.privacy.apply_actions(:actions);

    let channel_key = sender.compute_channel_key(:recipient);
    let note_id = compute_note_id(:channel_key, token: token_addr, index: 0);
    let stored = test.privacy.get_note(:note_id);
    let (stored_salt, _enc_amount) = unpack(packed_value: stored.packed_value);

    // The salt round-trips verbatim. This is the entire payload capacity of one note.
    assert_eq!(stored_salt, payload);

    // And the note carries nothing else: `token` is left zero for encrypted notes.
    assert!(stored.token.is_zero());
}

/// One bit over the lane and the write path rejects it. There is no wider field.
#[test]
fn probe_note_salt_rejects_a_full_felt_payload() {
    let mut test: Test = Default::default();
    let (sender, recipient, token_addr) = setup_channel(ref test);

    let input = sender
        .new_enc_note(:recipient, :token_addr, amount: 0, index: 0, salt: TWO_POW_120);

    sender
        .assert_actions_panic(
            [ClientAction::CreateEncNote(input)].span(),
            expected_error: errors::SALT_EXCEEDS_120_BITS,
        );
}
