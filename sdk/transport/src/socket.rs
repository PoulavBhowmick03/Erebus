//! Bounded TCP framing for the existing Noise XX session.
//!
//! This layer neither signs agreements nor acknowledges transcript persistence. The caller
//! must persist a sent message before delivery and a received message before acknowledgement.
//! Any uncertain frame closes this connection. Recovery establishes a fresh Noise handshake;
//! it must not resume this session's nonce counters.

use std::{
    fmt,
    io::{Read, Write},
    net::{Shutdown, TcpStream},
    time::{Duration, Instant},
};

use erebus_core::auth::Role;

use crate::{
    descriptor::ServiceDescriptor,
    identity::TransportIdentity,
    limits::MAX_MESSAGE_BYTES,
    message::Message,
    session::{
        first_message_version, prologue, Handshake, HandshakeVersion, RoutedResponder, Session,
        MAX_HANDSHAKE_MESSAGE_BYTES,
    },
};

/// A failed connection is never reusable, including after uncertain delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SocketError {
    /// Invalid duration, descriptor, role, or local identity binding.
    #[error("invalid authenticated connection configuration")]
    Configuration,
    /// Noise rejected a handshake, ciphertext, or message envelope.
    #[error("encrypted connection authentication failed; establish a new session")]
    Authentication,
    /// A peer sent a zero or oversized frame. No unbounded allocation is made.
    #[error("invalid encrypted frame length; establish a new session")]
    Frame,
    /// The whole operation exceeded its deadline, including partial progress.
    #[error("encrypted connection deadline exceeded; delivery may be uncertain")]
    Timeout,
    /// Socket I/O failed, with addresses and provider details deliberately redacted.
    #[error("encrypted connection unavailable; delivery may be uncertain")]
    Unavailable,
    /// A poisoned or peer-closed connection must be replaced, not retried in place.
    #[error("encrypted connection closed; establish a new session")]
    Closed,
}

/// One independently authenticated, synchronous connection.
/// Async callers must run its blocking operations outside their executor workers.
pub struct SocketChannel {
    stream: TcpStream,
    session: Session,
    timeout: Duration,
    closed: bool,
    descriptor_digests: [[u8; 32]; 2],
}

impl fmt::Debug for SocketChannel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SocketChannel { <redacted> }")
    }
}

impl SocketChannel {
    /// Authenticate an already-connected buyer socket against both signed descriptors.
    pub fn buyer(
        stream: TcpStream,
        identity: &TransportIdentity,
        buyer: &ServiceDescriptor,
        seller: &ServiceDescriptor,
        now: u64,
        timeout: Duration,
    ) -> Result<Self, SocketError> {
        Self::establish(stream, identity, buyer, seller, Role::Buyer, now, timeout)
    }

    /// Authenticate an accepted seller socket against both signed descriptors.
    pub fn seller(
        stream: TcpStream,
        identity: &TransportIdentity,
        seller: &ServiceDescriptor,
        buyer: &ServiceDescriptor,
        now: u64,
        timeout: Duration,
    ) -> Result<Self, SocketError> {
        Self::establish(stream, identity, seller, buyer, Role::Seller, now, timeout)
    }

    /// Authenticate a buyer socket with a routed (version 2) handshake.
    ///
    /// Use this only when the seller advertises routed transport out of band. The seller
    /// selects this buyer by its static key and the descriptor digest sent in message 3.
    pub fn buyer_routed(
        stream: TcpStream,
        identity: &TransportIdentity,
        buyer: &ServiceDescriptor,
        seller: &ServiceDescriptor,
        now: u64,
        timeout: Duration,
    ) -> Result<Self, SocketError> {
        let mut stream = prepare(stream, identity, buyer, seller, now, timeout)?;
        let descriptor_digests = digests(buyer, seller)?;
        let mut handshake = Handshake::routed_initiator(
            identity,
            Role::Buyer,
            Role::Seller,
            &descriptor_digests[1],
            seller.transport_key,
            descriptor_digests[0],
        )
        .map_err(|_| SocketError::Authentication)?;
        let deadline = Instant::now() + timeout;
        drive(&mut stream, &mut handshake, deadline)?;
        Self::open(stream, handshake, timeout, descriptor_digests)
    }

