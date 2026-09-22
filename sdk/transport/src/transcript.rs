//! Per-deal transcript: per-author hash chains, ordering, and the root the agreement commits to
//! (decision DM2-4).
//!
//! Two authors write concurrently, so there is no natural total order across directions. Each
//! author's messages form their own chain, and the root hashes the two heads in a fixed role
//! order:
//!
//! ```text
//! message_digest_i = Hash_suite("EREBUS_MESSAGE_V1" || encode(body_i))
//! link_0           = 0x00..00
//! link_i           = Hash_suite("EREBUS_TRANSCRIPT_LINK_V1" || author_tag || link_{i-1} || message_digest_i)
//! transcript_root  = Hash_suite("EREBUS_TRANSCRIPT_ROOT_V1" || deal_id || head_buyer || head_seller)
//! ```
//!
//! The chain shape is what makes the ordering attacks unrepresentable rather than remembered. A
//! duplicate repeats a sequence; a reordering skips one; a fork claims a parent that is no longer
//! the head; a cross-deal message names a different deal. Each is rejected before it can change
//! the root.
//!
//! The root is all-zero only when the deal has no messages, which is the legacy and pre-M2 case
//! ([metropolis-agreement.md](../../docs/metropolis-agreement.md) field 6).

use erebus_core::auth::Role;
use erebus_core::suite::{self, SuiteError};

use crate::limits::MAX_MESSAGES_PER_DEAL;
use crate::message::{Message, MessageError};

/// Domain separation for a transcript chain link.
pub const TRANSCRIPT_LINK_DOMAIN: &[u8] = b"EREBUS_TRANSCRIPT_LINK_V1";
/// Domain separation for the transcript root.
pub const TRANSCRIPT_ROOT_DOMAIN: &[u8] = b"EREBUS_TRANSCRIPT_ROOT_V1";

/// One author's hash chain.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AuthorChain {
    head: [u8; 32],
    next_sequence: u64,
    count: usize,
}

impl AuthorChain {
    fn new() -> Self {
        Self {
            head: [0u8; 32],
            next_sequence: 1,
            count: 0,
        }
    }
}

/// A message was not a valid continuation of the transcript.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TranscriptError {
    /// The message named a different deal.
    #[error("message belongs to a different deal")]
    DealMismatch,
    /// The message sequence was not the next one for its author.
    #[error("{author} message sequence {actual} is not the expected {expected}")]
    SequenceOutOfOrder {
        /// The author.
        author: &'static str,
        /// The next sequence the chain required.
        expected: u64,
        /// The sequence supplied.
        actual: u64,
    },
    /// The message claimed a parent that is not the author's current chain head.
    #[error("{author} message parent does not match the current transcript head")]
    ParentMismatch {
        /// The author.
        author: &'static str,
    },
    /// The transcript reached its per-deal bound.
    #[error("transcript has reached the {0} message limit")]
    TranscriptFull(usize),
    /// The agreement suite is not implemented.
    #[error(transparent)]
    Suite(#[from] SuiteError),
    /// The message could not be digested.
    #[error(transparent)]
    Message(#[from] MessageError),
}

/// The authenticated message chain for one deal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcript {
    deal_id: [u8; 16],
    suite_id: u16,
    buyer: AuthorChain,
    seller: AuthorChain,
}

impl Transcript {
    /// Creates an empty transcript for a deal under an agreement suite.
    pub fn new(deal_id: [u8; 16], suite_id: u16) -> Result<Self, TranscriptError> {
        suite::suite(suite_id)?;
        Ok(Self {
            deal_id,
            suite_id,
            buyer: AuthorChain::new(),
            seller: AuthorChain::new(),
        })
    }

    /// The deal this transcript covers.
    #[must_use]
    pub fn deal_id(&self) -> [u8; 16] {
        self.deal_id
    }

    /// The agreement suite that hashes this transcript.
    #[must_use]
    pub fn suite_id(&self) -> u16 {
        self.suite_id
    }

