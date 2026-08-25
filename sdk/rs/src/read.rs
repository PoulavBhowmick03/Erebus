//! Reads a negotiation from the pool.
//!
//! The write side stores a [`WireMessage`] in five encrypted zero-amount notes. This module
//! authenticates and decrypts them. An explicit wire version supports legacy four-note
//! transcripts.
//!
//! ## Reads are keyed, never scanned
//!
//! Each note uses `h(NOTE_ID_TAG, channel_key, token, index)` as its location. The reader
//! computes the exact slot from the key and index. It never scans pool storage
//! (CLAUDE.md constraint 3).
//!
//! ## Where the chain would go
//!
//! [`NoteSource`] supplies the value for one note id. An offline source can use a map.
//! Sepolia can use a storage read or the Discovery Service. The trait supports offline
//! transcript tests.
//!
//! ## Termination
//!
//! A transcript ends at the first missing note. `INDEX_NOT_SEQUENTIAL` prevents gaps, so a
//! missing note marks the end. See [`crate::subchannel`].

use starknet_types_core::felt::Felt;

use crate::actions::NoteSalt;
use crate::decrypt;
use crate::hashes;
use crate::negotiation::{Author, NegotiationError, OfferBook};
use crate::wire::{
    decode_legacy_message, decode_message, decode_message_v3, MessageType, WireContext, WireError,
    WireMessage, WireVersion, LEGACY_NOTES_PER_MESSAGE, NOTES_PER_MESSAGE,
};

/// Errors from reading.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReadError {
    /// The serialized viewing grant was corrupted or edited.
    #[error("viewing grant integrity check failed")]
    InvalidViewingGrant,
    /// A partly present message, caused by an incomplete source or an incorrect channel key.
    #[error("message {message_index} is incomplete: {found} of {expected} notes present")]
    PartialMessage {
        /// The message that was torn.
        message_index: u32,
        /// How many of its notes were found.
        found: usize,
        /// Number required by this channel's wire generation.
        expected: usize,
    },
    /// A wire-v3 acceptance frame did not contain its required payment note.
    #[error("acceptance frame at note {frame_start} has no payment note at {payment_index}")]
    MissingSettlementPayment {
        /// Physical start of the acceptance frame.
        frame_start: u32,
        /// Required payment-note index.
        payment_index: u64,
    },
    /// The notes were found but did not decode.
    #[error(transparent)]
    Wire(#[from] WireError),
    /// A note with a salt outside the message range, usually a value note or wrong key.
    #[error("note {slot} of message {message_index} is not an Erebus data note")]
    NotAnErebusNote {
        /// The message being read.
        message_index: u32,
        /// Which of its notes was wrong.
        slot: usize,
    },
    /// The decoded transcript violates the state machine.
    #[error(transparent)]
    Negotiation(#[from] NegotiationError),
}

/// Where stored note values come from.
///
/// Implement this source with a storage read, the Discovery Service, or a test map.
pub trait NoteSource {
    /// The `packed_value` stored at `note_id`, or `None` if the slot is empty.
    fn packed_value(&self, note_id: Felt) -> Option<Felt>;
}

impl<F> NoteSource for F
where
    F: Fn(Felt) -> Option<Felt>,
{
    fn packed_value(&self, note_id: Felt) -> Option<Felt> {
        self(note_id)
    }
}

/// A message recovered from the pool, with where it sat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadMessage {
    /// Which message in the subchannel this was.
    pub message_index: u32,
    /// The decoded message.
    pub message: WireMessage,
}

/// A reader for one direction of one subchannel.
///
/// Holds one channel key and no pool private key. A holder can read this channel but cannot
/// derive another channel.
#[derive(Clone, Copy)]
pub struct ChannelReader {
    chain_id: Felt,
    pool_address: Felt,
    channel_key: Felt,
    token: Felt,
    wire_version: WireVersion,
}

/// Redacts the key because it gives read access to the full channel.
impl core::fmt::Debug for ChannelReader {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ChannelReader")
            .field("chain_id", &self.chain_id)
            .field("pool_address", &self.pool_address)
            .field("channel_key", &"<redacted>")
            .field("token", &self.token)
            .field("wire_version", &self.wire_version)
            .finish()
    }
}

