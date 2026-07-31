//! End-to-end known-answer test: does the Rust build the same signed proof invocation the
//! TypeScript SDK builds?
//!
//! Fixture `fixtures/sdk-proof-invocation.json` comes from the upstream
//! `ProofInvocationFactory` used for prover calls.
//!
//! The other KATs each pin one function against one oracle function. This one pins the
//! *composition*: `__execute__` calldata layout, the v3 hash preimage, and the signature,
//! all at once. Unit tests can all pass while the pieces are wired together wrongly; this
//! is the test that catches that.
//!
//! The fixture also documents F14: `calldata[5]` contains the plaintext pool private key
//! sent to `starknet_proveTransaction`.

use erebus_sdk::signing::{public_key, sign, verify};
use erebus_sdk::tx::{DataAvailabilityMode, InvokeV3, ResourceBound, ResourceBounds};
use serde::Deserialize;
use starknet_crypto::Signature;
use starknet_types_core::felt::Felt;

#[derive(Deserialize)]
struct Fixture {
    account_signing_key: String,
    user_addr: String,
    pool_viewing_key: String,
    pool_address: String,
    chain_id: String,
    nonce: String,
    tip: String,
    resource_bounds: Bounds,
    paymaster_data: Vec<String>,
    account_deployment_data: Vec<String>,
    calldata: Vec<String>,
    signature: Vec<String>,
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

fn load() -> Fixture {
    serde_json::from_str(include_str!("fixtures/sdk-proof-invocation.json"))
        .expect("fixture parses")
}

fn bound(b: &Bound) -> ResourceBound {
    ResourceBound {
        max_amount: u64_of(&b.max_amount),
        max_price_per_unit: u128_of(&b.max_price_per_unit),
    }
}

fn invocation(f: &Fixture) -> InvokeV3 {
    InvokeV3 {
        // The pool is the sender: it is itself an account contract.
        sender_address: felt(&f.pool_address),
        calldata: f.calldata.iter().map(|c| felt(c)).collect(),
        chain_id: felt(&f.chain_id),
        nonce: felt(&f.nonce),
        account_deployment_data: f.account_deployment_data.iter().map(|d| felt(d)).collect(),
        nonce_da_mode: DataAvailabilityMode::L1,
        fee_da_mode: DataAvailabilityMode::L1,
        resource_bounds: ResourceBounds {
            l1_gas: bound(&f.resource_bounds.l1_gas),
            l2_gas: bound(&f.resource_bounds.l2_gas),
            l1_data_gas: bound(&f.resource_bounds.l1_data_gas),
        },
        tip: u64_of(&f.tip),
        paymaster_data: f.paymaster_data.iter().map(|p| felt(p)).collect(),
        // The proof invocation produces proof facts, so its input contains none.
        proof_facts: Vec::new(),
    }
}

/// The whole point: sign the invocation we build, and get the SDK's bytes back.
#[test]
fn we_reproduce_the_sdk_signature_exactly() {
    let f = load();
    let hash = invocation(&f).transaction_hash();
    let ours = sign(&felt(&f.account_signing_key), &hash).expect("signing succeeds");

    assert_eq!(f.signature.len(), 2, "fixture signature is (r, s)");
    assert_eq!(ours.r, felt(&f.signature[0]), "r differs from the SDK");
    assert_eq!(ours.s, felt(&f.signature[1]), "s differs from the SDK");
}

/// Weaker but independent of deterministic-k: the SDK's signature verifies against the
/// hash we computed. If this passes while the test above fails, the hash is right and
/// only the k derivation diverged.
#[test]
fn the_sdk_signature_verifies_against_our_hash() {
    let f = load();
    let hash = invocation(&f).transaction_hash();
    let sdk = Signature {
        r: felt(&f.signature[0]),
        s: felt(&f.signature[1]),
    };
    let pk = public_key(&felt(&f.account_signing_key));

    assert!(
        verify(&pk, &hash, &sdk).expect("verification runs"),
        "the SDK's own signature did not verify against our transaction hash"
    );
}

/// `__execute__` layout: `[array_len, to, selector, inner_len, ...inner]`, where inner is
/// `compile_actions(user_addr, user_private_key, client_actions)`.
#[test]
fn calldata_layout_is_the_execute_wrapper() {
    let f = load();
    let c: Vec<Felt> = f.calldata.iter().map(|x| felt(x)).collect();

    assert_eq!(c[0], Felt::ONE, "exactly one Call is wrapped");
    assert_eq!(c[1], felt(&f.pool_address), "the Call targets the pool");
    assert_eq!(c[3], Felt::THREE, "compile_actions takes three arguments");
    assert_eq!(c[4], felt(&f.user_addr), "arg 0 is user_addr");
    assert_eq!(c[6], Felt::ZERO, "empty action span");
    assert_eq!(c.len(), 7);
}

/// Pins F14 as an executable claim rather than a paragraph: the pool private key is in
/// the clear at index 5 of the payload that goes to the proving service.
#[test]
fn the_pool_private_key_is_in_the_clear_at_index_five() {
    let f = load();
    let c: Vec<Felt> = f.calldata.iter().map(|x| felt(x)).collect();
    let key = felt(&f.pool_viewing_key);

    assert_eq!(c[5], key, "arg 1 of compile_actions is user_private_key");

    let occurrences = c.iter().filter(|x| **x == key).count();
    assert_eq!(occurrences, 1, "the key appears exactly once, at index 5");

    // The signing key never enters this calldata.
    let signing = felt(&f.account_signing_key);
    assert!(
        !c.contains(&signing),
        "the account signing key must never appear in prover-bound calldata"
    );
}