    /// The next sequence expected from an author.
    #[must_use]
    pub fn next_sequence(&self, author: Role) -> u64 {
        self.chain(author).next_sequence
    }

    /// An author's current chain head.
    #[must_use]
    pub fn head(&self, author: Role) -> [u8; 32] {
        self.chain(author).head
    }

    /// The number of messages an author has contributed.
    #[must_use]
    pub fn count(&self, author: Role) -> usize {
        self.chain(author).count
    }

    /// The total number of messages.
    #[must_use]
    pub fn total(&self) -> usize {
        self.buyer.count + self.seller.count
    }

    /// Reports whether the deal has no messages and therefore a zero root.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }

    /// Appends a message after checking that it extends its author's chain.
    ///
    /// This is the only mutating operation, so every ordering guarantee is enforced here.
    pub fn append(&mut self, message: &Message) -> Result<(), TranscriptError> {
        if message.deal_id != self.deal_id {
            return Err(TranscriptError::DealMismatch);
        }
        if self.total() >= MAX_MESSAGES_PER_DEAL {
            return Err(TranscriptError::TranscriptFull(MAX_MESSAGES_PER_DEAL));
        }
        let suite = suite::suite(self.suite_id)?;
        let digest = message.digest(self.suite_id)?;
        let author = message.author;
        let chain = self.chain_mut(author);
        if message.sequence != chain.next_sequence {
            return Err(TranscriptError::SequenceOutOfOrder {
                author: author.name(),
                expected: chain.next_sequence,
                actual: message.sequence,
            });
        }
        if message.parent_hash != chain.head {
            return Err(TranscriptError::ParentMismatch {
                author: author.name(),
            });
        }
        let link = suite.hash(&[
            TRANSCRIPT_LINK_DOMAIN,
            &[author.tag()],
            &chain.head,
            &digest,
        ]);
        chain.head = link;
        chain.next_sequence += 1;
        chain.count += 1;
        Ok(())
    }

    /// Checks whether a message would be accepted, without changing the transcript.
    pub fn accepts(&self, message: &Message) -> Result<(), TranscriptError> {
        let mut probe = self.clone();
        probe.append(message)
    }

    /// The transcript root committed by an agreement revision.
    ///
    /// All-zero when the deal has no messages; the computed root otherwise. Both peers reach the
    /// same value from the same message set with no shared clock.
    pub fn root(&self) -> Result<[u8; 32], TranscriptError> {
        if self.is_empty() {
            return Ok([0u8; 32]);
        }
        let suite = suite::suite(self.suite_id)?;
        Ok(suite.hash(&[
            TRANSCRIPT_ROOT_DOMAIN,
            &self.deal_id,
            &self.buyer.head,
            &self.seller.head,
        ]))
    }

    /// Rebuilds a transcript by replaying stored messages in their original order.
    ///
    /// Appending is per-author and order-independent across authors, so a stored log replays to
    /// the same root after a restart.
    pub fn replay(
        deal_id: [u8; 16],
        suite_id: u16,
        messages: &[Message],
    ) -> Result<Self, TranscriptError> {
        let mut transcript = Self::new(deal_id, suite_id)?;
        for message in messages {
            transcript.append(message)?;
        }
        Ok(transcript)
    }

    fn chain(&self, author: Role) -> &AuthorChain {
        match author {
            Role::Buyer => &self.buyer,
            Role::Seller => &self.seller,
        }
    }

