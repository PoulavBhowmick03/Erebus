//! Recovers plaintext from pool storage.
//!
//! A note uses `packed_value = salt · 2^128 + (amount + mask)`. This module splits that felt
//! into its fields.
//!
//! ## Why this is implemented rather than imported
//!
//! `starkware-libs` ships these functions in `discovery-core`. That crate pins
//! `starknet-core`, `starknet-crypto` and
//! `starknet-providers` to a **`software-mansion/starknet-rust` fork by git rev**
//! (`7caedfe`). It also adds `starknet-providers`, `futures`, `async-trait` and `url`.
//! `Cargo.toml` explains why this crate does not use that fork.
//!
//! [`crate::hashes`] implements the five Poseidon masks and pins them to Cairo vectors. The
//! remaining work is field subtraction, `u128` wrapping subtraction, and ECDH point
//! recovery. Cairo remains the reference for both implementations.
//!
//! ## Encryption is additive, decryption is subtractive
//!
//! Each scheme uses `ciphertext = plaintext + mask` over the field. Amounts use
//! `wrapping_add` over `u128`. Decryption subtracts the same mask. A wrong key returns a
//! different plaintext instead of an authentication error. Callers must check decoded data.
//!
//! ## Scoped disclosure lives here
//!
//! [`crate::hashes::compute_enc_amount_hash`] takes the channel key instead of the pool
//! private key. A third party with one channel key can read only that channel. P2.2 uses
//! this boundary. The pool auditor escrow covers an identity's full history
//! (`privacy.cairo:329-334`).

use starknet_types_core::curve::AffinePoint;
use starknet_types_core::felt::Felt;

use crate::hashes;

/// The salt reserved for open (plaintext) notes. Their amount is not encrypted.
///
/// This is why [`crate::actions::NoteSalt`] refuses 0 and 1: an encrypted note that landed
/// on salt 1 would be read back as plaintext by every reader.
pub const OPEN_NOTE_SALT: u128 = 1;

/// Errors from decryption.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DecryptError {
    /// The stored ephemeral public key is not a valid x-coordinate on the Stark curve.
    ///
    /// The slot is empty or does not contain `EncChannelInfo`. A wrong key does not cause
    /// this error; it decrypts to another value.
    #[error("ephemeral public key is not a point on the curve; the slot is not channel info")]
    InvalidEphemeralPubkey,
}

/// What a note says once decrypted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteView {
    /// The value the note carries. Zero for the data notes that make up a message.
    pub amount: u128,
    /// The note's salt. For a data note this is one 119-bit chunk of a wire message; for a
    /// value note it is the random one-time-pad nonce and carries no meaning.
    pub salt: u128,
}

impl NoteView {
    /// Whether this note carries value, as opposed to being a message chunk.
    pub fn is_value_note(&self) -> bool {
        self.amount > 0
    }
}

/// A channel recovered from an `EncChannelInfo` written by the counterparty.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ChannelInfo {
    /// The channel key, which locates and decrypts every note in the channel.
    pub channel_key: Felt,
    /// Who opened the channel.
    pub sender_addr: Felt,
}

/// Redacts the channel key because it gives read access to the full channel.
impl core::fmt::Debug for ChannelInfo {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ChannelInfo")
            .field("channel_key", &"<redacted>")
            .field("sender_addr", &self.sender_addr)
            .finish()
    }
}

/// The low 128 bits of a felt.
fn low_u128(value: Felt) -> u128 {
    let digits = value.to_le_digits();
    u128::from(digits[0]) | (u128::from(digits[1]) << 64)
}

/// Splits a stored `packed_value` into its salt and its encrypted amount.
///
/// The layout is `packed = salt · 2^128 + enc_amount`. The high half is the salt. For a data
/// note, this salt is the payload chunk and needs no decryption.
pub fn unpack_note(packed: Felt) -> (u128, u128) {
    let digits = packed.to_le_digits();
    let enc_amount = u128::from(digits[0]) | (u128::from(digits[1]) << 64);
    let salt = u128::from(digits[2]) | (u128::from(digits[3]) << 64);
    (salt, enc_amount)
}

