//! Stark ECDSA signing over a transaction hash.
//!
//! What the pool actually requires is narrower than it looks. `assert_valid_signature`
//! (`utils.cairo:383`) does not verify a signature itself — it calls
//! `is_valid_signature` on the *user's own account contract*, trying three encodings in
//! turn (custom validation, the tx hash, the SNIP-12 `CallSet` hash). So what has to hold
//! is that the signature verifies under the account's own rules, not that it matches any
//! particular library's bytes.
//!
//! That distinction matters for the KATs: byte-for-byte agreement with starknet.js is an
//! *additional* property, requiring identical RFC-6979 deterministic-`k` derivation.
//! Nothing in the protocol needs it. It is pinned anyway because it is a far sharper
//! ratchet than "some valid signature came out" — see `tests/ecdsa.rs`.

use starknet_crypto::{
    get_public_key, rfc6979_generate_k, sign as ecdsa_sign, verify as ecdsa_verify, Signature,
};
use starknet_types_core::felt::Felt;

/// Errors from signing or verification.
#[derive(Debug, thiserror::Error)]
pub enum SigningError {
    /// The message hash is not a valid field element for signing.
    #[error("invalid message hash")]
    InvalidMessageHash,
    /// No valid `k` was found within the retry budget. Cryptographically implausible;
    /// a bound exists so a bad input cannot spin forever.
    #[error("no valid k after {0} attempts")]
    KExhausted(u32),
    /// The public key is not on the curve.
    #[error("invalid public key")]
    InvalidPublicKey,
}

/// How many `k` candidates to try before giving up.
const MAX_K_ATTEMPTS: u32 = 32;

/// Signs `message_hash` with `private_key`, deriving `k` per RFC 6979.
///
/// RFC 6979 makes signing deterministic: the same key and hash always produce the same
/// signature, with no RNG involved. That is why this is reproducible against a fixture at
/// all — and it is also why a repeated nonce cannot leak the key here the way it can with
/// a badly seeded random `k`.
///
/// On the rare `k` that yields an invalid signature, RFC 6979 says to re-derive with an
/// incremented seed rather than perturbing the key or the hash.
pub fn sign(private_key: &Felt, message_hash: &Felt) -> Result<Signature, SigningError> {
    let mut seed: Option<Felt> = None;
    for _ in 0..MAX_K_ATTEMPTS {
        let k = rfc6979_generate_k(message_hash, private_key, seed.as_ref());
        match ecdsa_sign(private_key, message_hash, &k) {
            Ok(extended) => {
                return Ok(Signature { r: extended.r, s: extended.s });
            }
            Err(_) => {
                seed = Some(match seed {
                    Some(previous) => previous + Felt::ONE,
                    None => Felt::ONE,
                });
            }
        }
    }
    Err(SigningError::KExhausted(MAX_K_ATTEMPTS))
}

/// Verifies `signature` over `message_hash` under `public_key`.
pub fn verify(
    public_key: &Felt,
    message_hash: &Felt,
    signature: &Signature,
) -> Result<bool, SigningError> {
    ecdsa_verify(public_key, message_hash, &signature.r, &signature.s)
        .map_err(|_| SigningError::InvalidPublicKey)
}

/// The Stark public key for a private key. This is the account's *signing* key, distinct
/// from the pool `user_private_key` that rides in the proof invocation's calldata — the
/// separation is what keeps a hostile prover unable to spend. See friction.md F14.
pub fn public_key(private_key: &Felt) -> Felt {
    get_public_key(private_key)
}
