//! Note-index allocation within a subchannel.
//!
//! ## Why this exists
//!
//! The pool enforces two rules on every note index, at three call sites each for channels,
//! subchannels and notes. Both are checked *after* a proof has been generated and paid for,
//! so getting either wrong costs ~29 s and a proving fee to learn something the client
//! already had enough information to know.
//!
//! 1. **Contiguity.** `privacy.cairo:737-746` asserts `index == 0 || note[index - 1] exists`,
//!    reverting with `INDEX_NOT_SEQUENTIAL`. Note that it checks *only the immediate
//!    predecessor* — it is a predecessor-exists test, not a scan of the whole run. That is
//!    enough to make gaps unreachable, because every index had to pass the same test on the
//!    way up.
//! 2. **Write-once.** `_apply_write_once` (`privacy.cairo:932-946`) reads each target slot
//!    and asserts it is zero before writing, reverting with `NON_ZERO_VALUE`. So an index
//!    cannot be reused, even by its original writer.
//!
//! Together those make the index space an **allocator**: hand out contiguously, never twice.
//! Nothing in the SDK was doing that. Both `write_message` and `accept_and_settle` took a
//! caller-supplied index and trusted it, which means every caller was independently
//! responsible for reproducing two contract invariants from memory.
//!
//! [`SubchannelCursor`] is that allocator. One per `(channel_key, token)` pair, because a
//! subchannel *is* a token and the index space is per subchannel.
//!
//! ## What this deliberately does not do
//!
//! It does not talk to the chain. A cursor is a local model of what this agent believes it
//! has written; [`SubchannelCursor::resume_at`] is how a returning agent reseats it from
//! observed state. If the local model and the chain disagree, the chain wins and the revert
//! is the symptom — this type narrows the window, it does not close it.

use core::ops::Range;

use crate::wire::NOTES_PER_MESSAGE;

/// Errors from index allocation.
///
/// Both variants name the contract error they prevent, so a failure here reads as "this
/// would have reverted with X" rather than as an SDK-internal complaint.
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
    /// A message was requested at an index that is not on the four-note grid.
    ///
    /// The wire format puts message `k` at notes `4k..4k+3` so a reader can seek without a
    /// framing search. Once an allocation lands off that grid, every later message is
    /// misaligned and the reader silently reassembles garbage.
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
/// Tracks the next free note index. Every write goes through it, so contiguity and
/// single-use hold by construction rather than by each call site remembering to check.
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
    /// `next` is the first index the agent believes is unwritten. A returning agent gets
    /// this by reading its own subchannel; there is no way to derive it locally, because
    /// the counterparty cannot write here but this agent may have written from another
    /// process or a previous run.
    pub fn resume_at(next: u32) -> Self {
        Self { next }
    }

    /// The next index this subchannel will accept.
    pub fn next_index(&self) -> u32 {
        self.next
    }

    /// The message index a new message would occupy, if the cursor is on the grid.
    ///
    /// Errors with [`IndexError::Misaligned`] rather than rounding. Rounding would skip an
    /// index and hit `INDEX_NOT_SEQUENTIAL`; truncating would overwrite and hit
    /// `NON_ZERO_VALUE`. There is no safe repair, so the caller has to know.
    pub fn next_message_index(&self) -> Result<u32, IndexError> {
        if !self.next.is_multiple_of(NOTES_PER_MESSAGE as u32) {
            return Err(IndexError::Misaligned { next: self.next });
        }
        Ok(self.next / NOTES_PER_MESSAGE as u32)
    }

    /// Check that `index` is exactly the next free one, without consuming it.
    ///
    /// This is the client-side mirror of both contract rules: anything below `next` is a
    /// write-once violation, anything above leaves a gap.
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
    /// Returns the *message* index, which is what the wire format and the reader speak in.
    pub fn reserve_message(&mut self) -> Result<u32, IndexError> {
        let message_index = self.next_message_index()?;
        self.reserve(NOTES_PER_MESSAGE as u32)?;
        Ok(message_index)
    }

    /// Reserve a single note index — the settlement payment note.
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
            IndexError::Misaligned { next: 5 }
        );
    }

    #[test]
    fn a_failed_reservation_leaves_the_cursor_where_it_was() {
        let mut cursor = SubchannelCursor::resume_at(5);
        assert!(cursor.reserve_message().is_err());
        assert_eq!(cursor.next_index(), 5, "cursor moved on a failed reserve");
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
