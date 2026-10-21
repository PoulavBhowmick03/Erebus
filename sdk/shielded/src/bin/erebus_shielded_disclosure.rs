//! Local disclosure command with independent shielded and public-bound payment verification.
//!
//! A shielded auditor cache may start from a paired finalized checkpoint (`checkpoint` method,
//! `<cache_root>/history-checkpoint.json`). The checkpoint narrows which blocks are scanned, never
//! what a verified payment means: a deal settled at or before the checkpoint block is reported as
//! `settled_before_checkpoint`, never as paid or unpaid.

use std::path::{Path, PathBuf};

use erebus_core::{domain::DeploymentDomain, ids::ChainNamespace, terms::SettlementMode};
use erebus_evm::{
    deployment::{parse_lowercase_address, EvmDeployment},
    disclosure::cli::{self, CheckpointRequest, PaymentRequest, PaymentVerification},
};
use erebus_shielded_prover::{
    checkpoint::{self, CheckpointError, Persisted},
    disclosure::{
        deal_consumed_at_checkpoint, verify_shielded_payment_certified, ShieldedDisclosureError,
    },
    index_store::{IndexDomain, IndexStore},
    indexer::TreeCheckpoint,
    observation::ObservationError,
    recovery::RecoveryError,
    rpc::{PoolRpc, PoolRuntimePins},
};
use erebus_transport::disclosure::verify_selected_agreement;
use serde::Deserialize;
use serde_json::{json, Value};

/// Cache directories a verification writes. Either one present means the cache already holds
/// (or started) full history, which is strictly stronger than any checkpoint.
const CACHE_DIRECTORIES: [&str; 2] = ["primary", "peer"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Deployment {
    namespace: String,
    settlement_contract: String,
    verifier_version: u32,
    rpc_url: String,
    peer_rpc_url: String,
    first_block: u64,
    first_hash: String,
    cache_root: PathBuf,
    runtime_keccak256: String,
    poseidon_keccak256: String,
    deposit_verifier_keccak256: String,
    transfer_verifier_keccak256: String,
    withdraw_verifier_keccak256: String,
}

fn digest(value: &str) -> Result<[u8; 32], &'static str> {
    let digits = value.strip_prefix("0x").ok_or("invalid runtime pin")?;
    if digits.len() != 64
        || !digits
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("invalid runtime pin");
    }
    let bytes: [u8; 32] = hex::decode(digits)
        .map_err(|_| "invalid runtime pin")?
        .try_into()
        .map_err(|_| "invalid runtime pin")?;
    if bytes == [0; 32] {
        return Err("invalid runtime pin");
    }
    Ok(bytes)
}

fn pending_progress(error: &ShieldedDisclosureError) -> Option<(Option<u64>, u64)> {
    match error {
        ShieldedDisclosureError::Observation(ObservationError::Recovery(
            RecoveryError::HistoryPending { next_block, .. },
        )) => Some((Some(*next_block), next_block.saturating_sub(1))),
        ShieldedDisclosureError::Observation(ObservationError::FinalityPending)
        | ShieldedDisclosureError::Payment => Some((None, 0)),
        _ => None,
    }
}

#[tokio::main]
async fn main() {
    cli::run_with_checkpoint(
        |request| async move {
            if cli::is_x402_request(&request.deployment) {
                return cli::verify_x402_payment(request).await;
            }
            if request.evidence.terms.settlement_mode == SettlementMode::PublicBound {
                return cli::verify_public_payment(request).await;
            }
            verify(request).await
        },
        checkpoint_command,
    )
    .await;
}

/// A parsed shielded auditor deployment: trusted pins, index domain, and the two providers.
struct Configured {
    pins: PoolRuntimePins,
    domain: IndexDomain,
    rpc: PoolRpc,
    peer_rpc: PoolRpc,
    cache_root: PathBuf,
}

