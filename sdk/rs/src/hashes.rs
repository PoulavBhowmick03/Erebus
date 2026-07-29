//! Domain-separated Poseidon hashes, ported from
//! `starkware-libs/starknet-privacy` `packages/privacy/src/hashes.cairo`.
//!
//! Known-answer tests pin every function to vectors from the Cairo contract. The vectors
//! are in `tests/fixtures/cairo-reference-data.json`; `generate_reference_data.cairo`
//! produces them. A wrong preimage does not return an error. It derives an unused storage
//! slot, so the client cannot find the note.
//!
//! ## Salt types differ
//!
//! [`compute_enc_token_hash`] and [`compute_enc_recipient_addr_hash`] take a `Felt`
//! salt. [`compute_enc_amount_hash`] takes a `u128` bounded to 120 bits. These signatures
//! match the contract. The OpenZeppelin audit reported the difference, but it remains.
//! Using one salt type causes silent decryption failures.

use starknet_crypto::poseidon_hash_many;
use starknet_types_core::felt::Felt;

/// Domain-separation tags. Short-string felts, matching `hashes.cairo`.
mod tags {
    use starknet_types_core::felt::Felt;

    /// Encodes an ASCII short string as a felt in Cairo's `'literal'` format.
    ///
    /// Cairo uses big-endian bytes aligned to the right of the field element. Do not use a
    /// `u128`. A felt short string can contain 31 bytes, and most tags exceed 16 bytes.
    /// A `u128` drops the high bytes without an error and produces the wrong storage slot.
    /// The Cairo known-answer tests caught this bug in the first version of this file.
    fn short_string(bytes: &[u8]) -> Felt {
        assert!(
            bytes.len() <= 31,
            "short string exceeds 31 bytes and cannot fit in a felt"
        );
        let mut buf = [0u8; 32];
        buf[32 - bytes.len()..].copy_from_slice(bytes);
        Felt::from_bytes_be(&buf)
    }

    macro_rules! tag {
        ($name:ident, $literal:literal) => {
            /// Domain-separation tag, verbatim from `hashes.cairo`.
            pub fn $name() -> Felt {
                short_string($literal.as_bytes())
            }
        };
    }

    tag!(channel_marker, "CHANNEL_MARKER_TAG:V1");
    tag!(channel_key, "CHANNEL_KEY_TAG:V1");
    tag!(subchannel_marker, "SUBCHANNEL_MARKER_TAG:V1");
    tag!(subchannel_id, "SUBCHANNEL_ID_TAG:V1");
    tag!(nullifier, "NULLIFIER_TAG:V1");
    tag!(enc_channel_key, "ENC_CHANNEL_KEY_TAG:V1");
    tag!(enc_sender_addr, "ENC_SENDER_ADDR_TAG:V1");
    tag!(note_id, "NOTE_ID_TAG:V1");
    tag!(enc_amount, "ENC_AMOUNT_TAG:V1");
    tag!(enc_token, "ENC_TOKEN_TAG:V1");
    tag!(enc_private_key, "ENC_PRIVATE_KEY_TAG:V1");
    tag!(enc_user_addr, "ENC_USER_ADDR_TAG:V1");
    tag!(enc_recipient_addr, "ENC_RECIPIENT_ADDR_TAG:V1");
    tag!(outgoing_channel_id, "OUTGOING_CHANNEL_ID_TAG:V1");
    tag!(identity_key, "IDENTITY_KEY_TAG:V1");
}

/// `poseidon_hash_span` over the given elements.
pub fn hash(elements: &[Felt]) -> Felt {
    poseidon_hash_many(elements)
}

/// `channel_key = h(CHANNEL_KEY_TAG, sender_addr, sender_private_key, recipient_addr,
/// recipient_public_key)`
///
/// This is not a symmetric ECDH secret. It includes the sender's private key, so only the
/// sender can derive it. The sender gives it to the recipient in `EncChannelInfo`, encrypted
/// with ephemeral ECDH. Each channel is directional.
pub fn compute_channel_key(
    sender_addr: Felt,
    sender_private_key: Felt,
    recipient_addr: Felt,
    recipient_public_key: Felt,
) -> Felt {
    hash(&[
        tags::channel_key(),
        sender_addr,
        sender_private_key,
        recipient_addr,
        recipient_public_key,
    ])
}

/// `channel_marker = h(CHANNEL_MARKER_TAG, channel_key, sender_addr, recipient_addr,
/// recipient_public_key)`
pub fn compute_channel_marker(
    channel_key: Felt,
    sender_addr: Felt,
    recipient_addr: Felt,
    recipient_public_key: Felt,
) -> Felt {
    hash(&[
        tags::channel_marker(),
        channel_key,
        sender_addr,
        recipient_addr,
        recipient_public_key,
    ])
}

