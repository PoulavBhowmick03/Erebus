//! JSON-RPC client for the proving service.
//!
//! ## Private data
//!
//! The invocation handed to `starknet_proveTransaction` carries the pool private key in
//! plaintext at `calldata[5]` (`tests/proof_invocation.rs`). A prover operator can decrypt
//! everything protected by that key. See friction.md F14 and ARCHITECTURE §5.
//! The `compile_actions` preflight exposes the same key to its Starknet RPC endpoint, so
//! both endpoints must be inside the operator trust boundary.
//!
//! ## Retries
//!
//! The client retries transport failures, HTTP 503, and the service's `-32005` busy response.
//! It uses capped exponential backoff. It does not retry transaction or screening
//! rejections.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::tx::SignedInvokeV3;

/// Default timeout for one proof request.
///
/// Server-class runs can finish much sooner, but a memory-constrained one-thread prover can
/// exceed three minutes while remaining healthy. Keep the client alive long enough for that
/// supported local configuration instead of abandoning a proof the server will still finish.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);
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
    #[error("proving service returned error {code}: {message}{}", .data.as_ref().and_then(public_diagnostic).map(|d| format!(": prover diagnostic {d}")).unwrap_or_default())]
    Rpc {
        /// JSON-RPC error code. `10000` is the screening interceptor's "transaction
        /// rejected". See friction.md F6.
        code: i64,
        /// Human-readable message.
        message: String,
        /// Optional structured diagnostic returned by the prover.
        data: Option<serde_json::Value>,
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
    /// The prover reports a blocked deposit only as JSON-RPC code `10000`.
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
/// The pool message contains `[class_hash, ...serialized_server_actions]`. The caller
/// removes the class hash before `apply_actions`.
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
/// Present for an allowed deposit. The contract binds it to the proven `from_addr` and
/// requires an age of less than 300 s.
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
    /// Optional screening signature for deposits.
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
    /// The endpoint receives the pool private key. See the module docs.
    pub fn new(base_url: impl Into<String>) -> Result<Self, ProverError> {
        let client = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()?;
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
    /// Also provides a health check against the deployed pool's expected prover tag.
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
            match self
                .call_once("starknet_proveTransaction", params.clone())
                .await
            {
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
        let response = self
            .client
            .post(&self.base_url)
            .json(&body)
            .send()
            .await?
            .error_for_status()?;
        let status = response.status();
        let value: serde_json::Value = response.json().await?;

        if let Some(error) = value.get("error") {
            return Err(ProverError::Rpc {
                code: error
                    .get("code")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0),
                message: error
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("<no message>")
                    .to_owned(),
                data: error.get("data").cloned(),
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

/// Returns true for transport failures, HTTP 503, and the prover busy code.
fn is_transient(error: &ProverError) -> bool {
    match error {
        ProverError::Transport(e) => {
            e.is_timeout()
                || e.is_connect()
                || e.status() == Some(reqwest::StatusCode::SERVICE_UNAVAILABLE)
        }
        ProverError::Rpc { code: -32005, .. } => true,
        ProverError::Rpc { .. } | ProverError::Malformed(_) | ProverError::RetriesExhausted(_) => {
            false
        }
    }
}

fn truncate(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        text.to_owned()
    } else {
        format!("{}…", &text[..limit])
    }
}

fn public_diagnostic(value: &serde_json::Value) -> Option<&'static str> {
    let text = value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string());
    // Prover diagnostics can contain the virtual invocation, including the pool private
    // key. Emit only reviewed labels and retain the full value for local classification.
    [
        "INVALID_SIGNATURE",
        "ENTRYPOINT_NOT_FOUND",
        "INVALID_TRANSACTION_NONCE",
    ]
    .into_iter()
    .find(|label| text.contains(label))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_service_busy_rpc_errors_are_transient() {
        assert!(is_transient(&ProverError::Rpc {
            code: -32005,
            message: "busy".to_owned(),
            data: None,
        }));
        assert!(!is_transient(&ProverError::Rpc {
            code: 10000,
            message: "screening rejected".to_owned(),
            data: None,
        }));
    }

    #[test]
    fn rpc_display_exposes_a_known_label_but_never_echoes_diagnostic_data() {
        let error = ProverError::Rpc {
            code: -32603,
            message: "Internal error".to_owned(),
            data: Some(serde_json::json!({
                "trace": "pool-private-key=secret INVALID_SIGNATURE"
            })),
        };
        let shown = error.to_string();
        assert!(shown.contains("INVALID_SIGNATURE"));
        assert!(!shown.contains("pool-private-key"));
        assert!(!shown.contains("secret"));
    }

    #[test]
    fn rpc_display_omits_unreviewed_diagnostic_data() {
        let error = ProverError::Rpc {
            code: -32603,
            message: "Internal error".to_owned(),
            data: Some(serde_json::json!({"calldata": ["secret"]})),
        };
        let shown = error.to_string();
        assert_eq!(
            shown,
            "proving service returned error -32603: Internal error"
        );
    }
}
