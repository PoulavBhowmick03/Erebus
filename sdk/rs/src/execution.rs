//! Action execution pipeline.
//!
//! ```text
//! ActionSet
//!   -> compile_actions calldata
//!   -> same-block view preflight
//!   -> signed pool __execute__ proof invocation
//!   -> starknet_proveTransaction
//!   -> compare proved server actions with the preflight
//!   -> signed account call to apply_actions
//!   -> receipt
//! ```
//!
//! The proof invocation exposes the pool viewing key to the prover. The executor does not
//! trust the prover to choose the state transition. It compares the proof's L2→L1 payload
//! byte-for-byte with an independent `compile_actions` result before submission.
//!
//! `compile_actions` calldata exposes the same key to the preflight RPC. Both the prover and
//! this RPC must be operator-controlled. Public RPC endpoints are only suitable for reads.

use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use starknet_crypto::Signature;
use starknet_types_core::felt::Felt;

use crate::action_set::ActionSet;
use crate::calldata;
use crate::journal::{JournalError, OperationLease, OperationStage};
use crate::prover::{BlockId, ProveTransactionResult, ProverError, ProvingService};
use crate::rpc::{Receipt, RpcError, StarknetRpc};
use crate::signer::AccountSigner;
use crate::signing::{self, SigningError};
use crate::tx::{
    DataAvailabilityMode, InvokeV3, PoolInvocation, PoolInvocationError, ResourceBounds,
    SignedInvokeV3,
};

/// How far behind the chain head a proof is anchored.
pub const DEFAULT_PROVING_BLOCK_LAG: u64 = 10;
/// How long submission waits for an accepted receipt.
pub const DEFAULT_RECEIPT_TIMEOUT: Duration = Duration::from_secs(180);
/// Receipt polling interval.
pub const DEFAULT_RECEIPT_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// What the Sepolia pool returned for `get_proof_validity_blocks` on 2026-08-22.
///
/// Reference only. It is not authoritative and nothing on the write path reads it: the pool
/// owns this number, it has no reason to stay fixed, and a recovered proof that is judged
/// against a stale window is either resubmitted when it cannot land or re-proven when it did
/// not need to be. [`Executor::execute`] takes the live value as an argument.
pub const OBSERVED_SEPOLIA_PROOF_VALIDITY_BLOCKS: u64 = 450;

/// Network addresses and timing for action execution.
#[derive(Debug, Clone)]
pub struct ExecutionConfig {
    /// Privacy-pool contract.
    pub pool_address: Felt,
    /// Starknet chain id, e.g. the short-string felt `SN_SEPOLIA`.
    pub chain_id: Felt,
    /// Account whose `is_valid_signature` validates the proof invocation and which submits
    /// the final `apply_actions` call.
    pub account_address: Felt,
    /// Proving block offset from the accepted head.
    pub proving_block_lag: u64,
    /// Maximum receipt wait.
    pub receipt_timeout: Duration,
    /// Delay between receipt reads.
    pub receipt_poll_interval: Duration,
}

impl ExecutionConfig {
    /// Configuration with conservative MVP timing defaults.
    pub fn new(pool_address: Felt, chain_id: Felt, account_address: Felt) -> Self {
        Self {
            pool_address,
            chain_id,
            account_address,
            proving_block_lag: DEFAULT_PROVING_BLOCK_LAG,
            receipt_timeout: DEFAULT_RECEIPT_TIMEOUT,
            receipt_poll_interval: DEFAULT_RECEIPT_POLL_INTERVAL,
        }
    }
}

/// Executes validated action sets.
#[derive(Debug, Clone)]
pub struct Executor {
    rpc: StarknetRpc,
    prover: ProvingService,
    config: ExecutionConfig,
}

impl Executor {
    /// Creates an executor.
    pub fn new(rpc: StarknetRpc, prover: ProvingService, config: ExecutionConfig) -> Self {
        Self {
            rpc,
            prover,
            config,
        }
    }

    /// The underlying RPC client, used by discovery and read operations.
    pub fn rpc(&self) -> &StarknetRpc {
        &self.rpc
    }

    /// The proving service, used by pre-flight reachability checks.
    pub fn prover(&self) -> &ProvingService {
        &self.prover
    }