/// `subchannel_id = h(SUBCHANNEL_ID_TAG, channel_key, index, 0)`
///
/// The trailing zero is a reserved placeholder the contract keeps for forward
/// compatibility. It must be present.
pub fn compute_subchannel_id(channel_key: Felt, index: u64) -> Felt {
    hash(&[
        tags::subchannel_id(),
        channel_key,
        Felt::from(index),
        Felt::ZERO,
    ])
}

/// `subchannel_marker = h(SUBCHANNEL_MARKER_TAG, channel_key, recipient_addr,
/// recipient_public_key, token)`
pub fn compute_subchannel_marker(
    channel_key: Felt,
    recipient_addr: Felt,
    recipient_public_key: Felt,
    token: Felt,
) -> Felt {
    hash(&[
        tags::subchannel_marker(),
        channel_key,
        recipient_addr,
        recipient_public_key,
        token,
    ])
}

/// `note_id = h(NOTE_ID_TAG, channel_key, token, index, 0)`
pub fn compute_note_id(channel_key: Felt, token: Felt, index: u64) -> Felt {
    hash(&[
        tags::note_id(),
        channel_key,
        token,
        Felt::from(index),
        Felt::ZERO,
    ])
}

/// `nullifier = h(NULLIFIER_TAG, channel_key, token, index, 0, owner_private_key)`
pub fn compute_nullifier(
    channel_key: Felt,
    token: Felt,
    index: u64,
    owner_private_key: Felt,
) -> Felt {
    hash(&[
        tags::nullifier(),
        channel_key,
        token,
        Felt::from(index),
        Felt::ZERO,
        owner_private_key,
    ])
}

/// `outgoing_channel_id = h(OUTGOING_CHANNEL_ID_TAG, sender_addr, sender_private_key,
/// index, 0)`
pub fn compute_outgoing_channel_id(
    sender_addr: Felt,
    sender_private_key: Felt,
    index: u64,
) -> Felt {
    hash(&[
        tags::outgoing_channel_id(),
        sender_addr,
        sender_private_key,
        Felt::from(index),
        Felt::ZERO,
    ])
}

/// `h(ENC_AMOUNT_TAG, channel_key, token, index, 0, salt)`
///
/// Salt is `u128` here, and the contract additionally requires `1 < salt < 2^120`. See
/// the module note on non-uniform salt types.
pub fn compute_enc_amount_hash(channel_key: Felt, token: Felt, index: u64, salt: u128) -> Felt {
    hash(&[
        tags::enc_amount(),
        channel_key,
        token,
        Felt::from(index),
        Felt::ZERO,
        Felt::from(salt),
    ])
}

/// `h(ENC_TOKEN_TAG, channel_key, index, 0, salt)`
///
/// Salt is a full `Felt` here, unlike [`compute_enc_amount_hash`].
pub fn compute_enc_token_hash(channel_key: Felt, index: u64, salt: Felt) -> Felt {
    hash(&[
        tags::enc_token(),
        channel_key,
        Felt::from(index),
        Felt::ZERO,
        salt,
    ])
}

/// `h(ENC_RECIPIENT_ADDR_TAG, sender_addr, sender_private_key, index, 0, salt)`
pub fn compute_enc_recipient_addr_hash(
    sender_addr: Felt,
    sender_private_key: Felt,
    index: u64,
    salt: Felt,
) -> Felt {
    hash(&[
        tags::enc_recipient_addr(),
        sender_addr,
        sender_private_key,
        Felt::from(index),
        Felt::ZERO,
        salt,
    ])
}

/// `h(ENC_CHANNEL_KEY_TAG, shared_x)`
pub fn compute_enc_channel_key_hash(shared_x: Felt) -> Felt {
    hash(&[tags::enc_channel_key(), shared_x])
}

/// `h(ENC_SENDER_ADDR_TAG, shared_x)`
pub fn compute_enc_sender_addr_hash(shared_x: Felt) -> Felt {
    hash(&[tags::enc_sender_addr(), shared_x])
}

/// `h(ENC_PRIVATE_KEY_TAG, shared_x)`
pub fn compute_enc_private_key_hash(shared_x: Felt) -> Felt {
    hash(&[tags::enc_private_key(), shared_x])
}

/// `h(ENC_USER_ADDR_TAG, shared_x)`
pub fn compute_enc_user_addr_hash(shared_x: Felt) -> Felt {
    hash(&[tags::enc_user_addr(), shared_x])
}

/// `identity_key = h(IDENTITY_KEY_TAG, user_addr, user_private_key, contract_address)`
pub fn compute_identity_key(
    user_addr: Felt,
    user_private_key: Felt,
    contract_address: Felt,
) -> Felt {
    hash(&[
        tags::identity_key(),
        user_addr,
        user_private_key,
        contract_address,
    ])
}
