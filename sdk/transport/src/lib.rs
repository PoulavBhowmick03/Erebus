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
//!
//! The design and its decisions are specified in `docs/metropolis-m2-decisions.md`. The crate is
//! chain-neutral: it depends on `erebus-core` for canonical encoding and the agreement suite
//! hash, and on no chain or proving library.
//!
//! ```text
//! Cdeal = Hash_suite("EREBUS_DEAL_COMMITMENT_V1" || terms || blinding)
//! transcript_root = Hash_suite("EREBUS_TRANSCRIPT_ROOT_V1" || deal_id || head_buyer || head_seller)
//! ```
#![forbid(unsafe_code)]

pub mod descriptor;
pub mod identity;
pub mod limits;
pub mod message;
pub mod relay;
pub mod session;
pub mod store;
pub mod transcript;
