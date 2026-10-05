//! Bounded, fail-closed observations of one RPC's canonical chain.
//!
//! `finalized` is required explicitly; confirmations and `safe` are never substitutes.
//! These are consistency checks, not consensus or receipt proofs against a malicious RPC.
//! No Monad finality compatibility is implied. Errors must retain reservations.

use alloy::consensus::Transaction as _;
use alloy::eips::eip2718::Encodable2718;
use alloy::network::TransactionResponse;
use alloy::primitives::{keccak256, Address, Bytes, B256, U64};
use alloy::providers::Provider;
use alloy::rpc::json_rpc::{RpcRecv, RpcSend};
use alloy::rpc::types::{Log, Transaction};
use erebus_core::auth::{verify_authorization_signature, Authorization, Role};
use erebus_core::commitment::{
    commit_agreement, deal_nullifier, CommitmentBlinding, DealNullifier,
};
use erebus_core::deal_state::{DealEvidence, DealReads, WinningSettlement};
use erebus_core::ids::{BaseUnits, SignatureBytes};
use erebus_core::terms::AgreementTerms;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::EvmChain;
use crate::x402::{encode_nonce_bitmap_call, transfer_topic, X402ExactEvidence, PERMIT2};
use crate::{abi, backend::validate_terms, error::EvmError};

mod history;
pub use history::{HistoricalObservation, ObservationJournal};

/// A pinned canonical block, as reported by the configured RPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockRef {
    /// Block height.
    pub number: u64,
    /// Block hash.
    pub hash: [u8; 32],
    /// Unix timestamp.
    pub timestamp: u64,
}

/// Account nonce read at an explicit, rechecked finalized anchor.
///
/// This establishes nonce consumption only, not whether any deal paid. Claim release must
/// also reconcile deal evidence and validate the journal's signer and nonce ownership.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinalizedNonce {
    sender: [u8; 20],
    nonce: u64,
    chain_id: u64,
    settlement_contract: [u8; 20],
    anchor: BlockRef,
}

impl FinalizedNonce {
    /// Account whose next nonce was read.
    pub const fn sender(&self) -> [u8; 20] {
        self.sender
    }
    /// Next nonce at the finalized anchor.
    pub const fn nonce(&self) -> u64 {
        self.nonce
    }
    /// Chain checked before and after the read.
    pub const fn chain_id(&self) -> u64 {
        self.chain_id
    }
    /// Settlement deployment associated with this observer.
    pub const fn settlement_contract(&self) -> [u8; 20] {
        self.settlement_contract
    }
    /// Canonical finalized block at which the nonce was read.
    pub const fn anchor(&self) -> BlockRef {
        self.anchor
    }
}

/// Limits on historical work. Exhaustion is an error, never absence evidence.
#[derive(Debug, Clone, Copy)]
pub struct ObservationLimits {
    /// Blocks per log query, starting from genesis so history cannot be omitted.
    pub log_block_range: u64,
    /// Maximum number of log queries.
    pub max_log_queries: u64,
    /// Maximum parent links from head to the oldest required anchor or winner.
    pub max_ancestry: u64,
    /// Maximum `eth_getLogs` queries in flight in one bounded slice.
    ///
    /// Ranges remain contiguous and non-overlapping; a slice is checkpointed only after every
    /// query in it succeeds, so a failed or interrupted slice resumes from the same start.
    pub max_concurrent_queries: u64,
}

/// Hard ceiling on in-flight log queries, independent of caller configuration.
pub const MAX_CONCURRENT_LOG_QUERIES: u64 = 32;

impl Default for ObservationLimits {
    fn default() -> Self {
        Self {
            log_block_range: 2_000,
            max_log_queries: 1_024,
            max_ancestry: 8_192,
            max_concurrent_queries: 8,
        }
    }
}

/// A canonical receipt bound to its transaction and block position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inclusion {
    /// The including block, checked before and after receipt/transaction reads.
    pub block: BlockRef,
    /// Receipt status; a revert says nothing about other attempts for the deal.
    pub success: bool,
    /// Transaction sender recovered from the signed envelope.
    pub from: [u8; 20],
    /// Destination, absent for contract creation.
    pub to: Option<[u8; 20]>,
    /// Sender nonce consumed by this transaction.
    pub nonce: u64,
    /// Transaction position in the block.
    pub transaction_index: u64,
}

/// Absence and pending status never prove that a deal cannot settle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxStatus {
    /// Neither a receipt nor a transaction was returned.
    NotFound,
    /// A validated transaction was returned without inclusion evidence.
    Pending,
    /// Canonical inclusion, not necessarily finalized.
    Included(Inclusion),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct Block {
    number: U64,
    hash: B256,
    parent_hash: B256,
    timestamp: U64,
    transactions: Vec<B256>,
}

impl Block {
    fn anchor(&self) -> BlockRef {
        BlockRef {
            number: self.number.to(),
            hash: self.hash.0,
            timestamp: self.timestamp.to(),
        }
    }
}

