//! The authenticated message envelope (decision DM2-3).
//!
//! Every negotiation message is one canonical structure carried inside a Noise transport
//! message: `session_id` (delivery metadata) and the transcript-relevant body. The body's fields
//! are `deal_id`, `revision`, `author`, `sequence`, `parent_hash`, `message_type`, and an opaque
//! payload.
//!
//! The one non-obvious rule is that `session_id` is in the envelope but **not** in
//! [`Message::digest`]. The transcript is per deal and must survive a re-handshake after a
//! restart (decision DM2-1), so it cannot be keyed by an ephemeral session. `session_id` is still
//! authenticated, because the whole envelope is the Noise AEAD plaintext.
//!
//! `Debug` prints only routing metadata. It never prints the body or `session_id`, so a message
//! cannot leak its content through a log line or a model-visible string.

use core::fmt;

use erebus_core::auth::Role;
use erebus_core::encoding::{EncodingError, Reader, Writer};
use erebus_core::suite::{self, SuiteError};

use crate::limits::MAX_BODY_BYTES;
use crate::session::TRANSPORT_PROTOCOL_VERSION;

/// Domain separation for a message digest.
pub const MESSAGE_DOMAIN: &[u8] = b"EREBUS_MESSAGE_V1";

/// The kind of negotiation message a body carries.
///
/// The transport treats the payload as opaque. The offchain Eleusis state machine interprets it;
/// the message type only tells the receiver which transition to attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageType {
    /// An initial offer.
    Offer,
    /// A counteroffer to a prior offer.
    Counter,
    /// An authorization over an agreed revision.
    Authorization,
}

impl MessageType {
    /// Stable encoding tag.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::Offer => 1,
            Self::Counter => 2,
            Self::Authorization => 3,
        }
    }

    /// Parses a stable encoding tag.
    pub fn from_tag(tag: u8) -> Result<Self, MessageError> {
        match tag {
            1 => Ok(Self::Offer),
            2 => Ok(Self::Counter),
            3 => Ok(Self::Authorization),
            _ => Err(MessageError::UnknownMessageType(tag)),
        }
    }

    /// Human-readable name for diagnostics.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Offer => "offer",
            Self::Counter => "counter",
            Self::Authorization => "authorization",
        }
    }
}

/// A message could not be constructed or decoded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MessageError {
    /// The protocol version is not the one this crate speaks.
    #[error("transport protocol version {0} is not supported")]
    UnsupportedProtocolVersion(u16),
    /// The body exceeded the transport bound.
    #[error("message body is {actual} bytes, above the {max} byte bound")]
    BodyTooLarge {
        /// Observed size.
        actual: usize,
        /// The bound.
        max: usize,
    },
    /// Agreement revisions start at one.
    #[error("message revision must be greater than zero")]
    InvalidRevision,
    /// Per-author transcript sequence numbers start at one.
    #[error("message sequence must be greater than zero")]
    InvalidSequence,
    /// The message type tag named no known type.
    #[error("unknown message type tag {0}")]
    UnknownMessageType(u8),
    /// A canonical field could not be read, including an unknown author role tag.
    #[error(transparent)]
    Encoding(#[from] EncodingError),
    /// The agreement suite is not implemented.
    #[error(transparent)]
    Suite(#[from] SuiteError),
}

/// One negotiation message.
#[derive(Clone, PartialEq, Eq)]
pub struct Message {
    /// The session that delivered the message. Envelope metadata, not part of the digest.
    pub session_id: [u8; 32],
    /// The deal this message belongs to.
    pub deal_id: [u8; 16],
    /// The agreement revision this message concerns.
    pub revision: u32,
    /// Which participant authored it.
    pub author: Role,
    /// Per-author, per-deal message counter, starting at one and contiguous.
    pub sequence: u64,
    /// The author's previous message link, or zero for the first message.
    pub parent_hash: [u8; 32],
    /// The transition the payload requests.
    pub message_type: MessageType,
    /// Opaque payload, interpreted by the offchain state machine.
    pub body: Vec<u8>,
}

impl fmt::Debug for Message {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Message")
            .field("deal_id", &hex::encode(self.deal_id))
            .field("revision", &self.revision)
            .field("author", &self.author)
            .field("sequence", &self.sequence)
            .field("message_type", &self.message_type)
            .field(
                "body",
                &format_args!("{} bytes <redacted>", self.body.len()),
            )
            .finish_non_exhaustive()
    }
}

