//! Minimal ciphertext relay (decision DM2-5).
//!
//! A relay is an authenticated mailbox that stores opaque blobs. It holds no session key and
//! cannot read a message: the mailbox id is an opaque client-chosen value, and the blob is
//! already Noise ciphertext. The relay sees mailbox ids, blob sizes, timing, and availability,
//! which is exactly the exposure decision DM2-9 accepts.
//!
//! Two implementations are provided. [`FileRelay`] is the durable, self-hostable one, used by the
//! `erebus-relay` service. [`MemoryRelay`] is an in-process implementation for tests and local
//! runs. Both enforce the same limits and the same idempotent `put`.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

use erebus_core::auth::Role;
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::limits::{MAX_BLOBS_PER_MAILBOX, MAX_BLOB_BYTES, RELAY_RETENTION_SECONDS};

/// Length of a mailbox identifier.
pub const MAILBOX_ID_BYTES: usize = 32;

/// An opaque mailbox identifier chosen by the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MailboxId([u8; MAILBOX_ID_BYTES]);

impl MailboxId {
    /// Wraps a mailbox id.
    #[must_use]
    pub const fn new(bytes: [u8; MAILBOX_ID_BYTES]) -> Self {
        Self(bytes)
    }

    /// Derives a direction-specific mailbox for one Noise session.
    ///
    /// Sessions are never resumed. A re-handshake therefore produces new mailboxes and prevents
    /// ciphertext from a dead Noise nonce space from blocking the restarted receiver.
    #[must_use]
    pub fn for_session(session_id: &[u8; 32], sender: Role) -> Self {
        use sha3::Digest;

        const DOMAIN: &[u8] = b"EREBUS_RELAY_MAILBOX_V1";
        let mut hasher = sha3::Sha3_256::new();
        hasher.update(DOMAIN);
        hasher.update(session_id);
        hasher.update([sender.tag()]);
        Self(hasher.finalize().into())
    }

    /// Parses a 64-character hex mailbox id.
    pub fn from_hex(value: &str) -> Result<Self, RelayError> {
        let bytes = hex::decode(value).map_err(|_| RelayError::InvalidMailbox)?;
        bytes
            .try_into()
            .map(Self)
            .map_err(|_| RelayError::InvalidMailbox)
    }

    /// Returns the raw bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; MAILBOX_ID_BYTES] {
        &self.0
    }
}

impl core::fmt::Display for MailboxId {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&hex::encode(self.0))
    }
}

/// A relay could not store or return a blob.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RelayError {
    /// The blob exceeded the relay bound.
    #[error("blob is {actual} bytes, above the {max} byte bound")]
    BlobTooLarge {
        /// Observed size.
        actual: usize,
        /// The bound.
        max: usize,
    },
    /// The mailbox already holds its maximum number of blobs.
    #[error("mailbox already holds the maximum {0} blobs")]
    MailboxFull(usize),
    /// The mailbox id was malformed.
    #[error("mailbox id is invalid")]
    InvalidMailbox,
    /// The underlying storage returned an error.
    #[error("relay I/O error: {0}")]
    Io(String),
    /// The stored index was not readable.
    #[error("relay index is corrupt: {0}")]
    Corrupt(String),
}

/// An acknowledgment of a stored blob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PutReceipt {
    /// The cursor the blob was stored at.
    pub cursor: u64,
    /// The time the blob expires.
    pub expires_at: u64,
    /// Whether an identical blob was already present.
    pub duplicate: bool,
}

/// A blob returned from a mailbox.
#[derive(Clone, PartialEq, Eq)]
pub struct StoredBlob {
    /// The blob's cursor, used to resume a read.
    pub cursor: u64,
    /// The opaque ciphertext.
    pub blob: Vec<u8>,
    /// The time the blob expires.
    pub expires_at: u64,
}

impl core::fmt::Debug for StoredBlob {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("StoredBlob")
            .field("cursor", &self.cursor)
            .field("bytes", &self.blob.len())
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// A ciphertext mailbox.
pub trait Relay: Send + Sync {
    /// Stores a blob and returns its acknowledgment. Identical blobs are idempotent.
    fn put(&self, mailbox: &MailboxId, blob: &[u8], now: u64) -> Result<PutReceipt, RelayError>;

    /// Returns non-expired blobs with a cursor greater than `after_cursor`.
    fn get(
        &self,
        mailbox: &MailboxId,
        after_cursor: u64,
        now: u64,
    ) -> Result<Vec<StoredBlob>, RelayError>;

