//! Native suite-2 driver. Private note openings remain in the encrypted wallet.

use super::*;
use erebus_shielded_prover::{
    artifacts::{ArtifactRelease, CircuitKind, InstallOptions, MAX_MANIFEST_BYTES},
    chain::ShieldedChain,
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
    buyer_key_hex: String,
    asset: String,
    signer_address: String,
    signer_journal_root: PathBuf,
    transaction_key_file: Option<PathBuf>,
    wallet_file: PathBuf,
    wallet_key_file: PathBuf,
    manifest_file: PathBuf,
    manifest_sha256: String,
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
    let maximum = amount(&config.maximum_price)?;
    let pool = parse_lowercase_address(&config.pool).ok_or("invalid shielded deployment")?;
    let signer = parse_lowercase_address(&config.signer_address).ok_or("invalid gas payer")?;
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
    let gas_budget = (config.gas_limit as u128)
        .checked_mul(fees.max_fee_per_gas())
        .ok_or("gas budget overflow")?;
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
        .map_err(|_| "invalid primary RPC")?;
    let peer_rpc = PoolRpc::with_timeout(&config.peer_rpc_url, chain_id, pool, timeout)
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
    let index = |name: &str| {
        IndexStore::new(
            config.state_root.join("payment-observation").join(name),
            domain,
        )
        .map_err(|_| "pool checkpoint unavailable")
    };
    let primary_index = index("shielded-primary.json")?;
    let peer_index = index("shielded-peer.json")?;
    let (_, revisions) = coordinator
        .recorded_deal(operation)
        .map_err(|_| "durable revisions unavailable")?;
    let nullifier = deal_nullifier(&terms).map_err(|_| "invalid agreement")?;
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
        Err(_) => return Err("paired pool evidence unavailable; retain reservations"),
    };
    response["measurements_ms"]["observation"] = json!(started.elapsed().as_millis());
    let wallet_key = read(&config.wallet_key_file, 32)?;
    let wallet = WalletStore::new(
        &config.wallet_file,
        WalletDomain { chain_id, pool },
        wallet_key
            .as_slice()
            .try_into()
            .map_err(|_| "wallet key must be 32 raw bytes")?,
    )
    .map_err(|_| "encrypted wallet unavailable")?;
    let wallet_primary = index("wallet-primary.json")?;
    let wallet_peer = index("wallet-peer.json")?;
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
        Err(_) => return Err("paired finalized wallet recovery failed; retain reservations"),
    }
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
    let journal = SignerJournal::open(&config.signer_journal_root, chain_id, signer)
        .map_err(|_| "gas-payer nonce journal unavailable")?;
    match assessment.state {
        DealState::PaidFinalized {
            commitment: winning,
        } => {
            response["status"] = json!("ok");
            response["payment_verified"] = json!(winning.to_hex() == response["deal_commitment"]);
            response["winning_commitment"] = json!(winning.to_hex());
            response["nonce_cleanup_pending"] = json!(match chain
                .evm()
                .verified_finalized_nonce_agreed(peer.evm(), signer)
                .await
            {
                Ok(nonce) => journal.release(&nonce).is_err(),
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
    if stage.broadcast_attempts != 0 || (!submit && !funding_only) {
        return Ok(response);
    }
    if !matches!(
        stage.stage,
        Stage::Authorized | Stage::Prepared | Stage::Signed
    ) {
        return Err("operation is not ready for payment");
    }
    let started = Instant::now();
    let balance = chain
        .evm()
        .gas_payer_balance(signer)
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
        let release = ArtifactRelease::authenticate(
            &read(&config.manifest_file, MAX_MANIFEST_BYTES)?,
            digest(&config.manifest_sha256)?,
        )
        .map_err(|_| "artifact manifest not authenticated")?;
        if !release.matches_deployment(chain_id, pool, config.verifier_version) {
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
    let gas = chain
        .evm()
        .estimate_call_gas(signer, pool, &prepared.backend_evidence)
        .await
        .map_err(|_| "pool call estimate failed; retain reservations")?;
    response["funding"]["estimated_gas"] = json!(gas);
    if gas > config.gas_limit {
        response["status"] = json!("funding_required");
        return Ok(response);
    }
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
                config.gas_limit,
            )
            .await
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
