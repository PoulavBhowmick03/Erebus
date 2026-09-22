//! Durable transcript storage (decision DM2-5).
//!
//! Clients persist the authenticated transcript **before** acknowledging a message (D05). The
//! store is therefore append-only and write-through: [`TranscriptStore::append`] validates the
//! message against the current transcript, writes it to disk, and only then returns.
//!
//! State is namespaced by an operator-chosen namespace (for example a deployment and local
//! identity fingerprint) and the deal id (D04), so a store is never shared accidentally across
//! deployments. Files are created with owner-only permissions; the transcript contains message
//! plaintext and is treated as sensitive.
//!
//! The on-disk format is a sequence of length-prefixed message bodies. Replaying it rebuilds the
//! same transcript root, which is what makes continuity across a restart and a fresh handshake
//! possible without persisting any session key.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

use fs2::FileExt;

use crate::message::{Message, MessageError};
use crate::transcript::{Transcript, TranscriptError};

const RECORD_CHECKSUM_BYTES: usize = 32;

/// A transcript could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// The namespace was empty or contained path-unsafe characters.
    #[error("store namespace `{0}` is invalid")]
    InvalidNamespace(String),
    /// The underlying filesystem returned an error.
    #[error("transcript store I/O error: {0}")]
    Io(String),
    /// A stored record could not be decoded, so the log is corrupt.
    #[error("stored transcript is corrupt: {0}")]
    Corrupt(String),
    /// The message did not extend the stored transcript.
    #[error(transparent)]
    Transcript(#[from] TranscriptError),
    /// The message could not be decoded.
    #[error(transparent)]
    Message(#[from] MessageError),
}

/// A durable, append-only transcript log.
pub trait TranscriptStore {
    /// Validates a message against the stored transcript and persists it.
    ///
    /// Returns the transcript including the new message. Callers persist before acknowledging.
    fn append(
        &self,
        namespace: &str,
        deal_id: [u8; 16],
        suite_id: u16,
        message: &Message,
    ) -> Result<Transcript, StoreError>;

    /// Loads and recomputes the transcript for a deal.
    fn load(
        &self,
        namespace: &str,
        deal_id: [u8; 16],
        suite_id: u16,
    ) -> Result<Transcript, StoreError>;

    /// Loads the stored messages for a deal in append order.
    fn messages(&self, namespace: &str, deal_id: [u8; 16]) -> Result<Vec<Message>, StoreError>;
}

/// A file-backed transcript store rooted at a directory.
#[derive(Debug, Clone)]
pub struct FileTranscriptStore {
    root: PathBuf,
}

impl FileTranscriptStore {
    /// Opens a store, creating its root directory if needed.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        create_private_directory(&root)?;
        Ok(Self { root })
    }

    /// The directory holding one deal's transcript.
    fn deal_directory(&self, namespace: &str, deal_id: [u8; 16]) -> Result<PathBuf, StoreError> {
        validate_namespace(namespace)?;
        Ok(self.root.join(namespace).join(hex::encode(deal_id)))
    }

    fn read_messages(&self, directory: &Path) -> Result<(Vec<Message>, usize, bool), StoreError> {
        let data = directory.join("transcript.log");
        let mut file = match File::open(&data) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((Vec::new(), 0, false));
            }
            Err(error) => return Err(io_error(error)),
        };
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(io_error)?;
        let mut messages = Vec::new();
        let mut cursor = 0usize;
        while cursor < bytes.len() {
            let record_start = cursor;
            if cursor + 4 > bytes.len() {
                return Ok((messages, record_start, true));
            }
            let length = u32::from_be_bytes([
                bytes[cursor],
                bytes[cursor + 1],
                bytes[cursor + 2],
                bytes[cursor + 3],
            ]) as usize;
            cursor += 4;
            if length > crate::limits::MAX_MESSAGE_BYTES {
                return Err(StoreError::Corrupt(
                    "stored record exceeds the message bound".to_owned(),
                ));
            }
            let Some(record_end) = cursor
                .checked_add(length)
                .and_then(|end| end.checked_add(RECORD_CHECKSUM_BYTES))
            else {
                return Err(StoreError::Corrupt(
                    "stored record length overflow".to_owned(),
                ));
            };
            if record_end > bytes.len() {
                return Ok((messages, record_start, true));
            }
            let record = &bytes[cursor..cursor + length];
            cursor += length;
            let checksum = &bytes[cursor..cursor + RECORD_CHECKSUM_BYTES];
            cursor += RECORD_CHECKSUM_BYTES;
            if checksum != record_checksum(record) {
                return Err(StoreError::Corrupt(
                    "stored record checksum mismatch".to_owned(),
                ));
            }
            messages.push(Message::decode_body(record)?);
        }
        Ok((messages, cursor, false))
    }

    fn recover_messages(&self, directory: &Path) -> Result<Vec<Message>, StoreError> {
        let (messages, valid_length, trailing_partial) = self.read_messages(directory)?;
        if trailing_partial {
            let path = directory.join("transcript.log");
            let file = OpenOptions::new()
                .write(true)
                .open(&path)
                .map_err(io_error)?;
            file.set_len(valid_length as u64).map_err(io_error)?;
            file.sync_all().map_err(io_error)?;
            sync_directory(directory)?;
        }
        Ok(messages)
    }

    fn write_record(handle: &mut File, message: &Message) -> Result<(), StoreError> {
        let body = message.encode_body();
        let length = u32::try_from(body.len())
            .map_err(|_| StoreError::Corrupt("record exceeds u32 length".to_owned()))?;
        let mut record = Vec::with_capacity(4 + body.len() + RECORD_CHECKSUM_BYTES);
        record.extend_from_slice(&length.to_be_bytes());
        record.extend_from_slice(&body);
        record.extend_from_slice(&record_checksum(&body));
        handle.write_all(&record).map_err(io_error)?;
        handle.flush().map_err(io_error)?;
        handle.sync_all().map_err(io_error)
    }
}

