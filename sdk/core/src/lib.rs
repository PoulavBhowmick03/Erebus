//! Canonical agreement and shared protocol types for Erebus.
//!
//! This crate owns the chain-neutral half of the Metropolis protocol: agreement identifiers,
//! canonical byte encoding, the blinded deal commitment, role-specific authorizations, the
//! service record, spending policy evaluation, and settlement backend capability types.
//!
//! It deliberately depends on no chain library and on no proving system. Settlement adapters
//! will translate between these types and a specific chain. The legacy STRK20 adapter does
//! not accept canonical v1 agreements. Shared logic must not branch on a chain name or
//! network; a chain namespace is opaque data. The byte
//! encoding is specified in `docs/metropolis-agreement.md` and pinned by the known-answer
//! vectors under `tests/fixtures`.
//!
//! Agreement consent signs the blinded commitment. Authorization verification requires its
//! opening and checks that the supplied terms produce the signed commitment.
#![forbid(unsafe_code)]

pub mod auth;
pub mod commitment;
pub mod domain;
pub mod encoding;
pub mod ids;
pub mod policy;
pub mod service;
pub mod settlement;
pub mod shielded;
pub mod suite;
pub mod terms;
