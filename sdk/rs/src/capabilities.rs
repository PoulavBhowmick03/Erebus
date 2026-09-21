//! What the existing STRK20 path actually guarantees.
//!
//! The shared core (`erebus-core`) selects a settlement backend by declared capabilities
//! instead of branching on a chain name. This module is the honest declaration for the
//! STRK20 implementation in this crate, and it is deliberately conservative:
//!
//! - **Mode**: shielded. Payments move through the STRK20 pool, so the amount and the
//!   recipient are not public.
//! - **Guarantees**: hidden amount and hidden recipient. Scoped disclosure is available
//!   through wire-v3 grants, but is not a guarantee common to all legacy channels.
//!   It does **not** declare
//!   [`Guarantee::AgreementBoundSettlement`]: the amount equality between the accepted offer
//!   and the payment is a Rust check in `channel::accept_and_settle_with_change`, not a
//!   predicate the pool proof enforces. The M0 enforcement map
//!   (`docs/metropolis-m0-baseline.md`) records that boundary.
//! - **Local proving**: false. The current path submits through a hosted proving service
//!   (`prover.rs`); only the planned EVM shielded backend proves locally by default
//!   (decisions D01).
//!
//! A session that requires proof-enforced agreement binding must not select this backend.
//! [`erebus_core::settlement::check_capabilities`] fails explicitly and names the missing
//! guarantee; nothing here silently downgrades a signed privacy requirement.

use std::collections::BTreeSet;

use erebus_core::settlement::BackendCapabilities;
use erebus_core::terms::{Guarantee, GuaranteeSet, SettlementMode};

/// Declares the capabilities of the existing STRK20 settlement path.
///
/// Changing this declaration is a protocol statement about what the current implementation
/// proves. It must match the source, not the intended design; the integration test in
/// `tests/capabilities.rs` pins the fields that would be easy to overstate.
#[must_use]
pub fn strk20_capabilities() -> BackendCapabilities {
    let mut modes = BTreeSet::new();
    modes.insert(SettlementMode::Shielded);

    let mut guarantees = GuaranteeSet::empty();
    guarantees.insert(Guarantee::HiddenAmount);
    guarantees.insert(Guarantee::HiddenRecipient);

    BackendCapabilities {
        suites: BTreeSet::new(),
        modes,
        guarantees,
        local_proving: false,
    }
}
