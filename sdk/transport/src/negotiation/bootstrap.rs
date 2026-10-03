//! Cold-start negotiation with no shared private proposal file.
//!
//! The operator selects settlement requirements before connecting. Shielded peers then
//! exchange descriptor-signed agreement identities and the seller's payment tag inside Noise.
//! Only the buyer constructs a draft. The seller authenticates and checks the received offer
//! against its own policy before storing it. Control messages are not transcript events.

use core::fmt;
use erebus_core::{
    auth::Role,
    commitment::CommitmentBlinding,
    encoding::{Reader, Writer},
    ids::KeyBytes,
    settlement::{check_capabilities, BackendCapabilities, SettlementContext},
    shielded::SHIELDED_GUARANTEES,
    terms::{AgreementTerms, SettlementMode},
};
use rand::{rngs::OsRng, RngCore};

use super::{peer::NegotiationPeer, peer::PeerError, NegotiationStore, Proposal};
use crate::{
    binding::{AgreementKeyBinding, MAX_BINDING_BYTES},
    descriptor::{DiscoveryFilter, ServiceDescriptor},
    hashing,
    message::{Message, MessageType},
    socket::SocketChannel,
    store::FileTranscriptStore,
};

const CONTEXT_DOMAIN: &[u8] = b"EREBUS_NEGOTIATION_CONTEXT_V1";

/// An authenticated connection ready to receive or send its first private offer.
pub struct NegotiationBootstrap {
    channel: SocketChannel,
    context: SettlementContext,
    buyer_key: KeyBytes,
    seller_key: KeyBytes,
    recipient: KeyBytes,
}

impl fmt::Debug for NegotiationBootstrap {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NegotiationBootstrap(<redacted>)")
    }
}

impl NegotiationBootstrap {
    /// Uses the exact suite-1 descriptor identities and the seller's address as recipient.
    /// No private draft or deal-specific key is required on the seller at this stage.
    pub fn public_bound(
        channel: SocketChannel,
        context: SettlementContext,
        buyer: &ServiceDescriptor,
        seller: &ServiceDescriptor,
        now: u64,
    ) -> Result<Self, PeerError> {
        Self::check_connection(&channel, &context, buyer, seller, now)?;
        if context.suite_id != 1 || context.mode != SettlementMode::PublicBound {
            return Err(PeerError::Binding);
        }
        Ok(Self {
            channel,
            context,
            buyer_key: KeyBytes::new(buyer.seller_address.to_vec())
                .map_err(|_| PeerError::Binding)?,
            seller_key: KeyBytes::new(seller.seller_address.to_vec())
                .map_err(|_| PeerError::Binding)?,
            recipient: KeyBytes::new(seller.seller_address.to_vec())
                .map_err(|_| PeerError::Binding)?,
        })
    }

    /// Learns the remote agreement key from its private signed identity binding.
    /// Only the seller supplies a payment tag; it never supplies its spending secret.
    /// The tag is authenticated by Noise and must later be authorized in the final agreement.
    /// Identity exchange neither proves possession nor authorizes or settles a payment.
    #[allow(clippy::too_many_arguments)]
    pub fn shielded(
        mut channel: SocketChannel,
        context: SettlementContext,
        buyer: &ServiceDescriptor,
        seller: &ServiceDescriptor,
        local_binding: &AgreementKeyBinding,
        seller_recipient: Option<KeyBytes>,
        now: u64,
    ) -> Result<Self, PeerError> {
        Self::check_connection(&channel, &context, buyer, seller, now)?;
        if context.suite_id != 2
            || context.mode != SettlementMode::Shielded
            || context.required_guarantees.bits() != SHIELDED_GUARANTEES
            || (channel.local_role() == Role::Seller) != seller_recipient.is_some()
            || seller_recipient.as_ref().is_some_and(|key| {
                key.as_bytes().len() != 32 || key.as_bytes().iter().all(|byte| *byte == 0)
            })
        {
            return Err(PeerError::Binding);
        }
        let (local_descriptor, remote_descriptor) = if channel.local_role() == Role::Buyer {
            (buyer, seller)
        } else {
            (seller, buyer)
        };
        let local_key = local_binding
            .attested_key(local_descriptor, &context.domain, now)
            .map_err(|_| PeerError::Binding)?;
        let mut writer = Writer::new();
        writer.u16(1);
        writer.u8(5);
        writer.fixed(&context_digest(&context)?);
        writer.bytes(&local_binding.encode().map_err(|_| PeerError::Binding)?);
        writer.bytes(seller_recipient.as_ref().map_or(&[], KeyBytes::as_bytes));
        // This session-level control envelope is never appended to a deal transcript.
        let message = Message::new(
            channel.session_id(),
            [0; 16],
            1,
            channel.local_role(),
            1,
            [0; 32],
            MessageType::Authorization,
            writer.finish(),
        )
        .map_err(|_| PeerError::Binding)?;
        let remote = if channel.local_role() == Role::Buyer {
            channel.send(&message)?;
            read_identity(&mut channel, &context, remote_descriptor, now)?
        } else {
            let remote = read_identity(&mut channel, &context, remote_descriptor, now)?;
            channel.send(&message)?;
            remote
        };
        let (buyer_key, seller_key, recipient) = if channel.local_role() == Role::Buyer {
            (local_key, remote.0, remote.1.ok_or(PeerError::Binding)?)
        } else {
            (
                remote.0,
                local_key,
                seller_recipient.ok_or(PeerError::Binding)?,
            )
        };
        Ok(Self {
            channel,
            context,
            buyer_key,
            seller_key,
            recipient,
        })
    }

