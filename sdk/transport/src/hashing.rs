//! Transcript hashing is independent of settlement agreement suites.
//!
//! Version 1 preserves the original Keccak byte preimages and stored roots.
//! A future hash or encoding change requires a new transport hash version.

/// Keccak hashing of the version-1 canonical transport records.
pub const TRANSCRIPT_HASH_VERSION: u16 = 1;

/// An unsupported transcript hash version was selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("transcript hash version {0} is not supported")]
pub struct HashVersionError(pub u16);

/// Rejects unknown hash versions independently of the agreement suite registry.
pub fn check_version(version: u16) -> Result<(), HashVersionError> {
    if version != TRANSCRIPT_HASH_VERSION {
        return Err(HashVersionError(version));
    }
    Ok(())
}

/// Hashes canonical, domain-separated transport bytes under version 1.
pub(crate) fn hash(parts: &[&[u8]]) -> [u8; 32] {
    erebus_core::suite::keccak256(parts)
}
