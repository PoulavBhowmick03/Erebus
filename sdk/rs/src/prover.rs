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

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

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
const CACHED_JOB_VERSION: u32 = 2;
const CACHED_JOB_INDEX_VERSION: u32 = 1;

/// Errors talking to the proving service.
#[derive(Debug, thiserror::Error)]
pub enum ProverError {
    /// Transport-level failure.
    #[error("proving service transport error (endpoint details redacted)")]
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
    /// The hosted prover rejected or could not safely complete a job.
    #[error("hosted proving service returned {code}: {message}")]
    Relay {
        /// Stable relay error or terminal-status code.
        code: String,
        /// Public diagnostic safe to show to the operator.
        message: String,
    },
    /// Durable hosted-prover job state could not be read or written.
    #[error("hosted prover job state error: {0}")]
    JobState(String),
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

#[derive(Clone)]
enum Transport {
    JsonRpc,
    Starkscan { api_key: String, job_dir: PathBuf },
}

impl core::fmt::Debug for Transport {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::JsonRpc => formatter.write_str("JsonRpc"),
            Self::Starkscan { job_dir, .. } => formatter
                .debug_struct("Starkscan")
                .field("api_key", &"<redacted>")
                .field("job_dir", job_dir)
                .finish(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedJob {
    version: u32,
    request_hash: String,
    /// Exact private request retained only until the relay job ID is durable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    request: Option<serde_json::Value>,
    #[serde(default)]
    job_id: Option<String>,
    #[serde(default)]
    result: Option<ProveTransactionResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedJobIndex {
    version: u32,
    request_hash: String,
    relay_key: String,
    cache_file: String,
}

/// Client for a local JSON-RPC prover or Starkscan's asynchronous hosted prover.
#[derive(Clone)]
pub struct ProvingService {
    base_url: String,
    client: reqwest::Client,
    max_retries: u32,
    base_backoff: Duration,
    transport: Transport,
}

impl core::fmt::Debug for ProvingService {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ProvingService")
            .field("endpoint", &public_endpoint(&self.base_url))
            .field("max_retries", &self.max_retries)
            .field("base_backoff", &self.base_backoff)
            .field("transport", &self.transport)
            .finish()
    }
}

impl ProvingService {
    /// Creates a client for `base_url`.
    ///
    /// The endpoint receives the pool private key. See the module docs.
    pub fn new(base_url: impl Into<String>) -> Result<Self, ProverError> {
        Self::build(base_url.into(), None)
    }

    /// Creates a prover client with durable hosted-job storage under `state_dir`.
    ///
    /// Starkscan delivers a proof result once. Erebus writes the result here before returning
    /// it to execution, so a process failure cannot silently discard the proof or its screening
    /// attestation. Local JSON-RPC provers do not use this directory.
    pub fn new_persistent(
        base_url: impl Into<String>,
        state_dir: impl AsRef<Path>,
    ) -> Result<Self, ProverError> {
        Self::build(base_url.into(), Some(state_dir.as_ref()))
    }

    fn build(base_url: String, state_dir: Option<&Path>) -> Result<Self, ProverError> {
        let client = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()?;
        let transport = if is_starkscan_prove_url(&base_url) {
            let api_key = std::env::var("STARKSCAN_API_KEY").map_err(|_| {
                ProverError::Malformed(
                    "STARKSCAN_API_KEY is required for a Starkscan prover URL".to_owned(),
                )
            })?;
            if api_key.trim().is_empty() {
                return Err(ProverError::Malformed(
                    "STARKSCAN_API_KEY is empty".to_owned(),
                ));
            }
            let state_dir = state_dir.ok_or_else(|| {
                ProverError::Malformed(
                    "Starkscan proving requires durable state; construct the client with new_persistent"
                        .to_owned(),
                )
            })?;
            let job_dir = state_dir.join("prover-jobs");
            create_private_dir(&job_dir)?;
            Transport::Starkscan { api_key, job_dir }
        } else {
            Transport::JsonRpc
        };
        Ok(Self {
            base_url,
            client,
            max_retries: DEFAULT_MAX_RETRIES,
            base_backoff: DEFAULT_BASE_BACKOFF,
            transport,
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
        if let Transport::Starkscan { api_key, .. } = &self.transport {
            return self.starkscan_health(api_key).await;
        }
        let value = self.call_once("starknet_specVersion", json!([])).await?;
        value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| ProverError::Malformed("specVersion was not a string".to_owned()))
    }

    /// Proves a transaction.
    pub async fn prove_transaction(
        &self,
        block_id: &BlockId,
        transaction: &SignedInvokeV3,
    ) -> Result<ProveTransactionResult, ProverError> {
        if matches!(self.transport, Transport::Starkscan { .. }) {
            return Err(ProverError::Malformed(
                "Starkscan proving requires a durable idempotency key".to_owned(),
            ));
        }
        self.prove_json_rpc(block_id, transaction).await
    }

    /// Proves a transaction with a caller-owned durable idempotency key.
    ///
    /// The seed identifies one durable operation. Local JSON-RPC ignores it; Starkscan
    /// combines it with the exact request hash for submission recovery and persists the
    /// returned one-time result.
    pub async fn prove_transaction_idempotent(
        &self,
        block_id: &BlockId,
        transaction: &SignedInvokeV3,
        idempotency_key: &str,
    ) -> Result<ProveTransactionResult, ProverError> {
        match &self.transport {
            Transport::JsonRpc => self.prove_json_rpc(block_id, transaction).await,
            Transport::Starkscan { api_key, job_dir } => {
                self.prove_starkscan(block_id, transaction, idempotency_key, api_key, job_dir)
                    .await
            }
        }
    }

    /// Resumes the hosted proof job recorded for one durable operation.
    ///
    /// Returns `None` when this is a local prover or the operation has no recoverable hosted
    /// job. The private request exists on disk only until the relay job ID is durable.
    pub async fn resume_transaction_idempotent(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<ProveTransactionResult>, ProverError> {
        let Transport::Starkscan { api_key, job_dir } = &self.transport else {
            return Ok(None);
        };
        validate_idempotency_key(idempotency_key)?;
        let index_path = operation_index_path(job_dir, idempotency_key);
        let Some(index) = read_cached_index(&index_path)? else {
            return Ok(None);
        };
        validate_cached_index(&index)?;
        let seed_hash = sha256_hex(idempotency_key.as_bytes());
        let expected_relay_key =
            format!("erebus-{}-{}", &seed_hash[..32], &index.request_hash[..32]);
        let expected_cache_file = format!("{}.json", sha256_hex(expected_relay_key.as_bytes()));
        if index.relay_key != expected_relay_key || index.cache_file != expected_cache_file {
            return Err(ProverError::JobState(
                "hosted prover operation index does not match its operation ID".to_owned(),
            ));
        }
        let cache_path = job_dir.join(&index.cache_file);
        let mut cached = read_cached_job(&cache_path)?.ok_or_else(|| {
            ProverError::JobState(format!(
                "hosted prover index points to missing cache {}",
                cache_path.display()
            ))
        })?;
        if cached.request_hash != index.request_hash {
            return Err(ProverError::JobState(
                "hosted prover index request hash does not match its cache".to_owned(),
            ));
        }
        if let Some(result) = cached.result.clone() {
            return Ok(Some(result));
        }

        let job_id = match cached.job_id.clone() {
            Some(job_id) => job_id,
            None => {
                let request = cached.request.as_ref().ok_or_else(|| {
                    ProverError::JobState(
                        "hosted prover job has neither a job ID nor its exact request".to_owned(),
                    )
                })?;
                let encoded = serde_json::to_vec(request)
                    .map_err(|error| ProverError::JobState(error.to_string()))?;
                if sha256_hex(&encoded) != cached.request_hash {
                    return Err(ProverError::JobState(
                        "hosted prover cached request does not match its request hash".to_owned(),
                    ));
                }
                let value = self
                    .starkscan_submit(api_key, &index.relay_key, request)
                    .await?;
                let job_id = required_string(&value, "jobId")?;
                validate_job_id(&job_id)?;
                cached.job_id = Some(job_id.clone());
                cached.request = None;
                write_cached_job(&cache_path, &cached)?;
                job_id
            }
        };

        self.poll_starkscan_job(api_key, &job_id, &mut cached, &cache_path, &index_path)
            .await
            .map(Some)
    }

    async fn prove_json_rpc(
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
                    return serde_json::from_value(value).map_err(|error| {
                        ProverError::Malformed(format!("invalid prove result: {error}"))
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

    async fn starkscan_health(&self, api_key: &str) -> Result<String, ProverError> {
        let capabilities_url = starkscan_capabilities_url(&self.base_url)?;
        let response = self
            .client
            .get(capabilities_url)
            .header("X-Starkscan-Api-Key", api_key)
            .send()
            .await?;
        let status = response.status();
        let value: serde_json::Value = response.json().await?;
        if !status.is_success() {
            return Err(relay_http_error(status, &value));
        }
        let has_prove = value
            .pointer("/caller/scopes")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|scopes| scopes.iter().any(|scope| scope == "prove"));
        if !has_prove {
            return Err(ProverError::Relay {
                code: "missing_prove_scope".to_owned(),
                message: "the Starkscan API key does not have prove scope".to_owned(),
            });
        }
        Ok("starkscan-async/prove".to_owned())
    }

    async fn prove_starkscan(
        &self,
        block_id: &BlockId,
        transaction: &SignedInvokeV3,
        idempotency_key: &str,
        api_key: &str,
        job_dir: &Path,
    ) -> Result<ProveTransactionResult, ProverError> {
        validate_idempotency_key(idempotency_key)?;
        let body = json!({
            "block_id": block_id.to_param(),
            "transaction": transaction.to_wire(),
        });
        let body_bytes =
            serde_json::to_vec(&body).map_err(|error| ProverError::Malformed(error.to_string()))?;
        let request_hash = sha256_hex(&body_bytes);
        let seed_hash = sha256_hex(idempotency_key.as_bytes());
        let relay_key = format!("erebus-{}-{}", &seed_hash[..32], &request_hash[..32]);
        let cache_path = job_dir.join(format!("{}.json", sha256_hex(relay_key.as_bytes())));
        let index_path = operation_index_path(job_dir, idempotency_key);
        let mut cached = read_cached_job(&cache_path)?.unwrap_or(CachedJob {
            version: CACHED_JOB_VERSION,
            request_hash: request_hash.clone(),
            request: Some(body.clone()),
            job_id: None,
            result: None,
        });
        if cached.request_hash != request_hash {
            return Err(ProverError::Relay {
                code: "idempotency_key_reused".to_owned(),
                message: "the durable idempotency key is bound to a different proof request"
                    .to_owned(),
            });
        }
        if let Some(result) = cached.result.clone() {
            return Ok(result);
        }
        let index = CachedJobIndex {
            version: CACHED_JOB_INDEX_VERSION,
            request_hash: request_hash.clone(),
            relay_key: relay_key.clone(),
            cache_file: cache_path
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .ok_or_else(|| {
                    ProverError::JobState("hosted prover cache filename is not UTF-8".to_owned())
                })?
                .to_owned(),
        };
        if cached.job_id.is_none() {
            cached.version = CACHED_JOB_VERSION;
            cached.request = Some(body.clone());
            write_cached_job(&cache_path, &cached)?;
        }
        write_cached_index(&index_path, &index)?;

        let job_id = match cached.job_id.clone() {
            Some(job_id) => job_id,
            None => {
                let value = self.starkscan_submit(api_key, &relay_key, &body).await?;
                let job_id = required_string(&value, "jobId")?;
                validate_job_id(&job_id)?;
                cached.job_id = Some(job_id.clone());
                cached.request = None;
                write_cached_job(&cache_path, &cached)?;
                job_id
            }
        };

        self.poll_starkscan_job(api_key, &job_id, &mut cached, &cache_path, &index_path)
            .await
    }

    async fn poll_starkscan_job(
        &self,
        api_key: &str,
        job_id: &str,
        cached: &mut CachedJob,
        cache_path: &Path,
        index_path: &Path,
    ) -> Result<ProveTransactionResult, ProverError> {
        loop {
            let value = self.starkscan_poll(api_key, job_id).await?;
            let terminal = value
                .get("terminal")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| {
                    ProverError::Malformed("poll response omitted terminal".to_owned())
                })?;
            if !terminal {
                let delay = value
                    .get("pollAfterSeconds")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(10)
                    .clamp(1, 60);
                tokio::time::sleep(Duration::from_secs(delay)).await;
                continue;
            }

            let status = value
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            if status == "succeeded" {
                let Some(result_value) = value.get("result").cloned() else {
                    let reason = value
                        .get("resultUnavailableReason")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("missing");
                    remove_cached_index(index_path)?;
                    return Err(ProverError::Relay {
                        code: "result_unavailable".to_owned(),
                        message: format!("proof result unavailable: {reason}"),
                    });
                };
                let result: ProveTransactionResult =
                    serde_json::from_value(result_value).map_err(|error| {
                        ProverError::Malformed(format!("invalid hosted prove result: {error}"))
                    })?;
                cached.result = Some(result.clone());
                write_cached_job(cache_path, cached)?;
                return Ok(result);
            }

            let error = starkscan_terminal_error(status, value.get("error"));
            remove_cached_index(index_path)?;
            return Err(error);
        }
    }

    async fn starkscan_submit(
        &self,
        api_key: &str,
        idempotency_key: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, ProverError> {
        self.starkscan_request_with_retry(|| {
            self.client
                .post(&self.base_url)
                .header("X-Starkscan-Api-Key", api_key)
                .header("Idempotency-Key", idempotency_key)
                .json(body)
        })
        .await
    }

    async fn starkscan_poll(
        &self,
        api_key: &str,
        job_id: &str,
    ) -> Result<serde_json::Value, ProverError> {
        let url = format!("{}/{}", self.base_url.trim_end_matches('/'), job_id);
        self.starkscan_request_with_retry(|| {
            self.client.get(&url).header("X-Starkscan-Api-Key", api_key)
        })
        .await
    }

    /// Repeats only transport failures and HTTP 503 with the exact same request.
    ///
    /// Starkscan binds an idempotency key to the request body. Repeating a submission after
    /// its response is lost therefore recovers the original job instead of spending another
    /// proof. Polls are read-only, but use the same bounded retry policy so a transient relay
    /// failure cannot discard a terminal result that is delivered once.
    async fn starkscan_request_with_retry<F>(
        &self,
        mut request: F,
    ) -> Result<serde_json::Value, ProverError>
    where
        F: FnMut() -> reqwest::RequestBuilder,
    {
        let mut attempt = 0;
        loop {
            let (retryable, result) = match request().send().await {
                Ok(response) => {
                    let retryable = response.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE;
                    (retryable, parse_relay_response(response).await)
                }
                Err(error) => {
                    let retryable = error.is_request()
                        || error.is_timeout()
                        || error.is_connect()
                        || error.status() == Some(reqwest::StatusCode::SERVICE_UNAVAILABLE);
                    (retryable, Err(ProverError::Transport(error)))
                }
            };

            if !retryable || attempt >= self.max_retries {
                return result;
            }
            tokio::time::sleep(self.base_backoff * 2u32.pow(attempt)).await;
            attempt += 1;
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

        value
            .get("result")
            .cloned()
            .ok_or_else(|| ProverError::Malformed(format!("HTTP {status}, no result field")))
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
        ProverError::Rpc { .. }
        | ProverError::Malformed(_)
        | ProverError::RetriesExhausted(_)
        | ProverError::Relay { .. }
        | ProverError::JobState(_) => false,
    }
}

fn is_starkscan_prove_url(url: &str) -> bool {
    url.trim_end_matches('/').ends_with("/v1/SN_MAIN/prove")
}

fn public_endpoint(url: &str) -> String {
    reqwest::Url::parse(url)
        .map(|parsed| parsed.origin().ascii_serialization())
        .unwrap_or_else(|_| "<invalid URL>".to_owned())
}

fn starkscan_capabilities_url(prove_url: &str) -> Result<String, ProverError> {
    let parsed = reqwest::Url::parse(prove_url)
        .map_err(|error| ProverError::Malformed(format!("invalid Starkscan URL: {error}")))?;
    let origin = parsed.origin().ascii_serialization();
    Ok(format!("{origin}/v1/meta/capabilities"))
}

fn validate_idempotency_key(key: &str) -> Result<(), ProverError> {
    let valid = (16..=128).contains(&key.len())
        && key
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b'"');
    if valid {
        Ok(())
    } else {
        Err(ProverError::Malformed(
            "Starkscan idempotency key must be 16-128 graphic ASCII characters without quotes"
                .to_owned(),
        ))
    }
}

fn validate_job_id(job_id: &str) -> Result<(), ProverError> {
    let valid = (8..=128).contains(&job_id.len())
        && job_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(ProverError::Malformed(
            "Starkscan returned an invalid jobId".to_owned(),
        ))
    }
}

async fn parse_relay_response(
    response: reqwest::Response,
) -> Result<serde_json::Value, ProverError> {
    let status = response.status();
    let value: serde_json::Value = response.json().await?;
    if status.is_success() {
        Ok(value)
    } else {
        Err(relay_http_error(status, &value))
    }
}

fn relay_http_error(status: reqwest::StatusCode, value: &serde_json::Value) -> ProverError {
    ProverError::Relay {
        code: value
            .get("code")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("http_{}", status.as_u16())),
        message: value
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("hosted prover request failed")
            .to_owned(),
    }
}

fn starkscan_terminal_error(status: &str, error: Option<&serde_json::Value>) -> ProverError {
    let code = error.and_then(|value| value.get("code"));
    if let Some(code) = code.and_then(serde_json::Value::as_i64) {
        return ProverError::Rpc {
            code,
            message: error
                .and_then(|value| value.get("message"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("hosted prover rejected the transaction")
                .to_owned(),
            data: error.and_then(|value| value.get("data")).cloned(),
        };
    }
    ProverError::Relay {
        code: code
            .and_then(serde_json::Value::as_str)
            .unwrap_or(status)
            .to_owned(),
        message: error
            .and_then(|value| value.get("message"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("hosted prover job did not succeed")
            .to_owned(),
    }
}

fn required_string(value: &serde_json::Value, field: &str) -> Result<String, ProverError> {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ProverError::Malformed(format!("response omitted {field}")))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn create_private_dir(path: &Path) -> Result<(), ProverError> {
    fs::create_dir_all(path).map_err(|error| {
        ProverError::JobState(format!("cannot create {}: {error}", path.display()))
    })?;
    set_mode(path, 0o700)
}

fn operation_index_path(job_dir: &Path, idempotency_key: &str) -> PathBuf {
    job_dir.join(format!(
        "operation-{}.json",
        sha256_hex(idempotency_key.as_bytes())
    ))
}

fn read_cached_job(path: &Path) -> Result<Option<CachedJob>, ProverError> {
    match fs::read(path) {
        Ok(bytes) => {
            let job: CachedJob = serde_json::from_slice(&bytes).map_err(|error| {
                ProverError::JobState(format!("cannot decode {}: {error}", path.display()))
            })?;
            if !(1..=CACHED_JOB_VERSION).contains(&job.version) {
                return Err(ProverError::JobState(format!(
                    "hosted prover job schema version {} is not supported",
                    job.version
                )));
            }
            Ok(Some(job))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ProverError::JobState(format!(
            "cannot read {}: {error}",
            path.display()
        ))),
    }
}

fn write_cached_job(path: &Path, job: &CachedJob) -> Result<(), ProverError> {
    write_private_json(path, job)
}

fn read_cached_index(path: &Path) -> Result<Option<CachedJobIndex>, ProverError> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|error| {
            ProverError::JobState(format!("cannot decode {}: {error}", path.display()))
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ProverError::JobState(format!(
            "cannot read {}: {error}",
            path.display()
        ))),
    }
}

fn validate_cached_index(index: &CachedJobIndex) -> Result<(), ProverError> {
    let hash_is_valid = |value: &str| {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    let cache_file_valid = index
        .cache_file
        .strip_suffix(".json")
        .is_some_and(hash_is_valid);
    if index.version != CACHED_JOB_INDEX_VERSION
        || !hash_is_valid(&index.request_hash)
        || !cache_file_valid
    {
        return Err(ProverError::JobState(
            "hosted prover operation index is invalid".to_owned(),
        ));
    }
    validate_idempotency_key(&index.relay_key)
}

fn write_cached_index(path: &Path, index: &CachedJobIndex) -> Result<(), ProverError> {
    write_private_json(path, index)
}

fn remove_cached_index(path: &Path) -> Result<(), ProverError> {
    match fs::remove_file(path) {
        Ok(()) => sync_parent(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ProverError::JobState(format!(
            "cannot remove {}: {error}",
            path.display()
        ))),
    }
}

fn write_private_json(path: &Path, value: &impl Serialize) -> Result<(), ProverError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| ProverError::JobState(format!("cannot encode job state: {error}")))?;
    let suffix = rand::random::<u64>();
    let temp = path.with_extension(format!("tmp-{suffix:016x}"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|error| {
            ProverError::JobState(format!("cannot create {}: {error}", temp.display()))
        })?;
    set_mode(&temp, 0o600)?;
    file.write_all(&bytes).map_err(|error| {
        ProverError::JobState(format!("cannot write {}: {error}", temp.display()))
    })?;
    file.sync_all().map_err(|error| {
        ProverError::JobState(format!("cannot sync {}: {error}", temp.display()))
    })?;
    fs::rename(&temp, path).map_err(|error| {
        ProverError::JobState(format!(
            "cannot replace {} with {}: {error}",
            path.display(),
            temp.display()
        ))
    })?;
    sync_parent(path)
}

fn sync_parent(path: &Path) -> Result<(), ProverError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let directory = fs::File::open(parent).map_err(|error| {
        ProverError::JobState(format!("cannot open {}: {error}", parent.display()))
    })?;
    directory.sync_all().map_err(|error| {
        ProverError::JobState(format!("cannot sync {}: {error}", parent.display()))
    })
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), ProverError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|error| {
        ProverError::JobState(format!("cannot protect {}: {error}", path.display()))
    })
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), ProverError> {
    Ok(())
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

    #[test]
    fn proving_service_debug_omits_credentials_in_the_url() {
        let service =
            ProvingService::new("https://user:password@example.com/prover-key?token=secret")
                .expect("client");
        let shown = format!("{service:?}");
        assert!(shown.contains("https://example.com"));
        for secret in ["user", "password", "prover-key", "token", "secret"] {
            assert!(!shown.contains(secret));
        }
    }
}
