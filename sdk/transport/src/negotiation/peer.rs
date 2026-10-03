//! SDK delivery helpers for a typed negotiation over an authenticated socket.
//!
//! Outgoing events are durable before send; incoming events are durable before a caller can
//! reply. Final authorizations use a separate persistence callback and never enter the frozen
//! transcript. An I/O failure requires a new connection and explicit re-delivery.

use core::fmt;
use erebus_core::{
    auth::{Authorization, Role},
    ids::BaseUnits,
};

use super::{Negotiation, NegotiationError, NegotiationStore};
use crate::{
    binding::{AgreementKeyBinding, MAX_BINDING_BYTES},
    descriptor::{DiscoveryFilter, ServiceDescriptor},
    limits::MAX_MESSAGES_PER_DEAL,
    message::{Message, MessageType},
    socket::{SocketChannel, SocketError},
    store::TranscriptStore,
};

/// Delivery, peer/context authentication, or durable storage failed.
#[derive(Debug, thiserror::Error)]
pub enum PeerError {
    /// A descriptor does not bind the selected agreement identity and capabilities.
    #[error("peer descriptors do not match the selected agreement")]
    Binding,
    /// The received initial offer does not satisfy the operator's service policy.
    #[error("initial offer rejected by local service policy")]
    Policy,
    /// Fresh per-deal randomness could not be obtained from the operating system.
    #[error("local entropy unavailable; no new deal created")]
    Entropy,
    /// A verified final authorization could not be made durable.
    #[error("final authorization persistence failed; peer acknowledgement withheld")]
    Persistence,
    /// The offchain negotiation transition was rejected.
    #[error(transparent)]
    Negotiation(#[from] NegotiationError),
    /// The connection failed. Establish a new Noise session before retrying delivery.
    #[error(transparent)]
    Socket(#[from] SocketError),
}

/// One participant's authenticated connection and private negotiation store.
pub struct NegotiationPeer {
    channel: SocketChannel,
    store: NegotiationStore,
}

impl fmt::Debug for NegotiationPeer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NegotiationPeer(<redacted>)")
    }
}

impl NegotiationPeer {
    pub(crate) fn from_bootstrap(channel: SocketChannel, store: NegotiationStore) -> Self {
        Self { channel, store }
    }

    /// Exchanges descriptor-signed shielded-key bindings inside Noise before negotiation.
    /// Both keys must match the operator-selected draft. The binding is not a payment signature.
    pub fn shielded(
        mut channel: SocketChannel,
        store: NegotiationStore,
        buyer: &ServiceDescriptor,
        seller: &ServiceDescriptor,
        local_binding: &AgreementKeyBinding,
        now: u64,
    ) -> Result<Self, PeerError> {
        store.load()?.check_live(now)?;
        let terms = store.initial.terms();
        let filter = DiscoveryFilter {
            chain_namespace: terms.domain.namespace.clone(),
            asset: terms.asset.clone(),
            mode: terms.settlement_mode,
            required_guarantees: terms.required_guarantees,
        };
        let digests = [
            buyer.digest().map_err(|_| PeerError::Binding)?,
            seller.digest().map_err(|_| PeerError::Binding)?,
        ];
        if terms.suite_id != 2
            || channel.descriptor_digests() != digests
            || !buyer
                .supports(&filter, now)
                .map_err(|_| PeerError::Binding)?
            || !seller
                .supports(&filter, now)
                .map_err(|_| PeerError::Binding)?
        {
            return Err(PeerError::Binding);
        }
        let local_role = channel.local_role();
        let (local_descriptor, local_key, remote_descriptor, remote_key) =
            if local_role == Role::Buyer {
                (
                    buyer,
                    &terms.buyer_authorization_key,
                    seller,
                    &terms.seller_authorization_key,
                )
            } else {
                (
                    seller,
                    &terms.seller_authorization_key,
                    buyer,
                    &terms.buyer_authorization_key,
                )
            };
        local_binding
            .verify(local_descriptor, &terms.domain, local_key, now)
            .map_err(|_| PeerError::Binding)?;
        let mut writer = erebus_core::encoding::Writer::new();
        writer.u16(1);
        writer.u8(4);
        writer.bytes(&local_binding.encode().map_err(|_| PeerError::Binding)?);
        let local_message = Message::new(
            channel.session_id(),
            terms.deal_id,
            1,
            local_role,
            1,
            [0; 32],
            MessageType::Authorization,
            writer.finish(),
        )
        .map_err(|_| PeerError::Binding)?;
        let remote_message = if local_role == Role::Buyer {
            channel.send(&local_message)?;
            channel.receive()?
        } else {
            let message = channel.receive()?;
            // Verify before replying, so a substituted buyer key receives no attestation.
            Self::verify_binding_message(&message, terms, remote_descriptor, remote_key, now)?;
            channel.send(&local_message)?;
            message
        };
        Self::verify_binding_message(&remote_message, terms, remote_descriptor, remote_key, now)?;
        Ok(Self { channel, store })
    }

