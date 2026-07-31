//! Client-side offer state machine (ARCHITECTURE §4).
//!
//! ## Why this is client-side
//!
//! The pool stores notes but has no `status`, `deadline`, or `replyTo`. The contract does not
//! reject an expired offer or a second payment for one acceptance. Only this module enforces
//! those rules.
//!
//! ## Missing `withdrawn` state
//!
//! ARCHITECTURE §4 lists `withdrawn` as an `OfferStatus` and draws a
//! `proposed --> withdrawn` transition. Nothing can reach it. `ErebusClient` exposes no
//! `withdrawOffer`, and the wire format's [`MessageType`] is `Offer | Counter | Accept` with
//! no Withdraw variant. Adding one changes the frozen interface and breaks the agent-track
//! mock (CLAUDE.md). P0.3 tracks this gap. The current wire cannot represent withdrawal.
//!
//! Expiry, by contrast, is fully expressible: `deadline` is on the wire already.

use crate::wire::{MessageType, WireMessage};

/// Where an offer sits in the state machine.
///
/// `Withdrawn` is absent because the wire cannot represent it. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferStatus {
    /// Written, live, nothing replying to it.
    Proposed,
    /// A later message replies to this one.
    Countered,
    /// Accepted and settled in one on-chain transition.
    Settled,
    /// Its deadline has passed.
    Expired,
}

/// Why an offer cannot be accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NegotiationError {
    /// No message recorded at that index.
    #[error("no offer at message index {index}")]
    UnknownOffer {
        /// The index asked for.
        index: u32,
    },
    /// The deadline has passed. Nothing on-chain enforces this.
    #[error("offer {index} expired at {deadline}, now {now}")]
    Expired {
        /// The index asked for.
        index: u32,
        /// The offer's deadline, unix seconds.
        deadline: u64,
        /// The time the decision was made against.
        now: u64,
    },
    /// Accepting your own offer is not a negotiation.
    #[error("offer {index} is our own; acceptance is the counterparty's move")]
    OwnOffer {
        /// The index asked for.
        index: u32,
    },
    /// Only an Offer or a Counter can be accepted.
    #[error("message {index} is a {kind:?}, which is not an acceptable offer")]
    NotAnOffer {
        /// The index asked for.
        index: u32,
        /// Message type at that index.
        kind: MessageType,
    },
    /// This negotiation already settled.
    #[error(
        "this channel already settled at message {index}; a second settlement would pay twice"
    )]
    AlreadySettled {
        /// The index of the acceptance that settled it.
        index: u32,
    },
    /// A reply pointed at a message that was never recorded.
    #[error("message {index} replies to {reply_to}, which does not exist")]
    DanglingReply {
        /// The replying message.
        index: u32,
        /// The index it claimed to reply to.
        reply_to: u32,
    },
}

/// Which side wrote a message.
///
/// A book combines the two directional subchannels into one negotiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Author {
    /// Written by us.
    Us,
    /// Written by the counterparty.
    Counterparty,
}

/// Identifies one message in a negotiation.
///
/// Each direction starts its indices at zero. An index without its author can identify two
/// messages. That collision can turn a counterparty acceptance into an own-offer acceptance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OfferId {
    /// Side and directional channel that contain the message.
    pub author: Author,
    /// Its index within that side's subchannel.
    pub index: u32,
}

impl OfferId {
    /// An id for `index` in `author`'s direction.
    pub fn new(author: Author, index: u32) -> Self {
        Self { author, index }
    }
}

impl Author {
    /// The other side.
    ///
    /// A reply crosses directions, so this resolves a `reply_to` index to an [`OfferId`].
    pub fn opposite(self) -> Self {
        match self {
            Author::Us => Author::Counterparty,
            Author::Counterparty => Author::Us,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    id: OfferId,
    message: WireMessage,
}

/// The decoded negotiation so far, and the rules over it.
#[derive(Debug, Clone, Default)]
pub struct OfferBook {
    entries: Vec<Entry>,
    settled: Option<OfferId>,
}

impl OfferBook {
    /// An empty book for a fresh channel.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            settled: None,
        }
    }

