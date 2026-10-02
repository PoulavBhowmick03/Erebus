//! Bounded continuation of complete log scans and canonical parent walks.
//! Checkpoints are trusted local cache state, not independently verifiable chain proofs.

use super::*;
use erebus_journal::{FaultHook, JournalRecord, NoFaults, RecordId, Store};
use std::{path::Path, sync::Arc};

/// A bounded observation is either incomplete or fully verified. Pending is not evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoricalObservation {
    /// More historical work is required. Retain all financial reservations.
    Pending {
        /// Next log range begins here, or None when all log ranges are scanned.
        next_log_block: Option<u64>,
        /// Parent walk resumes here once log scanning completes.
        ancestry_block: u64,
    },
    /// Evidence at explicit pinned anchors, returned only after every check completes.
    Complete {
        /// Explicit RPC finalized anchor.
        finalized: BlockRef,
        /// Pinned head through which the full log history was scanned.
        head: BlockRef,
        /// Verified backend evidence, safe to pass to the classifier.
        evidence: DealEvidence,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CursorId(String);
impl std::fmt::Display for CursorId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl RecordId for CursorId {
    fn as_file_stem(&self) -> &str {
        &self.0
    }
    fn from_file_stem(stem: &str) -> Option<Self> {
        (stem.len() == 64
            && stem
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        .then(|| Self(stem.to_owned()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scan {
    finalized: Block,
    head: Block,
    next_log: Option<u64>,
    logs: Vec<Log>,
    current: Block,
    complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u32,
    id: CursorId,
    generation: u64,
    chain_id: u64,
    contract: [u8; 20],
    verifier: u32,
    nullifier: [u8; 32],
    // Only a fully scanned and ancestry-checked finalized prefix is reusable.
    prefix: Option<Block>,
    prefix_log: Option<Log>,
    scan: Option<Scan>,
}
impl JournalRecord for Cursor {
    type Id = CursorId;
    const CURRENT_VERSION: u32 = 1;
    const OLDEST_READABLE_VERSION: u32 = 1;
    fn version(&self) -> u32 {
        self.version
    }
    fn record_id(&self) -> &CursorId {
        &self.id
    }
    fn attempt_count(&self) -> usize {
        1
    }
}

/// Private, atomic local history checkpoints, separate from financial operation state.
/// Share the directory across processes for this observer. Provider switching is allowed,
/// but every continuation rechecks deployment identity and cached canonical anchors.
pub struct ObservationJournal {
    store: Store<Cursor>,
}
impl ObservationJournal {
    /// Opens a versioned checkpoint directory. Corruption is an error, never an empty scan.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, EvmError> {
        Self::open_with_faults(root, Arc::new(NoFaults))
    }
    /// Opens with crash injection at the journal's durable boundaries.
    pub fn open_with_faults(
        root: impl AsRef<Path>,
        faults: Arc<dyn FaultHook>,
    ) -> Result<Self, EvmError> {
        Ok(Self {
            store: Store::open_with_fault_hook(root.as_ref().to_path_buf(), faults)
                .map_err(|_| inconsistent("history checkpoint unavailable"))?,
        })
    }
    fn read(&self, id: &CursorId) -> Result<Option<Cursor>, EvmError> {
        let _identity = self
            .store
            .lock_identity()
            .map_err(|_| inconsistent("history lock unavailable"))?;
        let _record = self
            .store
            .lock_record(id)
            .map_err(|_| inconsistent("history lock unavailable"))?;
        self.store
            .read(id)
            .map_err(|_| inconsistent("history checkpoint invalid"))
    }
    fn save(&self, expected: &Option<Cursor>, cursor: &mut Cursor) -> Result<(), EvmError> {
        let _identity = self
            .store
            .lock_identity()
            .map_err(|_| inconsistent("history lock unavailable"))?;
        let _record = self
            .store
            .lock_record(&cursor.id)
            .map_err(|_| inconsistent("history lock unavailable"))?;
        if self
            .store
            .read(&cursor.id)
            .map_err(|_| inconsistent("history checkpoint invalid"))?
            != *expected
        {
            return Err(inconsistent("concurrent history continuation"));
        }
        cursor.generation = cursor
            .generation
            .checked_add(1)
            .ok_or(EvmError::ObservationLimit)?;
        self.store
            .write(cursor)
            .map_err(|_| inconsistent("history checkpoint write failed"))
    }
}

impl EvmChain {
    fn history_id(&self, nullifier: &DealNullifier) -> CursorId {
        let mut identity = b"EREBUS_OBSERVATION_CHECKPOINT_V1".to_vec();
        identity.extend(self.deployment.chain_id.to_be_bytes());
        identity.extend(self.deployment.settlement_contract);
        identity.extend(self.deployment.verifier_version.to_be_bytes());
        identity.extend(nullifier.as_bytes());
        CursorId(hex::encode(keccak256(identity)))
    }

    /// Advances two independent checkpoint stores and requires matching completed evidence.
    /// Either pending scan retains reservations. Exact anchor equality is conservative;
    /// honest providers at different heights can disagree. Endpoint count is not consensus.
    pub async fn finalized_deal_evidence_resumable_agreed(
        &self,
        journal: &ObservationJournal,
        peer: &Self,
        peer_journal: &ObservationJournal,
        nullifier: &DealNullifier,
        budget: ObservationLimits,
    ) -> Result<HistoricalObservation, EvmError> {
        self.check_peer(peer)?;
        let first_root = std::fs::canonicalize(journal.store.root())
            .map_err(|_| inconsistent("history checkpoint unavailable"))?;
        let second_root = std::fs::canonicalize(peer_journal.store.root())
            .map_err(|_| inconsistent("history checkpoint unavailable"))?;
        if first_root == second_root {
            return Err(inconsistent("RPC peers must use separate history stores"));
        }
        let first_cursor = journal.read(&self.history_id(nullifier))?;
        let peer_cursor = peer_journal.read(&peer.history_id(nullifier))?;
        // Keep a completed scan pinned while its peer finishes the same historical work.
        let needs_work = |cursor: &Option<Cursor>| {
            cursor
                .as_ref()
                .and_then(|c| c.scan.as_ref())
                .is_some_and(|scan| !scan.complete)
        };
        let (first, second) = tokio::join!(
            self.continue_history(journal, nullifier, budget, !needs_work(&peer_cursor), 0),
            peer.continue_history(
                peer_journal,
                nullifier,
                budget,
                !needs_work(&first_cursor),
                0,
            ),
        );
        let first = first?;
        let second = second?;
        if matches!(first, HistoricalObservation::Pending { .. }) {
            return Ok(first);
        }
        if matches!(second, HistoricalObservation::Pending { .. }) {
            return Ok(second);
        }
        if first != second {
            return Err(inconsistent(
                "RPC providers disagree on anchored deal evidence",
            ));
        }
        if let HistoricalObservation::Complete {
            finalized, head, ..
        } = &first
        {
            for chain in [self, peer] {
                if chain.canonical_block(finalized.number).await?.anchor() != *finalized
                    || chain.canonical_block(head.number).await?.anchor() != *head
                {
                    return Err(inconsistent("agreed anchors changed during comparison"));
                }
            }
        }
        Ok(first)
    }

    /// Advances a complete historical scan by at most the supplied log queries and parent
    /// links. Reopen the same journal and call again on Pending. No lock crosses an await.
    /// A timeout, conflicting writer, changed finalized anchor, or malformed checkpoint
    /// yields an error. Reorganized nonfinal work is restarted from the verified prefix.
    pub async fn finalized_deal_evidence_resumable(
        &self,
        journal: &ObservationJournal,
        nullifier: &DealNullifier,
        budget: ObservationLimits,
    ) -> Result<HistoricalObservation, EvmError> {
        self.continue_history(journal, nullifier, budget, true, 0)
            .await
    }

    /// As [`Self::finalized_deal_evidence_resumable`], but begins a fresh scan at
    /// `start_block` instead of genesis.
    ///
    /// Live chains are far taller than a public RPC's `eth_getLogs` range cap, so a genesis
    /// scan is not possible. `start_block` must be at or before the earliest block in which
    /// this deployment could settle the deal (use the deployment block); a larger value would
    /// omit settlement logs and is the caller's error, not a safe default. Continuations use
    /// the stored checkpoint and ignore this argument.
    pub async fn finalized_deal_evidence_resumable_from(
        &self,
        journal: &ObservationJournal,
        nullifier: &DealNullifier,
        start_block: u64,
        budget: ObservationLimits,
    ) -> Result<HistoricalObservation, EvmError> {
        self.continue_history(journal, nullifier, budget, true, start_block)
            .await
    }

    async fn continue_history(
        &self,
        journal: &ObservationJournal,
        nullifier: &DealNullifier,
        budget: ObservationLimits,
        refresh_completed: bool,
        start_block: u64,
    ) -> Result<HistoricalObservation, EvmError> {
        if budget.log_block_range == 0 || budget.max_log_queries == 0 || budget.max_ancestry == 0 {
            return Err(EvmError::ObservationLimit);
        }
        self.check_chain().await?;
        let id = self.history_id(nullifier);
        let expected = journal.read(&id)?;
        let mut cursor = expected.clone().unwrap_or(Cursor {
            version: 1,
            id,
            generation: 0,
            chain_id: self.deployment.chain_id,
            contract: self.deployment.settlement_contract,
            verifier: self.deployment.verifier_version,
            nullifier: *nullifier.as_bytes(),
            prefix: None,
            prefix_log: None,
            scan: None,
        });
        if cursor.chain_id != self.deployment.chain_id
            || cursor.contract != self.deployment.settlement_contract
            || cursor.verifier != self.deployment.verifier_version
            || cursor.nullifier != *nullifier.as_bytes()
        {
            return Err(inconsistent("history deployment mismatch"));
        }
        if let Some(prefix) = &cursor.prefix {
            self.recheck_block(prefix).await?;
        }
        if let Some(scan) = &cursor.scan {
            self.recheck_block(&scan.finalized).await?;
        }
        let finalized_now = self
            .observation_block("finalized")
            .await?
            .ok_or(EvmError::FinalityUnavailable)?;
        let head_now = self
            .observation_block("latest")
            .await?
            .ok_or_else(|| inconsistent("head missing"))?;
        if finalized_now.number > head_now.number
            || finalized_now.timestamp > head_now.timestamp
            || cursor
                .scan
                .as_ref()
                .is_some_and(|scan| scan.finalized.number > finalized_now.number)
        {
            return Err(inconsistent("history finalized anchor regressed"));
        }
        let refresh = match &cursor.scan {
            None => true,
            Some(scan) if scan.complete && refresh_completed => {
                finalized_now != scan.finalized || head_now != scan.head
            }
            Some(scan) => {
                head_now.number < scan.head.number
                    || (head_now.number == scan.head.number && head_now != scan.head)
                    || self
                        .observation_block(&format!("{:#x}", scan.head.number))
                        .await?
                        .as_ref()
                        != Some(&scan.head)
            }
        };
        if refresh {
            let finalized = finalized_now;
            let head = head_now;
            if finalized.number > head.number
                || finalized.timestamp > head.timestamp
                || cursor.prefix.as_ref().is_some_and(|p| {
                    p.number > finalized.number || p.timestamp > finalized.timestamp
                })
            {
                return Err(inconsistent("history finalized anchor regressed"));
            }
            let from = cursor.prefix.as_ref().map_or(start_block, |prefix| {
                prefix.number.to::<u64>().saturating_add(1)
            });
            cursor.scan = Some(Scan {
                current: head.clone(),
                finalized,
                next_log: (from <= head.number.to::<u64>()).then_some(from),
                head,
                logs: cursor.prefix_log.iter().cloned().collect(),
                complete: false,
            });
        }
        let scan = cursor
            .scan
            .as_mut()
            .ok_or_else(|| inconsistent("history scan missing"))?;
        if scan.logs.len() > 1
            || scan.finalized.number > scan.head.number
            || scan.current.number > scan.head.number
            || scan
                .next_log
                .is_some_and(|from| from > scan.head.number.to::<u64>())
        {
            return Err(inconsistent("history checkpoint bounds invalid"));
        }
        self.recheck_block(&scan.head).await?;
        self.recheck_block(&scan.current).await?;
        for _ in 0..budget.max_log_queries {
            let Some(from) = scan.next_log else {
                break;
            };
            let to = from
                .saturating_add(budget.log_block_range - 1)
                .min(scan.head.number.to());
            let batch: Vec<Log> = self.observation_rpc("eth_getLogs", (json!({
                "address": Address::from(self.deployment.settlement_contract),
                "topics": [Some(B256::from(abi::deal_settled_topic())), Option::<B256>::None,
                    Some(B256::from(*nullifier.as_bytes()))],
                "fromBlock": U64::from(from), "toBlock": U64::from(to),
            }),)).await?;
            for log in batch {
                if !log.block_number.is_some_and(|n| n >= from && n <= to) {
                    return Err(inconsistent("log outside requested range"));
                }
                scan.logs.push(log);
                if scan.logs.len() > 1 {
                    return Err(inconsistent("multiple settlement logs"));
                }
            }
            scan.next_log = (to < scan.head.number.to::<u64>()).then(|| to + 1);
        }
        let mut complete_evidence = None;
        if scan.next_log.is_none() {
            let consumed_final = self.consumed(nullifier, &scan.finalized).await?;
            let consumed_head = self.consumed(nullifier, &scan.head).await?;
            if (consumed_final && !consumed_head) || consumed_head == scan.logs.is_empty() {
                return Err(inconsistent("consumption and settlement logs disagree"));
            }
            let (winner, winner_block) = self
                .log_winner(nullifier, scan.logs.first(), &scan.head, &scan.finalized)
                .await?;
            if winner
                .as_ref()
                .is_some_and(|winner| winner.is_final != consumed_final)
            {
                return Err(inconsistent("winner finality disagrees with consumption"));
            }
            let floor = if scan.complete {
                scan.current.number
            } else {
                cursor.prefix.as_ref().map_or_else(
                    || {
                        winner_block.as_ref().map_or(scan.finalized.number, |b| {
                            b.number.min(scan.finalized.number)
                        })
                    },
                    |prefix| prefix.number,
                )
            };
            if scan.current.number < floor {
                return Err(inconsistent("history ancestry bounds invalid"));
            }
            let mut links = 0;
            loop {
                for anchor in [
                    Some(&scan.finalized),
                    winner_block.as_ref(),
                    cursor.prefix.as_ref(),
                ]
                .into_iter()
                .flatten()
                {
                    if anchor.number == scan.current.number && *anchor != scan.current {
                        return Err(EvmError::AnchorNotCanonical);
                    }
                }
                if scan.current.number == floor {
                    scan.complete = true;
                    break;
                }
                if links == budget.max_ancestry {
                    break;
                }
                let parent = self
                    .canonical_block(scan.current.number.to::<u64>() - 1)
                    .await?;
                if scan.current.parent_hash != parent.hash
                    || parent.timestamp > scan.current.timestamp
                {
                    return Err(EvmError::AnchorNotCanonical);
                }
                scan.current = parent;
                links += 1;
            }
            if let Some(block) = &winner_block {
                self.recheck_block(block).await?;
            }
            if scan.complete {
                complete_evidence = Some(DealEvidence::Observed(DealReads {
                    deal_nullifier: *nullifier,
                    final_anchor_timestamp: scan.finalized.timestamp.to(),
                    consumed_at_final: consumed_final,
                    consumed_at_head: consumed_head,
                    winner,
                }));
            }
        }
        self.recheck_block(&scan.head).await?;
        self.recheck_block(&scan.finalized).await?;
        self.recheck_block(&scan.current).await?;
        if let Some(prefix) = &cursor.prefix {
            self.recheck_block(prefix).await?;
        }
        self.check_chain().await?;
        let outcome = if let Some(evidence) = complete_evidence {
            cursor.prefix = Some(scan.finalized.clone());
            cursor.prefix_log = scan
                .logs
                .first()
                .filter(|log| {
                    log.block_number
                        .is_some_and(|number| number <= scan.finalized.number.to())
                })
                .cloned();
            HistoricalObservation::Complete {
                finalized: scan.finalized.anchor(),
                head: scan.head.anchor(),
                evidence,
            }
        } else {
            HistoricalObservation::Pending {
                next_log_block: scan.next_log,
                ancestry_block: scan.current.number.to(),
            }
        };
        journal.save(&expected, &mut cursor)?;
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use erebus_journal::Boundary;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn cursor() -> Cursor {
        Cursor {
            version: 1,
            id: CursorId("11".repeat(32)),
            generation: 0,
            chain_id: 31337,
            contract: [1; 20],
            verifier: 1,
            nullifier: [2; 32],
            prefix: None,
            prefix_log: None,
            scan: None,
        }
    }

    #[test]
    fn stale_continuation_cannot_overwrite_a_newer_checkpoint() {
        let root = tempfile::tempdir().unwrap();
        let journal = ObservationJournal::open(root.path()).unwrap();
        let mut first = cursor();
        journal.save(&None, &mut first).unwrap();
        let mut stale = cursor();
        assert!(journal.save(&None, &mut stale).is_err());
        assert_eq!(journal.read(&first.id).unwrap(), Some(first));
    }

    struct Sweep {
        fail_at: usize,
        count: AtomicUsize,
    }
    impl FaultHook for Sweep {
        fn after(&self, _: Boundary<'_>) -> std::io::Result<()> {
            let step = self.count.fetch_add(1, Ordering::SeqCst) + 1;
            if step == self.fail_at {
                Err(std::io::Error::other("injected checkpoint crash"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn every_checkpoint_write_boundary_is_restartable() {
        let baseline = tempfile::tempdir().unwrap();
        let hook = Arc::new(Sweep {
            fail_at: usize::MAX,
            count: AtomicUsize::new(0),
        });
        let journal = ObservationJournal::open_with_faults(baseline.path(), hook.clone()).unwrap();
        journal.save(&None, &mut cursor()).unwrap();
        let steps = hook.count.load(Ordering::SeqCst);
        assert!(steps >= 4);
        for fail_at in 1..=steps {
            let root = tempfile::tempdir().unwrap();
            let journal = ObservationJournal::open_with_faults(
                root.path(),
                Arc::new(Sweep {
                    fail_at,
                    count: AtomicUsize::new(0),
                }),
            )
            .unwrap();
            let mut first = cursor();
            assert!(journal.save(&None, &mut first).is_err());
            drop(journal);
            let resumed = ObservationJournal::open(root.path()).unwrap();
            let expected = resumed.read(&first.id).unwrap();
            let mut next = expected.clone().unwrap_or_else(cursor);
            resumed.save(&expected, &mut next).unwrap();
            assert_eq!(resumed.read(&next.id).unwrap(), Some(next));
        }
    }

    #[test]
    fn future_or_malformed_checkpoint_is_not_an_empty_history() {
        let root = tempfile::tempdir().unwrap();
        let journal = ObservationJournal::open(root.path()).unwrap();
        let mut record = cursor();
        journal.save(&None, &mut record).unwrap();
        let path = journal.store.record_path(&record.id);
        let mut value = serde_json::to_value(&record).unwrap();
        value["version"] = json!(2);
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(journal.read(&record.id).is_err());
        std::fs::write(&path, b"invalid checkpoint").unwrap();
        assert!(journal.read(&record.id).is_err());
    }
}
