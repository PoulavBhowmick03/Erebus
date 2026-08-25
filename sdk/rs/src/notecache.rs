//! Durable cache of the immutable prefix of a subchannel's notes.
//!
//! Reading a channel walked every note from index zero on every call, one RPC each. A channel
//! with twenty notes cost twenty-one round trips to answer a question whose first twenty
//! answers cannot have changed.
//!
//! They cannot have changed because the pool writes notes through `WriteOnce`: a slot that
//! holds a value holds that value forever, and `INDEX_NOT_SEQUENTIAL` means the occupied
//! slots are a contiguous prefix. So the prefix below the first empty index is immutable, and
//! caching it is sound rather than a bet on how often things change.
//!
//! ## Why this cache cannot serve a wrong answer
//!
//! A note's location is `h(NOTE_ID_TAG, channel_key, token, index)`. The cache file is named
//! by a hash over the same channel key and token, so a cache written under one subchannel is
//! not *found* under another — a wrong key misses rather than matching the wrong entry. That
//! matters more here than performance: every failure in this protocol is silent, and a cache
//! that could return another channel's note would be the worst possible place to introduce
//! one.
//!
//! ## What is never cached
//!
//! The empty slot that ends a walk. A zero read means "nothing here **yet**", and the
//! counterparty can write there at any time. Caching absence would freeze a channel at the
//! length it had when it was first read, which is a stalled negotiation that looks healthy.
//! Only non-zero, token-checked notes are stored.
//!
//! A stale or unreadable cache is discarded rather than repaired: the chain is authoritative
//! and re-reading is merely slow, so there is never a reason to trust a file that does not
//! parse.

use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use starknet_types_core::felt::Felt;

use crate::hashes;

/// Cache schema. A file from a different version is discarded, not migrated: it is a cache.
const CACHE_VERSION: u32 = 1;
const CACHE_DIR: &str = "notecache";

/// One subchannel's immutable note prefix, in index order.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedPrefix {
    version: u32,
    /// Position `i` is note `i` as the pool returns it: `(packed_value, token)`. Contiguous
    /// by `INDEX_NOT_SEQUENTIAL`, so the vector's length is also the prefix length.
    ///
    /// Both felts are stored, not just the packed value. The second is zero for an encrypted
    /// note and the real token for an open one, and `check_note_token` distinguishes the two
    /// — so caching only the first would make a cached open note fail validation on every
    /// read after the first.
    notes: Vec<(Felt, Felt)>,
}

/// Durable per-subchannel note cache under the identity state directory.
///
/// Failures are swallowed on purpose. A cache that cannot be read or written must not break a
/// read that would otherwise succeed, so every method degrades to "no cache" rather than
/// returning an error.
#[derive(Debug, Clone)]
pub struct NoteCache {
    root: PathBuf,
}