    /// Waits until `state_block` can be observed by the configured historical proof anchor.
    ///
    /// A proof at `head - lag` cannot see a write from block `N` until the head reaches
    /// `N + lag`. The returned anchor is safe for note discovery and the following proof.
    pub async fn wait_until_provable(&self, state_block: u64) -> Result<BlockId, ExecutionError> {
        let target_head = state_block.saturating_add(self.config.proving_block_lag);
        let started = Instant::now();
        loop {
            let head = self.rpc.block_number().await?;
            if head >= target_head {
                return Ok(BlockId::Number(
                    head.saturating_sub(self.config.proving_block_lag).max(1),
                ));
            }
            if started.elapsed() >= self.config.receipt_timeout {
                return Err(ExecutionError::MaturityTimeout {
                    state_block,
                    target_head,
                    current_head: head,
                    waited: self.config.receipt_timeout,
                });
            }
            tokio::time::sleep(self.config.receipt_poll_interval).await;
        }
    }

    /// Preflights, proves, submits, and waits for one action set.
    ///
    /// Read both private keys from operator-owned files immediately before this call. The
    /// executor does not retain them.
    ///
    /// Stage names go to stderr as they start. A write is minutes of silence otherwise,
    /// and a caller that cannot tell "proving" from "hung" aborts and retries, which is
    /// how duplicate submissions happen. stderr is outside the CLI's one-envelope stdout
    /// contract, so consumers of the envelope are unaffected; the lines carry no key
    /// material, no addresses, and no amounts.
    pub async fn execute(
        &self,
        operation: &mut OperationLease,
        proof_validity_blocks: u64,
        user_address: Felt,
        pool_private_key: Felt,
        signer: &dyn AccountSigner,
        actions: &ActionSet,
    ) -> Result<ExecutionReceipt, ExecutionError> {
        let head = self.rpc.block_number().await?;
        let proving_number = head.saturating_sub(self.config.proving_block_lag).max(1);
        let proving_block = BlockId::Number(proving_number);

        stage("simulating the action set against the proving block");
        let compile_calldata = calldata::compile_actions(user_address, pool_private_key, actions);
        let simulated = self
            .rpc
            .call_contract(
                self.config.pool_address,
                "compile_actions",
                &compile_calldata,
                &proving_block,
            )
            .await?;

        // Preflight is done and agreed with. Nothing is proven and nothing can have
        // reached the chain, so this stage records only where the attempt is anchored.
        operation.amend(now(), |attempt| {
            attempt.proving_block = Some(proving_number);
            attempt.valid_until_block = Some(proving_number.saturating_add(proof_validity_blocks));
            attempt.simulation_hash = Some(server_actions_hash(&simulated));
        })?;
        operation.advance(OperationStage::Prepared, now())?;

        let proof_nonce = self
            .rpc
            .nonce(self.config.pool_address, &proving_block)
            .await?;
        let proof_invoke = proof_invoke(
            self.config.pool_address,
            self.config.chain_id,
            user_address,
            pool_private_key,
            proof_nonce,
            actions,
        )?;
        let proof_signature = signer.sign(&proof_invoke.transaction_hash()).await?;
        let proof_invocation = proof_invoke.with_signature(proof_signature);
        let proof_idempotency_seed = operation.record().operation_id.as_str();
        stage("proving, the long stage: tens of seconds to minutes");
        let proof = self
            .prover
            .prove_transaction_idempotent(&proving_block, &proof_invocation, proof_idempotency_seed)
            .await?;
        let server_actions = server_actions(&proof, self.config.pool_address)?;

        if simulated != server_actions {
            return Err(ExecutionError::SimulationMismatch {
                simulated_len: simulated.len(),
                proved_len: server_actions.len(),
            });
        }

        operation.advance(OperationStage::Proven, now())?;
        self.finish_proven(
            operation,
            proving_number,
            proof_validity_blocks,
            signer,
            proof,
            server_actions,
        )
        .await
    }

