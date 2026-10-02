//! The storage engine on its own, with a record type that has nothing to do with any chain.
//!
//! `sdk/rs` pins the engine's behaviour through the Starknet journal (its characterization
//! tests). These tests pin the mechanics directly: atomic writes, lock contention, the order
//! a stored transaction and its record reach disk, the fault hook at every durable boundary,
//! prune, and the fail-closed listing.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use erebus_journal::{
    Boundary, Corruption, FaultHook, JournalRecord, PruneDecision, PruneReport, RecordId, Step,
    Store, StoreError,
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

// ---- a chain-free record -------------------------------------------------------------------

/// `rec_` followed by four lowercase hex digits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Id(String);

impl Id {
    fn new(n: u16) -> Self {
        Self(format!("rec_{n:04x}"))
    }
}

impl core::fmt::Display for Id {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl RecordId for Id {
    fn as_file_stem(&self) -> &str {
        &self.0
    }

    fn from_file_stem(stem: &str) -> Option<Self> {
        let valid = stem.len() == 8
            && stem.starts_with("rec_")
            && stem[4..]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        valid.then(|| Self(stem.to_owned()))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Record {
    version: u32,
    id: Id,
    attempts: Vec<String>,
}

impl Record {
    fn new(n: u16, attempts: usize) -> Self {
        Self {
            version: 3,
            id: Id::new(n),
            attempts: (0..attempts).map(|a| format!("attempt-{a}")).collect(),
        }
    }
}

impl JournalRecord for Record {
    type Id = Id;

    const CURRENT_VERSION: u32 = 3;
    const OLDEST_READABLE_VERSION: u32 = 2;

    fn version(&self) -> u32 {
        self.version
    }

    fn record_id(&self) -> &Id {
        &self.id
    }

    fn attempt_count(&self) -> usize {
        self.attempts.len()
    }
}

// ---- helpers -------------------------------------------------------------------------------

/// A directory under the system temp dir, removed on drop. A counter, not a timestamp: tests
/// run in parallel.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        Self(std::env::temp_dir().join(format!(
            "erebus-journal-store-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }

    fn journal(&self) -> PathBuf {
        self.0.join("journal")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Records every boundary, and fails the one at `fail_at` (0-based) if set.
#[derive(Default)]
struct Recorder {
    seen: Mutex<Vec<(Step, PathBuf)>>,
    fail_at: Option<usize>,
}

impl Recorder {
    fn failing_at(index: usize) -> Self {
        Self {
            seen: Mutex::default(),
            fail_at: Some(index),
        }
    }

    fn seen(&self) -> Vec<(Step, PathBuf)> {
        self.seen.lock().expect("recorder").clone()
    }

    fn steps(&self) -> Vec<Step> {
        self.seen().into_iter().map(|(step, _)| step).collect()
    }
}

impl FaultHook for Recorder {
    fn after(&self, boundary: Boundary<'_>) -> std::io::Result<()> {
        let mut seen = self.seen.lock().expect("recorder");
        seen.push((boundary.step, boundary.path.to_path_buf()));
        if self.fail_at == Some(seen.len() - 1) {
            return Err(std::io::Error::other("injected fault"));
        }
        Ok(())
    }
}

fn open(dir: &TempDir) -> Store<Record> {
    Store::open(dir.journal()).expect("store opens")
}

fn open_with(dir: &TempDir, hook: &Arc<Recorder>) -> Store<Record> {
    let hook: Arc<dyn FaultHook> = Arc::clone(hook) as Arc<dyn FaultHook>;
    Store::open_with_fault_hook(dir.journal(), hook).expect("store opens")
}

fn names(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(root)
        .expect("list")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .into_string()
                .expect("utf8")
        })
        .collect();
    names.sort();
    names
}

fn is_temporary(path: &Path, root: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    path.parent() == Some(root) && name.starts_with('.') && name.ends_with(".tmp")
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).expect("stat").permissions().mode() & 0o777
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
}

#[cfg(unix)]
fn inode(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).expect("stat").ino()
}

