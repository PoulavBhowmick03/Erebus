//! Native suite-2 driver. Private note openings remain in the encrypted wallet.

use super::*;
use erebus_core::settlement::PreparedSettlement;
use erebus_evm::chain::NonceError;
use erebus_shielded_prover::{
    artifacts::{ArtifactRelease, CircuitKind, InstallOptions, MAX_MANIFEST_BYTES},
    chain::ShieldedChain,
    gas_policy::{GasPolicy, SETTLE_GAS_LIMIT},
    index_store::{IndexDomain, IndexStore},
    observation::{observe_shielded_deal_agreed_bounded, ObservationError},
    preparation::{capabilities, prepare_coordinated_transfer, validate_prepared},
    recovery::{recover_finalized_wallet_agreed_bounded, RecoveryError},
    rpc::{PoolDependencyPins, PoolRpc},
    wallet::{WalletDomain, WalletStore},
    WalletTransferRequest,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShieldedConfig {
    version: u16,
    mode: String,
    state_root: PathBuf,
    namespace: String,
    pool: String,
    verifier_version: u32,
    runtime_keccak256: String,
    poseidon_keccak256: String,
    deposit_verifier_keccak256: String,
    transfer_verifier_keccak256: String,
    withdraw_verifier_keccak256: String,
    first_block: u64,
    first_hash: String,
    rpc_url: String,
    peer_rpc_url: String,
    #[serde(default)]
    rpc_requests_per_second: Option<u32>,
    #[serde(default)]
    peer_rpc_requests_per_second: Option<u32>,
    buyer_key_hex: String,
    asset: String,
    signer_address: String,
    signer_journal_root: PathBuf,
    transaction_key_file: Option<PathBuf>,
    wallet_file: PathBuf,
    wallet_key_file: PathBuf,
    manifest_file: PathBuf,
    manifest_sha256: String,
    #[serde(default)]
    manifest_authorization: Option<erebus_shielded_prover::artifacts::ManifestAuthorization>,
    artifact_cache: PathBuf,
    #[serde(default)]
    allow_test_artifacts: bool,
    #[serde(default)]
    allow_loopback_http: bool,
    maximum_price: String,
    gas_limit: u64,
    max_fee_per_gas: String,
    max_priority_fee_per_gas: String,
    timeout_seconds: u64,
    max_scan_blocks: u64,
    #[serde(default)]
    relay: Option<erebus_shielded_prover::relay_client::RelayEndpoint>,
}

fn pending(response: &mut Value, error: &RecoveryError) -> bool {
    if let RecoveryError::HistoryPending {
        next_block,
        through,
    } = error
    {
        response["history_pending"] = json!(true);
        response["next_log_block"] = json!(next_block);
        response["through"] = json!(through);
        true
    } else {
        false
    }
}

pub(super) async fn run(
    bytes: &[u8],
    operation: [u8; 32],
    submit: bool,
    funding_only: bool,
) -> Result<Value, &'static str> {
    let config: ShieldedConfig =
        serde_json::from_slice(bytes).map_err(|_| "invalid shielded operator configuration")?;
    validate_read_rates(
        config.rpc_requests_per_second,
        config.peer_rpc_requests_per_second,
    )?;
    let maximum = amount(&config.maximum_price)?;
    let pool = parse_lowercase_address(&config.pool).ok_or("invalid shielded deployment")?;
    let signer = parse_lowercase_address(&config.signer_address).ok_or("invalid gas payer")?;
    let gas_sender = if let Some(relay) = &config.relay {
        relay.validate()?;
        digest(&relay.identity)?;
        if config.transaction_key_file.is_some() {
            return Err("relay mode cannot configure a buyer gas key");
        }
        parse_lowercase_address(&relay.sponsor_address)
            .filter(|address| *address != [0; 20])
            .ok_or("invalid relay sponsor")?
    } else {
        signer
    };
    let buyer_key =
        hex::decode(&config.buyer_key_hex).map_err(|_| "invalid buyer agreement key")?;
    if buyer_key.len() != 64
        || hex::encode(&buyer_key) != config.buyer_key_hex
        || config.version != 1
        || config.mode != "shielded"
        || maximum == 0
        || !(1..=30).contains(&config.timeout_seconds)
        || !(1..=1000).contains(&config.max_scan_blocks)
        || !(21_000..=30_000_000).contains(&config.gas_limit)
        || [
            &config.state_root,
            &config.signer_journal_root,
            &config.wallet_file,
            &config.wallet_key_file,
            &config.manifest_file,
            &config.artifact_cache,
        ]
        .iter()
        .any(|path| !path.is_absolute())
        || config
            .transaction_key_file
            .as_ref()
            .is_some_and(|path| !path.is_absolute())
    {
        return Err("invalid shielded operator policy");
    }
    let fees = Eip1559Fees::new(
        amount(&config.max_fee_per_gas)?,
        amount(&config.max_priority_fee_per_gas)?,
    )
    .map_err(|_| "invalid fee caps")?;
    // A new settlement signs at the settle method's limit (the configured gas_limit is the cap for
    // a plan saved before per-method limits); the funding check below uses the effective limit.
    let gas_policy = GasPolicy {
        limit: SETTLE_GAS_LIMIT,
        legacy_cap: config.gas_limit,
    };
    let runtime = digest(&config.runtime_keccak256)?;
    let first_hash = digest(&config.first_hash)?;
    let pins = PoolDependencyPins {
        poseidon: digest(&config.poseidon_keccak256)?,
        deposit: digest(&config.deposit_verifier_keccak256)?,
        transfer: digest(&config.transfer_verifier_keccak256)?,
        withdraw: digest(&config.withdraw_verifier_keccak256)?,
    };
    let _guard = command_lock(&config.state_root)?;
    let (terms, blinding, buyer, seller) = erebus_coordinator::read_disclosure_opening(
        config.state_root.join("coordinator"),
        operation,
    )
    .map_err(|_| "durable authorized agreement unavailable")?;
    if terms.suite_id != 2
        || terms.settlement_mode != SettlementMode::Shielded
        || terms.domain.namespace.to_string() != config.namespace
        || terms.domain.verifier_version != config.verifier_version
        || terms.domain.pool.as_ref().map(|p| p.as_bytes()) != Some(pool.as_slice())
        || terms.buyer_authorization_key.as_bytes() != buyer_key
        || terms.asset.to_string() != config.asset
        || terms.amount.get() > maximum
        || terms.fee_policy.fee.get() != 0
    {
        return Err("agreement differs from shielded operator policy; no downgrade is allowed");
    }
    let context = SettlementContext {
        domain: terms.domain.clone(),
        asset: terms.asset.clone(),
        suite_id: 2,
        mode: SettlementMode::Shielded,
        required_guarantees: terms.required_guarantees,
        require_local_proving: true,
    };
    let coordinator = Coordinator::open(
        config.state_root.join("coordinator"),
        terms.buyer_authorization_key.clone(),
        context.clone(),
        &context,
        &capabilities(),
        SpendingPolicy {
            per_deal_max: BaseUnits::new(maximum),
            allowed_assets: [terms.asset.clone()].into(),
            ..Default::default()
        },
    )
    .map_err(|_| "coordinator state or policy unavailable")?;
    let chain_id = terms
        .domain
        .namespace
        .reference()
        .parse::<u64>()
        .map_err(|_| "invalid chain namespace")?;
    let timeout = Duration::from_secs(config.timeout_seconds);
    let chain = ShieldedChain::connect(context.clone(), &config.rpc_url, timeout)
        .await
        .map_err(|_| "primary RPC unavailable")?;
    let peer = ShieldedChain::connect(context.clone(), &config.peer_rpc_url, timeout)
        .await
        .map_err(|_| "peer RPC unavailable")?;
    let rpc = PoolRpc::with_timeout(&config.rpc_url, chain_id, pool, timeout)
        .and_then(|rpc| {
            rpc.with_max_requests_per_second(
                config
                    .rpc_requests_per_second
                    .unwrap_or(erebus_shielded_prover::rpc::DEFAULT_MAX_REQUESTS_PER_SECOND),
            )
        })
        .map_err(|_| "invalid primary RPC")?;
    let peer_rpc = PoolRpc::with_timeout(&config.peer_rpc_url, chain_id, pool, timeout)
        .and_then(|rpc| {
            rpc.with_max_requests_per_second(
                config
                    .peer_rpc_requests_per_second
                    .unwrap_or(erebus_shielded_prover::rpc::DEFAULT_MAX_REQUESTS_PER_SECOND),
            )
        })
        .map_err(|_| "invalid peer RPC")?;
    if rpc.shares_endpoint(&peer_rpc) {
        return Err("shielded recovery requires distinct RPC endpoints");
    }
    let started = Instant::now();
    let first = chain
        .evm()
        .authenticate_deployment_runtime_agreed(peer.evm(), runtime, config.first_block, first_hash)
        .await
        .map_err(|_| "paired deployment authentication failed")?;
    rpc.authenticate_dependencies_at(first.hash, pins)
        .await
        .map_err(|_| "primary pool dependency authentication failed")?;
    peer_rpc
        .authenticate_dependencies_at(first.hash, pins)
        .await
        .map_err(|_| "peer pool dependency authentication failed")?;
    let mut response = json!({"status":"pending","mode":"shielded","agreement_verified":true,"payment_verified":false,
        "delivery_verified":false,"operation_ref":hex::encode(operation),"deal_commitment":commit_agreement(&terms,&blinding).map_err(|_|"invalid agreement")?.to_hex(),
        "deal_nullifier":deal_nullifier(&terms).map_err(|_|"invalid agreement")?.to_hex(),"submitted_this_call":false,"retry_without_new_payment":true,
        "measurements_ms":{"deployment_authentication":started.elapsed().as_millis()}});
    let domain = IndexDomain {
        chain_id,
        pool,
        first_block: config.first_block,
        first_hash,
    };
    // An explicit fresh-participant checkpoint, if recorded, bounds where observation starts.
    let checkpoint = erebus_shielded_prover::checkpoint::load(&config.state_root, domain)
        .map_err(|_| "history checkpoint record unavailable")?;
    let index = |directory: &str, name: &str| {
        IndexStore::with_checkpoint(
            config.state_root.join(directory).join(name),
            domain,
            checkpoint,
        )
        .map_err(|_| "pool checkpoint unavailable")
    };
    // Wallet recovery shares the wallet's own finalized-only caches, so a buyer replays pool
    // history once. It runs first; observation, which also tracks the non-final head and keeps
    // its own caches, then starts from the wallet's just-verified progress, so the new finalized
    // blocks are scanned once, not once per cache. Seeding copies verified caches and adds no
    // trust; observation keeps all of its own anchor and finality checks.
    let wallet_primary = index("wallet-index", "primary.json")?;
    let wallet_peer = index("wallet-index", "peer.json")?;
    let primary_index = index("payment-observation", "shielded-primary.json")?;
    let peer_index = index("payment-observation", "shielded-peer.json")?;
    let (_, revisions) = coordinator
        .recorded_deal(operation)
        .map_err(|_| "durable revisions unavailable")?;
    let nullifier = deal_nullifier(&terms).map_err(|_| "invalid agreement")?;
    let wallet_key = read(&config.wallet_key_file, 32)?;
    let wallet = WalletStore::new(
        &config.wallet_file,
        WalletDomain {
            chain_id,
            pool,
            verifier_version: config.verifier_version,
        },
        wallet_key
            .as_slice()
            .try_into()
            .map_err(|_| "wallet key must be 32 raw bytes")?,
    )
    .map_err(|_| "encrypted wallet unavailable")?;
    match recover_finalized_wallet_agreed_bounded(
        &rpc,
        &wallet_primary,
        &peer_rpc,
        &wallet_peer,
        &wallet,
        config.max_scan_blocks,
    )
    .await
    {
        Ok(_) => {}
        Err(error) if pending(&mut response, &error) => return Ok(response),
        Err(error) if error.is_rate_limited() => {
            return Err("RPC provider rate limited; retain reservations and retry later")
        }
        Err(_) => return Err("paired finalized wallet recovery failed; retain reservations"),
    }
    for (observation, wallet) in [
        (&primary_index, &wallet_primary),
        (&peer_index, &wallet_peer),
    ] {
        observation
            .seed_from(wallet)
            .map_err(|_| "pool history caches unavailable")?;
    }
    let started = Instant::now();
    let evidence = match observe_shielded_deal_agreed_bounded(
        &rpc,
        &primary_index,
        &peer_rpc,
        &peer_index,
        &context,
        &nullifier,
        &revisions,
        config.max_scan_blocks,
    )
    .await
    {
        Ok(evidence) => evidence,
        Err(ObservationError::Recovery(error)) if pending(&mut response, &error) => {
            return Ok(response)
        }
        Err(error) if error.is_rate_limited() => {
            return Err("RPC provider rate limited; retain reservations and retry later")
        }
        Err(_) => return Err("paired pool evidence unavailable; retain reservations"),
    };
    response["measurements_ms"]["observation"] = json!(started.elapsed().as_millis());
    let assessment = coordinator
        .reconcile(operation, &evidence, clock()?)
        .map_err(|_| "reconciliation failed; retain reservations")?;
    let stage = coordinator
        .diagnostics()
        .map_err(|_| "operation diagnostic unavailable")?
        .into_iter()
        .find(|d| d.operation_ref == operation)
        .ok_or("missing operation")?;
    response["stage"] = json!(format!("{:?}", stage.stage));
    response["broadcast_attempts"] = json!(stage.broadcast_attempts);
    response["submission_delegated"] = json!(stage.submission_delegated);
    let journal = SignerJournal::open(&config.signer_journal_root, chain_id, signer)
        .map_err(|_| "gas-payer nonce journal unavailable")?;
    match assessment.state {
        DealState::PaidFinalized {
            commitment: winning,
        } => {
            response["status"] = json!("ok");
            response["payment_verified"] = json!(winning.to_hex() == response["deal_commitment"]);
            response["winning_commitment"] = json!(winning.to_hex());
            // Without a claim there is nothing to release: skip the paired finalized-nonce reads.
            response["nonce_cleanup_pending"] = json!(match journal.claim_nonce() {
                Ok(None) => false,
                Ok(Some(_)) => match chain
                    .evm()
                    .verified_finalized_nonce_agreed(peer.evm(), signer)
                    .await
                {
                    Ok(nonce) => journal.release(&nonce).is_err(),
                    Err(_) => true,
                },
                Err(_) => true,
            });
            return Ok(response);
        }
        DealState::ClosedUnpaid => {
            response["status"] = json!("closed_unpaid");
            // Wallet inputs stay reserved until explicit no-effect reconciliation.
            response["wallet_release_required"] = json!(true);
            return Ok(response);
        }
        DealState::Open => {}
        _ => return Ok(response),
    }
    if stage.submission_delegated {
        if submit {
            if let Some(relay) = &config.relay {
                let prepared = coordinator
                    .prepared_settlement(operation)
                    .map_err(|_| "delegated proof unavailable; retain reservations")?;
                submit_relay(
                    &coordinator,
                    operation,
                    relay,
                    &context,
                    &prepared,
                    &mut response,
                )
                .await?;
            }
        }
        return Ok(response);
    }
    if stage.broadcast_attempts != 0 || (!submit && !funding_only) {
        return Ok(response);
    }
    if !matches!(
        stage.stage,
        Stage::Authorized | Stage::Prepared | Stage::Signed
    ) {
        return Err("operation is not ready for payment");
    }
    // The gas this settlement will actually sign or send at: a plan already saved for it (a
    // coordinator-retained signed transaction, or the signer journal's claim for its prepared
    // call) keeps its own limit; only a new plan uses the settle limit. Funding is checked
    // against that limit, so a resumed larger legacy plan is never sent underfunded.
    let effective_limit = if config.relay.is_some() {
        SETTLE_GAS_LIMIT
    } else {
        match saved_gas_limit(&coordinator, &journal, chain.evm(), operation)? {
            Some(saved) if gas_policy.resumes(saved) => saved,
            Some(_) => return Err("configured signing policy differs from durable plan"),
            None => SETTLE_GAS_LIMIT,
        }
    };
    let gas_budget = (effective_limit as u128)
        .checked_mul(fees.max_fee_per_gas())
        .ok_or("gas budget overflow")?;
    let started = Instant::now();
    let balance = chain
        .evm()
        .gas_payer_balance(gas_sender)
        .await
        .map_err(|_| "gas funding check unavailable")?;
    let snapshot = wallet.snapshot().map_err(|_| "wallet unavailable")?;
    let asset = parse_lowercase_address(terms.asset.asset_reference()).ok_or("invalid asset")?;
    let available = snapshot.notes().iter().any(|note| {
        note.asset() == asset
            && note.owner().as_slice() == buyer_key
            && note.amount() >= terms.amount.get()
            && snapshot
                .spendable_for(&note.commitment(), operation)
                .is_some()
    });
    response["funding"] = json!({"funded_note_available":available,"signer_shortfall":gas_budget.saturating_sub(balance).to_string()});
    response["measurements_ms"]["funding"] = json!(started.elapsed().as_millis());
    if !available || balance < gas_budget {
        response["status"] = json!("funding_required");
        return Ok(response);
    }
    if funding_only {
        response["status"] = json!("ready");
        response["proof_required"] = json!(stage.stage == Stage::Authorized);
        return Ok(response);
    }
    let prepared = if stage.stage == Stage::Authorized {
        let transcripts = FileTranscriptStore::open(config.state_root.join("transcripts"))
            .map_err(|_| "negotiation transcript unavailable")?;
        SelectedAgreement::from_store(
            terms.clone(),
            blinding.clone(),
            buyer.clone(),
            seller.clone(),
            TRANSCRIPT_HASH_VERSION,
            &transcripts,
            "agent",
        )
        .map_err(|_| "authorized agreement does not match retained negotiation")?;
        let release = ArtifactRelease::authenticate_authorized(
            &read(&config.manifest_file, MAX_MANIFEST_BYTES)?,
            digest(&config.manifest_sha256)?,
            config.manifest_authorization.as_ref(),
        )
        .map_err(|_| "artifact manifest not authenticated")?;
        if !release.matches_deployment(chain_id, pool, config.verifier_version)
            || !release.matches_verifier_pins(&pins)
        {
            return Err("artifact release differs from authorized deployment");
        }
        let started = Instant::now();
        let installed = release
            .install(
                CircuitKind::Transfer,
                &InstallOptions {
                    cache_root: config.artifact_cache,
                    timeout: Duration::from_secs(120),
                    allow_test_artifacts: config.allow_test_artifacts,
                    allow_loopback_http: config.allow_loopback_http,
                },
            )
            .await
            .map_err(|_| {
                "artifact installation failed; check trusted release and development opt-ins"
            })?;
        response["measurements_ms"]["artifact_installation"] = json!(started.elapsed().as_millis());
        response["downloaded_bytes"] = json!(installed.downloaded_bytes);
        let choice = wallet
            .update(|wallet| wallet.choose_transfer(&terms, &blinding, operation))
            .map_err(|_| "durable input selection failed")?;
        let change = choice.change();
        let index = wallet_primary
            .load()
            .map_err(|_| "verified wallet history unavailable")?;
        let snapshot = wallet.snapshot().map_err(|_| "wallet unavailable")?;
        let request = WalletTransferRequest {
            terms: &terms,
            blinding: &blinding,
            buyer: &buyer,
            seller: &seller,
            wallet: &snapshot,
            index: &index,
            change: &change,
            now: clock()?,
        };
        let started = Instant::now();
        let now = clock()?;
        let prepared = tokio::task::block_in_place(|| {
            prepare_coordinated_transfer(
                &coordinator,
                &context,
                &installed.artifacts,
                &wallet,
                &request,
                choice.input(),
                operation,
                now,
            )
        })
        .map_err(|_| {
            "local proof preparation failed; retain original input and change for retry"
        })?;
        response["measurements_ms"]["proof_preparation"] = json!(started.elapsed().as_millis());
        prepared
    } else {
        coordinator
            .prepared_settlement(operation)
            .map_err(|_| "durable proof preparation unavailable")?
    };
    validate_prepared(&context, &prepared)
        .map_err(|_| "prepared proof differs from authorized agreement")?;
    if let Some(relay) = &config.relay {
        submit_relay(
            &coordinator,
            operation,
            relay,
            &context,
            &prepared,
            &mut response,
        )
        .await?;
        return Ok(response);
    }
    let gas = chain
        .evm()
        .estimate_call_gas(signer, pool, &prepared.backend_evidence)
        .await
        .map_err(|_| "pool call estimate failed; retain reservations")?;
    response["funding"]["estimated_gas"] = json!(gas);
    // A claim already saved for this exact call (including one from before per-method limits)
    // resumes at its own limit within the cap. Otherwise the estimate plus margin must fit under
    // the settle limit, or this refuses before any claim is written or byte is signed.
    // Signing must use exactly the limit funding was checked against: a claim that appeared or
    // changed since (another process outside this driver's lock) refuses, retryably.
    let signed_already = coordinator
        .signed_transaction(operation)
        .map_err(|_| "signed state unavailable")?
        .is_some();
    let gas_limit = if signed_already {
        effective_limit
    } else {
        let saved = match journal.resume_call(
            chain.evm().deployment(),
            &prepared,
            chain.evm().deployment().settlement_contract,
            &prepared.backend_evidence,
        ) {
            Ok(plan) => plan.map(|plan| plan.params().gas_limit),
            Err(NonceError::Busy) => return Err("signer lane is busy; sync the wallet and retry"),
            Err(_) => return Err("gas-payer nonce journal unavailable"),
        };
        match signing_gas_limit(saved, effective_limit, gas_policy, gas)? {
            Some(limit) => limit,
            None => {
                response["funding"]["gas_limit_sufficient"] = json!(false);
                response["status"] = json!("funding_required");
                return Ok(response);
            }
        }
    };
    let started = Instant::now();
    if coordinator
        .signed_transaction(operation)
        .map_err(|_| "signed state unavailable")?
        .is_none()
    {
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
        let seed = read(
            config
                .transaction_key_file
                .as_ref()
                .ok_or("local transaction key not configured")?,
            32,
        )?;
        let key = TransactionKey::from_bytes(
            seed.as_slice()
                .try_into()
                .map_err(|_| "transaction key must be 32 raw bytes")?,
        )
        .map_err(|_| "invalid transaction key")?;
        if key.address() != signer {
            return Err("transaction key differs from configured gas payer");
        }
        chain
            .sign(
                &coordinator,
                &journal,
                operation,
                &key,
                clock()?,
                fees,
                gas_limit,
            )
            .await
            .map_err(|error| match error {
                erebus_shielded_prover::chain::ShieldedChainError::Nonce(NonceError::Busy) => {
                    "signer lane is busy; sync the wallet and retry"
                }
                _ => "local signing failed; retain nonce and reservations",
            })?;
    }
    let signed = coordinator
        .signed_transaction(operation)
        .map_err(|_| "signed state unavailable")?
        .ok_or("signed transaction unavailable")?;
    let plan = SigningPlan::decode(signed.plan()).map_err(|_| "invalid durable signing plan")?;
    if plan.sender() != signer
        || plan.params().fees != fees
        || !gas_policy.resumes(plan.params().gas_limit)
    {
        return Err("configured signing policy differs from durable plan");
    }
    if plan.params().gas_limit != gas_limit {
        return Err(FUNDING_CHANGED);
    }
    plan.validate_call(chain_id, pool, &prepared.backend_evidence, signed.raw())
        .map_err(|_| "durable transaction differs from agreement")?;
    response["transaction_hash"] = json!(format!(
        "0x{}",
        hex::encode(
            SignedTransaction::from_raw(signed.raw())
                .map_err(|_| "invalid durable transaction")?
                .hash()
        )
    ));
    response["measurements_ms"]["local_signing"] = json!(started.elapsed().as_millis());
    let started = Instant::now();
    chain
        .broadcast_initial(&coordinator, operation, clock()?)
        .await
        .map_err(|_| "submission state uncertain; observe without another payment")?;
    response["measurements_ms"]["submission"] = json!(started.elapsed().as_millis());
    response["submitted_this_call"] = json!(true);
    let submitted = coordinator
        .diagnostics()
        .map_err(|_| "submission persisted; diagnostic unavailable, observe without resending")?
        .into_iter()
        .find(|d| d.operation_ref == operation)
        .ok_or("submission persisted; missing diagnostic, observe without resending")?;
    response["stage"] = json!(format!("{:?}", submitted.stage));
    response["broadcast_attempts"] = json!(submitted.broadcast_attempts);
    Ok(response)
}