    /// Record a message, ours or theirs.
    ///
    /// `message.reply_to` is an index in the opposite direction. Resolving it in the same
    /// direction makes the reply dangle. Searching both directions reintroduces index
    /// collisions.
    ///
    /// Rejects a reply to an unseen message. A reader with an incorrect computed slot can
    /// otherwise negotiate against a message that does not exist.
    pub fn record(
        &mut self,
        index: u32,
        author: Author,
        message: WireMessage,
    ) -> Result<(), NegotiationError> {
        let id = OfferId::new(author, index);

        if let Some(reply_to) = message.reply_to {
            let target = OfferId::new(author.opposite(), reply_to);
            if !self.entries.iter().any(|e| e.id == target) {
                return Err(NegotiationError::DanglingReply { index, reply_to });
            }
        }
        if message.message_type == MessageType::Accept {
            self.settled = Some(id);
        }
        self.entries.push(Entry { id, message });
        Ok(())
    }

    /// Status of one message at time `now` (unix seconds).
    pub fn status(&self, id: OfferId, now: u64) -> Option<OfferStatus> {
        let entry = self.entries.iter().find(|e| e.id == id)?;

        if self.settled == Some(id) {
            return Some(OfferStatus::Settled);
        }
        // Acceptance and settlement form one transition.
        if let Some(settled_at) = self.settled {
            let accepted = self
                .entries
                .iter()
                .find(|e| e.id == settled_at)
                .and_then(|e| e.message.reply_to)
                .map(|reply_to| OfferId::new(settled_at.author.opposite(), reply_to));
            if accepted == Some(id) {
                return Some(OfferStatus::Settled);
            }
        }
        if now > entry.message.deadline {
            return Some(OfferStatus::Expired);
        }
        if self.replies_to(id).is_some() {
            return Some(OfferStatus::Countered);
        }
        Some(OfferStatus::Proposed)
    }

    /// The message replying to `id`, if any.
    fn replies_to(&self, id: OfferId) -> Option<OfferId> {
        self.entries
            .iter()
            .find(|e| e.id.author == id.author.opposite() && e.message.reply_to == Some(id.index))
            .map(|e| e.id)
    }

    /// Whether `id` may be accepted and settled right now.
    ///
    /// Call this before building the settlement action set. Later layers do not check the
    /// deadline and can submit a settlement for an expired offer.
    ///
    /// A countered offer remains acceptable under §4. A counter is a proposal, not a
    /// revocation. The wire has no `withdrawn` state.
    pub fn check_acceptable(&self, id: OfferId, now: u64) -> Result<(), NegotiationError> {
        if let Some(settled_at) = self.settled {
            return Err(NegotiationError::AlreadySettled {
                index: settled_at.index,
            });
        }

        let entry = self
            .entries
            .iter()
            .find(|e| e.id == id)
            .ok_or(NegotiationError::UnknownOffer { index: id.index })?;

        if id.author == Author::Us {
            return Err(NegotiationError::OwnOffer { index: id.index });
        }
        match entry.message.message_type {
            MessageType::Offer | MessageType::Counter => {}
            kind => {
                return Err(NegotiationError::NotAnOffer {
                    index: id.index,
                    kind,
                })
            }
        }
        if now > entry.message.deadline {
            return Err(NegotiationError::Expired {
                index: id.index,
                deadline: entry.message.deadline,
                now,
            });
        }
        Ok(())
    }

    /// Most recent live counterparty offer for policy evaluation.
    pub fn latest_acceptable(&self, now: u64) -> Option<(OfferId, WireMessage)> {
        self.entries
            .iter()
            .rev()
            .find(|e| self.check_acceptable(e.id, now).is_ok())
            .map(|e| (e.id, e.message))
    }

    /// Every message, in the order it was recorded.
    ///
    /// [`crate::read::reconstruct`] records by `created_at`, so reconstructed entries use
    /// negotiation order.
    pub fn entries(&self) -> impl Iterator<Item = (OfferId, WireMessage)> + '_ {
        self.entries.iter().map(|e| (e.id, e.message))
    }

    /// Every message recorded, in the order it was recorded.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing has been recorded yet.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