/// Stores a blob for the latest attempt and writes the record, with no fault.
fn sign(store: &Store<Record>, record: &mut Record, blob: &str) {
    let attempt = record.attempts.len() - 1;
    store
        .write_blob_then_record(record, attempt, blob.as_bytes(), |record| {
            record.attempts[attempt].push_str("-signed");
            Ok::<(), StoreError<Id>>(())
        })
        .expect("sign");
}

// ---- directory and atomic write ------------------------------------------------------------

#[cfg(unix)]
#[test]
fn opening_creates_a_private_directory_and_retightens_it() {
    let dir = TempDir::new("open");
    let store = open(&dir);
    assert_eq!(store.root(), dir.journal());
    assert_eq!(mode(&dir.journal()), 0o700);

    set_mode(&dir.journal(), 0o755);
    open(&dir);
    assert_eq!(mode(&dir.journal()), 0o700);
}

#[test]
fn opening_over_a_file_is_an_io_error_at_that_path() {
    let dir = TempDir::new("open-file");
    std::fs::create_dir_all(&dir.0).expect("root");
    std::fs::write(dir.journal(), b"").expect("file");
    let error = Store::<Record>::open(dir.journal()).expect_err("io");
    let StoreError::Io { path, .. } = &error else {
        panic!("expected Io, got {error:?}");
    };
    assert_eq!(path, &dir.journal());
}

#[cfg(unix)]
#[test]
fn a_write_replaces_the_record_by_rename_and_leaves_no_temporary() {
    let dir = TempDir::new("write");
    let store = open(&dir);
    let mut record = Record::new(1, 1);
    store.write(&record).expect("write");

    let path = store.record_path(&record.id);
    assert_eq!(path, dir.journal().join("rec_0001.json"));
    assert_eq!(
        std::fs::read(&path).expect("record"),
        serde_json::to_vec(&record).expect("encode"),
        "a write is exactly the serde_json encoding, nothing more"
    );
    assert_eq!(mode(&path), 0o600);
    assert_eq!(names(&dir.journal()), ["rec_0001.json"]);

    // Loosen the mode: the next write is a new file, created 0600.
    set_mode(&path, 0o644);
    let before = inode(&path);
    record.attempts[0] = "changed".to_owned();
    store.write(&record).expect("rewrite");
    assert_ne!(
        inode(&path),
        before,
        "a rewrite is a rename, not an in-place edit"
    );
    assert_eq!(mode(&path), 0o600);
    assert_eq!(names(&dir.journal()), ["rec_0001.json"]);
    assert_eq!(store.read(&record.id).expect("read"), Some(record));
}

#[test]
fn reading_an_absent_record_is_none() {
    let dir = TempDir::new("absent");
    let store = open(&dir);
    assert!(!store.contains(&Id::new(9)));
    assert_eq!(store.read(&Id::new(9)).expect("read"), None);
    store.write(&Record::new(9, 1)).expect("write");
    assert!(store.contains(&Id::new(9)));
}

// ---- the fault hook ------------------------------------------------------------------------

