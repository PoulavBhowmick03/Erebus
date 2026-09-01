//! Minimal Starknet JSON-RPC client for the privacy-pool path.
//!
//! This module contains only the calls that the Rust SDK needs. A full account SDK would
//! not support the custom `proof_facts` hash. A second transaction model can also diverge
//! between signing and submission.
//!
//! Write-path warning: `starknet_call(compile_actions)` includes the pool private key in its
//! calldata. Use an operator-controlled endpoint. Discovery-only calls do not carry it.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use starknet_types_core::felt::Felt;

use crate::prover::BlockId;
use crate::tx::{ResourceBound, ResourceBounds, SignedInvokeV3, QUERY_VERSION_3};

/// A Starknet JSON-RPC endpoint.
#[derive(Clone)]
pub struct StarknetRpc {
    url: String,
    client: reqwest::Client,
}

impl core::fmt::Debug for StarknetRpc {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("StarknetRpc")
            .field("endpoint", &public_endpoint(&self.url))
            .finish_non_exhaustive()
    }
}

impl StarknetRpc {
    /// Creates a client.
    pub fn new(url: impl Into<String>) -> Result<Self, RpcError> {
        Ok(Self {
            url: url.into(),
            client: reqwest::Client::builder().build()?,
        })
    }

    /// Current accepted block number.
    pub async fn block_number(&self) -> Result<u64, RpcError> {
        let value = self.call("starknet_blockNumber", json!([])).await?;
        value
            .as_u64()
            .ok_or_else(|| RpcError::Malformed("block number was not a u64".to_owned()))
    }

