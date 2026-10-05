//! `erebus-tx-relayer`: the public-bound transaction relayer (Metropolis M6, decision DM6-8).
//!
//! The relayer submits an already-authorized settlement so the buyer does not have to hold gas.
//! It never signs an authorization and cannot change a payment: the fee and its recipient are
//! committed terms of the agreement, checked by [`erebus_evm::relay::RelayPolicy`] before any
//! gas is spent, and re-checked by the settlement contract.
//!
//! Default mode handles one JSON request on stdin. `--serve` handles newline-delimited JSON
//! until EOF, retaining access-limit windows and counters across requests. These counters
//! are process-local, not durable across restarts. The trusted host sets the client identity.
//!
//! Configuration (environment):
//!
//! | Variable | Meaning |
//! |---|---|
//! | `EREBUS_RELAYER_RPC_URLS` | Comma-separated JSON-RPC endpoints, tried in order |
//! | `EREBUS_RELAYER_VERIFICATION_RPC_URL` | Distinct mandatory second observer for relay/recover |
//! | `EREBUS_RELAYER_NAMESPACE` | CAIP-2 namespace, for example `eip155:143` |
//! | `EREBUS_RELAYER_SETTLEMENT` | Deployed settlement contract, lowercase `0x` address |
//! | `EREBUS_RELAYER_VERIFIER_VERSION` | Verifier version the deployment enforces |
//! | `EREBUS_RELAYER_KEY` | Gas-paying private key, 32 hex bytes |
//! | `EREBUS_RELAYER_FEE_RECIPIENT` | Published fee recipient, lowercase `0x` address |
//! | `EREBUS_RELAYER_FEES` | `token=amount` pairs, comma-separated, in base units |
//! | `EREBUS_RELAYER_MAX_LIFETIME` | Maximum admitted agreement lifetime in seconds (default 3600) |
//! | `EREBUS_RELAYER_MAX_REQUESTS` | Access limit per window (default 60) |
//! | `EREBUS_RELAYER_WINDOW_SECONDS` | Access-limit window (default 60) |
//! | `EREBUS_RELAYER_CLIENT` | Opaque client identifier for the access limit (default `cli`) |
//! | `EREBUS_RELAYER_STATE_ROOT` | Private persistent relayer directory; required for relay/recover |
//! | `EREBUS_RELAYER_MAX_FEE_PER_GAS` | Explicit EIP-1559 fee cap in wei |
//! | `EREBUS_RELAYER_PRIORITY_FEE_PER_GAS` | Explicit EIP-1559 priority fee cap in wei |
//! | `EREBUS_RELAYER_GAS_LIMIT` | Explicit transaction gas limit |
//! | `EREBUS_RELAYER_RPC_TIMEOUT_SECONDS` | Per-provider deadline (default 15) |
//! | `EREBUS_RELAYER_LOG_BLOCK_RANGE` | Blocks per history query (default 2000) |
//! | `EREBUS_RELAYER_LOG_QUERIES` | History queries per call (default 1024) |
//! | `EREBUS_RELAYER_ANCESTRY_LINKS` | Parent links per call (default 8192) |
//!
//! Requests: `{"method":"policy"}` returns the published fee schedule; `{"method":"health"}`
//! returns redacted counters; `{"method":"funding","evidence":"<hex>"}` reports the gas payer's
//! native shortfall. `relay` persists and submits an authorized request; `recover` reconciles
//! an existing request without signing or sending. Both require persistent storage. No response or
//! log line contains the relayer key or a signature.

use std::collections::BTreeMap;
use std::io::{BufRead, Read, Write};

use erebus_core::ids::ChainNamespace;
use erebus_evm::backend::EvmSettlementBackend;
use erebus_evm::chain::{Eip1559Fees, ObservationLimits};
use erebus_evm::deployment::{parse_lowercase_address, EvmDeployment};
use erebus_evm::relay::{AccessLimits, DurableRelayer, RelayPolicy, RelayService};
use serde_json::json;

#[tokio::main]
async fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let serve = match arguments.as_slice() {
        [] => false,
        [mode] if mode == "--serve" => true,
        _ => fail("usage: erebus-tx-relayer [--serve]"),
    };
    let (service, client, operator) = match configure() {
        Ok(configured) => configured,
        Err(message) => fail(&message),
    };
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    loop {
        let mut bytes = Vec::new();
        let mut bounded = input.by_ref().take((MAX_REQUEST_BYTES + 1) as u64);
        let read = if serve {
            bounded.read_until(b'\n', &mut bytes)
        } else {
            bounded.read_to_end(&mut bytes)
        };
        if read.is_err() {
            fail("cannot read the request");
        }
        if bytes.len() > MAX_REQUEST_BYTES {
            fail("request exceeds size limit");
        }
        if serve && bytes.is_empty() {
            break;
        }
        let result = match serde_json::from_slice(&bytes) {
            Ok(request) => handle(&service, &client, operator.as_ref(), request).await,
            Err(_) => Err("request is not valid JSON".to_owned()),
        };
        let failed = result.is_err();
        let response = result.unwrap_or_else(|error| json!({"status": "error", "error": error}));
        println!("{response}");
        if std::io::stdout().flush().is_err() {
            std::process::exit(1);
        }
        if !serve {
            std::process::exit(i32::from(failed));
        }
    }
}

