//! Public pool-event verification independent of any RPC or hosted indexer.
//!
//! A caller must supply canonical blocks from its chosen endpoint and reconcile their hashes.
//! This module checks event order and each emitted Poseidon root before updating wallet state.

use std::collections::HashSet;

use erebus_core::shielded::{note_root_from_path, note_tree_parent, NOTE_TREE_DEPTH};
use serde::{Deserialize, Serialize};

use crate::wallet::{NoteConsumption, NoteInclusion, WalletError, WalletSnapshot};
use crate::NotePath;

const MAX_LEAVES: u32 = 1 << NOTE_TREE_DEPTH;

/// An insertion or spend event decoded from the pool's public logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PoolEvent {
    /// One new tree leaf, with the root emitted by the contract.
    Inserted {
        /// Position in the append-only tree.
        index: u32,
        /// New note commitment.
        commitment: [u8; 32],
        /// Root immediately after this insertion.
        root: [u8; 32],
        /// Transaction that emitted the event.
        tx_hash: [u8; 32],
        /// Log position in the block.
        log_index: u32,
    },
    /// A private transfer or withdrawal consumed one input note.
    Consumed {
        /// Public note nullifier.
        nullifier: [u8; 32],
        /// Transaction that emitted the event.
        tx_hash: [u8; 32],
        /// Log position in the block.
        log_index: u32,
    },
}

impl PoolEvent {
    fn log_index(&self) -> u32 {
        match self {
            Self::Inserted { log_index, .. } | Self::Consumed { log_index, .. } => *log_index,
        }
    }
}

/// One canonical block. Include empty blocks to make parent-hash continuity checkable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolBlock {
    /// EVM block number.
    pub number: u64,
    /// Hash of this block.
    pub hash: [u8; 32],
    /// Hash of the immediately preceding block.
    pub parent_hash: [u8; 32],
    /// Relevant pool events in original log order.
    pub events: Vec<PoolEvent>,
}

/// Invalid chain continuity, Merkle transition, or event ordering.
#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    /// A block does not extend the stored chain.
    #[error("pool block does not extend the indexed chain")]
    BlockLink,
    /// An event is missing, duplicated, out of order, or exceeds the tree capacity.
    #[error("pool event order or leaf index is invalid")]
    EventOrder,
    /// An event has a malformed field element or disagrees with the recomputed root.
    #[error("pool event Merkle root is invalid")]
    Root,
    /// A wallet could not apply a verified public event.
    #[error(transparent)]
    Wallet(#[from] WalletError),
}

/// Append-only verified public tree and its canonical block history.
#[derive(Clone)]
pub struct PoolIndex {
    first_block: u64,
    blocks: Vec<PoolBlock>,
    zero_hashes: [[u8; 32]; NOTE_TREE_DEPTH],
    filled_subtrees: [[u8; 32]; NOTE_TREE_DEPTH],
    leaves: Vec<[u8; 32]>,
    commitments: HashSet<[u8; 32]>,
    nullifiers: HashSet<[u8; 32]>,
    root: [u8; 32],
}

impl PoolIndex {
    /// Starts indexing at the pool deployment block, before its first insertion.
    pub fn new(first_block: u64) -> Result<Self, IndexError> {
        let mut zero_hashes = [[0u8; 32]; NOTE_TREE_DEPTH];
        let mut zero = [0u8; 32];
        for item in &mut zero_hashes {
            *item = zero;
            zero = note_tree_parent(&zero, &zero).map_err(|_| IndexError::Root)?;
        }
        Ok(Self {
            first_block,
            blocks: Vec::new(),
            zero_hashes,
            filled_subtrees: zero_hashes,
            leaves: Vec::new(),
            commitments: HashSet::new(),
            nullifiers: HashSet::new(),
            root: zero,
        })
    }

    /// Last verified block, if any.
    pub fn tip(&self) -> Option<&PoolBlock> {
        self.blocks.last()
    }

    /// Verified block history in canonical order, including empty blocks.
    pub fn blocks(&self) -> &[PoolBlock] {
        &self.blocks
    }

    /// First block whose logs this index expects to process.
    pub fn first_block(&self) -> u64 {
        self.first_block
    }

    /// Current locally verified pool root.
    pub fn root(&self) -> [u8; 32] {
        self.root
    }

    /// All verified leaves in insertion order.
    pub fn leaves(&self) -> &[[u8; 32]] {
        &self.leaves
    }

