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

use starknet_crypto::Signature;
use starknet_types_core::felt::Felt;

use crate::action_set::ActionSet;
use crate::calldata;
use crate::prover::{BlockId, ProveTransactionResult, ProverError, ProvingService};
use crate::rpc::{Receipt, RpcError, StarknetRpc};
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
/// Deployed Sepolia pool proof-validity window.
pub const DEFAULT_PROOF_VALIDITY_BLOCKS: u64 = 450;

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
    /// Maximum blocks between proof anchor and submission.
    pub proof_validity_blocks: u64,
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
            proof_validity_blocks: DEFAULT_PROOF_VALIDITY_BLOCKS,
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
    pub async fn execute(
        &self,
        user_address: Felt,
        pool_private_key: Felt,
        account_private_key: Felt,
        actions: &ActionSet,
    ) -> Result<ExecutionReceipt, ExecutionError> {
        let head = self.rpc.block_number().await?;
        let proving_number = head.saturating_sub(self.config.proving_block_lag).max(1);
        let proving_block = BlockId::Number(proving_number);

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

        let proof_nonce = self
            .rpc
            .nonce(self.config.pool_address, &proving_block)
            .await?;
        let proof_invocation = build_proof_invocation(
            self.config.pool_address,
            self.config.chain_id,
            user_address,
            pool_private_key,
            account_private_key,
            proof_nonce,
            actions,
        )?;
        let proof = self
            .prover
            .prove_transaction(&proving_block, &proof_invocation)
            .await?;
        let server_actions = server_actions(&proof, self.config.pool_address)?;

        if simulated != server_actions {
            return Err(ExecutionError::SimulationMismatch {
                simulated_len: simulated.len(),
                proved_len: server_actions.len(),
            });
        }

        let submission_head = self.rpc.block_number().await?;
        if submission_head > proving_number.saturating_add(self.config.proof_validity_blocks) {
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
        let signature = signing::sign(&account_private_key, &invoke.transaction_hash())?;
        let transaction = invoke.with_signature(signature).with_proof(proof.proof);
        let transaction_hash = self.rpc.add_invoke_transaction(&transaction).await?;
        let receipt = self.wait_for_receipt(transaction_hash).await?;

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
    /// Read `account_private_key` from its file immediately before calling. Not retained.
    pub async fn submit_call(
        &self,
        account_private_key: Felt,
        target: Felt,
        entrypoint: &str,
        call_calldata: &[Felt],
    ) -> Result<CallReceipt, ExecutionError> {
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
        let bounds = self.rpc.estimate_bounds(&estimate, &BlockId::Latest).await?;

        let invoke = submission_invoke(
            &self.config,
            account_calldata,
            nonce,
            bounds,
            Vec::new(),
        );
        let signature = signing::sign(&account_private_key, &invoke.transaction_hash())?;
        let transaction = invoke.with_signature(signature);
        let transaction_hash = self.rpc.add_invoke_transaction(&transaction).await?;
        let receipt = self.wait_for_receipt(transaction_hash).await?;

        Ok(CallReceipt {
            transaction_hash,
            receipt,
        })
    }

    async fn wait_for_receipt(&self, transaction_hash: Felt) -> Result<Receipt, ExecutionError> {
        let started = Instant::now();
        loop {
            match self.rpc.transaction_receipt(transaction_hash).await {
                Ok(receipt) if receipt.is_accepted() => return Ok(receipt),
                Ok(receipt) if receipt.is_reverted() => {
                    return Err(ExecutionError::Reverted(
                        receipt
                            .revert_reason
                            .unwrap_or_else(|| "<no revert reason>".to_owned()),
                    ));
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
    let signature = signing::sign(&account_private_key, &invoke.transaction_hash())?;
    Ok(invoke.with_signature(signature))
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