// Hex evidence uses two bytes per payload byte; the rest bounds JSON framing.
const MAX_REQUEST_BYTES: usize = 2 * erebus_evm::relay::MAX_RELAY_EVIDENCE_BYTES + 4096;

async fn handle(
    service: &RelayService,
    client: &str,
    operator: Option<&DurableRelayer>,
    request: serde_json::Value,
) -> Result<serde_json::Value, String> {
    match request["method"].as_str() {
        Some("policy") => Ok(policy_response(service.policy())),
        Some("health") => Ok(json!({ "status": "ok", "metrics": metrics(service) })),
        Some(method @ ("relay" | "recover")) => {
            let operator =
                operator.ok_or_else(|| "relay requires a durable operation journal".to_owned())?;
            let evidence = request["evidence"]
                .as_str()
                .ok_or_else(|| "relay requires hex evidence".to_owned())?;
            let bytes =
                hex::decode(evidence).map_err(|_| "evidence is not valid hex".to_owned())?;
            let now = unix_now()?;
            let operation = if method == "recover" {
                operator.recover(service, client, &bytes, now).await
            } else {
                operator.relay(service, client, &bytes, now).await
            }
            .map_err(|error| error.to_string())?;
            Ok(json!({
                "status": "ok",
                "operation_ref": hex::encode(operation.operation_ref),
                "stage": format!("{:?}", operation.stage),
                "transaction_hash": operation.transaction_hash.map(hex::encode),
                "gas_account_busy": operation.gas_account_busy,
            }))
        }
        Some("funding") => {
            let Some(evidence) = request["evidence"].as_str() else {
                return Err("relay requires hex evidence".to_owned());
            };
            let Ok(bytes) = hex::decode(evidence) else {
                return Err("evidence is not valid hex".to_owned());
            };
            let now = unix_now()?;
            let prepared = service
                .admit(client, &bytes, now)
                .map_err(|error| error.to_string())?;
            match service.funding(&prepared).await {
                Ok(funding) => Ok(json!({
                    "status": "ok",
                    "gas": funding.gas,
                    "gas_price": funding.gas_price.to_string(),
                    "required": funding.required.to_string(),
                    "balance": funding.balance.to_string(),
                    "shortfall": funding.shortfall().to_string(),
                    "funded": funding.is_funded(),
                })),
                Err(_) => Err("funding diagnostics unavailable".to_owned()),
            }
        }
        _ => Err("unknown method".to_owned()),
    }
}