// Required status avoids interpreting a missing status or pre-Byzantium root as a revert.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Receipt {
    transaction_hash: B256,
    transaction_index: U64,
    block_hash: B256,
    block_number: U64,
    from: Address,
    to: Option<Address>,
    status: U64,
    logs: Vec<Log>,
}

fn inconsistent(detail: &'static str) -> EvmError {
    EvmError::InconsistentObservation(detail)
}

impl EvmChain {
    /// Checks an operator-pinned runtime and its first deployment block at a finalized
    /// anchor. The preceding block must have no code at this address, so a later scan start
    /// cannot silently omit earlier settlements. This supports immutable deployments only.
    /// The pin must come from an independently verified deployment record. Matching RPC
    /// reads do not authenticate malicious providers or constitute a contract audit.
    pub async fn authenticate_deployment_runtime(
        &self,
        expected_runtime_hash: [u8; 32],
        first_block: u64,
        first_hash: [u8; 32],
    ) -> Result<BlockRef, EvmError> {
        if expected_runtime_hash == [0; 32] || first_hash == [0; 32] {
            return Err(EvmError::DeploymentMismatch);
        }
        self.check_chain().await?;
        let finalized = self
            .observation_block("finalized")
            .await?
            .ok_or(EvmError::FinalityUnavailable)?;
        if finalized.number.to::<u64>() < first_block {
            return Err(EvmError::FinalityUnavailable);
        }
        let first = self.canonical_block(first_block).await?;
        if first.hash.0 != first_hash {
            return Err(EvmError::DeploymentMismatch);
        }
        for block in [&first, &finalized] {
            let code = self.runtime_at(block.hash).await?;
            if code.is_empty() || keccak256(code).0 != expected_runtime_hash {
                return Err(EvmError::DeploymentMismatch);
            }
        }
        if first_block != 0 {
            let parent = self.canonical_block(first_block - 1).await?;
            if !self.runtime_at(parent.hash).await?.is_empty() {
                return Err(EvmError::DeploymentMismatch);
            }
            self.recheck_block(&parent).await?;
        }
        self.recheck_block(&first).await?;
        self.recheck_block(&finalized).await?;
        self.check_chain().await?;
        Ok(finalized.anchor())
    }

    /// Authenticates both immutable runtimes at one block finalized by both providers.
    /// Different current tips are allowed; different canonical hashes or runtime pins are not.
    pub async fn authenticate_deployment_runtime_agreed(
        &self,
        peer: &Self,
        expected_runtime_hash: [u8; 32],
        first_block: u64,
        first_hash: [u8; 32],
    ) -> Result<BlockRef, EvmError> {
        let (finalized, _) = self.common_observation_blocks(peer).await?;
        for chain in [self, peer] {
            let authenticated = chain
                .authenticate_deployment_runtime(expected_runtime_hash, first_block, first_hash)
                .await?;
            if authenticated.number < finalized.number.to::<u64>()
                || keccak256(chain.runtime_at(finalized.hash).await?).0 != expected_runtime_hash
            {
                return Err(EvmError::DeploymentMismatch);
            }
            chain.recheck_block(&finalized).await?;
        }
        Ok(finalized.anchor())
    }

    async fn common_observation_blocks(&self, peer: &Self) -> Result<(Block, Block), EvmError> {
        self.check_peer(peer)?;
        let mut reports = Vec::with_capacity(2);
        for chain in [self, peer] {
            chain.check_chain().await?;
            let finalized = chain
                .observation_block("finalized")
                .await?
                .ok_or(EvmError::FinalityUnavailable)?;
            let head = chain
                .observation_block("latest")
                .await?
                .ok_or_else(|| inconsistent("head missing"))?;
            if finalized.number > head.number || finalized.timestamp > head.timestamp {
                return Err(inconsistent("finalized anchor exceeds head"));
            }
            chain.recheck_block(&finalized).await?;
            chain.recheck_block(&head).await?;
            reports.push((finalized, head));
        }
        let final_number = reports[0].0.number.min(reports[1].0.number).to();
        let head_number = reports[0].1.number.min(reports[1].1.number).to();
        let finalized = self.canonical_block(final_number).await?;
        let head = self.canonical_block(head_number).await?;
        if peer.canonical_block(final_number).await? != finalized
            || peer.canonical_block(head_number).await? != head
            || finalized.timestamp > head.timestamp
        {
            return Err(inconsistent(
                "RPC providers disagree on shared canonical anchors",
            ));
        }
        for chain in [self, peer] {
            chain.check_chain().await?;
        }
        Ok((finalized, head))
    }

    async fn runtime_at(&self, hash: B256) -> Result<Bytes, EvmError> {
        self.observation_rpc(
            "eth_getCode",
            (
                Address::from(self.deployment.settlement_contract),
                json!({"blockHash": hash, "requireCanonical": true}),
            ),
        )
        .await
    }