const FUNDING_CHANGED: &str =
    "signer plan changed after the funding check; nothing was signed; retry the same operation_ref";

/// The limit to sign at, given the claim saved for this call (if any), the limit funding was
/// checked against, and the live estimate. `Ok(None)` means the estimate does not fit a new plan
/// (funding_required). A limit other than the funded one refuses retryably, before signing.
fn signing_gas_limit(
    saved: Option<u64>,
    funded: u64,
    policy: GasPolicy,
    estimate: u64,
) -> Result<Option<u64>, &'static str> {
    let limit = match saved {
        Some(saved) if !policy.resumes(saved) => {
            return Err("configured signing policy differs from durable plan")
        }
        Some(saved) => saved,
        None if !policy.admits(estimate) => return Ok(None),
        None => policy.limit,
    };
    if limit != funded {
        return Err(FUNDING_CHANGED);
    }
    Ok(Some(limit))
}

/// The gas limit of a plan already saved for this operation: the coordinator's signed
/// transaction, or else the signer journal's claim for its prepared call. `None` for a new plan.
fn saved_gas_limit(
    coordinator: &Coordinator,
    journal: &SignerJournal,
    chain: &EvmChain,
    operation: [u8; 32],
) -> Result<Option<u64>, &'static str> {
    if let Some(signed) = coordinator
        .signed_transaction(operation)
        .map_err(|_| "signed state unavailable")?
    {
        let plan =
            SigningPlan::decode(signed.plan()).map_err(|_| "invalid durable signing plan")?;
        return Ok(Some(plan.params().gas_limit));
    }
    let Ok(prepared) = coordinator.prepared_settlement(operation) else {
        return Ok(None);
    };
    match journal.resume_call(
        chain.deployment(),
        &prepared,
        chain.deployment().settlement_contract,
        &prepared.backend_evidence,
    ) {
        Ok(plan) => Ok(plan.map(|plan| plan.params().gas_limit)),
        Err(NonceError::Busy) => Ok(None),
        Err(_) => Err("gas-payer nonce journal unavailable"),
    }
}