#[test]
fn every_durable_boundary_reaches_the_hook_in_order() {
    let dir = TempDir::new("boundaries");
    let hook = Arc::new(Recorder::default());
    let store = open_with(&dir, &hook);
    let root = dir.journal();
    let mut record = Record::new(2, 1);
    let record_path = store.record_path(&record.id);

    store.write(&record).expect("write");
    let seen = hook.seen();
    assert_eq!(
        hook.steps(),
        [
            Step::FileSynced,
            Step::Renamed,
            Step::DirectorySynced,
            Step::RecordWritten
        ]
    );
    assert!(is_temporary(&seen[0].1, &root), "{:?}", seen[0].1);
    assert_eq!(seen[1].1, record_path);
    assert_eq!(seen[2].1, root);
    assert_eq!(seen[3].1, record_path);

    // A stored transaction: the blob's whole write, then the update, then the record's.
    let blob_path = store.blob_path(&record.id, 0);
    let boundaries_before_update = Mutex::new(None);
    store
        .write_blob_then_record(&mut record, 0, b"tx", |record| {
            *boundaries_before_update.lock().expect("lock") = Some(hook.seen().len());
            record.attempts[0].push_str("-signed");
            Ok::<(), StoreError<Id>>(())
        })
        .expect("sign");
    let seen = hook.seen()[4..].to_vec();
    let steps: Vec<Step> = seen.iter().map(|(step, _)| *step).collect();
    assert_eq!(
        steps,
        [
            Step::FileSynced,
            Step::Renamed,
            Step::DirectorySynced,
            Step::BlobWritten,
            Step::FileSynced,
            Step::Renamed,
            Step::DirectorySynced,
            Step::RecordWritten,
        ]
    );
    assert_eq!(
        *boundaries_before_update.lock().expect("lock"),
        Some(4 + 4),
        "the update runs only after the blob is durable"
    );
    assert!(is_temporary(&seen[0].1, &root));
    assert_eq!(seen[1].1, blob_path);
    assert_eq!(seen[3].1, blob_path);
    assert!(is_temporary(&seen[4].1, &root));
    assert_ne!(seen[0].1, seen[4].1, "every write has its own temporary");
    assert_eq!(seen[5].1, record_path);
    assert_eq!(seen[7].1, record_path);

    // Prune: every stored transaction, then the record, then the lock, then one sync.
    record.attempts.push("second".to_owned());
    sign(&store, &mut record, "tx-2");
    let before = hook.seen().len();
    store
        .prune(|_| PruneDecision::Remove)
        .expect("prune everything");
    let seen = hook.seen()[before..].to_vec();
    assert_eq!(
        seen,
        [
            (Step::Removed, store.blob_path(&record.id, 0)),
            (Step::Removed, store.blob_path(&record.id, 1)),
            (Step::Removed, record_path.clone()),
            (Step::Removed, store.lock_path(&record.id)),
            (Step::DirectorySynced, root.clone()),
        ]
    );
    assert_eq!(names(&root), [".identity.lock"]);
}

/// Stops a signing write at each of its eight boundaries in turn and checks what is on disk,
/// which is what a process killed at that instant would leave.
#[test]
fn a_fault_at_each_boundary_leaves_what_a_crash_there_would() {
    for fail_at in 0..8 {
        let dir = TempDir::new("crash");
        let original = Record::new(3, 1);
        open(&dir).write(&original).expect("seed");

        let hook = Arc::new(Recorder::failing_at(fail_at));
        let store = open_with(&dir, &hook);
        let mut record = original.clone();
        let updated = Mutex::new(false);
        let error = store
            .write_blob_then_record(&mut record, 0, b"signed-bytes", |record| {
                *updated.lock().expect("lock") = true;
                record.attempts[0] = "signed".to_owned();
                Ok::<(), StoreError<Id>>(())
            })
            .expect_err("injected fault");

        // The failure surfaces as I/O at the boundary's own path, and nothing runs after it.
        let seen = hook.seen();
        assert_eq!(seen.len(), fail_at + 1, "boundary {fail_at}");
        let StoreError::Io { path, source } = &error else {
            panic!("boundary {fail_at}: expected Io, got {error:?}");
        };
        assert_eq!(path, &seen[fail_at].1, "boundary {fail_at}");
        assert_eq!(source.to_string(), "injected fault");

        let blob = std::fs::read(store.blob_path(&original.id, 0)).ok();
        let on_disk = store.read(&original.id).expect("read").expect("record");
        let expected_blob = (fail_at >= 1).then(|| b"signed-bytes".to_vec());
        assert_eq!(blob, expected_blob, "boundary {fail_at}: blob");
        assert_eq!(
            *updated.lock().expect("lock"),
            fail_at >= 4,
            "boundary {fail_at}"
        );
        if fail_at >= 5 {
            assert_eq!(on_disk.attempts[0], "signed", "boundary {fail_at}");
        } else {
            assert_eq!(
                on_disk, original,
                "boundary {fail_at}: record must be untouched"
            );
        }
        // A fault before a rename leaves its temporary behind, as a crash would. Listing
        // ignores it.
        let temporaries = names(&dir.journal())
            .into_iter()
            .filter(|name| name.ends_with(".tmp"))
            .count();
        assert_eq!(
            temporaries,
            usize::from(fail_at == 0 || fail_at == 4),
            "boundary {fail_at}"
        );
        assert_eq!(store.records().expect("list").len(), 1);
    }
}