    /// Reads a nonce using EIP-1898 at an explicit RPC finalized block, with no fallback.
    pub async fn verified_finalized_nonce(
        &self,
        sender: [u8; 20],
    ) -> Result<FinalizedNonce, EvmError> {
        self.check_chain().await?;
        let block = self
            .observation_block("finalized")
            .await?
            .ok_or(EvmError::FinalityUnavailable)?;
        self.verified_nonce_at(sender, &block).await
    }

    async fn verified_nonce_at(
        &self,
        sender: [u8; 20],
        block: &Block,
    ) -> Result<FinalizedNonce, EvmError> {
        self.recheck_block(block).await?;
        let nonce: U64 = self
            .observation_rpc(
                "eth_getTransactionCount",
                (
                    Address::from(sender),
                    json!({"blockHash": block.hash, "requireCanonical": true}),
                ),
            )
            .await?;
        self.recheck_block(block).await?;
        self.check_chain().await?;
        Ok(FinalizedNonce {
            sender,
            nonce: nonce.to(),
            chain_id: self.deployment.chain_id,
            settlement_contract: self.deployment.settlement_contract,
            anchor: block.anchor(),
        })
    }

    /// Reads owned evidence for one x402 exact settlement from finalized chain state.
    ///
    /// The transaction hash is a lookup key, not evidence: the target, calldata, token
    /// `Transfer`, and Permit2 nonce bit are all read at a pinned finalized anchor and the
    /// anchors are rechecked afterwards. `Ok(None)` means the transaction is unknown, reverted,
    /// or not yet finalized: retain state and retry without another payment.
    pub async fn finalized_x402_evidence(
        &self,
        transaction_hash: [u8; 32],
        owner: [u8; 20],
        deal: &DealNullifier,
    ) -> Result<Option<X402ExactEvidence>, EvmError> {
        self.check_chain().await?;
        let hash = B256::from(transaction_hash);
        let first: Option<Receipt> = self
            .observation_rpc("eth_getTransactionReceipt", (hash,))
            .await?;
        let Some(first) = first else {
            return Ok(None);
        };
        let block = self.canonical_block(first.block_number.to()).await?;
        let final_block = self
            .observation_block("finalized")
            .await?
            .ok_or(EvmError::FinalityUnavailable)?;
        self.recheck_block(&final_block).await?;
        if block.number.to::<u64>() > final_block.number.to::<u64>() {
            return Ok(None);
        }
        let (receipt, tx) = self.receipt_pair(hash, &block).await?;
        if receipt.status != U64::from(1) {
            return Ok(None);
        }
        let transfer = receipt.logs.iter().find(|log| {
            log.topics().first().map(|topic| topic.0) == Some(transfer_topic())
                && log.topics().len() == 3
                && log.topics()[1].0[..12] == [0; 12]
                && log.topics()[2].0[..12] == [0; 12]
                && log.topics()[1].0[12..] == owner
        });
        let Some(transfer) = transfer else {
            return Ok(None);
        };
        let topics = transfer.topics();
        let mut transfer_to = [0u8; 20];
        transfer_to.copy_from_slice(&topics[2].0[12..]);
        let data = transfer.data().data.clone();
        if data.len() != 32 || data[..16] != [0; 16] {
            return Err(inconsistent("Transfer data is not one word"));
        }
        let transfer_amount = u128::from_be_bytes(data[16..32].try_into().expect("32-byte word"));
        let bitmap: Bytes = self
            .observation_rpc(
                "eth_call",
                (
                    json!({"to": Address::from(PERMIT2),
                        "data": Bytes::from(encode_nonce_bitmap_call(&owner, deal))}),
                    json!({"blockHash": final_block.hash, "requireCanonical": true}),
                ),
            )
            .await?;
        if bitmap.len() != 32 {
            return Err(inconsistent("nonceBitmap is not one word"));
        }
        let mut nonce_bit = [0u8; 32];
        nonce_bit.copy_from_slice(&bitmap);
        self.recheck_block(&final_block).await?;
        self.recheck_block(&block).await?;
        self.check_chain().await?;
        Ok(Some(X402ExactEvidence {
            finalized: true,
            transaction_to: tx
                .to()
                .map(|address| address.0 .0)
                .ok_or_else(|| inconsistent("settlement has no target"))?,
            calldata: tx.input().to_vec(),
            transfer_from: owner,
            transfer_to,
            transfer_token: transfer.address().0 .0,
            transfer_amount,
            nonce_bit,
        }))
    }

    /// Checks independently pinned Permit2 and proxy runtimes at a finalized anchor.
    pub async fn authenticate_x402_runtimes(
        &self,
        permit2_hash: [u8; 32],
        proxy_hash: [u8; 32],
    ) -> Result<(), EvmError> {
        if permit2_hash == [0; 32] || proxy_hash == [0; 32] {
            return Err(inconsistent("missing x402 runtime pins"));
        }
        self.check_chain().await?;
        let block = self
            .observation_block("finalized")
            .await?
            .ok_or(EvmError::FinalityUnavailable)?;
        for (target, expected) in [
            (PERMIT2, permit2_hash),
            (crate::x402::EXACT_PERMIT2_PROXY, proxy_hash),
        ] {
            let code: Bytes = self
                .observation_rpc(
                    "eth_getCode",
                    (
                        Address::from(target),
                        json!({"blockHash":block.hash,"requireCanonical":true}),
                    ),
                )
                .await?;
            if code.is_empty() || keccak256(code).0 != expected {
                return Err(inconsistent("x402 runtime differs from operator pin"));
            }
        }
        self.recheck_block(&block).await?;
        self.check_chain().await
    }

