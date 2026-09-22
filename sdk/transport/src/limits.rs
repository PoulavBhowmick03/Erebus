//! Every transport bound in one place (decision DM2-7).
//!
//! Bounds are enforced at construction and at decode. Nothing is truncated, padded, or repaired:
//! an over-size input is an error, because a silently shortened message would change what the
//! transcript root commits to.

/// Largest message body accepted inside an envelope.
pub const MAX_BODY_BYTES: usize = 8192;
/// Largest ciphertext envelope accepted from the wire, including AEAD overhead.
pub const MAX_MESSAGE_BYTES: usize = 65536;
/// Largest number of messages retained for one deal transcript.
pub const MAX_MESSAGES_PER_DEAL: usize = 4096;
/// Largest number of messages sent or received in one session before a re-handshake is required.
pub const MAX_MESSAGES_PER_SESSION: usize = 4096;
/// Largest number of plaintext bytes sent or received in one session before a re-handshake.
pub const MAX_SESSION_BYTES: usize = 16 * 1024 * 1024;
/// Largest blob the relay stores for one mailbox.
pub const MAX_BLOB_BYTES: usize = 65536;
/// Largest number of blobs retained for one mailbox.
pub const MAX_BLOBS_PER_MAILBOX: usize = 256;
/// Default relay retention for a stored blob, in seconds (7 days, decision D05).
pub const RELAY_RETENTION_SECONDS: u64 = 7 * 24 * 60 * 60;
/// Largest number of endpoints accepted in one service descriptor.
pub const MAX_DESCRIPTOR_ENDPOINTS: usize = 4;
/// Largest number of assets accepted in one service descriptor.
pub const MAX_DESCRIPTOR_ASSETS: usize = 16;
/// Largest number of suites accepted in one service descriptor.
pub const MAX_DESCRIPTOR_SUITES: usize = 8;
