//! `erebus-cli` — the Python ↔ Rust seam.
//!
//! One JSON request on stdin, one JSON response on stdout, one exit. Chosen over PyO3 on
//! 2026-07-30; the reasoning is in ARCHITECTURE §3 and the short version is that an OS
//! process boundary keeps CLAUDE.md constraint 6 *structural* rather than aspirational.
//!
//! ## Key material never enters the Python process
//!
//! This is the point of the whole design, so it is worth being precise. The request carries
//! a **path** to a key file, never a key. This binary opens it, uses it, and exits. The
//! agent's Python heap — where arbitrary framework and model-driven code runs — never holds
//! the pool private key at all, not even briefly in transit.
//!
//! The key is also not accepted on argv, which would publish it to every process on the box
//! via `/proc/<pid>/cmdline`, nor on stdin alongside the request, which would put it back in
//! the caller's heap.
//!
//! ## Entropy is generated here
//!
//! Channel randoms and note salts come from the OS via `rand`, inside this binary. If the
//! Python side supplied them, `sdk/py` would be making a cryptographic decision — the one
//! thing it must never do, because a second place that can get a salt wrong is a second
//! place a silent failure can hide.
//!
//! ## Errors are a contract, not a stringification
//!
//! Every failure maps to a `SettlementErrorCode` from ARCHITECTURE §4 and carries a
//! `retryable` flag. The agent layer branches on that flag and nothing else; the code is for
//! logs and for humans. Mapping here rather than in Python is what keeps the codes from
//! degrading into opaque strings on the way up.

#![forbid(unsafe_code)]

use std::io::Read;

use erebus_sdk::actions::FeltEntropy;
use erebus_sdk::channel::{Channel, ChannelError, Counterparty, PoolIdentity, SetupParams};
use erebus_sdk::subchannel::IndexError;
use erebus_sdk::wire::WireError;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use starknet_types_core::felt::Felt;

/// A request from the Python binding.
#[derive(Debug, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
enum Request {
    /// Liveness and version check. Takes nothing, touches no key material.
    Version,
    /// Derive a channel to a counterparty and build the setup action set.
    OpenChannel(OpenChannelParams),
}

#[derive(Debug, Deserialize)]
struct OpenChannelParams {
    /// Our pool address.
    address: String,
    /// Path to a file holding our pool private key. Never the key itself.
    key_file: String,
    /// The counterparty's pool address.
    counterparty_address: String,
    /// The counterparty's pool public key.
    counterparty_public_key: String,
    /// ERC-20 address this channel settles in.
    token: String,
    /// Channel index. Contiguous per sender.
    channel_index: u32,
    /// Subchannel index within the channel.
    subchannel_index: u32,
    /// Whether to include registration. Only true the first time an identity is used.
    register: bool,
}

/// The response envelope. Exactly one of `result` or `error` is present.
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
    /// A `SettlementErrorCode` from ARCHITECTURE §4.
    code: &'static str,
    /// Human-readable detail. For logs, not for branching.
    message: String,
    /// Whether retrying the same call could succeed. This is the only field the agent
    /// layer should branch on.
    retryable: bool,
}

