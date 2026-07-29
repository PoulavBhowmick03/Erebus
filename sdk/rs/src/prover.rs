//! JSON-RPC client for the proving service.
//!
//! ## What this sends
//!
//! The invocation handed to `starknet_proveTransaction` carries the pool private key in
//! plaintext at `calldata[5]` — verified, not assumed (`tests/proof_invocation.rs`). The
//! prover therefore sits **inside the trust boundary of whoever owns that key**. Pointing
//! this client at a third-party endpoint hands that operator the ability to decrypt
//! everything the key protects. See friction.md F14 and ARCHITECTURE §5.
//!
//! It is deliberately not this module's job to stop you doing that — but it is its job to
//! say so where you would be typing the URL.
//!
//! ## Retries
//!
//! Only on transport failures and HTTP 503, with exponential backoff and a small cap.
//! Proving is expensive and the shared dev endpoint is a courtesy; a tight retry loop
//! against it is antisocial. JSON-RPC application errors are never retried — a rejected
//! transaction is rejected however many times you ask.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::tx::SignedInvokeV3;

/// Default per-request timeout. Proving is ~29 s per transaction (friction.md F7), so this
/// has to be generous enough to cover a real proof plus queueing.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(180);
/// Default retry budget for transient failures.
pub const DEFAULT_MAX_RETRIES: u32 = 3;
/// Base backoff, doubled per attempt.
pub const DEFAULT_BASE_BACKOFF: Duration = Duration::from_millis(500);

/// Errors talking to the proving service.
#[derive(Debug, thiserror::Error)]
pub enum ProverError {
    /// Transport-level failure.
    #[error("proving service transport error: {0}")]
    Transport(#[from] reqwest::Error),
    /// The service returned a JSON-RPC error.
    #[error("proving service returned error {code}: {message}")]
    Rpc {
        /// JSON-RPC error code. `10000` is the screening interceptor's "transaction
        /// rejected" — see friction.md F6.
        code: i64,
        /// Human-readable message.
        message: String,
    },
    /// The response did not match the expected shape.
    #[error("proving service returned an unexpected response: {0}")]
    Malformed(String),
    /// Retries were exhausted on a transient failure.
    #[error("proving service still failing after {0} retries")]
    RetriesExhausted(u32),
}

impl ProverError {
    /// Whether this is the screening interceptor's rejection code.
    ///
    /// The prover surfaces a blocked deposit as JSON-RPC `10000` rather than anything
    /// structured, so this is the only handle on it.
    pub fn is_screening_rejection(&self) -> bool {
        matches!(self, Self::Rpc { code: 10000, .. })
    }
}

/// Which block the proof is built against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockId {
    /// The latest block.
    Latest,
    /// A specific block number.
    Number(u64),
    /// A specific block hash.
    Hash(String),
}

impl BlockId {
    fn to_param(&self) -> serde_json::Value {
        match self {
            Self::Latest => json!("latest"),
            Self::Number(n) => json!({ "block_number": n }),
            Self::Hash(h) => json!({ "block_hash": h }),
        }
    }
}

/// An L2→L1 message emitted during the proven execution.
///
/// The pool's own message carries `[class_hash, ...serialized_server_actions]`; the
/// class-hash prefix is stripped before calling `apply_actions`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct MessageToL1 {
    /// Emitting contract.
    pub from_address: String,
    /// Destination.
    pub to_address: String,
    /// Payload felts.
    pub payload: Vec<String>,
}

/// Screening attestation relayed by the prover for a deposit.
///
/// Present only when the transaction contains a deposit and screening allowed it. The
/// contract verifies it against the proven deposit's `from_addr`, fresh within 300 s.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ScreeningSignature {
    /// Unix seconds.
    pub issued_at: u64,
    /// Signature r.
    pub sig_r: String,
    /// Signature s.
    pub sig_s: String,
}

/// Typed side-channel on a prove response.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct AdditionalData {
    /// Screening attestation, when the transaction deposited.
    #[serde(default)]
    pub signature: Option<ScreeningSignature>,
}

