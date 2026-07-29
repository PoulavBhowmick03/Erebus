//! Viewing-key disclosure — P2.2, and the compliance half of the pitch.
//!
//! Someone is handed a secret and reconstructs a complete, verifiable record of one
//! negotiation and its settlement, from chain data alone. No off-chain log to trust, and
//! nothing about any other channel.
//!
//! ## This is not the pool's viewing key, and the difference matters
//!
//! STRK20 has a mechanism with the same name and a very different shape. `SetViewingKey` at
//! registration encrypts **your pool private key** to a single pool-wide
//! `auditor_public_key` held in contract state (`privacy.cairo:329-334`). It is set once,
//! it covers your entire history across every channel and counterparty, and it is not
//! something you grant — it happens the moment you register.
//!
//! What this module grants is the **channel key**. Every note location and every amount
//! mask in a channel derives from it (`compute_note_id`, `compute_enc_amount_hash`), and
//! nothing outside that channel does. So a grant reveals exactly one relationship on one
//! token, and the holder cannot walk sideways into anything else.
//!
//! That is a stronger property than the pool's own auditor escrow, and it is the honest
//! version of the claim: not "nobody else learns anything" — the pool auditor already holds
//! your key — but "this grant discloses this channel and only this channel."
//!
//! ## Both directions, or it is not the conversation
//!
//! Channels are directional, so a negotiation lives in two subchannels with two keys. A
//! grant carries both. The granting agent can supply both because it derived its outgoing
//! key and learned the incoming one from the counterparty's `EncChannelInfo` — so either
//! party alone can disclose the whole exchange, without the other's cooperation and without
//! either party's pool private key.
//!
//! ## What the holder cannot do
//!
//! Spend. Note ids and amount masks come from the channel key, but a nullifier
//! (`compute_nullifier`) needs the owner's pool private key, and no grant carries one. The
//! record is readable and the money is not movable.

use serde::{Deserialize, Serialize};
use starknet_types_core::felt::Felt;

use crate::negotiation::{Author, OfferBook, OfferId, OfferStatus};
use crate::read::{reconstruct, ChannelReader, NoteSource, ReadError};
use crate::wire::{MessageType, WireMessage};

/// A scoped disclosure secret for one channel pair on one token.
///
/// **Secret-bearing.** Serialization exists because granting means handing this to someone,
/// but anyone holding it can read the whole exchange. It carries no pool private key, so it
/// confers reading and never spending.
#[derive(Clone, Serialize, Deserialize)]
pub struct ViewingGrant {
    /// Channel key for granter → counterparty.
    outgoing_key: Felt,
    /// Channel key for counterparty → granter.
    incoming_key: Felt,
    /// The token whose subchannel this covers. A grant is scoped to one.
    pub token: Felt,
    /// The granting agent's address.
    pub granter: Felt,
    /// The counterparty's address.
    pub counterparty: Felt,
}

/// Redacts both keys. A grant in a log line is a disclosed channel.
impl core::fmt::Debug for ViewingGrant {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ViewingGrant")
            .field("outgoing_key", &"<redacted>")
            .field("incoming_key", &"<redacted>")
            .field("token", &self.token)
            .field("granter", &self.granter)
            .field("counterparty", &self.counterparty)
            .finish()
    }
}

impl ViewingGrant {
    /// Builds a grant from both directional channel keys.
    ///
    /// `incoming_key` is the counterparty's channel to us, which we learned from their
    /// `EncChannelInfo`. Without it the record is half a conversation: our own offers with
    /// nothing they said in reply.
    pub fn new(
        outgoing_key: Felt,
        incoming_key: Felt,
        token: Felt,
        granter: Felt,
        counterparty: Felt,
    ) -> Self {
        Self {
            outgoing_key,
            incoming_key,
            token,
            granter,
            counterparty,
        }
    }

    /// Readers for the two directions, from the granter's point of view.
    fn readers(&self) -> (ChannelReader, ChannelReader) {
        (
            ChannelReader::new(self.outgoing_key, self.token),
            ChannelReader::new(self.incoming_key, self.token),
        )
    }
}