impl Response {
    fn ok(result: serde_json::Value) -> Self {
        Self {
            ok: true,
            result: Some(result),
            error: None,
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

/// Errors this binary raises before reaching protocol code.
#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error("request is not valid JSON: {0}")]
    BadRequest(String),
    #[error("field {field} is not a felt: {value}")]
    BadFelt { field: &'static str, value: String },
    #[error("cannot read key file {path}: {source}")]
    KeyFile {
        path: String,
        source: std::io::Error,
    },
    #[error(transparent)]
    Channel(#[from] ChannelError),
}

impl CliError {
    /// Maps to the frozen `SettlementErrorCode` set. Everything here is a caller mistake
    /// that retrying verbatim cannot fix.
    fn to_response(&self) -> Response {
        let code = match self {
            CliError::BadRequest(_) | CliError::BadFelt { .. } => "INVALID_REQUEST",
            CliError::KeyFile { .. } => "IDENTITY_UNAVAILABLE",
            CliError::Channel(inner) => return channel_error_response(inner),
        };
        Response::err(code, self.to_string(), false)
    }
}

/// Maps SDK errors onto the interface's error codes.
///
/// Deliberately lossy in one direction only: the three index errors collapse to
/// `INDEX_CONFLICT`, because an agent cannot act differently on "not sequential" versus
/// "already written" — both mean the subchannel is not in the state we thought.
fn channel_error_response(error: &ChannelError) -> Response {
    let code = match error {
        ChannelError::NotAnAcceptance(_) => "NOT_YOUR_OFFER",
        ChannelError::ZeroPayment | ChannelError::AmountMismatch { .. } => "AMOUNT_MISMATCH",
        ChannelError::NothingToSpend => "INSUFFICIENT_NOTES",
        ChannelError::IndexCollision { .. } => "INDEX_CONFLICT",
        ChannelError::Index(
            IndexError::NotSequential { .. }
            | IndexError::AlreadyWritten { .. }
            | IndexError::Misaligned { .. }
            | IndexError::Exhausted { .. },
        ) => "INDEX_CONFLICT",
        ChannelError::Wire(WireError::FieldTooWide { .. }) => "INVALID_REQUEST",
        ChannelError::Wire(_) | ChannelError::ActionSet(_) => "INVALID_REQUEST",
    };
    Response::err(code, error.to_string(), false)
}

fn felt(field: &'static str, value: &str) -> Result<Felt, CliError> {
    Felt::from_hex(value).map_err(|_| CliError::BadFelt {
        field,
        value: value.to_string(),
    })
}

/// Non-zero entropy from the OS.
///
/// Retries rather than masking a bit: forcing a bit set would bias the value, and zero is
/// improbable enough that a loop is free.
fn entropy() -> FeltEntropy {
    let mut rng = rand::thread_rng();
    loop {
        let mut bytes = [0u8; 31];
        rng.fill_bytes(&mut bytes);
        if let Ok(value) = FeltEntropy::new(Felt::from_bytes_be_slice(&bytes)) {
            return value;
        }
    }
}

/// Reads the pool private key from disk. It does not travel through the caller.
fn read_key(path: &str) -> Result<Felt, CliError> {
    let raw = std::fs::read_to_string(path).map_err(|source| CliError::KeyFile {
        path: path.to_string(),
        source,
    })?;
    felt("key_file", raw.trim())
}

fn open_channel(params: OpenChannelParams) -> Result<serde_json::Value, CliError> {
    let identity = PoolIdentity::new(
        felt("address", &params.address)?,
        read_key(&params.key_file)?,
    );
    let counterparty = Counterparty {
        address: felt("counterparty_address", &params.counterparty_address)?,
        public_key: felt("counterparty_public_key", &params.counterparty_public_key)?,
    };
    let token = felt("token", &params.token)?;

    let channel = Channel::derive(&identity, counterparty);
    let setup = channel.setup(
        &identity,
        SetupParams {
            register: params.register.then(entropy),
            channel_index: params.channel_index,
            channel_random: entropy(),
            channel_salt: entropy(),
            subchannel_index: params.subchannel_index,
            token,
            subchannel_salt: entropy(),
        },
    )?;

    // The channel key is the handle. It is also the scoped disclosure secret, so it leaves
    // this process only because the caller is the operator who already owns it.
    Ok(serde_json::json!({
        "channel_handle": format!("{:#x}", channel.key()),
        "counterparty": format!("{:#x}", channel.counterparty().address),
        "action_count": setup.actions().len(),
        "registered": params.register,
    }))
}

fn handle(raw: &str) -> Response {
    let request: Request = match serde_json::from_str(raw) {
        Ok(request) => request,
        Err(error) => return CliError::BadRequest(error.to_string()).to_response(),
    };

    let outcome = match request {
        Request::Version => Ok(serde_json::json!({
            "name": env!("CARGO_PKG_NAME"),
            "version": env!("CARGO_PKG_VERSION"),
            "protocol": 1,
        })),
        Request::OpenChannel(params) => open_channel(params),
    };

    match outcome {
        Ok(result) => Response::ok(result),
        Err(error) => error.to_response(),
    }
}

fn main() {
    let mut raw = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut raw) {
        let response = Response::err("INVALID_REQUEST", error.to_string(), false);
        println!("{}", serde_json::to_string(&response).expect("response serializes"));
        std::process::exit(1);
    }

    let response = handle(&raw);
    let failed = !response.ok;
    println!("{}", serde_json::to_string(&response).expect("response serializes"));

    // Exit code mirrors the envelope so a caller can fail fast without parsing, but the
    // envelope is authoritative — the code carries no detail.
    if failed {
        std::process::exit(1);
    }
}