    /// Authenticate an accepted seller socket against one of many configured buyers.
    ///
    /// A routed (version 2) first message is answered under the routed prologue, and after
    /// message 3 `select` is asked for the configured buyer whose descriptor advertises the
    /// authenticated static key. That descriptor must also verify at `now`, share the seller's
    /// chain namespace, and hash to the digest the buyer sent; otherwise the connection fails.
    ///
    /// A direct (version 1) first message is accepted only for `legacy`, the one buyer
    /// configured before routed transport existed, with the version 1 prologue and key check.
    /// Without `legacy`, a direct first message fails. Either way the channel's descriptor
    /// digests are `[buyer, seller]`, exactly as [`SocketChannel::seller`] reports them.
    ///
    /// The whole handshake must finish within `handshake_timeout` (at most `timeout`), so a
    /// silent or slow connection cannot hold an accept loop; `timeout` bounds later messages.
    #[allow(clippy::too_many_arguments)]
    pub fn seller_routed<P>(
        stream: TcpStream,
        identity: &TransportIdentity,
        seller: &ServiceDescriptor,
        now: u64,
        timeout: Duration,
        handshake_timeout: Duration,
        legacy: Option<(ServiceDescriptor, P)>,
        select: impl FnOnce(&[u8; 32]) -> Option<(ServiceDescriptor, P)>,
    ) -> Result<(Self, ServiceDescriptor, P), SocketError> {
        if handshake_timeout.is_zero() {
            return Err(SocketError::Timeout);
        }
        let mut stream = prepare(stream, identity, seller, seller, now, timeout)?;
        let seller_digest = seller.digest().map_err(|_| SocketError::Configuration)?;
        let deadline = Instant::now() + handshake_timeout.min(timeout);
        let first = read_frame(&mut stream, MAX_HANDSHAKE_MESSAGE_BYTES, deadline)?;
        match first_message_version(&first).map_err(|_| SocketError::Authentication)? {
            HandshakeVersion::Direct => {
                let (buyer, tag) = legacy.ok_or(SocketError::Authentication)?;
                if !acceptable_peer(&buyer, seller, now) {
                    return Err(SocketError::Configuration);
                }
                let descriptor_digests = digests(&buyer, seller)?;
                let mut handshake = Handshake::responder(
                    identity,
                    Role::Seller,
                    Role::Buyer,
                    &prologue(&descriptor_digests[0], &descriptor_digests[1]),
                    buyer.transport_key,
                )
                .map_err(|_| SocketError::Authentication)?;
                handshake
                    .read(&first)
                    .map_err(|_| SocketError::Authentication)?;
                drive(&mut stream, &mut handshake, deadline)?;
                let channel = Self::open(stream, handshake, timeout, descriptor_digests)?;
                Ok((channel, buyer, tag))
            }
            HandshakeVersion::Routed => {
                let mut handshake = RoutedResponder::new(
                    identity,
                    Role::Seller,
                    Role::Buyer,
                    &seller_digest,
                    &first,
                )
                .map_err(|_| SocketError::Authentication)?;
                let second = handshake.write().map_err(|_| SocketError::Authentication)?;
                write_frame(&mut stream, &second, MAX_HANDSHAKE_MESSAGE_BYTES, deadline)?;
                let third = read_frame(&mut stream, MAX_HANDSHAKE_MESSAGE_BYTES, deadline)?;
                let (session, (buyer, tag)) = handshake
                    .finish(&third, |key, sent_digest| {
                        let (buyer, tag) = select(key)?;
                        let digest = buyer.digest().ok()?;
                        (buyer.transport_key == *key
                            && digest == *sent_digest
                            && acceptable_peer(&buyer, seller, now))
                        .then_some((buyer, tag))
                    })
                    .map_err(|_| SocketError::Authentication)?;
                let descriptor_digests = digests(&buyer, seller)?;
                Ok((
                    Self {
                        stream,
                        session,
                        timeout,
                        closed: false,
                        descriptor_digests,
                    },
                    buyer,
                    tag,
                ))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn establish(
        stream: TcpStream,
        identity: &TransportIdentity,
        local: &ServiceDescriptor,
        peer: &ServiceDescriptor,
        role: Role,
        now: u64,
        timeout: Duration,
    ) -> Result<Self, SocketError> {
        let mut stream = prepare(stream, identity, local, peer, now, timeout)?;
        let (buyer, seller) = if role == Role::Buyer {
            (local, peer)
        } else {
            (peer, local)
        };
        let descriptor_digests = digests(buyer, seller)?;
        let binding = prologue(&descriptor_digests[0], &descriptor_digests[1]);
        let mut handshake = if role == Role::Buyer {
            Handshake::initiator(
                identity,
                Role::Buyer,
                Role::Seller,
                &binding,
                peer.transport_key,
            )
        } else {
            Handshake::responder(
                identity,
                Role::Seller,
                Role::Buyer,
                &binding,
                peer.transport_key,
            )
        }
        .map_err(|_| SocketError::Authentication)?;
        let deadline = Instant::now() + timeout;
        drive(&mut stream, &mut handshake, deadline)?;
        Self::open(stream, handshake, timeout, descriptor_digests)
    }

    fn open(
        stream: TcpStream,
        handshake: Handshake,
        timeout: Duration,
        descriptor_digests: [[u8; 32]; 2],
    ) -> Result<Self, SocketError> {
        let session = handshake
            .finish()
            .map_err(|_| SocketError::Authentication)?;
        Ok(Self {
            stream,
            session,
            timeout,
            closed: false,
            descriptor_digests,
        })
    }

    /// Ephemeral handshake identity. Message digests deliberately do not include it.
    pub fn session_id(&self) -> [u8; 32] {
        self.session.session_id()
    }

    /// Role of this authenticated endpoint, not a role supplied by a received payload.
    pub fn local_role(&self) -> Role {
        self.session.local_role()
    }

    /// Buyer and seller descriptor digests authenticated in this connection's prologue.
    pub fn descriptor_digests(&self) -> [[u8; 32]; 2] {
        self.descriptor_digests
    }

    /// Encrypt and write one already-persisted message. This is not a durable peer acknowledgement.
    pub fn send(&mut self, message: &Message) -> Result<(), SocketError> {
        if self.closed {
            return Err(SocketError::Closed);
        }
        let result = self
            .session
            .send_message(message)
            .map_err(|_| SocketError::Authentication)
            .and_then(|ciphertext| {
                write_frame(
                    &mut self.stream,
                    &ciphertext,
                    MAX_MESSAGE_BYTES,
                    Instant::now() + self.timeout,
                )
            });
        self.finish_io(result)
    }

    /// Receive one authenticated message. Persist it before acknowledging it at the protocol layer.
    pub fn receive(&mut self) -> Result<Message, SocketError> {
        if self.closed {
            return Err(SocketError::Closed);
        }
        let result = read_frame(
            &mut self.stream,
            MAX_MESSAGE_BYTES,
            Instant::now() + self.timeout,
        )
        .and_then(|frame| {
            self.session
                .receive_message(&frame)
                .map_err(|_| SocketError::Authentication)
        });
        self.finish_io(result)
    }

    /// Close explicitly. No nonce counter or socket is resumed after closure.
    pub fn close(&mut self) {
        self.closed = true;
        let _ = self.stream.shutdown(Shutdown::Both);
    }

    fn finish_io<T>(&mut self, result: Result<T, SocketError>) -> Result<T, SocketError> {
        if result.is_err() {
            self.close();
        }
        result
    }
}

impl Drop for SocketChannel {
    fn drop(&mut self) {
        self.close();
    }
}

/// Checks the configuration every handshake shares and puts the stream in blocking mode.
fn prepare(
    stream: TcpStream,
    identity: &TransportIdentity,
    local: &ServiceDescriptor,
    peer: &ServiceDescriptor,
    now: u64,
    timeout: Duration,
) -> Result<TcpStream, SocketError> {
    if timeout.is_zero()
        || timeout > Duration::from_secs(300)
        || local.verify(now).is_err()
        || local.transport_key != identity.public_key()
        || !acceptable_peer(peer, local, now)
    {
        return Err(SocketError::Configuration);
    }
    // Accepted sockets can inherit a nonblocking listener's mode on some platforms.
    // These synchronous operations rely on blocking I/O with absolute deadlines.
    stream
        .set_nonblocking(false)
        .map_err(|_| SocketError::Unavailable)?;
    stream
        .set_nodelay(true)
        .map_err(|_| SocketError::Unavailable)?;
    Ok(stream)
}

/// A peer descriptor is usable against `local` if it verifies and shares its chain.
fn acceptable_peer(peer: &ServiceDescriptor, local: &ServiceDescriptor, now: u64) -> bool {
    peer.verify(now).is_ok() && peer.chain_namespace == local.chain_namespace
}

fn digests(
    buyer: &ServiceDescriptor,
    seller: &ServiceDescriptor,
) -> Result<[[u8; 32]; 2], SocketError> {
    Ok([
        buyer.digest().map_err(|_| SocketError::Configuration)?,
        seller.digest().map_err(|_| SocketError::Configuration)?,
    ])
}

/// Exchanges the remaining handshake frames within one absolute deadline.
fn drive(
    stream: &mut TcpStream,
    handshake: &mut Handshake,
    deadline: Instant,
) -> Result<(), SocketError> {
    while !handshake.is_finished() {
        if handshake.is_my_turn() {
            let frame = handshake.write().map_err(|_| SocketError::Authentication)?;
            write_frame(stream, &frame, MAX_HANDSHAKE_MESSAGE_BYTES, deadline)?;
        } else {
            handshake
                .read(&read_frame(stream, MAX_HANDSHAKE_MESSAGE_BYTES, deadline)?)
                .map_err(|_| SocketError::Authentication)?;
        }
    }
    Ok(())
}

fn remaining(deadline: Instant) -> Result<Duration, SocketError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|time| !time.is_zero())
        .ok_or(SocketError::Timeout)
}

fn io_error(error: std::io::Error) -> SocketError {
    match error.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => SocketError::Timeout,
        _ => SocketError::Unavailable,
    }
}