    /// Continues a hosted proof job recorded before a process restart.
    ///
    /// `None` means no recoverable hosted job exists, so the caller may use the ordinary
    /// reconciliation and rebuild policy. A proof is accepted only when its server actions
    /// match the preflight commitment written before proving began.
    pub(crate) async fn resume_hosted_proof(
        &self,
        operation: &mut OperationLease,
        signer: &dyn AccountSigner,
    ) -> Result<Option<ExecutionReceipt>, ExecutionError> {
        let recorded_stage = operation.record().stage();
        if !matches!(
            recorded_stage,
            OperationStage::Prepared | OperationStage::Proven
        ) {
            return Ok(None);
        }
        let attempt = operation.record().attempt();
        let (Some(proving_number), Some(valid_until_block), Some(expected_hash)) = (
            attempt.proving_block,
            attempt.valid_until_block,
            attempt.simulation_hash.clone(),
        ) else {
            return Ok(None);
        };
        let proof_validity_blocks = valid_until_block.saturating_sub(proving_number);
        if self.rpc.block_number().await? > valid_until_block {
            return Ok(None);
        }
        let Some(proof) = self
            .prover
            .resume_transaction_idempotent(operation.record().operation_id.as_str())
            .await?
        else {
            return Ok(None);
        };
        let server_actions = server_actions(&proof, self.config.pool_address)?;
        let received_hash = server_actions_hash(&server_actions);
        if received_hash != expected_hash {
            return Err(ExecutionError::SimulationCommitmentMismatch);
        }
        if recorded_stage == OperationStage::Prepared {
            operation.advance(OperationStage::Proven, now())?;
        }
        self.finish_proven(
            operation,
            proving_number,
            proof_validity_blocks,
            signer,
            proof,
            server_actions,
        )
        .await
        .map(Some)
    }

    async fn finish_proven(
        &self,
        operation: &mut OperationLease,
        proving_number: u64,
        proof_validity_blocks: u64,
        signer: &dyn AccountSigner,
        proof: ProveTransactionResult,
        server_actions: Vec<Felt>,
    ) -> Result<ExecutionReceipt, ExecutionError> {
        let submission_head = self.rpc.block_number().await?;
        if submission_head > proving_number.saturating_add(proof_validity_blocks) {
            return Err(ExecutionError::ProofExpired {
                proving_block: proving_number,
                current_block: submission_head,
            });
        }

        let apply_calldata =
            calldata::apply_actions(&server_actions, proof.additional_data.as_ref())?;
        let account_calldata =
            calldata::single_call(self.config.pool_address, "apply_actions", &apply_calldata);
        let proof_facts = parse_felts("proof_facts", &proof.proof_facts)?;
        let account_nonce = self
            .rpc
            .nonce(self.config.account_address, &BlockId::Latest)
            .await?;

        let estimate_invoke = submission_invoke(
            &self.config,
            account_calldata.clone(),
            account_nonce,
            ResourceBounds::default(),
            proof_facts.clone(),
        );
        // estimateFee skips validation but executes the transaction and verifies the proof.
        // A zero signature is sufficient.
        let estimate_transaction = estimate_invoke
            .with_signature(Signature {
                r: Felt::ZERO,
                s: Felt::ZERO,
            })
            .with_proof(proof.proof.clone());
        stage("estimating fee, which also verifies the proof");
        let bounds = self
            .rpc
            .estimate_bounds(&estimate_transaction, &BlockId::Latest)
            .await?;

        let invoke = submission_invoke(
            &self.config,
            account_calldata,
            account_nonce,
            bounds,
            proof_facts,
        );
        let signed_hash = invoke.transaction_hash();
        // The account key is read inside the signer and dropped when this returns, so it
        // never exists in this frame. A hardware or wallet signer never produces one at all.
        let signature = signer.sign(&signed_hash).await?;
        let transaction = invoke.with_signature(signature).with_proof(proof.proof);

        // The crash window this closes: the hash is computable before submission, so it is
        // written down before the chain can ever have seen it. Everything after this point
        // may have produced an effect, and the journal can name it.
        operation.amend(now(), |attempt| {
            attempt.account_nonce = Some(account_nonce);
        })?;
        persist_transaction(operation, signed_hash, &transaction)?;

        stage("submitting apply_actions and waiting for the receipt");
        let transaction_hash = self.rpc.add_invoke_transaction(&transaction).await?;
        operation.advance(OperationStage::Submitted, now())?;
        confirm_hash(operation, signed_hash, transaction_hash)?;
        let receipt = self.wait_for_receipt(operation, transaction_hash).await?;
        operation.record_receipt(receipt.clone(), now())?;
        let accepted_at = self.accepted_block_timestamp(&receipt).await?;
        operation.amend(now(), |attempt| attempt.accepted_at = Some(accepted_at))?;
        operation.advance(OperationStage::Accepted, now())?;

        Ok(ExecutionReceipt {
            transaction_hash,
            proving_block: proving_number,
            receipt,
        })
    }

