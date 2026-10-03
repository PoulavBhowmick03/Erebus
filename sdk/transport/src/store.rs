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
use erebus_core::encoding::{Reader, Writer};

const RECORD_CHECKSUM_BYTES: usize = 32;
const FREEZE_BYTES: usize = 116;

/// A transcript could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// The transcript was sealed before final agreement authorizations.
    #[error("negotiation transcript is frozen")]
    Frozen,
    /// A typed negotiation transition was rejected before writing.
    #[error("negotiation transition rejected")]
    Transition,
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

/// Durable boundary between negotiation messages and final agreement signatures.
///
/// This record binds a transcript prefix and selected proposal. It does not itself prove
/// consent; the typed negotiation layer checks both acceptance messages before creating it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrozenTranscript {
    hash_version: u16,
    deal_id: [u8; 16],
    root: [u8; 32],
    proposal_digest: [u8; 32],
}

impl FrozenTranscript {
    /// The immutable root used by the final agreement.
    #[must_use]
    pub fn root(&self) -> [u8; 32] {
        self.root
    }

    /// Digest of the accepted proposal, before inserting the transcript root.
    #[must_use]
    pub fn proposal_digest(&self) -> [u8; 32] {
        self.proposal_digest
    }

    fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.u16(1);
        writer.u16(self.hash_version);
        writer.fixed(&self.deal_id);
        writer.fixed(&self.root);
        writer.fixed(&self.proposal_digest);
        let mut bytes = writer.finish();
        bytes.extend_from_slice(&record_checksum(&bytes));
        bytes
    }

    fn decode(bytes: &[u8]) -> Result<Self, StoreError> {
        if bytes.len() != FREEZE_BYTES
            || bytes[FREEZE_BYTES - 32..] != record_checksum(&bytes[..FREEZE_BYTES - 32])
        {
            return Err(StoreError::Corrupt("invalid freeze record".into()));
        }
        let mut reader = Reader::new(&bytes[..FREEZE_BYTES - 32]);
        let decoded = (|| {
            if reader.u16("freeze_version")? != 1 {
                return Err(erebus_core::encoding::EncodingError::UnknownTag(
                    "freeze_version",
                    0,
                ));
            }
            let value = Self {
                hash_version: reader.u16("hash_version")?,
                deal_id: reader.fixed("deal_id")?,
                root: reader.fixed("root")?,
                proposal_digest: reader.fixed("proposal_digest")?,
            };
            reader.finish()?;
            Ok(value)
        })();
        decoded.map_err(|_| StoreError::Corrupt("invalid freeze record".into()))
    }
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
        hash_version: u16,
        message: &Message,
    ) -> Result<Transcript, StoreError>;

    /// Loads and recomputes the transcript for a deal.
    fn load(
        &self,
        namespace: &str,
        deal_id: [u8; 16],
        hash_version: u16,
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

    fn read_freeze(directory: &Path) -> Result<Option<FrozenTranscript>, StoreError> {
        let path = directory.join("negotiation.freeze");
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error(error)),
        };
        if !metadata.is_file() || metadata.len() != FREEZE_BYTES as u64 {
            return Err(StoreError::Corrupt("invalid freeze file".into()));
        }
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(io_error)?
            .take(FREEZE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        FrozenTranscript::decode(&bytes).map(Some)
    }

    fn validate_freeze(
        directory: &Path,
        transcript: &Transcript,
    ) -> Result<Option<FrozenTranscript>, StoreError> {
        let frozen = Self::read_freeze(directory)?;
        if let Some(record) = &frozen {
            if record.deal_id != transcript.deal_id()
                || record.hash_version != transcript.hash_version()
                || record.root != transcript.root()?
                || transcript.is_empty()
            {
                return Err(StoreError::Corrupt("frozen transcript mismatch".into()));
            }
        }
        Ok(frozen)
    }

    fn write_freeze(directory: &Path, record: &FrozenTranscript) -> Result<(), StoreError> {
        let temporary = directory.join(format!(
            "freeze-{}.tmp",
            hex::encode(rand::random::<[u8; 16]>())
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let result = (|| {
            let mut handle = options.open(&temporary).map_err(io_error)?;
            handle.write_all(&record.encode()).map_err(io_error)?;
            handle.sync_all().map_err(io_error)?;
            fs::rename(&temporary, directory.join("negotiation.freeze")).map_err(io_error)?;
            sync_directory(directory)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    /// Seals exactly the expected nonempty prefix under the same lock used by append.
    ///
    /// Repeating the same freeze is idempotent. Changing the root or selected proposal fails.
    /// A caller must establish bilateral acceptance before calling this low-level operation.
    pub fn freeze(
        &self,
        namespace: &str,
        deal_id: [u8; 16],
        hash_version: u16,
        expected_root: [u8; 32],
        proposal_digest: [u8; 32],
    ) -> Result<FrozenTranscript, StoreError> {
        let directory = self.deal_directory(namespace, deal_id)?;
        create_private_directory(&directory)?;
        let lock = open_lock(&directory)?;
        lock.lock_exclusive().map_err(io_error)?;
        let stored = self.recover_messages(&directory)?;
        let transcript = Transcript::replay(deal_id, hash_version, &stored)?;
        let record = FrozenTranscript {
            hash_version,
            deal_id,
            root: expected_root,
            proposal_digest,
        };
        if transcript.is_empty() || transcript.root()? != expected_root {
            return Err(StoreError::Transition);
        }
        if let Some(existing) = Self::validate_freeze(&directory, &transcript)? {
            return if existing == record {
                Ok(existing)
            } else {
                Err(StoreError::Transition)
            };
        }
        Self::write_freeze(&directory, &record)?;
        Ok(record)
    }

    /// Loads a frozen prefix and verifies that its log has not changed.
    pub fn frozen(
        &self,
        namespace: &str,
        deal_id: [u8; 16],
        hash_version: u16,
    ) -> Result<Option<FrozenTranscript>, StoreError> {
        let directory = self.deal_directory(namespace, deal_id)?;
        if !directory.exists() {
            return Ok(None);
        }
        let lock = open_lock(&directory)?;
        lock.lock_exclusive().map_err(io_error)?;
        let stored = self.recover_messages(&directory)?;
        let transcript = Transcript::replay(deal_id, hash_version, &stored)?;
        Self::validate_freeze(&directory, &transcript)
    }

    pub(crate) fn append_checked(
        &self,
        namespace: &str,
        deal_id: [u8; 16],
        hash_version: u16,
        message: &Message,
        allow_replay: bool,
        check: impl FnOnce(&[Message]) -> Result<Option<[u8; 32]>, StoreError>,
    ) -> Result<Transcript, StoreError> {
        let directory = self.deal_directory(namespace, deal_id)?;
        create_private_directory(&directory)?;
        let lock = open_lock(&directory)?;
        lock.lock_exclusive().map_err(io_error)?;
        let stored = self.recover_messages(&directory)?;
        let mut transcript = Transcript::replay(deal_id, hash_version, &stored)?;
        let frozen = Self::validate_freeze(&directory, &transcript)?;
        let replay = allow_replay
            && stored
                .iter()
                .any(|previous| previous.encode_body() == message.encode_body());
        if frozen.is_some() && !replay {
            return Err(StoreError::Frozen);
        }
        let proposal = check(&stored)?;
        if replay {
            if let Some(proposal_digest) = proposal {
                let record = FrozenTranscript {
                    hash_version,
                    deal_id,
                    root: transcript.root()?,
                    proposal_digest,
                };
                if let Some(existing) = frozen {
                    if existing != record {
                        return Err(StoreError::Transition);
                    }
                } else {
                    Self::write_freeze(&directory, &record)?;
                }
            }
            return Ok(transcript);
        }
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
        if let Some(proposal_digest) = proposal {
            Self::write_freeze(
                &directory,
                &FrozenTranscript {
                    hash_version,
                    deal_id,
                    root: transcript.root()?,
                    proposal_digest,
                },
            )?;
        }
        Ok(transcript)
    }
}

impl TranscriptStore for FileTranscriptStore {
    fn append(
        &self,
        namespace: &str,
        deal_id: [u8; 16],
        hash_version: u16,
        message: &Message,
    ) -> Result<Transcript, StoreError> {
        self.append_checked(namespace, deal_id, hash_version, message, false, |_| {
            Ok(None)
        })
    }

    fn load(
        &self,
        namespace: &str,
        deal_id: [u8; 16],
        hash_version: u16,
    ) -> Result<Transcript, StoreError> {
        let directory = self.deal_directory(namespace, deal_id)?;
        if !directory.exists() {
            return Transcript::new(deal_id, hash_version).map_err(StoreError::Transcript);
        }
        let lock = open_lock(&directory)?;
        lock.lock_exclusive().map_err(io_error)?;
        let stored = self.recover_messages(&directory)?;
        let transcript = Transcript::replay(deal_id, hash_version, &stored)?;
        Self::validate_freeze(&directory, &transcript)?;
        Ok(transcript)
    }

    fn messages(&self, namespace: &str, deal_id: [u8; 16]) -> Result<Vec<Message>, StoreError> {
        let directory = self.deal_directory(namespace, deal_id)?;
        if !directory.exists() {
            return Ok(Vec::new());
        }
        let lock = open_lock(&directory)?;
        lock.lock_exclusive().map_err(io_error)?;
        let messages = self.recover_messages(&directory)?;
        if let Some(frozen) = Self::read_freeze(&directory)? {
            let transcript = Transcript::replay(deal_id, frozen.hash_version, &messages)?;
            Self::validate_freeze(&directory, &transcript)?;
        }
        Ok(messages)
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
