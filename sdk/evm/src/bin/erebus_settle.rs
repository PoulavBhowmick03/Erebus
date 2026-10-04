//! Buyer-side public-bound settlement onboarding: one bounded JSON request on stdin.
//!
//! `funding` is read-only. It reports, before a buyer authorizes anything, the buyer's token
//! allowance and balance against the signed amount plus fee, and the gas payer's native
//! shortfall. It never signs, submits, or prints the endpoint, terms, or signatures.
//!
//! Request:
//! ```json
//! {"method":"funding",
//!  "deployment":{"namespace":"eip155:10143","settlement_contract":"0x...",
//!                "verifier_version":1,"rpc_url":"https://..."},
//!  "signer_address":"0x...",
//!  "evidence":"<hex SettlementEvidence>"}
//! ```

use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use erebus_core::auth::{Authorization, Role};
use erebus_core::commitment::{commit_agreement, deal_nullifier, CommitmentBlinding};
use erebus_core::deal_state::{
    assess_deal, DealEvidence, DealState, RevisionState, SignedRevision,
};
use erebus_core::ids::{ChainNamespace, SignatureBytes};
use erebus_core::terms::AgreementTerms;
use erebus_evm::backend::{public_bound_capabilities, EvmSettlementBackend};
use erebus_evm::chain::{EvmChain, HistoricalObservation, ObservationJournal, ObservationLimits};
use erebus_evm::deployment::{parse_lowercase_address, EvmDeployment};
use erebus_evm::evidence::SettlementEvidence;
use serde::Deserialize;
use serde_json::{json, Value};

const MAX_REQUEST_BYTES: usize = 64 * 1024;

#[derive(Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Capabilities,
    /// The address of an owner-only 32-byte gas key, so an operator knows what to fund.
    /// Reads no chain state and signs nothing.
    Address {
        key_file: PathBuf,
    },
    Funding {
        deployment: Deployment,
        signer_address: String,
        evidence: String,
    },
    Receipt {
        deployment: Deployment,
        state_root: PathBuf,
        operation_ref: String,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Deployment {
    namespace: String,
    settlement_contract: String,
    verifier_version: u32,
    rpc_url: String,
    #[serde(default)]
    from_block: u64,
    #[serde(default = "default_log_block_range")]
    log_block_range: u64,
    #[serde(default = "default_log_queries")]
    max_log_queries: u64,
    #[serde(default = "default_ancestry")]
    max_ancestry: u64,
}

fn default_log_block_range() -> u64 {
    2_000
}
fn default_log_queries() -> u64 {
    8
}
fn default_ancestry() -> u64 {
    64
}

const HELP: &str = "erebus-settle: buyer-side public-bound settlement as JSON on stdin.
Receipt observation persists public history under state_root/public-observation.
Pending history exits 2 with payment_finalized:false; repeat the same request to resume.
Configure deployment.from_block and log_block_range for the chosen RPC.
Methods: capabilities, funding, receipt.
capabilities reports the public-bound backend's declared suites, modes, and guarantees.
funding is read-only: it reports the buyer's token allowance and balance against the signed
amount plus fee, and the gas payer's native shortfall, before any authorization is signed.
receipt reads one durable agreement opening and its finalized chain evidence, then reports the
deal and revision state. It never signs or submits.
Output excludes the RPC URL, terms, signatures, and keys.
See docs/metropolis-m8-runbook.md.";

#[tokio::main]
async fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if matches!(arguments.as_slice(), [argument] if argument == "--help") {
        println!("{HELP}");
        return;
    }
    if !arguments.is_empty() {
        finish(Err("invalid arguments"));
    }
    let mut input = Vec::new();
    if std::io::stdin()
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut input)
        .is_err()
    {
        finish(Err("cannot read request"));
    }
    if input.len() > MAX_REQUEST_BYTES {
        finish(Err("request exceeds size limit"));
    }
    let request = match serde_json::from_slice(&input) {
        Ok(request) => request,
        Err(_) => finish(Err("invalid request")),
    };
    finish(handle(request).await);
}

fn finish(result: Result<Value, &'static str>) -> ! {
    let failed = result.is_err();
    let response = result.unwrap_or_else(|error| json!({"status": "error", "error": error}));
    let pending = response["status"] == "pending";
    println!("{response}");
    let flushed = std::io::stdout().flush().is_ok();
    std::process::exit(if !flushed {
        1
    } else if pending {
        2
    } else {
        i32::from(failed)
    });
}