impl Message {
    /// Builds a message for the current protocol version.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session_id: [u8; 32],
        deal_id: [u8; 16],
        revision: u32,
        author: Role,
        sequence: u64,
        parent_hash: [u8; 32],
        message_type: MessageType,
        body: Vec<u8>,
    ) -> Result<Self, MessageError> {
        if revision == 0 {
            return Err(MessageError::InvalidRevision);
        }
        if sequence == 0 {
            return Err(MessageError::InvalidSequence);
        }
        if body.len() > MAX_BODY_BYTES {
            return Err(MessageError::BodyTooLarge {
                actual: body.len(),
                max: MAX_BODY_BYTES,
            });
        }
        Ok(Self {
            session_id,
            deal_id,
            revision,
            author,
            sequence,
            parent_hash,
            message_type,
            body,
        })
    }

    /// Revalidates the public fields before a message crosses a trust boundary.
    pub fn validate(&self) -> Result<(), MessageError> {
        if self.revision == 0 {
            return Err(MessageError::InvalidRevision);
        }
        if self.sequence == 0 {
            return Err(MessageError::InvalidSequence);
        }
        if self.body.len() > MAX_BODY_BYTES {
            return Err(MessageError::BodyTooLarge {
                actual: self.body.len(),
                max: MAX_BODY_BYTES,
            });
        }
        Ok(())
    }

    /// Encodes the transcript-relevant body. `session_id` is deliberately excluded.
    #[must_use]
    pub fn encode_body(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.u16(TRANSPORT_PROTOCOL_VERSION);
        writer.fixed(&self.deal_id);
        writer.u32(self.revision);
        writer.u8(self.author.tag());
        writer.u64(self.sequence);
        writer.fixed(&self.parent_hash);
        writer.u8(self.message_type.tag());
        writer.bytes(&self.body);
        writer.finish()
    }

    /// Encodes the complete envelope, including `session_id`.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.fixed(&self.session_id);
        writer.fixed(&self.encode_body());
        writer.finish()
    }

    /// Decodes a complete envelope.
    pub fn decode(bytes: &[u8]) -> Result<Self, MessageError> {
        let mut reader = Reader::new(bytes);
        let session_id = reader.fixed::<32>("session_id")?;
        let message = Self::decode_body_after_session(&mut reader)?;
        reader.finish()?;
        Ok(Self {
            session_id,
            ..message
        })
    }

    fn decode_body_after_session(reader: &mut Reader<'_>) -> Result<Self, MessageError> {
        let protocol_version = reader.u16("protocol_version")?;
        if protocol_version != TRANSPORT_PROTOCOL_VERSION {
            return Err(MessageError::UnsupportedProtocolVersion(protocol_version));
        }
        let deal_id = reader.fixed::<16>("deal_id")?;
        let revision = reader.u32("revision")?;
        if revision == 0 {
            return Err(MessageError::InvalidRevision);
        }
        let author = Role::from_tag(reader.u8("author")?)?;
        let sequence = reader.u64("sequence")?;
        if sequence == 0 {
            return Err(MessageError::InvalidSequence);
        }
        let parent_hash = reader.fixed::<32>("parent_hash")?;
        let message_type = MessageType::from_tag(reader.u8("message_type")?)?;
        let body = reader.bytes("body", 0, MAX_BODY_BYTES)?.to_vec();
        Ok(Self {
            session_id: [0u8; 32],
            deal_id,
            revision,
            author,
            sequence,
            parent_hash,
            message_type,
            body,
        })
    }

    /// Decodes a transcript body that was stored without its session.
    pub fn decode_body(bytes: &[u8]) -> Result<Self, MessageError> {
        let mut reader = Reader::new(bytes);
        let message = Self::decode_body_after_session(&mut reader)?;
        reader.finish()?;
        Ok(message)
    }

    /// The digest of the transcript-relevant body under the agreement suite.
    ///
    /// This is the leaf that the transcript chains and the agreement's `transcript_root`
    /// ultimately commit to.
    pub fn digest(&self, suite_id: u16) -> Result<[u8; 32], MessageError> {
        self.validate()?;
        let suite = suite::suite(suite_id)?;
        Ok(suite.hash(&[MESSAGE_DOMAIN, &self.encode_body()]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUYER: Role = Role::Buyer;
    const SELLER: Role = Role::Seller;

    fn sample() -> Message {
        Message::new(
            [0x01; 32],
            [0x02; 16],
            1,
            BUYER,
            1,
            [0x00; 32],
            MessageType::Offer,
            b"terms".to_vec(),
        )
        .expect("valid message")
    }

    #[test]
    fn envelope_round_trips() {
        let message = sample();
        assert_eq!(Message::decode(&message.encode()).expect("decode"), message);
    }

    #[test]
    fn digest_ignores_the_session_id() {
        let message = sample();
        let mut other = message.clone();
        other.session_id = [0xff; 32];
        assert_eq!(
            message.digest(1).expect("digest"),
            other.digest(1).expect("digest"),
            "the transcript digest must survive a re-handshake after a restart"
        );
    }

    #[test]
    fn digest_changes_with_the_body() {
        let mut other = sample();
        other.body = b"different".to_vec();
        assert_ne!(
            sample().digest(1).expect("digest"),
            other.digest(1).expect("digest")
        );
    }

    #[test]
    fn an_unknown_protocol_version_is_rejected() {
        let message = sample();
        let mut body = message.encode_body();
        body[1] = 0x02;
        assert_eq!(
            Message::decode_body(&body),
            Err(MessageError::UnsupportedProtocolVersion(2))
        );
    }

    #[test]
    fn zero_revision_and_sequence_are_rejected() {
        assert_eq!(
            Message::new(
                [0; 32],
                [1; 16],
                0,
                Role::Buyer,
                1,
                [0; 32],
                MessageType::Offer,
                Vec::new(),
            ),
            Err(MessageError::InvalidRevision)
        );
        assert_eq!(
            Message::new(
                [0; 32],
                [1; 16],
                1,
                Role::Buyer,
                0,
                [0; 32],
                MessageType::Offer,
                Vec::new(),
            ),
            Err(MessageError::InvalidSequence)
        );
    }

    #[test]
    fn mutated_public_fields_are_revalidated_before_digesting() {
        let mut message = sample();
        message.revision = 0;
        assert_eq!(message.digest(1), Err(MessageError::InvalidRevision));
    }

    #[test]
    fn an_over_long_body_is_rejected_at_construction_and_decode() {
        assert!(matches!(
            Message::new(
                [0; 32],
                [0; 16],
                1,
                SELLER,
                1,
                [0; 32],
                MessageType::Counter,
                vec![0u8; MAX_BODY_BYTES + 1],
            ),
            Err(MessageError::BodyTooLarge { .. })
        ));
        // The body's `u16` length prefix precedes its five bytes; corrupt its high byte so the
        // declared length exceeds MAX_BODY_BYTES.
        let mut body = sample().encode_body();
        let length_high_byte = body.len() - 2 - 5;
        body[length_high_byte] = 0x20;
        assert!(Message::decode_body(&body).is_err());
    }

    #[test]
    fn debug_hides_the_body_and_session() {
        let message = sample();
        let rendered = format!("{message:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("terms"));
        assert!(!rendered.contains(&hex::encode(message.session_id)));
    }
}