#[test]
fn a_record_write_stopped_after_its_rename_is_already_visible() {
    for (fail_at, visible) in [(0, false), (1, true), (2, true), (3, true)] {
        let dir = TempDir::new("record-crash");
        let hook = Arc::new(Recorder::failing_at(fail_at));
        let store = open_with(&dir, &hook);
        let record = Record::new(4, 1);
        assert!(store.write(&record).is_err());
        assert_eq!(
            store.read(&record.id).expect("read").is_some(),
            visible,
            "boundary {fail_at}"
        );
    }
}

// ---- stored transactions -------------------------------------------------------------------

#[test]
fn a_stored_transaction_is_on_disk_before_the_record_can_name_it() {
    let dir = TempDir::new("blob-order");
    let store = open(&dir);
    let mut record = Record::new(5, 2);
    store.write(&record).expect("write");
    let blob_path = store.blob_path(&record.id, 1);
    assert_eq!(blob_path, dir.journal().join("rec_0005.1.tx"));

    store
        .write_blob_then_record(&mut record, 1, b"second attempt", |record| {
            assert_eq!(
                std::fs::read(&blob_path).expect("blob is already durable"),
                b"second attempt"
            );
            record.attempts[1] = "signed".to_owned();
            Ok::<(), StoreError<Id>>(())
        })
        .expect("sign");
    assert_eq!(
        store.read_blob(&record.id, 1).expect("blob"),
        b"second attempt"
    );
    assert_eq!(
        store.read_blob_to_string(&record.id, 1).expect("blob"),
        "second attempt"
    );
    assert_eq!(store.read(&record.id).expect("read"), Some(record));
    #[cfg(unix)]
    assert_eq!(mode(&blob_path), 0o600);
}

#[test]
fn an_update_that_fails_leaves_an_orphan_blob_and_an_unchanged_record() {
    #[derive(Debug)]
    enum Refused {
        Store,
        Update,
    }
    impl From<StoreError<Id>> for Refused {
        fn from(_: StoreError<Id>) -> Self {
            Self::Store
        }
    }

    let dir = TempDir::new("blob-orphan");
    let store = open(&dir);
    let original = Record::new(6, 1);
    store.write(&original).expect("write");

    let mut record = original.clone();
    let error = store
        .write_blob_then_record(&mut record, 0, b"orphan", |record| {
            record.attempts[0] = "changed in memory".to_owned();
            Err(Refused::Update)
        })
        .expect_err("refused");
    assert!(matches!(error, Refused::Update));
    assert_eq!(
        record.attempts[0], "changed in memory",
        "memory is the caller's"
    );
    assert_eq!(store.read(&original.id).expect("read"), Some(original));
    assert_eq!(store.read_blob(&record.id, 0).expect("orphan"), b"orphan");
}

#[test]
fn a_missing_stored_transaction_is_corruption_not_absence() {
    let dir = TempDir::new("blob-missing");
    let store = open(&dir);
    let id = Id::new(7);

    let error = store.read_blob(&id, 0).expect_err("missing");
    assert!(matches!(
        &error,
        StoreError::Corrupt { path, reason: Corruption::MissingBlob }
            if path == &store.blob_path(&id, 0)
    ));
    assert_eq!(
        error.to_string(),
        format!(
            "journal record at {} is corrupt: record claims a stored transaction that is missing",
            store.blob_path(&id, 0).display()
        )
    );
    assert!(matches!(
        store.read_blob_to_string(&id, 0),
        Err(StoreError::Corrupt {
            reason: Corruption::MissingBlob,
            ..
        })
    ));

    std::fs::write(store.blob_path(&id, 0), [0xff, 0xfe]).expect("write");
    let error = store.read_blob_to_string(&id, 0).expect_err("not utf-8");
    assert!(
        matches!(&error, StoreError::Io { source, .. } if source.kind() == std::io::ErrorKind::InvalidData)
    );
    assert_eq!(store.read_blob(&id, 0).expect("bytes"), [0xff, 0xfe]);
}

// ---- locks ---------------------------------------------------------------------------------

