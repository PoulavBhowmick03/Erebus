//! Wire format v1 — negotiation messages carried in note salts.
//!
//! Port of `sdk/ts/src/channel/wire.ts`. The TypeScript is the oracle: this format is
//! ours, so Cairo emits no reference vector and the only way to know the two agree is to
//! diff them (`tests/fixtures/ts-wire-salts.json`).
//!
//! ## The mechanism
//!
//! A pool note has no payload field. Its only client-writable space is the salt, which the
//! contract constrains to `2 <= salt < 2^120` and stores verbatim in the high 120 bits of
//! `packed_value`. Negotiation state rides there, in zero-amount notes that move no value.
//!
//! Bit 119 is pinned to 1 and payload occupies bits 0-118. A chunk that happened to come
//! out as 0 or 1 would be rejected by the contract (`ZERO_SALT` / `SALT_TOO_SMALL`) —
//! rare enough to survive testing and unpleasant to diagnose. Pinning the top bit puts
//! every salt in `[2^119, 2^120)` unconditionally.
//!
//! ## Layout
//!
//! Fields are packed most-significant-first into a 400-bit integer, then split into four
//! 119-bit chunks with **chunk 0 holding the least significant bits**:
//!
//! ```text
//!   type:8 | replyTo:32 | createdAt:40 | amount:128 | deadline:64 | memoHash:128
//!   \_______________________________ 400 bits _______________________________/
//!
//!   salts[3] = bits 357..399   (43 significant: type, replyTo, top of createdAt)
//!   salts[2] = bits 238..356
//!   salts[1] = bits 119..237
//!   salts[0] = bits   0..118   (low bits of memoHash)
//! ```
//!
//! Note the header lands in `salts[3]`, not `salts[0]`. The ASCII table in the TypeScript
//! module docs claims otherwise and is wrong — the code shifts `type` in first, so it ends
//! up most significant. Trust this table; it was derived from the emitted vectors.
//!
//! Fixed stride: message `k` occupies note indices `4k .. 4k+3`, so a reader seeks
//! directly and never scans for framing.
//!
//! ## Which notes may use this
//!
//! Structured salts go on **zero-amount** notes only. A value-bearing note must keep a
//! random salt, because the salt is the one-time-pad nonce for the encrypted amount:
//! reusing a mask across two notes with different amounts lets an observer subtract the
//! ciphertexts and recover the difference. Zero-amount notes have no variance to leak.
//!
//! This module cannot enforce that on its own — it only produces salts. The pairing is
//! enforced where notes are built.

use starknet_types_core::felt::Felt;

use crate::actions::{ActionError, NoteSalt};

/// Notes per negotiation message.
pub const NOTES_PER_MESSAGE: usize = 4;
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
/// Bits available across the four notes.
pub const CAPACITY_BITS: u32 = NOTES_PER_MESSAGE as u32 * PAYLOAD_BITS_PER_NOTE;

const FLAG_BIT: u128 = 1u128 << 119;
const PAYLOAD_MASK: u128 = FLAG_BIT - 1;

/// `2^32 - 1`, reserved to mean "no reply", so it is not a usable message index.
const NO_REPLY_TO: u32 = u32::MAX;

const _: () = assert!(
    MESSAGE_BITS <= CAPACITY_BITS,
    "wire layout does not fit the notes allocated for it"
);

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
    #[error("salt at slot {0} is missing the format flag — not an Erebus data note")]
    MissingFlag(usize),
    /// The type code did not name a known message type.
    #[error("unknown message type code: {0}")]
    UnknownType(u8),
    /// A produced salt fell outside the contract's accepted range.
    #[error(transparent)]
    Salt(#[from] ActionError),
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

/// A negotiation message as it goes on the wire.
///
/// `token` and `nonce` from `OfferTerms` are deliberately absent: a subchannel *is* a
/// token, so both parties already know it, and the note index already orders messages and
/// makes each unique. Dropping them is what brings the payload inside four notes.
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
/// This drops 124 bits, leaving 2^64 collision resistance under birthday bounds. The memo
/// hash commits to detail held off-chain; it is not a capability and nothing is spent on
/// it, so grinding a collision buys an attacker only the ability to claim a different memo
/// matched an agreed offer.
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

    fn from_chunks(chunks: [u128; NOTES_PER_MESSAGE]) -> Self {
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

/// Encodes a message into exactly [`NOTES_PER_MESSAGE`] salts, in note-index order.
pub fn encode_message(message: &WireMessage) -> Result<[NoteSalt; NOTES_PER_MESSAGE], WireError> {
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

    let mut salts = Vec::with_capacity(NOTES_PER_MESSAGE);
    for slot in 0..NOTES_PER_MESSAGE {
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

/// Inverse of [`encode_message`]. Salts must be in note-index order.
pub fn decode_message(salts: &[NoteSalt; NOTES_PER_MESSAGE]) -> Result<WireMessage, WireError> {
    let mut chunks = [0u128; NOTES_PER_MESSAGE];
    for (slot, salt) in salts.iter().enumerate() {
        let value = salt.get();
        if value & FLAG_BIT == 0 {
            return Err(WireError::MissingFlag(slot));
        }
        chunks[slot] = value & PAYLOAD_MASK;
    }

    let bits = Bits::from_chunks(chunks);
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
