//! Public-bound operator driver for an already negotiated and durably authorized deal.
//! Keys and deployment pins come from operator configuration, not an agent request.
//! After the first broadcast attempt, every invocation observes only; it never resends.

use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use erebus_coordinator::{Coordinator, Stage};
use erebus_core::{
    commitment::{commit_agreement, deal_nullifier},
    deal_state::DealState,
    ids::{BaseUnits, ChainNamespace},
    policy::SpendingPolicy,
    settlement::SettlementContext,
    terms::SettlementMode,
};
use erebus_evm::{
    backend::{public_bound_capabilities, EvmSettlementBackend},
    chain::{
        Eip1559Fees, EvmChain, HistoricalObservation, ObservationJournal, ObservationLimits,
        SignedTransaction, SignerJournal, SigningPlan, TransactionKey,
    },
    deployment::{parse_lowercase_address, EvmDeployment},
};
use erebus_transport::{
    disclosure::SelectedAgreement, hashing::TRANSCRIPT_HASH_VERSION, store::FileTranscriptStore,
};
use serde::Deserialize;
use serde_json::{json, Value};
use zeroize::Zeroizing;

const HELP: &str = "erebus-payment: one bounded JSON request on stdin.
Methods: version, funding, settle, observe. Requests use config_file and operation_ref.
This driver supports public-bound payment only. It never downgrades a shielded agreement.
Settle uses retained negotiation and coordinator consent, fixed operator fee caps, and a
local gas-payer key. A durable broadcast attempt disables automatic resubmission forever.
Observe needs no signing key and never submits. Two distinct RPCs and checkpoint stores
must agree on finalized evidence. Pending exits 2; errors exit 1 and retain reservations.
RPC responses, keys, amounts, authorizations, and calldata are excluded from output.
See docs/metropolis-payment-runbook.md.";

#[derive(Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Version {},
    Funding {
        config_file: PathBuf,
        operation_ref: String,
    },
    Settle {
        config_file: PathBuf,
        operation_ref: String,
    },
    Observe {
        config_file: PathBuf,
        operation_ref: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    version: u16,
    state_root: PathBuf,
    namespace: String,
    settlement_contract: String,
    verifier_version: u32,
    runtime_keccak256: String,
    first_block: u64,
    first_hash: String,
    rpc_url: String,
    peer_rpc_url: String,
    buyer_address: String,
    asset: String,
    signer_address: String,
    signer_journal_root: PathBuf,
    transaction_key_file: Option<PathBuf>,
    maximum_price: String,
    gas_limit: u64,
    max_fee_per_gas: String,
    max_priority_fee_per_gas: String,
    timeout_seconds: u64,
    log_block_range: u64,
    max_log_queries: u64,
    max_ancestry: u64,
}

fn read(path: &Path, limit: usize) -> Result<Zeroizing<Vec<u8>>, &'static str> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "local input unavailable")?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return Err("invalid local input");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("local input must have owner-only permissions");
        }
    }
    let file = fs::File::open(path).map_err(|_| "local input unavailable")?;
    let opened = file.metadata().map_err(|_| "local input unavailable")?;
    if !opened.is_file() || opened.len() != metadata.len() {
        return Err("local input changed");
    }
    let mut bytes = Zeroizing::new(Vec::new());
    file.take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "local input unavailable")?;
    if bytes.len() > limit {
        return Err("local input exceeds limit");
    }
    Ok(bytes)
}

fn digest(value: &str) -> Result<[u8; 32], &'static str> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("invalid identity or deployment pin");
    }
    let bytes: [u8; 32] = hex::decode(value)
        .map_err(|_| "invalid identity or deployment pin")?
        .try_into()
        .map_err(|_| "invalid identity or deployment pin")?;
    if bytes == [0; 32] {
        return Err("zero identity or deployment pin");
    }
    Ok(bytes)
}

fn amount(value: &str) -> Result<u128, &'static str> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|b| b.is_ascii_digit())
    {
        return Err("invalid numeric policy");
    }
    value.parse().map_err(|_| "invalid numeric policy")
}

fn clock() -> Result<u64, &'static str> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "local clock unavailable")?
        .as_secs())
}

