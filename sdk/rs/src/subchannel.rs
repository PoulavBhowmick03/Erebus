//! Note-index allocation within a subchannel.
//!
//! The pool checks two index rules after proof generation. A failure costs ~29 s and a
//! proving fee, so this module checks them first.
//!
//! 1. `privacy.cairo:737-746` requires `index == 0 || note[index - 1] exists`. A missing
//!    predecessor returns `INDEX_NOT_SEQUENTIAL`. Checking only the immediate predecessor is
//!    enough because each earlier index passed the same check.
//! 2. `_apply_write_once` (`privacy.cairo:932-946`) requires a zero target slot. Reusing an
//!    index returns `NON_ZERO_VALUE`, including reuse by the original writer.
//!
//! [`SubchannelCursor`] allocates contiguous indices once for each `(channel_key, token)`
//! pair. Each token subchannel has its own index space.
//!
//! ## Chain state
//!
//! A cursor is a local model and does not read the chain. A returning agent uses
//! [`SubchannelCursor::resume_at`] with observed state. An incorrect local cursor can still
//! cause a contract revert.

use core::ops::Range;

use crate::wire::NOTES_PER_MESSAGE;

/// Errors from index allocation.
///
/// Each failure names the corresponding contract error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IndexError {
    /// Writing here would leave a gap. On-chain this is `INDEX_NOT_SEQUENTIAL`.
    #[error(
        "note index {index} skips {}: the pool would revert with INDEX_NOT_SEQUENTIAL",
        if *next == 0 { "the whole subchannel".to_string() } else { format!("{}..{index}", *next) }
    )]
    NotSequential {
        /// The index the caller asked for.
        index: u32,
        /// The next index the subchannel will accept.
        next: u32,
    },
    /// Writing here would overwrite. On-chain this is `NON_ZERO_VALUE` from write-once.
    #[error("note index {index} was already written: the pool would revert with NON_ZERO_VALUE")]
    AlreadyWritten {
        /// The index the caller asked for.
        index: u32,
    },
    /// The index space ran out.
    #[error("subchannel index space exhausted at {index}")]
    Exhausted {
        /// The index that overflowed.
        index: u32,
    },
    /// A message was requested at an index that is not on the wire-v2 five-note grid.
    ///
    /// Message `k` uses notes `5k..5k+4`. An off-grid allocation misaligns every later
    /// message and makes the reader assemble incorrect data.
    #[error(
        "next free index {next} is not on the {NOTES_PER_MESSAGE}-note message grid; \
         a message cannot start here without breaking reader alignment"
    )]
    Misaligned {
        /// The next free index, which is not a multiple of [`NOTES_PER_MESSAGE`].
        next: u32,
    },
}

/// The index allocator for one subchannel.
///
/// Tracks the next free note index and enforces contiguous, single-use allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SubchannelCursor {
    next: u32,
}

impl SubchannelCursor {
    /// A cursor for a freshly opened subchannel. Nothing written, next index 0.
    pub fn new() -> Self {
        Self { next: 0 }
    }

    /// Reseat a cursor from observed chain state.
    ///
    /// `next` is the first index that the agent believes is unwritten. Recover it from the
    /// agent's subchannel after another process or earlier run changes chain state.
    pub fn resume_at(next: u32) -> Self {
        Self { next }
    }

    /// The next index this subchannel will accept.
    pub fn next_index(&self) -> u32 {
        self.next
    }

    /// Index a new message would occupy. Requires the cursor on a message boundary.
    ///
    /// Returns [`IndexError::Misaligned`] instead of rounding. Rounding up skips an index and
    /// hits `INDEX_NOT_SEQUENTIAL`. Rounding down overwrites and hits `NON_ZERO_VALUE`.
    pub fn next_message_index(&self) -> Result<u32, IndexError> {
        if !self.next.is_multiple_of(NOTES_PER_MESSAGE as u32) {
            return Err(IndexError::Misaligned { next: self.next });
        }
        Ok(self.next / NOTES_PER_MESSAGE as u32)
    }

    /// Checks that `index` is the next free index without consuming it.
    ///
    /// An index below `next` violates write-once. An index above `next` leaves a gap.
    pub fn check(&self, index: u32) -> Result<(), IndexError> {
        match index.cmp(&self.next) {
            core::cmp::Ordering::Less => Err(IndexError::AlreadyWritten { index }),
            core::cmp::Ordering::Greater => Err(IndexError::NotSequential {
                index,
                next: self.next,
            }),
            core::cmp::Ordering::Equal => Ok(()),
        }
    }

    /// Reserve `count` contiguous indices, advancing the cursor.
    ///
    /// Advances only on success, so a failed reservation leaves the cursor usable.
    pub fn reserve(&mut self, count: u32) -> Result<Range<u32>, IndexError> {
        let first = self.next;
        let end = first
            .checked_add(count)
            .ok_or(IndexError::Exhausted { index: first })?;
        self.next = end;
        Ok(first..end)
    }

    /// Reserve one full message: [`NOTES_PER_MESSAGE`] indices on the grid.
    ///
    /// Returns the message index used by the wire format and reader.
    pub fn reserve_message(&mut self) -> Result<u32, IndexError> {
        let message_index = self.next_message_index()?;
        self.reserve(NOTES_PER_MESSAGE as u32)?;
        Ok(message_index)
    }

    /// Reserves one note index for a settlement payment.
    pub fn reserve_note(&mut self) -> Result<u32, IndexError> {
        Ok(self.reserve(1)?.start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_cursor_starts_at_zero() {
        assert_eq!(SubchannelCursor::new().next_index(), 0);
        assert_eq!(SubchannelCursor::new(), SubchannelCursor::default());
    }

    #[test]
    fn reserving_a_message_advances_by_the_note_count() {
        let mut cursor = SubchannelCursor::new();
        assert_eq!(cursor.reserve_message().expect("grid"), 0);
        assert_eq!(cursor.next_index(), NOTES_PER_MESSAGE as u32);
        assert_eq!(cursor.reserve_message().expect("grid"), 1);
        assert_eq!(cursor.next_index(), 2 * NOTES_PER_MESSAGE as u32);
    }

    #[test]
    fn a_single_note_knocks_the_cursor_off_the_message_grid() {
        let mut cursor = SubchannelCursor::new();
        cursor.reserve_message().expect("grid");
        cursor.reserve_note().expect("space");

        assert_eq!(
            cursor.reserve_message().unwrap_err(),
            IndexError::Misaligned {
                next: NOTES_PER_MESSAGE as u32 + 1,
            }
        );
    }

    #[test]
    fn a_failed_reservation_leaves_the_cursor_where_it_was() {
        let start = NOTES_PER_MESSAGE as u32 + 1;
        let mut cursor = SubchannelCursor::resume_at(start);
        assert!(cursor.reserve_message().is_err());
        assert_eq!(
            cursor.next_index(),
            start,
            "cursor moved on a failed reserve"
        );
    }

    #[test]
    fn exhaustion_is_an_error_rather_than_a_wrap() {
        let mut cursor = SubchannelCursor::resume_at(u32::MAX);
        assert_eq!(
            cursor.reserve(2).unwrap_err(),
            IndexError::Exhausted { index: u32::MAX }
        );
    }
}
