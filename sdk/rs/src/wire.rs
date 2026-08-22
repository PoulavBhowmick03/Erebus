//! Negotiation messages stored in note salts.
//!
//! Wire v1 was a port of `sdk/ts/src/channel/wire.ts`. It stored the 400 message bits in
//! four public salts. It remains available for historical reads. Wire v2 encrypts and
//! authenticates the same message in five salts.
//!
//! A pool note has no payload field. The contract accepts a client-written salt in the range
//! `2 <= salt < 2^120`. It stores the salt in the high 120 bits of `packed_value`.
//! Negotiation messages use salts from zero-amount notes. Salts are public, so wire v2
//! encrypts each message before it splits the message across notes.
//!
//! Bit 119 is always 1. The payload uses bits 0 through 118. Without the high bit, a chunk
//! can equal 0 or 1. The contract rejects those values with `ZERO_SALT` or `SALT_TOO_SMALL`.
//! The high bit keeps every salt in `[2^119, 2^120)`.
//!
//! ## Layout
//!
//! Fields use most-significant-first order in a 400-bit plaintext:
//!
//! ```text
//!   type:8 | replyTo:32 | createdAt:40 | amount:128 | deadline:64 | memoHash:128
//!   \_______________________________ 400 bits _______________________________/
//!
//! ```
//!
//! Wire v2 encrypts these 50 bytes with AES-256-GCM-SIV. Its ciphertext, tag, and marker use
//! 536 of the 595 payload bits. The remaining 59 bits are zero.
//!
//! Wire v3 prepends a 64-bit deal id before encryption. Its 58-byte ciphertext and 16-byte
//! tag use 592 bits. A derived mask fills the three spare bits. Wire v3 identifies a frame
//! by its physical first-note index. An acceptance frame adds a payment note after its five
//! data notes.
//!
//! ## Which notes may use this
//!
//! Structured salts are valid only on zero-amount notes. A value note needs a random salt
//! because the salt is the one-time-pad nonce for its encrypted amount. Reusing a mask for
//! two different amounts lets an observer subtract the ciphertexts and recover the
//! difference. Zero-amount notes do not leak an amount difference.
//!
//! This module only produces salts. The note builder enforces the zero-amount requirement.

use aes_gcm_siv::aead::{AeadInPlace, KeyInit};
use aes_gcm_siv::{Aes256GcmSiv, Nonce, Tag};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use starknet_types_core::felt::Felt;

use crate::actions::{ActionError, NoteSalt};

/// Notes per wire-v2 negotiation message.
pub const NOTES_PER_MESSAGE: usize = 5;
/// Notes per legacy wire-v1 negotiation message.
pub const LEGACY_NOTES_PER_MESSAGE: usize = 4;
/// Payload bits per note. Bit 119 is the pinned format flag.
pub const PAYLOAD_BITS_PER_NOTE: u32 = 119;

const DEAL_ID_BITS: u32 = 64;
const TYPE_BITS: u32 = 8;
const REPLY_TO_BITS: u32 = 32;
const CREATED_AT_BITS: u32 = 40;
const AMOUNT_BITS: u32 = 128;
const DEADLINE_BITS: u32 = 64;
const MEMO_HASH_BITS: u32 = 128;

/// Total packed width.
pub const MESSAGE_BITS: u32 =
    TYPE_BITS + REPLY_TO_BITS + CREATED_AT_BITS + AMOUNT_BITS + DEADLINE_BITS + MEMO_HASH_BITS;
/// Bits available across five wire-v2 notes.
pub const CAPACITY_BITS: u32 = NOTES_PER_MESSAGE as u32 * PAYLOAD_BITS_PER_NOTE;

/// Bits available across four legacy wire-v1 notes.
pub const LEGACY_CAPACITY_BITS: u32 = LEGACY_NOTES_PER_MESSAGE as u32 * PAYLOAD_BITS_PER_NOTE;
const V2_MARKER: u8 = 2;
const PLAINTEXT_BYTES: usize = MESSAGE_BITS as usize / 8;
const TAG_BYTES: usize = 16;
const V2_PAYLOAD_BYTES: usize = 1 + PLAINTEXT_BYTES + TAG_BYTES;
const V2_PAYLOAD_BITS: u32 = V2_PAYLOAD_BYTES as u32 * 8;

const FLAG_BIT: u128 = 1u128 << 119;
const PAYLOAD_MASK: u128 = FLAG_BIT - 1;

/// Wire-v3 envelope width: an obfuscated 64-bit deal id plus the historical message fields.
pub const V3_MESSAGE_BITS: u32 = DEAL_ID_BITS + MESSAGE_BITS;
const V3_HEADER_BYTES: usize = DEAL_ID_BITS as usize / 8;
const V3_PLAINTEXT_BYTES: usize = PLAINTEXT_BYTES;
const V3_PAYLOAD_BYTES: usize = V3_HEADER_BYTES + V3_PLAINTEXT_BYTES + TAG_BYTES;
const V3_PAYLOAD_BITS: u32 = V3_PAYLOAD_BYTES as u32 * 8;
/// Bits wire v3 masks with a derived keystream: the three spare bits `592..594`.
const V3_MASK_LO: u32 = V3_PAYLOAD_BITS;
const V3_MASK_BITS: u32 = CAPACITY_BITS - V3_MASK_LO;
/// Bytes of HKDF output the mask consumes. `ceil(3 / 8)`.
const V3_MASK_BYTES: usize = V3_MASK_BITS.div_ceil(8) as usize;
/// Bytes needed to hold all 595 salt payload bits. `ceil(595 / 8)`.
const V3_ENVELOPE_BYTES: usize = CAPACITY_BITS.div_ceil(8) as usize;

