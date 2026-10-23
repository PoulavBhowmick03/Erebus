//! Explicit installed wallet operations: notes, sync, deposit, withdraw, and resume.
//!
//! One bounded JSON request on stdin. Keys, witnesses, and note openings stay in owner-only
//! local files. Deposit creates fresh random note material; nothing is derived from fixed
//! test strings. Sync replays only verified finalized pool history through two RPCs and never
//! creates or sends a transaction. After an unknown broadcast, only an explicit resume may
//! resend the exact retained bytes; no method ever re-signs or re-plans a retained operation.

use std::{
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use erebus_core::{
    domain::DeploymentDomain,
    ids::{AddressBytes, ChainNamespace},
};
use erebus_evm::{
    chain::{Eip1559Fees, EvmChain, SignerJournal, TransactionKey},
    deployment::{parse_lowercase_address, EvmDeployment},
};
use erebus_shielded_prover::{
    artifacts::{ArtifactRelease, InstallOptions, ManifestAuthorization},
    checkpoint,
    gas_policy::{self, GasPolicy},
    index_store::{IndexDomain, IndexStore},
    indexer::TreeCheckpoint,
    operations::{
        parse_address, parse_digest32, parse_field, prepare_approve_request, prepare_deposit,
        prepare_unwrap, prepare_withdraw, prepare_wrap, reconcile_finalized_operations,
        reconcile_operations, replace_wallet_call, resume_wallet_call, submit_wallet_call,
        submit_wallet_call_with_policy, OperationContext, OperationError, OperationStore,
        WalletOperationState, WalletSubmissionState,
    },
    recovery::{recover_finalized_wallet_agreed_bounded, RecoveryError},
    rpc::{PoolDependencyPins, PoolIdentity, PoolRpc, PoolRuntimePins, MAX_CONCURRENT_READS},
    wallet::{WalletDomain, WalletStore},
};
use serde::Deserialize;
use serde_json::{json, Value};
use zeroize::Zeroizing;

#[path = "wallet/deposit_requests.rs"]
mod deposit_requests;

const HELP: &str = "erebus-wallet: explicit installed shielded wallet operations.
One bounded JSON request on stdin: {\"method\":\"...\", ...}.
Methods: version, notes, status, sync, checkpoint, approve, wrap, unwrap, deposit, withdraw, resume, replace.
Requests use config_file; approve, wrap, unwrap, and deposit also amount; withdraw also commitment and recipient;
agents must supply operation_ref for approve, wrap, unwrap and deposit and reuse it after interruption;
resume also identity. Operator configuration pins the chain, pool, asset, artifact manifest
digest, wallet, signer journal, and two RPC endpoints. Keys and witnesses stay local.
sync replays verified finalized history and completes observed operations only.
checkpoint (fresh participants only, confirm_fresh_wallet: true) records a paired, verified finalized
pool frontier so later scans start after it; it never signs, sends or scans history.
Exit 2 means history or an operation is still pending/retained; exit 1 is an error.
No output contains a spend secret, salt, witness, or private key.
See docs/metropolis-operations.md.";

#[derive(Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Version {},
    Notes {
        config_file: PathBuf,
    },
    Status {
        config_file: PathBuf,
    },
    Sync {
        config_file: PathBuf,
    },
    Checkpoint {
        config_file: PathBuf,
        confirm_fresh_wallet: bool,
    },
    Approve {
        config_file: PathBuf,
        amount: String,
        #[serde(default)]
        operation_ref: Option<String>,
    },
    Wrap {
        config_file: PathBuf,
        amount: String,
        operation_ref: String,
    },
    Unwrap {
        config_file: PathBuf,
        amount: String,
        operation_ref: String,
    },
    Deposit {
        config_file: PathBuf,
        amount: String,
        #[serde(default)]
        operation_ref: Option<String>,
    },
    Withdraw {
        config_file: PathBuf,
        commitment: String,
        recipient: String,
    },
    Resume {
        config_file: PathBuf,
        identity: String,
    },
    Replace {
        config_file: PathBuf,
        identity: String,
        #[serde(default)]
        max_fee_per_gas: Option<String>,
        #[serde(default)]
        max_priority_fee_per_gas: Option<String>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    version: u16,
    state_root: PathBuf,
    namespace: String,
    pool: String,
    verifier_version: u32,
    asset: String,
    /// Optional native wrapper (WMON) contract, required for wrap and unwrap.
    #[serde(default)]
    wrapper: Option<String>,
    rpc_url: String,
    peer_rpc_url: String,
    #[serde(default)]
    rpc_requests_per_second: Option<u32>,
    #[serde(default)]
    peer_rpc_requests_per_second: Option<u32>,
    first_block: u64,
    first_hash: String,
    wallet_file: PathBuf,
    wallet_key_file: PathBuf,
    agreement_key_file: PathBuf,
    signer_address: String,
    signer_journal_root: PathBuf,
    transaction_key_file: Option<PathBuf>,
    artifact_manifest_file: PathBuf,
    artifact_manifest_sha256: String,
    #[serde(default)]
    artifact_manifest_authorization: Option<ManifestAuthorization>,
    artifact_cache_root: PathBuf,
    /// Independently reviewed deployed code; required outside explicit test mode.
    #[serde(default)]
    runtime_pins: Option<RuntimePins>,
    #[serde(default)]
    allow_test_artifacts: bool,
    #[serde(default)]
    allow_loopback_artifact_http: bool,
    gas_limit: u64,
    max_fee_per_gas: String,
    max_priority_fee_per_gas: String,
    timeout_seconds: u64,
    max_scan_blocks: u64,
    artifact_timeout_seconds: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimePins {
    pool: String,
    poseidon: String,
    deposit: String,
    transfer: String,
    withdraw: String,
}

impl RuntimePins {
    fn parse(&self) -> Result<([u8; 32], PoolDependencyPins), Failure> {
        let parse = |value: &str| {
            let hash = parse_digest32(value).map_err(|_| Failure::Error("invalid runtime pin"))?;
            if hash == [0; 32] {
                return Err(Failure::Error("runtime pins must be non-zero"));
            }
            Ok(hash)
        };
        Ok((
            parse(&self.pool)?,
            PoolDependencyPins {
                poseidon: parse(&self.poseidon)?,
                deposit: parse(&self.deposit)?,
                transfer: parse(&self.transfer)?,
                withdraw: parse(&self.withdraw)?,
            },
        ))
    }
}

struct Prepared {
    config: Config,
    namespace: ChainNamespace,
    chain_id: u64,
    pool: [u8; 20],
    asset: [u8; 20],
    signer: [u8; 20],
    first_hash: [u8; 32],
    fees: Eip1559Fees,
}

enum Failure {
    Pending(&'static str),
    /// Pending with a complete result (for example legacy records needing reconciliation).
    PendingResult(Value),
    Error(&'static str),
}

fn finish(result: Result<Value, Failure>) -> ! {
    match result {
        Ok(value) => {
            println!("{}", json!({"status": "ok", "result": value}));
            std::process::exit(0);
        }
        Err(Failure::Pending(error)) => {
            eprintln!("{}", json!({"status": "pending", "error": error}));
            std::process::exit(2);
        }
        Err(Failure::PendingResult(value)) => {
            let message = if value.get("history_pending") == Some(&json!(true)) {
                "verified pool history is not complete; resume sync"
            } else if value.get("resume_required") == Some(&json!(true)) {
                "deposit bytes retained; resume its original identity"
            } else {
                "legacy wallet operations require manual reconciliation"
            };
            eprintln!(
                "{}",
                json!({"status": "pending", "result": value,
                    "error": message})
            );
            std::process::exit(2);
        }
        Err(Failure::Error(error)) => {
            eprintln!("{}", json!({"status": "error", "error": error}));
            std::process::exit(1);
        }
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "--version" {
        let mut value = capabilities();
        value["status"] = json!("ok");
        println!("{value}");
        std::process::exit(0);
    }
    if args.len() == 1 && args[0] == "--help" {
        println!("{HELP}");
        std::process::exit(0);
    }
    if !args.is_empty() {
        finish(Err(Failure::Error("unexpected argument")));
    }
    let mut bytes = Zeroizing::new(Vec::new());
    if std::io::stdin()
        .take(16385)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > 16384
    {
        finish(Err(Failure::Error("invalid request")));
    }
    let request = match serde_json::from_slice(&bytes) {
        Ok(request) => request,
        Err(_) => finish(Err(Failure::Error("invalid request"))),
    };
    finish(run(request).await);
}

fn capabilities() -> Value {
    json!({"protocol":1, "deposit_operation_references":true,
        "funding_operation_references":true, "paired_account_diagnostics":true,
        "methods":["notes","status","sync","checkpoint","approve","wrap","unwrap","deposit","withdraw","resume","replace"]})
}

async fn run(request: Request) -> Result<Value, Failure> {
    match request {
        Request::Version {} => Ok(capabilities()),
        Request::Notes { config_file } => {
            let prepared = load(&config_file)?;
            notes(&prepared)
        }
        Request::Status { config_file } => {
            let prepared = load(&config_file)?;
            status(&prepared).await
        }
        Request::Sync { config_file } => {
            let prepared = load(&config_file)?;
            sync(&prepared).await
        }
        Request::Checkpoint {
            config_file,
            confirm_fresh_wallet,
        } => {
            if !confirm_fresh_wallet {
                return Err(Failure::Error(
                    "checkpoint requires confirm_fresh_wallet: true for a new participant",
                ));
            }
            let prepared = load(&config_file)?;
            checkpoint(&prepared).await
        }
        Request::Approve {
            config_file,
            amount,
            operation_ref,
        } => {
            let prepared = load(&config_file)?;
            let amount = decimal(&amount)?;
            let reference = operation_ref
                .as_deref()
                .map(parse_digest32)
                .transpose()
                .map_err(|_| Failure::Error("invalid approval reference"))?;
            approve(&prepared, amount, reference).await
        }
        Request::Wrap {
            config_file,
            amount,
            operation_ref,
        } => {
            let prepared = load(&config_file)?;
            let amount = decimal(&amount)?;
            let operation_ref = parse_digest32(&operation_ref)
                .map_err(|_| Failure::Error("invalid operation reference"))?;
            wrap_or_unwrap(&prepared, amount, operation_ref, true).await
        }
        Request::Unwrap {
            config_file,
            amount,
            operation_ref,
        } => {
            let prepared = load(&config_file)?;
            let amount = decimal(&amount)?;
            let operation_ref = parse_digest32(&operation_ref)
                .map_err(|_| Failure::Error("invalid operation reference"))?;
            wrap_or_unwrap(&prepared, amount, operation_ref, false).await
        }
        Request::Deposit {
            config_file,
            amount,
            operation_ref,
        } => {
            let prepared = load(&config_file)?;
            let amount = decimal(&amount)?;
            let reference = operation_ref
                .as_deref()
                .map(parse_digest32)
                .transpose()
                .map_err(|_| Failure::Error("invalid deposit reference"))?;
            deposit(&prepared, amount, reference).await
        }
        Request::Withdraw {
            config_file,
            commitment,
            recipient,
        } => {
            let prepared = load(&config_file)?;
            let commitment =
                parse_field(&commitment).map_err(|_| Failure::Error("invalid commitment"))?;
            let recipient =
                parse_address(&recipient).map_err(|_| Failure::Error("invalid recipient"))?;
            withdraw(&prepared, commitment, recipient).await
        }
        Request::Resume {
            config_file,
            identity,
        } => {
            let prepared = load(&config_file)?;
            let identity =
                parse_field(&identity).map_err(|_| Failure::Error("invalid identity"))?;
            resume(&prepared, identity).await
        }
        Request::Replace {
            config_file,
            identity,
            max_fee_per_gas,
            max_priority_fee_per_gas,
        } => {
            let prepared = load(&config_file)?;
            let identity =
                parse_field(&identity).map_err(|_| Failure::Error("invalid identity"))?;
            let fees = match (max_fee_per_gas, max_priority_fee_per_gas) {
                (None, None) => None,
                (Some(max_fee), Some(priority)) => Some(
                    Eip1559Fees::new(decimal(&max_fee)?, decimal(&priority)?)
                        .map_err(|_| Failure::Error("invalid replacement fees"))?,
                ),
                _ => return Err(Failure::Error("replacement requires both fee fields")),
            };
            replace(&prepared, identity, fees).await
        }
    }
}

fn load(path: &Path) -> Result<Prepared, Failure> {
    let bytes = private_read(path, 64 * 1024).map_err(|_| Failure::Error("config unavailable"))?;
    let config: Config =
        serde_json::from_slice(&bytes).map_err(|_| Failure::Error("invalid config"))?;
    if config.version != 1
        || config.timeout_seconds == 0
        || config.artifact_timeout_seconds == 0
        || config.max_scan_blocks == 0
        || config.gas_limit == 0
        || config.rpc_url == config.peer_rpc_url
        || [
            config.rpc_requests_per_second,
            config.peer_rpc_requests_per_second,
        ]
        .into_iter()
        .flatten()
        .any(|rate| {
            rate == 0 || rate > erebus_shielded_prover::rpc::DEFAULT_MAX_REQUESTS_PER_SECOND
        })
    {
        return Err(Failure::Error("invalid config"));
    }
    if let Some(pins) = &config.runtime_pins {
        pins.parse()?;
    } else if !config.allow_test_artifacts {
        return Err(Failure::Error(
            "wallet release requires trusted pool and verifier runtime pins",
        ));
    }
    let namespace =
        ChainNamespace::parse(&config.namespace).map_err(|_| Failure::Error("invalid config"))?;
    if namespace.family() != "eip155" {
        return Err(Failure::Error("invalid config"));
    }
    let chain_id: u64 = namespace
        .reference()
        .parse()
        .map_err(|_| Failure::Error("invalid config"))?;
    let pool = parse_lowercase_address(&config.pool).ok_or(Failure::Error("invalid config"))?;
    let asset = parse_lowercase_address(&config.asset).ok_or(Failure::Error("invalid config"))?;
    if let Some(wrapper) = &config.wrapper {
        let wrapper = parse_lowercase_address(wrapper).ok_or(Failure::Error("invalid config"))?;
        if wrapper == [0; 20] {
            return Err(Failure::Error("invalid config"));
        }
    }
    let signer =
        parse_address(&config.signer_address).map_err(|_| Failure::Error("invalid config"))?;
    let first_hash =
        parse_digest32(&config.first_hash).map_err(|_| Failure::Error("invalid config"))?;
    if pool == [0; 20] || asset == [0; 20] || signer == [0; 20] || first_hash == [0; 32] {
        return Err(Failure::Error("invalid config"));
    }
    let max_fee = decimal(&config.max_fee_per_gas)?;
    let priority = decimal(&config.max_priority_fee_per_gas)?;
    let fees = Eip1559Fees::new(max_fee, priority).map_err(|_| Failure::Error("invalid config"))?;
    if fees.max_fee_per_gas() != max_fee || fees.max_priority_fee_per_gas() != priority {
        return Err(Failure::Error("invalid config"));
    }
    Ok(Prepared {
        config,
        namespace,
        chain_id,
        pool,
        asset,
        signer,
        first_hash,
        fees,
    })
}

fn decimal(text: &str) -> Result<u128, Failure> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(Failure::Error("invalid amount"));
    }
    text.parse().map_err(|_| Failure::Error("invalid amount"))
}

fn now() -> Result<u64, Failure> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| Failure::Error("clock unavailable"))
}

fn domain(prepared: &Prepared) -> Result<DeploymentDomain, Failure> {
    let address = AddressBytes::new(prepared.pool.to_vec())
        .map_err(|_| Failure::Error("invalid pool address"))?;
    Ok(DeploymentDomain {
        namespace: prepared.namespace.clone(),
        settlement_contract: Some(address.clone()),
        pool: Some(address),
        verifier_version: prepared.config.verifier_version,
    })
}

fn wallet(prepared: &Prepared) -> Result<WalletStore, Failure> {
    let key = private_read(&prepared.config.wallet_key_file, 32)
        .map_err(|_| Failure::Error("wallet key unavailable"))?;
    let key: [u8; 32] = key
        .as_slice()
        .try_into()
        .map_err(|_| Failure::Error("wallet key must be 32 raw bytes"))?;
    WalletStore::new(
        &prepared.config.wallet_file,
        WalletDomain {
            chain_id: prepared.chain_id,
            pool: prepared.pool,
            verifier_version: prepared.config.verifier_version,
        },
        key,
    )
    .map_err(|_| Failure::Error("encrypted wallet unavailable"))
}

fn operations(prepared: &Prepared) -> Result<OperationStore, Failure> {
    OperationStore::open(prepared.config.state_root.join("wallet-operations"))
        .map_err(|_| Failure::Error("wallet operation journal unavailable"))
}

fn index_domain(prepared: &Prepared) -> IndexDomain {
    IndexDomain {
        chain_id: prepared.chain_id,
        pool: prepared.pool,
        first_block: prepared.config.first_block,
        first_hash: prepared.first_hash,
    }
}

fn recorded_checkpoint(prepared: &Prepared) -> Result<Option<TreeCheckpoint>, Failure> {
    checkpoint::load(&prepared.config.state_root, index_domain(prepared))
        .map_err(|_| Failure::Error("history checkpoint record unavailable"))
}

fn index_store(prepared: &Prepared, name: &str) -> Result<IndexStore, Failure> {
    IndexStore::with_checkpoint(
        prepared.config.state_root.join("wallet-index").join(name),
        index_domain(prepared),
        recorded_checkpoint(prepared)?,
    )
    .map_err(|_| Failure::Error("wallet index unavailable"))
}

/// Every store that can hold history for this participant. All must be empty for a checkpoint.
fn history_bearing_paths(prepared: &Prepared) -> Vec<PathBuf> {
    let root = &prepared.config.state_root;
    vec![
        root.join("wallet-index"),
        root.join("wallet-operations"),
        root.join("deposit-requests"),
        root.join("coordinator"),
        root.join("payment-observation"),
        root.join("pool-history.json"),
        root.join("pool-peer-history.json"),
        root.join("public-history"),
        prepared.config.signer_journal_root.clone(),
    ]
}

fn require_fresh(prepared: &Prepared) -> Result<(), Failure> {
    checkpoint::require_fresh_paths(&history_bearing_paths(prepared)).map_err(
        |error| match error {
            checkpoint::CheckpointError::NotFresh(reason) => Failure::Error(reason),
            _ => Failure::Error("participant state could not be inspected"),
        },
    )?;
    let snapshot = wallet(prepared)?
        .snapshot()
        .map_err(|_| Failure::Error("encrypted wallet unreadable; refusing checkpoint"))?;
    if !snapshot.is_empty() {
        return Err(Failure::Error(
            "wallet holds notes or reservations; a checkpoint would hide history",
        ));
    }
    Ok(())
}

fn checkpoint_view(checkpoint: &TreeCheckpoint, persisted: &str) -> Value {
    json!({
        "checkpoint": persisted,
        "checkpoint_block": checkpoint.block_number,
        "checkpoint_hash": hex::encode(checkpoint.block_hash),
        "leaf_count": checkpoint.leaf_count,
        "root": hex::encode(checkpoint.root),
        "history_from_block": checkpoint.block_number + 1,
        "scope": "pool events before history_from_block are not indexed; deployment-era absence cannot be shown",
        "payment_verified": false,
    })
}

/// Records a paired, verified finalized frontier for a fresh participant. Never signs or sends.
async fn checkpoint(prepared: &Prepared) -> Result<Value, Failure> {
    let pins = prepared.config.runtime_pins.as_ref().ok_or(Failure::Error(
        "a checkpoint requires trusted pool and verifier runtime pins",
    ))?;
    let (pool, dependencies) = pins.parse()?;
    let pins = PoolRuntimePins {
        pool,
        poseidon: dependencies.poseidon,
        deposit: dependencies.deposit,
        transfer: dependencies.transfer,
        withdraw: dependencies.withdraw,
    };
    let root = &prepared.config.state_root;
    std::fs::create_dir_all(root).map_err(|_| Failure::Error("state root unavailable"))?;
    let lock = checkpoint::InitLock::acquire(root).map_err(|error| match error {
        checkpoint::CheckpointError::Busy => {
            Failure::Error("another checkpoint initialization is in progress")
        }
        _ => Failure::Error("checkpoint lock unavailable"),
    })?;
    if let Some(existing) = recorded_checkpoint(prepared)? {
        // Idempotent: the record already exists; nothing is read from any provider.
        return Ok(checkpoint_view(&existing, "existing"));
    }
    require_fresh(prepared)?;
    let paired = |url: &str, rate: Option<u32>, message: &'static str| {
        PoolRpc::with_timeout(
            url,
            prepared.chain_id,
            prepared.pool,
            Duration::from_secs(prepared.config.timeout_seconds),
        )
        .and_then(|rpc| {
            rpc.with_max_requests_per_second(
                rate.unwrap_or(erebus_shielded_prover::rpc::DEFAULT_MAX_REQUESTS_PER_SECOND),
            )
        })
        .map_err(|_| Failure::Error(message))
    };
    let rpc = paired(
        &prepared.config.rpc_url,
        prepared.config.rpc_requests_per_second,
        "invalid RPC",
    )?;
    let peer_rpc = paired(
        &prepared.config.peer_rpc_url,
        prepared.config.peer_rpc_requests_per_second,
        "invalid peer RPC",
    )?;
    let acquired = rpc
        .acquire_checkpoint_agreed(&peer_rpc, index_domain(prepared), pins)
        .await
        .map_err(|_| Failure::Error("paired checkpoint acquisition failed"))?;
    // Nothing may have started using this state root while providers were read.
    require_fresh(prepared)?;
    let persisted = checkpoint::persist(root, index_domain(prepared), acquired, &lock)
        .map_err(|_| Failure::Error("checkpoint record could not be created"))?;
    Ok(checkpoint_view(
        &acquired,
        match persisted {
            checkpoint::Persisted::Created => "created",
            checkpoint::Persisted::Existing => "existing",
        },
    ))
}

fn release(prepared: &Prepared) -> Result<ArtifactRelease, Failure> {
    let bytes = std::fs::read(&prepared.config.artifact_manifest_file)
        .map_err(|_| Failure::Error("artifact manifest unavailable"))?;
    let digest = parse_digest32(&prepared.config.artifact_manifest_sha256)
        .map_err(|_| Failure::Error("artifact manifest digest"))?;
    let release = ArtifactRelease::authenticate_authorized(
        &bytes,
        digest,
        prepared.config.artifact_manifest_authorization.as_ref(),
    )
    .map_err(|_| Failure::Error("artifact manifest authentication"))?;
    if !release.matches_deployment(
        prepared.chain_id,
        prepared.pool,
        prepared.config.verifier_version,
    ) {
        return Err(Failure::Error(
            "artifact release differs from wallet deployment",
        ));
    }
    match &prepared.config.runtime_pins {
        Some(pins) if release.matches_verifier_pins(&pins.parse()?.1) => {}
        None if release.is_test_only() && prepared.config.allow_test_artifacts => {}
        _ => {
            return Err(Failure::Error(
                "artifact release differs from trusted verifier pins",
            ))
        }
    }
    Ok(release)
}

fn install_options(prepared: &Prepared) -> Result<InstallOptions, Failure> {
    let timeout = Duration::from_secs(prepared.config.artifact_timeout_seconds);
    if timeout < Duration::from_secs(1) || timeout > Duration::from_secs(600) {
        return Err(Failure::Error("invalid artifact timeout"));
    }
    Ok(InstallOptions {
        cache_root: prepared.config.artifact_cache_root.clone(),
        timeout,
        allow_test_artifacts: prepared.config.allow_test_artifacts,
        allow_loopback_http: prepared.config.allow_loopback_artifact_http,
    })
}

async fn connect_chain(
    prepared: &Prepared,
    url: &str,
    timeout_seconds: u64,
) -> Result<EvmChain, Failure> {
    if timeout_seconds == 0 {
        return Err(Failure::Error("invalid timeout"));
    }
    let deployment = EvmDeployment::new(
        prepared.namespace.clone(),
        prepared.pool,
        prepared.config.verifier_version,
        url,
    )
    .map_err(|_| Failure::Error("invalid deployment"))?;
    deployment
        .matches_domain(&domain(prepared)?)
        .map_err(|_| Failure::Error("deployment does not match the configured pool"))?;
    let chain = EvmChain::connect(deployment, Duration::from_secs(timeout_seconds))
        .await
        .map_err(|_| Failure::Error("chain unavailable"))?;
    if let Some(pins) = &prepared.config.runtime_pins {
        let (runtime, dependencies) = pins.parse()?;
        let anchor = chain
            .authenticate_deployment_runtime(
                runtime,
                prepared.config.first_block,
                prepared.first_hash,
            )
            .await
            .map_err(|_| Failure::Error("wallet pool runtime authentication failed"))?;
        let rpc = PoolRpc::with_timeout(
            url,
            prepared.chain_id,
            prepared.pool,
            Duration::from_secs(timeout_seconds),
        )
        .and_then(|rpc| {
            rpc.with_max_requests_per_second(
                if url == prepared.config.rpc_url {
                    prepared.config.rpc_requests_per_second
                } else {
                    prepared.config.peer_rpc_requests_per_second
                }
                .unwrap_or(erebus_shielded_prover::rpc::DEFAULT_MAX_REQUESTS_PER_SECOND),
            )
        })
        .map_err(|_| Failure::Error("invalid RPC"))?;
        rpc.authenticate_dependencies_at(anchor.hash, dependencies)
            .await
            .map_err(|_| Failure::Error("wallet verifier runtime authentication failed"))?;
        if rpc
            .canonical_block_hash(anchor.number)
            .await
            .map_err(|_| Failure::Error("wallet runtime anchor unavailable"))?
            != anchor.hash
        {
            return Err(Failure::Error("wallet runtime anchor changed"));
        }
    } else if !prepared.config.allow_test_artifacts {
        return Err(Failure::Error(
            "wallet release requires trusted pool and verifier runtime pins",
        ));
    }
    Ok(chain)
}

fn signer_journal(prepared: &Prepared) -> Result<SignerJournal, Failure> {
    SignerJournal::open(
        &prepared.config.signer_journal_root,
        prepared.chain_id,
        prepared.signer,
    )
    .map_err(|_| Failure::Error("signer journal unavailable"))
}

fn transaction_key(prepared: &Prepared) -> Result<TransactionKey, Failure> {
    let path = prepared
        .config
        .transaction_key_file
        .as_ref()
        .ok_or(Failure::Error("transaction key required"))?;
    let bytes =
        private_read(path, 32).map_err(|_| Failure::Error("transaction key unavailable"))?;
    let key = TransactionKey::from_bytes(
        bytes
            .as_slice()
            .try_into()
            .map_err(|_| Failure::Error("transaction key must be 32 raw bytes"))?,
    )
    .map_err(|_| Failure::Error("invalid transaction key"))?;
    if key.address() != prepared.signer {
        return Err(Failure::Error(
            "transaction key does not match signer address",
        ));
    }
    Ok(key)
}

fn wrapper(prepared: &Prepared) -> Result<[u8; 20], Failure> {
    let wrapper = prepared
        .config
        .wrapper
        .as_ref()
        .ok_or(Failure::Error("wrapper not configured"))?;
    parse_lowercase_address(wrapper).ok_or(Failure::Error("invalid wrapper"))
}

fn agreement_key(prepared: &Prepared) -> Result<[u8; 64], Failure> {
    let bytes = private_read(&prepared.config.agreement_key_file, 64)
        .map_err(|_| Failure::Error("agreement key unavailable"))?;
    bytes
        .as_slice()
        .try_into()
        .map_err(|_| Failure::Error("agreement key must be 64 raw bytes"))
}

fn note_view(note: &erebus_shielded_prover::wallet::OwnedNote) -> Value {
    json!({
        "commitment": hex::encode(note.commitment()),
        "nullifier": hex::encode(note.nullifier()),
        "amount": note.amount().to_string(),
        "asset": format!("0x{}", hex::encode(note.asset())),
        "included": note.inclusion().is_some(),
        "consumed": note.consumption().is_some(),
        "reserved": note.reserved(),
    })
}

fn notes(prepared: &Prepared) -> Result<Value, Failure> {
    let wallet = wallet(prepared)?;
    let snapshot = wallet
        .snapshot()
        .map_err(|_| Failure::Error("wallet unavailable"))?;
    let operations = operations(prepared)?;
    let operations = operations
        .all()
        .map_err(|_| Failure::Error("wallet operation journal unavailable"))?;
    Ok(json!({
        "notes": snapshot.notes().iter().map(note_view).collect::<Vec<_>>(),
        "operations": operations.iter().map(operation_view).collect::<Vec<_>>(),
    }))
}

async fn status(prepared: &Prepared) -> Result<Value, Failure> {
    let pins = prepared.config.runtime_pins.as_ref().ok_or(Failure::Error(
        "wallet diagnostics require trusted runtime pins",
    ))?;
    let (pool, dependencies) = pins.parse()?;
    let pins = PoolRuntimePins {
        pool,
        poseidon: dependencies.poseidon,
        deposit: dependencies.deposit,
        transfer: dependencies.transfer,
        withdraw: dependencies.withdraw,
    };
    let rpc = PoolRpc::with_timeout(
        &prepared.config.rpc_url,
        prepared.chain_id,
        prepared.pool,
        Duration::from_secs(prepared.config.timeout_seconds),
    )
    .and_then(|rpc| {
        rpc.with_max_requests_per_second(
            prepared
                .config
                .rpc_requests_per_second
                .unwrap_or(erebus_shielded_prover::rpc::DEFAULT_MAX_REQUESTS_PER_SECOND),
        )
    })
    .map_err(|_| Failure::Error("invalid RPC"))?;
    let peer_rpc = PoolRpc::with_timeout(
        &prepared.config.peer_rpc_url,
        prepared.chain_id,
        prepared.pool,
        Duration::from_secs(prepared.config.timeout_seconds),
    )
    .and_then(|rpc| {
        rpc.with_max_requests_per_second(
            prepared
                .config
                .peer_rpc_requests_per_second
                .unwrap_or(erebus_shielded_prover::rpc::DEFAULT_MAX_REQUESTS_PER_SECOND),
        )
    })
    .map_err(|_| Failure::Error("invalid peer RPC"))?;
    let primary = index_store(prepared, "primary.json")?;
    let peer = index_store(prepared, "peer.json")?;
    let account = rpc
        .account_state_agreed(
            &peer_rpc,
            primary.domain(),
            PoolIdentity {
                asset: prepared.asset,
                verifier_version: prepared.config.verifier_version,
            },
            pins,
            prepared.signer,
        )
        .await
        .map_err(|_| Failure::Error("paired wallet account diagnostics unavailable"))?;
    let mut checkpoints = Vec::new();
    for (source, store) in [("primary", primary), ("peer", peer)] {
        let index = store
            .load()
            .map_err(|_| Failure::Error("wallet checkpoint unavailable"))?;
        let tip = index.tip();
        let covered = index.covered_through();
        if let Some(tip) = tip {
            if tip.number > account.anchor.number
                || rpc
                    .canonical_block_hash(tip.number)
                    .await
                    .map_err(|_| Failure::Error("wallet checkpoint unavailable"))?
                    != tip.hash
                || peer_rpc
                    .canonical_block_hash(tip.number)
                    .await
                    .map_err(|_| Failure::Error("wallet checkpoint unavailable"))?
                    != tip.hash
            {
                return Err(Failure::Error(
                    "wallet checkpoint conflicts with finalized evidence",
                ));
            }
        }
        let next = covered.map_or(store.domain().first_block, |(number, _)| {
            number.saturating_add(1)
        });
        checkpoints.push(
            json!({"source":source, "checkpoint_block":covered.map(|(number, _)|number),
            "checkpoint_hash":covered.map(|(_, hash)|hex::encode(hash)), "next_log_block":next,
            "blocks_behind":account.anchor.number.saturating_add(1).saturating_sub(next),
            "caught_up":covered.is_some_and(|(number, _)|number == account.anchor.number)}),
        );
    }
    let decimal = |word: [u8; 32]| num_bigint::BigUint::from_bytes_be(&word).to_string();
    for source in [&rpc, &peer_rpc] {
        if source
            .canonical_block_hash(account.anchor.number)
            .await
            .map_err(|_| Failure::Error("wallet diagnostic anchor unavailable"))?
            != account.anchor.hash
            || source
                .finalized_head()
                .await
                .map_err(|_| Failure::Error("wallet diagnostic finality unavailable"))?
                .number
                < account.anchor.number
        {
            return Err(Failure::Error("wallet diagnostic finality changed"));
        }
        source
            .check_chain()
            .await
            .map_err(|_| Failure::Error("wallet diagnostic chain changed"))?;
    }
    let mut value = notes(prepared)?;
    value["account"] = json!({"native_balance":decimal(account.native_balance),
        "asset_balance":decimal(account.asset_balance), "allowance":decimal(account.allowance),
        "anchor_block":account.anchor.number,"anchor_hash":hex::encode(account.anchor.hash),
        "anchor_timestamp":account.anchor.timestamp});
    value["checkpoints"] = json!(checkpoints);
    let recorded = recorded_checkpoint(prepared)?;
    value["history_scope"] = json!(if recorded.is_some() {
        "checkpoint"
    } else {
        "deployment"
    });
    value["history_from_block"] = json!(recorded
        .map_or(prepared.config.first_block, |checkpoint| {
            checkpoint.block_number + 1
        }));
    value["payment_verified"] = json!(false);
    Ok(value)
}

fn operation_view(operation: &erebus_shielded_prover::operations::WalletOperationView) -> Value {
    json!({
        "kind": match operation.kind() {
            erebus_shielded_prover::operations::WalletOperationKind::Deposit => "deposit",
            erebus_shielded_prover::operations::WalletOperationKind::Withdraw => "withdraw",
            erebus_shielded_prover::operations::WalletOperationKind::Approve => "approve",
            erebus_shielded_prover::operations::WalletOperationKind::Wrap => "wrap",
            erebus_shielded_prover::operations::WalletOperationKind::Unwrap => "unwrap",
        },
        "identity": hex::encode(operation.identity()),
        "state": operation_state(operation.state()),
        "retained": operation.retained(),
        "target": format!("0x{}", hex::encode(operation.target())),
        "value": operation.value().to_string(),
        "amount": operation.amount().to_string(),
        "attempts": operation.attempts().iter().map(|attempt| json!({
            "tx_hash": hex::encode(attempt.tx_hash),
            "outcome": attempt_outcome(attempt.outcome),
            "block": attempt.block,
        })).collect::<Vec<_>>(),
    })
}

fn attempt_outcome(
    outcome: erebus_shielded_prover::operations::WalletAttemptOutcome,
) -> &'static str {
    use erebus_shielded_prover::operations::WalletAttemptOutcome;
    match outcome {
        WalletAttemptOutcome::Unknown => "unknown",
        WalletAttemptOutcome::Rejected => "rejected",
        WalletAttemptOutcome::Reverted => "reverted",
        WalletAttemptOutcome::Completed => "completed",
    }
}

