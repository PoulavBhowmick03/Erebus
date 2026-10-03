//! Offchain Eleusis transport for Erebus.
//!
//! The negotiation graph for a Metropolis deal moves over an authenticated, encrypted channel
//! between two participants, with no chain transaction. This crate owns that channel:
//!
//! - [`identity`]: the X25519 transport key and the secp256k1 authorization identity.
//! - [`session`]: a Noise XX session with a capability-binding prologue.
//! - [`message`]: the canonical message envelope and its authenticated body.
//! - [`transcript`]: per-author hash chains, ordering and replay rejection, and the root that
//!   the agreement commits to.
//! - [`store`]: the durable transcript and session state.
//! - [`relay`]: the minimal ciphertext mailbox.
//! - [`descriptor`]: signed service publication and discovery.
//! - [`binding`]: private discovery-to-shielded-agreement identity attestations.
//! - [`negotiation`]: typed price negotiation and the durable boundary before final signatures.
//! - [`socket`]: bounded descriptor-authenticated TCP delivery.
//!
//! The design and its decisions are specified in `docs/metropolis-m2-decisions.md`. The crate is
//! chain-neutral: it depends on `erebus-core` for canonical encoding. Transcript hash
//! version 1 uses Keccak independently of the selected settlement agreement suite.
//!
//! ```text
//! Cdeal = suite-specific commitment(terms, blinding)
//! transcript_root = Keccak256("EREBUS_TRANSCRIPT_ROOT_V1" || deal_id || head_buyer || head_seller)
//! ```
#![forbid(unsafe_code)]

pub mod binding;
pub mod descriptor;
pub mod disclosure;
pub mod hashing;
pub mod identity;
pub mod limits;
pub mod message;
pub mod negotiation;
pub mod relay;
pub mod session;
pub mod socket;
pub mod store;
pub mod transcript;
