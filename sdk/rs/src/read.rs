//! Reading a negotiation back out of the pool.
//!
//! The write side turns a [`WireMessage`] into four zero-amount notes. This turns four
//! notes back into a message, and a run of messages back into a transcript.
//!
//! ## Reads are keyed, never scanned
//!
//! Every note's location is computed: `h(NOTE_ID_TAG, channel_key, token, index)`. The
//! reader knows the channel key and the index, so it seeks the exact slot. Nothing here
//! enumerates, filters, or searches — scanning defeats the discovery design and does not
//! work at any real pool size (CLAUDE.md constraint 3).
//!
//! ## Where the chain would go
//!
//! [`NoteSource`] is the seam. It answers "what is stored at this note id", and that is the
//! *only* thing the read path needs from the outside world. Offline it is a map; against
//! Sepolia it is a storage read or the Discovery Service. Keeping it a trait means the
//! whole transcript path is testable today, with the chain still unreachable.
//!
//! ## Termination
//!
//! A transcript ends at the first missing note, which is sound precisely because of the
//! contiguity rule (`INDEX_NOT_SEQUENTIAL`, see [`crate::subchannel`]): the pool cannot
//! contain a gap, so a missing note means the end and never a hole in the middle.

use starknet_types_core::felt::Felt;

use crate::decrypt;
use crate::hashes;
use crate::negotiation::{Author, NegotiationError, OfferBook};
use crate::actions::NoteSalt;
use crate::wire::{decode_message, WireError, WireMessage, NOTES_PER_MESSAGE};

/// Errors from reading.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReadError {
    /// A message was partly present. Under the contiguity rule this cannot happen on-chain,
    /// so it means the source is inconsistent — a partial fetch, or the wrong channel key.
    #[error(
        "message {message_index} is incomplete: {found} of {NOTES_PER_MESSAGE} notes present"
    )]
    PartialMessage {
        /// The message that was torn.
        message_index: u32,
        /// How many of its notes were found.
        found: usize,
    },
    /// The notes were found but did not decode.
    #[error(transparent)]
    Wire(#[from] WireError),
    /// A note in the run carried a salt outside the structured range, so it is not part of
    /// an Erebus message — most likely a value note, or the wrong channel key.
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
/// The single dependency the read path has on the outside world. Implement it over a
/// storage read, the Discovery Service, or a map in a test.
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
/// Holds a channel key and nothing else — no pool private key. That is the whole basis of
/// scoped disclosure: everything below can be done by anyone handed this one secret, and
/// it reveals this channel and no other.
#[derive(Clone, Copy)]
pub struct ChannelReader {
    channel_key: Felt,
    token: Felt,
}

/// Redacts the channel key — it is the disclosure secret for an entire channel.
impl core::fmt::Debug for ChannelReader {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ChannelReader")
            .field("channel_key", &"<redacted>")
            .field("token", &self.token)
            .finish()
    }
}

impl ChannelReader {
    /// A reader for `channel_key`'s subchannel on `token`.
    pub fn new(channel_key: Felt, token: Felt) -> Self {
        Self { channel_key, token }
    }

    /// Note ids for the four notes of `message_index`.
    pub fn note_ids(&self, message_index: u32) -> [Felt; NOTES_PER_MESSAGE] {
        let first = u64::from(message_index) * NOTES_PER_MESSAGE as u64;
        core::array::from_fn(|slot| {
            hashes::compute_note_id(self.channel_key, self.token, first + slot as u64)
        })
    }

    /// Reads one note, decrypting its amount.
    pub fn note(&self, index: u64, source: &impl NoteSource) -> Option<decrypt::NoteView> {
        let note_id = hashes::compute_note_id(self.channel_key, self.token, index);
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
    /// Returns `Ok(None)` when the message is simply not there yet, which is the ordinary
    /// "nothing new" answer a polling agent gets. A *partly* present message is an error,
    /// not a `None` — contiguity means the pool cannot hold one.
    pub fn message(
        &self,
        message_index: u32,
        source: &impl NoteSource,
    ) -> Result<Option<WireMessage>, ReadError> {
        let ids = self.note_ids(message_index);
        let found: Vec<Felt> = ids.iter().filter_map(|id| source.packed_value(*id)).collect();

        if found.is_empty() {
            return Ok(None);
        }
        if found.len() != NOTES_PER_MESSAGE {
            return Err(ReadError::PartialMessage {
                message_index,
                found: found.len(),
            });
        }

        // The salt is the high half of the packed value — the payload needs no decryption,
        // only the location did. Range-checking through `NoteSalt` is not ceremony here: a
        // reader aimed at the wrong slots picks up random salts from value notes, and this
        // is where that surfaces as an error rather than as a decoded-looking message.
        let mut salts = [NoteSalt::new(2).expect("2 is in range"); NOTES_PER_MESSAGE];
        for (slot, packed) in found.iter().enumerate() {
            salts[slot] = NoteSalt::new(decrypt::unpack_note(*packed).0)
                .map_err(|_| ReadError::NotAnErebusNote { message_index, slot })?;
        }
        Ok(Some(decode_message(&salts)?))
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
            message_index += 1;
        }
        Ok(messages)
    }

    /// The settlement payment note, if this channel was settled.
    ///
    /// Lives at the index directly after the acceptance record, which is the only place it
    /// can be — see `Channel::settle_next`. Identified by carrying value; every negotiation
    /// note is zero-amount by construction.
    pub fn settlement_note(
        &self,
        acceptance_index: u32,
        source: &impl NoteSource,
    ) -> Option<decrypt::NoteView> {
        let index = u64::from(acceptance_index + 1) * NOTES_PER_MESSAGE as u64;
        self.note(index, source).filter(decrypt::NoteView::is_value_note)
    }
}

/// Reconstructs both directions of a negotiation into one ordered [`OfferBook`].
///
/// Channels are directional, so a negotiation is two subchannels: what we wrote and what
/// they wrote. Neither alone is the conversation. Messages are interleaved by `created_at`,
/// which is the only ordering both sides agree on — note indices are per-direction and say
/// nothing about cross-direction order.
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
    for (author, read) in all {
        book.record(read.message_index, author, read.message)?;
    }
    Ok(book)
}
