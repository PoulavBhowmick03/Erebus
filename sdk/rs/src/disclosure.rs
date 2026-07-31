//! Viewing-key disclosure for P2.2.
//!
//! A holder receives a secret and reconstructs one negotiation and settlement from chain
//! data. The grant does not depend on an off-chain log or disclose another channel.
//!
//! ## Pool and channel viewing keys
//!
//! STRK20 also has a viewing key. `SetViewingKey` at registration encrypts the pool private
//! key to a single pool-wide
//! `auditor_public_key` held in contract state (`privacy.cairo:329-334`). It is set once,
//! covers every channel and counterparty, and applies at registration.
//!
//! This module grants a channel key. `compute_note_id` and `compute_enc_amount_hash` derive
//! locations and masks from it. The grant covers one relationship and token. It does not
//! expose another channel. The pool auditor still holds the pool key.
//!
//! ## Both directions
//!
//! A negotiation uses two directional subchannels and two keys. A grant carries both. The
//! agent derives its outgoing key and reads the incoming key from `EncChannelInfo`. Either
//! party can disclose the exchange without the other party or a pool private key.
//!
//! ## What the holder cannot do
//!
//! A grant cannot spend. `compute_nullifier` needs the owner's pool private key, which is
//! absent from the grant.

use serde::{Deserialize, Serialize};
use starknet_types_core::felt::Felt;

use crate::negotiation::{Author, OfferBook, OfferId, OfferStatus};
use crate::read::{reconstruct, ChannelReader, NoteSource, ReadError};
use crate::wire::{MessageType, WireMessage, WireVersion};

/// A scoped disclosure secret for one channel pair on one token.
///
/// Anyone with the serialized grant can read the exchange. The grant has no pool private
/// key and cannot authorize a spend.
#[derive(Clone, Serialize, Deserialize)]
pub struct ViewingGrant {
    /// Export format version.
    version: u8,
    /// Starknet chain that scopes wire-v2 key derivation. Zero for historical v1 grants.
    #[serde(default)]
    chain_id: Felt,
    /// Pool that scopes wire-v2 key derivation. Zero for historical v1 grants.
    #[serde(default)]
    pool_address: Felt,
    /// Negotiation wire generation used by both directional channels.
    #[serde(default)]
    wire_version: WireVersion,
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
    /// Poseidon integrity checksum over the complete scope and both keys.
    checksum: Felt,
}

/// Construction fields grouped to prevent reordering same-typed `Felt` arguments.
pub(crate) struct ViewingGrantFields {
    pub(crate) chain_id: Felt,
    pub(crate) pool_address: Felt,
    pub(crate) wire_version: WireVersion,
    pub(crate) outgoing_key: Felt,
    pub(crate) incoming_key: Felt,
    pub(crate) token: Felt,
    pub(crate) granter: Felt,
    pub(crate) counterparty: Felt,
}

/// Redacts both keys. A grant in a log line is a disclosed channel.
impl core::fmt::Debug for ViewingGrant {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ViewingGrant")
            .field("chain_id", &self.chain_id)
            .field("pool_address", &self.pool_address)
            .field("wire_version", &self.wire_version)
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
    /// `incoming_key` is the counterparty's channel to us from `EncChannelInfo`. Without it,
    /// the grant omits all counterparty messages.
    pub(crate) fn new(fields: ViewingGrantFields) -> Self {
        let mut grant = Self {
            version: 2,
            chain_id: fields.chain_id,
            pool_address: fields.pool_address,
            wire_version: fields.wire_version,
            outgoing_key: fields.outgoing_key,
            incoming_key: fields.incoming_key,
            token: fields.token,
            granter: fields.granter,
            counterparty: fields.counterparty,
            checksum: Felt::ZERO,
        };
        grant.checksum = grant_checksum_v2(&grant);
        grant
    }

    fn is_valid(&self) -> bool {
        match self.version {
            1 => {
                self.chain_id == Felt::ZERO
                    && self.pool_address == Felt::ZERO
                    && self.wire_version == WireVersion::V1
                    && self.checksum
                        == grant_checksum_v1(
                            self.outgoing_key,
                            self.incoming_key,
                            self.token,
                            self.granter,
                            self.counterparty,
                        )
            }
            2 => self.checksum == grant_checksum_v2(self),
            _ => false,
        }
    }

    /// Readers for the two directions, from the granter's point of view.
    pub(crate) fn readers(&self) -> (ChannelReader, ChannelReader) {
        (
            ChannelReader::with_version(
                self.chain_id,
                self.pool_address,
                self.outgoing_key,
                self.token,
                self.wire_version,
            ),
            ChannelReader::with_version(
                self.chain_id,
                self.pool_address,
                self.incoming_key,
                self.token,
                self.wire_version,
            ),
        )
    }

    /// Chain and pool authenticated by a v2 grant. Historical v1 grants had no scope.
    pub(crate) fn authenticated_scope(&self) -> Option<(Felt, Felt)> {
        (self.version == 2).then_some((self.chain_id, self.pool_address))
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
    /// Payment-note amount decrypted from chain data.
    ///
    /// This remains separate from `agreed_amount` so an auditor can compare the offer with
    /// the payment.
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
/// `now` labels statuses but does not filter messages. Old messages remain visible and can
/// have an expired status.
pub fn reveal(
    grant: &ViewingGrant,
    source: &impl NoteSource,
    now: u64,
) -> Result<DisclosedRecord, ReadError> {
    if !grant.is_valid() {
        return Err(ReadError::InvalidViewingGrant);
    }
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

fn grant_checksum_v1(
    outgoing_key: Felt,
    incoming_key: Felt,
    token: Felt,
    granter: Felt,
    counterparty: Felt,
) -> Felt {
    let tag = Felt::from_bytes_be_slice(b"EREBUS_VIEW_GRANT_V1");
    crate::hashes::hash(&[
        tag,
        outgoing_key,
        incoming_key,
        token,
        granter,
        counterparty,
    ])
}

fn grant_checksum_v2(grant: &ViewingGrant) -> Felt {
    let tag = Felt::from_bytes_be_slice(b"EREBUS_VIEW_GRANT_V2");
    let wire = match grant.wire_version {
        WireVersion::V1 => Felt::ONE,
        WireVersion::V2 => Felt::TWO,
    };
    crate::hashes::hash(&[
        tag,
        grant.chain_id,
        grant.pool_address,
        wire,
        grant.outgoing_key,
        grant.incoming_key,
        grant.token,
        grant.granter,
        grant.counterparty,
    ])
}

/// Matches the acceptance with its payment note.
fn settlement_of(
    book: &OfferBook,
    grant: &ViewingGrant,
    source: &impl NoteSource,
) -> Option<DisclosedSettlement> {
    let (acceptance, message) = book
        .entries()
        .find(|(_, m)| m.message_type == MessageType::Accept)?;

    // The accepting party writes the payment note in its outgoing channel.
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