async fn handle(request: Request) -> Result<Value, &'static str> {
    match request {
        Request::Address { key_file } => {
            let metadata =
                std::fs::symlink_metadata(&key_file).map_err(|_| "key file unavailable")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 {
                    return Err("key file must be an owner-only regular file");
                }
            }
            #[cfg(not(unix))]
            let _ = metadata;
            let seed = zeroize::Zeroizing::new(
                std::fs::read(&key_file).map_err(|_| "key file unavailable")?,
            );
            let seed: &[u8; 32] = seed
                .as_slice()
                .try_into()
                .map_err(|_| "key file must hold 32 raw bytes")?;
            let key =
                erebus_evm::chain::TransactionKey::from_bytes(seed).map_err(|_| "invalid key")?;
            Ok(json!({"status":"ok","address":format!("0x{}", hex::encode(key.address()))}))
        }
        Request::Capabilities => {
            let capabilities = public_bound_capabilities();
            Ok(json!({
                "status": "ok",
                "backend": "evm-public-bound",
                "suites": capabilities.suites.iter().copied().collect::<Vec<_>>(),
                "modes": capabilities.modes.iter().map(|mode| mode.name()).collect::<Vec<_>>(),
                "guarantees": capabilities.guarantees.iter().map(|guarantee| guarantee.name()).collect::<Vec<_>>(),
                "local_proving": capabilities.local_proving,
            }))
        }
        Request::Receipt {
            deployment,
            state_root,
            operation_ref,
            timeout_ms,
        } => {
            let limits = ObservationLimits {
                log_block_range: deployment.log_block_range,
                max_log_queries: deployment.max_log_queries,
                max_ancestry: deployment.max_ancestry,
            };
            if limits.log_block_range == 0
                || limits.log_block_range > 2_000
                || limits.max_log_queries == 0
                || limits.max_log_queries > 1_024
                || limits.max_ancestry == 0
                || limits.max_ancestry > 8_192
            {
                return Err("invalid observation budget");
            }
            let from_block = deployment.from_block;
            let namespace = ChainNamespace::parse(&deployment.namespace)
                .map_err(|_| "invalid deployment namespace")?;
            let contract = parse_lowercase_address(&deployment.settlement_contract)
                .ok_or("invalid deployment contract")?;
            let deployment = EvmDeployment::new(
                namespace,
                contract,
                deployment.verifier_version,
                deployment.rpc_url,
            )
            .map_err(|_| "invalid deployment")?;
            let operation_ref = parse_operation_ref(&operation_ref)?;
            let timeout = timeout_ms.unwrap_or(15_000);
            if !(100..=30_000).contains(&timeout) {
                return Err("invalid timeout");
            }
            let (terms, blinding, _buyer, _seller) =
                erebus_coordinator::read_disclosure_opening(&state_root, operation_ref)
                    .map_err(|_| "cannot read durable agreement opening")?;
            let nullifier = deal_nullifier(&terms).map_err(|_| "invalid agreement")?;
            deployment
                .matches_domain(&terms.domain)
                .map_err(|_| "deployment does not match agreement")?;
            let journal = ObservationJournal::open(state_root.join("public-observation"))
                .map_err(|_| "public history cache unavailable")?;
            let chain = EvmChain::connect(deployment, Duration::from_millis(timeout))
                .await
                .map_err(|_| "chain unavailable")?;
            let observed = chain
                .finalized_deal_evidence_resumable_from(&journal, &nullifier, from_block, limits)
                .await
                .map_err(|_| "finalized evidence unavailable")?;
            let evidence = match observed {
                HistoricalObservation::Complete { evidence, .. } => evidence,
                HistoricalObservation::Pending {
                    next_log_block,
                    ancestry_block,
                } => {
                    let mut result = json!({
                        "status": "pending", "payment_finalized": false,
                        "deal_nullifier": nullifier.to_hex(), "ancestry_block": ancestry_block,
                    });
                    if let Some(block) = next_log_block {
                        result["next_log_block"] = json!(block);
                    }
                    return Ok(result);
                }
            };
            let revision =
                SignedRevision::from_opening(&terms, &blinding).map_err(|_| "invalid agreement")?;
            let assessment = assess_deal(std::slice::from_ref(&revision), &evidence);
            let revision_state = assessment
                .revisions
                .first()
                .map(|entry| entry.state)
                .ok_or("missing revision assessment")?;

            let mut result = json!({
                "status": "ok",
                "deal_id": hex::encode(terms.deal_id),
                "revision": terms.revision,
                "deal_commitment": revision.commitment().to_hex(),
                "deal_nullifier": nullifier.to_hex(),
                "deal_state": deal_state_name(assessment.state),
                "revision_state": revision_state_name(revision_state),
                "payment_finalized": matches!(assessment.state, DealState::PaidFinalized { .. }),
            });
            if let DealEvidence::Observed(reads) = &evidence {
                result["final_anchor_timestamp"] = json!(reads.final_anchor_timestamp);
                result["consumed_at_final"] = json!(reads.consumed_at_final);
                result["consumed_at_head"] = json!(reads.consumed_at_head);
                result["winner"] = match &reads.winner {
                    Some(winner) => json!({
                        "commitment": winner.commitment.to_hex(),
                        "amount": winner.amount.get().to_string(),
                        "fee": winner.fee.get().to_string(),
                        "is_final": winner.is_final,
                    }),
                    None => Value::Null,
                };
            }
            Ok(result)
        }
        Request::Funding {
            deployment,
            signer_address,
            evidence,
        } => {
            let namespace = ChainNamespace::parse(&deployment.namespace)
                .map_err(|_| "invalid deployment namespace")?;
            let contract = parse_lowercase_address(&deployment.settlement_contract)
                .ok_or("invalid deployment contract")?;
            let signer =
                parse_lowercase_address(&signer_address).ok_or("invalid signer address")?;
            let deployment = EvmDeployment::new(
                namespace,
                contract,
                deployment.verifier_version,
                deployment.rpc_url,
            )
            .map_err(|_| "invalid deployment")?;

            let bytes = hex::decode(&evidence).map_err(|_| "invalid evidence")?;
            let evidence = SettlementEvidence::decode(&bytes).map_err(|_| "invalid evidence")?;
            let terms =
                AgreementTerms::decode(&evidence.terms).map_err(|_| "invalid evidence terms")?;
            let blinding = CommitmentBlinding::from_bytes(evidence.blinding);
            let commitment =
                commit_agreement(&terms, &blinding).map_err(|_| "invalid evidence opening")?;
            let authorization = |role: Role, signature: [u8; 65]| Authorization {
                role,
                suite_id: terms.suite_id,
                commitment,
                signature: SignatureBytes::new(signature.to_vec())
                    .expect("a 65-byte signature is in range"),
            };
            let buyer = authorization(Role::Buyer, evidence.buyer_signature);
            let seller = authorization(Role::Seller, evidence.seller_signature);

            let backend = EvmSettlementBackend::read_only(deployment, signer)
                .map_err(|_| "invalid deployment")?;
            let estimate = backend
                .estimate(&terms, &blinding, &buyer, &seller, [1; 32])
                .await
                .map_err(|_| "funding check failed")?;
            let prepared = backend
                .prepare(&terms, &blinding, &buyer, &seller, [1; 32])
                .map_err(|_| "evidence does not match the deployment")?;
            let gas = backend
                .funding_diagnostics(&prepared)
                .await
                .map_err(|_| "funding check failed")?;

            Ok(json!({
                "status": "ok",
                "required_allowance": estimate.required_allowance.to_string(),
                "allowance": estimate.allowance.to_string(),
                "allowance_shortfall": estimate.allowance_shortfall().to_string(),
                "buyer_balance": estimate.buyer_balance.to_string(),
                "balance_shortfall": estimate.balance_shortfall().to_string(),
                "gas": estimate.gas,
                "signer_required": gas.required.to_string(),
                "signer_balance": gas.balance.to_string(),
                "signer_shortfall": gas.shortfall().to_string(),
                "funded": estimate.allowance_is_sufficient()
                    && estimate.balance_is_sufficient()
                    && gas.is_funded(),
            }))
        }
    }
}