/// Runs `take` on another thread and reports whether it returned within `wait`.
fn returns_within(wait: Duration, take: impl FnOnce() + Send + 'static) -> mpsc::Receiver<()> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        take();
        let _ = sender.send(());
    });
    assert!(
        receiver.recv_timeout(wait).is_err(),
        "the second lock returned while the first was held"
    );
    receiver
}

#[test]
fn a_record_lock_blocks_until_released() {
    let dir = TempDir::new("lock-record");
    let store = open(&dir);
    let id = Id::new(8);
    let held = store.lock_record(&id).expect("lock");

    let other = store.clone();
    let waiting = returns_within(Duration::from_millis(200), move || {
        drop(other.lock_record(&Id::new(8)).expect("second lock"));
    });
    drop(held);
    waiting
        .recv_timeout(Duration::from_secs(10))
        .expect("the second lock returns once the first is released");
}

#[test]
fn the_identity_lock_blocks_until_released_and_excludes_other_handles() {
    let dir = TempDir::new("lock-identity");
    let store = open(&dir);
    let held = store.lock_identity().expect("lock");

    let handle = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.journal().join(".identity.lock"))
        .expect("identity lock file");
    assert!(handle.try_lock_exclusive().is_err());

    let other = store.clone();
    let waiting = returns_within(Duration::from_millis(200), move || {
        drop(other.lock_identity().expect("second lock"));
    });
    drop(held);
    waiting
        .recv_timeout(Duration::from_secs(10))
        .expect("the second lock returns once the first is released");
    handle.try_lock_exclusive().expect("released");
}

#[test]
fn different_records_do_not_contend() {
    let dir = TempDir::new("lock-independent");
    let store = open(&dir);
    let _first = store.lock_record(&Id::new(1)).expect("first");
    let _second = store
        .lock_record(&Id::new(2))
        .expect("second, without waiting");
}

#[cfg(unix)]
#[test]
fn lock_files_are_private_and_retightened_on_every_lock() {
    let dir = TempDir::new("lock-modes");
    let store = open(&dir);
    let id = Id::new(3);
    drop(store.lock_identity().expect("identity"));
    drop(store.lock_record(&id).expect("record"));
    let identity = dir.journal().join(".identity.lock");
    let record = store.lock_path(&id);
    assert_eq!(record, dir.journal().join("rec_0003.lock"));
    assert_eq!((mode(&identity), mode(&record)), (0o600, 0o600));

    set_mode(&identity, 0o644);
    set_mode(&record, 0o644);
    drop(store.lock_identity().expect("identity"));
    drop(store.lock_record(&id).expect("record"));
    assert_eq!((mode(&identity), mode(&record)), (0o600, 0o600));
}

// ---- prune ---------------------------------------------------------------------------------

#[test]
fn prune_skips_a_locked_record_and_reports_what_it_kept() {
    let dir = TempDir::new("prune");
    let store = open(&dir);
    // 1: to remove, but locked. 2: to remove, two stored transactions. 3: unfinished.
    // 4: recent.
    let mut locked = Record::new(1, 1);
    let mut removed = Record::new(2, 2);
    for record in [&mut locked, &mut removed] {
        store.write(record).expect("write");
        sign(&store, record, "tx");
    }
    store.write(&Record::new(3, 1)).expect("write");
    store.write(&Record::new(4, 1)).expect("write");
    // Attempt 0 of record 2 never stored a transaction; prune removes what is there.
    let held = store.lock_record(&locked.id).expect("held elsewhere");

    let report = store
        .prune(|record| match record.id.0.as_str() {
            "rec_0003" => PruneDecision::RetainUnfinished,
            "rec_0004" => PruneDecision::RetainRecent,
            _ => PruneDecision::Remove,
        })
        .expect("prune");
    assert_eq!(
        report,
        PruneReport {
            pruned: 1,
            retained_unfinished: 1,
            retained_recent: 1,
            retained_locked: 1,
        }
    );
    assert_eq!(report.examined(), 4);
    assert_eq!(
        serde_json::to_value(report).expect("report"),
        json!({"pruned":1,"retained_unfinished":1,"retained_recent":1,"retained_locked":1})
    );
    // No lock file is created for a record prune keeps without trying to remove it.
    assert_eq!(
        names(&dir.journal()),
        [
            ".identity.lock",
            "rec_0001.0.tx",
            "rec_0001.json",
            "rec_0001.lock",
            "rec_0003.json",
            "rec_0004.json",
        ]
    );

    drop(held);
    let report = store.prune(|_| PruneDecision::Remove).expect("prune");
    assert_eq!(report.pruned, 3);
    assert_eq!(names(&dir.journal()), [".identity.lock"]);
}