/// `2^32 - 1`, reserved to mean "no reply", so it is not a usable message index.
const NO_REPLY_TO: u32 = u32::MAX;

const _: () = assert!(
    MESSAGE_BITS <= CAPACITY_BITS,
    "wire layout does not fit the notes allocated for it"
);
const _: () = assert!(MESSAGE_BITS <= LEGACY_CAPACITY_BITS);
const _: () = assert!(V2_PAYLOAD_BITS <= CAPACITY_BITS);
const _: () = assert!(V3_MESSAGE_BITS.is_multiple_of(8));
const _: () = assert!(V3_PAYLOAD_BITS <= CAPACITY_BITS);
const _: () = assert!(V3_MASK_LO + V3_MASK_BITS == CAPACITY_BITS);

/// Negotiation wire generation stored with a channel.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireVersion {
    /// Public four-note wire retained for historical reads only.
    #[default]
    V1,
    /// Five-note authenticated-encryption wire.
    V2,
    /// Five-note authenticated encryption with a deal id and derived spare-bit mask.
    V3,
}

impl WireVersion {
    /// Number of consecutive note slots occupied by one message.
    pub const fn notes_per_message(self) -> usize {
        match self {
            Self::V1 => LEGACY_NOTES_PER_MESSAGE,
            Self::V2 | Self::V3 => NOTES_PER_MESSAGE,
        }
    }
}

/// Immutable context that scopes wire-v2 key derivation and authentication.
#[derive(Clone, Copy)]
pub struct WireContext {
    /// Starknet chain whose state carries the ciphertext.
    pub chain_id: Felt,
    /// Pool whose public storage carries the ciphertext.
    pub pool_address: Felt,
    /// Directional channel secret.
    pub channel_key: Felt,
    /// Token subchannel.
    pub token: Felt,
    /// Message position within the subchannel.
    pub message_index: u32,
}

impl core::fmt::Debug for WireContext {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("WireContext")
            .field("chain_id", &self.chain_id)
            .field("pool_address", &self.pool_address)
            .field("channel_key", &"<redacted>")
            .field("token", &self.token)
            .field("message_index", &self.message_index)
            .finish()
    }
}

/// Errors encoding or decoding a wire message.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum WireError {
    /// A field was wider than its allotted bits.
    #[error("{field} does not fit in {bits} bits")]
    FieldTooWide {
        /// Field name.
        field: &'static str,
        /// Allotted width.
        bits: u32,
    },
    /// `replyTo` was the reserved sentinel.
    #[error("replyTo 2^32-1 is reserved as the 'no reply' sentinel")]
    ReservedReplyTo,
    /// Historical wires have no deal-id field.
    #[error("wire v1 and wire v2 cannot encode a deal id")]
    DealIdUnsupported,
    /// A salt lacked the pinned format flag, so it is not an Erebus data note.
    #[error("salt at slot {0} is missing the format flag, so it is not an Erebus data note")]
    MissingFlag(usize),
    /// The type code did not name a known message type.
    #[error("unknown message type code: {0}")]
    UnknownType(u8),
    /// A produced salt fell outside the contract's accepted range.
    #[error(transparent)]
    Salt(#[from] ActionError),
    /// Wire v1 is read-only because its payload is public.
    #[error("wire v1 is read-only; open a wire-v2 or wire-v3 channel before writing")]
    LegacyReadOnly,
    /// The authenticated ciphertext was changed or decoded under the wrong context.
    #[error("wire authentication failed")]
    Authentication,
    /// The five chunks did not carry the wire-v2 marker and canonical zero padding.
    #[error("invalid wire-v2 marker or padding")]
    InvalidV2Envelope,
    /// The five chunks did not carry canonical wire-v3 authenticated padding.
    #[error("invalid wire-v3 padding")]
    InvalidV3Envelope,
}

/// What a negotiation message is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    /// An opening offer.
    Offer,
    /// A counter to a previous message.
    Counter,
    /// Acceptance of a previous message.
    Accept,
}

impl MessageType {
    fn code(self) -> u8 {
        match self {
            Self::Offer => 1,
            Self::Counter => 2,
            Self::Accept => 3,
        }
    }

    fn from_code(code: u8) -> Result<Self, WireError> {
        match code {
            1 => Ok(Self::Offer),
            2 => Ok(Self::Counter),
            3 => Ok(Self::Accept),
            other => Err(WireError::UnknownType(other)),
        }
    }
}