impl NoteCache {
    /// Opens, or creates, the cache directory beside the state directory.
    ///
    /// Mode `0700` like the state and journal directories: note values are encrypted, but
    /// which notes exist and how many is exactly the traffic metadata the threat model tries
    /// not to leak, and there is no reason to hand it to another local user.
    pub fn new(state_dir: impl AsRef<Path>) -> Self {
        let root = state_dir.as_ref().join(CACHE_DIR);
        let _ = std::fs::create_dir_all(&root);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700));
        }
        Self { root }
    }

    /// The cached prefix for one subchannel, oldest note first. Empty when there is none.
    pub fn load(&self, channel_key: Felt, token: Felt) -> Vec<(Felt, Felt)> {
        let path = self.path(channel_key, token);
        let Ok(file) = File::open(&path) else {
            return Vec::new();
        };
        match serde_json::from_reader::<_, CachedPrefix>(BufReader::new(file)) {
            Ok(cached) if cached.version == CACHE_VERSION => cached.notes,
            // Unparseable or from another schema: the chain is authoritative and re-reading
            // is only slow, so there is no reason to trust this file.
            _ => Vec::new(),
        }
    }

    /// Replaces the cached prefix. `notes` must be the confirmed, contiguous prefix.
    ///
    /// Never call this with a trailing empty slot: see the module note on caching absence.
    pub fn store(&self, channel_key: Felt, token: Felt, notes: &[(Felt, Felt)]) {
        if notes.is_empty() {
            return;
        }
        let payload = CachedPrefix {
            version: CACHE_VERSION,
            notes: notes.to_vec(),
        };
        let Ok(encoded) = serde_json::to_vec(&payload) else {
            return;
        };

        // Atomic replace, matching state and journal: a torn cache file read by a concurrent
        // process would be discarded rather than believed, but writing one is still wrong.
        let temporary = self.root.join(format!(
            ".{}.{:016x}.tmp",
            std::process::id(),
            OsRng.next_u64()
        ));
        let written = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .and_then(|file| {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
                }
                let mut writer = BufWriter::new(file);
                writer.write_all(&encoded)?;
                writer.flush()?;
                Ok(())
            });
        if written.is_err() {
            let _ = std::fs::remove_file(&temporary);
            return;
        }
        if std::fs::rename(&temporary, self.path(channel_key, token)).is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
    }

    /// File name for a subchannel.
    ///
    /// Derived from the same channel key and token that derive a note id, so a cache written
    /// under one subchannel cannot be found under another. The name is a hash rather than the
    /// key itself: a channel key in a directory listing is a readable channel.
    fn path(&self, channel_key: Felt, token: Felt) -> PathBuf {
        let tag = Felt::from_bytes_be_slice(b"EREBUS_NOTE_CACHE_V1");
        let name = hashes::hash(&[tag, channel_key, token]);
        self.root.join(format!("{name:#x}.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "erebus-notecache-{}-{}",
            std::process::id(),
            OsRng.next_u64()
        ));
        std::fs::create_dir_all(&root).expect("root");
        root
    }

    #[test]
    fn a_stored_prefix_comes_back_in_order() {
        let root = root();
        let cache = NoteCache::new(&root);
        let notes = vec![
            (Felt::from(7u8), Felt::ZERO),
            (Felt::from(8u8), Felt::ZERO),
            (Felt::from(9u8), Felt::ZERO),
        ];

        cache.store(Felt::from(1u8), Felt::from(2u8), &notes);

        assert_eq!(cache.load(Felt::from(1u8), Felt::from(2u8)), notes);
        std::fs::remove_dir_all(&root).ok();
    }

    /// The property that makes this cache safe: a wrong key misses rather than matching.
    #[test]
    fn another_subchannel_never_reads_this_ones_notes() {
        let root = root();
        let cache = NoteCache::new(&root);
        let notes = vec![(Felt::from(7u8), Felt::ZERO)];
        cache.store(Felt::from(1u8), Felt::from(2u8), &notes);

        assert!(cache.load(Felt::from(99u8), Felt::from(2u8)).is_empty());
        assert!(cache.load(Felt::from(1u8), Felt::from(99u8)).is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    /// A cache is a cache. An unreadable one is discarded, never repaired or trusted.
    #[test]
    fn a_corrupt_cache_file_reads_as_no_cache() {
        let root = root();
        let cache = NoteCache::new(&root);
        cache.store(
            Felt::from(1u8),
            Felt::from(2u8),
            &[(Felt::from(7u8), Felt::ZERO)],
        );

        let path = cache.path(Felt::from(1u8), Felt::from(2u8));
        std::fs::write(&path, b"{ this is not json").expect("corrupt it");

        assert!(cache.load(Felt::from(1u8), Felt::from(2u8)).is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    /// A file from a future schema is discarded rather than misread.
    #[test]
    fn a_cache_from_another_schema_is_discarded() {
        let root = root();
        let cache = NoteCache::new(&root);
        let path = cache.path(Felt::from(1u8), Felt::from(2u8));
        std::fs::write(
            &path,
            serde_json::to_vec(&CachedPrefix {
                version: CACHE_VERSION + 1,
                notes: vec![(Felt::from(7u8), Felt::ZERO)],
            })
            .expect("encode"),
        )
        .expect("write");

        assert!(cache.load(Felt::from(1u8), Felt::from(2u8)).is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    /// Storing nothing must not create a file that later reads as an empty prefix.
    #[test]
    fn an_empty_prefix_is_not_written() {
        let root = root();
        let cache = NoteCache::new(&root);

        cache.store(Felt::from(1u8), Felt::from(2u8), &[]);

        assert!(!cache.path(Felt::from(1u8), Felt::from(2u8)).exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn cache_files_are_not_readable_by_anyone_else() {
        use std::os::unix::fs::PermissionsExt;

        let root = root();
        let cache = NoteCache::new(&root);
        cache.store(
            Felt::from(1u8),
            Felt::from(2u8),
            &[(Felt::from(7u8), Felt::ZERO)],
        );

        let path = cache.path(Felt::from(1u8), Felt::from(2u8));
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();

        assert_eq!(mode & 0o077, 0, "cache file is group or world readable");
        std::fs::remove_dir_all(&root).ok();
    }
}
