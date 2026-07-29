//! Rust client for Erebus — private agent coordination and settlement on STRK20.
//!
//! ## Why this exists
//!
//! `starkware-libs/starknet-privacy` ships a TypeScript SDK and a Rust
//! `discovery-core` crate. `discovery-core` covers the *read* side — hashes, storage
//! slots, decryption, note discovery — but there is no Rust write side: nothing builds
//! `ClientAction`s, serialises Cairo calldata, signs the invoke, or calls the proving
//! service. This crate is that gap.
//!
//! ## How correctness is maintained
//!
//! Every derivation is pinned by a known-answer test against vectors emitted by the
//! Cairo contract itself (`packages/privacy/src/tests/generate_reference_data.cairo`).
//! This matters more than usual here: there is no written spec for the wire format, and
//! a wrong preimage fails *silently* — the note lands at a storage slot nobody reads,
//! or decrypts to garbage. See `docs/friction.md` F2.
//!
//! Where a KAT is not available from Cairo (notably Cairo Serde of `ClientAction`), the
//! TypeScript SDK is the oracle and we diff byte-for-byte.

#![forbid(unsafe_code)]

pub mod action_set;
pub mod actions;
pub mod channel;
pub mod hashes;
pub mod prover;
pub mod signing;
pub mod subchannel;
pub mod tx;
pub mod wire;