    fn verify_binding_message(
        message: &Message,
        terms: &erebus_core::terms::AgreementTerms,
        descriptor: &ServiceDescriptor,
        expected_key: &erebus_core::ids::KeyBytes,
        now: u64,
    ) -> Result<(), PeerError> {
        if message.deal_id != terms.deal_id
            || message.revision != 1
            || message.message_type != MessageType::Authorization
            || message.sequence != 1
            || message.parent_hash != [0; 32]
        {
            return Err(PeerError::Binding);
        }
        let mut reader = erebus_core::encoding::Reader::new(&message.body);
        if reader.u16("profile").map_err(|_| PeerError::Binding)? != 1
            || reader.u8("event").map_err(|_| PeerError::Binding)? != 4
        {
            return Err(PeerError::Binding);
        }
        let binding = AgreementKeyBinding::decode(
            reader
                .bytes("binding", 1, MAX_BINDING_BYTES)
                .map_err(|_| PeerError::Binding)?,
        )
        .map_err(|_| PeerError::Binding)?;
        reader.finish().map_err(|_| PeerError::Binding)?;
        binding
            .verify(descriptor, &terms.domain, expected_key, now)
            .map_err(|_| PeerError::Binding)
    }

    /// Binds suite-1 agreement keys to the exact descriptors used by the Noise handshake.
    /// This constructor rejects suite 2, whose separate discovery-key binding is not implicit.
    pub fn public_bound(
        channel: SocketChannel,
        store: NegotiationStore,
        buyer: &ServiceDescriptor,
        seller: &ServiceDescriptor,
        now: u64,
    ) -> Result<Self, PeerError> {
        store.load()?.check_live(now)?;
        let terms = store.initial.terms();
        let filter = DiscoveryFilter {
            chain_namespace: terms.domain.namespace.clone(),
            asset: terms.asset.clone(),
            mode: terms.settlement_mode,
            required_guarantees: terms.required_guarantees,
        };
        let digests = [
            buyer.digest().map_err(|_| PeerError::Binding)?,
            seller.digest().map_err(|_| PeerError::Binding)?,
        ];
        if terms.suite_id != 1
            || terms.buyer_authorization_key.as_bytes() != buyer.seller_address
            || terms.seller_authorization_key.as_bytes() != seller.seller_address
            || channel.descriptor_digests() != digests
            || !buyer.suites.contains(&1)
            || !seller.suites.contains(&1)
            || !buyer
                .supports(&filter, now)
                .map_err(|_| PeerError::Binding)?
            || !seller
                .supports(&filter, now)
                .map_err(|_| PeerError::Binding)?
        {
            return Err(PeerError::Binding);
        }
        Ok(Self { channel, store })
    }

    /// Current private semantic state, rebuilt from durable events.
    pub fn state(&self) -> Result<Negotiation, PeerError> {
        Ok(self.store.load()?)
    }

    /// Current authenticated endpoint role.
    #[must_use]
    pub fn role(&self) -> Role {
        self.channel.local_role()
    }

    /// Reconciles retained negotiation events after a fresh handshake, without new signatures.
    /// Each side re-delivers its own durable events; exact duplicates remain idempotent.
    /// Buyer sends its batch first, avoiding simultaneous unbounded socket writes.
    /// Both resulting roots must match before this returns. No payment state is changed.
    pub fn synchronize(&mut self, now: u64) -> Result<Negotiation, PeerError> {
        let result = self.synchronize_inner(now);
        if result.is_err() {
            self.channel.close();
        }
        result
    }

    fn synchronize_inner(&mut self, now: u64) -> Result<Negotiation, PeerError> {
        self.state()?.check_live(now)?;
        let local: Vec<_> = self
            .store
            .store
            .messages(&self.store.namespace, self.store.initial.terms.deal_id)
            .map_err(super::NegotiationError::from)?
            .into_iter()
            .filter(|message| message.author == self.role())
            .collect();
        let mut writer = erebus_core::encoding::Writer::new();
        writer.u64(local.len() as u64);
        self.send_control(6, &writer.finish())?;
        let remote = self.receive_control(6)?;
        let mut reader = erebus_core::encoding::Reader::new(&remote);
        let count = reader
            .u64("retained_count")
            .map_err(|_| PeerError::Binding)?;
        reader.finish().map_err(|_| PeerError::Binding)?;
        if count > MAX_MESSAGES_PER_DEAL as u64 || local.len() > MAX_MESSAGES_PER_DEAL {
            return Err(PeerError::Binding);
        }
        let send = |peer: &mut Self| {
            for mut message in local {
                message.session_id = peer.channel.session_id();
                peer.channel.send(&message)?;
            }
            Ok::<_, PeerError>(())
        };
        let receive = |peer: &mut Self| {
            for _ in 0..count {
                peer.receive(now)?;
            }
            Ok::<_, PeerError>(())
        };
        if self.role() == Role::Buyer {
            send(self)?;
            receive(self)?;
        } else {
            receive(self)?;
            send(self)?;
        }
        let state = self.state()?;
        let root = state.transcript().root().map_err(|_| PeerError::Binding)?;
        self.send_control(7, &root)?;
        if self.receive_control(7)? != root {
            return Err(PeerError::Binding);
        }
        Ok(state)
    }