fn validate_read_rates(primary: Option<u32>, peer: Option<u32>) -> Result<(), &'static str> {
    if [primary, peer].into_iter().flatten().any(|rate| {
        rate == 0 || rate > erebus_shielded_prover::rpc::DEFAULT_MAX_REQUESTS_PER_SECOND
    }) {
        return Err("invalid shielded RPC read rate");
    }
    Ok(())
}

#[cfg(test)]
mod signing_gas_tests {
    use super::*;

    const POLICY: GasPolicy = GasPolicy {
        limit: SETTLE_GAS_LIMIT,
        legacy_cap: 5_000_000,
    };

    #[test]
    fn signing_uses_exactly_the_funded_limit() {
        // New plan, funded at the settle limit.
        assert_eq!(
            signing_gas_limit(None, SETTLE_GAS_LIMIT, POLICY, 2_733_865),
            Ok(Some(SETTLE_GAS_LIMIT))
        );
        // A saved legacy plan funded at its own limit.
        assert_eq!(
            signing_gas_limit(Some(5_000_000), 5_000_000, POLICY, 2_733_865),
            Ok(Some(5_000_000))
        );
        // An estimate that does not fit a new plan is funding_required.
        assert_eq!(
            signing_gas_limit(None, SETTLE_GAS_LIMIT, POLICY, 2_900_000),
            Ok(None)
        );
    }

