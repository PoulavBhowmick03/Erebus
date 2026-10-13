//! Relayer shared-account semantics: one exclusive execution lock per sponsor root.
//!
//! `SponsoredRelay` maps an unavailable execution lock to `SponsorError::Busy` (HTTP 503,
//! "relay busy; retain the operation"). This pins the underlying exclusion mechanism that
//! `RelayJournal::try_execution_lock` uses, including release on drop and per-root isolation.
//! Quota persistence and the one-signer/one-initial-send journal behavior are covered by the
//! in-crate unit suites (`relay_http`, `relay_journal`); they are run and recorded in the
//! handoff because their stores are crate-private.

use core::fmt;
use std::path::Path;

use erebus_journal::{JournalRecord, RecordId, Store};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Probe {
    id: String,
    version: u32,
}

#[derive(Clone, Debug, PartialEq)]
struct ProbeId(String);

impl fmt::Display for ProbeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl RecordId for ProbeId {
    fn as_file_stem(&self) -> &str {
        &self.0
    }

    fn from_file_stem(stem: &str) -> Option<Self> {
        (!stem.is_empty()).then(|| Self(stem.to_owned()))
    }
}

impl JournalRecord for Probe {
    type Id = ProbeId;

    const CURRENT_VERSION: u32 = 1;
    const OLDEST_READABLE_VERSION: u32 = 1;

    fn version(&self) -> u32 {
        self.version
    }

    fn record_id(&self) -> &Self::Id {
        // The engine only needs the filed id for its own checks; the probe never writes.
        static ID: std::sync::OnceLock<ProbeId> = std::sync::OnceLock::new();
        ID.get_or_init(|| ProbeId("probe".to_owned()))
    }

    fn attempt_count(&self) -> usize {
        1
    }
}

fn lock(root: &Path) -> Option<erebus_journal::IdentityLock> {
    Store::<Probe>::open(root.to_path_buf())
        .expect("open probe store")
        .try_lock_identity()
        .expect("lock probe store")
}

#[test]
fn execution_lock_is_exclusive_per_sponsor_root_and_releases_on_drop() {
    let root = tempfile::tempdir().unwrap().path().to_path_buf();
    let other = tempfile::tempdir().unwrap().path().to_path_buf();

    let held = lock(&root).expect("first holder acquires the execution lock");
    assert!(
        lock(&root).is_none(),
        "a second holder on the same sponsor root must see contention (mapped to Busy/503)"
    );
    assert!(
        lock(&other).is_some(),
        "a different sponsor root must not contend"
    );
    drop(held);
    assert!(
        lock(&root).is_some(),
        "the execution lock must be released when the holder drops"
    );
}
