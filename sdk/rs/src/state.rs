//! Rust-owned persistent state for the one-shot CLI.
//!
//! A [`ChannelHandle`] is a random identifier, not a channel key in disguise. The channel
//! keys and note cursor remain in a file readable only by the local operator. This is what
//! lets Python retain a harmless handle across CLI invocations without ever receiving the
//! locator/decryption secret.
//!
//! State files are protected from other OS users (directory mode `0700`, file mode `0600`)
//! and updates use a locked, atomic replace. They are not encrypted from the operator who
//! runs the process; the local OS account is the MVP trust boundary.

use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use starknet_types_core::felt::Felt;

/// Current on-disk record version.
const STATE_VERSION: u32 = 1;

/// Opaque identifier exposed across the Python ↔ Rust seam.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ChannelHandle(String);

impl ChannelHandle {
    /// Parses and validates a handle.
    pub fn parse(value: impl Into<String>) -> Result<Self, StateError> {
        let value = value.into();
        let valid = value.len() == 67
            && value.starts_with("ch_")
            && value[3..].bytes().all(|byte| byte.is_ascii_hexdigit());
        if !valid {
            return Err(StateError::InvalidHandle(value));
        }
        Ok(Self(value))
    }

    /// String form passed through transport layers.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn random() -> Self {
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        let mut value = String::with_capacity(67);
        value.push_str("ch_");
        for byte in bytes {
            use core::fmt::Write as _;
            write!(&mut value, "{byte:02x}").expect("writing to String cannot fail");
        }
        Self(value)
    }
}

impl core::fmt::Display for ChannelHandle {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Secret-bearing state for one bilateral channel.
#[derive(Clone, Serialize, Deserialize)]
pub struct StoredChannel {
    version: u32,
    /// Public handle, repeated inside the record to detect a misplaced file.
    pub handle: ChannelHandle,
    /// Local pool identity address.
    pub owner: Felt,
    /// Counterparty address.
    pub counterparty_address: Felt,
    /// Counterparty registered pool public key.
    pub counterparty_public_key: Felt,
    /// Token subchannel.
    pub token: Felt,
    /// Local → counterparty locator/decryption key.
    pub outgoing_key: Felt,
    /// Counterparty → local key, populated after reverse-channel discovery.
    pub incoming_key: Option<Felt>,
    /// Next free note index in the outgoing token subchannel.
    pub outgoing_next_note: u32,
    /// Channel position used at setup.
    pub channel_index: u32,
    /// Token-subchannel position used at setup.
    pub subchannel_index: u32,
    /// Setup transaction.
    pub opened_transaction: Felt,
    /// Most recent accepted local write, used to wait for the historical proof anchor.
    pub last_write_block: u64,
    /// Once settled, this channel is terminal in the MVP.
    pub settled: bool,
}

impl StoredChannel {
    /// Creates a versioned record. The store supplies the opaque handle.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        handle: ChannelHandle,
        owner: Felt,
        counterparty_address: Felt,
        counterparty_public_key: Felt,
        token: Felt,
        outgoing_key: Felt,
        channel_index: u32,
        subchannel_index: u32,
        opened_transaction: Felt,
        opened_block: u64,
    ) -> Self {
        Self {
            version: STATE_VERSION,
            handle,
            owner,
            counterparty_address,
            counterparty_public_key,
            token,
            outgoing_key,
            incoming_key: None,
            outgoing_next_note: 0,
            channel_index,
            subchannel_index,
            opened_transaction,
            last_write_block: opened_block,
            settled: false,
        }
    }
}

/// Never render channel keys in logs.
impl core::fmt::Debug for StoredChannel {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("StoredChannel")
            .field("version", &self.version)
            .field("handle", &self.handle)
            .field("owner", &self.owner)
            .field("counterparty_address", &self.counterparty_address)
            .field("counterparty_public_key", &self.counterparty_public_key)
            .field("token", &self.token)
            .field("outgoing_key", &"<redacted>")
            .field("incoming_key", &self.incoming_key.map(|_| "<redacted>"))
            .field("outgoing_next_note", &self.outgoing_next_note)
            .field("channel_index", &self.channel_index)
            .field("subchannel_index", &self.subchannel_index)
            .field("opened_transaction", &self.opened_transaction)
            .field("last_write_block", &self.last_write_block)
            .field("settled", &self.settled)
            .finish()
    }
}

/// Filesystem-backed channel state.
#[derive(Debug, Clone)]
pub struct StateStore {
    root: PathBuf,
}