fn operation_state(state: WalletOperationState) -> &'static str {
    match state {
        WalletOperationState::Planned => "planned",
        WalletOperationState::Signed => "signed",
        WalletOperationState::BroadcastUnknown => "broadcast_unknown",
        WalletOperationState::Completed => "completed",
    }
}

fn submission_view(submission: &erebus_shielded_prover::operations::WalletSubmission) -> Value {
    json!({
        "state": match submission.state() {
            WalletSubmissionState::Submitted => "submitted",
            WalletSubmissionState::Unknown => "unknown",
            WalletSubmissionState::Rejected => "rejected",
            WalletSubmissionState::Retained => "retained",
            WalletSubmissionState::Completed => "completed",
        },
        "transaction_hash": submission.transaction_hash().map(hex::encode),
        "retry_without_new_payment": true,
        "payment_verified": false,
    })
}

fn submission_error(error: erebus_shielded_prover::operations::OperationError) -> Failure {
    if matches!(
        error,
        erebus_shielded_prover::operations::OperationError::Nonce(
            erebus_evm::chain::NonceError::Busy
        )
    ) {
        return Failure::Pending("signer lane is busy; run sync and retry");
    }
    if matches!(error, OperationError::PlanAboveCap { .. }) {
        return Failure::Error(
            "retained plan exceeds the configured gas cap; operator action required; nothing was sent",
        );
    }
    if matches!(error, OperationError::GasEstimate) {
        return Failure::Error(
            "gas estimate failed; the call would revert or the provider failed; nothing was signed",
        );
    }
    if matches!(error, OperationError::GasLimit { .. }) {
        return Failure::Error("gas estimate exceeds the method gas limit; nothing was signed");
    }
    Failure::Error("submission failed; retained state is preserved")
}

