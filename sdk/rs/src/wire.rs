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
//! Wire v2 encrypts these 50 bytes with AES-256-GCM-SIV. The ciphertext and 16-byte tag use
//! 528 of the 595 payload bits. An 8-bit version marker follows. The remaining 59 high bits
//! must be zero. Chunk 0 holds the least-significant 119 bits, as it does in wire v1.
//!
//! Message `k` uses note indices `5k .. 5k+4`. This fixed stride lets a reader find each
//! message without a framing scan.
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

/// `2^32 - 1`, reserved to mean "no reply", so it is not a usable message index.
const NO_REPLY_TO: u32 = u32::MAX;

const _: () = assert!(
    MESSAGE_BITS <= CAPACITY_BITS,
    "wire layout does not fit the notes allocated for it"
);
const _: () = assert!(MESSAGE_BITS <= LEGACY_CAPACITY_BITS);
const _: () = assert!(V2_PAYLOAD_BITS <= CAPACITY_BITS);

/// Negotiation wire generation stored with a channel.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireVersion {
    /// Public four-note wire retained for historical reads only.
    #[default]
    V1,
    /// Five-note authenticated-encryption wire.
    V2,
}

impl WireVersion {
    /// Number of consecutive note slots occupied by one message.
    pub const fn notes_per_message(self) -> usize {
        match self {
            Self::V1 => LEGACY_NOTES_PER_MESSAGE,
            Self::V2 => NOTES_PER_MESSAGE,
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
    #[error("wire v1 is read-only; open a wire-v2 channel before writing")]
    LegacyReadOnly,
    /// The authenticated ciphertext was changed or decoded under the wrong context.
    #[error("wire-v2 authentication failed")]
    Authentication,
    /// The five chunks did not carry the wire-v2 marker and canonical zero padding.
    #[error("invalid wire-v2 marker or padding")]
    InvalidV2Envelope,
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
    let bytes = memo_hash.to_bytes_le();
    let mut low = [0u8; 16];
    low.copy_from_slice(&bytes[..16]);
    u128::from_le_bytes(low)
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

fn associated_data(context: &WireContext) -> Vec<u8> {
    let mut data = b"EREBUS_WIRE_V2_AAD".to_vec();
    data.extend_from_slice(&context.chain_id.to_bytes_be());
    data.extend_from_slice(&context.pool_address.to_bytes_be());
    data.extend_from_slice(&context.token.to_bytes_be());
    data.extend_from_slice(&context.message_index.to_be_bytes());
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