    /// Reads one observation value with bounded retries. Every request here is read-only and
    /// idempotent, so a transient rate limit (public providers answer 429 under a burst) or
    /// transport error can be retried without affecting any payment or reservation. The
    /// caller still validates every returned value; a retry never widens what is accepted.
    async fn observation_rpc<P, R>(&self, method: &'static str, params: P) -> Result<R, EvmError>
    where
        P: RpcSend,
        R: RpcRecv,
    {
        const ATTEMPTS: u32 = 4;
        let mut attempt = 0;
        loop {
            attempt += 1;
            match tokio::time::timeout(
                self.timeout,
                self.provider.client().request(method, params.clone()),
            )
            .await
            {
                Ok(Ok(value)) => return Ok(value),
                Ok(Err(_)) if attempt < ATTEMPTS => {}
                Ok(Err(_)) => return Err(EvmError::Rpc("observation request failed".into())),
                Err(_) if attempt < ATTEMPTS => {}
                Err(_) => return Err(EvmError::Rpc("observation timed out".into())),
            }
            tokio::time::sleep(std::time::Duration::from_millis(100 * (1 << (attempt - 1)))).await;
        }
    }

    async fn observation_block(&self, tag: &str) -> Result<Option<Block>, EvmError> {
        self.observation_rpc("eth_getBlockByNumber", (tag, false))
            .await
    }

    async fn canonical_block(&self, number: u64) -> Result<Block, EvmError> {
        let block = self
            .observation_block(&format!("0x{number:x}"))
            .await?
            .ok_or(EvmError::AnchorNotCanonical)?;
        if block.number.to::<u64>() != number || block.hash == B256::ZERO {
            return Err(EvmError::AnchorNotCanonical);
        }
        Ok(block)
    }

    async fn recheck_block(&self, block: &Block) -> Result<(), EvmError> {
        if self.canonical_block(block.number.to()).await? != *block {
            return Err(EvmError::AnchorNotCanonical);
        }
        Ok(())
    }

    /// Observes one signed transaction. Errors and missing receipts are not no-effect proof.
    pub async fn observe(&self, transaction_hash: [u8; 32]) -> Result<TxStatus, EvmError> {
        self.check_chain().await?;
        let hash = B256::from(transaction_hash);
        let first: Option<Receipt> = self
            .observation_rpc("eth_getTransactionReceipt", (hash,))
            .await?;
        let Some(first) = first else {
            let tx: Option<Transaction> = self
                .observation_rpc("eth_getTransactionByHash", (hash,))
                .await?;
            if let Some(tx) = &tx {
                validate_transaction(tx, hash, self.deployment.chain_id)?;
                if tx.block_hash.is_some()
                    || tx.block_number.is_some()
                    || tx.transaction_index.is_some()
                {
                    return Err(inconsistent("mined transaction has no receipt"));
                }
            }
            self.check_chain().await?;
            return Ok(if tx.is_some() {
                TxStatus::Pending
            } else {
                TxStatus::NotFound
            });
        };
        let block = self.canonical_block(first.block_number.to()).await?;
        // Read the receipt again after pinning the canonical anchor.
        let (receipt, tx) = self.receipt_pair(hash, &block).await?;
        self.recheck_block(&block).await?;
        self.check_chain().await?;
        Ok(TxStatus::Included(Inclusion {
            block: block.anchor(),
            success: receipt.status == U64::from(1),
            from: tx.from().0 .0,
            to: tx.to().map(|a| a.0 .0),
            nonce: tx.nonce(),
            transaction_index: receipt.transaction_index.to(),
        }))
    }

    async fn receipt_pair(
        &self,
        hash: B256,
        block: &Block,
    ) -> Result<(Receipt, Transaction), EvmError> {
        let receipt: Receipt = self
            .observation_rpc::<_, Option<Receipt>>("eth_getTransactionReceipt", (hash,))
            .await?
            .ok_or(EvmError::MissingReceipt)?;
        let tx: Transaction = self
            .observation_rpc::<_, Option<Transaction>>("eth_getTransactionByHash", (hash,))
            .await?
            .ok_or(EvmError::MissingTransaction)?;
        validate_transaction(&tx, hash, self.deployment.chain_id)?;
        validate_receipt(&receipt, &tx, hash, block)?;
        Ok((receipt, tx))
    }

    /// Builds core evidence for any directly submitted revision, including a foreign winner.
    ///
    /// Searches the complete history through a pinned head and reads consumption at head and
    /// explicit RPC `finalized`. Both anchors and the winner are checked again after the reads.
    /// Missing/duplicate logs, indirect calls, unavailable finality, and invalid authorizations
    /// return errors. Callers must map errors to `DealEvidence::Unknown`, retaining claims.
    /// This relies on honest RPC chain/state answers and monotonic chain timestamps.
    pub async fn finalized_deal_evidence(
        &self,
        nullifier: &DealNullifier,
        limits: ObservationLimits,
    ) -> Result<DealEvidence, EvmError> {
        Ok(self
            .anchored_deal_evidence_from(nullifier, 0, limits)
            .await?
            .2)
    }

    /// As [`Self::finalized_deal_evidence`], but begins the log scan at `start_block`.
    ///
    /// Live chains are far taller than a public RPC's `eth_getLogs` range cap, so a genesis
    /// scan is not possible. `start_block` must be at or before the earliest block in which
    /// the deployment could settle the deal (use the deployment block); a larger value would
    /// omit settlement logs.
    pub async fn finalized_deal_evidence_from(
        &self,
        nullifier: &DealNullifier,
        start_block: u64,
        limits: ObservationLimits,
    ) -> Result<DealEvidence, EvmError> {
        Ok(self
            .anchored_deal_evidence_from(nullifier, start_block, limits)
            .await?
            .2)
    }

    /// Requires two separately configured RPC endpoints to agree on both anchors and deal reads.
    /// Errors, differing heads, or differing finalized anchors retain all reservations. Operators
    /// must arrange provider independence; two URLs alone do not prove independent infrastructure.
    /// This opt-in check does not change the single-provider observation method.
    pub async fn finalized_deal_evidence_agreed(
        &self,
        peer: &Self,
        nullifier: &DealNullifier,
        limits: ObservationLimits,
    ) -> Result<DealEvidence, EvmError> {
        self.check_peer(peer)?;
        let (first, second) = tokio::join!(
            self.anchored_deal_evidence_from(nullifier, 0, limits),
            peer.anchored_deal_evidence_from(nullifier, 0, limits)
        );
        let first = first?;
        let second = second?;
        if first != second {
            return Err(inconsistent(
                "RPC providers disagree on anchored deal evidence",
            ));
        }
        for chain in [self, peer] {
            if chain.canonical_block(first.0.number).await?.anchor() != first.0
                || chain.canonical_block(first.1.number).await?.anchor() != first.1
            {
                return Err(inconsistent("agreed anchors changed during comparison"));
            }
        }
        Ok(first.2)
    }

    /// Releases nonce claims only from matching finalized nonce evidence at both RPCs.
    pub async fn verified_finalized_nonce_agreed(
        &self,
        peer: &Self,
        sender: [u8; 20],
    ) -> Result<FinalizedNonce, EvmError> {
        let (block, _) = self.common_observation_blocks(peer).await?;
        let (first, second) = tokio::join!(
            self.verified_nonce_at(sender, &block),
            peer.verified_nonce_at(sender, &block)
        );
        let first = first?;
        if first != second? {
            return Err(inconsistent("RPC providers disagree on finalized nonce"));
        }
        for chain in [self, peer] {
            if chain.canonical_block(first.anchor.number).await?.anchor() != first.anchor {
                return Err(inconsistent("agreed nonce anchor changed"));
            }
        }
        Ok(first)
    }

    /// Requires distinct configured RPC endpoints for the same settlement deployment.
    pub fn check_peer(&self, peer: &Self) -> Result<(), EvmError> {
        if crate::deployment::normalized_rpc_url(&self.deployment.rpc_url)?
            == crate::deployment::normalized_rpc_url(&peer.deployment.rpc_url)?
            || self.deployment.namespace != peer.deployment.namespace
            || self.deployment.chain_id != peer.deployment.chain_id
            || self.deployment.settlement_contract != peer.deployment.settlement_contract
            || self.deployment.verifier_version != peer.deployment.verifier_version
        {
            return Err(inconsistent(
                "RPC peer must use a distinct endpoint for the same deployment",
            ));
        }
        Ok(())
    }

    async fn anchored_deal_evidence_from(
        &self,
        nullifier: &DealNullifier,
        start_block: u64,
        limits: ObservationLimits,
    ) -> Result<(BlockRef, BlockRef, DealEvidence), EvmError> {
        if limits.log_block_range == 0 || limits.max_log_queries == 0 {
            return Err(EvmError::ObservationLimit);
        }
        self.check_chain().await?;
        let final_block = self
            .observation_block("finalized")
            .await?
            .ok_or(EvmError::FinalityUnavailable)?;
        let head = self
            .observation_block("latest")
            .await?
            .ok_or_else(|| inconsistent("head missing"))?;
        if final_block.number > head.number || final_block.timestamp > head.timestamp {
            return Err(inconsistent("finalized anchor is ahead of head"));
        }
        self.recheck_block(&final_block).await?;
        self.recheck_block(&head).await?;
        let consumed_at_final = self.consumed(nullifier, &final_block).await?;
        let consumed_at_head = self.consumed(nullifier, &head).await?;
        let logs = self
            .deal_logs(nullifier, head.number.to(), start_block, limits)
            .await?;
        if (consumed_at_final && !consumed_at_head)
            || logs.len() > 1
            || consumed_at_head == logs.is_empty()
        {
            return Err(inconsistent("consumption and settlement logs disagree"));
        }
        let (winner, winner_block) = self
            .log_winner(nullifier, logs.first(), &head, &final_block)
            .await?;
        if winner
            .as_ref()
            .is_some_and(|winner| winner.is_final != consumed_at_final)
        {
            return Err(inconsistent("winner finality disagrees with consumption"));
        }
        self.check_ancestry(
            &head,
            &final_block,
            winner_block.as_ref(),
            limits.max_ancestry,
        )
        .await?;
        if let Some(block) = &winner_block {
            self.recheck_block(block).await?;
        }
        self.recheck_block(&head).await?;
        self.recheck_block(&final_block).await?;
        self.check_chain().await?;
        Ok((
            final_block.anchor(),
            head.anchor(),
            DealEvidence::Observed(DealReads {
                deal_nullifier: *nullifier,
                final_anchor_timestamp: final_block.timestamp.to(),
                consumed_at_final,
                consumed_at_head,
                winner,
            }),
        ))
    }

    async fn log_winner(
        &self,
        nullifier: &DealNullifier,
        log: Option<&Log>,
        head: &Block,
        final_block: &Block,
    ) -> Result<(Option<WinningSettlement>, Option<Block>), EvmError> {
        if let Some(log) = log {
            let number = log
                .block_number
                .ok_or_else(|| inconsistent("log has no block number"))?;
            if number > head.number.to::<u64>() {
                return Err(inconsistent("log is beyond head"));
            }
            let block = self.canonical_block(number).await?;
            let hash = log
                .transaction_hash
                .ok_or_else(|| inconsistent("log has no transaction hash"))?;
            let (receipt, tx) = self.receipt_pair(hash, &block).await?;
            if receipt.status != U64::from(1) {
                return Err(EvmError::TransactionReverted);
            }
            let fields = self.verify_winner(log, &receipt, &tx, &block, nullifier)?;
            let is_final = number <= final_block.number.to::<u64>();
            Ok((
                Some(WinningSettlement {
                    commitment: fields.0,
                    amount: BaseUnits::new(fields.1),
                    fee: BaseUnits::new(fields.2),
                    is_final,
                }),
                Some(block),
            ))
        } else {
            Ok((None, None))
        }
    }

    async fn consumed(&self, nullifier: &DealNullifier, block: &Block) -> Result<bool, EvmError> {
        let data: Bytes = self
            .observation_rpc(
                "eth_call",
                (
                    json!({"to": Address::from(self.deployment.settlement_contract),
                "data": Bytes::from(abi::encode_consumed_deals_call(nullifier.as_bytes()))}),
                    json!({"blockHash": block.hash, "requireCanonical": true}),
                ),
            )
            .await?;
        abi::decode_bool_word(&data).ok_or_else(|| inconsistent("consumedDeals is not an ABI bool"))
    }

    /// One bounded log range. Returns its start so concurrent results can be reordered.
    /// The range bounds are rechecked here, before any result can enter evidence.
    async fn fetch_log_range(
        &self,
        from: u64,
        to: u64,
        nullifier: &DealNullifier,
    ) -> Result<(u64, Vec<Log>), EvmError> {
        let batch: Vec<Log> = self
            .observation_rpc(
                "eth_getLogs",
                (json!({
                    "address": Address::from(self.deployment.settlement_contract),
                    "topics": [Some(B256::from(abi::deal_settled_topic())), Option::<B256>::None,
                        Some(B256::from(*nullifier.as_bytes()))],
                    "fromBlock": U64::from(from), "toBlock": U64::from(to),
                }),),
            )
            .await?;
        for log in &batch {
            if !log.block_number.is_some_and(|n| n >= from && n <= to) {
                return Err(inconsistent("log outside requested range"));
            }
        }
        Ok((from, batch))
    }

    /// Runs one window of contiguous, non-overlapping log ranges concurrently.
    /// Returns the ordered logs, the next un-scanned block (`None` at the head), and the
    /// number of ranges issued. An error or interruption leaves no partial result: the
    /// caller's checkpoint only advances after this returns.
    async fn scan_log_window(
        &self,
        nullifier: &DealNullifier,
        from: u64,
        head: u64,
        limits: ObservationLimits,
    ) -> Result<(Vec<Log>, Option<u64>, u64), EvmError> {
        if limits.log_block_range == 0
            || limits.max_log_queries == 0
            || limits.max_concurrent_queries == 0
            || limits.max_concurrent_queries > MAX_CONCURRENT_LOG_QUERIES
        {
            return Err(EvmError::ObservationLimit);
        }
        if from > head {
            return Ok((Vec::new(), None, 0));
        }
        let mut ranges = Vec::new();
        let mut next = from;
        for _ in 0..limits.max_concurrent_queries.min(limits.max_log_queries) {
            let to = next.saturating_add(limits.log_block_range - 1).min(head);
            ranges.push((next, to));
            if to >= head {
                break;
            }
            next = to + 1;
        }
        let mut tasks = tokio::task::JoinSet::new();
        for (range_from, range_to) in &ranges {
            let chain = self.clone();
            let nullifier = *nullifier;
            let (range_from, range_to) = (*range_from, *range_to);
            tasks.spawn(async move {
                chain
                    .fetch_log_range(range_from, range_to, &nullifier)
                    .await
            });
        }
        let mut batches = Vec::with_capacity(ranges.len());
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok(Ok(batch)) => batches.push(batch),
                Ok(Err(error)) => {
                    tasks.abort_all();
                    return Err(error);
                }
                Err(_) => {
                    tasks.abort_all();
                    return Err(inconsistent("log query task failed"));
                }
            }
        }
        // Results arrive out of order; restore block order before recording logs.
        batches.sort_by_key(|(range_from, _)| *range_from);
        let mut logs = Vec::new();
        for (_, batch) in batches {
            logs.extend(batch);
            if logs.len() > 1 {
                return Err(inconsistent("multiple settlement logs"));
            }
        }
        let last = ranges.last().expect("window is non-empty").1;
        Ok((logs, (last < head).then(|| last + 1), ranges.len() as u64))
    }

    async fn deal_logs(
        &self,
        nullifier: &DealNullifier,
        head: u64,
        start_block: u64,
        limits: ObservationLimits,
    ) -> Result<Vec<Log>, EvmError> {
        let first = start_block.min(head);
        if (head - first) / limits.log_block_range >= limits.max_log_queries {
            return Err(EvmError::ObservationLimit);
        }
        let mut next = Some(first);
        let mut queries = 0;
        let mut logs = Vec::new();
        while let Some(from) = next {
            let remaining = limits.max_log_queries - queries;
            if remaining == 0 {
                return Err(EvmError::ObservationLimit);
            }
            let window = ObservationLimits {
                max_log_queries: remaining,
                ..limits
            };
            let (batch, resumed, used) =
                self.scan_log_window(nullifier, from, head, window).await?;
            logs.extend(batch);
            if logs.len() > 1 {
                return Err(inconsistent("multiple settlement logs"));
            }
            queries += used;
            next = resumed;
        }
        Ok(logs)
    }

    async fn check_ancestry(
        &self,
        head: &Block,
        finalized: &Block,
        winner: Option<&Block>,
        limit: u64,
    ) -> Result<(), EvmError> {
        let oldest = winner.map_or(finalized.number, |b| b.number.min(finalized.number));
        if head.number.to::<u64>() - oldest.to::<u64>() > limit {
            return Err(EvmError::ObservationLimit);
        }
        let mut current = head.clone();
        loop {
            for expected in [Some(finalized), winner].into_iter().flatten() {
                if current.number == expected.number && current != *expected {
                    return Err(EvmError::AnchorNotCanonical);
                }
            }
            if current.number == oldest {
                break;
            }
            let parent = self.canonical_block(current.number.to::<u64>() - 1).await?;
            if current.parent_hash != parent.hash || parent.timestamp > current.timestamp {
                return Err(EvmError::AnchorNotCanonical);
            }
            current = parent;
        }
        Ok(())
    }

    fn verify_winner(
        &self,
        log: &Log,
        receipt: &Receipt,
        tx: &Transaction,
        block: &Block,
        nullifier: &DealNullifier,
    ) -> Result<(erebus_core::commitment::DealCommitment, u128, u128), EvmError> {
        if tx.to() != Some(Address::from(self.deployment.settlement_contract))
            || !tx.value().is_zero()
        {
            return Err(inconsistent("winner is not a zero-value direct settlement"));
        }
        let candidates: Vec<_> = receipt
            .logs
            .iter()
            .filter(|l| {
                l.address() == Address::from(self.deployment.settlement_contract)
                    && l.topics().first() == Some(&B256::from(abi::deal_settled_topic()))
                    && l.topics().get(2) == Some(&B256::from(*nullifier.as_bytes()))
            })
            .collect();
        if candidates.len() != 1 || candidates[0] != log {
            return Err(inconsistent(
                "queried log is not the exact unique receipt event",
            ));
        }
        let topics: Vec<_> = log.topics().iter().map(|t| t.0).collect();
        let event = abi::decode_deal_settled(&topics, &log.data().data)?;
        let call = abi::decode_settle_call(tx.input())?;
        let terms = AgreementTerms::decode(&call.terms)?;
        validate_terms(&self.deployment, &terms)?;
        let blinding = CommitmentBlinding::from_bytes(call.blinding);
        let commitment = commit_agreement(&terms, &blinding)?;
        if deal_nullifier(&terms)? != *nullifier
            || event.deal_nullifier != *nullifier.as_bytes()
            || event.commitment != *commitment.as_bytes()
            || event.buyer.as_slice() != terms.buyer_authorization_key.as_bytes()
            || event.payment_recipient.as_slice() != terms.payment_recipient.as_bytes()
            || event.token != self.deployment.token_address(&terms.asset)?
            || event.token != call.token
            || event.amount != terms.amount.get()
            || event.fee != terms.fee_policy.fee.get()
            || block.timestamp.to::<u64>() >= terms.expiry
            || event.amount.checked_add(event.fee).is_none()
        {
            return Err(inconsistent("winner disagrees with accepted agreement"));
        }
        for (role, signature) in [
            (Role::Buyer, call.buyer_signature),
            (Role::Seller, call.seller_signature),
        ] {
            if signature.len() != 65 {
                return Err(EvmError::SignatureLength(signature.len()));
            }
            let authorization = Authorization {
                role,
                suite_id: terms.suite_id,
                commitment,
                signature: SignatureBytes::new(signature)?,
            };
            verify_authorization_signature(&terms, &commitment, &blinding, &authorization)?;
        }
        Ok((commitment, event.amount, event.fee))
    }
}

