//! Noise XX sessions between two participants (decision DM2-1).
//!
//! This module is a thin, safe wrapper over `snow`. It does not implement any cryptography
//! itself; it fixes the parameter set, binds the prologue, records the session identity, and
//! enforces the session bounds. The handshake is driven by the caller exchanging the returned
//! byte messages, so the same code serves an in-process test, a relay, or any future transport.
//!
//! Two properties matter for the rest of the crate:
//!
//! - The **prologue** commits each side to the other's signed descriptor, so a substituted
//!   endpoint or a downgraded guarantee set fails the handshake instead of being noticed only at
//!   discovery time.
//! - The **session id** is the Noise handshake hash. Both sides compute the same value without
//!   transmitting it.
//!
//! Sessions are ephemeral and are never resumed after a restart (decision DM2-1). A restarted
//! participant performs a fresh handshake, which is what makes nonce reuse impossible: a dead
//! session's send counter is discarded with the process.

use erebus_core::auth::Role;
use erebus_core::encoding::Writer;
use snow::params::NoiseParams;
use snow::{HandshakeState, TransportState};

use crate::identity::TransportIdentity;
use crate::limits::{MAX_MESSAGES_PER_SESSION, MAX_MESSAGE_BYTES, MAX_SESSION_BYTES};
use crate::message::{Message, MessageError};

/// Transport protocol version bound into the prologue.
pub const TRANSPORT_PROTOCOL_VERSION: u16 = 1;

/// Transport protocol version of a routed handshake: one responder, many configured peers.
///
/// The initiator announces it in clear as the whole payload of handshake message 1. Noise mixes
/// that payload into the handshake hash, and the routed prologue starts with the same version,
/// so removing or adding the announcement changes both sides' hash and fails at message 2.
pub const ROUTED_TRANSPORT_PROTOCOL_VERSION: u16 = 2;

/// Length of an X25519 ephemeral key, the whole of a direct (version 1) message 1.
const EPHEMERAL_KEY_BYTES: usize = 32;

/// The clear message-1 payload of a routed handshake.
const ROUTED_HELLO: [u8; 2] = ROUTED_TRANSPORT_PROTOCOL_VERSION.to_be_bytes();

/// Largest handshake message accepted. XX handshake messages are small; the bound only exists so
/// a malicious peer cannot make us allocate an unbounded buffer.
pub const MAX_HANDSHAKE_MESSAGE_BYTES: usize = 1024;

/// A session could not be established or a message could not be processed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionError {
    /// The Noise implementation rejected a message or a state transition.
    #[error("transport session error: {0}")]
    Noise(String),
    /// The caller tried to write when the handshake expected it to read, or the reverse.
    #[error("transport session was asked to {action} out of turn")]
    OutOfTurn {
        /// The requested action, `read` or `write`.
        action: &'static str,
    },
    /// A handshake message was larger than the handshake bound.
    #[error("handshake message is {actual} bytes, above the {max} byte bound")]
    HandshakeTooLarge {
        /// Observed size.
        actual: usize,
        /// The bound.
        max: usize,
    },
    /// The peer's static transport key did not match the key its descriptor advertised.
    #[error("peer transport key does not match the descriptor")]
    PeerKeyMismatch,
    /// The handshake hash was not the expected width.
    #[error("handshake hash was {0} bytes, expected 32")]
    UnexpectedHandshakeHash(usize),
    /// A first handshake message announced no version this endpoint speaks.
    #[error("handshake message does not announce a supported transport version")]
    UnsupportedVersion,
    /// A routed handshake's authenticated static key or descriptor digest named no peer.
    #[error("routed handshake named no configured peer")]
    UnknownPeer,
    /// A ciphertext envelope exceeded the wire bound.
    #[error("ciphertext is {actual} bytes, above the {max} byte bound")]
    MessageTooLarge {
        /// Observed size.
        actual: usize,
        /// The bound.
        max: usize,
    },
    /// A session reached its message or byte bound and must be re-established.
    #[error("transport session is exhausted and must be re-established")]
    Exhausted,
    /// A message passed to the send path claimed the peer's role.
    #[error("outgoing message author does not match the session's local role")]
    WrongLocalAuthor,
    /// A decrypted message did not claim the authenticated peer's role.
    #[error("incoming message author does not match the session's authenticated peer role")]
    WrongPeerAuthor,
    /// An envelope named a session other than the Noise session that delivered it.
    #[error("message session id does not match the delivering Noise session")]
    SessionIdMismatch,
    /// The authenticated plaintext was not a valid canonical message.
    #[error(transparent)]
    Message(#[from] MessageError),
}