    /// The retention applied to newly stored blobs, in seconds.
    fn retention_seconds(&self) -> u64;
}

/// One entry in a file-backed mailbox index.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexEntry {
    cursor: u64,
    expires_at: u64,
    hash: String,
    size: usize,
}

/// A durable, self-hostable mailbox store.
#[derive(Debug, Clone)]
pub struct FileRelay {
    root: PathBuf,
    retention_seconds: u64,
}

impl FileRelay {
    /// Opens a relay with the default retention (decision D05).
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, RelayError> {
        Self::with_retention(root, RELAY_RETENTION_SECONDS)
    }

    /// Opens a relay with an explicit retention.
    pub fn with_retention(
        root: impl Into<PathBuf>,
        retention_seconds: u64,
    ) -> Result<Self, RelayError> {
        let root = root.into();
        create_private_directory(&root)?;
        Ok(Self {
            root,
            retention_seconds,
        })
    }

    fn mailbox_directory(&self, mailbox: &MailboxId) -> PathBuf {
        self.root.join(mailbox.to_string())
    }

    fn load_index(directory: &Path) -> Result<Vec<IndexEntry>, RelayError> {
        let path = directory.join("index.json");
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| RelayError::Corrupt(error.to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(io_error(error)),
        }
    }

    fn save_index(directory: &Path, entries: &[IndexEntry]) -> Result<(), RelayError> {
        let bytes =
            serde_json::to_vec(entries).map_err(|error| RelayError::Corrupt(error.to_string()))?;
        let path = directory.join("index.json");
        let temporary = directory.join("index.json.tmp");
        write_private_file(&temporary, &bytes)?;
        fs::rename(&temporary, &path).map_err(io_error)
    }

    /// Removes expired entries and their blob files, returning the surviving index.
    fn prune(
        directory: &Path,
        entries: &[IndexEntry],
        now: u64,
    ) -> Result<Vec<IndexEntry>, RelayError> {
        let mut survivors = Vec::with_capacity(entries.len());
        for entry in entries {
            if entry.expires_at <= now {
                let _ = fs::remove_file(directory.join(format!("{}.bin", entry.cursor)));
            } else {
                survivors.push(entry.clone());
            }
        }
        Ok(survivors)
    }
}

impl Relay for FileRelay {
    fn put(&self, mailbox: &MailboxId, blob: &[u8], now: u64) -> Result<PutReceipt, RelayError> {
        if blob.len() > MAX_BLOB_BYTES {
            return Err(RelayError::BlobTooLarge {
                actual: blob.len(),
                max: MAX_BLOB_BYTES,
            });
        }
        let directory = self.mailbox_directory(mailbox);
        create_private_directory(&directory)?;
        let lock = open_lock(&directory)?;
        lock.lock_exclusive().map_err(io_error)?;

        let entries = Self::prune(&directory, &Self::load_index(&directory)?, now)?;
        let hash = blob_hash(blob);

        if let Some(existing) = entries
            .iter()
            .find(|entry| entry.hash == hash && entry.size == blob.len())
        {
            let receipt = PutReceipt {
                cursor: existing.cursor,
                expires_at: existing.expires_at,
                duplicate: true,
            };
            drop(lock);
            return Ok(receipt);
        }

        if entries.len() >= MAX_BLOBS_PER_MAILBOX {
            return Err(RelayError::MailboxFull(MAX_BLOBS_PER_MAILBOX));
        }

        let cursor = entries.iter().map(|entry| entry.cursor).max().unwrap_or(0) + 1;
        let expires_at = now.saturating_add(self.retention_seconds);
        write_private_file(&directory.join(format!("{cursor}.bin")), blob)?;
        let mut updated = entries;
        updated.push(IndexEntry {
            cursor,
            expires_at,
            hash,
            size: blob.len(),
        });
        Self::save_index(&directory, &updated)?;
        drop(lock);
        Ok(PutReceipt {
            cursor,
            expires_at,
            duplicate: false,
        })
    }