fn read_exact(
    stream: &mut TcpStream,
    mut bytes: &mut [u8],
    deadline: Instant,
) -> Result<(), SocketError> {
    while !bytes.is_empty() {
        stream
            .set_read_timeout(Some(remaining(deadline)?))
            .map_err(io_error)?;
        match stream.read(bytes) {
            Ok(0) => return Err(SocketError::Closed),
            Ok(count) => bytes = &mut bytes[count..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(io_error(error)),
        }
    }
    Ok(())
}

fn write_all(
    stream: &mut TcpStream,
    mut bytes: &[u8],
    deadline: Instant,
) -> Result<(), SocketError> {
    while !bytes.is_empty() {
        stream
            .set_write_timeout(Some(remaining(deadline)?))
            .map_err(io_error)?;
        match stream.write(bytes) {
            Ok(0) => return Err(SocketError::Closed),
            Ok(count) => bytes = &bytes[count..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(io_error(error)),
        }
    }
    Ok(())
}

fn read_frame(
    stream: &mut TcpStream,
    max: usize,
    deadline: Instant,
) -> Result<Vec<u8>, SocketError> {
    let mut prefix = [0; 4];
    read_exact(stream, &mut prefix, deadline)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 || length > max {
        return Err(SocketError::Frame);
    }
    let mut frame = vec![0; length];
    read_exact(stream, &mut frame, deadline)?;
    Ok(frame)
}

fn write_frame(
    stream: &mut TcpStream,
    frame: &[u8],
    max: usize,
    deadline: Instant,
) -> Result<(), SocketError> {
    if frame.is_empty() || frame.len() > max {
        return Err(SocketError::Frame);
    }
    write_all(stream, &(frame.len() as u32).to_be_bytes(), deadline)?;
    write_all(stream, frame, deadline)
}

#[cfg(test)]
mod tests;