#[test]
fn prune_waits_for_a_writer_holding_the_identity_lock() {
    let dir = TempDir::new("prune-identity");
    let store = open(&dir);
    store.write(&Record::new(1, 1)).expect("write");
    let held = store.lock_identity().expect("writer in progress");

    let other = store.clone();
    let waiting = returns_within(Duration::from_millis(200), move || {
        other.prune(|_| PruneDecision::Remove).expect("prune");
    });
    drop(held);
    waiting
        .recv_timeout(Duration::from_secs(10))
        .expect("prune runs once the writer is done");
    assert!(store.records().expect("list").is_empty());
}

/// A prune stopped partway (here: after the first stored transaction went) leaves the record,
/// which reads as corrupt when its missing transaction is asked for, and a rerun finishes.
#[test]
fn an_interrupted_prune_removes_the_record_last_and_can_be_rerun() {
    let dir = TempDir::new("prune-crash");
    let mut record = Record::new(5, 1);
    {
        let store = open(&dir);
        store.write(&record).expect("write");
        sign(&store, &mut record, "tx");
    }

    let hook = Arc::new(Recorder::failing_at(0));
    let store = open_with(&dir, &hook);
    assert!(store.prune(|_| PruneDecision::Remove).is_err());
    assert_eq!(hook.steps(), [Step::Removed]);
    assert!(store.contains(&record.id), "the record goes last");
    assert!(matches!(
        store.read_blob(&record.id, 0),
        Err(StoreError::Corrupt {
            reason: Corruption::MissingBlob,
            ..
        })
    ));

    let report = open(&dir).prune(|_| PruneDecision::Remove).expect("rerun");
    assert_eq!(report.pruned, 1);
    assert_eq!(names(&dir.journal()), [".identity.lock"]);
}

// ---- listing -------------------------------------------------------------------------------

#[test]
fn only_json_files_are_records() {
    let dir = TempDir::new("list");
    let store = open(&dir);
    let mut record = Record::new(1, 1);
    store.write(&record).expect("write");
    sign(&store, &mut record, "tx");
    store.write(&Record::new(2, 1)).expect("write");
    drop(store.lock_identity().expect("identity"));
    drop(store.lock_record(&record.id).expect("lock"));
    std::fs::write(dir.journal().join(".12345.00000000deadbeef.tmp"), b"{").expect("tmp");
    std::fs::write(dir.journal().join("README"), b"not a record").expect("readme");
    std::fs::write(dir.journal().join("stray.tx"), b"{").expect("tx");

    let mut ids: Vec<String> = store
        .records()
        .expect("list")
        .into_iter()
        .map(|record| record.id.0)
        .collect();
    ids.sort();
    assert_eq!(ids, ["rec_0001", "rec_0002"]);
}

#[test]
fn a_stray_json_file_fails_every_listing() {
    let dir = TempDir::new("list-stray");
    let store = open(&dir);
    store.write(&Record::new(1, 1)).expect("write");
    let stray = dir.journal().join("notes.json");
    std::fs::copy(store.record_path(&Id::new(1)), &stray).expect("copy");

    let error = store.records().expect_err("stray");
    assert!(matches!(
        &error,
        StoreError::Corrupt { path, reason: Corruption::FileName } if path == &stray
    ));
    assert_eq!(
        error.to_string(),
        format!(
            "journal record at {} is corrupt: record filename is not a valid operation id",
            stray.display()
        )
    );
}