    fn check_connection(
        channel: &SocketChannel,
        context: &SettlementContext,
        buyer: &ServiceDescriptor,
        seller: &ServiceDescriptor,
        now: u64,
    ) -> Result<(), PeerError> {
        let digests = [
            buyer.digest().map_err(|_| PeerError::Binding)?,
            seller.digest().map_err(|_| PeerError::Binding)?,
        ];
        let filter = DiscoveryFilter {
            chain_namespace: context.domain.namespace.clone(),
            asset: context.asset.clone(),
            mode: context.mode,
            required_guarantees: context.required_guarantees,
        };
        if channel.descriptor_digests() != digests {
            return Err(PeerError::Binding);
        }
        for descriptor in [buyer, seller] {
            if !descriptor
                .supports(&filter, now)
                .map_err(|_| PeerError::Binding)?
                || !descriptor.suites.contains(&context.suite_id)
            {
                return Err(PeerError::Binding);
            }
        }
        // Descriptor capability checks authenticate requirements, not live backend code.
        check_capabilities(
            context,
            &BackendCapabilities {
                suites: [context.suite_id].into(),
                modes: [context.mode].into(),
                guarantees: context.required_guarantees,
                local_proving: true,
            },
        )
        .map_err(|_| PeerError::Binding)
    }

    /// Authenticated buyer key to insert in the new private draft and access recipient.
    #[must_use]
    pub fn buyer_key(&self) -> &KeyBytes {
        &self.buyer_key
    }

    /// Authenticated seller agreement key, distinct from its shielded payment tag.
    #[must_use]
    pub fn seller_key(&self) -> &KeyBytes {
        &self.seller_key
    }

    /// The seller's payment address or private note-spending tag, never a spending secret.
    #[must_use]
    pub fn payment_recipient(&self) -> &KeyBytes {
        &self.recipient
    }

    /// Creates a fresh buyer draft from the caller's private service/price template.
    /// Replaces identities, access recipient, ID, nonce, revision, and transcript root.
    /// Deployment, asset, mode, guarantees, fees, service promise, and expiry are not weakened.
    /// This does not persist or send: pass the result to `offer` before reporting a deal.
    /// Never call this for recovery; recover the retained initial offer instead.
    pub fn create_proposal(
        &self,
        mut template: AgreementTerms,
        now: u64,
    ) -> Result<Proposal, PeerError> {
        if self.channel.local_role() != Role::Buyer {
            return Err(PeerError::Binding);
        }
        template.buyer_authorization_key = self.buyer_key.clone();
        template.seller_authorization_key = self.seller_key.clone();
        template.payment_recipient = self.recipient.clone();
        template.service.access_recipient = self.buyer_key.clone();
        template.revision = 1;
        template.transcript_root = [0; 32];
        OsRng
            .try_fill_bytes(&mut template.deal_id)
            .map_err(|_| PeerError::Entropy)?;
        OsRng
            .try_fill_bytes(&mut template.settlement_nonce)
            .map_err(|_| PeerError::Entropy)?;
        // Reject invalid non-random fields before sampling a canonical suite-2 blinding.
        let checked = Proposal::new(template.clone(), CommitmentBlinding::from_bytes([1; 32]))?;
        self.check_proposal(&checked, now)?;
        for _ in 0..128 {
            let mut blinding = [0; 32];
            OsRng
                .try_fill_bytes(&mut blinding)
                .map_err(|_| PeerError::Entropy)?;
            if let Ok(proposal) =
                Proposal::new(template.clone(), CommitmentBlinding::from_bytes(blinding))
            {
                return Ok(proposal);
            }
        }
        Err(PeerError::Entropy)
    }

