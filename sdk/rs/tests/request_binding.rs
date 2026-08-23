//! Canonical request bindings: what must collide, and what must not.
//!
//! A binding decides whether reusing an operation id is a replay or a conflict. A collision
//! between two different requests would let a caller resume the wrong effect under an id it
//! already trusts, so the interesting tests here are the ones that must *differ*.

use erebus_sdk::operation::{RequestBinding, WriteOperation};
use starknet_types_core::felt::Felt;

const CHAIN: Felt = Felt::from_hex_unchecked("0x534e5f5345504f4c4941");
const POOL: Felt = Felt::from_hex_unchecked("0x4e4f");
const TOKEN: Felt = Felt::from_hex_unchecked("0x53545f");

fn shield(amount: u128) -> RequestBinding {
    RequestBinding::builder(WriteOperation::Shield, CHAIN, POOL, TOKEN)
        .u128_be(amount)
        .finish()
}

#[test]
fn the_same_request_binds_the_same_way() {
    assert_eq!(shield(1_000), shield(1_000));
}

#[test]
fn a_different_amount_binds_differently() {
    assert_ne!(shield(1_000), shield(1_001));
}

#[test]
fn the_same_amount_under_a_different_method_binds_differently() {
    // The hazard this catches: `approve_pool(2 STRK)` and `shield(2 STRK)` carry identical
    // parameters. Without the method tag, one operation id would treat them as replays of
    // each other and an approval could resolve as an already-completed deposit.
    let approve = RequestBinding::builder(WriteOperation::ApprovePool, CHAIN, POOL, TOKEN)
        .u128_be(1_000)
        .finish();

    assert_ne!(shield(1_000), approve);
}

#[test]
fn repointing_the_configuration_binds_differently() {
    let other_chain = RequestBinding::builder(WriteOperation::Shield, TOKEN, POOL, TOKEN)
        .u128_be(1_000)
        .finish();
    let other_pool = RequestBinding::builder(WriteOperation::Shield, CHAIN, TOKEN, TOKEN)
        .u128_be(1_000)
        .finish();
    let other_token = RequestBinding::builder(WriteOperation::Shield, CHAIN, POOL, POOL)
        .u128_be(1_000)
        .finish();

    for repointed in [other_chain, other_pool, other_token] {
        assert_ne!(shield(1_000), repointed);
    }
}

#[test]
fn adjacent_text_fields_cannot_be_confused_for_one_another() {
    // Length prefixing is the whole reason `push_text` exists. Concatenating the fields
    // instead would make these two requests bind identically.
    let split_one = RequestBinding::builder(WriteOperation::CounterOffer, CHAIN, POOL, TOKEN)
        .text("ab")
        .text("c")
        .finish();
    let split_two = RequestBinding::builder(WriteOperation::CounterOffer, CHAIN, POOL, TOKEN)
        .text("a")
        .text("bc")
        .finish();

    assert_ne!(split_one, split_two);
}

#[test]
fn a_narrow_and_a_wide_field_of_equal_value_bind_differently() {
    // `u64_be` and `u128_be` write different widths, so a deadline of 7 and an amount of 7
    // cannot alias even though both are the integer 7.
    let narrow = RequestBinding::builder(WriteOperation::ProposeOffer, CHAIN, POOL, TOKEN)
        .u64_be(7)
        .finish();
    let wide = RequestBinding::builder(WriteOperation::ProposeOffer, CHAIN, POOL, TOKEN)
        .u128_be(7)
        .finish();

    assert_ne!(narrow, wide);
}

#[test]
fn field_order_is_part_of_the_binding() {
    let forward = RequestBinding::builder(WriteOperation::ProposeOffer, CHAIN, POOL, TOKEN)
        .u128_be(1)
        .u64_be(2)
        .finish();
    let reversed = RequestBinding::builder(WriteOperation::ProposeOffer, CHAIN, POOL, TOKEN)
        .u64_be(2)
        .u128_be(1)
        .finish();

    assert_ne!(forward, reversed);
}

#[test]
fn every_write_method_has_a_distinct_tag() {
    let operations = [
        WriteOperation::Shield,
        WriteOperation::ApprovePool,
        WriteOperation::OpenChannel,
        WriteOperation::ProposeOffer,
        WriteOperation::CounterOffer,
        WriteOperation::AcceptAndSettle,
    ];

    let mut tags: Vec<&str> = operations.iter().map(|op| op.tag()).collect();
    tags.sort_unstable();
    let count = tags.len();
    tags.dedup();

    assert_eq!(tags.len(), count, "two write methods share a binding tag");
}

#[test]
fn serde_round_trip_preserves_the_binding_and_validates_input() {
    let binding = shield(1_000);
    let json = serde_json::to_string(&binding).expect("binding serializes");

    assert_eq!(json, format!("\"{}\"", binding.to_hex()));
    assert_eq!(
        serde_json::from_str::<RequestBinding>(&json).expect("binding deserializes"),
        binding
    );

    let uppercase = format!("\"{}\"", binding.to_hex().to_uppercase());
    assert!(serde_json::from_str::<RequestBinding>(&uppercase).is_err());
    assert!(serde_json::from_str::<RequestBinding>("\"beef\"").is_err());
}