fn validate_transaction(tx: &Transaction, hash: B256, chain_id: u64) -> Result<(), EvmError> {
    // RPC deserialization wraps the supplied `from` with `Recovered::new_unchecked`.
    // Recover from the signature rather than comparing that metadata to itself.
    let envelope = tx.inner.inner();
    if envelope.signature().normalize_s().is_some() {
        return Err(inconsistent("transaction signature has high s"));
    }
    let signer = envelope
        .signature()
        .recover_address_from_prehash(&envelope.signature_hash())
        .map_err(|_| inconsistent("transaction signature recovery failed"))?;
    if tx.tx_hash() != hash
        || keccak256(tx.inner.encoded_2718()) != hash
        || tx.chain_id() != Some(chain_id)
        || signer != tx.from()
    {
        return Err(inconsistent(
            "transaction hash, signature, or chain id mismatch",
        ));
    }
    Ok(())
}

fn validate_receipt(
    receipt: &Receipt,
    tx: &Transaction,
    hash: B256,
    block: &Block,
) -> Result<(), EvmError> {
    let index = receipt.transaction_index.to::<u64>();
    if receipt.transaction_hash != hash
        || receipt.block_hash != block.hash
        || receipt.block_number != block.number
        || tx.block_hash != Some(block.hash)
        || tx.block_number != Some(block.number.to())
        || tx.transaction_index != Some(index)
        || receipt.from != tx.from()
        || receipt.to != tx.to()
        || receipt.status.to::<u64>() > 1
        || usize::try_from(index)
            .ok()
            .and_then(|i| block.transactions.get(i))
            != Some(&hash)
    {
        return Err(inconsistent(
            "receipt, transaction, and canonical block disagree",
        ));
    }
    let mut last_index = None;
    for log in &receipt.logs {
        let log_index = log
            .log_index
            .ok_or_else(|| inconsistent("receipt log index missing"))?;
        if log.removed
            || log.block_hash != Some(block.hash)
            || log.block_number != Some(block.number.to())
            || log.transaction_hash != Some(hash)
            || log.transaction_index != Some(index)
            || last_index.is_some_and(|previous| log_index <= previous)
        {
            return Err(inconsistent("receipt log location mismatch"));
        }
        last_index = Some(log_index);
    }
    if receipt.status == U64::ZERO && !receipt.logs.is_empty() {
        return Err(inconsistent("reverted receipt contains logs"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::consensus::{SignableTransaction, TxEip1559, TxEnvelope};
    use alloy::signers::{local::PrivateKeySigner, SignerSync};

    #[test]
    fn rpc_sender_must_match_the_signed_envelope() {
        let key = PrivateKeySigner::from_bytes(&B256::from([1; 32])).unwrap();
        let unsigned = TxEip1559 {
            chain_id: 31337,
            gas_limit: 21_000,
            ..Default::default()
        };
        let signature = key.sign_hash_sync(&unsigned.signature_hash()).unwrap();
        let envelope = TxEnvelope::from(unsigned.into_signed(signature));
        let hash = keccak256(envelope.encoded_2718());
        let mut json = serde_json::to_value(&envelope).unwrap();
        json["from"] = serde_json::to_value(key.address()).unwrap();
        let honest: Transaction = serde_json::from_value(json.clone()).unwrap();
        validate_transaction(&honest, hash, 31337).unwrap();

        // RPC metadata is outside the signed envelope, so its hash stays valid.
        json["from"] = serde_json::to_value(Address::from([2; 20])).unwrap();
        let forged: Transaction = serde_json::from_value(json).unwrap();
        assert_eq!(keccak256(forged.inner.encoded_2718()), hash);
        assert!(validate_transaction(&forged, hash, 31337).is_err());
    }
}