/// Result of `starknet_proveTransaction`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ProveTransactionResult {
    /// Base64-encoded proof blob.
    pub proof: String,
    /// Proof facts, which go into the `apply_actions` transaction.
    pub proof_facts: Vec<String>,
    /// Messages emitted during the proven execution.
    pub l2_to_l1_messages: Vec<MessageToL1>,
    /// Optional side-channel; carries the screening signature for deposits.
    #[serde(default)]
    pub additional_data: Option<AdditionalData>,
}

/// JSON-RPC client for a proving service.
#[derive(Debug, Clone)]
pub struct ProvingService {
    base_url: String,
    client: reqwest::Client,
    max_retries: u32,
    base_backoff: Duration,
}

impl ProvingService {
    /// Creates a client for `base_url`.
    ///
    /// Remember what this URL is trusted with — see the module docs.
    pub fn new(base_url: impl Into<String>) -> Result<Self, ProverError> {
        let client = reqwest::Client::builder().timeout(DEFAULT_TIMEOUT).build()?;
        Ok(Self {
            base_url: base_url.into(),
            client,
            max_retries: DEFAULT_MAX_RETRIES,
            base_backoff: DEFAULT_BASE_BACKOFF,
        })
    }

    /// Overrides the retry budget.
    pub fn with_max_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries;
        self
    }

    /// The JSON-RPC spec version the service speaks.
    ///
    /// Also serves as a health check, and is worth asserting against the deployed pool's
    /// expected prover tag before trusting anything else.
    pub async fn spec_version(&self) -> Result<String, ProverError> {
        let value = self.call_once("starknet_specVersion", json!([])).await?;
        value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| ProverError::Malformed(format!("specVersion was not a string: {value}")))
    }

    /// Proves a transaction.
    pub async fn prove_transaction(
        &self,
        block_id: &BlockId,
        transaction: &SignedInvokeV3,
    ) -> Result<ProveTransactionResult, ProverError> {
        let params = json!({
            "block_id": block_id.to_param(),
            "transaction": transaction.to_wire(),
        });

        let mut attempt = 0;
        loop {
            match self.call_once("starknet_proveTransaction", params.clone()).await {
                Ok(value) => {
                    return serde_json::from_value(value.clone()).map_err(|e| {
                        ProverError::Malformed(format!(
                            "{e}; response was {}",
                            truncate(&value.to_string(), 500)
                        ))
                    });
                }
                Err(error) if is_transient(&error) && attempt < self.max_retries => {
                    tokio::time::sleep(self.base_backoff * 2u32.pow(attempt)).await;
                    attempt += 1;
                }
                Err(ProverError::Transport(e)) if attempt >= self.max_retries && e.is_timeout() => {
                    return Err(ProverError::RetriesExhausted(self.max_retries));
                }
                Err(error) => return Err(error),
            }
        }
    }

    async fn call_once(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, ProverError> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let response = self.client.post(&self.base_url).json(&body).send().await?;
        let status = response.status();
        let value: serde_json::Value = response.json().await?;

        if let Some(error) = value.get("error") {
            return Err(ProverError::Rpc {
                code: error.get("code").and_then(serde_json::Value::as_i64).unwrap_or(0),
                message: error
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("<no message>")
                    .to_owned(),
            });
        }

        value.get("result").cloned().ok_or_else(|| {
            ProverError::Malformed(format!(
                "HTTP {status}, no result field: {}",
                truncate(&value.to_string(), 500)
            ))
        })
    }
}

/// Transport failures and 503 are worth retrying; a rejected transaction is not.
fn is_transient(error: &ProverError) -> bool {
    match error {
        ProverError::Transport(e) => {
            e.is_timeout() || e.is_connect() || e.status() == Some(reqwest::StatusCode::SERVICE_UNAVAILABLE)
        }
        _ => false,
    }
}

fn truncate(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        text.to_owned()
    } else {
        format!("{}…", &text[..limit])
    }
}
