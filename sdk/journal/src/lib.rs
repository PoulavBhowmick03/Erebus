//! Durable storage for operation journals.
//!
//! A settlement backend journals every write so that a crash mid-write can never turn into a
//! duplicate chain effect or a forgotten one. This crate owns the part of that which does not
//! depend on the chain: where records live, how they are locked, how they reach disk, and how
//! they are read back and pruned. The record types, their stages, and the rules for moving
//! between stages belong to the backend; the Starknet client in `sdk/rs` is the first one.
//!
//! One directory per journal:
//!
//! | Name | What it is |
//! |---|---|
//! | `<id>.json` | one record, replaced only by atomic rename, mode `0600` |
//! | `<id>.<n>.tx` | the stored transaction of attempt `n`, written before any record names it, mode `0600` |
//! | `<id>.lock` | advisory lock held while a record is leased |
//! | `.identity.lock` | advisory lock held by every writer and by exclusive readers |
//!
//! The directory itself is mode `0700`. Nothing here decides what a record means; a record
//! the engine cannot read, or one filed under the wrong name, fails closed rather than being
//! skipped.
//!
//! Every durable step reports itself to a [`FaultHook`]. Production uses [`NoFaults`]; a fault
//! matrix passes a hook that records the steps, or fails at one of them, which stops a write
//! exactly where a killed process would.
#![forbid(unsafe_code)]

mod error;
mod fault;
mod record;
mod store;

pub use error::{Corruption, StoreError};
pub use fault::{Boundary, FaultHook, NoFaults, Step};
pub use record::{JournalRecord, RecordId};
pub use store::{IdentityLock, PruneDecision, PruneReport, RecordLock, Store};