#[test]
fn a_record_filed_under_another_id_is_refused() {
    let dir = TempDir::new("list-mismatch");
    let store = open(&dir);
    let (a, b) = (Id::new(1), Id::new(2));
    store.write(&Record::new(1, 1)).expect("write");
    std::fs::copy(store.record_path(&a), store.record_path(&b)).expect("copy");

    let error = store.read(&b).expect_err("mismatch");
    assert!(matches!(
        &error,
        StoreError::IdMismatch { expected, found } if expected == &b && found == &a
    ));
    assert_eq!(
        error.to_string(),
        "journal record under rec_0002 claims to be rec_0001"
    );
    assert!(matches!(
        store.records(),
        Err(StoreError::IdMismatch { .. })
    ));
}

#[test]
fn the_version_window_is_checked_before_anything_else_in_the_record() {
    let dir = TempDir::new("list-version");
    let store = open(&dir);
    let id = Id::new(1);
    let path = store.record_path(&id);

    for (version, readable) in [(0, false), (1, false), (2, true), (3, true), (4, false)] {
        let mut record = Record::new(1, 1);
        record.version = version;
        std::fs::write(&path, serde_json::to_vec(&record).expect("encode")).expect("write");
        match store.read(&id) {
            Ok(Some(read)) => assert!(readable, "version {version} read as {read:?}"),
            Err(StoreError::UnsupportedVersion(v)) => {
                assert!(!readable, "version {version} refused");
                assert_eq!(v, version);
            }
            other => panic!("version {version}: {other:?}"),
        }
    }

    // Out of window and empty: the version wins.
    std::fs::write(
        &path,
        serde_json::to_vec(&json!({"version": 9, "id": "rec_0001", "attempts": []}))
            .expect("encode"),
    )
    .expect("write");
    let error = store.read(&id).expect_err("version");
    assert_eq!(
        error.to_string(),
        "journal record schema version 9 is not supported"
    );

    // In window and empty: corrupt.
    std::fs::write(
        &path,
        serde_json::to_vec(&json!({"version": 3, "id": "rec_0001", "attempts": []}))
            .expect("encode"),
    )
    .expect("write");
    let error = store.read(&id).expect_err("empty");
    assert!(matches!(
        &error,
        StoreError::Corrupt {
            reason: Corruption::NoAttempts,
            ..
        }
    ));
    assert_eq!(
        error.to_string(),
        format!(
            "journal record at {} is corrupt: record has no attempts",
            path.display()
        )
    );
}

#[test]
fn an_unparseable_record_fails_closed() {
    let dir = TempDir::new("list-json");
    let store = open(&dir);
    let id = Id::new(1);
    let path = store.record_path(&id);
    std::fs::write(&path, b"{ not json").expect("write");

    let error = store.read(&id).expect_err("json");
    assert_eq!(
        error.to_string(),
        format!(
            "journal record at {} is not readable: key must be a string at line 1 column 3",
            path.display()
        )
    );
    assert!(matches!(store.records(), Err(StoreError::Json { .. })));
}

#[test]
fn corruption_reasons_are_fixed_text() {
    assert_eq!(Corruption::NoAttempts.as_str(), "record has no attempts");
    assert_eq!(
        Corruption::FileName.as_str(),
        "record filename is not a valid operation id"
    );
    assert_eq!(
        Corruption::MissingBlob.as_str(),
        "record claims a stored transaction that is missing"
    );
    let error: StoreError<Id> = StoreError::Io {
        path: PathBuf::from("/state/journal/record.json"),
        source: std::io::Error::other("disk on fire"),
    };
    assert_eq!(
        error.to_string(),
        "journal io error at /state/journal/record.json: disk on fire"
    );
}

/// A store, and the locks a lease holds, are as thread- and unwind-safe as the plain path and
/// files `sdk/rs` held before the engine was shared.
#[test]
fn stores_and_locks_keep_their_auto_traits() {
    fn assert_auto<T: Send + Sync + Unpin + std::panic::UnwindSafe + std::panic::RefUnwindSafe>() {}
    assert_auto::<Store<Record>>();
    assert_auto::<erebus_journal::IdentityLock>();
    assert_auto::<erebus_journal::RecordLock>();

    // Like the error it maps into, it carries an `io::Error`, so it crosses threads but is not
    // unwind-safe.
    fn assert_threads<T: Send + Sync>() {}
    assert_threads::<StoreError<Id>>();
}