/// Negotiation message stored on the wire.
///
/// `OfferTerms::token` is absent because each subchannel identifies one token. The note
/// index orders messages and makes each message unique, so the wire also omits the nonce.
/// These omissions keep the plaintext at 400 bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireMessage {
    /// Negotiation identifier. Historical v1/v2 messages decode with deal id zero.
    pub deal_id: u64,
    /// Message kind.
    pub message_type: MessageType,
    /// Index of the message being replied to, or `None` for an opening offer.
    pub reply_to: Option<u32>,
    /// Unix seconds. 40 bits, which runs well past any plausible deadline.
    pub created_at: u64,
    /// Amount in the token's smallest unit.
    pub amount: u128,
    /// Unix seconds.
    pub deadline: u64,
    /// Memo hash, already truncated to 128 bits. See [`truncate_memo_hash`].
    pub memo_hash: u128,
}

/// Truncates a `felt252` memo hash to the low 128 bits carried on the wire.
///
/// This drops 124 bits and leaves 2^64 collision resistance under birthday bounds. The hash
/// commits to an off-chain memo. A collision can support a false memo claim, but cannot
/// authorize a spend.
pub fn truncate_memo_hash(memo_hash: Felt) -> u128 {
    truncate_memo_hash_bytes(&memo_hash.to_bytes_be())
}

/// Truncates a memo hash of any width to the low 128 bits carried on the wire.
///
/// Takes big-endian bytes rather than a [`Felt`] because the common input is a whole digest
/// and a `felt252` cannot hold one: SHA-256 produces 256 bits, above the field modulus, so
/// parsing a real digest into a `Felt` either fails or wraps to a different value.
///
/// Truncation is a wire rule and belongs here. Asking a caller to pre-truncate puts a
/// protocol detail in a layer that must not hold one, and gives every caller its own chance
/// to take the wrong end of the digest. Shorter input is left-padded, so a 64-bit memo and
/// the same value inside a 256-bit digest agree.
pub fn truncate_memo_hash_bytes(digest: &[u8]) -> u128 {
    let mut low = [0u8; 16];
    let take = digest.len().min(16);
    low[16 - take..].copy_from_slice(&digest[digest.len() - take..]);
    u128::from_be_bytes(low)
}

/// First note index of message `message_index` within a subchannel.
pub fn note_index_for_message(message_index: u32) -> u32 {
    message_index * NOTES_PER_MESSAGE as u32
}

/// First note index of a historical wire-v1 message.
pub fn legacy_note_index_for_message(message_index: u32) -> u32 {
    message_index * LEGACY_NOTES_PER_MESSAGE as u32
}

/// Bits of the packed message, most significant first.
struct Bits {
    bits: Vec<bool>,
}

impl Bits {
    fn new() -> Self {
        Self {
            bits: Vec::with_capacity(MESSAGE_BITS as usize),
        }
    }

    fn push(&mut self, value: u128, width: u32) {
        for i in (0..width).rev() {
            self.bits.push((value >> i) & 1 == 1);
        }
    }

    /// The bit at integer position `pos` (0 = least significant).
    fn at(&self, pos: u32) -> bool {
        if pos >= MESSAGE_BITS {
            return false;
        }
        self.bits[(MESSAGE_BITS - 1 - pos) as usize]
    }

    fn from_legacy_chunks(chunks: [u128; LEGACY_NOTES_PER_MESSAGE]) -> Self {
        let mut bits = vec![false; MESSAGE_BITS as usize];
        for (slot, chunk) in chunks.iter().enumerate() {
            for j in 0..PAYLOAD_BITS_PER_NOTE {
                let pos = slot as u32 * PAYLOAD_BITS_PER_NOTE + j;
                if pos < MESSAGE_BITS && (chunk >> j) & 1 == 1 {
                    bits[(MESSAGE_BITS - 1 - pos) as usize] = true;
                }
            }
        }
        Self { bits }
    }

    fn to_bytes(&self) -> [u8; PLAINTEXT_BYTES] {
        let mut bytes = [0u8; PLAINTEXT_BYTES];
        for (index, bit) in self.bits.iter().enumerate() {
            if *bit {
                bytes[index / 8] |= 1 << (7 - index % 8);
            }
        }
        bytes
    }

    fn from_bytes(bytes: &[u8; PLAINTEXT_BYTES]) -> Self {
        let bits = (0..MESSAGE_BITS as usize)
            .map(|index| bytes[index / 8] & (1 << (7 - index % 8)) != 0)
            .collect();
        Self { bits }
    }

    /// Reads `width` bits starting at MSB-first cursor `offset`.
    fn read(&self, offset: u32, width: u32) -> u128 {
        let mut value = 0u128;
        for i in 0..width {
            value = (value << 1) | u128::from(self.bits[(offset + i) as usize]);
        }
        value
    }
}

fn fits(value: u128, bits: u32, field: &'static str) -> Result<(), WireError> {
    if bits < 128 && value >= 1u128 << bits {
        return Err(WireError::FieldTooWide { field, bits });
    }
    Ok(())
}