/// Builds the prologue both peers feed to the Noise handshake.
///
/// The initiator and responder digests are in initiator-then-responder order on both sides, so
/// the two peers compute identical bytes.
#[must_use]
pub fn prologue(
    initiator_descriptor_digest: &[u8; 32],
    responder_descriptor_digest: &[u8; 32],
) -> Vec<u8> {
    let mut writer = Writer::new();
    writer.u16(TRANSPORT_PROTOCOL_VERSION);
    writer.fixed(initiator_descriptor_digest);
    writer.fixed(responder_descriptor_digest);
    writer.finish()
}

/// Builds the prologue of a routed handshake.
///
/// Only the responder's descriptor is bound here, because the responder does not know which
/// peer is connecting until message 3. The initiator's descriptor digest travels encrypted in
/// message 3 instead, and the responder checks it against the peer it selects.
#[must_use]
pub fn routed_prologue(responder_descriptor_digest: &[u8; 32]) -> Vec<u8> {
    let mut writer = Writer::new();
    writer.u16(ROUTED_TRANSPORT_PROTOCOL_VERSION);
    writer.fixed(responder_descriptor_digest);
    writer.finish()
}

/// The transport version a first handshake message announces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeVersion {
    /// Version 1: an empty payload, both descriptor digests in the prologue.
    Direct,
    /// Version 2: the routed announcement, the responder digest in the prologue.
    Routed,
}

/// Classifies a first handshake message by its exact shape, before any Noise state exists.
///
/// The responder needs the version to choose its prologue, which Noise fixes at build time.
/// A message that is neither a bare ephemeral key nor an ephemeral key followed by exactly the
/// routed announcement is rejected, so no other payload is ever read in clear.
pub fn first_message_version(message: &[u8]) -> Result<HandshakeVersion, SessionError> {
    match message.len() {
        EPHEMERAL_KEY_BYTES => Ok(HandshakeVersion::Direct),
        length
            if length == EPHEMERAL_KEY_BYTES + ROUTED_HELLO.len()
                && message[EPHEMERAL_KEY_BYTES..] == ROUTED_HELLO =>
        {
            Ok(HandshakeVersion::Routed)
        }
        _ => Err(SessionError::UnsupportedVersion),
    }
}

/// The one parameter set this protocol speaks: Noise XX over X25519, ChaCha20-Poly1305, BLAKE2s.
///
/// The string is a compile-time constant, so its parse cannot fail at runtime.
fn noise_params() -> NoiseParams {
    "Noise_XX_25519_ChaChaPoly_BLAKE2s"
        .parse()
        .expect("the transport parameter string is a valid Noise name")
}

/// One side of an in-progress Noise XX handshake.
pub struct Handshake {
    inner: HandshakeState,
    local_role: Role,
    peer_role: Role,
    expected_peer_transport_key: [u8; 32],
    /// Payloads of this side's next writes, in order. Empty for a direct handshake.
    payloads: Vec<Vec<u8>>,
}

impl core::fmt::Debug for Handshake {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Handshake")
            .field("local_role", &self.local_role)
            .field("peer_role", &self.peer_role)
            .field("finished", &self.inner.is_handshake_finished())
            .finish()
    }
}

impl Handshake {
    /// Starts the initiator's side of an XX handshake.
    pub fn initiator(
        identity: &TransportIdentity,
        local_role: Role,
        peer_role: Role,
        prologue: &[u8],
        expected_peer_transport_key: [u8; 32],
    ) -> Result<Self, SessionError> {
        let builder = snow::Builder::new(noise_params())
            .local_private_key(identity.private_key())
            .prologue(prologue);
        let inner = builder
            .build_initiator()
            .map_err(|error| SessionError::Noise(error.to_string()))?;
        Ok(Self {
            inner,
            local_role,
            peer_role,
            expected_peer_transport_key,
            payloads: Vec::new(),
        })
    }

