//! Does wire v2 hide *that* a negotiation happened, as well as what it said?
//!
//! Encryption answered the second question (F30). This asks the first, because the observer
//! in `scripts/observer.py` identifies Erebus traffic from salt structure without decoding.

use erebus_sdk::wire::{
    decode_message, decode_message_v3, encode_message, encode_message_v3, MessageType, WireContext,
    WireMessage,
};
use starknet_types_core::felt::Felt;

// This pins the public observer fixture to the Rust
// implementation without changing the wire or using the channel key in the observer.
#[test]
fn observer_v2_fixture_matches_the_rust_encoder_byte_for_byte() {
    let salts = encode_message(&context(0), &message(500)).expect("encode");
    let expected = [
        0x00dd_aa87_98e0_1766_7cab_e10a_9dab_cfd9u128,
        0x00d1_bf5c_7a72_2b84_da07_250e_c0ef_fc20,
        0x0086_06b1_59b6_205b_7b8c_77bd_bb2b_9efd,
        0x009b_5686_6232_ede3_f950_1f9a_69b1_7188,
        0x0080_0000_0000_0000_002c_2099_c8fc_7578,
    ];
    assert_eq!(salts.map(|salt| salt.get()), expected);
}

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
        deal_id: 0,
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

/// The property we want. Wire v3 delivers it; wire v2 is left as the negative control.
///
/// Masking the spare bits makes the fifth salt uniform over its range. An
/// Erebus salt is then distinguishable only by the pinned bit 119, which about half of all
/// ordinary random pool salts share, so messages blend instead of standing out.
#[test]
fn every_v3_salt_is_indistinguishable_from_a_random_one() {
    let mut tops = Vec::new();
    for index in 0..64u32 {
        let salts = encode_message_v3(&context(index), &message(index as u128)).expect("encode");
        tops.push(salts[4].get() >> 60);
    }
    tops.sort_unstable();
    tops.dedup();
    assert!(
        tops.len() > 1,
        "the fifth salt's high bits are constant across 64 messages"
    );
}

/// The v2 marker occupied salt-4 bits 52..59. Wire v3 has no marker, so this byte varies.
#[test]
fn the_old_marker_lane_is_not_constant_in_v3() {
    let markers: std::collections::BTreeSet<u128> = (0..64u32)
        .map(|index| {
            let salts =
                encode_message_v3(&context(index), &message(index as u128)).expect("encode");
            (salts[4].get() >> 52) & 0xff
        })
        .collect();

    assert!(
        markers.len() > 1,
        "the marker byte is constant across 64 messages, so it classifies Erebus traffic"
    );

    // The v2 control: its marker really is constant, which is what v3 had to fix.
    let v2_markers: std::collections::BTreeSet<u128> = (0..64u32)
        .map(|index| {
            let salts = encode_message(&context(index), &message(index as u128)).expect("encode");
            (salts[4].get() >> 52) & 0xff
        })
        .collect();
    assert_eq!(
        v2_markers.len(),
        1,
        "wire v2's marker should be the constant this test guards against"
    );
}

/// Every bit of every v3 salt below the pinned flag must vary across messages.
///
/// Salt 4 was the one F31 named, but a fixed bit anywhere else would classify traffic just
/// as well. This asserts the whole surface rather than the one slot we already knew about.
#[test]
fn no_v3_salt_has_a_constant_bit_below_the_format_flag() {
    let samples: Vec<[u128; 5]> = (0..128u32)
        .map(|index| {
            let salts =
                encode_message_v3(&context(index), &message(index as u128)).expect("encode");
            [
                salts[0].get(),
                salts[1].get(),
                salts[2].get(),
                salts[3].get(),
                salts[4].get(),
            ]
        })
        .collect();

    for slot in 0..5 {
        for bit in 0..119u32 {
            let ones = samples
                .iter()
                .filter(|salts| salts[slot] >> bit & 1 == 1)
                .count();
            assert!(
                ones > 0 && ones < samples.len(),
                "salt {slot} bit {bit} is constant across {} messages",
                samples.len()
            );
        }
    }
}

/// Encoding is a pure function of context and message, which is why the mask is derived
/// rather than random. A retry after a failed submission must rebuild the identical salts.
#[test]
fn v3_encoding_is_deterministic() {
    let first = encode_message_v3(&context(7), &message(42)).expect("encode");
    let second = encode_message_v3(&context(7), &message(42)).expect("encode");
    assert_eq!(first.map(|s| s.get()), second.map(|s| s.get()));
}

/// The mask is outside the AEAD, so nothing else would notice a flipped spare bit. The
/// decoder recomputes it, which authenticates all three spare bits.
#[test]
fn v3_rejects_a_flipped_spare_bit() {
    use erebus_sdk::actions::NoteSalt;

    let mut salts = encode_message_v3(&context(3), &message(9)).expect("encode");
    assert!(decode_message_v3(&context(3), &salts).is_ok());

    // Salt 4 bits 116..118 are the three masked spare bits.
    salts[4] = NoteSalt::new(salts[4].get() ^ (1u128 << 116)).expect("still in range");

    assert_eq!(
        decode_message_v3(&context(3), &salts),
        Err(erebus_sdk::wire::WireError::InvalidV3Envelope),
        "a flipped spare bit decoded cleanly, so the mask is not verified"
    );
}

/// A v2 reader must not silently accept a v3 message, and the reverse. Versions are recorded
/// per channel; a decoder that guessed would turn corruption into a version change.
#[test]
fn the_two_wires_do_not_decode_each_other() {
    let v3 = encode_message_v3(&context(1), &message(11)).expect("encode");
    let v2 = encode_message(&context(1), &message(11)).expect("encode");

    assert!(decode_message(&context(1), &v3).is_err());
    assert!(decode_message_v3(&context(1), &v2).is_err());
}

/// v3 still round-trips, which the shape tests above would not catch on their own.
#[test]
fn v3_round_trips_every_message_field() {
    let original = message(123_456_789);
    let salts = encode_message_v3(&context(5), &original).expect("encode");
    let decoded = decode_message_v3(&context(5), &salts).expect("decode");
    assert_eq!(decoded, original);
}

/// The mask is scoped like the key: a different channel, token, chain, pool or index must
/// produce a different mask, or two channels would share spare-bit patterns.
#[test]
fn a_different_context_does_not_decode() {
    let salts = encode_message_v3(&context(2), &message(5)).expect("encode");
    assert!(decode_message_v3(&context(3), &salts).is_err());
}