/// Parses the deployment; with an agreement domain, it must match before anything else is read.
fn configure(
    value: Value,
    agreement: Option<&DeploymentDomain>,
) -> Result<Configured, &'static str> {
    let deployment: Deployment =
        serde_json::from_value(value).map_err(|_| "invalid shielded deployment")?;
    let pins = PoolRuntimePins {
        pool: digest(&deployment.runtime_keccak256)?,
        poseidon: digest(&deployment.poseidon_keccak256)?,
        deposit: digest(&deployment.deposit_verifier_keccak256)?,
        transfer: digest(&deployment.transfer_verifier_keccak256)?,
        withdraw: digest(&deployment.withdraw_verifier_keccak256)?,
    };
    let namespace =
        ChainNamespace::parse(&deployment.namespace).map_err(|_| "invalid deployment")?;
    let pool =
        parse_lowercase_address(&deployment.settlement_contract).ok_or("invalid deployment")?;
    let configured = EvmDeployment::new(
        namespace,
        pool,
        deployment.verifier_version,
        &deployment.rpc_url,
    )
    .map_err(|_| "invalid deployment")?;
    if let Some(agreement) = agreement {
        configured
            .matches_domain(agreement)
            .map_err(|_| "deployment does not match agreement")?;
    }
    let digits = deployment
        .first_hash
        .strip_prefix("0x")
        .ok_or("invalid deployment anchor")?;
    if digits.len() != 64
        || !digits
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("invalid deployment anchor");
    }
    let first_hash = hex::decode(digits)
        .map_err(|_| "invalid deployment anchor")?
        .try_into()
        .map_err(|_| "invalid deployment anchor")?;
    let domain = IndexDomain {
        chain_id: configured.chain_id,
        pool,
        first_block: deployment.first_block,
        first_hash,
    };
    let rpc = PoolRpc::new(&deployment.rpc_url, configured.chain_id, pool)
        .map_err(|_| "invalid pool RPC configuration")?;
    let peer_rpc = PoolRpc::new(&deployment.peer_rpc_url, configured.chain_id, pool)
        .map_err(|_| "invalid peer pool RPC configuration")?;
    Ok(Configured {
        pins,
        domain,
        rpc,
        peer_rpc,
        cache_root: deployment.cache_root,
    })
}

/// The auditor cache's recorded checkpoint, if any. A record for another chain, pool or
/// deployment anchor, or a tampered frontier, is refused rather than ignored.
fn recorded_checkpoint(
    cache_root: &Path,
    domain: IndexDomain,
) -> Result<Option<TreeCheckpoint>, &'static str> {
    checkpoint::load(cache_root, domain)
        .map_err(|_| "auditor history checkpoint record unavailable")
}

async fn verify(request: PaymentRequest) -> Result<PaymentVerification, &'static str> {
    let Configured {
        pins,
        domain,
        rpc,
        peer_rpc,
        cache_root,
    } = configure(request.deployment, Some(&request.evidence.terms.domain))?;
    // Without a record this is the unchanged full replay from the deployment block.
    let recorded = recorded_checkpoint(&cache_root, domain)?;
    let index =
        IndexStore::with_checkpoint(cache_root.join("primary/index.json"), domain, recorded)
            .map_err(|_| "public cache unavailable")?;
    let peer_index =
        IndexStore::with_checkpoint(cache_root.join("peer/index.json"), domain, recorded)
            .map_err(|_| "peer public cache unavailable")?;
    let agreement = verify_selected_agreement(&request.evidence)
        .map_err(|_| "disclosure agreement not verified")?;
    let error = match verify_shielded_payment_certified(
        request.evidence,
        &rpc,
        &index,
        &peer_rpc,
        &peer_index,
        pins,
    )
    .await
    {
        Ok(result) => return Ok(PaymentVerification::Finalized(result.payment.agreement)),
        Err(error) => error,
    };
    if let Some((next_log_block, ancestry_block)) = pending_progress(&error) {
        return Ok(PaymentVerification::Pending {
            agreement,
            next_log_block,
            ancestry_block,
        });
    }
    match (recorded, &error) {
        (
            Some(recorded),
            ShieldedDisclosureError::Observation(ObservationError::HistoryIncomplete),
        ) => {
            let read = deal_consumed_at_checkpoint(&agreement, &rpc, &peer_rpc, &recorded).await;
            before_checkpoint(read)?;
            Ok(PaymentVerification::SettledBeforeCheckpoint {
                agreement,
                checkpoint_block: recorded.block_number,
            })
        }
        _ => Err("payment not independently verified; retain caches and retry"),
    }
}