fn configure() -> Result<(RelayService, String, Option<DurableRelayer>), String> {
    let namespace = ChainNamespace::parse(&env("EREBUS_RELAYER_NAMESPACE")?)
        .map_err(|_| "EREBUS_RELAYER_NAMESPACE is not a CAIP-2 namespace".to_owned())?;
    let settlement = address(&env("EREBUS_RELAYER_SETTLEMENT")?)?;
    let verifier_version = env("EREBUS_RELAYER_VERIFIER_VERSION")?
        .parse::<u32>()
        .map_err(|_| "EREBUS_RELAYER_VERIFIER_VERSION is not a u32".to_owned())?;
    let fee_recipient = address(&env("EREBUS_RELAYER_FEE_RECIPIENT")?)?;
    let max_lifetime = optional_number("EREBUS_RELAYER_MAX_LIFETIME", 3_600)?;
    let max_requests = optional_number("EREBUS_RELAYER_MAX_REQUESTS", 60)?;
    let window_seconds = optional_number("EREBUS_RELAYER_WINDOW_SECONDS", 60)?;
    let token_fees = parse_fees(&env("EREBUS_RELAYER_FEES")?)?;
    let key = parse_key(&env("EREBUS_RELAYER_KEY")?)?;
    let client = std::env::var("EREBUS_RELAYER_CLIENT").unwrap_or_else(|_| "cli".to_owned());

    let urls = env("EREBUS_RELAYER_RPC_URLS")?;
    let mut backends = Vec::new();
    for url in urls.split(',').map(str::trim).filter(|url| !url.is_empty()) {
        let deployment = EvmDeployment::new(
            namespace.clone(),
            settlement,
            verifier_version,
            url.to_owned(),
        )
        .map_err(|_| "relayer deployment is invalid".to_owned())?;
        backends.push(
            EvmSettlementBackend::connect(deployment, &key)
                .map_err(|_| "relayer key is invalid".to_owned())?,
        );
    }
    let deployment = EvmDeployment::new(namespace, settlement, verifier_version, urls.clone())
        .map_err(|_| "relayer deployment is invalid".to_owned())?;
    let policy = RelayPolicy::new(deployment, fee_recipient, token_fees, max_lifetime)
        .map_err(|error| error.to_string())?;
    let service = RelayService::new(
        policy,
        backends,
        AccessLimits::new(max_requests, window_seconds),
    )
    .map_err(|error| error.to_string())?;
    let operator = match std::env::var("EREBUS_RELAYER_STATE_ROOT") {
        Ok(root) => {
            let verification_rpc_url = env("EREBUS_RELAYER_VERIFICATION_RPC_URL")?;
            let fees = Eip1559Fees::new(
                required_number("EREBUS_RELAYER_MAX_FEE_PER_GAS")?,
                required_number("EREBUS_RELAYER_PRIORITY_FEE_PER_GAS")?,
            )
            .map_err(|_| "relayer fee caps are invalid".to_owned())?;
            let operator = DurableRelayer::new(
                root,
                &key,
                fees,
                required_number("EREBUS_RELAYER_GAS_LIMIT")?,
                std::time::Duration::from_secs(optional_number(
                    "EREBUS_RELAYER_RPC_TIMEOUT_SECONDS",
                    15,
                )?),
                &verification_rpc_url,
            )
            .map_err(|error| error.to_string())?
            .with_observation_budget(ObservationLimits {
                log_block_range: optional_number("EREBUS_RELAYER_LOG_BLOCK_RANGE", 2000)?,
                max_log_queries: optional_number("EREBUS_RELAYER_LOG_QUERIES", 1024)?,
                max_ancestry: optional_number("EREBUS_RELAYER_ANCESTRY_LINKS", 8192)?,
                max_concurrent_queries: optional_number("EREBUS_RELAYER_LOG_CONCURRENCY", 8)?,
            })
            .map_err(|error| error.to_string())?;
            operator
                .check_configuration(&service)
                .map_err(|error| error.to_string())?;
            Some(operator)
        }
        Err(_) => None,
    };
    Ok((service, client, operator))
}

fn required_number<T: std::str::FromStr>(name: &str) -> Result<T, String> {
    env(name)?
        .parse()
        .map_err(|_| format!("{name} is not a number"))
}

fn metrics(service: &RelayService) -> serde_json::Value {
    let metrics = service.metrics();
    json!({
        "admitted": metrics.admitted,
        "rejected": metrics.rejected,
        "rate_limited": metrics.rate_limited,
        "submitted": metrics.submitted,
        "submit_failed": metrics.submit_failed,
    })
}

fn env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("{name} is not set"))
}

fn optional_number<T: std::str::FromStr>(name: &str, default: T) -> Result<T, String> {
    match std::env::var(name) {
        Ok(value) => value.parse().map_err(|_| format!("{name} is not a number")),
        Err(_) => Ok(default),
    }
}

fn address(value: &str) -> Result<[u8; 20], String> {
    parse_lowercase_address(value).ok_or_else(|| "address must be lowercase 0x hex".to_owned())
}

fn parse_fees(value: &str) -> Result<BTreeMap<[u8; 20], u128>, String> {
    let mut fees = BTreeMap::new();
    for entry in value.split(',') {
        let Some((token, amount)) = entry.split_once('=') else {
            return Err("EREBUS_RELAYER_FEES entries must be token=amount".to_owned());
        };
        let token = address(token.trim())?;
        let amount = amount
            .trim()
            .parse::<u128>()
            .map_err(|_| "fee amount is not a u128".to_owned())?;
        fees.insert(token, amount);
    }
    if fees.is_empty() {
        return Err("EREBUS_RELAYER_FEES is empty".to_owned());
    }
    Ok(fees)
}

fn policy_response(policy: &RelayPolicy) -> serde_json::Value {
    let fees: Vec<serde_json::Value> = policy
        .token_fees()
        .iter()
        .map(|(token, fee)| {
            json!({ "token": format!("0x{}", hex::encode(token)), "fee": fee.to_string() })
        })
        .collect();
    json!({
        "status": "ok",
        "fee_recipient": format!("0x{}", hex::encode(policy.fee_recipient())),
        "fees": fees,
    })
}

fn unix_now() -> Result<u64, String> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| "system clock is before the Unix epoch".to_owned())
}

fn parse_key(value: &str) -> Result<[u8; 32], String> {
    hex::decode(value.trim_start_matches("0x"))
        .map_err(|_| "EREBUS_RELAYER_KEY is not hex".to_owned())?
        .try_into()
        .map_err(|_| "EREBUS_RELAYER_KEY must be 32 bytes".to_owned())
}

fn fail(message: &str) -> ! {
    // Redacted: never echo evidence, calldata, keys, or provider response text.
    println!("{}", json!({ "status": "error", "error": message }));
    std::process::exit(1);
}