    fn chain_mut(&mut self, author: Role) -> &mut AuthorChain {
        match author {
            Role::Buyer => &mut self.buyer,
            Role::Seller => &mut self.seller,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::MessageType;

    const DEAL: [u8; 16] = [0x42; 16];

    fn message(author: Role, sequence: u64, parent_hash: [u8; 32], body: &[u8]) -> Message {
        Message::new(
            [0x01; 32],
            DEAL,
            1,
            author,
            sequence,
            parent_hash,
            MessageType::Offer,
            body.to_vec(),
        )
        .expect("valid message")
    }

    fn transcript() -> Transcript {
        Transcript::new(DEAL, 1).expect("suite 1")
    }

    #[test]
    fn both_authors_extend_their_own_chain() {
        let mut transcript = transcript();
        let buyer_first = message(Role::Buyer, 1, [0; 32], b"offer");
        transcript.append(&buyer_first).expect("buyer offer");
        let seller_first = message(Role::Seller, 1, [0; 32], b"counter");
        transcript.append(&seller_first).expect("seller counter");
        assert_eq!(transcript.count(Role::Buyer), 1);
        assert_eq!(transcript.count(Role::Seller), 1);
        assert_ne!(transcript.root().expect("root"), [0u8; 32]);
    }

    #[test]
    fn replay_reaches_the_same_root() {
        let mut transcript = transcript();
        let buyer_first = message(Role::Buyer, 1, [0; 32], b"offer");
        let seller_first = message(Role::Seller, 1, [0; 32], b"counter");
        transcript.append(&buyer_first).expect("buyer offer");
        transcript.append(&seller_first).expect("seller counter");
        let buyer_second = message(Role::Buyer, 2, transcript.head(Role::Buyer), b"accept");
        transcript.append(&buyer_second).expect("buyer accept");
        let root = transcript.root().expect("root");

        let replayed = Transcript::replay(DEAL, 1, &[buyer_first, seller_first, buyer_second])
            .expect("replay");
        assert_eq!(replayed.root().expect("root"), root);
    }

    #[test]
    fn an_empty_transcript_has_a_zero_root() {
        assert_eq!(transcript().root().expect("root"), [0u8; 32]);
    }

    #[test]
    fn a_duplicate_is_rejected() {
        let mut transcript = transcript();
        let first = message(Role::Buyer, 1, [0; 32], b"offer");
        transcript.append(&first).expect("first");
        let error = transcript.append(&first).expect_err("duplicate");
        assert!(matches!(
            error,
            TranscriptError::SequenceOutOfOrder {
                author: "buyer",
                expected: 2,
                actual: 1
            }
        ));
    }

    #[test]
    fn a_gap_is_rejected() {
        let mut transcript = transcript();
        let gap = message(Role::Buyer, 2, [0; 32], b"offer");
        assert!(matches!(
            transcript.append(&gap),
            Err(TranscriptError::SequenceOutOfOrder {
                expected: 1,
                actual: 2,
                ..
            })
        ));
    }

    #[test]
    fn a_forked_message_is_rejected() {
        let mut transcript = transcript();
        let first = message(Role::Buyer, 1, [0; 32], b"offer");
        transcript.append(&first).expect("first");
        // A second message that claims the old (zero) parent, not the new head.
        let fork = message(Role::Buyer, 2, [0; 32], b"conflict");
        assert!(matches!(
            transcript.append(&fork),
            Err(TranscriptError::ParentMismatch { author: "buyer" })
        ));
    }

    #[test]
    fn a_first_message_with_a_non_zero_parent_is_rejected() {
        let mut transcript = transcript();
        let bad_parent = message(Role::Seller, 1, [0xaa; 32], b"offer");
        assert!(matches!(
            transcript.append(&bad_parent),
            Err(TranscriptError::ParentMismatch { author: "seller" })
        ));
    }

    #[test]
    fn a_message_for_another_deal_is_rejected() {
        let mut transcript = transcript();
        let mut other = message(Role::Buyer, 1, [0; 32], b"offer");
        other.deal_id = [0x99; 16];
        assert_eq!(
            transcript.append(&other),
            Err(TranscriptError::DealMismatch)
        );
    }

    #[test]
    fn a_change_in_an_earlier_message_changes_the_root() {
        let mut original = transcript();
        original
            .append(&message(Role::Buyer, 1, [0; 32], b"offer"))
            .expect("offer");
        let mut altered = transcript();
        altered
            .append(&message(Role::Buyer, 1, [0; 32], b"offer!"))
            .expect("offer");
        assert_ne!(
            original.root().expect("root"),
            altered.root().expect("root")
        );
    }
}