    /// Submits one signed account call that carries no proof.
    ///
    /// This is not a shortcut around [`Self::execute`] and must never be used for pool state
    /// transitions. The pool's only state-changing entrypoint is `apply_actions`, which
    /// verifies a proof; a call arriving here with pool calldata would simply revert. What it
    /// exists for is the ERC-20 `approve` that has to land *before* a charged `apply_actions`,
    /// which is an ordinary token call with no actions, no simulation, and no prover.
    ///
    /// Signs through `signer`, so the account key is never held by this frame.
    pub async fn submit_call(
        &self,
        operation: &mut OperationLease,
        signer: &dyn AccountSigner,
        target: Felt,
        entrypoint: &str,
        call_calldata: &[Felt],
    ) -> Result<CallReceipt, ExecutionError> {
        // No proof, so this attempt goes straight from prepared to signed.
        operation.advance(OperationStage::Prepared, now())?;
        let account_calldata = calldata::single_call(target, entrypoint, call_calldata);
        let nonce = self
            .rpc
            .nonce(self.config.account_address, &BlockId::Latest)
            .await?;

        // Same two-pass shape as `execute`: estimate with a zero signature under
        // SKIP_VALIDATE, then sign the invoke that carries the resulting bounds.
        let estimate = submission_invoke(
            &self.config,
            account_calldata.clone(),
            nonce,
            ResourceBounds::default(),
            Vec::new(),
        )
        .with_signature(Signature {
            r: Felt::ZERO,
            s: Felt::ZERO,
        });
        let bounds = self
            .rpc
            .estimate_bounds(&estimate, &BlockId::Latest)
            .await?;

        let invoke = submission_invoke(&self.config, account_calldata, nonce, bounds, Vec::new());
        let signed_hash = invoke.transaction_hash();
        let signature = signer.sign(&signed_hash).await?;
        let transaction = invoke.with_signature(signature);

        operation.amend(now(), |attempt| {
            attempt.account_nonce = Some(nonce);
        })?;
        persist_transaction(operation, signed_hash, &transaction)?;

        let transaction_hash = self.rpc.add_invoke_transaction(&transaction).await?;
        operation.advance(OperationStage::Submitted, now())?;
        confirm_hash(operation, signed_hash, transaction_hash)?;
        let receipt = self.wait_for_receipt(operation, transaction_hash).await?;
        operation.record_receipt(receipt.clone(), now())?;
        let accepted_at = self.accepted_block_timestamp(&receipt).await?;
        operation.amend(now(), |attempt| attempt.accepted_at = Some(accepted_at))?;
        operation.advance(OperationStage::Accepted, now())?;

        Ok(CallReceipt {
            transaction_hash,
            receipt,
        })
    }

    async fn wait_for_receipt(
        &self,
        operation: &mut OperationLease,
        transaction_hash: Felt,
    ) -> Result<Receipt, ExecutionError> {
        let started = Instant::now();
        loop {
            match self.rpc.transaction_receipt(transaction_hash).await {
                Ok(receipt) if receipt.is_accepted() => return Ok(receipt),
                Ok(receipt) if receipt.is_reverted() => {
                    // Recorded before returning: a revert is the one outcome that proves no
                    // effect exists, and losing it would leave the attempt looking pending.
                    let reason = receipt
                        .revert_reason
                        .clone()
                        .unwrap_or_else(|| "<no revert reason>".to_owned());
                    operation.record_receipt(receipt, now())?;
                    operation.advance(OperationStage::Reverted, now())?;
                    return Err(ExecutionError::Reverted(reason));
                }
                Ok(_) | Err(RpcError::Rpc { code: 29, .. }) => {}
                Err(error) => return Err(error.into()),
            }

            if started.elapsed() >= self.config.receipt_timeout {
                return Err(ExecutionError::ReceiptTimeout {
                    transaction_hash,
                    waited: self.config.receipt_timeout,
                });
            }
            tokio::time::sleep(self.config.receipt_poll_interval).await;
        }
    }