impl TranscriptStore for FileTranscriptStore {
    fn append(
        &self,
        namespace: &str,
        deal_id: [u8; 16],
        suite_id: u16,
        message: &Message,
    ) -> Result<Transcript, StoreError> {
        let directory = self.deal_directory(namespace, deal_id)?;
        create_private_directory(&directory)?;
        let lock = open_lock(&directory)?;
        lock.lock_exclusive().map_err(io_error)?;

        let stored = self.recover_messages(&directory)?;
        let mut transcript = Transcript::replay(deal_id, suite_id, &stored)?;
        transcript.append(message)?;

        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        options.mode(0o600);
        let path = directory.join("transcript.log");
        let mut handle = options.open(&path).map_err(io_error)?;
        set_private_file_permissions(&path)?;
        Self::write_record(&mut handle, message)?;
        sync_directory(&directory)?;
        // The caller may acknowledge only after this point. Dropping the lock is the ack grant.
        drop(lock);
        Ok(transcript)
    }

    fn load(
        &self,
        namespace: &str,
        deal_id: [u8; 16],
        suite_id: u16,
    ) -> Result<Transcript, StoreError> {
        let directory = self.deal_directory(namespace, deal_id)?;
        if !directory.exists() {
            return Transcript::new(deal_id, suite_id).map_err(StoreError::Transcript);
        }
        let lock = open_lock(&directory)?;
        lock.lock_exclusive().map_err(io_error)?;
        let stored = self.recover_messages(&directory)?;
        Transcript::replay(deal_id, suite_id, &stored).map_err(StoreError::Transcript)
    }

    fn messages(&self, namespace: &str, deal_id: [u8; 16]) -> Result<Vec<Message>, StoreError> {
        let directory = self.deal_directory(namespace, deal_id)?;
        if !directory.exists() {
            return Ok(Vec::new());
        }
        let lock = open_lock(&directory)?;
        lock.lock_exclusive().map_err(io_error)?;
        self.recover_messages(&directory)
    }
}

fn open_lock(directory: &Path) -> Result<File, StoreError> {
    let path = directory.join("transcript.lock");
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(&path).map_err(io_error)?;
    set_private_file_permissions(&path)?;
    Ok(file)
}

fn create_private_directory(path: &Path) -> Result<(), StoreError> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(path).map_err(io_error)?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
    Ok(())
}

fn set_private_file_permissions(path: &Path) -> Result<(), StoreError> {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(io_error)?;
    Ok(())
}

fn record_checksum(record: &[u8]) -> [u8; RECORD_CHECKSUM_BYTES] {
    use sha3::Digest;
    sha3::Sha3_256::digest(record).into()
}

fn sync_directory(path: &Path) -> Result<(), StoreError> {
    #[cfg(unix)]
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(io_error)?;
    Ok(())
}

