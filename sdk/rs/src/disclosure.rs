//! Recipient-bound deal disclosure, with legacy viewing-grant compatibility.
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
//! Historical grants carry channel keys. Wire-v3 grants instead carry native per-deal keys
//! plus exact opaque note locations and amount masks. STRK20 derives those locations and
//! masks from the parent channel key, so the grantor computes the capabilities without
//! exporting that parent key. The pool auditor still holds the pool key.
//!
//! ## Both directions
//!
//! A negotiation uses two directional subchannels. A v3 grant carries one native deal key
//! and the selected note capabilities for each direction. Either party can create the
//! capsule. Only the named recipient's pool key can open it before its expiry.
//!
//! ## What the holder cannot do
//!
//! A grant cannot spend. `compute_nullifier` needs the owner's pool private key, which is
//! absent from the grant.

use aes_gcm_siv::aead::{Aead, KeyInit, Payload};
use aes_gcm_siv::{Aes256GcmSiv, Nonce};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use starknet_types_core::{curve::AffinePoint, felt::Felt};
use std::collections::HashMap;

use crate::actions::NoteSalt;
use crate::channel::PoolIdentity;
use crate::decrypt;
use crate::negotiation::{Author, OfferBook, OfferId, OfferStatus};
use crate::read::{reconstruct, ChannelReader, NoteSource, ReadError};
use crate::wire::{
    decode_message_v3_with_deal_key, MessageType, WireContext, WireMessage, WireVersion,
    NOTES_PER_MESSAGE,
};

/// A versioned disclosure grant.
///
/// Version 3 is recipient-bound and deal-scoped. Historical versions are bearer grants for
/// one channel pair and token. No version contains spending authority.
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
    /// Wire-v3 deal id as a decimal string. Empty for legacy grants.
    #[serde(default)]
    deal_id: String,
    /// Account address whose registered pool key encrypts the capsule.
    #[serde(default)]
    recipient: Felt,
    /// Unix time after which a v3 verifier refuses to open the capsule.
    #[serde(default)]
    expires_at: u64,
    /// X-coordinate of the one-time Stark-curve ECDH public key.
    #[serde(default)]
    ephemeral_pubkey: Felt,
    /// AES-256-GCM-SIV ciphertext containing only this deal's capabilities.
    #[serde(default)]
    ciphertext: Vec<u8>,
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

/// One frame whose opaque STRK20 locations a deal grant may read.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct FrameCapability {
    pub(crate) message_index: u32,
    pub(crate) notes: [NoteCapability; NOTES_PER_MESSAGE],
    pub(crate) payment: Option<NoteCapability>,
}

/// One exact STRK20 note location and amount mask.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct NoteCapability {
    pub(crate) note_id: Felt,
    pub(crate) amount_mask: [u8; 16],
}

/// Capabilities for one direction of one deal.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct DirectionCapability {
    pub(crate) deal_key: [u8; 32],
    pub(crate) frames: Vec<FrameCapability>,
}

/// Plaintext protected by recipient ECDH and never returned directly by an API.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct DealGrantPayload {
    pub(crate) outgoing: DirectionCapability,
    pub(crate) incoming: DirectionCapability,
}

/// Fields authenticated outside the encrypted v3 capsule.
pub(crate) struct DealGrantFields {
    pub(crate) chain_id: Felt,
    pub(crate) pool_address: Felt,
    pub(crate) token: Felt,
    pub(crate) granter: Felt,
    pub(crate) counterparty: Felt,
    pub(crate) deal_id: u64,
    pub(crate) recipient: Felt,
    pub(crate) expires_at: u64,
}