    /// Waits for a transaction submitted by recovery and durably records its outcome.
    pub(crate) async fn finish_resubmission(
        &self,
        operation: &mut OperationLease,
        transaction_hash: Felt,
    ) -> Result<Receipt, ExecutionError> {
        if operation.record().stage() == OperationStage::Signed {
            operation.advance(OperationStage::Submitted, now())?;
        }
        let receipt = self.wait_for_receipt(operation, transaction_hash).await?;
        operation.record_receipt(receipt.clone(), now())?;
        let accepted_at = self.accepted_block_timestamp(&receipt).await?;
        operation.amend(now(), |attempt| attempt.accepted_at = Some(accepted_at))?;
        operation.advance(OperationStage::Accepted, now())?;
        Ok(receipt)
    }

    async fn accepted_block_timestamp(&self, receipt: &Receipt) -> Result<u64, ExecutionError> {
        let block_number = receipt.block_number.ok_or_else(|| {
            ExecutionError::Rpc(RpcError::Malformed(
                "accepted receipt omitted its block number".to_owned(),
            ))
        })?;
        Ok(self.rpc.block_timestamp(block_number).await?)
    }
}

/// Builds the exact proof invocation from an action set.
///
/// Public so conformance tests can cover the composition from [`ActionSet`] to calldata.
#[allow(clippy::too_many_arguments)]
pub fn build_proof_invocation(
    pool_address: Felt,
    chain_id: Felt,
    user_address: Felt,
    pool_private_key: Felt,
    account_private_key: Felt,
    nonce: Felt,
    actions: &ActionSet,
) -> Result<SignedInvokeV3, ExecutionError> {
    let invoke = proof_invoke(
        pool_address,
        chain_id,
        user_address,
        pool_private_key,
        nonce,
        actions,
    )?;
    let signature = signing::sign(&account_private_key, &invoke.transaction_hash())?;
    Ok(invoke.with_signature(signature))
}

fn proof_invoke(
    pool_address: Felt,
    chain_id: Felt,
    user_address: Felt,
    pool_private_key: Felt,
    nonce: Felt,
    actions: &ActionSet,
) -> Result<InvokeV3, ExecutionError> {
    let inner = calldata::compile_actions(user_address, pool_private_key, actions);
    let invoke = InvokeV3 {
        sender_address: pool_address,
        calldata: calldata::proof_execute(pool_address, &inner),
        chain_id,
        nonce,
        account_deployment_data: Vec::new(),
        nonce_da_mode: DataAvailabilityMode::L1,
        fee_da_mode: DataAvailabilityMode::L1,
        resource_bounds: ResourceBounds::for_proof_invocation(),
        tip: 0,
        paymaster_data: Vec::new(),
        proof_facts: Vec::new(),
    };
    let invoke = PoolInvocation::new(invoke)?.into_inner();
    Ok(invoke)
}

fn submission_invoke(
    config: &ExecutionConfig,
    calldata: Vec<Felt>,
    nonce: Felt,
    resource_bounds: ResourceBounds,
    proof_facts: Vec<Felt>,
) -> InvokeV3 {
    InvokeV3 {
        sender_address: config.account_address,
        calldata,
        chain_id: config.chain_id,
        nonce,
        account_deployment_data: Vec::new(),
        nonce_da_mode: DataAvailabilityMode::L1,
        fee_da_mode: DataAvailabilityMode::L1,
        resource_bounds,
        tip: 0,
        paymaster_data: Vec::new(),
        proof_facts,
    }
}