fn submission_failure(
    submission: &erebus_shielded_prover::operations::WalletSubmission,
) -> Failure {
    match submission.state() {
        WalletSubmissionState::Rejected => {
            Failure::Error("broadcast rejected; retained bytes may be resumed")
        }
        WalletSubmissionState::Completed => Failure::Error("operation already completed"),
        // A retained/unknown outcome is never an error to retry blindly.
        _ => Failure::Pending("broadcast outcome unknown; sync or resume the retained bytes"),
    }
}

/// The method's gas policy: its fixed limit, with the configured `gas_limit` as the cap for a
/// claim saved before per-method limits.
fn gas(prepared: &Prepared, limit: u64) -> GasPolicy {
    GasPolicy {
        limit,
        legacy_cap: prepared.config.gas_limit,
    }
}

async fn sync(prepared: &Prepared) -> Result<Value, Failure> {
    sync_session(prepared).await.map(|(value, _)| value)
}

/// Syncs and returns the primary chain it authenticated, so a send in the same process signs
/// and broadcasts through that chain instead of authenticating the deployment a third time.
async fn sync_session(prepared: &Prepared) -> Result<(Value, EvmChain), Failure> {
    // Authenticate before applying pool history or finalized receipts to durable wallet state.
    // The two providers authenticate independently, so both run together.
    let (chain, peer_chain) = tokio::try_join!(
        connect_chain(
            prepared,
            &prepared.config.rpc_url,
            prepared.config.timeout_seconds,
        ),
        connect_chain(
            prepared,
            &prepared.config.peer_rpc_url,
            prepared.config.timeout_seconds,
        )
    )?;
    // Keep every header check and the per-source rate cap; use the existing bounded read fanout.
    let rpc = PoolRpc::with_timeout(
        &prepared.config.rpc_url,
        prepared.chain_id,
        prepared.pool,
        Duration::from_secs(prepared.config.timeout_seconds),
    )
    .and_then(|rpc| rpc.with_max_concurrent_reads(MAX_CONCURRENT_READS))
    .and_then(|rpc| {
        rpc.with_max_requests_per_second(
            prepared
                .config
                .rpc_requests_per_second
                .unwrap_or(erebus_shielded_prover::rpc::DEFAULT_MAX_REQUESTS_PER_SECOND),
        )
    })
    .map_err(|_| Failure::Error("invalid RPC"))?;
    let peer_rpc = PoolRpc::with_timeout(
        &prepared.config.peer_rpc_url,
        prepared.chain_id,
        prepared.pool,
        Duration::from_secs(prepared.config.timeout_seconds),
    )
    .and_then(|rpc| rpc.with_max_concurrent_reads(MAX_CONCURRENT_READS))
    .and_then(|rpc| {
        rpc.with_max_requests_per_second(
            prepared
                .config
                .peer_rpc_requests_per_second
                .unwrap_or(erebus_shielded_prover::rpc::DEFAULT_MAX_REQUESTS_PER_SECOND),
        )
    })
    .map_err(|_| Failure::Error("invalid peer RPC"))?;
    let primary = index_store(prepared, "primary.json")?;
    let peer = index_store(prepared, "peer.json")?;
    let wallet = wallet(prepared)?;
    let report = match recover_finalized_wallet_agreed_bounded(
        &rpc,
        &primary,
        &peer_rpc,
        &peer,
        &wallet,
        prepared.config.max_scan_blocks,
    )
    .await
    {
        Ok(report) => report,
        Err(RecoveryError::HistoryPending {
            next_block,
            through,
        }) => {
            return Err(Failure::PendingResult(json!({"history_pending":true,
                "next_log_block":next_block, "through":through,
                "retry_without_new_payment":true})));
        }
        Err(error) if error.is_rate_limited() => {
            return Err(Failure::Error(
                "RPC provider rate limited; retry later or lower its requests_per_second",
            ))
        }
        Err(_) => return Err(Failure::Error("paired wallet recovery failed")),
    };
    let operations = operations(prepared)?;
    let context = OperationContext {
        chain_id: prepared.chain_id,
        pool: prepared.pool,
        verifier_version: prepared.config.verifier_version,
        asset: prepared.asset,
    };
    let receipts = reconcile_finalized_operations(&chain, &context, &wallet, &operations, now()?)
        .await
        .map_err(|_| Failure::Error("finalized receipt reconciliation failed"))?;
    let completed = reconcile_operations(&wallet, &operations, now()?)
        .map_err(|_| Failure::Error("operation reconciliation failed"))?;
    let journal = signer_journal(prepared)?;
    // Without a claim there is nothing to release, so the paired finalized-nonce reads are skipped.
    if journal
        .claim_nonce()
        .map_err(|_| Failure::Error("signer journal unavailable"))?
        .is_some()
    {
        if let Ok(nonce) = chain
            .verified_finalized_nonce_agreed(&peer_chain, prepared.signer)
            .await
        {
            // NotConsumed means the claimed nonce is not yet finalized; the lane stays held.
            let _ = journal.release(&nonce);
        }
    }
    let nonce_released = journal
        .claim_nonce()
        .map_err(|_| Failure::Error("signer journal unavailable"))?
        .is_none();
    let snapshot = wallet
        .snapshot()
        .map_err(|_| Failure::Error("wallet unavailable"))?;
    let pending = operations
        .pending()
        .map_err(|_| Failure::Error("journal unavailable"))?;
    let all = operations
        .all()
        .map_err(|_| Failure::Error("journal unavailable"))?;
    let migration_required = !receipts.migration_required.is_empty();
    let value = json!({
        "scanned_through": report.through,
        "completed": completed.iter().map(operation_view).collect::<Vec<_>>(),
        "reverted": receipts.changed.iter().map(operation_view).collect::<Vec<_>>(),
        "migration_required": receipts.migration_required.iter().map(operation_view).collect::<Vec<_>>(),
        "pending": pending.iter().map(operation_view).collect::<Vec<_>>(),
        "operations": all.iter().map(operation_view).collect::<Vec<_>>(),
        "nonce_released": nonce_released,
        "notes": snapshot.notes().iter().map(note_view).collect::<Vec<_>>(),
    });
    if migration_required {
        return Err(Failure::PendingResult(value));
    }
    Ok((value, chain))
}