    /// Starts the initiator's side of a routed XX handshake.
    ///
    /// Message 1 carries the routed announcement in clear and message 3 carries
    /// `local_descriptor_digest` encrypted, so the responder can select this peer by its
    /// authenticated static key. The prologue binds only the responder's descriptor.
    pub fn routed_initiator(
        identity: &TransportIdentity,
        local_role: Role,
        peer_role: Role,
        responder_descriptor_digest: &[u8; 32],
        expected_peer_transport_key: [u8; 32],
        local_descriptor_digest: [u8; 32],
    ) -> Result<Self, SessionError> {
        let mut handshake = Self::initiator(
            identity,
            local_role,
            peer_role,
            &routed_prologue(responder_descriptor_digest),
            expected_peer_transport_key,
        )?;
        handshake.payloads = vec![ROUTED_HELLO.to_vec(), local_descriptor_digest.to_vec()];
        Ok(handshake)
    }

    /// Starts the responder's side of an XX handshake.
    pub fn responder(
        identity: &TransportIdentity,
        local_role: Role,
        peer_role: Role,
        prologue: &[u8],
        expected_peer_transport_key: [u8; 32],
    ) -> Result<Self, SessionError> {
        let builder = snow::Builder::new(noise_params())
            .local_private_key(identity.private_key())
            .prologue(prologue);
        let inner = builder
            .build_responder()
            .map_err(|error| SessionError::Noise(error.to_string()))?;
        Ok(Self {
            inner,
            local_role,
            peer_role,
            expected_peer_transport_key,
            payloads: Vec::new(),
        })
    }

    /// Reports whether the next step is a write.
    #[must_use]
    pub fn is_my_turn(&self) -> bool {
        self.inner.is_my_turn()
    }

    /// Reports whether the handshake has produced its transport keys.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.inner.is_handshake_finished()
    }

    /// Produces the next handshake message.
    pub fn write(&mut self) -> Result<Vec<u8>, SessionError> {
        if !self.inner.is_my_turn() {
            return Err(SessionError::OutOfTurn { action: "write" });
        }
        let payload = if self.payloads.is_empty() {
            Vec::new()
        } else {
            self.payloads.remove(0)
        };
        let mut buffer = vec![0u8; MAX_HANDSHAKE_MESSAGE_BYTES];
        let written = self
            .inner
            .write_message(&payload, &mut buffer)
            .map_err(|error| SessionError::Noise(error.to_string()))?;
        buffer.truncate(written);
        Ok(buffer)
    }

    /// Consumes one handshake message from the peer.
    pub fn read(&mut self, message: &[u8]) -> Result<(), SessionError> {
        if message.len() > MAX_HANDSHAKE_MESSAGE_BYTES {
            return Err(SessionError::HandshakeTooLarge {
                actual: message.len(),
                max: MAX_HANDSHAKE_MESSAGE_BYTES,
            });
        }
        let mut payload = vec![0u8; MAX_HANDSHAKE_MESSAGE_BYTES];
        self.inner
            .read_message(message, &mut payload)
            .map_err(|error| SessionError::Noise(error.to_string()))?;
        Ok(())
    }

    /// Finishes the handshake and returns the transport session.
    ///
    /// Fails if the peer's static transport key does not equal the key its descriptor
    /// advertised. This is the endpoint half of peer authentication (decision DM2-2).
    pub fn finish(self) -> Result<Session, SessionError> {
        if !self.inner.is_handshake_finished() {
            return Err(SessionError::OutOfTurn { action: "finish" });
        }
        let handshake_hash = self.inner.get_handshake_hash();
        let session_id: [u8; 32] =
            handshake_hash
                .try_into()
                .map_err(|_: core::array::TryFromSliceError| {
                    SessionError::UnexpectedHandshakeHash(handshake_hash.len())
                })?;
        let remote_static = self.inner.get_remote_static().ok_or_else(|| {
            SessionError::Noise("handshake produced no peer static key".to_owned())
        })?;
        if remote_static != &self.expected_peer_transport_key[..] {
            return Err(SessionError::PeerKeyMismatch);
        }
        let inner = self
            .inner
            .into_transport_mode()
            .map_err(|error| SessionError::Noise(error.to_string()))?;
        Ok(Session {
            inner,
            session_id,
            remote_static: self.expected_peer_transport_key,
            local_role: self.local_role,
            peer_role: self.peer_role,
            sent_messages: 0,
            received_messages: 0,
            sent_bytes: 0,
            received_bytes: 0,
        })
    }
}