fn command_lock(root: &Path) -> Result<fs::File, &'static str> {
    if !fs::symlink_metadata(root)
        .map_err(|_| "participant state unavailable")?
        .is_dir()
    {
        return Err("invalid participant state");
    }
    let path = root.join(".payment-driver.lock");
    match fs::symlink_metadata(&path) {
        Ok(meta) if !meta.is_file() => return Err("invalid command lock"),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("command lock unavailable"),
    }
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(|_| "command lock unavailable")?;
    if !file
        .metadata()
        .map_err(|_| "command lock unavailable")?
        .is_file()
    {
        return Err("invalid command lock");
    }
    fs2::FileExt::try_lock_exclusive(&file).map_err(|_| "another payment command is active")?;
    Ok(file)
}

#[tokio::main]
async fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if matches!(args.as_slice(), [arg] if arg == "--help") {
        println!("{HELP}");
        return;
    }
    if !args.is_empty() {
        finish(Err("invalid arguments"));
    }
    let mut input = Zeroizing::new(Vec::new());
    if std::io::stdin()
        .take(16 * 1024 + 1)
        .read_to_end(&mut input)
        .is_err()
        || input.len() > 16 * 1024
    {
        finish(Err("invalid request size"));
    }
    let request = match serde_json::from_slice(&input) {
        Ok(request) => request,
        Err(_) => finish(Err("invalid request")),
    };
    finish(handle(request).await);
}

fn finish(result: Result<Value, &'static str>) -> ! {
    let failed = result.is_err();
    let response = result.unwrap_or_else(|error| {
        json!({"status":"error","error":error,
        "payment_verified":false,"delivery_verified":false,"retry_without_new_payment":true})
    });
    let pending = response["status"] == "pending" || response["status"] == "funding_required";
    println!("{response}");
    let flushed = std::io::stdout().flush().is_ok();
    std::process::exit(if !flushed || failed {
        1
    } else if pending {
        2
    } else {
        0
    });
}