    #[test]
    fn a_claim_that_changed_after_the_funding_check_refuses_before_signing() {
        // Funded for a new 3,010,000 plan, but a 5,000,000 claim appeared before signing.
        assert_eq!(
            signing_gas_limit(Some(5_000_000), SETTLE_GAS_LIMIT, POLICY, 2_733_865),
            Err(FUNDING_CHANGED)
        );
        // Funded for a saved 5,000,000 plan that is gone by signing time.
        assert_eq!(
            signing_gas_limit(None, 5_000_000, POLICY, 2_733_865),
            Err(FUNDING_CHANGED)
        );
        // Above the cap is the durable-policy refusal, not a funding change.
        assert_eq!(
            signing_gas_limit(Some(6_000_000), 6_000_000, POLICY, 1),
            Err("configured signing policy differs from durable plan")
        );
    }
}

#[cfg(test)]
mod read_rate_tests {
    #[test]
    fn rates_are_optional_bounded_and_independent() {
        assert!(super::validate_read_rates(None, None).is_ok());
        for rate in [1, 10, 40] {
            assert!(super::validate_read_rates(Some(rate), None).is_ok());
            assert!(super::validate_read_rates(None, Some(rate)).is_ok());
        }
        assert!(super::validate_read_rates(Some(40), Some(10)).is_ok());
        for rate in [0, 41, u32::MAX] {
            assert!(super::validate_read_rates(Some(rate), Some(10)).is_err());
            assert!(super::validate_read_rates(Some(40), Some(rate)).is_err());
        }
    }
}

