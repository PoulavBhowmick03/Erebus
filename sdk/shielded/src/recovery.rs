//! Confirmed public-event replay into a locally encrypted note wallet.
//!
//! The index cache is public and rebuildable. Wallet openings never leave the
//! operator's process. Confirmation depth is caller-selected, not a finality claim.

use std::collections::HashSet;

use crate::{
    index_store::{IndexStore, IndexStoreError},
    indexer::{IndexError, PoolEvent, PoolIndex},
    rpc::{PoolRpc, RpcError},
    wallet::{WalletError, WalletStore},
};

/// A confirmed-prefix recovery step failed.
#[derive(Debug, thiserror::Error)]
pub enum RecoveryError {
    /// Wallet, index, and RPC refer to different chain instances or pools.
    #[error("wallet, index, and RPC chain or pool identities differ")]
    Deployment,
    /// The chain has not reached the requested confirmation depth.
    #[error("not enough blocks for requested confirmation depth")]
    Confirmations,
    /// A bounded scan saved progress but has not covered the requested history.
    #[error("pool history pending; retry from block {next_block} through {through}")]
    HistoryPending {
        /// First block not yet scanned.
        next_block: u64,
        /// Requested inclusive end of the scan.
        through: u64,
    },
    /// RPC evidence could not be accepted.
    #[error(transparent)]
    Rpc(#[from] RpcError),
    /// Public cache could not be loaded or saved.
    #[error(transparent)]
    IndexStore(#[from] IndexStoreError),
    /// Indexed block history could not be rewound or replayed.
    #[error(transparent)]
    Index(#[from] IndexError),
    /// Local encrypted wallet could not be updated.
    #[error(transparent)]
    Wallet(#[from] WalletError),
}

/// Restores a wallet only through an explicit finalized pool prefix.
///
/// Cached events referring to local notes are re-read from their canonical blocks before
/// replay. A concurrent change to the note inventory requires a retry. No wallet lock crosses
/// network I/O, and failures do not change note reservations or spend state.
pub async fn recover_finalized_wallet(
    rpc: &PoolRpc,
    index_store: &IndexStore,
    wallet_store: &WalletStore,
) -> Result<RecoveryReport, RecoveryError> {
    recover_finalized_wallet_inner(rpc, index_store, wallet_store, None, None).await
}

/// Restores notes only after two distinct RPCs agree on the entire finalized pool prefix.
/// Separate caches, canonical wallet-event reads, and matching anchors are required.
/// Any disagreement or unavailable read leaves the encrypted wallet unchanged.
pub async fn recover_finalized_wallet_agreed(
    rpc: &PoolRpc,
    index_store: &IndexStore,
    peer_rpc: &PoolRpc,
    peer_index: &IndexStore,
    wallet_store: &WalletStore,
) -> Result<RecoveryReport, RecoveryError> {
    if rpc.endpoint() == peer_rpc.endpoint()
        || index_store.domain() != peer_index.domain()
        || index_store.shares_cache(peer_index)?
    {
        return Err(RecoveryError::Deployment);
    }
    recover_finalized_wallet_inner(
        rpc,
        index_store,
        wallet_store,
        Some((peer_rpc, peer_index)),
        None,
    )
    .await
}

/// Performs a paired finalized wallet scan with at most `max_blocks` new blocks per provider.
/// Pending scans persist public progress but never change the wallet or release reservations.
pub async fn recover_finalized_wallet_agreed_bounded(
    rpc: &PoolRpc,
    index_store: &IndexStore,
    peer_rpc: &PoolRpc,
    peer_index: &IndexStore,
    wallet_store: &WalletStore,
    max_blocks: u64,
) -> Result<RecoveryReport, RecoveryError> {
    if rpc.shares_endpoint(peer_rpc)
        || index_store.domain() != peer_index.domain()
        || index_store.shares_cache(peer_index)?
    {
        return Err(RecoveryError::Deployment);
    }
    recover_finalized_wallet_inner(
        rpc,
        index_store,
        wallet_store,
        Some((peer_rpc, peer_index)),
        Some(max_blocks),
    )
    .await
}

async fn recover_finalized_wallet_inner(
    rpc: &PoolRpc,
    index_store: &IndexStore,
    wallet_store: &WalletStore,
    peer: Option<(&PoolRpc, &IndexStore)>,
    max_blocks: Option<u64>,
) -> Result<RecoveryReport, RecoveryError> {
    let domain = index_store.domain();
    if wallet_store.domain().chain_id != domain.chain_id
        || wallet_store.domain().pool != domain.pool
    {
        return Err(RecoveryError::Deployment);
    }
    let snapshot = wallet_store.snapshot()?;
    let commitments: HashSet<_> = snapshot
        .notes()
        .iter()
        .map(|note| note.commitment())
        .collect();
    let nullifiers: HashSet<_> = snapshot
        .notes()
        .iter()
        .map(|note| note.nullifier())
        .collect();
    let relevant = |event: &&PoolEvent| match event {
        PoolEvent::Inserted { commitment, .. } => commitments.contains(commitment),
        PoolEvent::Consumed { nullifier, .. } => nullifiers.contains(nullifier),
        PoolEvent::Transferred {
            input_nullifier, ..
        } => nullifiers.contains(input_nullifier),
    };
    let (index, report) = if let Some((peer_rpc, peer_index)) = peer {
        let (first, second) = tokio::join!(
            sync_finalized_public_index_inner(rpc, index_store, max_blocks),
            sync_finalized_public_index_inner(peer_rpc, peer_index, max_blocks)
        );
        let (index, report) = first?;
        let (peer_index, peer_report) = second?;
        if report != peer_report || index.blocks() != peer_index.blocks() {
            return Err(RecoveryError::Rpc(RpcError::Evidence(
                "RPC providers disagree on finalized wallet history",
            )));
        }
        (index, report)
    } else {
        sync_finalized_public_index_inner(rpc, index_store, max_blocks).await?
    };
    for block in index.blocks() {
        let cached: Vec<_> = block.events.iter().filter(relevant).collect();
        if cached.is_empty() {
            continue;
        }
        let canonical = rpc.read_block(block.number).await?;
        if canonical.hash != block.hash
            || canonical.events.iter().filter(relevant).collect::<Vec<_>>() != cached
        {
            return Err(RecoveryError::Rpc(RpcError::Evidence(
                "cached wallet events differ from canonical pool logs",
            )));
        }
        if let Some((peer_rpc, _)) = peer {
            let canonical = peer_rpc.read_block(block.number).await?;
            if canonical.hash != block.hash
                || canonical.events.iter().filter(relevant).collect::<Vec<_>>() != cached
            {
                return Err(RecoveryError::Rpc(RpcError::Evidence(
                    "peer wallet events differ from canonical pool logs",
                )));
            }
        }
    }
    if let Some((peer_rpc, _)) = peer {
        if peer_rpc.canonical_block_hash(report.through).await? != report.block_hash {
            return Err(RecoveryError::Rpc(RpcError::Evidence(
                "peer finalized wallet anchor changed during recovery",
            )));
        }
    }
    if rpc.canonical_block_hash(report.through).await? != report.block_hash {
        return Err(RecoveryError::Rpc(RpcError::Evidence(
            "finalized wallet anchor changed during recovery",
        )));
    }
    wallet_store.update(|wallet| {
        let current: HashSet<_> = wallet
            .notes()
            .iter()
            .map(|note| note.commitment())
            .collect();
        if current != commitments {
            return Err(WalletError::Note(
                "wallet inventory changed during recovery",
            ));
        }
        index
            .replay_wallet(wallet)
            .map_err(|_| WalletError::Note("finalized public event replay"))
    })?;
    Ok(report)
}

/// Public evidence after a wallet recovery scan. No note openings are included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryReport {
    /// Last scanned block number.
    pub through: u64,
    /// Hash of the scanned block.
    pub block_hash: [u8; 32],
    /// Recomputed and onchain-checked pool root.
    pub root: [u8; 32],
    /// Number of public note commitments observed.
    pub leaves: usize,
}

/// Restores one wallet from a confirmed prefix of the pool's public history.
///
/// An index write is durable before wallet replay. If a crash happens between those
/// writes, re-running this operation replays the same verified index idempotently.
/// This function does not authorize spending, prove, submit, or establish economic
/// finality. A caller must choose `confirmations` for its own chain risk policy.
pub async fn recover_wallet(
    rpc: &PoolRpc,
    index_store: &IndexStore,
    wallet_store: &WalletStore,
    confirmations: u64,
) -> Result<RecoveryReport, RecoveryError> {
    let domain = index_store.domain();
    if wallet_store.domain().chain_id != domain.chain_id
        || wallet_store.domain().pool != domain.pool
    {
        return Err(RecoveryError::Deployment);
    }
    let (index, report) = sync_public_index(rpc, index_store, confirmations).await?;
    wallet_store.update(|wallet| {
        index
            .replay_wallet(wallet)
            .map_err(|_| WalletError::Note("verified public event replay"))
    })?;
    Ok(report)
}

/// Rebuilds the confirmed public pool prefix and persists it for later restart.
///
/// This operation has no access to a wallet, note opening, or spend key. It is shared
/// by the local recovery client and the optional public indexer service.
pub async fn sync_public_index(
    rpc: &PoolRpc,
    index_store: &IndexStore,
    confirmations: u64,
) -> Result<(PoolIndex, RecoveryReport), RecoveryError> {
    let head = rpc.head().await?;
    let through = head
        .checked_sub(confirmations)
        .filter(|through| *through >= index_store.domain().first_block)
        .ok_or(RecoveryError::Confirmations)?;
    sync_public_index_through(rpc, index_store, through).await
}

/// Rebuilds the pool index through one caller-selected height, with durable progress after
/// each verified batch. This method does not assert that `through` is finalized.
pub async fn sync_public_index_through(
    rpc: &PoolRpc,
    index_store: &IndexStore,
    through: u64,
) -> Result<(PoolIndex, RecoveryReport), RecoveryError> {
    let domain = index_store.domain();
    if rpc.chain_id() != domain.chain_id || rpc.pool() != domain.pool {
        return Err(RecoveryError::Deployment);
    }
    if through < domain.first_block {
        return Err(RecoveryError::Confirmations);
    }
    let mut index = index_store.load()?;
    let mut prior_tip = index.tip().map(|tip| tip.hash);
    if index.tip().is_some_and(|tip| tip.number > through) {
        index.rewind_from(through.saturating_add(1))?;
    }
    loop {
        let count = rpc.sync(&mut index, through, 1_000).await?;
        index_store.save_if_unchanged(&index, prior_tip)?;
        prior_tip = index.tip().map(|tip| tip.hash);
        if index.tip().is_some_and(|tip| tip.number >= through) {
            break;
        }
        if count == 0 {
            return Err(RecoveryError::Rpc(RpcError::Evidence(
                "pool scan did not advance",
            )));
        }
    }
    let tip = index.tip().ok_or(RecoveryError::Confirmations)?;
    let report = RecoveryReport {
        through: tip.number,
        block_hash: tip.hash,
        root: index.root(),
        leaves: index.leaves().len(),
    };
    Ok((index, report))
}

/// Saves at most `max_blocks` newly verified blocks before returning.
///
/// `HistoryPending` is progress, not evidence of nonpayment. Reopen the same index and
/// retry. A completed prefix still needs the caller's canonical and finality checks.
/// Cache loading, replay, and serialization remain proportional to stored history.
pub async fn sync_public_index_step(
    rpc: &PoolRpc,
    index_store: &IndexStore,
    through: u64,
    max_blocks: u64,
) -> Result<(PoolIndex, RecoveryReport), RecoveryError> {
    let domain = index_store.domain();
    if rpc.chain_id() != domain.chain_id || rpc.pool() != domain.pool {
        return Err(RecoveryError::Deployment);
    }
    if through < domain.first_block {
        return Err(RecoveryError::Confirmations);
    }
    if max_blocks == 0 || max_blocks > 1_000 {
        return Err(RecoveryError::Rpc(RpcError::Configuration));
    }
    let mut index = index_store.load()?;
    let prior_tip = index.tip().map(|tip| tip.hash);
    if index.tip().is_some_and(|tip| tip.number > through) {
        index.rewind_from(through.saturating_add(1))?;
    }
    rpc.sync(&mut index, through, max_blocks).await?;
    index_store.save_if_unchanged(&index, prior_tip)?;
    let tip = index.tip().ok_or(RecoveryError::Confirmations)?;
    if tip.number < through {
        return Err(RecoveryError::HistoryPending {
            next_block: tip.number + 1,
            through,
        });
    }
    let report = RecoveryReport {
        through: tip.number,
        block_hash: tip.hash,
        root: index.root(),
        leaves: index.leaves().len(),
    };
    Ok((index, report))
}

/// Rebuilds a pool prefix only through an explicit RPC-finalized block.
/// The chosen anchor is rechecked after the scan. A confirmation count is never substituted.
pub async fn sync_finalized_public_index(
    rpc: &PoolRpc,
    index_store: &IndexStore,
) -> Result<(PoolIndex, RecoveryReport), RecoveryError> {
    sync_finalized_public_index_inner(rpc, index_store, None).await
}

async fn sync_finalized_public_index_inner(
    rpc: &PoolRpc,
    index_store: &IndexStore,
    max_blocks: Option<u64>,
) -> Result<(PoolIndex, RecoveryReport), RecoveryError> {
    let anchor = rpc.finalized_head().await?;
    let (index, report) = match max_blocks {
        Some(limit) => sync_public_index_step(rpc, index_store, anchor.number, limit).await?,
        None => sync_public_index_through(rpc, index_store, anchor.number).await?,
    };
    if report.block_hash != anchor.hash
        || rpc.canonical_block_hash(anchor.number).await? != anchor.hash
    {
        return Err(RecoveryError::Rpc(RpcError::Evidence(
            "finalized pool anchor changed during scan",
        )));
    }
    let later = rpc.finalized_head().await?;
    if later.number < anchor.number || rpc.canonical_block_hash(anchor.number).await? != anchor.hash
    {
        return Err(RecoveryError::Rpc(RpcError::Evidence(
            "finalized pool anchor changed during scan",
        )));
    }
    Ok((index, report))
}
