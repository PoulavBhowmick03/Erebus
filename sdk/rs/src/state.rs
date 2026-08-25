//! Rust-owned persistent state for the one-shot CLI.
//!
//! A [`ChannelHandle`] is a random identifier. Channel keys and the note cursor remain in a
//! local operator file. Python retains the handle across CLI calls without receiving a
//! locator or decryption key.
//!
//! State files are protected from other OS users (directory mode `0700`, file mode `0600`)
//! and updates use a locked atomic replacement. The files are not encrypted from the local
//! operator. The local OS account is the MVP trust boundary.

use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use starknet_types_core::felt::Felt;

use crate::wire::WireVersion;

/// Current on-disk record version.
const STATE_VERSION: u32 = 1;

/// Opaque identifier exposed across the Python-to-Rust seam.
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
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredChannel {
    version: u32,
    /// Negotiation wire generation. Missing means legacy v1 when loading old records.
    #[serde(default)]
    pub wire_version: WireVersion,
    /// Starknet chain bound into wire-v2 authentication. Zero for migrated v1 records.
    #[serde(default)]
    pub chain_id: Felt,
    /// Privacy pool bound into wire-v2 authentication. Zero for migrated v1 records.
    #[serde(default)]
    pub pool_address: Felt,
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
    /// Local-to-counterparty locator and decryption key.
    pub outgoing_key: Felt,
    /// Counterparty-to-local key set after reverse-channel discovery.
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
    /// Whether this channel has a settlement. Only historical v1/v2 channels are terminal.
    pub settled: bool,
}