impl StateStore {
    /// Opens or creates a state directory.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, StateError> {
        let root = root.into();
        std::fs::create_dir_all(&root).map_err(|source| StateError::Io {
            path: root.clone(),
            source,
        })?;
        set_mode(&root, 0o700)?;
        Ok(Self { root })
    }

    /// Allocates an opaque handle and atomically persists a new channel.
    pub fn create(
        &self,
        build: impl FnOnce(ChannelHandle) -> StoredChannel,
    ) -> Result<ChannelHandle, StateError> {
        // Collision probability is negligible, but create-new semantics make the guarantee
        // structural rather than probabilistic.
        for _ in 0..16 {
            let handle = ChannelHandle::random();
            let lock_path = self.lock_path(&handle);
            let lock = match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(source) => {
                    return Err(StateError::Io {
                        path: lock_path,
                        source,
                    })
                }
            };
            set_file_mode(&lock, &self.lock_path(&handle), 0o600)?;
            lock.lock_exclusive().map_err(|source| StateError::Io {
                path: self.lock_path(&handle),
                source,
            })?;

            let state = build(handle.clone());
            self.write_atomic(&state)?;
            return Ok(handle);
        }
        Err(StateError::HandleCollision)
    }

    /// Locks and loads a channel. Keep the returned lease alive through any async operation
    /// that uses or advances its cursor.
    pub fn lock(&self, handle: &ChannelHandle) -> Result<ChannelLease, StateError> {
        let lock_path = self.lock_path(handle);
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|source| StateError::Io {
                path: lock_path.clone(),
                source,
            })?;
        set_file_mode(&lock, &lock_path, 0o600)?;
        lock.lock_exclusive().map_err(|source| StateError::Io {
            path: lock_path,
            source,
        })?;

        let state_path = self.state_path(handle);
        let file = File::open(&state_path).map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                StateError::NotFound(handle.clone())
            } else {
                StateError::Io {
                    path: state_path.clone(),
                    source,
                }
            }
        })?;
        let state: StoredChannel =
            serde_json::from_reader(BufReader::new(file)).map_err(|source| StateError::Json {
                path: state_path.clone(),
                source,
            })?;
        if state.version != STATE_VERSION {
            return Err(StateError::UnsupportedVersion(state.version));
        }
        if state.handle != *handle {
            return Err(StateError::HandleMismatch {
                expected: handle.clone(),
                found: state.handle,
            });
        }

        Ok(ChannelLease {
            store: self.clone(),
            _lock: lock,
            state,
        })
    }

    /// Finds an existing channel to `counterparty` for `token`, if this identity has one.
    ///
    /// **There can only ever be one.** The pool's `compute_channel_key` takes no index
    /// (`hashes.cairo:119-124`) and the marker derived from it is written `WriteOnce`, so a
    /// second `open_channel` between the same pair reverts with a bare `Contract error`
    /// after the proof has already been generated and the gas already spent. Callers use
    /// this to make opening idempotent rather than to discover that the hard way. F29.
    pub fn find_channel(
        &self,
        owner: Felt,
        counterparty: Felt,
        token: Felt,
    ) -> Result<Option<ChannelHandle>, StateError> {
        for state in self.records()? {
            if state.owner == owner
                && state.counterparty_address == counterparty
                && state.token == token
            {
                return Ok(Some(state.handle));
            }
        }
        Ok(None)
    }

    fn records(&self) -> Result<Vec<StoredChannel>, StateError> {
        let mut records = Vec::new();
        let entries = std::fs::read_dir(&self.root).map_err(|source| StateError::Io {
            path: self.root.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| StateError::Io {
                path: self.root.clone(),
                source,
            })?;
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            let file = File::open(&path).map_err(|source| StateError::Io {
                path: path.clone(),
                source,
            })?;
            records.push(
                serde_json::from_reader(BufReader::new(file)).map_err(|source| {
                    StateError::Json {
                        path: path.clone(),
                        source,
                    }
                })?,
            );
        }
        Ok(records)
    }

    /// Incoming channel keys already paired with local handles.
    ///
    /// Reverse channels have no on-chain pair identifier. Excluding keys already claimed
    /// by other records makes a newly opened reverse channel unambiguous without exposing
    /// any key outside Rust.
    pub fn claimed_incoming_keys(&self) -> Result<Vec<Felt>, StateError> {
        let mut claimed = Vec::new();
        let entries = std::fs::read_dir(&self.root).map_err(|source| StateError::Io {
            path: self.root.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| StateError::Io {
                path: self.root.clone(),
                source,
            })?;
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            let file = File::open(&path).map_err(|source| StateError::Io {
                path: path.clone(),
                source,
            })?;
            let state: StoredChannel =
                serde_json::from_reader(BufReader::new(file)).map_err(|source| {
                    StateError::Json {
                        path: path.clone(),
                        source,
                    }
                })?;
            if let Some(key) = state.incoming_key {
                claimed.push(key);
            }
        }
        Ok(claimed)
    }

    fn write_atomic(&self, state: &StoredChannel) -> Result<(), StateError> {
        let path = self.state_path(&state.handle);
        let temporary = self.root.join(format!(
            ".{}.{}.{:016x}.tmp",
            state.handle.as_str(),
            std::process::id(),
            OsRng.next_u64(),
        ));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|source| StateError::Io {
                path: temporary.clone(),
                source,
            })?;
        set_file_mode(&file, &temporary, 0o600)?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer(&mut writer, state).map_err(|source| StateError::Json {
            path: temporary.clone(),
            source,
        })?;
        writer.flush().map_err(|source| StateError::Io {
            path: temporary.clone(),
            source,
        })?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|source| StateError::Io {
                path: temporary.clone(),
                source,
            })?;
        std::fs::rename(&temporary, &path).map_err(|source| StateError::Io { path, source })
    }

    fn state_path(&self, handle: &ChannelHandle) -> PathBuf {
        self.root.join(format!("{}.json", handle.as_str()))
    }

    fn lock_path(&self, handle: &ChannelHandle) -> PathBuf {
        self.root.join(format!("{}.lock", handle.as_str()))
    }
}

