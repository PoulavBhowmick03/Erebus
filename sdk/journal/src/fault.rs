//! Durable boundaries, and the hook a fault matrix uses to stop at one.

use std::panic::RefUnwindSafe;
use std::path::Path;

/// A durable step the engine has just completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Step {
    /// A temporary file's contents reached disk. Nothing names it yet.
    FileSynced,
    /// The temporary file replaced its final name. The new name may still live only in the
    /// page cache.
    Renamed,
    /// The journal directory was synced: every rename and removal before it is durable.
    DirectorySynced,
    /// A stored transaction is durable under its final name. No record names it yet.
    BlobWritten,
    /// A record is durable under its final name.
    RecordWritten,
    /// Prune removed a file. Not durable until the next [`Step::DirectorySynced`].
    Removed,
}

/// One boundary: which step completed, and the path it applied to.
///
/// For [`Step::FileSynced`] the path is the temporary file, for [`Step::DirectorySynced`] the
/// journal directory, and otherwise the file's final name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Boundary<'a> {
    /// The step that completed.
    pub step: Step,
    /// The path it applied to.
    pub path: &'a Path,
}

/// Called after every durable step, in the order the steps happen.
///
/// An error stops the operation at that boundary: nothing after it runs, and the caller sees
/// an I/O failure at the boundary's path, which is what a real failure there looks like. What
/// is left on disk is what a process killed at that instant would leave.
///
/// Production passes [`NoFaults`], so the only production cost is the call itself. The engine
/// has no other test-only branch.
///
/// `RefUnwindSafe` keeps a [`crate::Store`] exactly as thread- and unwind-safe as the plain
/// path it replaced in `sdk/rs`, whose public journal types hold one.
pub trait FaultHook: Send + Sync + RefUnwindSafe {
    /// The step in `boundary` has completed.
    fn after(&self, _boundary: Boundary<'_>) -> std::io::Result<()> {
        Ok(())
    }
}

/// The production hook: every boundary passes.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoFaults;

impl FaultHook for NoFaults {}
