//! Known-answer tests for `INVOKE_TXN_V3` hashing, against starknet.js.
//!
//! Fixture is `fixtures/starknetjs-invoke-v3-txhash.json`, produced by calling
//! `hash.calculateInvokeTransactionHash` in starknet.js v10. The upstream SDK uses this
//! library, so an Erebus signature must produce an invocation that the prover's virtual
//! `__execute__` accepts.
//!
//! The vectors cover structural branches: proof facts present or absent, empty or non-empty
//! paymaster and deployment data, and non-trivial resource bounds to catch a wrong shift
//! width in the packing.

use erebus_sdk::tx::{DataAvailabilityMode, InvokeV3, ResourceBound, ResourceBounds};
use serde::Deserialize;
use starknet_types_core::felt::Felt;

#[derive(Deserialize)]
struct Fixture {
    constants: Constants,
    vectors: Vec<Vector>,
}

#[derive(Deserialize)]
struct Constants {
    invoke_prefix: String,
    l1_gas_name: String,
    l2_gas_name: String,
    l1_data_gas_name: String,
}

#[derive(Deserialize)]
struct Vector {
    name: String,
    sender_address: String,
    version: String,
    compiled_calldata: Vec<String>,
    chain_id: String,
    nonce: String,
    account_deployment_data: Vec<String>,
    nonce_da_mode: u8,
    fee_da_mode: u8,
    resource_bounds: Bounds,
    tip: String,
    paymaster_data: Vec<String>,
    proof_facts: Vec<String>,
    tx_hash: String,
}

#[derive(Deserialize)]
struct Bounds {
    l1_gas: Bound,
    l2_gas: Bound,
    l1_data_gas: Bound,
}

#[derive(Deserialize)]
struct Bound {
    max_amount: String,
    max_price_per_unit: String,
}

fn felt(hex: &str) -> Felt {
    Felt::from_hex(hex).expect("fixture felt parses")
}

fn u64_of(hex: &str) -> u64 {
    u64::from_str_radix(hex.trim_start_matches("0x"), 16).expect("fixture u64 parses")
}

fn u128_of(hex: &str) -> u128 {
    u128::from_str_radix(hex.trim_start_matches("0x"), 16).expect("fixture u128 parses")
}

fn da(mode: u8) -> DataAvailabilityMode {
    match mode {
        0 => DataAvailabilityMode::L1,
        1 => DataAvailabilityMode::L2,
        other => panic!("unknown DA mode {other}"),
    }
}

fn bound(b: &Bound) -> ResourceBound {
    ResourceBound {
        max_amount: u64_of(&b.max_amount),
        max_price_per_unit: u128_of(&b.max_price_per_unit),
    }
}

fn load() -> Fixture {
    serde_json::from_str(include_str!("fixtures/starknetjs-invoke-v3-txhash.json"))
        .expect("fixture parses")
}

fn build(v: &Vector) -> InvokeV3 {
    InvokeV3 {
        sender_address: felt(&v.sender_address),
        calldata: v.compiled_calldata.iter().map(|f| felt(f)).collect(),
        chain_id: felt(&v.chain_id),
        nonce: felt(&v.nonce),
        account_deployment_data: v.account_deployment_data.iter().map(|f| felt(f)).collect(),
        nonce_da_mode: da(v.nonce_da_mode),
        fee_da_mode: da(v.fee_da_mode),
        resource_bounds: ResourceBounds {
            l1_gas: bound(&v.resource_bounds.l1_gas),
            l2_gas: bound(&v.resource_bounds.l2_gas),
            l1_data_gas: bound(&v.resource_bounds.l1_data_gas),
        },
        tip: u64_of(&v.tip),
        paymaster_data: v.paymaster_data.iter().map(|f| felt(f)).collect(),
        proof_facts: v.proof_facts.iter().map(|f| felt(f)).collect(),
    }
}

#[test]
fn tx_hashes_match_starknetjs() {
    let fixture = load();
    assert!(!fixture.vectors.is_empty(), "fixture must not be empty");
    for v in &fixture.vectors {
        assert_eq!(v.version, "0x3", "{}: only v3 is supported", v.name);
        assert_eq!(
            build(v).transaction_hash(),
            felt(&v.tx_hash),
            "tx hash mismatch for {}",
            v.name
        );
    }
}

#[test]
fn proof_facts_are_covered_both_ways() {
    // A fixture that only exercised one branch would pass while the other stayed wrong.
    let fixture = load();
    assert!(
        fixture.vectors.iter().any(|v| v.proof_facts.is_empty()),
        "no vector without proof facts"
    );
    assert!(
        fixture.vectors.iter().any(|v| !v.proof_facts.is_empty()),
        "no vector with proof facts"
    );
}

#[test]
fn proof_facts_change_the_hash() {
    let fixture = load();
    let base = fixture
        .vectors
        .iter()
        .find(|v| v.proof_facts.is_empty())
        .expect("a vector without proof facts");

    let without = build(base);
    let mut with = without.clone();
    with.proof_facts = vec![Felt::ONE];

    assert_ne!(
        without.transaction_hash(),
        with.transaction_hash(),
        "adding proof facts must change the hash, otherwise the proof is not bound to the tx"
    );
}

#[test]
fn short_string_constants_match_starknetjs() {
    // These are hardcoded in tx.rs; if starknet.js ever changes them the fixture moves
    // and this catches it rather than the hashes silently diverging.
    let c = load().constants;
    assert_eq!(felt(&c.invoke_prefix), felt("0x696e766f6b65"));
    assert_eq!(felt(&c.l1_gas_name), felt("0x4c315f474153"));
    assert_eq!(felt(&c.l2_gas_name), felt("0x4c325f474153"));
    assert_eq!(felt(&c.l1_data_gas_name), felt("0x4c315f44415441"));
}

#[test]
fn proof_invocation_bounds_have_zero_prices() {
    // __validate__ asserts NON_ZERO_RESOURCE_PRICE on every resource.
    let b = ResourceBounds::for_proof_invocation();
    assert_eq!(b.l1_gas.max_price_per_unit, 0);
    assert_eq!(b.l2_gas.max_price_per_unit, 0);
    assert_eq!(b.l1_data_gas.max_price_per_unit, 0);
}