/// Failures specific to recipient-bound deal disclosure.
#[derive(Debug, thiserror::Error)]
pub enum DisclosureError {
    /// The recipient public key or capsule ephemeral key is not a curve point.
    #[error("invalid disclosure recipient or ephemeral public key")]
    InvalidPublicKey,
    /// The caller is not the recipient named in the capsule.
    #[error("viewing grant belongs to a different recipient")]
    WrongRecipient,
    /// The grant's explicit verification window ended.
    #[error("viewing grant expired at {expires_at}, now {now}")]
    Expired {
        /// Last Unix second at which the recipient may open the capsule.
        expires_at: u64,
        /// Current Unix time used by the verifier.
        now: u64,
    },
    /// The encrypted payload or its authenticated scope was changed.
    #[error("viewing grant authentication failed")]
    Authentication,
    /// The encrypted payload could not be represented.
    #[error("viewing grant payload serialization failed")]
    Serialization,
    /// The version does not name a supported grant shape.
    #[error("unsupported viewing grant version")]
    UnsupportedVersion,
    /// A listed capability was incomplete or did not decode as its selected deal.
    #[error("viewing grant capability is invalid")]
    InvalidCapability,
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
            deal_id: String::new(),
            recipient: Felt::ZERO,
            expires_at: 0,
            ephemeral_pubkey: Felt::ZERO,
            ciphertext: Vec::new(),
        };
        grant.checksum = grant_checksum_v2(&grant);
        grant
    }

    /// Builds a recipient-bound wire-v3 grant containing native per-deal keys.
    pub(crate) fn new_deal(
        fields: DealGrantFields,
        payload: &DealGrantPayload,
        recipient_public_key: Felt,
        ephemeral_secret: Felt,
    ) -> Result<Self, DisclosureError> {
        let recipient_point = AffinePoint::new_from_x(&recipient_public_key, false)
            .ok_or(DisclosureError::InvalidPublicKey)?;
        let ephemeral_pubkey = (&AffinePoint::generator() * ephemeral_secret).x();
        let shared_x = (&recipient_point * ephemeral_secret).x();
        let mut grant = Self {
            version: 3,
            chain_id: fields.chain_id,
            pool_address: fields.pool_address,
            wire_version: WireVersion::V3,
            outgoing_key: Felt::ZERO,
            incoming_key: Felt::ZERO,
            token: fields.token,
            granter: fields.granter,
            counterparty: fields.counterparty,
            checksum: Felt::ZERO,
            deal_id: fields.deal_id.to_string(),
            recipient: fields.recipient,
            expires_at: fields.expires_at,
            ephemeral_pubkey,
            ciphertext: Vec::new(),
        };
        let aad = grant.deal_aad()?;
        let (key, nonce) = grant_cipher_material(shared_x, &aad);
        let plaintext = serde_json::to_vec(payload).map_err(|_| DisclosureError::Serialization)?;
        let cipher =
            Aes256GcmSiv::new_from_slice(&key).expect("AES-256-GCM-SIV accepts every 32-byte key");
        grant.ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| DisclosureError::Authentication)?;
        Ok(grant)
    }

    /// Opens a wire-v3 capsule only for its named recipient and verification window.
    pub(crate) fn open_deal(
        &self,
        recipient: &PoolIdentity,
        now: u64,
    ) -> Result<DealGrantPayload, DisclosureError> {
        if self.version != 3 {
            return Err(DisclosureError::UnsupportedVersion);
        }
        if recipient.address() != self.recipient {
            return Err(DisclosureError::WrongRecipient);
        }
        if now > self.expires_at {
            return Err(DisclosureError::Expired {
                expires_at: self.expires_at,
                now,
            });
        }
        let point = AffinePoint::new_from_x(&self.ephemeral_pubkey, false)
            .ok_or(DisclosureError::InvalidPublicKey)?;
        let shared_x = (&point * recipient.disclosure_private_key()).x();
        let aad = self.deal_aad()?;
        let (key, nonce) = grant_cipher_material(shared_x, &aad);
        let cipher =
            Aes256GcmSiv::new_from_slice(&key).expect("AES-256-GCM-SIV accepts every 32-byte key");
        let plaintext = cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &self.ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| DisclosureError::Authentication)?;
        serde_json::from_slice(&plaintext).map_err(|_| DisclosureError::Authentication)
    }

    fn deal_aad(&self) -> Result<Vec<u8>, DisclosureError> {
        let deal_id = self
            .deal_id
            .parse::<u64>()
            .map_err(|_| DisclosureError::Authentication)?;
        let mut aad = b"EREBUS_DEAL_GRANT_V3_AAD".to_vec();
        aad.extend_from_slice(&self.chain_id.to_bytes_be());
        aad.extend_from_slice(&self.pool_address.to_bytes_be());
        aad.extend_from_slice(&self.token.to_bytes_be());
        aad.extend_from_slice(&self.granter.to_bytes_be());
        aad.extend_from_slice(&self.counterparty.to_bytes_be());
        aad.extend_from_slice(&deal_id.to_be_bytes());
        aad.extend_from_slice(&self.recipient.to_bytes_be());
        aad.extend_from_slice(&self.expires_at.to_be_bytes());
        aad.extend_from_slice(&self.ephemeral_pubkey.to_bytes_be());
        Ok(aad)
    }

    /// Deal selected by a v3 grant.
    pub fn deal_id(&self) -> Option<u64> {
        (self.version == 3)
            .then(|| self.deal_id.parse().ok())
            .flatten()
    }

    /// Whether this is the recipient-bound per-deal format.
    pub(crate) fn is_deal_grant(&self) -> bool {
        self.version == 3
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

    /// Chain and pool authenticated by a scoped grant. Historical v1 grants had no scope.
    pub(crate) fn authenticated_scope(&self) -> Option<(Felt, Felt)> {
        matches!(self.version, 2 | 3).then_some((self.chain_id, self.pool_address))
    }
}

