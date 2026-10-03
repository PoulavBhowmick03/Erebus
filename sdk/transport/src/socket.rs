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
    session::{prologue, Handshake, Session, MAX_HANDSHAKE_MESSAGE_BYTES},
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

    #[allow(clippy::too_many_arguments)]
    fn establish(
        mut stream: TcpStream,
        identity: &TransportIdentity,
        local: &ServiceDescriptor,
        peer: &ServiceDescriptor,
        role: Role,
        now: u64,
        timeout: Duration,
    ) -> Result<Self, SocketError> {
        if timeout.is_zero()
            || timeout > Duration::from_secs(300)
            || local.verify(now).is_err()
            || peer.verify(now).is_err()
            || local.transport_key != identity.public_key()
            || local.chain_namespace != peer.chain_namespace
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
        let (buyer, seller) = if role == Role::Buyer {
            (local, peer)
        } else {
            (peer, local)
        };
        let descriptor_digests = [
            buyer.digest().map_err(|_| SocketError::Configuration)?,
            seller.digest().map_err(|_| SocketError::Configuration)?,
        ];
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
        while !handshake.is_finished() {
            if handshake.is_my_turn() {
                let frame = handshake.write().map_err(|_| SocketError::Authentication)?;
                write_frame(&mut stream, &frame, MAX_HANDSHAKE_MESSAGE_BYTES, deadline)?;
            } else {
                handshake
                    .read(&read_frame(
                        &mut stream,
                        MAX_HANDSHAKE_MESSAGE_BYTES,
                        deadline,
                    )?)
                    .map_err(|_| SocketError::Authentication)?;
            }
        }
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