fn retain_action(
    prepared: &Prepared,
    kind: &[u8],
    amount: u128,
    reference: [u8; 32],
) -> Result<deposit_requests::DepositRequest, Failure> {
    let binding = erebus_core::suite::keccak256(&[
        b"erebus-wallet-action-request-v1",
        kind,
        &prepared.chain_id.to_be_bytes(),
        &prepared.pool,
        &prepared.asset,
        &prepared.signer,
        prepared.config.wrapper.as_deref().unwrap_or("").as_bytes(),
        &amount.to_be_bytes(),
    ]);
    deposit_requests::DepositRequest::retain_if_new(
        &prepared.config.state_root.join("wallet-action-requests"),
        reference,
        binding,
        || Ok(()),
    )
    .map_err(Failure::Error)
}

async fn approve(
    prepared: &Prepared,
    amount: u128,
    reference: Option<[u8; 32]>,
) -> Result<Value, Failure> {
    let (_, chain) = sync_session(prepared).await?;
    let operations = operations(prepared)?;
    let mut request = reference
        .map(|reference| retain_action(prepared, b"approve", amount, reference))
        .transpose()?;
    let prepared_call = prepare_approve_request(
        &operations,
        prepared.chain_id,
        prepared.pool,
        prepared.asset,
        amount,
        reference,
        now()?,
    )
    .map_err(|_| Failure::Error("approval preparation failed; nothing was sent"))?;
    if let Some(request) = &mut request {
        request
            .bind(prepared_call.identity)
            .map_err(Failure::Error)?;
    }
    let journal = signer_journal(prepared)?;
    let key = transaction_key(prepared)?;
    let submission = submit_wallet_call_with_policy(
        &chain,
        &journal,
        &operations,
        &domain(prepared)?,
        &prepared_call,
        &key,
        prepared.fees,
        gas(prepared, gas_policy::APPROVE_GAS_LIMIT),
        now()?,
        false,
    )
    .await
    .map_err(submission_error)?;
    if submission.state() == WalletSubmissionState::Completed {
        return Ok(json!({"kind": "approve", "already_completed": true}));
    }
    if submission.state() != WalletSubmissionState::Submitted {
        return Err(submission_failure(&submission));
    }
    Ok(json!({
        "kind": "approve",
        "identity": hex::encode(prepared_call.identity),
        "spender": prepared.config.pool,
        "amount": amount.to_string(),
        "submission": submission_view(&submission),
    }))
}