    /// Constructs an inclusion path against the current verified root.
    pub fn path(&self, index: u32) -> Result<NotePath, IndexError> {
        if index as usize >= self.leaves.len() {
            return Err(IndexError::EventOrder);
        }
        let mut nodes = self.leaves.clone();
        let mut position = index as usize;
        let mut siblings = [[0u8; 32]; NOTE_TREE_DEPTH];
        for (level, sibling) in siblings.iter_mut().enumerate() {
            *sibling = nodes
                .get(position ^ 1)
                .copied()
                .unwrap_or(self.zero_hashes[level]);
            nodes = nodes
                .chunks(2)
                .map(|pair| {
                    note_tree_parent(
                        &pair[0],
                        &pair.get(1).copied().unwrap_or(self.zero_hashes[level]),
                    )
                    .map_err(|_| IndexError::Root)
                })
                .collect::<Result<Vec<_>, _>>()?;
            position >>= 1;
        }
        let root = note_root_from_path(&self.leaves[index as usize], index, &siblings)
            .map_err(|_| IndexError::Root)?;
        if root != self.root || nodes.first().copied() != Some(self.root) {
            return Err(IndexError::Root);
        }
        Ok(NotePath {
            index,
            siblings,
            root,
        })
    }

    /// Checks and applies one consecutive canonical block atomically.
    pub fn apply_block(&mut self, block: PoolBlock) -> Result<(), IndexError> {
        match self.blocks.last() {
            Some(previous)
                if block.number != previous.number + 1 || block.parent_hash != previous.hash =>
            {
                return Err(IndexError::BlockLink);
            }
            None if block.number != self.first_block => return Err(IndexError::BlockLink),
            _ => {}
        }
        if block.hash == [0; 32] || block.hash == block.parent_hash {
            return Err(IndexError::BlockLink);
        }
        let mut filled = self.filled_subtrees;
        let mut root = self.root;
        let mut next = self.leaves.len() as u32;
        let mut last_log = None;
        let mut additions = Vec::new();
        let mut new_commitments = HashSet::new();
        let mut new_nullifiers = HashSet::new();
        for event in &block.events {
            if last_log.is_some_and(|last| event.log_index() <= last) {
                return Err(IndexError::EventOrder);
            }
            last_log = Some(event.log_index());
            match event {
                PoolEvent::Inserted {
                    index,
                    commitment,
                    root: claimed,
                    tx_hash,
                    ..
                } => {
                    if *index != next
                        || next >= MAX_LEAVES
                        || *tx_hash == [0; 32]
                        || self.commitments.contains(commitment)
                        || !new_commitments.insert(*commitment)
                    {
                        return Err(IndexError::EventOrder);
                    }
                    let mut node = *commitment;
                    let mut position = next;
                    for (level, subtree) in filled.iter_mut().enumerate() {
                        node = if position & 1 == 0 {
                            *subtree = node;
                            note_tree_parent(&node, &self.zero_hashes[level])
                        } else {
                            note_tree_parent(subtree, &node)
                        }
                        .map_err(|_| IndexError::Root)?;
                        position >>= 1;
                    }
                    if node != *claimed {
                        return Err(IndexError::Root);
                    }
                    root = node;
                    additions.push(*commitment);
                    next += 1;
                }
                PoolEvent::Consumed {
                    nullifier, tx_hash, ..
                } => {
                    if *nullifier == [0; 32]
                        || *tx_hash == [0; 32]
                        || self.nullifiers.contains(nullifier)
                        || !new_nullifiers.insert(*nullifier)
                    {
                        return Err(IndexError::EventOrder);
                    }
                }
            }
        }
        self.filled_subtrees = filled;
        self.root = root;
        self.leaves.extend(additions);
        self.commitments.extend(new_commitments);
        self.nullifiers.extend(new_nullifiers);
        self.blocks.push(block);
        Ok(())
    }

    /// Rewinds from a reorg height and deterministically rebuilds the remaining prefix.
    pub fn rewind_from(&mut self, height: u64) -> Result<(), IndexError> {
        let retained: Vec<_> = self
            .blocks
            .iter()
            .filter(|block| block.number < height)
            .cloned()
            .collect();
        let mut rebuilt = Self::new(self.first_block)?;
        for block in retained {
            rebuilt.apply_block(block)?;
        }
        *self = rebuilt;
        Ok(())
    }

    /// Rebuilds a private wallet's chain observations from verified public events.
    ///
    /// Local note openings and unresolved reservations are retained. Callers must verify
    /// that this index's tip hash matches their chosen canonical RPC endpoint first.
    pub fn replay_wallet(&self, wallet: &mut WalletSnapshot) -> Result<(), IndexError> {
        let mut staged = wallet.clone();
        staged.rewind_from(self.first_block);
        for block in &self.blocks {
            for event in &block.events {
                match event {
                    PoolEvent::Inserted {
                        index,
                        commitment,
                        root,
                        ..
                    } => {
                        staged.observe_insertion(
                            commitment,
                            NoteInclusion {
                                index: *index,
                                block_number: block.number,
                                block_hash: block.hash,
                                root: *root,
                            },
                        )?;
                    }
                    PoolEvent::Consumed {
                        nullifier, tx_hash, ..
                    } => {
                        staged.observe_consumption(
                            nullifier,
                            NoteConsumption {
                                block_number: block.number,
                                block_hash: block.hash,
                                tx_hash: *tx_hash,
                            },
                        )?;
                    }
                }
            }
        }
        *wallet = staged;
        Ok(())
    }
}