fn pack_message(message: &WireMessage) -> Result<Bits, WireError> {
    if message.deal_id != 0 {
        return Err(WireError::DealIdUnsupported);
    }
    let reply_to = match message.reply_to {
        Some(NO_REPLY_TO) => return Err(WireError::ReservedReplyTo),
        Some(index) => index,
        None => NO_REPLY_TO,
    };

    fits(u128::from(message.created_at), CREATED_AT_BITS, "createdAt")?;
    fits(u128::from(message.deadline), DEADLINE_BITS, "deadline")?;

    let mut bits = Bits::new();
    bits.push(u128::from(message.message_type.code()), TYPE_BITS);
    bits.push(u128::from(reply_to), REPLY_TO_BITS);
    bits.push(u128::from(message.created_at), CREATED_AT_BITS);
    bits.push(message.amount, AMOUNT_BITS);
    bits.push(u128::from(message.deadline), DEADLINE_BITS);
    bits.push(message.memo_hash, MEMO_HASH_BITS);

    Ok(bits)
}

fn unpack_message(bits: &Bits) -> Result<WireMessage, WireError> {
    let mut cursor = 0u32;
    let mut take = |width: u32| {
        let value = bits.read(cursor, width);
        cursor += width;
        value
    };

    let message_type = MessageType::from_code(take(TYPE_BITS) as u8)?;
    let reply_to_raw = take(REPLY_TO_BITS) as u32;
    let created_at = take(CREATED_AT_BITS) as u64;
    let amount = take(AMOUNT_BITS);
    let deadline = take(DEADLINE_BITS) as u64;
    let memo_hash = take(MEMO_HASH_BITS);

    Ok(WireMessage {
        deal_id: 0,
        message_type,
        reply_to: (reply_to_raw != NO_REPLY_TO).then_some(reply_to_raw),
        created_at,
        amount,
        deadline,
        memo_hash,
    })
}

fn write_msb(bytes: &mut [u8], cursor: &mut u32, value: u128, width: u32) {
    for bit in (0..width).rev() {
        if value >> bit & 1 == 1 {
            let position = *cursor as usize;
            bytes[position / 8] |= 1 << (7 - position % 8);
        }
        *cursor += 1;
    }
}

fn read_msb(bytes: &[u8], cursor: &mut u32, width: u32) -> u128 {
    let mut value = 0u128;
    for _ in 0..width {
        let position = *cursor as usize;
        value = (value << 1) | u128::from(bytes[position / 8] >> (7 - position % 8) & 1);
        *cursor += 1;
    }
    value
}

fn pack_message_v3(message: &WireMessage) -> Result<[u8; V3_PLAINTEXT_BYTES], WireError> {
    let reply_to = match message.reply_to {
        Some(NO_REPLY_TO) => return Err(WireError::ReservedReplyTo),
        Some(index) => index,
        None => NO_REPLY_TO,
    };
    fits(u128::from(message.created_at), CREATED_AT_BITS, "createdAt")?;
    fits(u128::from(message.deadline), DEADLINE_BITS, "deadline")?;

    let mut bytes = [0u8; V3_PLAINTEXT_BYTES];
    let mut cursor = 0u32;
    write_msb(
        &mut bytes,
        &mut cursor,
        u128::from(message.message_type.code()),
        TYPE_BITS,
    );
    write_msb(&mut bytes, &mut cursor, u128::from(reply_to), REPLY_TO_BITS);
    write_msb(
        &mut bytes,
        &mut cursor,
        u128::from(message.created_at),
        CREATED_AT_BITS,
    );
    write_msb(&mut bytes, &mut cursor, message.amount, AMOUNT_BITS);
    write_msb(
        &mut bytes,
        &mut cursor,
        u128::from(message.deadline),
        DEADLINE_BITS,
    );
    write_msb(&mut bytes, &mut cursor, message.memo_hash, MEMO_HASH_BITS);
    debug_assert_eq!(cursor, MESSAGE_BITS);
    Ok(bytes)
}

fn unpack_message_v3(
    deal_id: u64,
    bytes: &[u8; V3_PLAINTEXT_BYTES],
) -> Result<WireMessage, WireError> {
    let mut cursor = 0u32;
    let message_type = MessageType::from_code(read_msb(bytes, &mut cursor, TYPE_BITS) as u8)?;
    let reply_to_raw = read_msb(bytes, &mut cursor, REPLY_TO_BITS) as u32;
    let created_at = read_msb(bytes, &mut cursor, CREATED_AT_BITS) as u64;
    let amount = read_msb(bytes, &mut cursor, AMOUNT_BITS);
    let deadline = read_msb(bytes, &mut cursor, DEADLINE_BITS) as u64;
    let memo_hash = read_msb(bytes, &mut cursor, MEMO_HASH_BITS);
    debug_assert_eq!(cursor, MESSAGE_BITS);

    Ok(WireMessage {
        deal_id,
        message_type,
        reply_to: (reply_to_raw != NO_REPLY_TO).then_some(reply_to_raw),
        created_at,
        amount,
        deadline,
        memo_hash,
    })
}

fn bit_at_lsb(bytes: &[u8], position: u32) -> bool {
    let bit_from_msb = bytes.len() as u32 * 8 - 1 - position;
    bytes[(bit_from_msb / 8) as usize] & (1 << (7 - bit_from_msb % 8)) != 0
}

fn set_bit_lsb(bytes: &mut [u8], position: u32) {
    let bit_from_msb = bytes.len() as u32 * 8 - 1 - position;
    bytes[(bit_from_msb / 8) as usize] |= 1 << (7 - bit_from_msb % 8);
}

