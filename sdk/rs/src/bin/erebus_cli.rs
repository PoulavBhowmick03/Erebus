//! `erebus-cli` — one JSON request, one JSON response, one process.
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
use erebus_sdk::prover::ProverError;
use erebus_sdk::state::{ChannelHandle, StateError};
use erebus_sdk::subchannel::IndexError;
use erebus_sdk::wire::WireError;
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
        counterparty: String,
    },
    ProposeOffer {
        config: ConfigParams,
        handle: String,
        terms: TermsParams,
    },
    CounterOffer {
        config: ConfigParams,
        handle: String,
        reply_to: String,
        terms: TermsParams,
    },
    ReadChannelState {
        config: ConfigParams,
        handle: String,
    },
    AcceptAndSettle {
        config: ConfigParams,
        handle: String,
        offer_id: String,
    },
    GrantViewingKey {
        config: ConfigParams,
        handle: String,
        grantee: String,
    },
    Reveal {
        config: ConfigParams,
        viewing_key: ViewingKeyGrant,
    },
    /// Administrative funding helper required before a buyer can settle.
    Shield {
        config: ConfigParams,
        amount: String,
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
}

impl ConfigParams {
    fn build(self) -> Result<Client, CliError> {
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
            memo_hash: u128_value("memo_hash", &self.memo_hash)?,
        })
    }
}

#[derive(Debug, Serialize)]
struct Response {
    ok: bool,
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
                result: Some(result),
                error: None,
            },
            Err(error) => Self::err("PROOF_FAILED", error.to_string(), false),
        }
    }

    fn err(code: &'static str, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            ok: false,
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
            Self::KeyFile(_) => Response::err("IDENTITY_UNAVAILABLE", self.to_string(), false),
            Self::Client(error) => client_error_response(error),
        }
    }
}

async fn dispatch(request: Request) -> Result<serde_json::Value, CliError> {
    match request {
        Request::Version => Ok(serde_json::json!({
            "name": env!("CARGO_PKG_NAME"),
            "version": env!("CARGO_PKG_VERSION"),
            "protocol": 2,
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
            counterparty,
        } => {
            let client = config.build()?;
            let handle = client
                .open_channel(felt("counterparty", &counterparty)?)
                .await?;
            serialize(serde_json::json!({ "channel_handle": handle }))
        }
        Request::ProposeOffer {
            config,
            handle,
            terms,
        } => {
            let client = config.build()?;
            let offer_id = client
                .propose_offer(ChannelHandle::parse(handle)?, terms.parse()?)
                .await?;
            serialize(serde_json::json!({ "offer_id": offer_id }))
        }
        Request::CounterOffer {
            config,
            handle,
            reply_to,
            terms,
        } => {
            let client = config.build()?;
            let offer_id = client
                .counter_offer(
                    ChannelHandle::parse(handle)?,
                    offer_id(reply_to),
                    terms.parse()?,
                )
                .await?;
            serialize(serde_json::json!({ "offer_id": offer_id }))
        }
        Request::ReadChannelState { config, handle } => {
            let client = config.build()?;
            serialize(
                client
                    .read_channel_state(ChannelHandle::parse(handle)?)
                    .await?,
            )
        }
        Request::AcceptAndSettle {
            config,
            handle,
            offer_id: id,
        } => {
            let client = config.build()?;
            serialize(
                client
                    .accept_and_settle(ChannelHandle::parse(handle)?, offer_id(id))
                    .await?,
            )
        }
        Request::GrantViewingKey {
            config,
            handle,
            grantee,
        } => {
            let client = config.build()?;
            serialize(
                client
                    .grant_viewing_key(ChannelHandle::parse(handle)?, felt("grantee", &grantee)?)
                    .await?,
            )
        }
        Request::Reveal {
            config,
            viewing_key,
        } => {
            let client = config.build()?;
            serialize(client.reveal(viewing_key).await?)
        }
        Request::Shield { config, amount } => {
            let client = config.build()?;
            serialize(client.shield(u128_value("amount", &amount)?).await?)
        }
    }
}

fn serialize(value: impl Serialize) -> Result<serde_json::Value, CliError> {
    serde_json::to_value(value).map_err(|error| CliError::BadRequest(error.to_string()))
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

fn client_error_response(error: &ClientError) -> Response {
    let (code, retryable) = match error {
        ClientError::InvalidRequest(_)
        | ClientError::TokenMismatch { .. }
        | ClientError::InvalidOfferId(_)
        | ClientError::AmbiguousReverseChannel(_)
        | ClientError::Protocol(_)
        | ClientError::DiscoveryLimit(_) => ("INVALID_REQUEST", false),
        ClientError::KeyFile { .. }
        | ClientError::InvalidKey { .. }
        | ClientError::IdentityMismatch { .. } => ("IDENTITY_UNAVAILABLE", false),
        ClientError::CounterpartyUnregistered(_) | ClientError::ChannelNotReady => {
            ("SUBMIT_FAILED", true)
        }
        ClientError::NotCounterpartyOffer => ("NOT_YOUR_OFFER", false),
        ClientError::AlreadySettled => ("ALREADY_SETTLED", false),
        ClientError::InsufficientNotes { .. } => ("INSUFFICIENT_NOTES", false),
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
        NegotiationError::DanglingReply { .. } => ("PROOF_FAILED", false),
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
        | ChannelError::Index(
            IndexError::NotSequential { .. }
            | IndexError::AlreadyWritten { .. }
            | IndexError::Misaligned { .. }
            | IndexError::Exhausted { .. },
        ) => ("INDEX_CONFLICT", false),
        ChannelError::Wire(WireError::FieldTooWide { .. })
        | ChannelError::Wire(_)
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
