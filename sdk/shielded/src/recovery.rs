//! Confirmed public-event replay into a locally encrypted note wallet.
//!
//! The index cache is public and rebuildable. Wallet openings never leave the
//! operator's process. Confirmation depth is caller-selected, not a finality claim.

use crate::{
    index_store::{IndexStore, IndexStoreError},
    indexer::{IndexError, PoolIndex},
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
    let domain = index_store.domain();
    if rpc.chain_id() != domain.chain_id || rpc.pool() != domain.pool {
        return Err(RecoveryError::Deployment);
    }
    let head = rpc.head().await?;
    let through = head
        .checked_sub(confirmations)
        .filter(|through| *through >= domain.first_block)
        .ok_or(RecoveryError::Confirmations)?;
    let mut index = index_store.load()?;
    let prior_tip = index.tip().map(|tip| tip.hash);
    if index.tip().is_some_and(|tip| tip.number > through) {
        index.rewind_from(through.saturating_add(1))?;
    }
    loop {
        let count = rpc.sync(&mut index, through, 1_000).await?;
        if index.tip().is_some_and(|tip| tip.number >= through) {
            break;
        }
        if count == 0 {
            return Err(RecoveryError::Rpc(RpcError::Evidence(
                "pool scan did not advance",
            )));
        }
    }
    index_store.save_if_unchanged(&index, prior_tip)?;
    let tip = index.tip().ok_or(RecoveryError::Confirmations)?;
    let report = RecoveryReport {
        through: tip.number,
        block_hash: tip.hash,
        root: index.root(),
        leaves: index.leaves().len(),
    };
    Ok((index, report))
}