/// Reads one ERC-20 `allowance(owner, spender)` word.
async fn allowance(
    chain: &EvmChain,
    token: [u8; 20],
    owner: [u8; 20],
    spender: [u8; 20],
) -> Result<u128, Failure> {
    let mut data = vec![0xdd, 0x62, 0xed, 0x3e];
    data.extend_from_slice(&address_word(owner));
    data.extend_from_slice(&address_word(spender));
    let result = chain
        .call(token, &data)
        .await
        .map_err(|_| Failure::Error("allowance read failed"))?;
    if result.len() < 32 {
        return Err(Failure::Error("allowance read failed"));
    }
    let mut word = [0u8; 32];
    word.copy_from_slice(&result[..32]);
    if word[..16].iter().any(|byte| *byte != 0) {
        return Err(Failure::Error("allowance out of range"));
    }
    Ok(u128::from_be_bytes(
        word[16..].try_into().expect("16 bytes"),
    ))
}

/// Reads one ERC-20 `balanceOf(owner)` word at the latest block.
async fn token_balance(
    chain: &EvmChain,
    token: [u8; 20],
    owner: [u8; 20],
) -> Result<u128, Failure> {
    let mut data = vec![0x70, 0xa0, 0x82, 0x31];
    data.extend_from_slice(&address_word(owner));
    let result = chain
        .call(token, &data)
        .await
        .map_err(|_| Failure::Error("asset balance read failed"))?;
    let word: [u8; 32] = result
        .get(..32)
        .and_then(|word| word.try_into().ok())
        .ok_or(Failure::Error("asset balance read failed"))?;
    if word[..16].iter().any(|byte| *byte != 0) {
        // Above u128: certainly enough for any representable amount.
        return Ok(u128::MAX);
    }
    let mut low = [0u8; 16];
    low.copy_from_slice(&word[16..]);
    Ok(u128::from_be_bytes(low))
}

