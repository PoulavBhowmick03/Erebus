//! Offline transaction preparation for journaled settlement (Metropolis M6).
//!
//! Sign with explicit nonce and fees, persist the raw bytes, then broadcast separately.
//! Restoring bytes must validate the full expected intent, not merely decode a transaction.
//! RPC nonce acquisition and exact-byte broadcast are separate from signing and storage.
//! Supported submission uses the coordinator-backed broadcast path. Operator reconciliation
//! requires paired finalized evidence; signer claims release only from verified consumed nonces.

mod estimate;
mod nonce;
mod observation;
mod plan;
mod rpc;
mod transaction;

pub use observation::{
    BlockRef, FinalizedNonce, HistoricalObservation, Inclusion, InclusionLog, ObservationJournal,
    ObservationLimits, TxStatus, MAX_CONCURRENT_LOG_QUERIES, MAX_INCLUSION_LOGS,
    MAX_INCLUSION_LOG_DATA, MAX_INCLUSION_LOG_TOPICS,
};
pub use rpc::{Broadcast, BroadcastOutcome, EvmChain, JournaledBroadcastError};

pub use nonce::{NonceError, SignerJournal};
pub use plan::SigningPlan;

pub use transaction::{
    build_signed, build_signed_call, settlement_calldata, Eip1559Fees, SignedTransaction,
    TransactionKey, TransactionParams, REPLACEMENT_BUMP_PERCENT,
};