    /// Unix timestamp assigned to an accepted block by Starknet.
    pub async fn block_timestamp(&self, block_number: u64) -> Result<u64, RpcError> {
        let value = self
            .call(
                "starknet_getBlockWithTxHashes",
                json!({ "block_id": { "block_number": block_number } }),
            )
            .await?;
        value
            .get("timestamp")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                RpcError::Malformed(format!(
                    "block {block_number} timestamp was missing or not a u64"
                ))
            })
    }

    /// The chain id this endpoint serves.
    ///
    /// Worth reading rather than trusting configuration: pointing a Sepolia key file at a
    /// mainnet RPC produces valid-looking derivations against the wrong chain, and every
    /// resulting failure is a "not found" rather than an error.
    pub async fn chain_id(&self) -> Result<Felt, RpcError> {
        let value = self.call("starknet_chainId", json!([])).await?;
        parse_felt("chain_id", &value)
    }

    /// Contract nonce at `block_id`.
    pub async fn nonce(&self, address: Felt, block_id: &BlockId) -> Result<Felt, RpcError> {
        let value = self
            .call(
                "starknet_getNonce",
                json!({
                    "contract_address": hex(address),
                    "block_id": block_param(block_id),
                }),
            )
            .await?;
        parse_felt("nonce", &value)
    }

    /// Executes a view call.
    pub async fn call_contract(
        &self,
        contract_address: Felt,
        entrypoint: &str,
        calldata: &[Felt],
        block_id: &BlockId,
    ) -> Result<Vec<Felt>, RpcError> {
        let value = self
            .call(
                "starknet_call",
                json!({
                    "request": {
                        "contract_address": hex(contract_address),
                        "entry_point_selector": hex(crate::calldata::selector(entrypoint)),
                        "calldata": calldata.iter().copied().map(hex).collect::<Vec<_>>(),
                    },
                    "block_id": block_param(block_id),
                }),
            )
            .await?;
        parse_felt_array("call result", &value)
    }

    /// Estimates the final proof-carrying invoke and returns buffered resource bounds.
    ///
    /// The unsigned query skips validation. It still executes `apply_actions`, verifies the
    /// proof, and runs the submitted state transition.
    pub async fn estimate_bounds(
        &self,
        transaction: &SignedInvokeV3,
        block_id: &BlockId,
    ) -> Result<ResourceBounds, RpcError> {
        let value = self
            .call(
                "starknet_estimateFee",
                json!({
                    "request": [transaction.to_wire_with_version(QUERY_VERSION_3)],
                    "simulation_flags": ["SKIP_VALIDATE"],
                    "block_id": block_param(block_id),
                }),
            )
            .await?;
        let estimates = value
            .as_array()
            .ok_or_else(|| RpcError::Malformed("fee estimate was not an array".to_owned()))?;
        let estimate = estimates
            .first()
            .ok_or_else(|| RpcError::Malformed("fee estimate array was empty".to_owned()))?;

        Ok(ResourceBounds {
            l1_gas: estimate_bound(estimate, "l1_gas_consumed", "l1_gas_price")?,
            l2_gas: estimate_bound(estimate, "l2_gas_consumed", "l2_gas_price")?,
            l1_data_gas: estimate_bound(estimate, "l1_data_gas_consumed", "l1_data_gas_price")?,
        })
    }

    /// Submits an invoke transaction and returns its transaction hash.
    pub async fn add_invoke_transaction(
        &self,
        transaction: &SignedInvokeV3,
    ) -> Result<Felt, RpcError> {
        let value = self
            .call(
                "starknet_addInvokeTransaction",
                json!({ "invoke_transaction": transaction.to_wire() }),
            )
            .await?;
        parse_felt(
            "transaction_hash",
            value.get("transaction_hash").ok_or_else(|| {
                RpcError::Malformed("submission omitted transaction_hash".to_owned())
            })?,
        )
    }

    /// Resubmits a transaction exactly as it was recorded.
    ///
    /// The parameter is the stored wire JSON, forwarded without being parsed into our own
    /// types and re-serialized. Round-tripping it could change a field encoding, which would
    /// change the hash, and the only reason resubmission is safe is that the hash does not
    /// change: a duplicate of a transaction the chain already has is a no-op, whereas a
    /// transaction that differs by one byte is a second transaction that can land alongside
    /// the first.
    pub async fn resubmit_invoke_transaction(
        &self,
        wire_transaction: &Value,
    ) -> Result<Felt, RpcError> {
        let value = self
            .call(
                "starknet_addInvokeTransaction",
                json!({ "invoke_transaction": wire_transaction }),
            )
            .await?;
        parse_felt(
            "transaction_hash",
            value.get("transaction_hash").ok_or_else(|| {
                RpcError::Malformed("submission omitted transaction_hash".to_owned())
            })?,
        )
    }

    /// Reads a transaction receipt.
    pub async fn transaction_receipt(&self, transaction_hash: Felt) -> Result<Receipt, RpcError> {
        let value = self
            .call(
                "starknet_getTransactionReceipt",
                json!({ "transaction_hash": hex(transaction_hash) }),
            )
            .await?;
        serde_json::from_value(value)
            .map_err(|error| RpcError::Malformed(format!("invalid transaction receipt: {error}")))
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });
        let response = self.client.post(&self.url).json(&body).send().await?;
        let status = response.status();
        let value: Value = response.json().await?;
        if let Some(error) = value.get("error") {
            return Err(RpcError::Rpc {
                code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("<no message>")
                    .to_owned(),
                data: error.get("data").cloned(),
            });
        }
        value
            .get("result")
            .cloned()
            .ok_or_else(|| RpcError::Malformed(format!("HTTP {status}, no result field")))
    }
}

/// Receipt fields the client needs to decide whether a transaction landed.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Receipt {
    /// Transaction hash.
    pub transaction_hash: String,
    /// Block number once accepted.
    #[serde(default)]
    pub block_number: Option<u64>,
    /// `RECEIVED`, `PRE_CONFIRMED`, `ACCEPTED_ON_L2`, or `ACCEPTED_ON_L1`.
    #[serde(default)]
    pub finality_status: Option<String>,
    /// `SUCCEEDED` or `REVERTED`.
    #[serde(default)]
    pub execution_status: Option<String>,
    /// Revert reason when execution failed.
    #[serde(default)]
    pub revert_reason: Option<String>,
}