/// Removes the one-time-pad mask from an encrypted amount.
///
/// `amount = enc_amount - low128(h(ENC_AMOUNT_TAG, channel_key, token, index, 0, salt))`,
/// wrapping at 2^128.
///
/// The salt keys the mask. Two value notes with the same mask expose their amount difference
/// when an observer subtracts their ciphertexts. Structured salts are invalid on value notes.
pub fn note_amount(
    enc_amount: u128,
    salt: u128,
    channel_key: Felt,
    token: Felt,
    index: u64,
) -> u128 {
    let mask = low_u128(hashes::compute_enc_amount_hash(
        channel_key,
        token,
        index,
        salt,
    ));
    enc_amount.wrapping_sub(mask)
}

/// The exact one-time mask needed to disclose one encrypted value note.
///
/// A deal grant carries this value instead of the parent channel key. It opens one listed
/// payment note and cannot derive another note location or mask.
pub fn note_amount_mask(channel_key: Felt, token: Felt, index: u64, salt: u128) -> u128 {
    low_u128(hashes::compute_enc_amount_hash(
        channel_key,
        token,
        index,
        salt,
    ))
}

/// Reads a stored note.
///
/// Open notes (`salt == 1`) carry their amount in plaintext; everything else is masked.
pub fn packed_value(packed: Felt, channel_key: Felt, token: Felt, index: u64) -> NoteView {
    let (salt, enc_amount) = unpack_note(packed);
    let amount = if salt == OPEN_NOTE_SALT {
        enc_amount
    } else {
        note_amount(enc_amount, salt, channel_key, token, index)
    };
    NoteView { amount, salt }
}

/// Recovers a channel from the `EncChannelInfo` the sender wrote for this recipient.
///
/// This is the one place the recipient's **pool private key** is needed, because the pool
/// encrypts channel info with ephemeral-static ECDH: the sender picks an ephemeral scalar,
/// publishes `x(ephemeral · G)`, and masks with `h(tag, x(ephemeral · recipient_pubkey))`.
/// The recipient recomputes the same x as `x(ephemeral_pubkey · private_key)`.
///
/// Only the x-coordinate is used. The `false` y parity below can select `-P`, but
/// `x(k · -P) == x(k · P)`. Both roots produce the same shared x-coordinate.
pub fn channel_info(
    ephemeral_pubkey: Felt,
    enc_channel_key: Felt,
    enc_sender_addr: Felt,
    private_key: &Felt,
) -> Result<ChannelInfo, DecryptError> {
    let point = AffinePoint::new_from_x(&ephemeral_pubkey, false)
        .ok_or(DecryptError::InvalidEphemeralPubkey)?;
    let shared_x = (&point * *private_key).x();

    Ok(ChannelInfo {
        channel_key: enc_channel_key - hashes::compute_enc_channel_key_hash(shared_x),
        sender_addr: enc_sender_addr - hashes::compute_enc_sender_addr_hash(shared_x),
    })
}

/// Recovers which token a subchannel is for.
///
/// `token = enc_token - h(ENC_TOKEN_TAG, channel_key, index, 0, salt)`. Needs only the
/// channel key, so it is inside the scoped-disclosure boundary.
pub fn subchannel_token(enc_token: Felt, salt: Felt, channel_key: Felt, index: u64) -> Felt {
    enc_token - hashes::compute_enc_token_hash(channel_key, index, salt)
}

/// Recovers the recipient of one of *our own* outgoing channels.
///
/// Outgoing channel records use the sender's private key, not the channel key. This lets an
/// agent recover outgoing recipients after it loses local state.
pub fn outgoing_recipient_addr(
    enc_recipient_addr: Felt,
    sender_addr: Felt,
    private_key: &Felt,
    index: u64,
    salt: Felt,
) -> Felt {
    enc_recipient_addr
        - hashes::compute_enc_recipient_addr_hash(sender_addr, *private_key, index, salt)
}