fn server_actions(
    proof: &ProveTransactionResult,
    pool_address: Felt,
) -> Result<Vec<Felt>, ExecutionError> {
    let mut matching = proof
        .l2_to_l1_messages
        .iter()
        .filter(|message| Felt::from_hex(&message.from_address).ok() == Some(pool_address));
    let message = matching.next().ok_or(ExecutionError::MissingPoolMessage)?;
    if matching.next().is_some() {
        return Err(ExecutionError::AmbiguousPoolMessage);
    }

    let payload = parse_felts("L2 to L1 payload", &message.payload)?;
    if payload.is_empty() {
        return Err(ExecutionError::EmptyPoolMessage);
    }
    // The first felt is the pool class hash. `apply_actions` receives the following
    // `Span<ServerAction>`.
    Ok(payload[1..].to_vec())
}

fn parse_felts(field: &'static str, values: &[String]) -> Result<Vec<Felt>, ExecutionError> {
    values
        .iter()
        .map(|value| {
            Felt::from_hex(value).map_err(|_| ExecutionError::InvalidProverFelt {
                field,
                value: value.clone(),
            })
        })
        .collect()
}

/// A successful proof-carrying submission.
#[derive(Debug, Clone)]
pub struct ExecutionReceipt {
    /// Submitted transaction.
    pub transaction_hash: Felt,
    /// Historical block used by both preflight and proof.
    pub proving_block: u64,
    /// Accepted Starknet receipt.
    pub receipt: Receipt,
}

/// Result of a proofless account call.
///
/// Carries no proving block because [`Executor::submit_call`] anchors nothing: there is no
/// simulation and no historical read, so there is no block for a caller to wait behind.
#[derive(Debug)]
pub struct CallReceipt {
    /// Submitted transaction.
    pub transaction_hash: Felt,
    /// Accepted Starknet receipt.
    pub receipt: Receipt,
}

/// Failure at a specific execution stage.
#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    /// Pool invocation violated its zero-fee validation rules.
    #[error(transparent)]
    PoolInvocation(#[from] PoolInvocationError),
    /// Signing failed.
    #[error(transparent)]
    Signing(#[from] SigningError),
    /// The account signer could not, or would not, produce a signature.
    #[error(transparent)]
    Signer(#[from] crate::signer::SignerError),
    /// Starknet node failure.
    #[error(transparent)]
    Rpc(#[from] RpcError),
    /// Proving-service failure.
    #[error(transparent)]
    Prover(#[from] ProverError),
    /// Malformed apply-actions calldata.
    #[error(transparent)]
    Calldata(#[from] calldata::CalldataError),
    /// Prover response did not contain the pool's L2→L1 message.
    #[error("proof response omitted the privacy pool's L2 to L1 message")]
    MissingPoolMessage,
    /// More than one pool message made the server-action payload ambiguous.
    #[error("proof response contained multiple messages from the privacy pool")]
    AmbiguousPoolMessage,
    /// The pool message omitted its class-hash prefix.
    #[error("proof response contained an empty privacy-pool message")]
    EmptyPoolMessage,
    /// A prover-returned field was not a canonical felt.
    #[error("prover {field} contained a non-felt value: {value}")]
    InvalidProverFelt {
        /// Which response field.
        field: &'static str,
        /// Received text.
        value: String,
    },
    /// The independently simulated transition differed from the proof.
    #[error(
        "proved server actions differ from same-block simulation \
         (simulation {simulated_len} felts, proof {proved_len} felts)"
    )]
    SimulationMismatch {
        /// Preflight result length.
        simulated_len: usize,
        /// Proof result length.
        proved_len: usize,
    },
    /// A recovered proof did not match the preflight output committed before restart.
    #[error("recovered proof does not match the recorded same-block simulation")]
    SimulationCommitmentMismatch,
    /// Proof aged past the pool's accepted block window before submission.
    #[error(
        "proof anchored at block {proving_block} expired before current block {current_block}"
    )]
    ProofExpired {
        /// Proof anchor.
        proving_block: u64,
        /// Head observed after proving.
        current_block: u64,
    },
    /// A prior accepted write never became visible at the historical proof anchor.
    #[error(
        "timed out after {waited:?} waiting for state block {state_block} to become provable \
         (target head {target_head}, current head {current_head})"
    )]
    MaturityTimeout {
        /// Block containing the dependency.
        state_block: u64,
        /// Minimum head required by the configured lag.
        target_head: u64,
        /// Last observed head.
        current_head: u64,
        /// Configured wait.
        waited: Duration,
    },
    /// The operation journal could not be advanced.
    #[error(transparent)]
    Journal(#[from] JournalError),
    /// The signed transaction could not be encoded for persistence.
    #[error("signed transaction could not be recorded: {0}")]
    TransactionNotSerializable(serde_json::Error),
    /// The node answered a submission with a hash other than the one that was signed.
    #[error("submitted transaction {signed:#x} but the node returned {returned:#x}")]
    TransactionHashMismatch {
        /// Hash computed locally and written to the journal before submission.
        signed: Felt,
        /// Hash the node answered with.
        returned: Felt,
    },
    /// Final transaction reverted.
    #[error("apply_actions transaction reverted: {0}")]
    Reverted(String),
    /// The node never reported an accepted receipt.
    #[error("timed out after {waited:?} waiting for transaction {transaction_hash:#x}")]
    ReceiptTimeout {
        /// Submitted hash.
        transaction_hash: Felt,
        /// Configured wait.
        waited: Duration,
    },
}