fn validate_namespace(namespace: &str) -> Result<(), StoreError> {
    let valid = !namespace.is_empty()
        && namespace.len() <= 128
        && namespace
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && namespace != "."
        && namespace != "..";
    if valid {
        Ok(())
    } else {
        Err(StoreError::InvalidNamespace(namespace.to_owned()))
    }
}

fn io_error(error: std::io::Error) -> StoreError {
    StoreError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::MessageType;
    use erebus_core::auth::Role;

    const DEAL: [u8; 16] = [0x77; 16];

    fn message(author: Role, sequence: u64, parent: [u8; 32], body: &[u8]) -> Message {
        Message::new(
            [0x01; 32],
            DEAL,
            1,
            author,
            sequence,
            parent,
            MessageType::Offer,
            body.to_vec(),
        )
        .expect("valid message")
    }

    fn store() -> (tempfile::TempDir, FileTranscriptStore) {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = FileTranscriptStore::open(dir.path()).expect("store");
        (dir, store)
    }

    #[test]
    fn messages_survive_a_reopen() {
        let (dir, store) = store();
        let first = message(Role::Buyer, 1, [0; 32], b"offer");
        let root = store
            .append("testnet.market", DEAL, 1, &first)
            .expect("append")
            .root()
            .expect("root");

        let reopened = FileTranscriptStore::open(dir.path()).expect("reopen");
        let transcript = reopened.load("testnet.market", DEAL, 1).expect("load");
        assert_eq!(transcript.root().expect("root"), root);
        assert_eq!(transcript.total(), 1);
    }

    #[test]
    fn appending_enforces_ordering() {
        let (_dir, store) = store();
        store
            .append("ns", DEAL, 1, &message(Role::Buyer, 1, [0; 32], b"offer"))
            .expect("first");
        let duplicate = message(Role::Buyer, 1, [0; 32], b"offer");
        assert!(matches!(
            store.append("ns", DEAL, 1, &duplicate),
            Err(StoreError::Transcript(
                TranscriptError::SequenceOutOfOrder { .. }
            ))
        ));
    }

    #[test]
    fn a_rejected_append_writes_nothing() {
        let (_dir, store) = store();
        let bad = message(Role::Buyer, 2, [0; 32], b"out of order");
        assert!(store.append("ns", DEAL, 1, &bad).is_err());
        assert_eq!(store.messages("ns", DEAL).expect("messages").len(), 0);
    }

    #[test]
    fn path_traversal_namespaces_are_rejected() {
        let (_dir, store) = store();
        let message = message(Role::Buyer, 1, [0; 32], b"offer");
        for namespace in ["../escape", "a/b", "", "."] {
            assert!(matches!(
                store.append(namespace, DEAL, 1, &message),
                Err(StoreError::InvalidNamespace(_))
            ));
        }
    }

    #[test]
    fn complete_corrupt_records_are_reported() {
        let (dir, store) = store();
        store
            .append("ns", DEAL, 1, &message(Role::Buyer, 1, [0; 32], b"offer"))
            .expect("append");
        let log = dir
            .path()
            .join("ns")
            .join(hex::encode(DEAL))
            .join("transcript.log");
        let mut bytes = fs::read(&log).expect("read");
        bytes[8] ^= 0xff;
        fs::write(&log, bytes).expect("write");
        assert!(matches!(
            store.load("ns", DEAL, 1),
            Err(StoreError::Corrupt(_))
        ));
    }

    #[test]
    fn an_interrupted_final_record_is_removed_during_recovery() {
        let (dir, store) = store();
        store
            .append("ns", DEAL, 1, &message(Role::Buyer, 1, [0; 32], b"offer"))
            .expect("append");
        let log = dir
            .path()
            .join("ns")
            .join(hex::encode(DEAL))
            .join("transcript.log");
        let mut bytes = fs::read(&log).expect("read");
        bytes.truncate(bytes.len() - 1);
        fs::write(&log, bytes).expect("write");

        let transcript = store.load("ns", DEAL, 1).expect("recover");
        assert!(transcript.is_empty());
        assert_eq!(fs::metadata(log).expect("metadata").len(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn transcript_storage_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let (dir, store) = store();
        store
            .append("ns", DEAL, 1, &message(Role::Buyer, 1, [0; 32], b"secret"))
            .expect("append");
        let deal_directory = dir.path().join("ns").join(hex::encode(DEAL));
        assert_eq!(
            fs::metadata(&deal_directory)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        for name in ["transcript.log", "transcript.lock"] {
            assert_eq!(
                fs::metadata(deal_directory.join(name))
                    .expect("metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}