fn parse_operation_ref(text: &str) -> Result<[u8; 32], &'static str> {
    let digits = text.strip_prefix("0x").unwrap_or(text);
    if digits.len() != 64
        || !digits
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("invalid operation reference");
    }
    hex::decode(digits)
        .map_err(|_| "invalid operation reference")?
        .try_into()
        .map_err(|_| "invalid operation reference")
}

fn deal_state_name(state: DealState) -> &'static str {
    match state {
        DealState::Open => "open",
        DealState::PaidIncluded { .. } => "paid_included",
        DealState::PaidFinalized { .. } => "paid_finalized",
        DealState::ConsumedUnresolved => "consumed_unresolved",
        DealState::ClosedUnpaid => "closed_unpaid",
        DealState::UnexplainedConsumption => "unexplained_consumption",
        DealState::Unknown => "unknown",
    }
}

fn revision_state_name(state: RevisionState) -> &'static str {
    match state {
        RevisionState::Open => "open",
        RevisionState::PaidIncluded => "paid_included",
        RevisionState::PaidFinalized => "paid_finalized",
        RevisionState::SupersededIncluded => "superseded_included",
        RevisionState::SupersededFinal => "superseded_final",
        RevisionState::ConsumedUnresolved => "consumed_unresolved",
        RevisionState::ExpiredUnpaid => "expired_unpaid",
        RevisionState::Unknown => "unknown",
    }
}