impl ChannelReader {
    /// A reader for `channel_key`'s subchannel on `token`.
    pub fn new(chain_id: Felt, pool_address: Felt, channel_key: Felt, token: Felt) -> Self {
        Self::with_version(chain_id, pool_address, channel_key, token, WireVersion::V3)
    }

    /// A reader for an explicitly versioned historical channel.
    pub fn with_version(
        chain_id: Felt,
        pool_address: Felt,
        channel_key: Felt,
        token: Felt,
        wire_version: WireVersion,
    ) -> Self {
        Self {
            chain_id,
            pool_address,
            channel_key,
            token,
            wire_version,
        }
    }

    /// Note ids for one versioned message.
    /// The channel key this reader locates notes with.
    ///
    /// Exposed so a caller can scope a cache to the same subchannel a note id derives from.
    /// It is a secret: anything that stores or logs it is disclosing the channel.
    pub fn channel_key(&self) -> Felt {
        self.channel_key
    }

    /// Note ids for one versioned message.
    pub fn note_ids(&self, message_index: u32) -> Vec<Felt> {
        let width = self.wire_version.notes_per_message();
        let first = match self.wire_version {
            WireVersion::V3 => u64::from(message_index),
            WireVersion::V1 | WireVersion::V2 => u64::from(message_index) * width as u64,
        };
        (0..width)
            .map(|slot| hashes::compute_note_id(self.channel_key, self.token, first + slot as u64))
            .collect()
    }

    /// Storage id for one note index.
    pub fn note_id(&self, index: u64) -> Felt {
        hashes::compute_note_id(self.channel_key, self.token, index)
    }

    /// Reads one note, decrypting its amount.
    pub fn note(&self, index: u64, source: &impl NoteSource) -> Option<decrypt::NoteView> {
        let note_id = self.note_id(index);
        let packed = source.packed_value(note_id)?;
        Some(decrypt::packed_value(
            packed,
            self.channel_key,
            self.token,
            index,
        ))
    }

    /// Reads one negotiation message.
    ///
    /// Returns `Ok(None)` when no message exists. A partial message is an error because the
    /// pool cannot contain a gap.
    pub fn message(
        &self,
        message_index: u32,
        source: &impl NoteSource,
    ) -> Result<Option<WireMessage>, ReadError> {
        let ids = self.note_ids(message_index);

        // A settlement payment follows its acceptance where the next message would start.
        // Treat the value note as the end of the transcript, not a partial message.
        let width = self.wire_version.notes_per_message();
        let first = match self.wire_version {
            WireVersion::V3 => u64::from(message_index),
            WireVersion::V1 | WireVersion::V2 => u64::from(message_index) * width as u64,
        };
        if let Some(note) = self.note(first, source) {
            if note.is_value_note() {
                return Ok(None);
            }
        }

        let found: Vec<Felt> = ids
            .iter()
            .filter_map(|id| source.packed_value(*id))
            .collect();

        if found.is_empty() {
            return Ok(None);
        }
        if found.len() != width {
            return Err(ReadError::PartialMessage {
                message_index,
                found: found.len(),
                expected: width,
            });
        }

        // The salt is the public high half of `packed_value`. The range check rejects value
        // notes before wire v2 authenticates and decrypts the message.
        let salts: Vec<NoteSalt> = found
            .iter()
            .enumerate()
            .map(|(slot, packed)| {
                NoteSalt::new(decrypt::unpack_note(*packed).0).map_err(|_| {
                    ReadError::NotAnErebusNote {
                        message_index,
                        slot,
                    }
                })
            })
            .collect::<Result<_, _>>()?;

        let message = match self.wire_version {
            WireVersion::V1 => {
                let salts: [NoteSalt; LEGACY_NOTES_PER_MESSAGE] = salts
                    .try_into()
                    .expect("wire-v1 width checked before conversion");
                decode_legacy_message(&salts)?
            }
            version @ (WireVersion::V2 | WireVersion::V3) => {
                let salts: [NoteSalt; NOTES_PER_MESSAGE] = salts
                    .try_into()
                    .expect("wire-v2 width checked before conversion");
                let context = WireContext {
                    chain_id: self.chain_id,
                    pool_address: self.pool_address,
                    channel_key: self.channel_key,
                    token: self.token,
                    message_index,
                };
                // Not a fallback chain: a channel's version is recorded when it opens, and
                // trying the other decoder on failure would turn a corrupted note into a
                // silent version guess. Each version decodes only its own wire.
                if version == WireVersion::V3 {
                    decode_message_v3(&context, &salts)?
                } else {
                    decode_message(&context, &salts)?
                }
            }
        };
        Ok(Some(message))
    }