async fn submit_relay(
    coordinator: &Coordinator,
    operation: [u8; 32],
    relay: &erebus_shielded_prover::relay_client::RelayEndpoint,
    context: &SettlementContext,
    prepared: &PreparedSettlement,
    response: &mut Value,
) -> Result<(), &'static str> {
    // No outbound byte is authorized until the durable buyer fence has succeeded.
    let relay_identity = erebus_core::suite::keccak256(&[
        b"erebus-relay-destination-v1",
        &digest(&relay.identity)?,
        relay.url.as_bytes(),
        relay.sponsor_address.as_bytes(),
    ]);
    coordinator
        .delegate_submission(operation, relay_identity, clock()?)
        .map_err(|_| "relay delegation unavailable; retain reservations")?;
    response["submission_delegated"] = json!(true);
    let bytes = read(&relay.authentication_key_file, 32)?;
    let token: &[u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| "relay key must be 32 raw bytes")?;
    let started = Instant::now();
    let reply = relay.submit(context, prepared, token).await?;
    response["measurements_ms"]["relay_submission"] = json!(started.elapsed().as_millis());
    response["relay_status"] = json!("unavailable");
    if let Some(reply) = reply {
        response["relay_operation_ref"] = json!(reply.operation_ref);
        response["relay_status"] = json!(reply.status);
        if let Some(hash) = reply.transaction_hash {
            response["relay_transaction_hash"] = json!(hash);
        }
    }
    // Remote finality is not buyer-verified payment evidence. Recovery observes locally.
    Ok(())
}