    fn get(
        &self,
        mailbox: &MailboxId,
        after_cursor: u64,
        now: u64,
    ) -> Result<Vec<StoredBlob>, RelayError> {
        let directory = self.mailbox_directory(mailbox);
        if !directory.exists() {
            return Ok(Vec::new());
        }
        let lock = open_lock(&directory)?;
        lock.lock_shared().map_err(io_error)?;

        let entries = Self::prune(&directory, &Self::load_index(&directory)?, now)?;
        let mut blobs = Vec::new();
        for entry in entries.iter().filter(|entry| entry.cursor > after_cursor) {
            let mut blob = Vec::new();
            File::open(directory.join(format!("{}.bin", entry.cursor)))
                .and_then(|mut file| file.read_to_end(&mut blob))
                .map_err(io_error)?;
            blobs.push(StoredBlob {
                cursor: entry.cursor,
                blob,
                expires_at: entry.expires_at,
            });
        }
        blobs.sort_by_key(|blob| blob.cursor);
        Ok(blobs)
    }

    fn retention_seconds(&self) -> u64 {
        self.retention_seconds
    }
}

/// An in-process mailbox store.
#[derive(Debug, Default)]
pub struct MemoryRelay {
    retention_seconds: u64,
    mailboxes: std::sync::Mutex<BTreeMap<MailboxId, Vec<StoredBlob>>>,
}

impl MemoryRelay {
    /// Creates an in-process relay with the default retention.
    #[must_use]
    pub fn new() -> Self {
        Self::with_retention(RELAY_RETENTION_SECONDS)
    }

    /// Creates an in-process relay with an explicit retention.
    #[must_use]
    pub fn with_retention(retention_seconds: u64) -> Self {
        Self {
            retention_seconds,
            mailboxes: std::sync::Mutex::new(BTreeMap::new()),
        }
    }
}

impl Relay for MemoryRelay {
    fn put(&self, mailbox: &MailboxId, blob: &[u8], now: u64) -> Result<PutReceipt, RelayError> {
        if blob.len() > MAX_BLOB_BYTES {
            return Err(RelayError::BlobTooLarge {
                actual: blob.len(),
                max: MAX_BLOB_BYTES,
            });
        }
        let mut mailboxes = self
            .mailboxes
            .lock()
            .map_err(|_| RelayError::Io("relay lock poisoned".to_owned()))?;
        let entries = mailboxes.entry(*mailbox).or_default();
        entries.retain(|entry| entry.expires_at > now);
        if let Some(existing) = entries.iter().find(|entry| entry.blob == blob) {
            return Ok(PutReceipt {
                cursor: existing.cursor,
                expires_at: existing.expires_at,
                duplicate: true,
            });
        }
        if entries.len() >= MAX_BLOBS_PER_MAILBOX {
            return Err(RelayError::MailboxFull(MAX_BLOBS_PER_MAILBOX));
        }
        let cursor = entries.iter().map(|entry| entry.cursor).max().unwrap_or(0) + 1;
        let stored = StoredBlob {
            cursor,
            blob: blob.to_vec(),
            expires_at: now.saturating_add(self.retention_seconds),
        };
        entries.push(stored);
        Ok(PutReceipt {
            cursor,
            expires_at: now.saturating_add(self.retention_seconds),
            duplicate: false,
        })
    }

    fn get(
        &self,
        mailbox: &MailboxId,
        after_cursor: u64,
        now: u64,
    ) -> Result<Vec<StoredBlob>, RelayError> {
        let mut mailboxes = self
            .mailboxes
            .lock()
            .map_err(|_| RelayError::Io("relay lock poisoned".to_owned()))?;
        let entries = mailboxes.entry(*mailbox).or_default();
        entries.retain(|entry| entry.expires_at > now);
        let mut blobs: Vec<StoredBlob> = entries
            .iter()
            .filter(|entry| entry.cursor > after_cursor)
            .cloned()
            .collect();
        blobs.sort_by_key(|blob| blob.cursor);
        Ok(blobs)
    }

    fn retention_seconds(&self) -> u64 {
        self.retention_seconds
    }
}

fn blob_hash(blob: &[u8]) -> String {
    use sha3::Digest;
    hex::encode(sha3::Sha3_256::digest(blob))
}

fn open_lock(directory: &Path) -> Result<File, RelayError> {
    let path = directory.join("relay.lock");
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(&path).map_err(io_error)?;
    set_private_file_permissions(&path)?;
    Ok(file)
}

fn create_private_directory(path: &Path) -> Result<(), RelayError> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(path).map_err(io_error)?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
    Ok(())
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), RelayError> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).map_err(io_error)?;
    set_private_file_permissions(path)?;
    file.write_all(bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)
}

