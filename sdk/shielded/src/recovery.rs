//! Confirmed public-event replay into a locally encrypted note wallet.
//!
//! The index cache is public and rebuildable. Wallet openings never leave the
//! operator's process. Confirmation depth is caller-selected, not a finality claim.

use std::collections::HashSet;

use futures_util::stream::{self, TryStreamExt};

use crate::{
    index_store::{IndexStore, IndexStoreError},
    indexer::{IndexError, PoolBlock, PoolEvent, PoolIndex},
    rpc::{PoolRpc, RpcError},
    wallet::{WalletError, WalletStore},
};

/// Wallet-event block re-reads kept in flight per provider during finalized recovery.
const WALLET_EVENT_REREADS: usize = crate::rpc::DEFAULT_MAX_CONCURRENT_READS as usize;

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

impl RecoveryError {
    /// Whether a provider kept throttling reads after every bounded backoff.
    pub fn is_rate_limited(&self) -> bool {
        matches!(self, Self::Rpc(RpcError::RateLimited(_)))
    }
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
        || !index_store.same_history(peer_index)
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
        || !index_store.same_history(peer_index)
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
        let anchor = rpc.shared_snapshot(peer_rpc).await?.finalized;
        let (first, second) = tokio::join!(
            sync_finalized_public_index_at(rpc, index_store, max_blocks, anchor),
            sync_finalized_public_index_at(peer_rpc, peer_index, max_blocks, anchor)
        );
        let (index, report) = first?;
        let (peer_index, peer_report) = second?;
        // The history digest covers every block and event of both finalized prefixes.
        if report != peer_report || index.history_digest() != peer_index.history_digest() {
            return Err(RecoveryError::Rpc(RpcError::Evidence(
                "RPC providers disagree on finalized wallet history",
            )));
        }
        (index, report)
    } else {
        sync_finalized_public_index_inner(rpc, index_store, max_blocks).await?
    };
    // Re-read every block holding one of this wallet's events from each provider. The reads are
    // independent: each provider's run concurrently (bounded), and the two providers together.
    // Every block is still compared in full, and both anchors are rechecked afterwards.
    let wallet_blocks: Vec<_> = index
        .event_blocks()
        .map(|block| {
            (
                block,
                block.events.iter().filter(relevant).collect::<Vec<_>>(),
            )
        })
        .filter(|(_, cached)| !cached.is_empty())
        .collect();
    let peer_reread = async {
        match peer {
            Some((peer_rpc, _)) => {
                reread_wallet_blocks(
                    peer_rpc,
                    &wallet_blocks,
                    &relevant,
                    "peer wallet events differ from canonical pool logs",
                )
                .await
            }
            None => Ok(()),
        }
    };
    tokio::try_join!(
        reread_wallet_blocks(
            rpc,
            &wallet_blocks,
            &relevant,
            "cached wallet events differ from canonical pool logs"
        ),
        peer_reread
    )?;
    let peer_anchor = async {
        match peer {
            Some((peer_rpc, _)) => peer_rpc
                .canonical_block_hash(report.through)
                .await
                .map(Some),
            None => Ok(None),
        }
    };
    let (anchor, peer_anchor) =
        tokio::try_join!(rpc.canonical_block_hash(report.through), peer_anchor)?;
    if peer_anchor.is_some_and(|hash| hash != report.block_hash) {
        return Err(RecoveryError::Rpc(RpcError::Evidence(
            "peer finalized wallet anchor changed during recovery",
        )));
    }
    if anchor != report.block_hash {
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

/// Re-reads each cached wallet-event block from one provider, bounded concurrently, and
/// requires the canonical hash and this wallet's events to match the cache exactly.
async fn reread_wallet_blocks(
    source: &PoolRpc,
    blocks: &[(&PoolBlock, Vec<&PoolEvent>)],
    relevant: &(dyn Fn(&&PoolEvent) -> bool + Sync),
    differs: &'static str,
) -> Result<(), RecoveryError> {
    stream::iter(blocks.iter().map(Ok::<_, RecoveryError>))
        .try_for_each_concurrent(WALLET_EVENT_REREADS, |(block, cached)| async move {
            let canonical = source.read_block(block.number).await?;
            if canonical.hash != block.hash
                || canonical.events.iter().filter(relevant).collect::<Vec<_>>() != *cached
            {
                return Err(RecoveryError::Rpc(RpcError::Evidence(differs)));
            }
            Ok(())
        })
        .await
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
    if through < domain.first_block || before_checkpoint(index_store, through) {
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
        if index.tail_blocks().len() as u64 > index_store.retained_tail_limit() {
            // Reloading seals what the store sealed, so a long scan does not keep every block.
            index = index_store.load()?;
        }
        prior_tip = index.tip().map(|tip| tip.hash);
        if index
            .covered_through()
            .is_some_and(|(number, _)| number >= through)
        {
            break;
        }
        if count == 0 {
            return Err(RecoveryError::Rpc(RpcError::Evidence(
                "pool scan did not advance",
            )));
        }
    }
    let report = covered_report(&index)?;
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
    // One bounded step at a time per cache within this process (for example the access service's
    // background sync and a request), so two writers never repeat the same reads.
    let step_lock = step_lock(index_store.path());
    let _step = step_lock.lock().await;
    sync_public_index_step_locked(rpc, index_store, through, max_blocks).await
}

/// One bounded step only if no other step on this cache is running in this process; `Ok(None)`
/// when one is. Background sync uses it so it always yields to requests.
pub async fn sync_public_index_step_when_idle(
    rpc: &PoolRpc,
    index_store: &IndexStore,
    through: u64,
    max_blocks: u64,
) -> Result<Option<(PoolIndex, RecoveryReport)>, RecoveryError> {
    let step_lock = step_lock(index_store.path());
    let Ok(_step) = step_lock.try_lock() else {
        return Ok(None);
    };
    sync_public_index_step_locked(rpc, index_store, through, max_blocks)
        .await
        .map(Some)
}

async fn sync_public_index_step_locked(
    rpc: &PoolRpc,
    index_store: &IndexStore,
    through: u64,
    max_blocks: u64,
) -> Result<(PoolIndex, RecoveryReport), RecoveryError> {
    let domain = index_store.domain();
    if rpc.chain_id() != domain.chain_id || rpc.pool() != domain.pool {
        return Err(RecoveryError::Deployment);
    }
    if through < domain.first_block || before_checkpoint(index_store, through) {
        return Err(RecoveryError::Confirmations);
    }
    if max_blocks == 0 || max_blocks > 1_000 {
        return Err(RecoveryError::Rpc(RpcError::Configuration));
    }
    // A shared cache can be advanced by another process (a background sync, a second command)
    // while this step reads the chain. Losing that save race keeps the winner's verified progress:
    // reload it and continue from there, a bounded number of times, before reporting progress only.
    let mut attempts = 0;
    let index = loop {
        let mut index = index_store.load()?;
        let prior_tip = index.tip().map(|tip| tip.hash);
        let resume = covered_report(&index)
            .map(|report| report.through.saturating_add(1))
            .unwrap_or(domain.first_block);
        if index.tip().is_some_and(|tip| tip.number > through) {
            index.rewind_from(through.saturating_add(1))?;
        }
        rpc.sync(&mut index, through, max_blocks).await?;
        match index_store.save_if_unchanged(&index, prior_tip) {
            Err(IndexStoreError::Stale) if attempts < STALE_RETRIES => attempts += 1,
            Err(IndexStoreError::Stale) => {
                return Err(RecoveryError::HistoryPending {
                    next_block: resume,
                    through,
                })
            }
            saved => {
                saved?;
                break index;
            }
        }
    };
    let report = covered_report(&index)?;
    if report.through < through {
        return Err(RecoveryError::HistoryPending {
            next_block: report.through + 1,
            through,
        });
    }
    Ok((index, report))
}

/// Blocks one background catch-up step reads: about 100 header reads, one `eth_getLogs`, and a
/// few identity and state checks, so a request that needs the same cache waits at most one short
/// step (about five seconds at 20 requests per second) for its lock.
pub const BACKGROUND_STEP_BLOCKS: u64 = 100;

/// Keeps one verified history cache at its provider's head, forever, in bounded steps.
///
/// While the cache is behind, steps run back to back with no pause, so catch-up is bounded by the
/// provider's request budget rather than by a fixed rate: at about one request per block it
/// clearly outpaces a chain making a few blocks per second. It yields entirely while `busy()`
/// reports a request in flight (requests read through the same budget and sync the cache
/// themselves) and whenever a request holds the cache's step lock. At the head it rechecks every
/// few seconds. Errors are passed to `failed` and retried after a pause; they never change what
/// a request may accept.
pub async fn keep_history_current(
    rpc: &PoolRpc,
    index_store: &IndexStore,
    busy: impl Fn() -> bool,
    mut failed: impl FnMut(&RecoveryError),
) {
    use std::time::Duration;
    loop {
        if busy() {
            tokio::time::sleep(Duration::from_millis(250)).await;
            continue;
        }
        let outcome = match rpc.head().await {
            Ok(head) => {
                sync_public_index_step_when_idle(rpc, index_store, head, BACKGROUND_STEP_BLOCKS)
                    .await
            }
            Err(error) => Err(error.into()),
        };
        let pause = match outcome {
            // At the head.
            Ok(Some(_)) => Duration::from_secs(5),
            // A request is stepping this cache now.
            Ok(None) => Duration::from_millis(250),
            // Behind: take the next step at once.
            Err(RecoveryError::HistoryPending { .. }) => Duration::ZERO,
            Err(error) => {
                failed(&error);
                Duration::from_secs(10)
            }
        };
        if !pause.is_zero() {
            tokio::time::sleep(pause).await;
        }
    }
}

fn step_lock(path: &std::path::Path) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    static LOCKS: std::sync::OnceLock<
        std::sync::Mutex<
            std::collections::HashMap<std::path::PathBuf, std::sync::Arc<tokio::sync::Mutex<()>>>,
        >,
    > = std::sync::OnceLock::new();
    let locks = LOCKS.get_or_init(Default::default);
    let mut locks = locks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    std::sync::Arc::clone(locks.entry(path.to_path_buf()).or_default())
}