fn derive_key_and_nonce(context: &WireContext) -> ([u8; 32], [u8; 12]) {
    const SALT: &[u8] = b"EREBUS_WIRE_V2_HKDF_SHA256";
    let channel_key = context.channel_key.to_bytes_be();
    let hkdf = Hkdf::<Sha256>::new(Some(SALT), &channel_key);

    let mut scope = Vec::with_capacity(96);
    scope.extend_from_slice(&context.chain_id.to_bytes_be());
    scope.extend_from_slice(&context.pool_address.to_bytes_be());
    scope.extend_from_slice(&context.token.to_bytes_be());

    let mut key_info = b"EREBUS_WIRE_V2_KEY".to_vec();
    key_info.extend_from_slice(&scope);
    let mut key = [0u8; 32];
    hkdf.expand(&key_info, &mut key)
        .expect("32-byte HKDF output is always valid");

    let mut nonce_info = b"EREBUS_WIRE_V2_NONCE".to_vec();
    nonce_info.extend_from_slice(&scope);
    nonce_info.extend_from_slice(&context.message_index.to_be_bytes());
    let mut nonce = [0u8; 12];
    hkdf.expand(&nonce_info, &mut nonce)
        .expect("12-byte HKDF output is always valid");

    (key, nonce)
}

fn v3_scope(context: &WireContext) -> Vec<u8> {
    let mut scope = Vec::with_capacity(96);
    scope.extend_from_slice(&context.chain_id.to_bytes_be());
    scope.extend_from_slice(&context.pool_address.to_bytes_be());
    scope.extend_from_slice(&context.token.to_bytes_be());
    scope
}

/// Native wire-v3 encryption key for one deal in one direction.
///
/// The parent channel key never enters a v3 grant. Granting this key opens only messages
/// whose authenticated `deal_id` matches.
pub fn derive_deal_key(context: &WireContext, deal_id: u64) -> [u8; 32] {
    const SALT: &[u8] = b"EREBUS_WIRE_V3_DEAL_KEY_HKDF_SHA256";
    let channel_key = context.channel_key.to_bytes_be();
    let hkdf = Hkdf::<Sha256>::new(Some(SALT), &channel_key);
    let mut key_info = b"EREBUS_WIRE_V3_DEAL_KEY".to_vec();
    key_info.extend_from_slice(&v3_scope(context));
    key_info.extend_from_slice(&deal_id.to_be_bytes());
    let mut key = [0u8; 32];
    hkdf.expand(&key_info, &mut key)
        .expect("32-byte HKDF output is always valid");
    key
}

fn derive_v3_nonce(context: &WireContext, deal_key: &[u8; 32]) -> [u8; 12] {
    const SALT: &[u8] = b"EREBUS_WIRE_V3_HKDF_SHA256";
    let hkdf = Hkdf::<Sha256>::new(Some(SALT), deal_key);
    let mut nonce_info = b"EREBUS_WIRE_V3_NONCE".to_vec();
    nonce_info.extend_from_slice(&v3_scope(context));
    nonce_info.extend_from_slice(&context.message_index.to_be_bytes());
    let mut nonce = [0u8; 12];
    hkdf.expand(&nonce_info, &mut nonce)
        .expect("12-byte HKDF output is always valid");
    nonce
}

fn obfuscated_deal_id(context: &WireContext, deal_id: u64) -> [u8; V3_HEADER_BYTES] {
    const SALT: &[u8] = b"EREBUS_WIRE_V3_HEADER_HKDF_SHA256";
    let channel_key = context.channel_key.to_bytes_be();
    let hkdf = Hkdf::<Sha256>::new(Some(SALT), &channel_key);
    let mut info = b"EREBUS_WIRE_V3_DEAL_HEADER".to_vec();
    info.extend_from_slice(&v3_scope(context));
    info.extend_from_slice(&context.message_index.to_be_bytes());
    let mut mask = [0u8; V3_HEADER_BYTES];
    hkdf.expand(&info, &mut mask)
        .expect("8-byte HKDF output is always valid");
    let mut header = deal_id.to_be_bytes();
    for (byte, mask_byte) in header.iter_mut().zip(mask) {
        *byte ^= mask_byte;
    }
    header
}

fn recover_deal_id(context: &WireContext, header: &[u8; V3_HEADER_BYTES]) -> u64 {
    let masked_zero = obfuscated_deal_id(context, 0);
    let mut bytes = *header;
    for (byte, mask_byte) in bytes.iter_mut().zip(masked_zero) {
        *byte ^= mask_byte;
    }
    u64::from_be_bytes(bytes)
}

/// Derives the deal id for a wire-v3 opening offer at this physical frame start.
pub fn derive_deal_id(context: &WireContext) -> u64 {
    const SALT: &[u8] = b"EREBUS_WIRE_V3_DEAL_HKDF_SHA256";
    let channel_key = context.channel_key.to_bytes_be();
    let hkdf = Hkdf::<Sha256>::new(Some(SALT), &channel_key);
    let mut info = b"EREBUS_WIRE_V3_DEAL_ID".to_vec();
    info.extend_from_slice(&v3_scope(context));
    info.extend_from_slice(&context.message_index.to_be_bytes());
    let mut bytes = [0u8; 8];
    hkdf.expand(&info, &mut bytes)
        .expect("8-byte HKDF output is always valid");
    u64::from_be_bytes(bytes)
}