    fn send_control(&mut self, event: u8, payload: &[u8]) -> Result<(), PeerError> {
        let mut writer = erebus_core::encoding::Writer::new();
        writer.u16(1);
        writer.u8(event);
        writer.bytes(payload);
        let message = Message::new(
            self.channel.session_id(),
            self.store.initial.terms.deal_id,
            1,
            self.role(),
            1,
            [0; 32],
            MessageType::Authorization,
            writer.finish(),
        )
        .map_err(|_| PeerError::Binding)?;
        self.channel.send(&message)?;
        Ok(())
    }

    fn receive_control(&mut self, event: u8) -> Result<Vec<u8>, PeerError> {
        let message = self.channel.receive()?;
        if message.deal_id != self.store.initial.terms.deal_id
            || message.revision != 1
            || message.sequence != 1
            || message.parent_hash != [0; 32]
            || message.message_type != MessageType::Authorization
        {
            return Err(PeerError::Binding);
        }
        let mut reader = erebus_core::encoding::Reader::new(&message.body);
        if reader.u16("profile").map_err(|_| PeerError::Binding)? != 1
            || reader.u8("event").map_err(|_| PeerError::Binding)? != event
        {
            return Err(PeerError::Binding);
        }
        let payload = reader
            .bytes("control", 1, 32)
            .map_err(|_| PeerError::Binding)?
            .to_vec();
        reader.finish().map_err(|_| PeerError::Binding)?;
        Ok(payload)
    }

    /// Persists an offer/counter before attempting encrypted delivery.
    /// A failed send does not remove the event; explicitly redeliver it on a new connection.
    pub fn propose(&mut self, amount: BaseUnits, now: u64) -> Result<Negotiation, PeerError> {
        let message = self
            .state()?
            .propose(self.channel.session_id(), self.role(), amount, now)?;
        let state = self.store.append(&message)?;
        self.channel.send(&message)?;
        Ok(state)
    }

    /// Persists acceptance of the current proposal before encrypted delivery.
    pub fn accept(&mut self, now: u64) -> Result<Negotiation, PeerError> {
        let message = self
            .state()?
            .accept(self.channel.session_id(), self.role(), now)?;
        let state = self.store.append(&message)?;
        self.channel.send(&message)?;
        Ok(state)
    }

    /// Authenticates and persists one incoming offer, counter, or acceptance before returning.
    /// This method rejects final authorizations; receive them through the separate callback.
    pub fn receive(&mut self, now: u64) -> Result<Negotiation, PeerError> {
        self.state()?.check_live(now)?;
        let message = self.channel.receive()?;
        match self.store.append(&message) {
            Ok(state) => Ok(state),
            Err(error) => {
                self.channel.close();
                Err(error.into())
            }
        }
    }

    /// Explicitly re-delivers an already-persisted local event using a fresh session envelope.
    /// No new event or signature is created. The peer store recognizes exact-body re-delivery.
    pub fn redeliver(&mut self, sequence: u64, now: u64) -> Result<(), PeerError> {
        self.state()?.check_live(now)?;
        let mut message = self
            .store
            .store
            .messages(&self.store.namespace, self.store.initial.terms.deal_id)
            .map_err(NegotiationError::from)?
            .into_iter()
            .find(|message| message.author == self.role() && message.sequence == sequence)
            .ok_or(NegotiationError::Transition)?;
        message.session_id = self.channel.session_id();
        self.channel.send(&message)?;
        Ok(())
    }

    /// Sends a final authorization only after the caller makes it durable.
    /// For the buyer, use the coordinator's authorization fence and reservation first.
    pub fn send_authorization<E>(
        &mut self,
        authorization: &Authorization,
        now: u64,
        persist: impl FnOnce(&Authorization) -> Result<(), E>,
    ) -> Result<(), PeerError> {
        if authorization.role != self.role() {
            return Err(PeerError::Binding);
        }
        let message =
            self.state()?
                .authorization_message(self.channel.session_id(), authorization, now)?;
        if persist(authorization).is_err() {
            self.channel.close();
            return Err(PeerError::Persistence);
        }
        self.channel.send(&message)?;
        Ok(())
    }

    /// Verifies and persists the peer's final authorization without changing the frozen root.
    /// Returning is permission to acknowledge; a failed persistence callback withholds that.
    pub fn receive_authorization<E>(
        &mut self,
        now: u64,
        persist: impl FnOnce(&Authorization) -> Result<(), E>,
    ) -> Result<Authorization, PeerError> {
        let message = self.channel.receive()?;
        let result = (|| {
            let authorization = self.state()?.verify_final_authorization(&message, now)?;
            persist(&authorization).map_err(|_| PeerError::Persistence)?;
            Ok(authorization)
        })();
        if result.is_err() {
            self.channel.close();
        }
        result
    }
}

#[cfg(test)]
pub(super) mod tests;
