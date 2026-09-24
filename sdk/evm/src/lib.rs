//! Public-bound EVM settlement adapter for Erebus (Metropolis M3).
//!
//! This crate turns one authorized canonical agreement into one on-chain payment, exactly once,
//! on an EVM chain. It is the EVM half of the settlement backend contract declared in
//! `erebus_core::settlement`:
//!
//! - [`backend::EvmSettlementBackend::prepare`] validates the terms and both role
//!   authorizations locally and produces backend evidence.
//! - [`backend::EvmSettlementBackend::submit`] signs and submits the settlement transaction.
//! - [`backend::EvmSettlementBackend::verify`] reads chain evidence and returns a normalized
//!   `SettlementReceipt`.
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
pub mod deployment;
pub mod error;
pub mod evidence;
