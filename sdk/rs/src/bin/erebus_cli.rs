//! `erebus-cli`: one JSON request, one JSON response, one process.
//!
//! Requests carry paths to the pool and account key files. They never carry key values.
//! Channel handles are opaque identifiers into the Rust-owned state directory; channel
//! keys never appear in ordinary operation results. `grant_viewing_key` is the sole
//! intentional secret export.

#![forbid(unsafe_code)]

use std::io::Read;
use std::path::PathBuf;

use erebus_sdk::channel::ChannelError;
use erebus_sdk::client::{
    Client, ClientConfig, ClientError, ErebusClient, OfferId, OfferTerms, ViewingKeyGrant,
};
use erebus_sdk::execution::ExecutionError;
use erebus_sdk::keys::{generate_pool_key_file, KeyFileError};
use erebus_sdk::negotiation::NegotiationError;
use erebus_sdk::operation::OperationId;
use erebus_sdk::prover::ProverError;
use erebus_sdk::state::{ChannelHandle, StateError};
use erebus_sdk::subchannel::IndexError;
use erebus_sdk::wire::{WireError, WireVersion};
use serde::{Deserialize, Serialize};
use starknet_types_core::felt::Felt;

#[derive(Debug, Deserialize)]
#[serde(
    tag = "method",
    content = "params",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum Request {
    Version,
    /// Creates a pool identity key directly in a protected Rust-owned file.
    GeneratePoolKey {
        path: PathBuf,
    },
    OpenChannel {
        config: ConfigParams,
        operation_id: String,
        counterparty: String,
    },
    ProposeOffer {
        config: ConfigParams,
        operation_id: String,
        handle: String,
        terms: TermsParams,
    },
    CounterOffer {
        config: ConfigParams,
        operation_id: String,
        handle: String,
        reply_to: String,
        terms: TermsParams,
    },
    ReadChannelState {
        config: ConfigParams,
        handle: String,
    },
    /// Classify every journalled operation against the chain. Read-only: it submits
    /// nothing and never repairs by itself (plan.md decision 5).
    Reconcile {
        config: ConfigParams,
    },
    /// Act on one reconciled operation. The only path that may resubmit, and only when
    /// asked by name.
    ResumeOperation {
        config: ConfigParams,
        operation_id: String,
    },
    /// Rebuild channel records from the pool key and chain data. Additive: an existing
    /// record is left alone, never overwritten.
    RebuildState {
        config: ConfigParams,
    },
    AcceptAndSettle {
        config: ConfigParams,
        operation_id: String,
        handle: String,
        offer_id: String,
    },
    GrantViewingKey {
        config: ConfigParams,
        handle: String,
        deal_id: String,
        grantee: String,
        expires_at: u64,
    },
    Reveal {
        config: ConfigParams,
        viewing_key: Box<ViewingKeyGrant>,
    },
    Approve {
        config: ConfigParams,
        operation_id: String,
        amount: String,
    },
    Allowance {
        config: ConfigParams,
    },
    /// Read-only pre-flight inspection and reports every fault in one run.
    Doctor {
        config: ConfigParams,
    },
    /// Administrative funding helper required before a buyer can settle.
    Shield {
        config: ConfigParams,
        operation_id: String,
        amount: String,
    },
    /// Unspent note denominations for payment pricing.
    Balance {
        config: ConfigParams,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigParams {
    rpc_url: String,
    prover_url: String,
    pool_address: String,
    chain_id: String,
    account_address: String,
    pool_key_file: PathBuf,
    account_key_file: PathBuf,
    state_dir: PathBuf,
    token: String,
    #[serde(default = "default_wire_version")]
    wire_version: WireVersion,
}

fn default_wire_version() -> WireVersion {
    WireVersion::V3
}

impl ConfigParams {
    fn build(self) -> Result<Client, CliError> {
        if self.wire_version == WireVersion::V1 {
            return Err(CliError::BadValue {
                field: "wire_version",
                value: "v1 is read-only; select v2 or v3".to_owned(),
            });
        }
        Client::new(ClientConfig {
            rpc_url: self.rpc_url,
            prover_url: self.prover_url,
            pool_address: felt("pool_address", &self.pool_address)?,
            chain_id: felt("chain_id", &self.chain_id)?,
            account_address: felt("account_address", &self.account_address)?,
            pool_key_file: self.pool_key_file,
            account_key_file: self.account_key_file,
            state_dir: self.state_dir,
            token: felt("token", &self.token)?,
            new_channel_wire_version: self.wire_version,
        })
        .map_err(CliError::Client)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TermsParams {
    amount: String,
    token: String,
    deadline: u64,
    memo_hash: String,
}

impl TermsParams {
    fn parse(self) -> Result<OfferTerms, CliError> {
        Ok(OfferTerms {
            amount: u128_value("amount", &self.amount)?,
            token: felt("token", &self.token)?,
            deadline: self.deadline,
            memo_hash: memo_hash_value("memo_hash", &self.memo_hash)?,
        })
    }
}

/// Version of the request/response contract this binary speaks. Carried on every envelope
/// so a consumer can fail with a named mismatch instead of a shape error deep inside its
/// own decoding — a stale server against a newer binary surfaced exactly that way on
/// 2026-08-19. Bump on any change to a request or result shape, not only breaking ones.
const PROTOCOL: u8 = 4;

#[derive(Debug, Serialize)]
struct Response {
    ok: bool,
    protocol: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ErrorBody>,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    code: &'static str,
    message: String,
    retryable: bool,
}

impl Response {
    fn ok(value: impl Serialize) -> Self {
        match serde_json::to_value(value) {
            Ok(result) => Self {
                ok: true,
                protocol: PROTOCOL,
                result: Some(result),
                error: None,
            },
            Err(error) => Self::err("PROOF_FAILED", error.to_string(), false),
        }
    }

    fn err(code: &'static str, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            ok: false,
            protocol: PROTOCOL,
            result: None,
            error: Some(ErrorBody {
                code,
                message: message.into(),
                retryable,
            }),
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error("request is not valid JSON: {0}")]
    BadRequest(String),
    /// The result computed fine and could not be encoded. Labeled distinctly from
    /// `BadRequest`: reporting an output failure as "request is not valid JSON" once sent
    /// a debugging session through the request parser when the request was blameless.
    #[error("response failed to serialize: {0}")]
    BadResponse(String),
    #[error("field {field} is invalid: {value}")]
    BadValue { field: &'static str, value: String },
    #[error(transparent)]
    State(#[from] StateError),
    #[error(transparent)]
    KeyFile(#[from] KeyFileError),
    #[error(transparent)]
    Client(#[from] ClientError),
}

impl CliError {
    fn to_response(&self) -> Response {
        match self {
            Self::BadRequest(_) | Self::BadValue { .. } | Self::State(_) => {
                Response::err("INVALID_REQUEST", self.to_string(), false)
            }
            Self::BadResponse(_) => Response::err("INTERNAL", self.to_string(), false),
            Self::KeyFile(_) => Response::err("IDENTITY_UNAVAILABLE", self.to_string(), false),
            Self::Client(error) => client_error_response(error),
        }
    }
}

fn operation_id(value: String) -> Result<OperationId, CliError> {
    OperationId::parse(&value).map_err(|_| CliError::BadValue {
        field: "operation_id",
        value,
    })
}

async fn dispatch(request: Request) -> Result<serde_json::Value, CliError> {
    match request {
        Request::Version => Ok(serde_json::json!({
            "name": env!("CARGO_PKG_NAME"),
            "version": env!("CARGO_PKG_VERSION"),
            "protocol": PROTOCOL,
            "default_wire_version": "v3",
        })),
        Request::GeneratePoolKey { path } => {
            let generated = generate_pool_key_file(path)?;
            Ok(serde_json::json!({
                "pool_key_file": generated.path,
                "public_key": format!("{:#x}", generated.public_key),
            }))
        }
        Request::OpenChannel {
            config,
            operation_id: id,
            counterparty,
        } => {
            let client = config.build()?;
            let handle = client
                .open_channel(&operation_id(id)?, felt("counterparty", &counterparty)?)
                .await?;
            serialize(serde_json::json!({ "channel_handle": handle }))
        }
        Request::ProposeOffer {
            config,
            operation_id: id,
            handle,
            terms,
        } => {
            let client = config.build()?;
            let offer_id = client
                .propose_offer(
                    &operation_id(id)?,
                    ChannelHandle::parse(handle)?,
                    terms.parse()?,
                )
                .await?;
            serialize(serde_json::json!({ "offer_id": offer_id }))
        }
        Request::CounterOffer {
            config,
            operation_id: id,
            handle,
            reply_to,
            terms,
        } => {
            let client = config.build()?;
            let offer_id = client
                .counter_offer(
                    &operation_id(id)?,
                    ChannelHandle::parse(handle)?,
                    offer_id(reply_to),
                    terms.parse()?,
                )
                .await?;
            serialize(serde_json::json!({ "offer_id": offer_id }))
        }
        Request::ReadChannelState { config, handle } => {
            let client = config.build()?;
            let state = client
                .read_channel_state(ChannelHandle::parse(handle)?)
                .await?;
            serialize(state)
        }
        Request::Reconcile { config } => {
            let client = config.build()?;
            serialize(client.reconcile().await?)
        }
        Request::RebuildState { config } => {
            let client = config.build()?;
            serialize(client.rebuild_state().await?)
        }
        Request::ResumeOperation {
            config,
            operation_id: id,
        } => {
            let client = config.build()?;
            let id = operation_id(id)?;
            serialize(client.resume_operation(&id).await?)
        }
        Request::AcceptAndSettle {
            config,
            operation_id: operation,
            handle,
            offer_id: id,
        } => {
            let client = config.build()?;
            serialize(
                client
                    .accept_and_settle(
                        &operation_id(operation)?,
                        ChannelHandle::parse(handle)?,
                        offer_id(id),
                    )
                    .await?,
            )
        }
        Request::GrantViewingKey {
            config,
            handle,
            deal_id,
            grantee,
            expires_at,
        } => {
            let deal_id = u64_value("deal_id", &deal_id)?;
            let client = config.build()?;
            serialize(
                client
                    .grant_viewing_key(
                        ChannelHandle::parse(handle)?,
                        deal_id,
                        felt("grantee", &grantee)?,
                        expires_at,
                    )
                    .await?,
            )
        }
        Request::Reveal {
            config,
            viewing_key,
        } => {
            let client = config.build()?;
            serialize(client.reveal(*viewing_key).await?)
        }
        Request::Approve {
            config,
            operation_id: id,
            amount,
        } => {
            let client = config.build()?;
            serialize(
                client
                    .approve_pool(&operation_id(id)?, u128_value("amount", &amount)?)
                    .await?,
            )
        }
        Request::Allowance { config } => {
            let client = config.build()?;
            serialize(client.pool_allowance().await?)
        }
        Request::Doctor { config } => {
            let client = config.build()?;
            let report = client.doctor().await;
            // A report of faults is a successful inspection. `ok:false` is reserved for the
            // inspection itself failing, so a caller can tell "I looked and found problems"
            // apart from "I could not look".
            serialize(serde_json::json!({
                "ready": report.ready(),
                "checks": report.checks,
                "repairs": report.repairs(),
            }))
        }
        Request::Shield {
            config,
            operation_id: id,
            amount,
        } => {
            let client = config.build()?;
            serialize(
                client
                    .shield(&operation_id(id)?, u128_value("amount", &amount)?)
                    .await?,
            )
        }
        Request::Balance { config } => {
            let client = config.build()?;
            serialize(client.note_balance().await?)
        }
    }
}

fn serialize(value: impl Serialize) -> Result<serde_json::Value, CliError> {
    serde_json::to_value(value).map_err(|error| CliError::BadResponse(error.to_string()))
}

fn offer_id(value: String) -> OfferId {
    serde_json::from_value(serde_json::Value::String(value))
        .expect("transparent string deserializes as OfferId")
}

fn felt(field: &'static str, value: &str) -> Result<Felt, CliError> {
    Felt::from_hex(value).map_err(|_| CliError::BadValue {
        field,
        value: value.to_owned(),
    })
}

/// Parses a memo hash of any width and truncates it to the 128 bits the wire carries.
///
/// Accepts a whole digest as hex, `0x` optional, of any length. A caller passing a SHA-256
/// digest gets the same 128 bits `truncate_memo_hash` would produce, without having to know
/// that rule or which end to cut. Decimal is still accepted up to `u128` so existing callers
/// keep working.
///
/// Truncation is silent because it is the wire's documented behaviour rather than an error:
/// the value that reaches the chain is always 128 bits. Results report what was committed,
/// so a caller can compare.
fn memo_hash_value(field: &'static str, value: &str) -> Result<u128, CliError> {
    let bad = || CliError::BadValue {
        field,
        value: value.to_owned(),
    };
    let Some(hex) = value.strip_prefix("0x") else {
        // No prefix: decimal, as before. A bare hex digest without `0x` is ambiguous with
        // decimal, so it is not guessed at.
        return value.parse().map_err(|_| bad());
    };
    if hex.is_empty() || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(bad());
    }
    // Only the low 128 bits survive, so take the last 32 hex digits and let the rest go.
    let low = &hex[hex.len().saturating_sub(32)..];
    u128::from_str_radix(low, 16).map_err(|_| bad())
}

fn u128_value(field: &'static str, value: &str) -> Result<u128, CliError> {
    let parsed = if let Some(hex) = value.strip_prefix("0x") {
        u128::from_str_radix(hex, 16)
    } else {
        value.parse()
    };
    parsed.map_err(|_| CliError::BadValue {
        field,
        value: value.to_owned(),
    })
}

fn u64_value(field: &'static str, value: &str) -> Result<u64, CliError> {
    let parsed = if let Some(hex) = value.strip_prefix("0x") {
        u64::from_str_radix(hex, 16)
    } else {
        value.parse()
    };
    parsed.map_err(|_| CliError::BadValue {
        field,
        value: value.to_owned(),
    })
}

fn client_error_response(error: &ClientError) -> Response {
    let (code, retryable) = match error {
        ClientError::InvalidRequest(_)
        | ClientError::TokenMismatch { .. }
        | ClientError::InvalidOfferId(_)
        | ClientError::AmbiguousReverseChannel(_)
        | ClientError::Protocol(_)
        | ClientError::Erc20(_)
        | ClientError::DiscoveryLimit(_)
        | ClientError::Disclosure(_) => ("INVALID_REQUEST", false),
        ClientError::KeyFile { .. }
        | ClientError::InvalidKey { .. }
        | ClientError::IdentityMismatch { .. } => ("IDENTITY_UNAVAILABLE", false),
        ClientError::CounterpartyUnregistered(_) | ClientError::ChannelNotReady => {
            ("SUBMIT_FAILED", true)
        }
        ClientError::OperationConflict { .. } => ("OPERATION_CONFLICT", false),
        ClientError::OperationInProgress { .. } | ClientError::RecoveryRequired { .. } => {
            ("RECONCILIATION_REQUIRED", false)
        }
        // Not retryable by the caller: a journal that cannot be read cannot rule out a
        // pending transaction, so a blind retry is exactly the wrong move.
        ClientError::Journal(_) => ("RECONCILIATION_REQUIRED", false),
        ClientError::NotCounterpartyOffer => ("NOT_YOUR_OFFER", false),
        ClientError::AlreadySettled => ("ALREADY_SETTLED", false),
        ClientError::InsufficientNotes { .. } => ("INSUFFICIENT_NOTES", false),
        // Not retryable: both need an operator to approve or fund before the same call can
        // succeed, and an automatic retry would just burn RPC reads reaching the same answer.
        ClientError::InsufficientAllowance { .. } => ("INSUFFICIENT_ALLOWANCE", false),
        ClientError::InsufficientPublicBalance { .. } => ("INSUFFICIENT_BALANCE", false),
        ClientError::ClockBeforeEpoch => ("INVALID_REQUEST", false),
        ClientError::State(StateError::NotFound(_)) => ("INVALID_REQUEST", false),
        ClientError::State(_) | ClientError::Rpc(_) => ("SUBMIT_FAILED", true),
        ClientError::Prover(inner) => prover_error_code(inner),
        ClientError::Channel(inner) => channel_error_code(inner),
        ClientError::Execution(inner) => execution_error_code(inner),
        ClientError::Decrypt(_) | ClientError::Read(_) => ("PROOF_FAILED", false),
        ClientError::Negotiation(inner) => negotiation_error_code(*inner),
    };
    Response::err(code, error.to_string(), retryable)
}

fn prover_error_code(error: &ProverError) -> (&'static str, bool) {
    if error.is_screening_rejection() {
        return ("SCREENING_REJECTED", false);
    }
    match error {
        ProverError::Transport(_) | ProverError::RetriesExhausted(_) => {
            ("PROVER_UNAVAILABLE", true)
        }
        ProverError::Rpc { code: -32005, .. } => ("PROVER_UNAVAILABLE", true),
        ProverError::Rpc { .. } | ProverError::Malformed(_) => ("PROOF_FAILED", false),
    }
}

fn execution_error_code(error: &ExecutionError) -> (&'static str, bool) {
    match error {
        ExecutionError::Prover(error) => prover_error_code(error),
        ExecutionError::Rpc(_)
        | ExecutionError::ReceiptTimeout { .. }
        | ExecutionError::MaturityTimeout { .. } => ("SUBMIT_FAILED", true),
        ExecutionError::ProofExpired { .. } => ("PROOF_EXPIRED", true),
        ExecutionError::Reverted(_) => ("SUBMIT_FAILED", false),
        // Both leave the operation in a state only an operator can resolve: the journal
        // either could not record what was about to happen, or could not record what did.
        ExecutionError::Journal(_)
        | ExecutionError::TransactionNotSerializable(_)
        | ExecutionError::TransactionHashMismatch { .. } => ("RECONCILIATION_REQUIRED", false),
        // A signer failure is the key file, a declining device, or a wallet the user
        // dismissed. Not retryable by the SDK: something outside it has to change.
        ExecutionError::Signer(_) => ("IDENTITY_UNAVAILABLE", false),
        ExecutionError::PoolInvocation(_)
        | ExecutionError::Signing(_)
        | ExecutionError::Calldata(_)
        | ExecutionError::MissingPoolMessage
        | ExecutionError::AmbiguousPoolMessage
        | ExecutionError::EmptyPoolMessage
        | ExecutionError::InvalidProverFelt { .. }
        | ExecutionError::SimulationMismatch { .. } => ("PROOF_FAILED", false),
    }
}

fn negotiation_error_code(error: NegotiationError) -> (&'static str, bool) {
    match error {
        NegotiationError::UnknownOffer { .. } => ("OFFER_UNKNOWN", false),
        NegotiationError::Expired { .. } => ("OFFER_EXPIRED", false),
        NegotiationError::OwnOffer { .. } | NegotiationError::NotAnOffer { .. } => {
            ("NOT_YOUR_OFFER", false)
        }
        NegotiationError::AlreadySettled { .. } => ("ALREADY_SETTLED", false),
        NegotiationError::DanglingReply { .. } | NegotiationError::CrossDealReply { .. } => {
            ("PROOF_FAILED", false)
        }
    }
}

fn channel_error_code(error: &ChannelError) -> (&'static str, bool) {
    match error {
        ChannelError::NotAnAcceptance(_) => ("NOT_YOUR_OFFER", false),
        ChannelError::ZeroPayment
        | ChannelError::ZeroDeposit
        | ChannelError::AmountMismatch { .. } => ("AMOUNT_MISMATCH", false),
        ChannelError::NothingToSpend => ("INSUFFICIENT_NOTES", false),
        ChannelError::IndexCollision { .. }
        | ChannelError::OutputIndexCollision { .. }
        | ChannelError::Index(
            IndexError::NotSequential { .. }
            | IndexError::AlreadyWritten { .. }
            | IndexError::Misaligned { .. }
            | IndexError::Exhausted { .. },
        ) => ("INDEX_CONFLICT", false),
        ChannelError::Wire(WireError::FieldTooWide { .. })
        | ChannelError::Wire(_)
        | ChannelError::ZeroChange
        | ChannelError::MissingV3Change
        | ChannelError::V3DisclosureRequiresDealScope
        | ChannelError::ActionSet(_) => ("INVALID_REQUEST", false),
    }
}

#[tokio::main]
async fn main() {
    let mut raw = String::new();
    let response = match std::io::stdin().read_to_string(&mut raw) {
        Ok(_) => match serde_json::from_str(&raw) {
            Ok(request) => match dispatch(request).await {
                Ok(result) => Response::ok(result),
                Err(error) => error.to_response(),
            },
            Err(error) => CliError::BadRequest(error.to_string()).to_response(),
        },
        Err(error) => Response::err("INVALID_REQUEST", error.to_string(), false),
    };
    let failed = !response.ok;
    println!(
        "{}",
        serde_json::to_string(&response).expect("response envelope serializes")
    );
    if failed {
        std::process::exit(1);
    }
}