/// One message in a disclosed record, attributed to whoever wrote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisclosedMessage {
    /// Which side wrote it and where it sat.
    pub id: OfferId,
    /// The address that wrote it, resolved so the record reads without knowing who granted.
    pub author_addr: Felt,
    /// The message itself.
    pub message: WireMessage,
    /// Its status at the time of disclosure.
    pub status: OfferStatus,
}

/// What actually settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisclosedSettlement {
    /// The acceptance record.
    pub acceptance: OfferId,
    /// The offer that was accepted, if the acceptance named one.
    pub accepted_offer: Option<OfferId>,
    /// The amount the acceptance committed to.
    pub agreed_amount: u128,
    /// The amount the payment note actually carries, decrypted from chain data.
    ///
    /// Separate from `agreed_amount` on purpose: one is what the message *said*, the other
    /// is what was *paid*. An auditor's first question is whether they match, and a record
    /// that conflated them could not answer it.
    pub paid_amount: Option<u128>,
}

impl DisclosedSettlement {
    /// Whether the amount paid matches the amount agreed.
    ///
    /// `None` when no payment note was found, which is not the same as a mismatch.
    pub fn is_consistent(&self) -> Option<bool> {
        self.paid_amount.map(|paid| paid == self.agreed_amount)
    }
}

/// A reconstructed negotiation and its settlement.
#[derive(Debug, Clone)]
pub struct DisclosedRecord {
    /// The two parties.
    pub participants: [Felt; 2],
    /// The token this channel settles in.
    pub token: Felt,
    /// Every message, ordered as the negotiation happened.
    pub messages: Vec<DisclosedMessage>,
    /// The settlement, if the negotiation reached one.
    pub settlement: Option<DisclosedSettlement>,
}

impl DisclosedRecord {
    /// Whether this negotiation settled.
    pub fn is_settled(&self) -> bool {
        self.settlement.is_some()
    }
}

/// Reconstructs the full record a grant discloses.
///
/// `now` is used only to label statuses; it does not gate what is returned. An auditor
/// reading a year later should still see every message, correctly marked expired.
pub fn reveal(
    grant: &ViewingGrant,
    source: &impl NoteSource,
    now: u64,
) -> Result<DisclosedRecord, ReadError> {
    let (ours, theirs) = grant.readers();
    let book = reconstruct(&ours, &theirs, source)?;

    let messages: Vec<DisclosedMessage> = book
        .entries()
        .map(|(id, message)| DisclosedMessage {
            id,
            author_addr: match id.author {
                Author::Us => grant.granter,
                Author::Counterparty => grant.counterparty,
            },
            message,
            status: book.status(id, now).unwrap_or(OfferStatus::Proposed),
        })
        .collect();

    let settlement = settlement_of(&book, grant, source);

    Ok(DisclosedRecord {
        participants: [grant.granter, grant.counterparty],
        token: grant.token,
        messages,
        settlement,
    })
}

/// Finds the acceptance and matches it against the payment note actually written.
fn settlement_of(
    book: &OfferBook,
    grant: &ViewingGrant,
    source: &impl NoteSource,
) -> Option<DisclosedSettlement> {
    let (acceptance, message) = book
        .entries()
        .find(|(_, m)| m.message_type == MessageType::Accept)?;

    // The payment note sits in the accepting party's own outgoing channel — they paid, so
    // they wrote it.
    let (ours, theirs) = grant.readers();
    let payer = match acceptance.author {
        Author::Us => ours,
        Author::Counterparty => theirs,
    };

    Some(DisclosedSettlement {
        acceptance,
        accepted_offer: message
            .reply_to
            .map(|index| OfferId::new(acceptance.author.opposite(), index)),
        agreed_amount: message.amount,
        paid_amount: payer
            .settlement_note(acceptance.index, source)
            .map(|note| note.amount),
    })
}