/// Keystream covering wire v3's three spare bits.
///
/// Derived rather than random, for four reasons that all point the same way:
///
/// - **Encoding stays deterministic.** The same message at the same index produces the same
///   salts, so a known-answer test can pin the wire byte-for-byte. Random padding would make
///   the encoder untestable against a fixture.
/// - **A retry rebuilds identical salts.** Wire v2 chose AES-GCM-SIV precisely because an
///   attempt can fail before `WriteOnce` applies and then be rebuilt at the same index.
///   Random padding would emit different salts for the same message on the second attempt.
/// - **The reader can verify it.** The mask is outside the AEAD, so nothing else would catch
///   a flipped spare bit. Recomputing it authenticates all three spare bits.
/// - **No new entropy source.** The encode path stays a pure function of context and message.
///
/// An observer without the channel key cannot distinguish HKDF output from random, so
/// deriving costs nothing against the observer this defends against.
fn derive_v3_mask(
    context: &WireContext,
    deal_key: &[u8; 32],
    header: &[u8; V3_HEADER_BYTES],
) -> [u8; V3_MASK_BYTES] {
    const SALT: &[u8] = b"EREBUS_WIRE_V3_MASK_HKDF_SHA256";
    let hkdf = Hkdf::<Sha256>::new(Some(SALT), deal_key);

    let mut info = b"EREBUS_WIRE_V3_MASK".to_vec();
    info.extend_from_slice(&context.chain_id.to_bytes_be());
    info.extend_from_slice(&context.pool_address.to_bytes_be());
    info.extend_from_slice(&context.token.to_bytes_be());
    info.extend_from_slice(&context.message_index.to_be_bytes());
    info.extend_from_slice(header);

    let mut mask = [0u8; V3_MASK_BYTES];
    hkdf.expand(&info, &mut mask)
        .expect("one-byte HKDF output is always valid");
    mask
}

/// Whether the mask's bit `offset` (0 = first masked position) is set.
fn mask_bit(mask: &[u8; V3_MASK_BYTES], offset: u32) -> bool {
    mask[(offset / 8) as usize] & (1 << (offset % 8)) != 0
}

fn associated_data(context: &WireContext) -> Vec<u8> {
    let mut data = b"EREBUS_WIRE_V2_AAD".to_vec();
    data.extend_from_slice(&context.chain_id.to_bytes_be());
    data.extend_from_slice(&context.pool_address.to_bytes_be());
    data.extend_from_slice(&context.token.to_bytes_be());
    data.extend_from_slice(&context.message_index.to_be_bytes());
    data
}

fn associated_data_v3(
    context: &WireContext,
    deal_id: u64,
    header: &[u8; V3_HEADER_BYTES],
) -> Vec<u8> {
    let mut data = b"EREBUS_WIRE_V3_AAD".to_vec();
    data.extend_from_slice(&v3_scope(context));
    data.extend_from_slice(&context.message_index.to_be_bytes());
    data.extend_from_slice(&deal_id.to_be_bytes());
    data.extend_from_slice(header);
    data
}

/// Encrypts a message into exactly five contract-valid salts, in note-index order.
pub fn encode_message(
    context: &WireContext,
    message: &WireMessage,
) -> Result<[NoteSalt; NOTES_PER_MESSAGE], WireError> {
    let mut ciphertext = pack_message(message)?.to_bytes();
    let (key, nonce) = derive_key_and_nonce(context);
    let cipher =
        Aes256GcmSiv::new_from_slice(&key).expect("AES-256-GCM-SIV accepts every 32-byte key");
    let tag = cipher
        .encrypt_in_place_detached(
            Nonce::from_slice(&nonce),
            &associated_data(context),
            &mut ciphertext,
        )
        .map_err(|_| WireError::Authentication)?;

    let mut payload = [0u8; V2_PAYLOAD_BYTES];
    payload[0] = V2_MARKER;
    payload[1..1 + PLAINTEXT_BYTES].copy_from_slice(&ciphertext);
    payload[1 + PLAINTEXT_BYTES..].copy_from_slice(&tag);

    let mut salts = Vec::with_capacity(NOTES_PER_MESSAGE);
    for slot in 0..NOTES_PER_MESSAGE {
        let mut chunk = 0u128;
        for j in 0..PAYLOAD_BITS_PER_NOTE {
            let position = slot as u32 * PAYLOAD_BITS_PER_NOTE + j;
            if position < V2_PAYLOAD_BITS && bit_at_lsb(&payload, position) {
                chunk |= 1u128 << j;
            }
        }
        salts.push(NoteSalt::new(chunk | FLAG_BIT)?);
    }

    Ok([salts[0], salts[1], salts[2], salts[3], salts[4]])
}