/// How often a bounded step reloads and continues after another writer saved the same cache first.
const STALE_RETRIES: u32 = 3;

/// Whether `through` predates a checkpoint's end state, which this cache cannot represent.
fn before_checkpoint(index_store: &IndexStore, through: u64) -> bool {
    index_store
        .checkpoint()
        .is_some_and(|checkpoint| through < checkpoint.block_number)
}

/// The verified end state: the scanned tip, or the checkpoint before any later block is scanned.
fn covered_report(index: &PoolIndex) -> Result<RecoveryReport, RecoveryError> {
    let (through, block_hash) = index
        .covered_through()
        .ok_or(RecoveryError::Confirmations)?;
    Ok(RecoveryReport {
        through,
        block_hash,
        root: index.root(),
        leaves: index.leaf_count() as usize,
    })
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
    sync_finalized_public_index_at(rpc, index_store, max_blocks, anchor).await
}

async fn sync_finalized_public_index_at(
    rpc: &PoolRpc,
    index_store: &IndexStore,
    max_blocks: Option<u64>,
    anchor: crate::rpc::PoolFinalizedBlock,
) -> Result<(PoolIndex, RecoveryReport), RecoveryError> {
    if index_store
        .load()?
        .covered_through()
        .is_some_and(|(number, _)| number > anchor.number)
    {
        return Err(RecoveryError::Rpc(RpcError::Evidence(
            "finalized wallet history cannot regress to a lagging anchor",
        )));
    }
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

#[cfg(test)]
mod finalized_prefix_tests {
    use super::*;
    use crate::{index_store::IndexDomain, indexer::PoolBlock, rpc::PoolFinalizedBlock};

    #[tokio::test]
    async fn lagging_finality_cannot_rewind_a_retained_wallet_prefix() {
        let directory = tempfile::tempdir().unwrap();
        let domain = IndexDomain {
            chain_id: 31337,
            pool: [3; 20],
            first_block: 2,
            first_hash: [2; 32],
        };
        let store = IndexStore::new(directory.path().join("index.json"), domain).unwrap();
        let mut index = PoolIndex::new(2).unwrap();
        for number in 2..=3 {
            index
                .apply_block(PoolBlock {
                    number,
                    hash: [number as u8; 32],
                    parent_hash: [(number - 1) as u8; 32],
                    events: Vec::new(),
                })
                .unwrap();
        }
        store.save_if_unchanged(&index, None).unwrap();
        let rpc = PoolRpc::new("http://127.0.0.1:9", 31337, domain.pool).unwrap();
        let result = sync_finalized_public_index_at(
            &rpc,
            &store,
            Some(10),
            PoolFinalizedBlock {
                number: 2,
                hash: [2; 32],
                timestamp: 100,
            },
        )
        .await;
        assert!(matches!(
            result,
            Err(RecoveryError::Rpc(RpcError::Evidence(
                "finalized wallet history cannot regress to a lagging anchor"
            )))
        ));
        assert_eq!(store.load().unwrap().tail_blocks(), index.tail_blocks());
    }
}
