//! The read side: recovering plaintext from what the pool stores.
//!
//! Every write path in this crate has a mirror here. A note is written as
//! `packed_value = salt · 2^128 + (amount + mask)`; this module takes that felt back apart.
//!
//! ## Why this is implemented rather than imported
//!
//! `starkware-libs` ships `discovery-core`, which has exactly these five functions. It was
//! not used, for a reason worth recording: it pins `starknet-core`, `starknet-crypto` and
//! `starknet-providers` to a **`software-mansion/starknet-rust` fork by git rev**
//! (`7caedfe`), and pulls in `starknet-providers`, `futures`, `async-trait` and `url` along
//! with them. The write side already declined that fork — see the note in `Cargo.toml`.
//!
//! What we would be importing is small. The masks are Poseidon hashes, and this crate
//! already computes all five of them in [`crate::hashes`], already pinned to the Cairo
//! reference vectors. What remains is field subtraction, one `u128` wrapping subtraction,
//! and one ECDH point recovery. Reimplementing that against the *same* Cairo vectors
//! discovery-core is tested against is not a second source of truth — Cairo is the source
//! of truth, and both implementations answer to it.
//!
//! ## Encryption is additive, decryption is subtractive
//!
//! Every scheme here is `ciphertext = plaintext + mask` over the field, or
//! `wrapping_add` over `u128` for amounts. So decryption is the same mask, subtracted.
//! There is no authentication anywhere: a wrong key does not fail, it returns a different
//! plaintext. Nothing in this module can tell you the key was wrong, which is why the
//! callers above it check what they decoded rather than trusting it.
//!
//! ## Scoped disclosure lives here
//!
//! [`crate::hashes::compute_enc_amount_hash`] takes the **channel key**, not the pool
//! private key. So handing a third party one channel key discloses exactly one channel and
//! nothing else — the basis for P2.2, and a stronger property than the pool's own auditor
//! escrow, which is all-or-nothing over an identity's entire history
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
    /// This means the slot held something that was not an `EncChannelInfo` — an empty slot,
    /// or a misderived location. It does **not** mean the key was wrong; a wrong key
    /// decrypts successfully to garbage.
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

/// Redacts the channel key: it is the scoped disclosure secret, and a `Debug` that printed
/// it would leak a whole channel into any log line that happened to include one.
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
/// The layout is `packed = salt · 2^128 + enc_amount`, so the salt is simply the high half.
/// This is also the salt-lane read: for a data note the returned salt *is* the payload
/// chunk, and no decryption is needed to get it.
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
/// The mask is keyed on the salt, which is why a structured salt must never appear on a
/// value note: two notes sharing a mask let an observer subtract the ciphertexts and read
/// the difference of the amounts.
pub fn note_amount(enc_amount: u128, salt: u128, channel_key: Felt, token: Felt, index: u64) -> u128 {
    let mask = low_u128(hashes::compute_enc_amount_hash(channel_key, token, index, salt));
    enc_amount.wrapping_sub(mask)
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
/// Only the x-coordinate is used, which is what makes the `false` y-parity below
/// irrelevant: recovering the other root gives `-P`, and `x(k · -P) == x(k · P)`. Both
/// parties agree regardless of which root either recovered.
pub fn channel_info(
    ephemeral_pubkey: Felt,
    enc_channel_key: Felt,
    enc_sender_addr: Felt,
    private_key: &Felt,
) -> Result<ChannelInfo, DecryptError> {
    let point =
        AffinePoint::new_from_x(&ephemeral_pubkey, false).ok_or(DecryptError::InvalidEphemeralPubkey)?;
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
/// The sender cannot derive this from the channel key — outgoing channel records are keyed
/// on the sender's own private key, so this is how an agent enumerates who it has open
/// channels with after losing local state.
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
