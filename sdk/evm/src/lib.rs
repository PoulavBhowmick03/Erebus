//! Public-bound EVM settlement adapter for Erebus (Metropolis M3).
//!
//! This crate turns one authorized canonical agreement into one on-chain payment, exactly once,
//! on an EVM chain. It is the EVM half of the settlement backend contract declared in
//! `erebus_core::settlement`:
//!
//! - [`backend::EvmSettlementBackend::prepare`] validates the terms and both role
//!   authorizations locally and produces backend evidence.
//! - [`chain::SigningPlan`] signs locally through the durable coordinator lifecycle.
//! - [`chain::EvmChain::broadcast_journaled`] sends the exact persisted transaction bytes.
//! - [`chain::EvmChain::finalized_deal_evidence_resumable_agreed`] checks two providers before
//!   the coordinator reconciles payment. Confirmation counts are not finality evidence.
//!
//! The former backend `submit`, `settle`, `verify`, and `with_confirmations` APIs have been
//! removed. Preparation and estimates do not provide a submission shortcut.
//!
//! ```compile_fail
//! use erebus_evm::backend::EvmSettlementBackend;
//! use erebus_core::settlement::PreparedSettlement;
//! async fn unjournaled(backend: &EvmSettlementBackend, prepared: &PreparedSettlement) {
//!     backend.submit(prepared).await;
//! }
//! ```
//!
//! ```compile_fail
//! use erebus_evm::backend::EvmSettlementBackend;
//! use erebus_core::settlement::PreparedSettlement;
//! async fn unjournaled(backend: &EvmSettlementBackend, prepared: &PreparedSettlement) {
//!     backend.settle(prepared).await;
//! }
//! ```
//!
//! ```compile_fail
//! use erebus_evm::backend::EvmSettlementBackend;
//! use erebus_core::settlement::PreparedSettlement;
//! async fn legacy_finality(backend: &EvmSettlementBackend, prepared: &PreparedSettlement) {
//!     backend.verify(prepared, &[0; 32]).await;
//! }
//! ```
//!
//! ```compile_fail
//! use erebus_evm::backend::EvmSettlementBackend;
//! fn confirmation_finality(backend: EvmSettlementBackend) {
//!     backend.with_confirmations(0);
//! }
//! ```
//!
//! Payment amount, asset, payer, and recipient are public in this mode. That is the intended
//! trade for M3: it validates the full architecture — authorized agreement to atomic payment to
//! receipt — without claiming payment privacy. Shielded settlement is M4/M5, and this adapter
//! never declares a privacy guarantee it does not provide.
//!
//! The contract is in [`contracts/evm`](../../contracts/evm). Its decoder is pinned against the
//! Rust agreement vectors by a Foundry known-answer test, so the bytes the adapter sends are the
//! bytes the contract recomputes the commitment from.
#![forbid(unsafe_code)]

pub mod abi;
pub mod backend;
pub mod chain;
pub mod deployment;
pub mod disclosure;
pub mod error;
pub mod evidence;
pub mod readiness;
pub mod relay;