    /// Reads every message in this direction, stopping at the first absent one.
    ///
    /// Sound because the pool cannot contain a gap: a missing note is the end of the run,
    /// never a hole. See [`crate::subchannel`].
    pub fn transcript(&self, source: &impl NoteSource) -> Result<Vec<ReadMessage>, ReadError> {
        let mut messages = Vec::new();
        let mut message_index = 0u32;

        while let Some(message) = self.message(message_index, source)? {
            messages.push(ReadMessage {
                message_index,
                message,
            });
            if self.wire_version == WireVersion::V3 {
                let frame_width = if message.message_type == MessageType::Accept {
                    let payment_index = u64::from(message_index) + NOTES_PER_MESSAGE as u64;
                    if self
                        .note(payment_index, source)
                        .filter(decrypt::NoteView::is_value_note)
                        .is_none()
                    {
                        return Err(ReadError::MissingSettlementPayment {
                            frame_start: message_index,
                            payment_index,
                        });
                    }
                    NOTES_PER_MESSAGE as u32 + 1
                } else {
                    NOTES_PER_MESSAGE as u32
                };
                message_index = message_index
                    .checked_add(frame_width)
                    .ok_or(ReadError::Wire(WireError::FieldTooWide {
                        field: "frameStart",
                        bits: 32,
                    }))?;
            } else {
                message_index += 1;
            }
        }
        Ok(messages)
    }

    /// The settlement payment note, if this channel was settled.
    ///
    /// The payment follows the acceptance record. See `Channel::settle_next`. A value
    /// distinguishes it from zero-amount negotiation notes.
    pub fn settlement_note(
        &self,
        acceptance_index: u32,
        source: &impl NoteSource,
    ) -> Option<decrypt::NoteView> {
        let index = match self.wire_version {
            WireVersion::V3 => u64::from(acceptance_index) + NOTES_PER_MESSAGE as u64,
            WireVersion::V1 | WireVersion::V2 => {
                u64::from(acceptance_index + 1) * self.wire_version.notes_per_message() as u64
            }
        };
        self.note(index, source)
            .filter(decrypt::NoteView::is_value_note)
    }
}

/// Reconstructs both directions of a negotiation into one ordered [`OfferBook`].
///
/// A negotiation uses one subchannel per direction. This function combines messages by
/// `created_at`. Per-direction note indices do not define cross-direction order.
pub fn reconstruct(
    ours: &ChannelReader,
    theirs: &ChannelReader,
    source: &impl NoteSource,
) -> Result<OfferBook, ReadError> {
    let mut all: Vec<(Author, ReadMessage)> = Vec::new();
    for message in ours.transcript(source)? {
        all.push((Author::Us, message));
    }
    for message in theirs.transcript(source)? {
        all.push((Author::Counterparty, message));
    }

    all.sort_by_key(|(_, read)| (read.message.created_at, read.message_index));

    let mut book = OfferBook::new();
    while !all.is_empty() {
        let ready = all.iter().position(|(author, read)| {
            read.message.reply_to.is_none_or(|reply_to| {
                let target = crate::negotiation::OfferId::new(author.opposite(), reply_to);
                book.entries().any(|(id, _)| id == target)
            })
        });
        let position = ready.unwrap_or(0);
        let (author, read) = all.remove(position);
        book.record(read.message_index, author, read.message)?;
    }
    Ok(book)
}