/// Classifies a consumed deal that a checkpointed cache cannot locate.
///
/// `Ok(())` only when both providers show it consumed at the checkpoint block itself: the pool
/// consumes a deal once, so its settlement is at or before that block. Consumed now but not at
/// the checkpoint means a later settlement the scan did not find: provider inconsistency. A
/// failed read keeps the incomplete-history error; it is never read as unpaid.
fn before_checkpoint(read: Result<bool, ObservationError>) -> Result<(), &'static str> {
    match read {
        Ok(true) => Ok(()),
        Ok(false) | Err(ObservationError::Inconsistent) => Err(
            "pool providers are inconsistent: the deal is consumed but no settlement after the auditor checkpoint was found; retain caches and retry",
        ),
        Err(_) => Err(
            "deal consumed but its settlement is not in this auditor cache's history; retain caches and retry",
        ),
    }
}

/// Starts a new, empty auditor cache from a paired finalized checkpoint. Never reads logs.
async fn checkpoint_command(request: CheckpointRequest) -> Result<Value, &'static str> {
    if !request.confirm_new_cache {
        return Err("checkpoint requires confirm_new_cache: true for a new auditor cache");
    }
    let Configured {
        pins,
        domain,
        rpc,
        peer_rpc,
        cache_root,
    } = configure(request.deployment, None)?;
    create_private_directory(&cache_root)?;
    let lock = checkpoint::InitLock::acquire(&cache_root).map_err(|error| match error {
        CheckpointError::Busy => "another checkpoint initialization is in progress",
        _ => "checkpoint lock unavailable",
    })?;
    if let Some(existing) = recorded_checkpoint(&cache_root, domain)? {
        // Idempotent: nothing is read from any provider.
        return Ok(checkpoint_view(&existing, "existing"));
    }
    require_empty_cache(&cache_root)?;
    let acquired = rpc
        .acquire_checkpoint_agreed(&peer_rpc, domain, pins)
        .await
        .map_err(|_| "paired checkpoint acquisition failed")?;
    // A verification may have started a full-history cache while the providers were read.
    require_empty_cache(&cache_root)?;
    let persisted = checkpoint::persist(&cache_root, domain, acquired, &lock)
        .map_err(|_| "checkpoint record could not be created")?;
    Ok(checkpoint_view(
        &acquired,
        match persisted {
            Persisted::Created => "created",
            Persisted::Existing => "existing",
        },
    ))
}

fn create_private_directory(path: &Path) -> Result<(), &'static str> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|_| "auditor cache root unavailable")?;
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        _ => Err("auditor cache root must be a real directory"),
    }
}

/// Refuses a cache that already holds or began full history; a checkpoint would only narrow it.
fn require_empty_cache(cache_root: &Path) -> Result<(), &'static str> {
    for name in CACHE_DIRECTORIES {
        match std::fs::symlink_metadata(cache_root.join(name)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(
                    "auditor cache already holds full history; a checkpoint would only narrow it",
                )
            }
            Err(_) => return Err("auditor cache could not be inspected"),
        }
    }
    Ok(())
}

