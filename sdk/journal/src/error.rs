//! Storage failures. Every variant fails closed: none of them may be read as "no effect".

use core::fmt;
use std::path::PathBuf;

/// Why a file in the journal cannot be trusted.
///
/// The text of each reason is part of the error message a backend shows, and `sdk/rs` pins it,
/// so it is fixed here rather than composed at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corruption {
    /// A record with an empty attempt list.
    NoAttempts,
    /// A `.json` file whose name is not a valid record id.
    FileName,
    /// A record says a transaction was stored and the file is not there.
    ///
    /// Reconciliation must not read that as "nothing was submitted".
    MissingBlob,
}

impl Corruption {
    /// The reason as it appears in error messages.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoAttempts => "record has no attempts",
            Self::FileName => "record filename is not a valid operation id",
            Self::MissingBlob => "record claims a stored transaction that is missing",
        }
    }
}

impl fmt::Display for Corruption {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Storage failure, generic over the backend's record id.
///
/// Deliberately exhaustive: a backend maps every variant into its own error type, and a new
/// variant should break that mapping at compile time rather than fall into a catch-all.
#[derive(Debug, thiserror::Error)]
pub enum StoreError<Id: fmt::Debug + fmt::Display> {
    /// Filesystem failure, including one injected by a [`crate::FaultHook`].
    #[error("journal io error at {}: {source}", path.display())]
    Io {
        /// Path involved.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
    /// A record could not be parsed, or could not be encoded for writing.
    #[error("journal record at {} is not readable: {source}", path.display())]
    Json {
        /// Path involved.
        path: PathBuf,
        /// Underlying error.
        source: serde_json::Error,
    },
    /// A record's schema version is outside the backend's readable window.
    #[error("journal record schema version {0} is not supported")]
    UnsupportedVersion(u32),
    /// A record, or a file filed beside it, contradicts itself.
    #[error("journal record at {} is corrupt: {reason}", path.display())]
    Corrupt {
        /// Path involved.
        path: PathBuf,
        /// What was wrong.
        reason: Corruption,
    },
    /// A record was found under the wrong id.
    #[error("journal record under {expected} claims to be {found}")]
    IdMismatch {
        /// Id the file name says.
        expected: Id,
        /// Id the record carries.
        found: Id,
    },
}