/// Authenticates and decrypts five wire-v2 salts in note-index order.
pub fn decode_message(
    context: &WireContext,
    salts: &[NoteSalt; NOTES_PER_MESSAGE],
) -> Result<WireMessage, WireError> {
    let mut payload = [0u8; V2_PAYLOAD_BYTES];
    for (slot, salt) in salts.iter().enumerate() {
        let value = salt.get();
        if value & FLAG_BIT == 0 {
            return Err(WireError::MissingFlag(slot));
        }
        let chunk = value & PAYLOAD_MASK;
        for j in 0..PAYLOAD_BITS_PER_NOTE {
            let position = slot as u32 * PAYLOAD_BITS_PER_NOTE + j;
            if chunk & (1 << j) == 0 {
                continue;
            }
            if position >= V2_PAYLOAD_BITS {
                return Err(WireError::InvalidV2Envelope);
            }
            set_bit_lsb(&mut payload, position);
        }
    }

    if payload[0] != V2_MARKER {
        return Err(WireError::InvalidV2Envelope);
    }

    let mut plaintext = [0u8; PLAINTEXT_BYTES];
    plaintext.copy_from_slice(&payload[1..1 + PLAINTEXT_BYTES]);
    let tag = Tag::from_slice(&payload[1 + PLAINTEXT_BYTES..]);
    let (key, nonce) = derive_key_and_nonce(context);
    let cipher =
        Aes256GcmSiv::new_from_slice(&key).expect("AES-256-GCM-SIV accepts every 32-byte key");
    cipher
        .decrypt_in_place_detached(
            Nonce::from_slice(&nonce),
            &associated_data(context),
            &mut plaintext,
            tag,
        )
        .map_err(|_| WireError::Authentication)?;

    unpack_message(&Bits::from_bytes(&plaintext))
}

/// Encrypts a message into five salts that carry no fixed shape.
///
/// Wire v2 filled 536 of 595 payload bits and zero-filled the rest, so the fifth salt of
/// every message had bits 60..118 clear whatever the message said. A random salt has that
/// shape with probability 2^-59, so the fifth salt identified an Erebus message essentially
/// every time. Measured at balanced accuracy 1.0000 in `scripts/linkage.py`; tracked as F31
/// and as M1 in `docs/threat-model.md`.
///
/// Wire v3 covers the three spare bits with a separately derived keystream. The ciphertext
/// and tag below them are already uniform, so all 595 bits become
/// indistinguishable from random and only the pinned bit 119 remains — which about half of
/// all ordinary pool salts carry anyway.
///
/// **This is not backward compatible, by construction.** A v2 reader validates the spare
/// bits as zero and will reject every v3 message. Both parties must run v3, which is why it
/// is a wire version rather than a patch.
pub fn encode_message_v3(
    context: &WireContext,
    message: &WireMessage,
) -> Result<[NoteSalt; NOTES_PER_MESSAGE], WireError> {
    let mut ciphertext = pack_message_v3(message)?;
    let header = obfuscated_deal_id(context, message.deal_id);
    let deal_key = derive_deal_key(context, message.deal_id);
    let nonce = derive_v3_nonce(context, &deal_key);
    let cipher =
        Aes256GcmSiv::new_from_slice(&deal_key).expect("AES-256-GCM-SIV accepts every 32-byte key");
    let tag = cipher
        .encrypt_in_place_detached(
            Nonce::from_slice(&nonce),
            &associated_data_v3(context, message.deal_id, &header),
            &mut ciphertext,
        )
        .map_err(|_| WireError::Authentication)?;

    let mut payload = [0u8; V3_PAYLOAD_BYTES];
    payload[..V3_HEADER_BYTES].copy_from_slice(&header);
    payload[V3_HEADER_BYTES..V3_HEADER_BYTES + V3_PLAINTEXT_BYTES].copy_from_slice(&ciphertext);
    payload[V3_HEADER_BYTES + V3_PLAINTEXT_BYTES..].copy_from_slice(&tag);

    let mask = derive_v3_mask(context, &deal_key, &header);
    let mut salts = Vec::with_capacity(NOTES_PER_MESSAGE);
    for slot in 0..NOTES_PER_MESSAGE {
        let mut chunk = 0u128;
        for j in 0..PAYLOAD_BITS_PER_NOTE {
            let position = slot as u32 * PAYLOAD_BITS_PER_NOTE + j;
            if position >= CAPACITY_BITS {
                continue;
            }
            // Below the masked region the envelope is ciphertext and tag; above it, the
            // spare bits contribute nothing but the mask.
            let carried = position < V3_PAYLOAD_BITS && bit_at_lsb(&payload, position);
            let masked = position >= V3_MASK_LO && mask_bit(&mask, position - V3_MASK_LO);
            if carried != masked {
                chunk |= 1u128 << j;
            }
        }
        salts.push(NoteSalt::new(chunk | FLAG_BIT)?);
    }

    Ok([salts[0], salts[1], salts[2], salts[3], salts[4]])
}

/// Authenticates and decrypts five wire-v3 salts in note-index order.
///
/// Unmasking is verified rather than discarded: the mask sits outside the AEAD, so a flipped
/// spare bit would otherwise pass silently. Recomputing it and rejecting a mismatch makes
/// those three bits authenticated in practice.
pub fn decode_message_v3(
    context: &WireContext,
    salts: &[NoteSalt; NOTES_PER_MESSAGE],
) -> Result<WireMessage, WireError> {
    let payload = v3_payload(salts)?;
    let header: [u8; V3_HEADER_BYTES] = payload[..V3_HEADER_BYTES]
        .try_into()
        .expect("wire-v3 header has fixed width");
    let deal_id = recover_deal_id(context, &header);
    let deal_key = derive_deal_key(context, deal_id);
    decode_message_v3_with_deal_key(context, deal_id, &deal_key, salts)
}