async fn handle(request: Request) -> Result<Value, &'static str> {
    let (path, operation, submit, funding_only) = match request {
        Request::Version {} => {
            return Ok(json!({"status":"ok","protocol_version":1,
            "service":"erebus-payment","modes":["public-bound"],"automatic_rebroadcast":false}))
        }
        Request::Funding {
            config_file,
            operation_ref,
        } => (config_file, operation_ref, false, true),
        Request::Settle {
            config_file,
            operation_ref,
        } => (config_file, operation_ref, true, false),
        Request::Observe {
            config_file,
            operation_ref,
        } => (config_file, operation_ref, false, false),
    };
    let config: Config = serde_json::from_slice(&read(&path, 16 * 1024)?)
        .map_err(|_| "invalid operator configuration")?;
    let operation = digest(&operation)?;
    let runtime_hash = digest(&config.runtime_keccak256)?;
    let first_hash = digest(&config.first_hash)?;
    let signer = parse_lowercase_address(&config.signer_address).ok_or("invalid gas payer")?;
    let buyer_address = parse_lowercase_address(&config.buyer_address).ok_or("invalid buyer")?;
    let contract =
        parse_lowercase_address(&config.settlement_contract).ok_or("invalid deployment")?;
    let maximum = amount(&config.maximum_price)?;
    let fees = Eip1559Fees::new(
        amount(&config.max_fee_per_gas)?,
        amount(&config.max_priority_fee_per_gas)?,
    )
    .map_err(|_| "invalid fee caps")?;
    let gas_budget = (config.gas_limit as u128)
        .checked_mul(fees.max_fee_per_gas())
        .ok_or("gas budget overflow")?;
    if config.version != 1
        || !config.state_root.is_absolute()
        || !config.signer_journal_root.is_absolute()
        || maximum == 0
        || !(1..=30).contains(&config.timeout_seconds)
        || !(21_000..=30_000_000).contains(&config.gas_limit)
        || !(1..=2_000).contains(&config.log_block_range)
        || !(1..=1_024).contains(&config.max_log_queries)
        || !(1..=8_192).contains(&config.max_ancestry)
    {
        return Err("invalid operator policy or observation budget");
    }
    let _guard = command_lock(&config.state_root)?;
    let (terms, blinding, buyer, seller) = erebus_coordinator::read_disclosure_opening(
        config.state_root.join("coordinator"),
        operation,
    )
    .map_err(|_| "durable authorized agreement unavailable")?;
    if terms.suite_id != 1 || terms.settlement_mode != SettlementMode::PublicBound {
        return Err("this driver does not support shielded settlement; no downgrade is allowed");
    }
    if terms.buyer_authorization_key.as_bytes() != buyer_address
        || terms.asset.to_string() != config.asset
        || terms
            .amount
            .get()
            .checked_add(terms.fee_policy.fee.get())
            .is_none_or(|total| total > maximum)
    {
        return Err("agreement exceeds configured buyer or asset policy");
    }
    let namespace =
        ChainNamespace::parse(&config.namespace).map_err(|_| "invalid deployment namespace")?;
    let deployment = EvmDeployment::new(
        namespace,
        contract,
        config.verifier_version,
        config.rpc_url.clone(),
    )
    .map_err(|_| "invalid deployment")?;
    deployment
        .matches_domain(&terms.domain)
        .map_err(|_| "deployment differs from authorized deal")?;
    let context = SettlementContext {
        require_local_proving: true,
        mode: terms.settlement_mode,
        domain: terms.domain.clone(),
        suite_id: terms.suite_id,
        asset: terms.asset.clone(),
        required_guarantees: terms.required_guarantees,
    };
    let coordinator = Coordinator::open(
        config.state_root.join("coordinator"),
        terms.buyer_authorization_key.clone(),
        context.clone(),
        &context,
        &public_bound_capabilities(),
        SpendingPolicy {
            per_deal_max: BaseUnits::new(maximum),
            allowed_assets: [terms.asset.clone()].into(),
            ..Default::default()
        },
    )
    .map_err(|_| "coordinator state or policy unavailable")?;
    let timeout = Duration::from_secs(config.timeout_seconds);
    let chain = EvmChain::connect(deployment.clone(), timeout)
        .await
        .map_err(|_| "primary RPC unavailable")?;
    let mut peer_deployment = deployment.clone();
    peer_deployment.rpc_url = config.peer_rpc_url.clone();
    let peer = EvmChain::connect(peer_deployment, timeout)
        .await
        .map_err(|_| "peer RPC unavailable")?;
    let authentication = Instant::now();
    let first = chain
        .authenticate_deployment_runtime(runtime_hash, config.first_block, first_hash)
        .await
        .map_err(|_| "primary deployment authentication failed")?;
    let second = peer
        .authenticate_deployment_runtime(runtime_hash, config.first_block, first_hash)
        .await
        .map_err(|_| "peer deployment authentication failed")?;
    if first != second {
        return Err("RPC providers disagree on deployment finality");
    }
    let authentication_ms = authentication.elapsed().as_millis();
    let nullifier = deal_nullifier(&terms).map_err(|_| "invalid agreement")?;
    let commitment = commit_agreement(&terms, &blinding).map_err(|_| "invalid agreement")?;
    let journal = ObservationJournal::open(config.state_root.join("payment-observation/primary"))
        .map_err(|_| "primary history checkpoint unavailable")?;
    let peer_journal = ObservationJournal::open(config.state_root.join("payment-observation/peer"))
        .map_err(|_| "peer history checkpoint unavailable")?;
    let observed_at = Instant::now();
    let observed = chain
        .finalized_deal_evidence_resumable_agreed_from(
            &journal,
            &peer,
            &peer_journal,
            &nullifier,
            config.first_block,
            ObservationLimits {
                log_block_range: config.log_block_range,
                max_log_queries: config.max_log_queries,
                max_ancestry: config.max_ancestry,
            },
        )
        .await
        .map_err(|_| "paired finalized evidence unavailable; retain reservations")?;
    let mut response = json!({"status":"pending","mode":"public-bound","agreement_verified":true,
        "payment_verified":false,"delivery_verified":false,"operation_ref":hex::encode(operation),
        "deal_commitment":commitment.to_hex(),"deal_nullifier":nullifier.to_hex(),
        "submitted_this_call":false,"retry_without_new_payment":true,
        "measurements_ms":{"deployment_authentication":authentication_ms,"observation":observed_at.elapsed().as_millis()}});
    let evidence = match observed {
        HistoricalObservation::Pending {
            next_log_block,
            ancestry_block,
        } => {
            response["history_pending"] = json!(true);
            response["next_log_block"] = json!(next_log_block);
            response["ancestry_block"] = json!(ancestry_block);
            return Ok(response);
        }
        HistoricalObservation::Complete { evidence, .. } => evidence,
    };
    let assessment = coordinator
        .reconcile(operation, &evidence, clock()?)
        .map_err(|_| "reconciliation failed; retain reservations")?;
    let stage = coordinator
        .diagnostics()
        .map_err(|_| "operation diagnostic unavailable")?
        .into_iter()
        .find(|d| d.operation_ref == operation)
        .ok_or("missing operation")?;
    response["broadcast_attempts"] = json!(stage.broadcast_attempts);
    response["stage"] = json!(format!("{:?}", stage.stage));
    let nonce_journal =
        SignerJournal::open(&config.signer_journal_root, deployment.chain_id, signer)
            .map_err(|_| "gas-payer nonce journal unavailable")?;
    match assessment.state {
        DealState::PaidFinalized {
            commitment: winning,
        } => {
            response["status"] = json!("ok");
            response["payment_verified"] = json!(winning == commitment);
            response["winning_commitment"] = json!(winning.to_hex());
            if let Some(signed) = coordinator
                .signed_transaction(operation)
                .map_err(|_| "signed recovery state unavailable")?
            {
                let tx = SignedTransaction::from_raw(signed.raw())
                    .map_err(|_| "invalid durable transaction")?;
                response["locally_signed_transaction_hash"] =
                    json!(format!("0x{}", hex::encode(tx.hash())));
            }
            // Payment evidence survives a separate nonce-cleanup failure. Do not resubmit.
            response["nonce_cleanup_pending"] = json!(match chain
                .verified_finalized_nonce_agreed(&peer, signer)
                .await
            {
                Ok(nonce) => nonce_journal.release(&nonce).is_err(),
                Err(_) => true,
            });
            return Ok(response);
        }
        DealState::ClosedUnpaid => {
            response["status"] = json!("closed_unpaid");
            response["nonce_cleanup_pending"] = json!(match chain
                .verified_finalized_nonce_agreed(&peer, signer)
                .await
            {
                Ok(nonce) => nonce_journal.release(&nonce).is_err(),
                Err(_) => true,
            });
            return Ok(response);
        }
        DealState::Open => {}
        _ => return Ok(response),
    }
    // An uncertain, rejected, or acknowledged attempt never becomes permission to send again.
    // Observe can complete with the signer key removed and never requires a proving setup.
    if stage.broadcast_attempts != 0 || (!submit && !funding_only) {
        return Ok(response);
    }
    if !matches!(
        stage.stage,
        Stage::Authorized | Stage::Prepared | Stage::Signed
    ) {
        return Err("operation is not ready for payment");
    }
    let backend = EvmSettlementBackend::read_only(deployment.clone(), signer)
        .map_err(|_| "invalid backend configuration")?;
    let prepared = coordinator
        .prepare(operation, clock()?, |terms, blinding, buyer, seller| {
            backend.prepare(terms, blinding, buyer, seller, operation)
        })
        .map_err(|_| "payment preparation failed; retain reservations")?;
    response["stage"] = json!(format!(
        "{:?}",
        coordinator
            .diagnostics()
            .map_err(|_| "operation diagnostic unavailable")?
            .into_iter()
            .find(|d| d.operation_ref == operation)
            .ok_or("missing operation")?
            .stage
    ));
    let funding_at = Instant::now();
    let estimate = tokio::time::timeout(
        timeout,
        backend.estimate(&terms, &blinding, &buyer, &seller, operation),
    )
    .await
    .map_err(|_| "funding checks timed out")?
    .map_err(|_| "funding checks unavailable")?;
    response["funding"] = json!({"allowance_shortfall":estimate.allowance_shortfall().to_string(),
        "balance_shortfall":estimate.balance_shortfall().to_string(),"estimated_gas":estimate.gas});
    if !estimate.allowance_is_sufficient() || !estimate.balance_is_sufficient() {
        response["status"] = json!("funding_required");
        return Ok(response);
    }
    let funding = tokio::time::timeout(timeout, backend.funding_diagnostics(&prepared))
        .await
        .map_err(|_| "gas funding check timed out")?
        .map_err(|_| "gas funding check unavailable")?;
    response["funding"]["signer_shortfall"] =
        json!(gas_budget.saturating_sub(funding.balance).to_string());
    response["funding"]["gas_limit_sufficient"] = json!(estimate.gas <= config.gas_limit);
    response["measurements_ms"]["funding"] = json!(funding_at.elapsed().as_millis());
    if estimate.gas > config.gas_limit || funding.balance < gas_budget {
        response["status"] = json!("funding_required");
        return Ok(response);
    }
    if funding_only {
        response["status"] = json!("ready");
        return Ok(response);
    }
    let transaction_at = Instant::now();
    if coordinator
        .signed_transaction(operation)
        .map_err(|_| "signed state unavailable")?
        .is_none()
    {
        // Reconstruct and authenticate the complete retained negotiation before the first
        // transaction signature. Recovery after a broadcast does not need this transcript.
        let transcripts = FileTranscriptStore::open(config.state_root.join("transcripts"))
            .map_err(|_| "negotiation transcript unavailable")?;
        SelectedAgreement::from_store(
            terms,
            blinding,
            buyer,
            seller,
            TRANSCRIPT_HASH_VERSION,
            &transcripts,
            "agent",
        )
        .map_err(|_| "authorized agreement does not match retained negotiation")?;
        let path = config
            .transaction_key_file
            .as_ref()
            .ok_or("local transaction key not configured")?;
        let bytes = read(path, 32)?;
        let seed = Zeroizing::new(
            bytes
                .as_slice()
                .try_into()
                .map_err(|_| "transaction key must be 32 raw bytes")?,
        );
        let key = TransactionKey::from_bytes(&seed).map_err(|_| "invalid transaction key")?;
        if key.address() != signer {
            return Err("transaction key differs from configured gas payer");
        }
        let plan = chain
            .reserve_nonce(&nonce_journal, &prepared, fees, config.gas_limit)
            .await
            .map_err(|_| "gas-payer nonce unavailable; retain existing claim")?;
        coordinator
            .sign_transaction(
                operation,
                clock()?,
                &plan.encode(),
                |prepared, bytes| {
                    SigningPlan::decode(bytes)?
                        .sign(&deployment, prepared, &key)
                        .map(|tx| tx.raw().to_vec())
                },
                |prepared, bytes, raw| {
                    SigningPlan::decode(bytes)?.validate(&deployment, prepared, raw)
                },
            )
            .map_err(|_| "local signing failed; retain nonce and reservations")?;
    }
    let signed = coordinator
        .signed_transaction(operation)
        .map_err(|_| "signed state unavailable")?
        .ok_or("signed transaction unavailable")?;
    let plan = SigningPlan::decode(signed.plan()).map_err(|_| "invalid durable signing plan")?;
    if plan.sender() != signer
        || plan.params().fees != fees
        || plan.params().gas_limit != config.gas_limit
    {
        return Err("configured signing policy differs from durable plan");
    }
    plan.validate(&deployment, &prepared, signed.raw())
        .map_err(|_| "durable transaction differs from agreement")?;
    let tx =
        SignedTransaction::from_raw(signed.raw()).map_err(|_| "invalid durable transaction")?;
    response["transaction_hash"] = json!(format!("0x{}", hex::encode(tx.hash())));
    response["measurements_ms"]["local_signing"] = json!(transaction_at.elapsed().as_millis());
    let submission_at = Instant::now();
    chain
        .broadcast_initial_journaled(&coordinator, operation, clock()?)
        .await
        .map_err(|_| "submission state uncertain; observe without another payment")?;
    response["submitted_this_call"] = json!(true);
    response["measurements_ms"]["submission"] = json!(submission_at.elapsed().as_millis());
    let submitted = coordinator
        .diagnostics()
        .map_err(|_| "submission diagnostic unavailable; observe without another payment")?
        .into_iter()
        .find(|d| d.operation_ref == operation)
        .ok_or("missing operation")?;
    response["stage"] = json!(format!("{:?}", submitted.stage));
    response["broadcast_attempts"] = json!(submitted.broadcast_attempts);
    Ok(response)
}
