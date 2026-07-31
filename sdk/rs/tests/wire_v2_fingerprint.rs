//! Does wire v2 hide *that* a negotiation happened, as well as what it said?
//!
//! Encryption answered the second question (F30). This asks the first, because the observer
//! in `scripts/observer.py` never needed to decode anything to find Erebus traffic — its
//! first filter was a structural property of the salts.

use erebus_sdk::wire::{encode_message, MessageType, WireContext, WireMessage};
use starknet_types_core::felt::Felt;

fn context(index: u32) -> WireContext {
    WireContext {
        chain_id: Felt::from_hex("0x534e5f5345504f4c4941").expect("chain"),
        pool_address: Felt::from_hex("0x254a6b2").expect("pool"),
        token: Felt::from_hex("0x4718f5a").expect("token"),
        channel_key: Felt::from_hex("0xc0ffee").expect("key"),
        message_index: index,
    }
}

fn message(amount: u128) -> WireMessage {
    WireMessage {
        message_type: MessageType::Offer,
        reply_to: None,
        created_at: 1_785_000_000,
        amount,
        deadline: 1_785_600_000,
        memo_hash: 0x1234,
    }
}

#[test]
fn the_fifth_salt_is_structurally_distinguishable_from_a_random_one() {
    let mut fifths = Vec::new();
    for (i, amount) in [1u128, 500, 999_999].into_iter().enumerate() {
        let salts = encode_message(&context(i as u32), &message(amount)).expect("encode");
        fifths.push(salts[4].get());
    }

    // A random 120-bit salt is what every ordinary pool transfer emits. Wire v2's fifth
    // salt is not one: the payload is 536 bits into 595 bits of capacity, so the top 59
    // payload bits of the last note are structurally zero regardless of content.
    for salt in &fifths {
        let top = salt >> 60;
        assert_eq!(
            top,
            1u128 << 59,
            "bits 60..118 should be zero and bit 119 pinned; got {salt:#x}"
        );
    }

    // Ciphertext still varies underneath, so this is a shape leak rather than a content one.
    assert_ne!(fifths[0], fifths[1]);
}

/// The property we want, kept ignored so the fix has a target to turn green.
///
/// Filling the 59 spare bits with random padding makes the fifth salt uniform over its
/// range. An Erebus salt is then distinguishable only by the pinned bit 119, which about
/// half of all ordinary random pool salts share, so messages blend instead of standing out.
#[test]
#[ignore = "wire v2 zero-fills the spare bits; see F31"]
fn every_salt_should_be_indistinguishable_from_a_random_one() {
    let mut tops = Vec::new();
    for index in 0..64u32 {
        let salts = encode_message(&context(index), &message(index as u128)).expect("encode");
        tops.push(salts[4].get() >> 60);
    }
    tops.sort_unstable();
    tops.dedup();
    assert!(
        tops.len() > 1,
        "the fifth salt's high bits are constant across {} messages",
        64
    );
}