/// One stage line on stderr. Freestanding so the write path reads as its stages.
fn stage(name: &str) {
    eprintln!("stage: {name}");
}

fn server_actions_hash(actions: &[Felt]) -> String {
    let mut hasher = Sha256::new();
    hasher.update((actions.len() as u64).to_be_bytes());
    for action in actions {
        hasher.update(action.to_bytes_be());
    }
    format!("{:x}", hasher.finalize())
}

/// Unix seconds, saturating at the epoch.
///
/// The journal timestamps are for operator reporting, not for any protocol decision, so a
/// clock behind the epoch degrades to zero rather than failing a write that is already in
/// flight.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// Writes the exact transaction the RPC is about to receive, then records its hash.
fn persist_transaction(
    operation: &mut OperationLease,
    transaction_hash: Felt,
    transaction: &SignedInvokeV3,
) -> Result<(), ExecutionError> {
    // Stored in RPC wire form rather than as our own struct: resubmission has to reproduce
    // the request byte for byte, and this is the shape that goes on the wire.
    let encoded = serde_json::to_string(&transaction.to_wire())
        .map_err(ExecutionError::TransactionNotSerializable)?;
    operation.persist_signed(transaction_hash, &encoded, now())?;
    Ok(())
}

/// Checks that the node accepted the transaction we actually signed.
///
/// The hash is computed locally before submission and written to the journal, so a node that
/// answers with a different one has left the journal naming a transaction nobody is
/// watching, while the one being watched is not the one that was recorded. Polling the
/// returned hash and reporting success would hide a signed transaction still in flight, so
/// this escalates instead.
fn confirm_hash(
    operation: &mut OperationLease,
    signed: Felt,
    returned: Felt,
) -> Result<(), ExecutionError> {
    if signed == returned {
        return Ok(());
    }
    operation.advance(OperationStage::NeedsAttention, now())?;
    Err(ExecutionError::TransactionHashMismatch { signed, returned })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action_set::ActionSetBuilder;
    use crate::actions::{ClientAction, OpenChannelInput};

    #[test]
    fn proof_constructor_starts_from_the_action_set() {
        let mut builder = ActionSetBuilder::new();
        builder
            .push(ClientAction::OpenChannel(OpenChannelInput {
                recipient_addr: Felt::from(7u8),
                index: 0,
                random: Felt::from(11u8),
                salt: Felt::from(13u8),
            }))
            .expect("action");
        let actions = builder.build().expect("set");

        let pool = Felt::from(17u8);
        let signed = build_proof_invocation(
            pool,
            Felt::from(19u8),
            Felt::from(23u8),
            Felt::from(29u8),
            Felt::from(31u8),
            Felt::ZERO,
            &actions,
        )
        .expect("invocation");

        let expected_inner =
            calldata::compile_actions(Felt::from(23u8), Felt::from(29u8), &actions);
        assert_eq!(
            signed.invoke.calldata,
            calldata::proof_execute(pool, &expected_inner)
        );
        assert_eq!(
            signed.invoke.resource_bounds,
            ResourceBounds::for_proof_invocation()
        );
        assert!(signed.invoke.proof_facts.is_empty());
        assert!(signed.proof.is_none());
    }
}