fn grant_cipher_material(shared_x: Felt, aad: &[u8]) -> ([u8; 32], [u8; 12]) {
    let hkdf = Hkdf::<Sha256>::new(
        Some(b"EREBUS_DEAL_GRANT_V3_HKDF_SHA256"),
        &shared_x.to_bytes_be(),
    );
    let mut key_info = b"EREBUS_DEAL_GRANT_V3_KEY".to_vec();
    key_info.extend_from_slice(aad);
    let mut key = [0u8; 32];
    hkdf.expand(&key_info, &mut key)
        .expect("32-byte HKDF output is always valid");
    let mut nonce_info = b"EREBUS_DEAL_GRANT_V3_NONCE".to_vec();
    nonce_info.extend_from_slice(aad);
    let mut nonce = [0u8; 12];
    hkdf.expand(&nonce_info, &mut nonce)
        .expect("12-byte HKDF output is always valid");
    (key, nonce)
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

/// Reconstructs exactly one wire-v3 deal from an opened recipient-bound capsule.
pub(crate) fn reveal_deal(
    grant: &ViewingGrant,
    payload: &DealGrantPayload,
    source: &impl NoteSource,
    now: u64,
) -> Result<DisclosedRecord, DisclosureError> {
    let deal_id = grant.deal_id().ok_or(DisclosureError::Authentication)?;
    let mut pending = Vec::new();
    let mut payments = HashMap::new();
    decode_direction(
        grant,
        deal_id,
        Author::Us,
        &payload.outgoing,
        source,
        &mut pending,
        &mut payments,
    )?;
    decode_direction(
        grant,
        deal_id,
        Author::Counterparty,
        &payload.incoming,
        source,
        &mut pending,
        &mut payments,
    )?;
    pending.sort_by_key(|(_, index, message)| (message.created_at, *index));

    let mut book = OfferBook::new();
    while !pending.is_empty() {
        let ready = pending.iter().position(|(author, _, message)| {
            message.reply_to.is_none_or(|reply_to| {
                let target = OfferId::new(author.opposite(), reply_to);
                book.entries().any(|(id, _)| id == target)
            })
        });
        let position = ready.ok_or(DisclosureError::InvalidCapability)?;
        let (author, index, message) = pending.remove(position);
        book.record(index, author, message)
            .map_err(|_| DisclosureError::InvalidCapability)?;
    }

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
    let settlement = book
        .entries()
        .find(|(_, message)| message.message_type == MessageType::Accept)
        .map(|(acceptance, message)| DisclosedSettlement {
            acceptance,
            accepted_offer: message
                .reply_to
                .map(|index| OfferId::new(acceptance.author.opposite(), index)),
            agreed_amount: message.amount,
            paid_amount: payments.get(&acceptance).copied(),
        });

    Ok(DisclosedRecord {
        participants: [grant.granter, grant.counterparty],
        token: grant.token,
        messages,
        settlement,
    })
}

fn decode_direction(
    grant: &ViewingGrant,
    deal_id: u64,
    author: Author,
    capability: &DirectionCapability,
    source: &impl NoteSource,
    messages: &mut Vec<(Author, u32, WireMessage)>,
    payments: &mut HashMap<OfferId, u128>,
) -> Result<(), DisclosureError> {
    for frame in &capability.frames {
        let salts = frame
            .notes
            .iter()
            .map(|note| {
                let packed = source
                    .packed_value(note.note_id)
                    .ok_or(DisclosureError::InvalidCapability)?;
                let (salt, encrypted_amount) = decrypt::unpack_note(packed);
                let mask = u128::from_be_bytes(note.amount_mask);
                if encrypted_amount.wrapping_sub(mask) != 0 {
                    return Err(DisclosureError::InvalidCapability);
                }
                NoteSalt::new(salt).map_err(|_| DisclosureError::InvalidCapability)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let salts: [NoteSalt; NOTES_PER_MESSAGE] = salts
            .try_into()
            .map_err(|_| DisclosureError::InvalidCapability)?;
        let context = WireContext {
            chain_id: grant.chain_id,
            pool_address: grant.pool_address,
            channel_key: Felt::ZERO,
            token: grant.token,
            message_index: frame.message_index,
        };
        let message =
            decode_message_v3_with_deal_key(&context, deal_id, &capability.deal_key, &salts)
                .map_err(|_| DisclosureError::InvalidCapability)?;
        if message.deal_id != deal_id {
            return Err(DisclosureError::InvalidCapability);
        }
        if let Some(payment) = &frame.payment {
            if message.message_type != MessageType::Accept {
                return Err(DisclosureError::InvalidCapability);
            }
            let packed = source
                .packed_value(payment.note_id)
                .ok_or(DisclosureError::InvalidCapability)?;
            let (_, encrypted_amount) = decrypt::unpack_note(packed);
            let amount = encrypted_amount.wrapping_sub(u128::from_be_bytes(payment.amount_mask));
            if amount == 0 {
                return Err(DisclosureError::InvalidCapability);
            }
            payments.insert(OfferId::new(author, frame.message_index), amount);
        }
        messages.push((author, frame.message_index, message));
    }
    Ok(())
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
        WireVersion::V3 => Felt::THREE,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::ClientAction;
    use crate::channel::{Channel, Counterparty};
    use crate::wire::{derive_deal_key, MessageType};

    const NOW: u64 = 1_753_699_200;

    fn identity(address: u64, secret: u64) -> PoolIdentity {
        PoolIdentity::new(Felt::from(address), Felt::from(secret))
    }

    fn message(deal_id: u64, amount: u128) -> WireMessage {
        WireMessage {
            deal_id,
            message_type: MessageType::Offer,
            reply_to: None,
            created_at: NOW,
            amount,
            deadline: NOW + 3_600,
            memo_hash: 0xa0d17,
        }
    }

    fn one_frame(
        channel: &Channel,
        token: Felt,
        deal_id: u64,
        amount: u128,
        message_index: u32,
    ) -> (DirectionCapability, HashMap<Felt, Felt>) {
        let set = channel
            .write_message(token, message_index, &message(deal_id, amount))
            .expect("v3 frame");
        let mut source = HashMap::new();
        let note_caps = set
            .actions()
            .iter()
            .filter_map(|action| match action {
                ClientAction::CreateEncNote(note) => {
                    let index = u64::from(note.index);
                    let note_id = crate::hashes::compute_note_id(channel.key(), token, index);
                    let mask =
                        decrypt::note_amount_mask(channel.key(), token, index, note.salt.get());
                    let packed = Felt::from(note.salt.get()) * (Felt::from(u128::MAX) + Felt::ONE)
                        + Felt::from(mask);
                    source.insert(note_id, packed);
                    Some(NoteCapability {
                        note_id,
                        amount_mask: mask.to_be_bytes(),
                    })
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let notes = match note_caps.try_into() {
            Ok(notes) => notes,
            Err(_) => panic!("five data notes"),
        };
        let context = WireContext {
            chain_id: Felt::from(1_u8),
            pool_address: Felt::from(2_u8),
            channel_key: channel.key(),
            token,
            message_index,
        };
        (
            DirectionCapability {
                deal_key: derive_deal_key(&context, deal_id),
                frames: vec![FrameCapability {
                    message_index,
                    notes,
                    payment: None,
                }],
            },
            source,
        )
    }

    fn deal_grant(
        recipient: &PoolIdentity,
        deal_id: u64,
    ) -> (ViewingGrant, DealGrantPayload, HashMap<Felt, Felt>, Felt) {
        let alice = identity(0xa11ce, 0x12345);
        let bob = identity(0xb0b, 0x23456);
        let channel = Channel::derive_with_version(
            Felt::from(1_u8),
            Felt::from(2_u8),
            &alice,
            Counterparty {
                address: bob.address(),
                public_key: bob.public_key(),
            },
            WireVersion::V3,
        );
        let token = Felt::from(3_u8);
        let parent_key = channel.key();
        let (outgoing, mut source) = one_frame(&channel, token, deal_id, 900, 0);
        let (_, other_deal) = one_frame(&channel, token, deal_id + 1, 777, 5);
        source.extend(other_deal);
        let payload = DealGrantPayload {
            outgoing,
            incoming: DirectionCapability {
                deal_key: [7; 32],
                frames: Vec::new(),
            },
        };
        let grant = ViewingGrant::new_deal(
            DealGrantFields {
                chain_id: Felt::from(1_u8),
                pool_address: Felt::from(2_u8),
                token,
                granter: alice.address(),
                counterparty: bob.address(),
                deal_id,
                recipient: recipient.address(),
                expires_at: NOW + 600,
            },
            &payload,
            recipient.public_key(),
            Felt::from(0x34567_u64),
        )
        .expect("deal grant");
        (grant, payload, source, parent_key)
    }

    #[test]
    fn deal_capsule_is_recipient_bound_expiring_and_authenticated() {
        let recipient = identity(0xca401, 0x45678);
        let wrong_recipient = identity(0xda7a, 0x56789);
        let (grant, _, _, _) = deal_grant(&recipient, 41);

        let opened = grant
            .open_deal(&recipient, NOW)
            .expect("named recipient opens");
        assert_eq!(opened.outgoing.frames.len(), 1);
        assert!(matches!(
            grant.open_deal(&wrong_recipient, NOW),
            Err(DisclosureError::WrongRecipient)
        ));
        assert!(matches!(
            grant.open_deal(&recipient, NOW + 601),
            Err(DisclosureError::Expired { .. })
        ));

        let mut changed = grant.clone();
        changed.ciphertext[0] ^= 1;
        assert!(matches!(
            changed.open_deal(&recipient, NOW),
            Err(DisclosureError::Authentication)
        ));
    }

    #[test]
    fn deal_grant_contains_no_parent_key_and_reveals_only_selected_deal() {
        let recipient = identity(0xca401, 0x45678);
        let (grant, payload, source, parent_key) = deal_grant(&recipient, 41);

        assert_eq!(grant.outgoing_key, Felt::ZERO);
        assert_eq!(grant.incoming_key, Felt::ZERO);
        assert_ne!(parent_key, Felt::ZERO);
        let record = reveal_deal(&grant, &payload, &|id| source.get(&id).copied(), NOW)
            .expect("selected deal reveals");
        assert_eq!(record.messages.len(), 1);
        assert_eq!(record.messages[0].message.deal_id, 41);
        assert_eq!(record.messages[0].message.amount, 900);

        let mut wrong = payload.clone();
        wrong.outgoing.deal_key[0] ^= 1;
        assert!(matches!(
            reveal_deal(&grant, &wrong, &|id| source.get(&id).copied(), NOW),
            Err(DisclosureError::InvalidCapability)
        ));
    }
}