impl Receipt {
    /// Whether the transaction has executed successfully and reached an accepted block.
    pub fn is_accepted(&self) -> bool {
        self.execution_status.as_deref() == Some("SUCCEEDED")
            && matches!(
                self.finality_status.as_deref(),
                Some("ACCEPTED_ON_L2" | "ACCEPTED_ON_L1")
            )
    }

    /// Whether execution reverted.
    pub fn is_reverted(&self) -> bool {
        self.execution_status.as_deref() == Some("REVERTED")
    }
}

/// Starknet RPC failure.
#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    /// HTTP or JSON transport failure.
    #[error("Starknet RPC transport error (endpoint details redacted)")]
    Transport(#[from] reqwest::Error),
    /// JSON-RPC application error.
    ///
    /// `data` can contain the revert reason for code 40 (`CONTRACT_ERROR`), but some RPCs
    /// also echo the request calldata there. `compile_actions` calldata carries the pool
    /// private key, so display only reviewed diagnostic labels.
    #[error("Starknet RPC error {code}: {message}{}", .data.as_ref().and_then(public_diagnostic).map(|d| format!(": RPC diagnostic {d}")).unwrap_or_default())]
    Rpc {
        /// RPC error code.
        code: i64,
        /// Human-readable message.
        message: String,
        /// Optional structured detail.
        data: Option<Value>,
    },
    /// A successful response had the wrong shape.
    #[error("malformed Starknet RPC response: {0}")]
    Malformed(String),
}

impl RpcError {
    /// `starknet_getTransactionReceipt` uses code 29 while a submitted transaction is not
    /// yet visible to the node.
    pub fn is_transaction_not_found(&self) -> bool {
        matches!(self, Self::Rpc { code: 29, .. })
    }
}

fn public_diagnostic(value: &Value) -> Option<&'static str> {
    let text = value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string());
    // These labels identify the actionable contract failure without forwarding arbitrary
    // RPC data through the CLI and MCP server. Additions require a regression test that the
    // surrounding payload remains redacted.
    [
        "NON_ZERO_VALUE",
        "ENTRYPOINT_FAILED",
        "ENTRYPOINT_NOT_FOUND",
        "INVALID_SIGNATURE",
        "INVALID_TRANSACTION_NONCE",
        "INDEX_NOT_SEQUENTIAL",
    ]
    .into_iter()
    .find(|label| text.contains(label))
}

fn public_endpoint(url: &str) -> String {
    reqwest::Url::parse(url)
        .map(|parsed| parsed.origin().ascii_serialization())
        .unwrap_or_else(|_| "<invalid URL>".to_owned())
}

fn block_param(block_id: &BlockId) -> Value {
    match block_id {
        BlockId::Latest => json!("latest"),
        BlockId::Number(number) => json!({ "block_number": number }),
        BlockId::Hash(hash) => json!({ "block_hash": hash }),
    }
}

fn estimate_bound(
    estimate: &Value,
    amount_field: &'static str,
    price_field: &'static str,
) -> Result<ResourceBound, RpcError> {
    let amount = parse_hex_u64(
        amount_field,
        estimate
            .get(amount_field)
            .ok_or_else(|| RpcError::Malformed(format!("fee estimate omitted {amount_field}")))?,
    )?;
    let price = parse_hex_u128(
        price_field,
        estimate
            .get(price_field)
            .ok_or_else(|| RpcError::Malformed(format!("fee estimate omitted {price_field}")))?,
    )?;

    // A 50% buffer covers changes between estimation and inclusion. Saturating arithmetic
    // prevents a malicious endpoint from wrapping the bound.
    Ok(ResourceBound {
        max_amount: amount.saturating_add(amount / 2).max(1),
        max_price_per_unit: price.saturating_add(price / 2).max(1),
    })
}

fn parse_felt(field: &'static str, value: &Value) -> Result<Felt, RpcError> {
    let text = value
        .as_str()
        .ok_or_else(|| RpcError::Malformed(format!("{field} was not a string")))?;
    Felt::from_hex(text).map_err(|_| RpcError::Malformed(format!("{field} was not a felt")))
}