impl StoredChannel {
    /// Creates a versioned record. The store supplies the opaque handle.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        handle: ChannelHandle,
        chain_id: Felt,
        pool_address: Felt,
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
        Self::new_with_wire_version(
            handle,
            chain_id,
            pool_address,
            owner,
            counterparty_address,
            counterparty_public_key,
            token,
            outgoing_key,
            channel_index,
            subchannel_index,
            opened_transaction,
            opened_block,
            WireVersion::V3,
        )
    }

    /// Creates a record for a channel opened with an explicit wire generation.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_wire_version(
        handle: ChannelHandle,
        chain_id: Felt,
        pool_address: Felt,
        owner: Felt,
        counterparty_address: Felt,
        counterparty_public_key: Felt,
        token: Felt,
        outgoing_key: Felt,
        channel_index: u32,
        subchannel_index: u32,
        opened_transaction: Felt,
        opened_block: u64,
        wire_version: WireVersion,
    ) -> Self {
        Self {
            version: STATE_VERSION,
            wire_version,
            chain_id,
            pool_address,
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
            .field("wire_version", &self.wire_version)
            .field("chain_id", &self.chain_id)
            .field("pool_address", &self.pool_address)
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
        // create_new prevents overwrite if a random handle collides.
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

    /// Allocates an opaque handle without writing channel state yet.
    ///
    /// A crash-safe channel open must put the chosen handle in the operation journal before
    /// submission. The handle has 256 random bits, and the later recovered write still uses
    /// `create_new` semantics, so an intervening collision fails closed instead of
    /// overwriting another record.
    pub fn allocate_handle(&self) -> Result<ChannelHandle, StateError> {
        for _ in 0..16 {
            let handle = ChannelHandle::random();
            if !self.state_path(&handle).exists() && !self.lock_path(&handle).exists() {
                return Ok(handle);
            }
        }
        Err(StateError::HandleCollision)
    }

    /// Creates an exact journal-planned channel, or accepts an identical existing record.
    ///
    /// Recovery calls this after the chain accepted an open but before the original process
    /// durably saved its local state. Reapplying the same mutation is harmless; a different
    /// record under the planned handle is a contradiction and fails closed.
    pub fn create_recovered(&self, state: StoredChannel) -> Result<(), StateError> {
        let handle = state.handle.clone();
        let lock_path = self.lock_path(&handle);
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

        let path = self.state_path(&handle);
        if path.exists() {
            let existing = self.read_state(&handle)?;
            if existing == state {
                return Ok(());
            }
            return Err(StateError::RecoveryConflict {
                handle,
                reason: "the planned channel handle already contains different state".to_owned(),
            });
        }

        self.write_atomic(&state)
    }

    /// Reads one channel snapshot without leaving its lock held.
    pub fn snapshot(&self, handle: &ChannelHandle) -> Result<Option<StoredChannel>, StateError> {
        if !self.state_path(handle).exists() {
            return Ok(None);
        }
        let lease = self.lock(handle)?;
        Ok(Some(lease.state().clone()))
    }

    /// Locks and loads a channel. Keep the returned lease alive through any async operation
    /// that uses or advances its cursor.
    pub fn lock(&self, handle: &ChannelHandle) -> Result<ChannelLease, StateError> {
        // Checked before the lock file is created: opening the lock first left a
        // `<handle>.lock` behind for every unknown handle ever asked about, and a state
        // directory accumulating locks for channels that do not exist reads as corruption
        // during an incident. Racing a concurrent `create` is benign — the caller that
        // loses simply reports the handle as it found it, not found.
        let state_path = self.state_path(handle);
        if !state_path.exists() {
            return Err(StateError::NotFound(handle.clone()));
        }

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

        let state = self.read_state(handle)?;

        Ok(ChannelLease {
            store: self.clone(),
            _lock: lock,
            state,
        })
    }

    fn read_state(&self, handle: &ChannelHandle) -> Result<StoredChannel, StateError> {
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

        Ok(state)
    }

    /// Finds an existing channel to `counterparty` for `token`, if this identity has one.
    ///
    /// Only one can exist. `compute_channel_key` has no index (`hashes.cairo:119-124`), and
    /// its marker is `WriteOnce`. A second `open_channel` for the pair returns
    /// `Contract error` after proof generation and gas spending. This lookup makes channel
    /// opening idempotent. See F29.
    pub fn find_channel(
        &self,
        chain_id: Felt,
        pool_address: Felt,
        owner: Felt,
        counterparty: Felt,
        token: Felt,
    ) -> Result<Option<ChannelHandle>, StateError> {
        for state in self.records()? {
            if state.owner == owner
                && state.counterparty_address == counterparty
                && state.token == token
                && (state.wire_version == WireVersion::V1
                    || (state.chain_id == chain_id && state.pool_address == pool_address))
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

    /// Every channel record in the store, in unspecified order.
    ///
    /// A single unreadable record fails the whole listing, for the same reason the journal
    /// does it: a listing that silently skipped a record it could not parse would let a
    /// rebuild recreate a channel that already exists, and two records for one relationship
    /// is how a note index gets written twice.
    pub fn snapshots(&self) -> Result<Vec<StoredChannel>, StateError> {
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
            let state: StoredChannel =
                serde_json::from_reader(BufReader::new(file)).map_err(|source| {
                    StateError::Json {
                        path: path.clone(),
                        source,
                    }
                })?;
            records.push(state);
        }
        Ok(records)
    }

    /// Incoming channel keys already paired with local handles.
    ///
    /// Reverse channels have no on-chain pair identifier. Excluding claimed keys identifies
    /// a new reverse channel without exposing a key outside Rust.
    pub fn claimed_incoming_keys(&self) -> Result<Vec<Felt>, StateError> {
        Ok(self
            .snapshots()?
            .into_iter()
            .filter_map(|state| state.incoming_key)
            .collect())
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
        std::fs::rename(&temporary, &path).map_err(|source| StateError::Io { path, source })?;
        sync_dir(&self.root)
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
    /// A recovery mutation would overwrite state that does not match its durable plan.
    #[error("cannot recover channel {handle}: {reason}")]
    RecoveryConflict {
        /// Planned handle.
        handle: ChannelHandle,
        /// Contradiction found on disk.
        reason: String,
    },
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

#[cfg(unix)]
fn sync_dir(path: &Path) -> Result<(), StateError> {
    let directory = File::open(path).map_err(|source| StateError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    directory.sync_all().map_err(|source| StateError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
fn sync_dir(_path: &Path) -> Result<(), StateError> {
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

    /// Asking about a handle that does not exist must not leave anything behind. The lock
    /// file used to be created before the existence check, so every unknown handle ever
    /// queried left a permanent `<handle>.lock` in the state directory.
    #[test]
    fn an_unknown_handle_leaves_no_trace_in_the_state_directory() {
        let (root, store) = temporary_store();
        let absent = ChannelHandle::parse(format!("ch_{}", "ef".repeat(32))).expect("handle");
        assert!(matches!(store.lock(&absent), Err(StateError::NotFound(_))));
        let leftovers: Vec<_> = std::fs::read_dir(&root)
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .collect();
        assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
    }

    #[test]
    fn handle_is_opaque_and_keys_round_trip_only_through_the_store() {
        let (root, store) = temporary_store();
        let handle = store
            .create(|handle| {
                StoredChannel::new(
                    handle,
                    Felt::from(0xaau8),
                    Felt::from(0xbbu8),
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
    fn recovered_channel_creation_is_idempotent_and_never_overwrites() {
        let (_root, store) = temporary_store();
        let handle = store.allocate_handle().expect("handle");
        let planned = StoredChannel::new(
            handle.clone(),
            Felt::from(0xaau8),
            Felt::from(0xbbu8),
            Felt::from(1u8),
            Felt::from(2u8),
            Felt::from(3u8),
            Felt::from(4u8),
            Felt::from(5u8),
            0,
            0,
            Felt::from(6u8),
            7,
        );

        store
            .create_recovered(planned.clone())
            .expect("first recovery creates");
        store
            .create_recovered(planned.clone())
            .expect("same recovery is idempotent");
        assert_eq!(store.snapshot(&handle).expect("snapshot"), Some(planned));

        let conflicting = StoredChannel::new(
            handle,
            Felt::from(0xaau8),
            Felt::from(0xbbu8),
            Felt::from(1u8),
            Felt::from(99u8),
            Felt::from(3u8),
            Felt::from(4u8),
            Felt::from(5u8),
            0,
            0,
            Felt::from(6u8),
            7,
        );
        assert!(matches!(
            store.create_recovered(conflicting),
            Err(StateError::RecoveryConflict { .. })
        ));
    }

    #[test]
    fn malformed_handle_cannot_become_a_path() {
        assert!(ChannelHandle::parse("../../pool-key").is_err());
        assert!(ChannelHandle::parse("ch_1234").is_err());
    }

    #[test]
    fn a_record_without_wire_version_loads_as_legacy_v1() {
        let handle = ChannelHandle::parse(format!("ch_{}", "11".repeat(32))).expect("handle");
        let current = StoredChannel::new(
            handle,
            Felt::from(0xaau8),
            Felt::from(0xbbu8),
            Felt::from(1u8),
            Felt::from(2u8),
            Felt::from(3u8),
            Felt::from(4u8),
            Felt::from(5u8),
            0,
            0,
            Felt::from(6u8),
            7,
        );
        assert_eq!(current.wire_version, WireVersion::V3);

        let mut serialized = serde_json::to_value(current).expect("serialize");
        serialized
            .as_object_mut()
            .expect("record object")
            .remove("wire_version");
        serialized
            .as_object_mut()
            .expect("record object")
            .remove("chain_id");
        serialized
            .as_object_mut()
            .expect("record object")
            .remove("pool_address");
        let migrated: StoredChannel = serde_json::from_value(serialized).expect("legacy loads");

        assert_eq!(migrated.wire_version, WireVersion::V1);
        assert_eq!(migrated.chain_id, Felt::ZERO);
        assert_eq!(migrated.pool_address, Felt::ZERO);
    }
}