fn v3_payload(salts: &[NoteSalt; NOTES_PER_MESSAGE]) -> Result<[u8; V3_PAYLOAD_BYTES], WireError> {
    let mut envelope = [0u8; V3_ENVELOPE_BYTES];
    for (slot, salt) in salts.iter().enumerate() {
        let value = salt.get();
        if value & FLAG_BIT == 0 {
            return Err(WireError::MissingFlag(slot));
        }
        let chunk = value & PAYLOAD_MASK;
        for j in 0..PAYLOAD_BITS_PER_NOTE {
            let position = slot as u32 * PAYLOAD_BITS_PER_NOTE + j;
            if position >= CAPACITY_BITS {
                // Salt 4 ends exactly at the capacity bound, so a set bit here means the
                // salt was not produced by this encoder.
                if chunk & (1 << j) != 0 {
                    return Err(WireError::InvalidV3Envelope);
                }
                continue;
            }
            if position < V3_PAYLOAD_BITS && chunk & (1 << j) != 0 {
                set_bit_lsb(&mut envelope, position);
            }
        }
    }

    let payload_lo = V3_ENVELOPE_BYTES - V3_PAYLOAD_BYTES;
    Ok(envelope[payload_lo..]
        .try_into()
        .expect("the envelope is wider than the payload"))
}

/// Authenticates one wire-v3 frame with a native per-deal key.
///
/// This entry point is for a deal-scoped grant. It does not require the parent channel key.
pub fn decode_message_v3_with_deal_key(
    context: &WireContext,
    deal_id: u64,
    deal_key: &[u8; 32],
    salts: &[NoteSalt; NOTES_PER_MESSAGE],
) -> Result<WireMessage, WireError> {
    let payload = v3_payload(salts)?;
    let header: [u8; V3_HEADER_BYTES] = payload[..V3_HEADER_BYTES]
        .try_into()
        .expect("wire-v3 header has fixed width");
    let mask = derive_v3_mask(context, deal_key, &header);

    // The three spare bits contain only the deal-key-derived mask.
    for position in V3_PAYLOAD_BITS..CAPACITY_BITS {
        let slot = position / PAYLOAD_BITS_PER_NOTE;
        let bit = position % PAYLOAD_BITS_PER_NOTE;
        let carried = salts[slot as usize].get() & (1u128 << bit) != 0;
        if carried != mask_bit(&mask, position - V3_MASK_LO) {
            return Err(WireError::InvalidV3Envelope);
        }
    }

    let mut plaintext = [0u8; V3_PLAINTEXT_BYTES];
    plaintext.copy_from_slice(&payload[V3_HEADER_BYTES..V3_HEADER_BYTES + V3_PLAINTEXT_BYTES]);
    let tag = Tag::from_slice(&payload[V3_HEADER_BYTES + V3_PLAINTEXT_BYTES..]);
    let nonce = derive_v3_nonce(context, deal_key);
    let cipher =
        Aes256GcmSiv::new_from_slice(deal_key).expect("AES-256-GCM-SIV accepts every 32-byte key");
    cipher
        .decrypt_in_place_detached(
            Nonce::from_slice(&nonce),
            &associated_data_v3(context, deal_id, &header),
            &mut plaintext,
            tag,
        )
        .map_err(|_| WireError::Authentication)?;

    unpack_message_v3(deal_id, &plaintext)
}

/// Encodes the public four-note wire for compatibility vectors only.
///
/// New channels must never call this; it exists so the historical format remains pinned.
pub fn encode_legacy_message(
    message: &WireMessage,
) -> Result<[NoteSalt; LEGACY_NOTES_PER_MESSAGE], WireError> {
    let bits = pack_message(message)?;
    let mut salts = Vec::with_capacity(LEGACY_NOTES_PER_MESSAGE);
    for slot in 0..LEGACY_NOTES_PER_MESSAGE {
        let mut chunk = 0u128;
        for j in 0..PAYLOAD_BITS_PER_NOTE {
            if bits.at(slot as u32 * PAYLOAD_BITS_PER_NOTE + j) {
                chunk |= 1u128 << j;
            }
        }
        salts.push(NoteSalt::new(chunk | FLAG_BIT)?);
    }
    Ok([salts[0], salts[1], salts[2], salts[3]])
}

/// Decodes a historical public four-note message.
pub fn decode_legacy_message(
    salts: &[NoteSalt; LEGACY_NOTES_PER_MESSAGE],
) -> Result<WireMessage, WireError> {
    let mut chunks = [0u128; LEGACY_NOTES_PER_MESSAGE];
    for (slot, salt) in salts.iter().enumerate() {
        let value = salt.get();
        if value & FLAG_BIT == 0 {
            return Err(WireError::MissingFlag(slot));
        }
        chunks[slot] = value & PAYLOAD_MASK;
    }
    unpack_message(&Bits::from_legacy_chunks(chunks))
}