fn address_word(address: [u8; 20]) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[12..].copy_from_slice(&address);
    word
}

async fn wrap_or_unwrap(
    prepared: &Prepared,
    amount: u128,
    operation_ref: [u8; 32],
    wrap: bool,
) -> Result<Value, Failure> {
    let (_, chain) = sync_session(prepared).await?;
    let wrapper = wrapper(prepared)?;
    let mut request = retain_action(
        prepared,
        if wrap { b"wrap" } else { b"unwrap" },
        amount,
        operation_ref,
    )?;
    let operations = operations(prepared)?;
    let prepared_call = if wrap {
        prepare_wrap(
            &operations,
            prepared.chain_id,
            wrapper,
            amount,
            operation_ref,
            now()?,
        )
    } else {
        prepare_unwrap(
            &operations,
            prepared.chain_id,
            wrapper,
            amount,
            operation_ref,
            now()?,
        )
    }
    .map_err(|_| Failure::Error("native wrapper preparation failed; nothing was sent"))?;
    request
        .bind(prepared_call.identity)
        .map_err(Failure::Error)?;
    if wrap {
        let balance = chain
            .gas_payer_balance(prepared.signer)
            .await
            .map_err(|_| Failure::Error("native balance unavailable"))?;
        if balance < amount {
            return Err(Failure::Error("native balance is below the wrap amount"));
        }
    }
    let journal = signer_journal(prepared)?;
    let key = transaction_key(prepared)?;
    let submission = submit_wallet_call_with_policy(
        &chain,
        &journal,
        &operations,
        &domain(prepared)?,
        &prepared_call,
        &key,
        prepared.fees,
        gas(
            prepared,
            if wrap {
                gas_policy::WRAP_GAS_LIMIT
            } else {
                gas_policy::UNWRAP_GAS_LIMIT
            },
        ),
        now()?,
        false,
    )
    .await
    .map_err(submission_error)?;
    if submission.state() == WalletSubmissionState::Completed {
        return Ok(json!({
            "kind": if wrap { "wrap" } else { "unwrap" },
            "already_completed": true,
        }));
    }
    if submission.state() != WalletSubmissionState::Submitted {
        return Err(submission_failure(&submission));
    }
    Ok(json!({
        "kind": if wrap { "wrap" } else { "unwrap" },
        "identity": hex::encode(prepared_call.identity),
        "wrapper": prepared.config.wrapper,
        "amount": amount.to_string(),
        "submission": submission_view(&submission),
    }))
}

