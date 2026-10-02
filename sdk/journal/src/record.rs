//! What the storage engine needs to know about a backend's records, and nothing more.

use core::fmt;

use serde::de::DeserializeOwned;
use serde::Serialize;

/// The identifier a record is filed under.
///
/// The engine turns it into file names (`<stem>.json`, `<stem>.<n>.tx`, `<stem>.lock`) and
/// parses record file names back into it when listing. Both directions belong to the backend,
/// so the backend decides what a valid id is. The engine relies on one thing: a stem is a
/// single path component with no separator, which any validated id format guarantees and an
/// arbitrary caller string does not.
pub trait RecordId: Clone + PartialEq + fmt::Debug + fmt::Display {
    /// The id as it appears in front of the extension of every file filed under it.
    fn as_file_stem(&self) -> &str;

    /// Parses the stem of a record's file name.
    ///
    /// `None` fails the read closed as [`crate::Corruption::FileName`]: a `.json` file that
    /// is not named after a valid id is not something the engine may skip.
    fn from_file_stem(stem: &str) -> Option<Self>;
}

/// A durable record, as the storage engine sees it.
///
/// The engine serializes and deserializes it whole with `serde_json`, so the backend's derives
/// are the on-disk format. The engine checks only what it has to before handing a record back:
/// that its schema is in the readable window, that it has an attempt, and that it is filed
/// under its own id.
pub trait JournalRecord: Serialize + DeserializeOwned {
    /// The id the record is filed under.
    type Id: RecordId;

    /// Schema version this backend writes. A record with a higher version fails closed.
    const CURRENT_VERSION: u32;

    /// Oldest schema this backend can still read. A record with a lower version fails closed.
    const OLDEST_READABLE_VERSION: u32;

    /// Schema version the record carries.
    fn version(&self) -> u32;

    /// The id the record claims to be.
    fn record_id(&self) -> &Self::Id;

    /// How many attempts the record holds.
    ///
    /// A record with none is corrupt: it is created with one and attempts are only appended.
    /// The count also bounds the stored transactions prune removes with the record.
    fn attempt_count(&self) -> usize;
}