/// The responder's side of a routed XX handshake.
///
/// It is built from the first message, because that message's announcement selects the
/// prologue. The peer is unknown until message 3, which proves possession of a static key and
/// carries the peer's descriptor digest; [`RoutedResponder::finish`] then asks the caller to
/// select the configured peer for exactly that key and digest, or rejects the connection.
pub struct RoutedResponder {
    inner: HandshakeState,
    local_role: Role,
    peer_role: Role,
}

impl core::fmt::Debug for RoutedResponder {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("RoutedResponder")
            .field("local_role", &self.local_role)
            .field("peer_role", &self.peer_role)
            .finish()
    }
}

impl RoutedResponder {
    /// Reads a routed first message under the routed prologue for this responder's descriptor.
    pub fn new(
        identity: &TransportIdentity,
        local_role: Role,
        peer_role: Role,
        responder_descriptor_digest: &[u8; 32],
        first_message: &[u8],
    ) -> Result<Self, SessionError> {
        if first_message_version(first_message)? != HandshakeVersion::Routed {
            return Err(SessionError::UnsupportedVersion);
        }
        let mut inner = snow::Builder::new(noise_params())
            .local_private_key(identity.private_key())
            .prologue(&routed_prologue(responder_descriptor_digest))
            .build_responder()
            .map_err(|error| SessionError::Noise(error.to_string()))?;
        let mut payload = vec![0u8; MAX_HANDSHAKE_MESSAGE_BYTES];
        let read = inner
            .read_message(first_message, &mut payload)
            .map_err(|error| SessionError::Noise(error.to_string()))?;
        if payload[..read] != ROUTED_HELLO {
            return Err(SessionError::UnsupportedVersion);
        }
        Ok(Self {
            inner,
            local_role,
            peer_role,
        })
    }

    /// Produces handshake message 2.
    pub fn write(&mut self) -> Result<Vec<u8>, SessionError> {
        if !self.inner.is_my_turn() {
            return Err(SessionError::OutOfTurn { action: "write" });
        }
        let mut buffer = vec![0u8; MAX_HANDSHAKE_MESSAGE_BYTES];
        let written = self
            .inner
            .write_message(&[], &mut buffer)
            .map_err(|error| SessionError::Noise(error.to_string()))?;
        buffer.truncate(written);
        Ok(buffer)
    }

    /// Reads message 3 and selects the peer it authenticates.
    ///
    /// `select` receives the authenticated remote static key and the descriptor digest the peer
    /// sent encrypted in message 3. It must return a value only for a configured peer whose
    /// descriptor advertises exactly that key and hashes to exactly that digest. The session
    /// is returned with that selection; `None` rejects the connection.
    pub fn finish<P>(
        mut self,
        third_message: &[u8],
        select: impl FnOnce(&[u8; 32], &[u8; 32]) -> Option<P>,
    ) -> Result<(Session, P), SessionError> {
        if third_message.len() > MAX_HANDSHAKE_MESSAGE_BYTES {
            return Err(SessionError::HandshakeTooLarge {
                actual: third_message.len(),
                max: MAX_HANDSHAKE_MESSAGE_BYTES,
            });
        }
        if self.inner.is_my_turn() {
            return Err(SessionError::OutOfTurn { action: "read" });
        }
        let mut payload = vec![0u8; MAX_HANDSHAKE_MESSAGE_BYTES];
        let read = self
            .inner
            .read_message(third_message, &mut payload)
            .map_err(|error| SessionError::Noise(error.to_string()))?;
        if !self.inner.is_handshake_finished() {
            return Err(SessionError::OutOfTurn { action: "finish" });
        }
        let digest: [u8; 32] = payload[..read]
            .try_into()
            .map_err(|_| SessionError::UnknownPeer)?;
        let remote_static: [u8; 32] = self
            .inner
            .get_remote_static()
            .ok_or_else(|| SessionError::Noise("handshake produced no peer static key".to_owned()))?
            .try_into()
            .map_err(|_| SessionError::UnknownPeer)?;
        let selected = select(&remote_static, &digest).ok_or(SessionError::UnknownPeer)?;
        let handshake_hash = self.inner.get_handshake_hash();
        let session_id: [u8; 32] =
            handshake_hash
                .try_into()
                .map_err(|_: core::array::TryFromSliceError| {
                    SessionError::UnexpectedHandshakeHash(handshake_hash.len())
                })?;
        let inner = self
            .inner
            .into_transport_mode()
            .map_err(|error| SessionError::Noise(error.to_string()))?;
        Ok((
            Session {
                inner,
                session_id,
                remote_static,
                local_role: self.local_role,
                peer_role: self.peer_role,
                sent_messages: 0,
                received_messages: 0,
                sent_bytes: 0,
                received_bytes: 0,
            },
            selected,
        ))
    }
}

