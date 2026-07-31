//! Stark ECDSA signing over a transaction hash.
//!
//! `assert_valid_signature` (`utils.cairo:383`) calls `is_valid_signature` on the user's
//! account contract. It tries custom validation, the transaction hash, and the SNIP-12
//! `CallSet` hash. The signature must satisfy that account's rules.
//!
//! `tests/ecdsa.rs` also requires byte-for-byte agreement with starknet.js. This pins the
//! RFC-6979 deterministic-`k` derivation, although the protocol only requires validity.

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
    /// No valid `k` was found before the retry limit.
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
/// RFC 6979 derives the same signature from the same key and hash without an RNG. This makes
/// known-answer tests reproducible and avoids random-`k` nonce reuse.
///
/// If `k` produces an invalid signature, RFC 6979 derives another value with an incremented
/// seed.
pub fn sign(private_key: &Felt, message_hash: &Felt) -> Result<Signature, SigningError> {
    let mut seed: Option<Felt> = None;
    for _ in 0..MAX_K_ATTEMPTS {
        let k = rfc6979_generate_k(message_hash, private_key, seed.as_ref());
        match ecdsa_sign(private_key, message_hash, &k) {
            Ok(extended) => {
                return Ok(Signature {
                    r: extended.r,
                    s: extended.s,
                });
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

/// Stark public key for an account signing key. This differs from the pool
/// `user_private_key` in proof calldata, so a hostile prover cannot spend. See friction.md
/// F14.
pub fn public_key(private_key: &Felt) -> Felt {
    get_public_key(private_key)
}