async fn deposit(
    prepared: &Prepared,
    amount: u128,
    reference: Option<[u8; 32]>,
) -> Result<Value, Failure> {
    // Refresh verified inclusion and release a finalized signer-lane claim from a prior
    // operation before preparing a new one. This never sends a transaction.
    let (_, chain) = sync_session(prepared).await?;
    let wallet = wallet(prepared)?;
    let operations = operations(prepared)?;
    let owner = agreement_key(prepared)?;
    let binding = erebus_core::suite::keccak256(&[
        b"erebus-wallet-deposit-request-v1",
        &prepared.chain_id.to_be_bytes(),
        &prepared.pool,
        &prepared.config.verifier_version.to_be_bytes(),
        &prepared.asset,
        &prepared.signer,
        &owner,
        &amount.to_be_bytes(),
    ]);
    let requests_root = prepared.config.state_root.join("deposit-requests");
    let _legacy_lock = if reference.is_none() {
        Some(
            deposit_requests::DepositRequest::lock_legacy(&requests_root)
                .map_err(Failure::Error)?,
        )
    } else {
        None
    };
    let mut retained = reference
        .map(|reference| {
            deposit_requests::DepositRequest::retain_if_new(
                &requests_root,
                reference,
                binding,
                || {
                    if operations
                        .pending()
                        .map_err(|_| "deposit operation journal unavailable")?
                        .iter()
                        .any(|operation| {
                            operation.kind()
                                == erebus_shielded_prover::operations::WalletOperationKind::Deposit
                        })
                    {
                        Err("another deposit is retained; resume its original identity")
                    } else {
                        Ok(())
                    }
                },
            )
        })
        .transpose()
        .map_err(Failure::Error)?;
    if let Some(identity) = retained.as_ref().and_then(|record| record.commitment()) {
        let operation = operations
            .get(&identity)
            .map_err(|_| Failure::Error("retained deposit operation unavailable"))?
            .ok_or(Failure::Error("retained deposit operation missing"))?;
        if operation.state() == WalletOperationState::Completed {
            return Ok(
                json!({"kind":"deposit", "identity":hex::encode(identity), "already_completed":true}),
            );
        }
        if operation.retained() {
            return Err(Failure::PendingResult(
                json!({"kind":"deposit", "identity":hex::encode(identity),
                "retry_without_new_payment":true, "resume_required":true}),
            ));
        }
    }
    let journal = signer_journal(prepared)?;
    let key = transaction_key(prepared)?;
    let available = allowance(&chain, prepared.asset, prepared.signer, prepared.pool).await?;
    if available < amount {
        return Err(Failure::Error(
            "deposit allowance insufficient; run approve first",
        ));
    }
    // A wrap or transfer that is not visible yet refuses here, before any proof or claim.
    if token_balance(&chain, prepared.asset, prepared.signer).await? < amount {
        return Err(Failure::Error(
            "deposit asset balance insufficient; wrap or fund the asset first",
        ));
    }
    let release = release(prepared)?;
    let options = install_options(prepared)?;
    let prepared_call = prepare_deposit(
        &release,
        &options,
        &wallet,
        &operations,
        WalletDomain {
            chain_id: prepared.chain_id,
            pool: prepared.pool,
            verifier_version: prepared.config.verifier_version,
        },
        prepared.asset,
        owner,
        prepared.config.verifier_version,
        amount,
        now()?,
    )
    .await
    .map_err(|_| Failure::Error("deposit preparation failed; no funds were sent"))?;
    if let Some(retained) = &mut retained {
        retained
            .bind(prepared_call.commitment)
            .map_err(Failure::Error)?;
    }
    let submission = submit_wallet_call_with_policy(
        &chain,
        &journal,
        &operations,
        &domain(prepared)?,
        &prepared_call,
        &key,
        prepared.fees,
        gas(prepared, gas_policy::DEPOSIT_GAS_LIMIT),
        now()?,
        false,
    )
    .await
    .map_err(submission_error)?;
    if submission.state() == WalletSubmissionState::Completed {
        return Ok(json!({"kind": "deposit", "already_completed": true}));
    }
    if submission.state() != WalletSubmissionState::Submitted {
        return Err(submission_failure(&submission));
    }
    Ok(json!({
        "kind": "deposit",
        "commitment": hex::encode(prepared_call.commitment),
        "installation_ms": prepared_call.installation_ms,
        "proving_ms": prepared_call.proving_ms,
        "submission": submission_view(&submission),
    }))
}

async fn withdraw(
    prepared: &Prepared,
    commitment: [u8; 32],
    recipient: [u8; 20],
) -> Result<Value, Failure> {
    let (_, chain) = sync_session(prepared).await?;
    let wallet = wallet(prepared)?;
    let operations = operations(prepared)?;
    let primary = index_store(prepared, "primary.json")?;
    let index = primary
        .load()
        .map_err(|_| Failure::Error("verified wallet index unavailable"))?;
    let release = release(prepared)?;
    let options = install_options(prepared)?;
    let prepared_call = prepare_withdraw(
        &release,
        &options,
        &wallet,
        &operations,
        &index,
        WalletDomain {
            chain_id: prepared.chain_id,
            pool: prepared.pool,
            verifier_version: prepared.config.verifier_version,
        },
        prepared.config.verifier_version,
        commitment,
        recipient,
        now()?,
    )
    .await
    .map_err(|_| Failure::Error("withdrawal preparation failed; no funds were sent"))?;
    let journal = signer_journal(prepared)?;
    let key = transaction_key(prepared)?;
    let submission = submit_wallet_call(
        &chain,
        &journal,
        &operations,
        &domain(prepared)?,
        &prepared_call,
        &key,
        prepared.fees,
        prepared.config.gas_limit,
        now()?,
        false,
    )
    .await
    .map_err(submission_error)?;
    if submission.state() == WalletSubmissionState::Completed {
        return Ok(json!({"kind": "withdraw", "already_completed": true}));
    }
    if submission.state() != WalletSubmissionState::Submitted {
        return Err(submission_failure(&submission));
    }
    Ok(json!({
        "kind": "withdraw",
        "nullifier": hex::encode(prepared_call.nullifier),
        "recipient": format!("0x{}", hex::encode(recipient)),
        "installation_ms": prepared_call.installation_ms,
        "proving_ms": prepared_call.proving_ms,
        "submission": submission_view(&submission),
    }))
}