fn checkpoint_view(checkpoint: &TreeCheckpoint, persisted: &str) -> Value {
    json!({
        "status": "ok",
        "checkpoint": persisted,
        "checkpoint_block": checkpoint.block_number,
        "checkpoint_hash": hex::encode(checkpoint.block_hash),
        "leaf_count": checkpoint.leaf_count,
        "root": hex::encode(checkpoint.root),
        "history_from_block": checkpoint.block_number.saturating_add(1),
        "scope": "deals settled at or before checkpoint_block are reported as settled_before_checkpoint, never as paid or unpaid",
        "payment_verified": false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_history_is_pending_but_conflicting_evidence_is_not() {
        let pending = ShieldedDisclosureError::Observation(ObservationError::Recovery(
            RecoveryError::HistoryPending {
                next_block: 101,
                through: 200,
            },
        ));
        assert_eq!(pending_progress(&pending), Some((Some(101), 100)));
        assert_eq!(
            pending_progress(&ShieldedDisclosureError::Observation(
                ObservationError::Inconsistent
            )),
            None
        );
        assert_eq!(
            pending_progress(&ShieldedDisclosureError::Observation(
                ObservationError::FinalityPending
            )),
            Some((None, 0))
        );
    }

    #[test]
    fn a_consumed_deal_missing_after_the_checkpoint_is_never_paid_or_unpaid() {
        // Consumed at the checkpoint block: settled at or before it.
        assert_eq!(before_checkpoint(Ok(true)), Ok(()));
        // Consumed now, not at the checkpoint, yet no later event: providers are inconsistent.
        for read in [Ok(false), Err(ObservationError::Inconsistent)] {
            assert!(before_checkpoint(read)
                .unwrap_err()
                .starts_with("pool providers are inconsistent"));
        }
        // A failed read stays incomplete history.
        for read in [
            Err(ObservationError::FinalityPending),
            Err(ObservationError::Rpc(
                erebus_shielded_prover::rpc::RpcError::Configuration,
            )),
        ] {
            assert!(before_checkpoint(read)
                .unwrap_err()
                .starts_with("deal consumed but its settlement is not in this auditor cache"));
        }
    }

    fn domain() -> IndexDomain {
        IndexDomain {
            chain_id: 31337,
            pool: [3; 20],
            first_block: 10,
            first_hash: [10; 32],
        }
    }

    fn empty_checkpoint(block_number: u64) -> TreeCheckpoint {
        let mut zero = [[0u8; 32]; 20];
        let mut value = [0u8; 32];
        for slot in &mut zero {
            *slot = value;
            value = erebus_core::shielded::note_tree_parent(&value, &value).unwrap();
        }
        TreeCheckpoint {
            block_number,
            block_hash: [block_number as u8; 32],
            leaf_count: 0,
            root: erebus_shielded_prover::indexer::PoolIndex::new(1)
                .unwrap()
                .root(),
            filled_subtrees: zero,
        }
    }

    #[test]
    fn the_record_binds_the_cache_to_its_deployment() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(recorded_checkpoint(root.path(), domain()), Ok(None));
        let lock = checkpoint::InitLock::acquire(root.path()).unwrap();
        checkpoint::persist(root.path(), domain(), empty_checkpoint(20), &lock).unwrap();
        assert_eq!(
            recorded_checkpoint(root.path(), domain()),
            Ok(Some(empty_checkpoint(20)))
        );
        // A record for another chain, pool or deployment anchor is refused, never ignored.
        for foreign in [
            IndexDomain {
                chain_id: 1,
                ..domain()
            },
            IndexDomain {
                pool: [4; 20],
                ..domain()
            },
            IndexDomain {
                first_hash: [11; 32],
                ..domain()
            },
        ] {
            assert_eq!(
                recorded_checkpoint(root.path(), foreign),
                Err("auditor history checkpoint record unavailable")
            );
        }
        // A tampered frontier is refused.
        let path = root.path().join(checkpoint::RECORD_FILE);
        let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["checkpoint"]["root"][0] =
            json!(value["checkpoint"]["root"][0].as_u64().unwrap() ^ 1);
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(
            recorded_checkpoint(root.path(), domain()),
            Err("auditor history checkpoint record unavailable")
        );
    }

    #[test]
    fn only_an_empty_cache_may_take_a_checkpoint() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(require_empty_cache(root.path()), Ok(()));
        std::fs::write(root.path().join("history-checkpoint.lock"), b"").unwrap();
        assert_eq!(require_empty_cache(root.path()), Ok(()));
        for name in CACHE_DIRECTORIES {
            let cache = root.path().join(name);
            std::fs::create_dir(&cache).unwrap();
            assert!(require_empty_cache(root.path())
                .unwrap_err()
                .starts_with("auditor cache already holds full history"));
            std::fs::remove_dir(&cache).unwrap();
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/nonexistent", root.path().join("peer")).unwrap();
            assert!(require_empty_cache(root.path()).is_err());
        }
    }

    #[test]
    fn runtime_pins_are_nonzero_canonical_digests() {
        assert_eq!(digest(&format!("0x{}", "01".repeat(32))), Ok([1; 32]));
        for value in [
            "01".repeat(32),
            format!("0x{}", "00".repeat(32)),
            format!("0x{}", "AB".repeat(32)),
        ] {
            assert!(digest(&value).is_err());
        }
    }
}