/// Exclusively locked state record.
pub struct ChannelLease {
    store: StateStore,
    _lock: File,
    state: StoredChannel,
}

impl ChannelLease {
    /// Current record.
    pub fn state(&self) -> &StoredChannel {
        &self.state
    }

    /// Mutable record. Call [`ChannelLease::commit`] after a successful on-chain change.
    pub fn state_mut(&mut self) -> &mut StoredChannel {
        &mut self.state
    }

    /// Atomically saves the updated record and releases the lock.
    pub fn commit(self) -> Result<(), StateError> {
        self.store.write_atomic(&self.state)
    }
}

impl core::fmt::Debug for ChannelLease {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ChannelLease")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

/// Persistent-state failure.
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    /// Handle is malformed and therefore never used as a path component.
    #[error("invalid channel handle: {0}")]
    InvalidHandle(String),
    /// No state exists for the handle.
    #[error("unknown channel handle: {0}")]
    NotFound(ChannelHandle),
    /// Record version is newer or older than this SDK understands.
    #[error("unsupported channel-state version {0}")]
    UnsupportedVersion(u32),
    /// Record contents do not match their filename.
    #[error("channel-state handle mismatch: expected {expected}, found {found}")]
    HandleMismatch {
        /// Requested handle.
        expected: ChannelHandle,
        /// Handle inside the file.
        found: ChannelHandle,
    },
    /// Sixteen cryptographically random handles collided with existing files.
    #[error("could not allocate a unique channel handle")]
    HandleCollision,
    /// Filesystem failure.
    #[error("state I/O at {}: {source}", path.display())]
    Io {
        /// Affected path.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
    /// Malformed on-disk JSON.
    #[error("invalid state JSON at {}: {source}", path.display())]
    Json {
        /// Affected path.
        path: PathBuf,
        /// Underlying error.
        source: serde_json::Error,
    },
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), StateError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(|source| {
        StateError::Io {
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), StateError> {
    Ok(())
}

#[cfg(unix)]
fn set_file_mode(file: &File, path: &Path, mode: u32) -> Result<(), StateError> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(std::fs::Permissions::from_mode(mode))
        .map_err(|source| StateError::Io {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(not(unix))]
fn set_file_mode(_file: &File, _path: &Path, _mode: u32) -> Result<(), StateError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_store() -> (PathBuf, StateStore) {
        let root = std::env::temp_dir().join(format!(
            "erebus-state-test-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let store = StateStore::new(&root).expect("store");
        (root, store)
    }

    #[test]
    fn handle_is_opaque_and_keys_round_trip_only_through_the_store() {
        let (root, store) = temporary_store();
        let handle = store
            .create(|handle| {
                StoredChannel::new(
                    handle,
                    Felt::from(1u8),
                    Felt::from(2u8),
                    Felt::from(3u8),
                    Felt::from(4u8),
                    Felt::from(5u8),
                    0,
                    0,
                    Felt::from(6u8),
                    7,
                )
            })
            .expect("create");

        assert!(handle.as_str().starts_with("ch_"));
        assert!(!handle.as_str().contains("0x5"));
        let mut lease = store.lock(&handle).expect("lock");
        assert_eq!(lease.state().outgoing_key, Felt::from(5u8));
        assert_eq!(lease.state().last_write_block, 7);
        lease.state_mut().outgoing_next_note = 4;
        lease.commit().expect("commit");
        assert_eq!(
            store
                .lock(&handle)
                .expect("relock")
                .state()
                .outgoing_next_note,
            4
        );

        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn malformed_handle_cannot_become_a_path() {
        assert!(ChannelHandle::parse("../../pool-key").is_err());
        assert!(ChannelHandle::parse("ch_1234").is_err());
    }
}