fn parse_felt_array(field: &'static str, value: &Value) -> Result<Vec<Felt>, RpcError> {
    value
        .as_array()
        .ok_or_else(|| RpcError::Malformed(format!("{field} was not an array")))?
        .iter()
        .map(|item| parse_felt(field, item))
        .collect()
}

fn parse_hex_u64(field: &'static str, value: &Value) -> Result<u64, RpcError> {
    let text = value
        .as_str()
        .ok_or_else(|| RpcError::Malformed(format!("{field} was not a string")))?;
    u64::from_str_radix(text.trim_start_matches("0x"), 16)
        .map_err(|_| RpcError::Malformed(format!("{field} was not a u64")))
}

fn parse_hex_u128(field: &'static str, value: &Value) -> Result<u128, RpcError> {
    let text = value
        .as_str()
        .ok_or_else(|| RpcError::Malformed(format!("{field} was not a string")))?;
    u128::from_str_radix(text.trim_start_matches("0x"), 16)
        .map_err(|_| RpcError::Malformed(format!("{field} was not a u128")))
}

fn hex(value: Felt) -> String {
    format!("{value:#x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fee_bounds_are_buffered() {
        let estimate = json!({
            "l2_gas_consumed": "0x64",
            "l2_gas_price": "0xa",
        });
        assert_eq!(
            estimate_bound(&estimate, "l2_gas_consumed", "l2_gas_price").expect("bound"),
            ResourceBound {
                max_amount: 150,
                max_price_per_unit: 15,
            }
        );
    }

    #[test]
    fn receipt_state_is_explicit() {
        let accepted = Receipt {
            transaction_hash: "0x1".to_owned(),
            block_number: Some(7),
            finality_status: Some("ACCEPTED_ON_L2".to_owned()),
            execution_status: Some("SUCCEEDED".to_owned()),
            revert_reason: None,
        };
        assert!(accepted.is_accepted());
        assert!(!accepted.is_reverted());
    }

    #[test]
    fn rpc_display_keeps_a_reviewed_label_but_redacts_the_payload() {
        let error = RpcError::Rpc {
            code: 40,
            message: "Contract error".to_owned(),
            data: Some(json!({
                "revert_error": "NON_ZERO_VALUE, ENTRYPOINT_FAILED",
                "request": {"calldata": ["pool-private-key=secret"]}
            })),
        };

        let shown = error.to_string();
        assert!(shown.contains("NON_ZERO_VALUE"));
        assert!(!shown.contains("calldata"));
        assert!(!shown.contains("pool-private-key"));
        assert!(!shown.contains("secret"));
    }

    #[test]
    fn rpc_display_omits_unreviewed_data() {
        let error = RpcError::Rpc {
            code: -32603,
            message: "Internal error".to_owned(),
            data: Some(json!({"request": {"calldata": ["secret"]}})),
        };

        assert_eq!(
            error.to_string(),
            "Starknet RPC error -32603: Internal error"
        );
    }

    #[test]
    fn rpc_debug_omits_credentials_in_the_url() {
        let rpc = StarknetRpc::new("https://user:password@example.com/api-key?token=secret")
            .expect("client");
        let shown = format!("{rpc:?}");
        assert!(shown.contains("https://example.com"));
        for secret in ["user", "password", "api-key", "token", "secret"] {
            assert!(!shown.contains(secret));
        }
    }

    #[test]
    fn malformed_values_are_not_echoed() {
        let felt_error =
            parse_felt("nonce", &json!("pool-private-key=secret")).expect_err("invalid felt");
        let array_error = parse_felt_array(
            "call result",
            &json!({"calldata": ["pool-private-key=secret"]}),
        )
        .expect_err("invalid array");

        for shown in [felt_error.to_string(), array_error.to_string()] {
            assert!(!shown.contains("pool-private-key"));
            assert!(!shown.contains("secret"));
        }
    }
}
