//! Shielded deal evidence from one RPC and a verified public pool prefix.
//!
//! The pool event exposes a deal commitment, not an amount. Amount and zero fee come only
//! from a locally signed revision with the same commitment; an unknown winner remains unresolved.
//! Errors must be treated as unknown evidence by a coordinator, never as proof of nonpayment.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use erebus_coordinator::Coordinator;
use erebus_core::{
    commitment::DealNullifier,
    deal_state::{DealAssessment, DealEvidence, DealReads, SignedRevision, WinningSettlement},
    settlement::SettlementContext,
    shielded::{validate_domain, SHIELDED_GUARANTEES},
    suite::SHIELDED_POSEIDON_EDDSA_SUITE_ID,
    terms::SettlementMode,
};

use crate::{
    index_store::IndexStore,
    indexer::{DealTransfer, PoolBlock, PoolEvent, PoolIndex},
    recovery::{sync_public_index_step, RecoveryError},
    rpc::{PoolFinalizedBlock, PoolIdentity, PoolRpc, PoolSnapshot, RpcError},
};

/// Pool observations cannot safely decide this deal.
#[derive(Debug, thiserror::Error)]
pub enum ObservationError {
    /// The durable operation could not be read or reconciled.
    #[error(transparent)]
    Coordinator(#[from] erebus_coordinator::Error),
    /// Public index synchronization failed.
    #[error(transparent)]
    Recovery(#[from] RecoveryError),
    /// An RPC read or anchor check failed.
    #[error(transparent)]
    Rpc(#[from] RpcError),
    /// The supplied revisions do not describe this one shielded deal.
    #[error("shielded revisions do not describe one deal")]
    Revisions,
    /// The accepted settlement context does not name this pool deployment.
    #[error("shielded settlement context does not match the pool deployment")]
    Context,
    /// The finalized anchor predates the pool deployment.
    #[error("shielded pool deployment has not reached finality")]
    FinalityPending,
    /// The indexed event and consumed-state reads disagree.
    #[error("shielded pool event and consumed state disagree")]
    Inconsistent,
    /// The pool's state shows this deal consumed, but its event predates the index checkpoint.
    /// Neither payment location nor nonpayment can be certified from this index.
    #[error("deal consumed before this index's checkpoint; history is incomplete")]
    HistoryIncomplete,
}

/// Where the pool event that consumed a deal sits on the canonical chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettlementLocation {
    /// Canonical block holding the pool event.
    pub block_number: u64,
    /// Hash of that block, rechecked against the canonical numeric lookup.
    pub block_hash: [u8; 32],
    /// Transaction that emitted the pool event.
    pub transaction_hash: [u8; 32],
    /// Log position of the pool event in its block.
    pub log_index: u32,
}

/// One deal observation together with the anchors it was read at.
///
/// The paired observer returns this only when both providers produced every field identically,
/// including the settlement location and pool identity. It is still not a consensus proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchoredDealObservation {
    /// RPC-finalized anchor the consumed state and finality decision were read at.
    pub finalized: PoolFinalizedBlock,
    /// Head block number the history was synchronized to.
    pub head_number: u64,
    /// Head block hash the history was synchronized to.
    pub head_hash: [u8; 32],
    /// Pool asset and verifier version read at the finalized anchor.
    pub pool: PoolIdentity,
    /// The indexed pool event that consumed the deal, if any.
    pub settlement: Option<SettlementLocation>,
    /// Deal evidence derived from the same reads.
    pub evidence: DealEvidence,
}

/// Reconciles one shielded operation using only its durable openings and verified pool reads.
///
/// An observation error leaves all reservations unchanged. The caller must retry observation;
/// it must not turn a timeout or an inconsistent RPC response into unpaid evidence.
pub async fn reconcile_shielded_deal(
    coordinator: &Coordinator,
    operation_ref: [u8; 32],
    rpc: &PoolRpc,
    index_store: &IndexStore,
    peer_rpc: &PoolRpc,
    peer_index: &IndexStore,
    now: u64,
) -> Result<DealAssessment, ObservationError> {
    let (context, revisions) = coordinator.recorded_deal(operation_ref)?;
    let nullifier = revisions
        .first()
        .ok_or(ObservationError::Revisions)?
        .deal_nullifier();
    let evidence = observe_shielded_deal_agreed(
        rpc,
        index_store,
        peer_rpc,
        peer_index,
        &context,
        &nullifier,
        &revisions,
    )
    .await?;
    Ok(coordinator.reconcile(operation_ref, &evidence, now)?)
}

/// Reads a deal through one pinned head and an explicit RPC-finalized anchor.
///
/// `revisions` must come from the coordinator's verified, durable agreement openings. The
/// returned evidence can drive `Coordinator::reconcile` only after this call succeeds. This
/// uses one internally consistent RPC, not a provider quorum or a contract-code audit.
pub async fn observe_shielded_deal(
    rpc: &PoolRpc,
    index_store: &IndexStore,
    context: &SettlementContext,
    nullifier: &DealNullifier,
    revisions: &[SignedRevision],
) -> Result<DealEvidence, ObservationError> {
    observe_shielded_deal_bounded(rpc, index_store, context, nullifier, revisions, 1_000).await
}

/// Reads at most `max_blocks` new pool blocks before yielding pending history.
/// Retry with the same durable index. Partial history never produces deal evidence.
/// This bounds network scan work, not local cache replay or serialization.
pub async fn observe_shielded_deal_bounded(
    rpc: &PoolRpc,
    index_store: &IndexStore,
    context: &SettlementContext,
    nullifier: &DealNullifier,
    revisions: &[SignedRevision],
    max_blocks: u64,
) -> Result<DealEvidence, ObservationError> {
    Ok(anchored_shielded_deal(
        rpc,
        index_store,
        context,
        nullifier,
        revisions,
        max_blocks,
        None,
    )
    .await?
    .evidence)
}

/// Opt-in observation through two RPCs with separate index caches and matching deployment anchors.
/// Honest tip lag uses a shared canonical snapshot; differing block or deal reads are errors.
/// The operator must arrange independent providers; this does not prove their honesty.
#[allow(clippy::too_many_arguments)]
pub async fn observe_shielded_deal_agreed(
    rpc: &PoolRpc,
    index_store: &IndexStore,
    peer_rpc: &PoolRpc,
    peer_index: &IndexStore,
    context: &SettlementContext,
    nullifier: &DealNullifier,
    revisions: &[SignedRevision],
) -> Result<DealEvidence, ObservationError> {
    observe_shielded_deal_agreed_bounded(
        rpc,
        index_store,
        peer_rpc,
        peer_index,
        context,
        nullifier,
        revisions,
        1_000,
    )
    .await
}

/// Paired deal observation with a bounded number of new blocks per provider.
/// Partial history is never evidence of payment or nonpayment.
#[allow(clippy::too_many_arguments)]
pub async fn observe_shielded_deal_agreed_bounded(
    rpc: &PoolRpc,
    index_store: &IndexStore,
    peer_rpc: &PoolRpc,
    peer_index: &IndexStore,
    context: &SettlementContext,
    nullifier: &DealNullifier,
    revisions: &[SignedRevision],
    max_blocks: u64,
) -> Result<DealEvidence, ObservationError> {
    let (first, second) = paired_observations(
        rpc,
        index_store,
        peer_rpc,
        peer_index,
        context,
        nullifier,
        revisions,
        max_blocks,
        false,
    )
    .await?;
    // Compares exactly what this function has always compared; the stricter certified variant
    // below also requires agreement on the settlement location and pool identity.
    if (
        first.finalized,
        first.head_number,
        first.head_hash,
        &first.evidence,
    ) != (
        second.finalized,
        second.head_number,
        second.head_hash,
        &second.evidence,
    ) {
        return Err(ObservationError::Inconsistent);
    }
    recheck_anchors(rpc, peer_rpc, &first).await?;
    Ok(first.evidence)
}

/// Paired observation that also returns the anchors and settlement location it agreed on.
///
/// Both providers must agree on every returned field, including the settlement block, its
/// transaction hash and log index, and the pool identity at the finalized anchor. A
/// disagreement on any of them is [`ObservationError::Inconsistent`], never unpaid evidence.
#[allow(clippy::too_many_arguments)]
pub async fn observe_shielded_deal_certified_bounded(
    rpc: &PoolRpc,
    index_store: &IndexStore,
    peer_rpc: &PoolRpc,
    peer_index: &IndexStore,
    context: &SettlementContext,
    nullifier: &DealNullifier,
    revisions: &[SignedRevision],
    max_blocks: u64,
) -> Result<AnchoredDealObservation, ObservationError> {
    let (first, second) = paired_observations(
        rpc,
        index_store,
        peer_rpc,
        peer_index,
        context,
        nullifier,
        revisions,
        max_blocks,
        true,
    )
    .await?;
    if first != second {
        return Err(ObservationError::Inconsistent);
    }
    recheck_anchors(rpc, peer_rpc, &first).await?;
    Ok(first)
}

#[allow(clippy::too_many_arguments)]
async fn paired_observations(
    rpc: &PoolRpc,
    index_store: &IndexStore,
    peer_rpc: &PoolRpc,
    peer_index: &IndexStore,
    context: &SettlementContext,
    nullifier: &DealNullifier,
    revisions: &[SignedRevision],
    max_blocks: u64,
    certify_cached_payment: bool,
) -> Result<(AnchoredDealObservation, AnchoredDealObservation), ObservationError> {
    if !index_store.same_history(peer_index)
        || rpc.endpoint() == peer_rpc.endpoint()
        || index_store
            .shares_cache(peer_index)
            .map_err(RecoveryError::from)?
    {
        return Err(ObservationError::Context);
    }
    let mut snapshot = rpc
        .shared_snapshot(peer_rpc)
        .await
        .map_err(|error| match error {
            RpcError::Evidence(_) => ObservationError::Inconsistent,
            other => ObservationError::Rpc(other),
        })?;
    if certify_cached_payment {
        let first = index_store.load().map_err(RecoveryError::from)?;
        let second = peer_index.load().map_err(RecoveryError::from)?;
        if let (Some(first_tip), Some(second_tip), Some(first_payment), Some(second_payment)) = (
            first.tip(),
            second.tip(),
            first.deal_transfer(nullifier.as_bytes()),
            second.deal_transfer(nullifier.as_bytes()),
        ) {
            // A known finalized winner needs its complete prefix, not unrelated later activity.
            // Absence evidence and wallet/coordinator recovery still scan to the current head.
            if first_tip.number == second_tip.number
                && first_tip.hash == second_tip.hash
                && first_tip.number <= snapshot.finalized.number
                && first_payment == second_payment
                && revisions.iter().any(|revision| {
                    revision.commitment().as_bytes() == &first_payment.deal_commitment
                })
            {
                snapshot = rpc
                    .shared_finalized_prefix(peer_rpc, snapshot, first_tip.number, first_tip.hash)
                    .await?;
            }
        }
    }
    let (first, second) = tokio::join!(
        anchored_shielded_deal(
            rpc,
            index_store,
            context,
            nullifier,
            revisions,
            max_blocks,
            Some(snapshot)
        ),
        anchored_shielded_deal(
            peer_rpc,
            peer_index,
            context,
            nullifier,
            revisions,
            max_blocks,
            Some(snapshot)
        )
    );
    Ok((first?, second?))
}

/// Rechecks both agreed anchors on both providers after every dependent read.
async fn recheck_anchors(
    rpc: &PoolRpc,
    peer_rpc: &PoolRpc,
    observed: &AnchoredDealObservation,
) -> Result<(), ObservationError> {
    for source in [rpc, peer_rpc] {
        if source
            .canonical_block_hash(observed.finalized.number)
            .await?
            != observed.finalized.hash
            || source.canonical_block_hash(observed.head_number).await? != observed.head_hash
        {
            return Err(ObservationError::Inconsistent);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn anchored_shielded_deal(
    rpc: &PoolRpc,
    index_store: &IndexStore,
    context: &SettlementContext,
    nullifier: &DealNullifier,
    revisions: &[SignedRevision],
    max_blocks: u64,
    snapshot: Option<PoolSnapshot>,
) -> Result<AnchoredDealObservation, ObservationError> {
    if context.mode != SettlementMode::Shielded
        || context.suite_id != SHIELDED_POSEIDON_EDDSA_SUITE_ID
        || context.required_guarantees.bits() != SHIELDED_GUARANTEES
        || validate_domain(&context.domain).is_err()
        || context.domain.namespace.family() != "eip155"
        || context.domain.namespace.reference().parse::<u64>().ok() != Some(rpc.chain_id())
        || context.domain.pool.as_ref().map(|pool| pool.as_bytes()) != Some(rpc.pool().as_slice())
        || context.asset.namespace() != &context.domain.namespace
        || context.asset.asset_namespace() != "erc20"
    {
        return Err(ObservationError::Context);
    }
    let mut commitments = HashSet::new();
    if revisions.iter().any(|revision| {
        revision.deal_nullifier() != *nullifier
            || revision.fee().get() != 0
            || !commitments.insert(revision.commitment())
    }) {
        return Err(ObservationError::Revisions);
    }

    let finalized = match snapshot {
        Some(snapshot) => snapshot.finalized,
        None => rpc.finalized_head().await?,
    };
    if finalized.number < index_store.domain().first_block {
        return Err(ObservationError::FinalityPending);
    }
    let identity = rpc.pool_identity_at(finalized.hash).await?;
    if !matches_pool_identity(context, identity) {
        return Err(ObservationError::Context);
    }
    let head_number = match snapshot {
        Some(snapshot) => snapshot.head_number,
        None => rpc.head().await?,
    };
    if head_number < finalized.number {
        return Err(ObservationError::Inconsistent);
    }
    let (index, report) = sync_public_index_step(rpc, index_store, head_number, max_blocks).await?;
    let consumed_at_final = rpc
        .consumed_deal_at(*nullifier.as_bytes(), finalized.hash)
        .await?;
    let consumed_at_head = rpc
        .consumed_deal_at(*nullifier.as_bytes(), report.block_hash)
        .await?;
    let settlement = match index.deal_transfer(nullifier.as_bytes()) {
        Some(transfer) => {
            let canonical = rpc.read_block(transfer.block_number).await?;
            if !matches_canonical_transfer(transfer, &canonical) {
                return Err(ObservationError::Inconsistent);
            }
            Some(SettlementLocation {
                block_number: transfer.block_number,
                block_hash: transfer.block_hash,
                transaction_hash: transfer.tx_hash,
                log_index: transfer.log_index,
            })
        }
        None => None,
    };
    let evidence = indexed_evidence(
        &index,
        nullifier,
        revisions,
        finalized,
        consumed_at_final,
        consumed_at_head,
    )?;

    if snapshot.is_some_and(|snapshot| snapshot.head_hash != report.block_hash)
        || rpc.canonical_block_hash(head_number).await? != report.block_hash
        || rpc.canonical_block_hash(finalized.number).await? != finalized.hash
    {
        return Err(ObservationError::Inconsistent);
    }
    let later = rpc.finalized_head().await?;
    if later.number < finalized.number
        || rpc.canonical_block_hash(finalized.number).await? != finalized.hash
    {
        return Err(ObservationError::Inconsistent);
    }
    Ok(AnchoredDealObservation {
        finalized,
        head_number,
        head_hash: report.block_hash,
        pool: identity,
        settlement,
        evidence,
    })
}

fn matches_pool_identity(context: &SettlementContext, identity: PoolIdentity) -> bool {
    let Some(token) = context.asset.asset_reference().strip_prefix("0x") else {
        return false;
    };
    token.len() == 40
        && token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && hex::decode(token).ok().as_deref() == Some(identity.asset.as_slice())
        && context.domain.verifier_version == identity.verifier_version
}

fn matches_canonical_transfer(transfer: &DealTransfer, block: &PoolBlock) -> bool {
    block.number == transfer.block_number
        && block.hash == transfer.block_hash
        && block.events.iter().any(|event| {
            matches!(
                event,
                PoolEvent::Transferred {
                    deal_commitment,
                    deal_nullifier,
                    input_nullifier,
                    tx_hash,
                    log_index,
                } if *deal_commitment == transfer.deal_commitment
                    && *deal_nullifier == transfer.deal_nullifier
                    && *input_nullifier == transfer.input_nullifier
                    && *tx_hash == transfer.tx_hash
                    && *log_index == transfer.log_index
            )
        })
}

fn indexed_evidence(
    index: &PoolIndex,
    nullifier: &DealNullifier,
    revisions: &[SignedRevision],
    finalized: PoolFinalizedBlock,
    consumed_at_final: bool,
    consumed_at_head: bool,
) -> Result<DealEvidence, ObservationError> {
    let transfer = index.deal_transfer(nullifier.as_bytes());
    if index.checkpoint().is_some() && transfer.is_none() && (consumed_at_head || consumed_at_final)
    {
        return Err(ObservationError::HistoryIncomplete);
    }
    if consumed_at_head != transfer.is_some()
        || consumed_at_final
            != transfer.is_some_and(|transfer| transfer.block_number <= finalized.number)
    {
        return Err(ObservationError::Inconsistent);
    }
    let winner = transfer.and_then(|transfer| {
        revisions
            .iter()
            .find(|revision| revision.commitment().as_bytes() == &transfer.deal_commitment)
            .map(|revision| WinningSettlement {
                commitment: revision.commitment(),
                amount: revision.amount(),
                fee: revision.fee(),
                is_final: transfer.block_number <= finalized.number,
            })
    });
    Ok(DealEvidence::Observed(DealReads {
        deal_nullifier: *nullifier,
        final_anchor_timestamp: finalized.timestamp,
        consumed_at_final,
        consumed_at_head,
        winner,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::{PoolBlock, PoolEvent};
    use erebus_core::{commitment::DealCommitment, ids::BaseUnits};

    fn revision() -> SignedRevision {
        SignedRevision::new(
            DealNullifier::from_bytes([9; 32]),
            DealCommitment::from_bytes([8; 32]),
            200,
            BaseUnits::new(70),
            BaseUnits::new(0),
        )
        .expect("revision")
    }

    fn index(with_transfer: bool) -> PoolIndex {
        let mut index = PoolIndex::new(100).expect("index");
        index
            .apply_block(PoolBlock {
                number: 100,
                hash: [1; 32],
                parent_hash: [2; 32],
                events: if with_transfer {
                    vec![PoolEvent::Transferred {
                        deal_commitment: [8; 32],
                        deal_nullifier: [9; 32],
                        input_nullifier: [7; 32],
                        tx_hash: [6; 32],
                        log_index: 0,
                    }]
                } else {
                    Vec::new()
                },
            })
            .expect("block");
        index
    }

    fn finalized() -> PoolFinalizedBlock {
        PoolFinalizedBlock {
            number: 100,
            hash: [1; 32],
            timestamp: 150,
        }
    }

    #[test]
    fn known_winner_uses_only_its_signed_private_opening() {
        let revision = revision();
        let observed = indexed_evidence(
            &index(true),
            &revision.deal_nullifier(),
            std::slice::from_ref(&revision),
            finalized(),
            true,
            true,
        )
        .expect("evidence");
        let DealEvidence::Observed(reads) = observed else {
            panic!("observed")
        };
        let winner = reads.winner.expect("known winner");
        assert_eq!(winner.amount, BaseUnits::new(70));
        assert_eq!(winner.fee, BaseUnits::new(0));
        assert!(winner.is_final);

        let unknown = indexed_evidence(
            &index(true),
            &revision.deal_nullifier(),
            &[],
            finalized(),
            true,
            true,
        )
        .expect("unknown winner is still consumed");
        assert!(matches!(
            unknown,
            DealEvidence::Observed(DealReads {
                consumed_at_final: true,
                winner: None,
                ..
            })
        ));
    }

    #[test]
    fn absent_or_inconsistent_pool_evidence_cannot_release_a_deal() {
        let revision = revision();
        let nullifier = revision.deal_nullifier();
        assert!(matches!(
            indexed_evidence(
                &index(false),
                &nullifier,
                std::slice::from_ref(&revision),
                finalized(),
                false,
                false
            ),
            Ok(DealEvidence::Observed(DealReads {
                winner: None,
                consumed_at_final: false,
                ..
            }))
        ));
        assert!(matches!(
            indexed_evidence(
                &index(false),
                &nullifier,
                std::slice::from_ref(&revision),
                finalized(),
                true,
                true
            ),
            Err(ObservationError::Inconsistent)
        ));
        assert!(matches!(
            indexed_evidence(
                &index(true),
                &nullifier,
                &[revision],
                finalized(),
                false,
                false
            ),
            Err(ObservationError::Inconsistent)
        ));
    }

    #[test]
    fn a_cached_transfer_must_match_the_canonical_rpc_block() {
        let index = index(true);
        let transfer = index.deal_transfer(&[9; 32]).expect("transfer");
        let canonical = index.tip().expect("block").clone();
        assert!(matches_canonical_transfer(transfer, &canonical));

        let mut forged_event = canonical.clone();
        if let PoolEvent::Transferred {
            deal_commitment, ..
        } = &mut forged_event.events[0]
        {
            *deal_commitment = [3; 32];
        }
        assert!(!matches_canonical_transfer(transfer, &forged_event));
        let mut wrong_block = canonical;
        wrong_block.hash = [4; 32];
        assert!(!matches_canonical_transfer(transfer, &wrong_block));
    }
}
