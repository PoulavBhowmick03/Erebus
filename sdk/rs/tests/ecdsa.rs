//! Known-answer tests for Stark ECDSA signing, against starknet.js.
//!
//! Fixture: `fixtures/starknetjs-ecdsa.json`.
//!
//! The protocol requires valid signatures. A separate byte-equality check pins RFC-6979
//! derivation. If the two libraries diverge, `signatures_verify_under_starknetjs_public_keys`
//! must still pass, and
//! only `signatures_match_starknetjs_byte_for_byte` should fail. That failure would be
//! informative, not fatal.

use erebus_sdk::signing::{public_key, sign, verify};
use serde::Deserialize;
use starknet_crypto::Signature;
use starknet_types_core::felt::Felt;

#[derive(Deserialize)]
struct Fixture {
    vectors: Vec<Vector>,
}

#[derive(Deserialize)]
struct Vector {
    private_key: String,
    public_key: String,
    message_hash: String,
    r: String,
    s: String,
}

fn felt(hex: &str) -> Felt {
    Felt::from_hex(hex).expect("fixture felt parses")
}

fn load() -> Fixture {
    serde_json::from_str(include_str!("fixtures/starknetjs-ecdsa.json")).expect("fixture parses")
}

#[test]
fn public_keys_match_starknetjs() {
    for v in &load().vectors {
        assert_eq!(
            public_key(&felt(&v.private_key)),
            felt(&v.public_key),
            "public key mismatch for {}",
            v.private_key
        );
    }
}

/// The claim the protocol needs: our signature is acceptable under the account's key.
#[test]
fn our_signatures_verify_under_the_matching_public_key() {
    for v in &load().vectors {
        let sk = felt(&v.private_key);
        let hash = felt(&v.message_hash);
        let sig = sign(&sk, &hash).expect("signing succeeds");
        assert!(
            verify(&felt(&v.public_key), &hash, &sig).expect("verification runs"),
            "our signature did not verify for hash {}",
            v.message_hash
        );
    }
}

/// The converse direction: signatures produced by starknet.js verify under our code, so
/// we can validate what a TypeScript peer produced.
#[test]
fn starknetjs_signatures_verify_under_our_code() {
    for v in &load().vectors {
        let sig = Signature {
            r: felt(&v.r),
            s: felt(&v.s),
        };
        assert!(
            verify(&felt(&v.public_key), &felt(&v.message_hash), &sig).expect("verification runs"),
            "starknet.js signature rejected for hash {}",
            v.message_hash
        );
    }
}

/// Stronger than required, but a much sharper ratchet if it holds: identical RFC-6979
/// derivation means identical bytes.
#[test]
fn signatures_match_starknetjs_byte_for_byte() {
    for v in &load().vectors {
        let sig = sign(&felt(&v.private_key), &felt(&v.message_hash)).expect("signing succeeds");
        assert_eq!(sig.r, felt(&v.r), "r mismatch for hash {}", v.message_hash);
        assert_eq!(sig.s, felt(&v.s), "s mismatch for hash {}", v.message_hash);
    }
}

/// Determinism is a property of RFC 6979, not an accident. If this ever fails, an RNG has
/// crept into the signing path.
#[test]
fn signing_is_deterministic() {
    let sk = felt("0x2a");
    let hash = felt("0x1");
    let first = sign(&sk, &hash).expect("signing succeeds");
    let second = sign(&sk, &hash).expect("signing succeeds");
    assert_eq!(first.r, second.r);
    assert_eq!(first.s, second.s);
}

#[test]
fn a_wrong_public_key_rejects_the_signature() {
    let sk = felt("0x2a");
    let hash = felt("0x1");
    let sig = sign(&sk, &hash).expect("signing succeeds");
    let wrong = public_key(&felt("0x2b"));
    assert!(
        !verify(&wrong, &hash, &sig).expect("verification runs"),
        "signature verified under the wrong key"
    );
}

#[test]
fn a_tampered_message_rejects_the_signature() {
    let sk = felt("0x2a");
    let hash = felt("0x1");
    let sig = sign(&sk, &hash).expect("signing succeeds");
    assert!(
        !verify(&public_key(&sk), &felt("0x2"), &sig).expect("verification runs"),
        "signature verified over a different message"
    );
}