/// An established transport session.
pub struct Session {
    inner: TransportState,
    session_id: [u8; 32],
    remote_static: [u8; 32],
    local_role: Role,
    peer_role: Role,
    sent_messages: u64,
    received_messages: u64,
    sent_bytes: usize,
    received_bytes: usize,
}

impl core::fmt::Debug for Session {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Session")
            .field("session_id", &hex::encode(self.session_id))
            .field("local_role", &self.local_role)
            .field("peer_role", &self.peer_role)
            .field("sent_messages", &self.sent_messages)
            .field("received_messages", &self.received_messages)
            .finish()
    }
}

impl Session {
    /// The Noise handshake hash, identical on both peers.
    #[must_use]
    pub fn session_id(&self) -> [u8; 32] {
        self.session_id
    }

    /// The peer's transport static public key, already checked against its descriptor.
    #[must_use]
    pub fn remote_transport_key(&self) -> [u8; 32] {
        self.remote_static
    }

    /// This endpoint's role in the deal.
    #[must_use]
    pub fn local_role(&self) -> Role {
        self.local_role
    }

    /// The peer's role in the deal.
    #[must_use]
    pub fn peer_role(&self) -> Role {
        self.peer_role
    }

    /// Checks and encrypts one canonical message for this endpoint.
    pub fn send_message(&mut self, message: &Message) -> Result<Vec<u8>, SessionError> {
        message.validate()?;
        if message.author != self.local_role {
            return Err(SessionError::WrongLocalAuthor);
        }
        if message.session_id != self.session_id {
            return Err(SessionError::SessionIdMismatch);
        }
        self.send(&message.encode())
    }

    /// Decrypts and validates one canonical message from the authenticated peer.
    pub fn receive_message(&mut self, ciphertext: &[u8]) -> Result<Message, SessionError> {
        let plaintext = self.receive(ciphertext)?;
        let message = Message::decode(&plaintext)?;
        if message.session_id != self.session_id {
            return Err(SessionError::SessionIdMismatch);
        }
        if message.author != self.peer_role {
            return Err(SessionError::WrongPeerAuthor);
        }
        Ok(message)
    }

