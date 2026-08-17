//! Rust client for Erebus private coordination and settlement on STRK20.
//!
//! ## Why this exists
//!
//! `starkware-libs/starknet-privacy` ships a TypeScript SDK and a Rust
//! `discovery-core` crate. `discovery-core` reads hashes, storage slots, and notes. It does
//! not build `ClientAction`s, serialize Cairo calldata, sign invokes, or call the prover.
//! This crate provides the Rust write path.
//!
//! ## How correctness is maintained
//!
//! Every derivation is pinned by a known-answer test against vectors emitted by the
//! Cairo contract itself (`packages/privacy/src/tests/generate_reference_data.cairo`).
//! There is no written wire specification. A wrong preimage derives an unread storage slot
//! or decrypts to another value without an error. See `docs/friction.md` F2.
//!
//! Where a KAT is not available from Cairo (notably Cairo Serde of `ClientAction`), the
//! TypeScript SDK is the oracle and we diff byte-for-byte.

#![forbid(unsafe_code)]

pub mod action_set;
pub mod actions;
pub mod calldata;
pub mod channel;
pub mod client;
pub mod decrypt;
pub mod disclosure;
pub mod doctor;
pub mod erc20;
pub mod execution;
pub mod hashes;
pub mod keys;
pub mod negotiation;
pub mod prover;
pub mod read;
pub mod rpc;
pub mod signing;
pub mod state;
pub mod subchannel;
pub mod tx;
pub mod wire;