async fn replace(
    prepared: &Prepared,
    identity: [u8; 32],
    fees: Option<Eip1559Fees>,
) -> Result<Value, Failure> {
    let operations = operations(prepared)?;
    let chain = connect_chain(
        prepared,
        &prepared.config.rpc_url,
        prepared.config.timeout_seconds,
    )
    .await?;
    let key = transaction_key(prepared)?;
    let submission = replace_wallet_call(&chain, &operations, &identity, &key, fees, now()?)
        .await
        .map_err(|_| Failure::Error("replacement failed; retained state is preserved"))?;
    if submission.state() == WalletSubmissionState::Completed {
        return Ok(json!({"identity": hex::encode(identity), "already_completed": true}));
    }
    if submission.state() != WalletSubmissionState::Submitted {
        return Err(submission_failure(&submission));
    }
    Ok(json!({
        "identity": hex::encode(identity),
        "replaced": true,
        "submission": submission_view(&submission),
    }))
}

async fn resume(prepared: &Prepared, identity: [u8; 32]) -> Result<Value, Failure> {
    let operations = operations(prepared)?;
    let chain = connect_chain(
        prepared,
        &prepared.config.rpc_url,
        prepared.config.timeout_seconds,
    )
    .await?;
    let submission = resume_wallet_call(&chain, &operations, &identity, now()?)
        .await
        .map_err(|_| Failure::Error("retained operation is unavailable or completed"))?;
    if submission.state() == WalletSubmissionState::Completed {
        return Ok(json!({"identity": hex::encode(identity), "already_completed": true}));
    }
    if submission.state() != WalletSubmissionState::Submitted {
        return Err(submission_failure(&submission));
    }
    Ok(json!({
        "identity": hex::encode(identity),
        "submission": submission_view(&submission),
    }))
}

fn private_read(path: &Path, limit: usize) -> Result<Zeroizing<Vec<u8>>, ()> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| ())?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return Err(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(());
        }
    }
    let file = std::fs::File::open(path).map_err(|_| ())?;
    let opened = file.metadata().map_err(|_| ())?;
    if !opened.is_file() || opened.len() != metadata.len() {
        return Err(());
    }
    let mut bytes = Zeroizing::new(Vec::new());
    file.take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() > limit {
        return Err(());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_capability_and_request_schema_are_read_only_and_fixed_config() {
        assert_eq!(capabilities()["paired_account_diagnostics"], true);
        assert!(capabilities()["methods"]
            .as_array()
            .unwrap()
            .iter()
            .any(|method| method == "status"));
        assert!(serde_json::from_value::<Request>(
            json!({"method":"status","config_file":"/operator/wallet.json"})
        )
        .is_ok());
        assert!(serde_json::from_value::<Request>(
            json!({"method":"status","config_file":"/operator/wallet.json","target":"0x01"})
        )
        .is_err());
    }

    #[test]
    fn release_wallet_refuses_missing_runtime_pins_before_keys_or_rpc() {
        let root = tempfile::tempdir().unwrap();
        let config = json!({
            "version": 1, "state_root": root.path(), "namespace": "eip155:10143",
            "pool": format!("0x{}", "01".repeat(20)), "verifier_version": 1,
            "asset": format!("0x{}", "02".repeat(20)),
            "rpc_url": "http://127.0.0.1:1", "peer_rpc_url": "http://127.0.0.1:2",
            "first_block": 1, "first_hash": "11".repeat(32),
            "wallet_file": root.path().join("absent-wallet"),
            "wallet_key_file": root.path().join("absent-key"),
            "agreement_key_file": root.path().join("absent-agreement-key"),
            "signer_address": format!("0x{}", "03".repeat(20)),
            "signer_journal_root": root.path().join("absent-journal"),
            "artifact_manifest_file": root.path().join("absent-manifest"),
            "artifact_manifest_sha256": "22".repeat(32),
            "artifact_cache_root": root.path().join("absent-cache"),
            "gas_limit": 100_000, "max_fee_per_gas": "10", "max_priority_fee_per_gas": "1",
            "timeout_seconds": 1, "max_scan_blocks": 100, "artifact_timeout_seconds": 1,
        });
        let path = root.path().join("config.json");
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert!(matches!(
            load(&path),
            Err(Failure::Error(
                "wallet release requires trusted pool and verifier runtime pins"
            ))
        ));
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
        for field in ["rpc_requests_per_second", "peer_rpc_requests_per_second"] {
            for rate in [0, 41, u32::MAX] {
                let mut invalid = config.clone();
                invalid[field] = json!(rate);
                std::fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
                assert!(matches!(load(&path), Err(Failure::Error("invalid config"))));
            }
            for rate in [1, 10, 40] {
                let mut valid = config.clone();
                valid[field] = json!(rate);
                std::fs::write(&path, serde_json::to_vec(&valid).unwrap()).unwrap();
                assert!(matches!(
                    load(&path),
                    Err(Failure::Error(
                        "wallet release requires trusted pool and verifier runtime pins"
                    ))
                ));
            }
        }
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn wallet_runtime_pins_are_complete_nonzero_and_strict() {
        let value = json!({"pool":"11".repeat(32),"poseidon":"22".repeat(32),
            "deposit":"33".repeat(32),"transfer":"44".repeat(32),"withdraw":"55".repeat(32)});
        let pins: RuntimePins = serde_json::from_value(value.clone()).unwrap();
        let (pool, dependencies) = pins
            .parse()
            .unwrap_or_else(|_| panic!("valid pins refused"));
        assert_eq!(pool, [0x11; 32]);
        assert_eq!(dependencies.withdraw, [0x55; 32]);
        for field in ["pool", "poseidon", "deposit", "transfer", "withdraw"] {
            let mut missing = value.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<RuntimePins>(missing).is_err());
            let mut zero = value.clone();
            zero[field] = json!("00".repeat(32));
            assert!(serde_json::from_value::<RuntimePins>(zero)
                .unwrap()
                .parse()
                .is_err());
        }
        let mut unknown = value;
        unknown["extra"] = json!(true);
        assert!(serde_json::from_value::<RuntimePins>(unknown).is_err());
    }

    #[test]
    fn wrapped_asset_pool_accepts_matching_wrapper_but_refuses_zero_or_malformed_wrapper() {
        let root = tempfile::tempdir().unwrap();
        let asset = format!("0x{}", "02".repeat(20));
        let mut config = json!({
            "version": 1, "state_root": root.path(), "namespace": "eip155:31337",
            "pool": format!("0x{}", "01".repeat(20)), "verifier_version": 2,
            "asset": asset, "wrapper": asset,
            "rpc_url": "http://127.0.0.1:1", "peer_rpc_url": "http://127.0.0.1:2",
            "first_block": 1, "first_hash": "11".repeat(32),
            "wallet_file": root.path().join("absent-wallet"),
            "wallet_key_file": root.path().join("absent-key"),
            "agreement_key_file": root.path().join("absent-agreement-key"),
            "signer_address": format!("0x{}", "03".repeat(20)),
            "signer_journal_root": root.path().join("absent-journal"),
            "artifact_manifest_file": root.path().join("absent-manifest"),
            "artifact_manifest_sha256": "22".repeat(32),
            "artifact_cache_root": root.path().join("absent-cache"), "allow_test_artifacts": true,
            "gas_limit": 100_000, "max_fee_per_gas": "10", "max_priority_fee_per_gas": "1",
            "timeout_seconds": 1, "max_scan_blocks": 100, "artifact_timeout_seconds": 1,
        });
        let path = root.path().join("config.json");
        let write = |config: &Value| {
            std::fs::write(&path, serde_json::to_vec(config).unwrap()).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
        };
        write(&config);
        assert!(load(&path).is_ok());
        for wrapper in [
            format!("0x{}", "00".repeat(20)),
            "not-an-address".to_owned(),
        ] {
            config["wrapper"] = json!(wrapper);
            write(&config);
            assert!(matches!(load(&path), Err(Failure::Error("invalid config"))));
        }
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }
}
