//! Errors for the EVM settlement adapter.

use erebus_core::auth::AuthError;
use erebus_core::commitment::CommitmentError;
use erebus_core::domain::DomainError;
use erebus_core::encoding::EncodingError;
use erebus_core::ids::IdError;
use erebus_core::settlement::SelectionError;
use erebus_core::suite::SuiteError;
use erebus_core::terms::TermsError;

use crate::evidence::EvidenceError;

/// The adapter could not prepare, submit, or verify a settlement.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EvmError {
    /// RPC answers disagree or omit evidence required for a safe observation.
    #[error("inconsistent observation: {0}")]
    InconsistentObservation(&'static str),
    /// A pinned block changed during the observation.
    #[error("observation anchor is no longer canonical")]
    AnchorNotCanonical,
    /// The endpoint did not supply an explicit finalized block.
    #[error("explicit RPC finalized block unavailable")]
    FinalityUnavailable,
    /// A bounded history scan or ancestry walk could not complete.
    #[error("observation history limit exceeded")]
    ObservationLimit,
    /// Settlement calldata or event data was malformed.
    #[error(transparent)]
    Abi(#[from] crate::abi::AbiDecodeError),
    /// The agreement terms were invalid.
    #[error(transparent)]
    Terms(#[from] TermsError),
    /// The commitment or deal identity could not be derived.
    #[error(transparent)]
    Commitment(#[from] CommitmentError),
    /// An authorization failed verification.
    #[error(transparent)]
    Authorization(#[from] AuthError),
    /// The deployment domain was invalid.
    #[error(transparent)]
    Domain(#[from] DomainError),
    /// The agreement suite is not implemented.
    #[error(transparent)]
    Suite(#[from] SuiteError),
    /// An identifier was invalid.
    #[error(transparent)]
    Id(#[from] IdError),
    /// A canonical encoding could not be read.
    #[error(transparent)]
    Encoding(#[from] EncodingError),
    /// Backend evidence could not be read or was not produced by this adapter.
    #[error(transparent)]
    Evidence(#[from] EvidenceError),
    /// The selected backend cannot provide the agreement's required capabilities.
    #[error(transparent)]
    Selection(#[from] SelectionError),
    /// The chain namespace is not an `eip155` namespace with a numeric chain id.
    #[error("deployment namespace `{0}` is not an EVM chain namespace")]
    NotEvmNamespace(String),
    /// This adapter only settles public-bound agreements.
    #[error("EVM public-bound settlement does not implement the agreement's settlement mode")]
    UnsupportedMode,
    /// The agreement's deployment is not this adapter's deployment.
    #[error("agreement deployment does not match the configured EVM deployment")]
    DeploymentMismatch,
    /// The RPC endpoint serves a different chain than the configured deployment.
    #[error("RPC chain id {found} does not match configured chain id {expected}")]
    ChainIdMismatch {
        /// Chain id in the deployment configuration.
        expected: u64,
        /// Chain id returned by the RPC endpoint.
        found: u64,
    },
    /// The asset identifier is not a lowercase ERC-20 address on this chain.
    #[error("asset identifier `{0}` is not a supported ERC-20 address")]
    UnsupportedAsset(String),
    /// An authorization was for the wrong role.
    #[error("authorization for role {found} was supplied where {expected} was required")]
    WrongRole {
        /// The role the caller supplied.
        found: &'static str,
        /// The role the parameter requires.
        expected: &'static str,
    },
    /// The 65-byte `r || s || v` signature had the wrong length.
    #[error("authorization signature must be 65 bytes, found {0}")]
    SignatureLength(usize),
    /// The transaction signing key was invalid.
    #[error("EVM transaction signing key is invalid")]
    InvalidSigningKey,
    /// The JSON-RPC endpoint returned an error.
    #[error("EVM RPC error: {0}")]
    Rpc(String),
    /// The receipt for a submitted transaction could not be found.
    #[error("no receipt found for the submitted transaction")]
    MissingReceipt,
    /// The transaction named by a receipt could not be loaded.
    #[error("transaction for the submitted receipt could not be found")]
    MissingTransaction,
    /// The transaction reverted on chain.
    #[error("settlement transaction reverted")]
    TransactionReverted,
    /// The settled event did not carry the expected commitment or deal identity.
    #[error("settlement receipt does not match the prepared agreement")]
    ReceiptMismatch,
    /// Routing fields in a prepared settlement did not match its canonical evidence.
    #[error("prepared settlement fields do not match their canonical evidence")]
    PreparedMismatch,
    /// A contract artifact could not be read (test and deployment tooling only).
    #[error("contract artifact error: {0}")]
    Artifact(String),
    /// The system clock was before the Unix epoch.
    #[error("system time is before the Unix epoch")]
    Clock,
    /// EIP-1559 fee parameters were unusable.
    #[error("invalid EIP-1559 fees: {0}")]
    InvalidFees(&'static str),
    /// Raw bytes do not represent a supported canonical signed transaction.
    #[error("invalid signed transaction: {0}")]
    TransactionEncoding(String),
    /// Persisted transaction fields differ from the expected operation.
    #[error("signed transaction does not match the expected settlement intent")]
    SignedIntentMismatch,
}