    fn check_proposal(&self, proposal: &Proposal, now: u64) -> Result<(), PeerError> {
        let terms = proposal.terms();
        if terms.suite_id != self.context.suite_id
            || terms.domain != self.context.domain
            || terms.asset != self.context.asset
            || terms.settlement_mode != self.context.mode
            || terms.required_guarantees != self.context.required_guarantees
            || terms.buyer_authorization_key != self.buyer_key
            || terms.seller_authorization_key != self.seller_key
            || terms.payment_recipient != self.recipient
            || terms.revision != 1
        {
            return Err(PeerError::Binding);
        }
        super::Negotiation::new(proposal.clone())?.check_live(now)?;
        Ok(())
    }

    /// Persists the buyer's first offer before sending; a retained offer is re-delivered.
    /// Generate the deal ID, nonce, and blinding locally before calling this method.
    /// Recovery must reuse that retained draft, never generate a replacement deal.
    pub fn offer(
        self,
        store: FileTranscriptStore,
        namespace: String,
        proposal: Proposal,
        now: u64,
    ) -> Result<NegotiationPeer, PeerError> {
        if self.channel.local_role() != Role::Buyer {
            return Err(PeerError::Binding);
        }
        self.check_proposal(&proposal, now)?;
        let amount = proposal.terms().amount;
        let store = NegotiationStore::new(store, namespace, proposal)?;
        let mut peer = NegotiationPeer::from_bootstrap(self.channel, store);
        if peer.state()?.transcript().is_empty() {
            peer.propose(amount, now)?;
        } else {
            peer.redeliver(1, now)?;
        }
        Ok(peer)
    }

    /// Receives, validates, and persists a buyer offer without any shared private draft.
    /// The callback must check service identity, promise, expiry, fees, and operator policy.
    /// It runs before the incoming offer creates any deal record or acknowledgement.
    pub fn receive_offer<E>(
        mut self,
        store: FileTranscriptStore,
        namespace: String,
        now: u64,
        policy: impl FnOnce(&Proposal) -> Result<(), E>,
    ) -> Result<NegotiationPeer, PeerError> {
        if self.channel.local_role() != Role::Seller {
            return Err(PeerError::Binding);
        }
        let message = self.channel.receive()?;
        let proposal = Proposal::from_initial_offer(&message, now)?;
        self.check_proposal(&proposal, now)?;
        policy(&proposal).map_err(|_| PeerError::Policy)?;
        let store = NegotiationStore::new(store, namespace, proposal)?;
        store.append(&message)?;
        Ok(NegotiationPeer::from_bootstrap(self.channel, store))
    }
}

fn context_digest(context: &SettlementContext) -> Result<[u8; 32], PeerError> {
    let mut writer = Writer::new();
    writer.bytes(&context.domain.encode().map_err(|_| PeerError::Binding)?);
    writer.u16(context.suite_id);
    writer.u8(context.mode.tag());
    writer.text(&context.asset.to_string());
    writer.u32(context.required_guarantees.bits());
    Ok(hashing::hash(&[CONTEXT_DOMAIN, &writer.finish()]))
}

fn read_identity(
    channel: &mut SocketChannel,
    context: &SettlementContext,
    descriptor: &ServiceDescriptor,
    now: u64,
) -> Result<(KeyBytes, Option<KeyBytes>), PeerError> {
    let message = channel.receive()?;
    if message.deal_id != [0; 16]
        || message.revision != 1
        || message.sequence != 1
        || message.parent_hash != [0; 32]
        || message.message_type != MessageType::Authorization
    {
        return Err(PeerError::Binding);
    }
    let parse = (|| {
        let mut reader = Reader::new(&message.body);
        if reader.u16("profile").map_err(|_| PeerError::Binding)? != 1
            || reader.u8("event").map_err(|_| PeerError::Binding)? != 5
            || reader
                .fixed::<32>("context_digest")
                .map_err(|_| PeerError::Binding)?
                != context_digest(context)?
        {
            return Err(PeerError::Binding);
        }
        let binding = AgreementKeyBinding::decode(
            reader
                .bytes("binding", 1, MAX_BINDING_BYTES)
                .map_err(|_| PeerError::Binding)?,
        )
        .map_err(|_| PeerError::Binding)?;
        let key = binding
            .attested_key(descriptor, &context.domain, now)
            .map_err(|_| PeerError::Binding)?;
        let recipient = reader
            .bytes("recipient", 0, 32)
            .map_err(|_| PeerError::Binding)?;
        if (message.author == Role::Seller && recipient.len() != 32)
            || (message.author == Role::Buyer && !recipient.is_empty())
            || (message.author == Role::Seller && recipient.iter().all(|byte| *byte == 0))
        {
            return Err(PeerError::Binding);
        }
        let recipient = if recipient.is_empty() {
            None
        } else {
            Some(KeyBytes::new(recipient.to_vec()).map_err(|_| PeerError::Binding)?)
        };
        reader.finish().map_err(|_| PeerError::Binding)?;
        Ok((key, recipient))
    })();
    if parse.is_err() {
        channel.close();
    }
    parse
}

#[cfg(test)]
mod tests;