    fn send(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, SessionError> {
        if plaintext.len() + 16 > MAX_MESSAGE_BYTES {
            return Err(SessionError::MessageTooLarge {
                actual: plaintext.len(),
                max: MAX_MESSAGE_BYTES,
            });
        }
        self.check_capacity(plaintext.len())?;
        let mut buffer = vec![0u8; plaintext.len() + 16];
        let written = self
            .inner
            .write_message(plaintext, &mut buffer)
            .map_err(|error| SessionError::Noise(error.to_string()))?;
        buffer.truncate(written);
        self.sent_messages += 1;
        self.sent_bytes += plaintext.len();
        Ok(buffer)
    }

    /// Decrypts one wire envelope. A message that does not authenticate is rejected here, which
    /// is what makes a cross-session message fail.
    fn receive(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, SessionError> {
        if ciphertext.len() > MAX_MESSAGE_BYTES {
            return Err(SessionError::MessageTooLarge {
                actual: ciphertext.len(),
                max: MAX_MESSAGE_BYTES,
            });
        }
        let plaintext_bound = ciphertext.len().saturating_sub(16);
        self.check_capacity(plaintext_bound)?;
        let mut buffer = vec![0u8; ciphertext.len()];
        let written = self
            .inner
            .read_message(ciphertext, &mut buffer)
            .map_err(|error| SessionError::Noise(error.to_string()))?;
        buffer.truncate(written);
        self.received_messages += 1;
        self.received_bytes += written;
        Ok(buffer)
    }

    /// Reports whether the session has reached a bound and must be re-established.
    #[must_use]
    pub fn is_exhausted(&self) -> bool {
        self.sent_messages + self.received_messages >= MAX_MESSAGES_PER_SESSION as u64
            || self.sent_bytes + self.received_bytes >= MAX_SESSION_BYTES
    }

    fn check_capacity(&self, additional_bytes: usize) -> Result<(), SessionError> {
        let message_limit_reached =
            self.sent_messages + self.received_messages >= MAX_MESSAGES_PER_SESSION as u64;
        let byte_limit_exceeded = self
            .sent_bytes
            .saturating_add(self.received_bytes)
            .saturating_add(additional_bytes)
            > MAX_SESSION_BYTES;
        if message_limit_reached || byte_limit_exceeded {
            Err(SessionError::Exhausted)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::MessageType;

    fn identities() -> (TransportIdentity, TransportIdentity) {
        (
            TransportIdentity::generate().expect("entropy"),
            TransportIdentity::generate().expect("entropy"),
        )
    }

    fn run_handshake(
        initiator_identity: &TransportIdentity,
        responder_identity: &TransportIdentity,
        initiator_digest: [u8; 32],
        responder_digest: [u8; 32],
    ) -> (Session, Session) {
        let prologue = prologue(&initiator_digest, &responder_digest);
        let mut initiator = Handshake::initiator(
            initiator_identity,
            Role::Buyer,
            Role::Seller,
            &prologue,
            responder_identity.public_key(),
        )
        .expect("initiator");
        let mut responder = Handshake::responder(
            responder_identity,
            Role::Seller,
            Role::Buyer,
            &prologue,
            initiator_identity.public_key(),
        )
        .expect("responder");

        let first = initiator.write().expect("message one");
        responder.read(&first).expect("read one");
        let second = responder.write().expect("message two");
        initiator.read(&second).expect("read two");
        let third = initiator.write().expect("message three");
        responder.read(&third).expect("read three");

        (
            initiator.finish().expect("initiator session"),
            responder.finish().expect("responder session"),
        )
    }

    #[test]
    fn both_peers_agree_on_the_session_id() {
        let (buyer, seller) = identities();
        let (initiator, responder) = run_handshake(&buyer, &seller, [0x11; 32], [0x22; 32]);
        assert_eq!(initiator.session_id(), responder.session_id());
        assert_eq!(initiator.local_role(), Role::Buyer);
        assert_eq!(responder.local_role(), Role::Seller);
    }

    #[test]
    fn messages_round_trip_between_the_two_peers() {
        let (buyer, seller) = identities();
        let (mut initiator, mut responder) = run_handshake(&buyer, &seller, [0x01; 32], [0x02; 32]);
        let ciphertext = initiator.send(b"offer").expect("send");
        assert_ne!(&ciphertext[..], b"offer");
        assert_eq!(ciphertext.len(), b"offer".len() + 16);
        assert_eq!(responder.receive(&ciphertext).expect("receive"), b"offer");
    }

    #[test]
    fn message_roles_and_session_ids_are_bound_to_the_noise_session() {
        let (buyer, seller) = identities();
        let (mut initiator, mut responder) = run_handshake(&buyer, &seller, [0x41; 32], [0x42; 32]);
        let wrong_author = Message::new(
            initiator.session_id(),
            [0x11; 16],
            1,
            Role::Seller,
            1,
            [0; 32],
            MessageType::Offer,
            b"forged author".to_vec(),
        )
        .expect("message");
        assert_eq!(
            initiator.send_message(&wrong_author),
            Err(SessionError::WrongLocalAuthor)
        );

        let wrong_session = Message::new(
            [0x99; 32],
            [0x11; 16],
            1,
            Role::Buyer,
            1,
            [0; 32],
            MessageType::Offer,
            b"wrong session".to_vec(),
        )
        .expect("message");
        assert_eq!(
            initiator.send_message(&wrong_session),
            Err(SessionError::SessionIdMismatch)
        );

        let forged_incoming = Message::new(
            initiator.session_id(),
            [0x11; 16],
            1,
            Role::Seller,
            1,
            [0; 32],
            MessageType::Offer,
            b"forged incoming author".to_vec(),
        )
        .expect("message");
        let ciphertext = initiator
            .send(&forged_incoming.encode())
            .expect("raw attacker send");
        assert_eq!(
            responder.receive_message(&ciphertext),
            Err(SessionError::WrongPeerAuthor)
        );
    }

    #[test]
    fn the_byte_bound_rejects_the_message_that_would_cross_it() {
        let (buyer, seller) = identities();
        let (mut initiator, _) = run_handshake(&buyer, &seller, [0x51; 32], [0x52; 32]);
        initiator.sent_bytes = MAX_SESSION_BYTES - 1;
        assert_eq!(initiator.send(b"ab"), Err(SessionError::Exhausted));
        assert_eq!(initiator.sent_bytes, MAX_SESSION_BYTES - 1);
    }

    #[test]
    fn a_session_from_another_key_cannot_open_the_message() {
        let (buyer, seller) = identities();
        let (third_party, _) = identities();
        let (mut initiator, _) = run_handshake(&buyer, &seller, [0x03; 32], [0x04; 32]);
        let (_, mut stranger) = run_handshake(&third_party, &seller, [0x05; 32], [0x06; 32]);
        let ciphertext = initiator.send(b"private").expect("send");
        assert!(stranger.receive(&ciphertext).is_err());
    }

    #[test]
    fn a_different_prologue_produces_a_different_session() {
        let (buyer, seller) = identities();
        let prologue_a = prologue(&[0x0a; 32], &[0x0b; 32]);
        let prologue_b = prologue(&[0x0a; 32], &[0x0c; 32]);
        let mut initiator = Handshake::initiator(
            &buyer,
            Role::Buyer,
            Role::Seller,
            &prologue_a,
            seller.public_key(),
        )
        .expect("initiator");
        let mut responder = Handshake::responder(
            &seller,
            Role::Seller,
            Role::Buyer,
            &prologue_b,
            buyer.public_key(),
        )
        .expect("responder");
        let first = initiator.write().expect("message one");
        responder.read(&first).expect("read one");
        let second = responder.write().expect("message two");
        assert!(
            initiator.read(&second).is_err(),
            "a mismatched prologue must fail the handshake"
        );
    }

    #[test]
    fn a_peer_with_the_wrong_static_key_is_rejected() {
        let (buyer, seller) = identities();
        let (_, other) = identities();
        let prologue = prologue(&[0x31; 32], &[0x32; 32]);
        let mut initiator = Handshake::initiator(
            &buyer,
            Role::Buyer,
            Role::Seller,
            &prologue,
            other.public_key(),
        )
        .expect("initiator");
        let mut responder = Handshake::responder(
            &seller,
            Role::Seller,
            Role::Buyer,
            &prologue,
            buyer.public_key(),
        )
        .expect("responder");
        let first = initiator.write().expect("message one");
        responder.read(&first).expect("read one");
        let second = responder.write().expect("message two");
        initiator.read(&second).expect("read two");
        let third = initiator.write().expect("message three");
        responder.read(&third).expect("read three");
        assert!(
            matches!(initiator.finish(), Err(SessionError::PeerKeyMismatch)),
            "a peer that is not the advertised key must be rejected"
        );
    }

    /// Runs a routed handshake; `tamper` may rewrite message 1 in flight.
    fn routed(
        buyer: &TransportIdentity,
        seller: &TransportIdentity,
        seller_digest: [u8; 32],
        buyer_digest: [u8; 32],
    ) -> (Handshake, Vec<u8>) {
        let mut initiator = Handshake::routed_initiator(
            buyer,
            Role::Buyer,
            Role::Seller,
            &seller_digest,
            seller.public_key(),
            buyer_digest,
        )
        .expect("routed initiator");
        let first = initiator.write().expect("message one");
        (initiator, first)
    }

    #[test]
    fn a_direct_first_message_is_exactly_the_ephemeral_key() {
        let (buyer, seller) = identities();
        let mut initiator = Handshake::initiator(
            &buyer,
            Role::Buyer,
            Role::Seller,
            &prologue(&[1; 32], &[2; 32]),
            seller.public_key(),
        )
        .expect("initiator");
        let first = initiator.write().expect("message one");
        assert_eq!(first.len(), EPHEMERAL_KEY_BYTES);
        assert_eq!(first_message_version(&first), Ok(HandshakeVersion::Direct));
        let (_, routed_first) = routed(&buyer, &seller, [2; 32], [1; 32]);
        assert_eq!(routed_first.len(), EPHEMERAL_KEY_BYTES + 2);
        assert_eq!(&routed_first[EPHEMERAL_KEY_BYTES..], &[0x00, 0x02]);
        assert_eq!(
            first_message_version(&routed_first),
            Ok(HandshakeVersion::Routed)
        );
        for malformed in [
            &routed_first[..EPHEMERAL_KEY_BYTES + 1],
            &[routed_first.as_slice(), &[0]].concat()[..],
            &[&routed_first[..EPHEMERAL_KEY_BYTES], &[0x00, 0x01][..]].concat()[..],
            &routed_first[..EPHEMERAL_KEY_BYTES - 1],
        ] {
            assert_eq!(
                first_message_version(malformed),
                Err(SessionError::UnsupportedVersion)
            );
        }
        assert_ne!(routed_prologue(&[2; 32]), prologue(&[1; 32], &[2; 32]));
    }

    #[test]
    fn a_routed_handshake_selects_the_peer_by_key_and_sent_digest() {
        let (buyer, seller) = identities();
        let (mut initiator, first) = routed(&buyer, &seller, [0x22; 32], [0x11; 32]);
        let mut responder =
            RoutedResponder::new(&seller, Role::Seller, Role::Buyer, &[0x22; 32], &first)
                .expect("responder");
        initiator
            .read(&responder.write().expect("two"))
            .expect("read two");
        let third = initiator.write().expect("three");
        let (session, selected) = responder
            .finish(&third, |key, digest| {
                (key == &buyer.public_key() && digest == &[0x11; 32]).then_some("peer-a")
            })
            .expect("selected");
        let initiator = initiator.finish().expect("initiator session");
        assert_eq!(selected, "peer-a");
        assert_eq!(session.session_id(), initiator.session_id());
        assert_eq!(session.remote_transport_key(), buyer.public_key());
        assert_eq!(session.peer_role(), Role::Buyer);
    }

    #[test]
    fn a_routed_handshake_with_no_selected_peer_is_rejected() {
        let (buyer, seller) = identities();
        let (mut initiator, first) = routed(&buyer, &seller, [0x22; 32], [0x11; 32]);
        let mut responder =
            RoutedResponder::new(&seller, Role::Seller, Role::Buyer, &[0x22; 32], &first)
                .expect("responder");
        initiator
            .read(&responder.write().expect("two"))
            .expect("read two");
        let third = initiator.write().expect("three");
        let result = responder.finish(&third, |_, _| None::<()>);
        assert_eq!(result.err(), Some(SessionError::UnknownPeer));
    }

    #[test]
    fn a_routed_handshake_for_another_responder_descriptor_fails_at_message_two() {
        let (buyer, seller) = identities();
        let (mut initiator, first) = routed(&buyer, &seller, [0x23; 32], [0x11; 32]);
        let mut responder =
            RoutedResponder::new(&seller, Role::Seller, Role::Buyer, &[0x22; 32], &first)
                .expect("responder");
        assert!(initiator.read(&responder.write().expect("two")).is_err());
    }

    #[test]
    fn stripping_the_announcement_downgrades_into_a_failed_handshake() {
        let (buyer, seller) = identities();
        let (mut initiator, first) = routed(&buyer, &seller, [0x22; 32], [0x11; 32]);
        // An active attacker removes the clear announcement: the responder sees version 1 and
        // uses the legacy prologue of the very same descriptors.
        let stripped = &first[..EPHEMERAL_KEY_BYTES];
        assert_eq!(
            first_message_version(stripped),
            Ok(HandshakeVersion::Direct)
        );
        let mut legacy = Handshake::responder(
            &seller,
            Role::Seller,
            Role::Buyer,
            &prologue(&[0x11; 32], &[0x22; 32]),
            buyer.public_key(),
        )
        .expect("legacy responder");
        legacy
            .read(stripped)
            .expect("legacy reads a bare ephemeral key");
        let second = legacy.write().expect("two");
        assert!(
            initiator.read(&second).is_err(),
            "the routed initiator's hash includes the announcement, so message 2 must not decrypt"
        );
        // Removing one byte of it is not a version at all.
        assert_eq!(
            RoutedResponder::new(
                &seller,
                Role::Seller,
                Role::Buyer,
                &[0x22; 32],
                &first[..EPHEMERAL_KEY_BYTES + 1],
            )
            .err(),
            Some(SessionError::UnsupportedVersion)
        );
    }

    #[test]
    fn adding_the_announcement_to_a_direct_handshake_fails_at_message_two() {
        let (buyer, seller) = identities();
        let mut initiator = Handshake::initiator(
            &buyer,
            Role::Buyer,
            Role::Seller,
            &prologue(&[0x11; 32], &[0x22; 32]),
            seller.public_key(),
        )
        .expect("initiator");
        let first = [initiator.write().expect("one"), ROUTED_HELLO.to_vec()].concat();
        let mut responder =
            RoutedResponder::new(&seller, Role::Seller, Role::Buyer, &[0x22; 32], &first)
                .expect("responder");
        assert!(initiator.read(&responder.write().expect("two")).is_err());
    }
}
