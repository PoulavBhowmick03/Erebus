//! Client-side enforcement of the offer state machine (ARCHITECTURE §4).
//!
//! ## Why this is client-side
//!
//! The pool has no `status`, no `deadline`, and no `replyTo`. It stores notes. Nothing
//! on-chain stops an agent from settling against an offer that expired an hour ago, or from
//! paying twice for the same acceptance. Every rule in this module is a rule *only* because
//! this module enforces it — there is no second line of defence underneath.
//!
//! That is worth being blunt about, because the rest of the SDK has the opposite property.
//! A wrong note index or a malformed action set reverts on-chain; the contract is a backstop
//! for those. Here, a missing check is simply a missing check.
//!
//! ## `withdrawn` is not implemented, on purpose
//!
//! ARCHITECTURE §4 lists `withdrawn` as an `OfferStatus` and draws a
//! `proposed --> withdrawn` transition. Nothing can reach it: `ErebusClient` exposes no
//! `withdrawOffer`, and the wire format's [`MessageType`] is `Offer | Counter | Accept` with
//! no Withdraw variant. Adding one is not an SDK decision — it changes the frozen interface
//! and breaks the mock the agent track builds against (CLAUDE.md). Recorded as a P0.3 item;
//! until it is settled, withdrawal is unrepresentable rather than silently unenforced.
//!
//! Expiry, by contrast, is fully expressible: `deadline` is on the wire already.

use crate::wire::{MessageType, WireMessage};

/// Where an offer sits in the state machine.
///
/// Deliberately missing `Withdrawn` — see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferStatus {
    /// Written, live, nothing replying to it.
    Proposed,
    /// A later message replies to this one.
    Countered,
    /// Accepted and settled. On-chain these are one atomic transition; §4 separates them
    /// only for observability, and the SDK has no way to observe the gap, so it does not
    /// pretend to.
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
        /// What was actually there.
        kind: MessageType,
    },
    /// This negotiation already settled.
    #[error("this channel already settled at message {index}; a second settlement would pay twice")]
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
/// Channels are directional, so in practice these come from two different subchannels; the
/// book is the place they are reconciled into one ordered negotiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Author {
    /// Written by us.
    Us,
    /// Written by the counterparty.
    Counterparty,
}

/// Identifies one message in a negotiation.
///
/// **A message index alone is not an identifier.** Channels are directional, so each side
/// numbers its own messages from zero — A's first offer and B's first counter are both
/// index 0, in different subchannels. Keying a book on the bare index conflates them, which
/// silently turns "accept their counter" into "accept your own offer".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OfferId {
    /// Which side wrote it, i.e. which of the two directional channels it lives in.
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
    /// A reply always crosses directions — you answer the counterparty, not yourself — so
    /// this is how a `reply_to` index is resolved to a full [`OfferId`].
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
    /// `message.reply_to` is an index in the **opposite** direction, because a reply always
    /// crosses the table. Resolving it against the same author would make every reply
    /// dangle, and resolving it against both would reintroduce the collision this type
    /// exists to prevent.
    ///
    /// Rejects a reply to a message that was never seen. That is a real failure mode rather
    /// than a hypothetical: notes are fetched by computed slot, so a reader that miscounts
    /// its indices gets a message whose `reply_to` points nowhere, and without this check it
    /// would negotiate against a phantom.
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
        // An accepted offer is settled with its acceptance — they are one transition.
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
            .find(|e| {
                e.id.author == id.author.opposite() && e.message.reply_to == Some(id.index)
            })
            .map(|e| e.id)
    }

    /// Whether `id` may be accepted and settled right now.
    ///
    /// Call this before building the settlement action set. Past this point the SDK will
    /// happily construct a valid, provable, on-chain-accepted settlement against a
    /// three-day-old offer, because nothing underneath knows what a deadline is.
    ///
    /// A countered offer is still acceptable — §4 draws `countered --> accepted` — since
    /// countering is a proposal, not a revocation. Revocation would be `withdrawn`, which
    /// does not exist.
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

    /// The most recent live offer from the counterparty — what a policy engine evaluates on
    /// its turn.
    pub fn latest_acceptable(&self, now: u64) -> Option<(OfferId, WireMessage)> {
        self.entries
            .iter()
            .rev()
            .find(|e| self.check_acceptable(e.id, now).is_ok())
            .map(|e| (e.id, e.message))
    }

    /// Every message, in the order it was recorded.
    ///
    /// [`crate::read::reconstruct`] records in `created_at` order, so for a reconstructed
    /// book this is the order the negotiation actually happened in.
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