fn set_private_file_permissions(path: &Path) -> Result<(), RelayError> {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(io_error)?;
    Ok(())
}

fn io_error(error: std::io::Error) -> RelayError {
    RelayError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mailbox() -> MailboxId {
        MailboxId::new([0x5a; MAILBOX_ID_BYTES])
    }

    fn exercise(relay: &dyn Relay) {
        assert_eq!(relay.retention_seconds(), RELAY_RETENTION_SECONDS);
        let first = relay.put(&mailbox(), b"one", 1_000).expect("put one");
        assert!(!first.duplicate);
        assert_eq!(first.expires_at, 1_000 + RELAY_RETENTION_SECONDS);

        let duplicate = relay.put(&mailbox(), b"one", 1_000).expect("put one again");
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.cursor, first.cursor);

        let second = relay.put(&mailbox(), b"two", 1_000).expect("put two");
        assert!(second.cursor > first.cursor);

        let blobs = relay.get(&mailbox(), 0, 1_000).expect("get");
        assert_eq!(blobs.len(), 2);
        assert_eq!(blobs[0].blob, b"one");
        assert_eq!(blobs[1].blob, b"two");

        let after_first = relay.get(&mailbox(), first.cursor, 1_000).expect("get");
        assert_eq!(after_first.len(), 1);
        assert_eq!(after_first[0].blob, b"two");
    }

    #[test]
    fn the_memory_relay_obeys_the_mailbox_contract() {
        exercise(&MemoryRelay::new());
    }

    #[test]
    fn the_file_relay_obeys_the_mailbox_contract() {
        let dir = tempfile::tempdir().expect("temp dir");
        let relay = FileRelay::open(dir.path()).expect("relay");
        exercise(&relay);
    }

    #[test]
    fn expired_blobs_are_pruned() {
        let relay = MemoryRelay::with_retention(100);
        relay.put(&mailbox(), b"one", 1_000).expect("put");
        assert_eq!(relay.get(&mailbox(), 0, 1_050).expect("get").len(), 1);
        assert_eq!(relay.get(&mailbox(), 0, 1_100).expect("get").len(), 0);
    }

    #[test]
    fn an_over_size_blob_is_rejected() {
        let relay = MemoryRelay::new();
        let error = relay
            .put(&mailbox(), &vec![0u8; MAX_BLOB_BYTES + 1], 1_000)
            .expect_err("too large");
        assert!(matches!(error, RelayError::BlobTooLarge { .. }));
    }

    #[test]
    fn a_mailbox_fills_to_its_bound() {
        let relay = MemoryRelay::new();
        for index in 0..MAX_BLOBS_PER_MAILBOX {
            relay
                .put(&mailbox(), &[index as u8; 8], 1_000)
                .expect("put within bound");
        }
        assert!(matches!(
            relay.put(&mailbox(), b"overflow", 1_000),
            Err(RelayError::MailboxFull(_))
        ));
    }

    #[test]
    fn mailbox_ids_round_trip_through_hex() {
        let id = MailboxId::new([0x11; MAILBOX_ID_BYTES]);
        assert_eq!(MailboxId::from_hex(&id.to_string()).expect("hex"), id);
        assert_eq!(MailboxId::from_hex("00"), Err(RelayError::InvalidMailbox));
    }

    #[test]
    fn mailboxes_are_scoped_to_session_and_direction() {
        let first = MailboxId::for_session(&[0x11; 32], Role::Buyer);
        let restarted = MailboxId::for_session(&[0x12; 32], Role::Buyer);
        let reverse = MailboxId::for_session(&[0x11; 32], Role::Seller);
        assert_ne!(first, restarted);
        assert_ne!(first, reverse);
    }

    #[cfg(unix)]
    #[test]
    fn file_relay_storage_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("temp dir");
        let relay = FileRelay::open(dir.path().join("relay")).expect("relay");
        let receipt = relay.put(&mailbox(), b"secret", 1_000).expect("put");
        let mailbox_directory = dir.path().join("relay").join(mailbox().to_string());
        assert_eq!(
            fs::metadata(&mailbox_directory)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        for name in [
            "relay.lock".to_owned(),
            "index.json".to_owned(),
            format!("{}.bin", receipt.cursor),
        ] {
            assert_eq!(
                fs::metadata(mailbox_directory.join(name))
                    .expect("metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}
