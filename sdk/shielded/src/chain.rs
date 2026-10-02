//! Journaled EVM submission of a locally prepared shielded pool transfer.

use std::time::Duration;

use erebus_coordinator::Coordinator;
use erebus_core::settlement::{check_capabilities, PreparedSettlement, SettlementContext};
use erebus_evm::{
    chain::{
        Broadcast, Eip1559Fees, EvmChain, JournaledBroadcastError, NonceError, SignedTransaction,
        SignerJournal, SigningPlan, TransactionKey, TransactionParams,
    },
    deployment::EvmDeployment,
    error::EvmError,
};

use crate::{
    index_store::IndexStore,
    observation::{observe_shielded_deal, observe_shielded_deal_agreed, ObservationError},
    preparation::{capabilities, validate_prepared, PreparationError},
    recovery::{recover_finalized_wallet, recover_finalized_wallet_agreed, RecoveryError},
    rpc::PoolRpc,
    wallet::WalletStore,
};

/// A failure in local signing, durable submission, or verified reconciliation.
#[derive(Debug, thiserror::Error)]
pub enum ShieldedChainError {
    /// The accepted context or proof calldata does not match the pool.
    #[error(transparent)]
    Preparation(#[from] PreparationError),
    /// The EVM chain or transaction is invalid.
    #[error(transparent)]
    Evm(#[from] EvmError),
    /// The gas-payer nonce claim could not be allocated or released.
    #[error(transparent)]
    Nonce(#[from] NonceError),
    /// The coordinator's durable operation is unavailable.
    #[error(transparent)]
    Coordinator(#[from] erebus_coordinator::Error),
    /// A journaled send failed before or after network I/O; retain uncertainty.
    #[error(transparent)]
    Broadcast(#[from] JournaledBroadcastError),
    /// Pool observations are missing or inconsistent; retain reservations.
    #[error(transparent)]
    Observation(#[from] ObservationError),
    /// Finalized note-wallet recovery failed; retain the payment reservation.
    #[error(transparent)]
    Recovery(#[from] RecoveryError),
    /// The configured context cannot be served by this backend.
    #[error("shielded chain context is unsupported")]
    Context,
}

/// One shielded pool deployment using the shared EVM signer lane and durable coordinator.
pub struct ShieldedChain {
    context: SettlementContext,
    evm: EvmChain,
}

impl ShieldedChain {
    /// Default two-provider reconciliation. Neither wallet/accounting nor nonce claims change
    /// until both providers agree on pinned deal evidence and the finalized signer nonce.
    /// Operators must choose independent RPC infrastructure and separate index caches.
    #[allow(clippy::too_many_arguments)]
    pub async fn reconcile(
        &self,
        peer: &Self,
        coordinator: &Coordinator,
        journal: &SignerJournal,
        pool_rpc: &PoolRpc,
        index_store: &IndexStore,
        peer_rpc: &PoolRpc,
        peer_index: &IndexStore,
        wallet_store: &WalletStore,
        operation_ref: [u8; 32],
        now: u64,
    ) -> Result<erebus_core::deal_state::DealAssessment, ShieldedChainError> {
        let (context, revisions) = coordinator.recorded_deal(operation_ref)?;
        if context != self.context
            || context != peer.context
            || !pool_rpc.matches_endpoint(&self.evm.deployment().rpc_url)
            || !peer_rpc.matches_endpoint(&peer.evm.deployment().rpc_url)
        {
            return Err(ShieldedChainError::Context);
        }
        let nullifier = revisions
            .first()
            .ok_or(ShieldedChainError::Context)?
            .deal_nullifier();
        let evidence = observe_shielded_deal_agreed(
            pool_rpc,
            index_store,
            peer_rpc,
            peer_index,
            &context,
            &nullifier,
            &revisions,
        )
        .await?;
        let finalized_nonce = self
            .evm
            .verified_finalized_nonce_agreed(&peer.evm, journal.account_address())
            .await?;
        recover_finalized_wallet_agreed(pool_rpc, index_store, peer_rpc, peer_index, wallet_store)
            .await?;
        let assessment = coordinator.reconcile(operation_ref, &evidence, now)?;
        match journal.release(&finalized_nonce) {
            Ok(()) | Err(NonceError::NotConsumed) => {}
            Err(error) => return Err(error.into()),
        }
        Ok(assessment)
    }

    /// Compatibility name for the default mandatory paired-provider reconciliation.
    #[allow(clippy::too_many_arguments)]
    pub async fn reconcile_agreed(
        &self,
        peer: &Self,
        coordinator: &Coordinator,
        journal: &SignerJournal,
        pool_rpc: &PoolRpc,
        index_store: &IndexStore,
        peer_rpc: &PoolRpc,
        peer_index: &IndexStore,
        wallet_store: &WalletStore,
        operation_ref: [u8; 32],
        now: u64,
    ) -> Result<erebus_core::deal_state::DealAssessment, ShieldedChainError> {
        self.reconcile(
            peer,
            coordinator,
            journal,
            pool_rpc,
            index_store,
            peer_rpc,
            peer_index,
            wallet_store,
            operation_ref,
            now,
        )
        .await
    }
    /// Connects to an EVM pool fixed by the accepted agreement context.
    pub async fn connect(
        context: SettlementContext,
        rpc_url: &str,
        timeout: Duration,
    ) -> Result<Self, ShieldedChainError> {
        check_capabilities(&context, &capabilities()).map_err(|_| ShieldedChainError::Context)?;
        let pool: [u8; 20] = context
            .domain
            .pool
            .as_ref()
            .ok_or(ShieldedChainError::Context)?
            .as_bytes()
            .try_into()
            .map_err(|_| ShieldedChainError::Context)?;
        let deployment = EvmDeployment::new(
            context.domain.namespace.clone(),
            pool,
            context.domain.verifier_version,
            rpc_url,
        )?;
        deployment.matches_domain(&context.domain)?;
        deployment.token_address(&context.asset)?;
        let evm = EvmChain::connect(deployment, timeout).await?;
        Ok(Self { context, evm })
    }

    fn checked_call<'a>(
        &self,
        prepared: &'a PreparedSettlement,
    ) -> Result<([u8; 20], &'a [u8]), ShieldedChainError> {
        validate_prepared(&self.context, prepared)?;
        Ok((
            self.evm.deployment().settlement_contract,
            &prepared.backend_evidence,
        ))
    }

    /// Reserves the shared gas-payer nonce, then persists exact signed bytes locally.
    /// A failed signer leaves both the nonce claim and spending reservation held.
    #[allow(clippy::too_many_arguments)]
    pub async fn sign(
        &self,
        coordinator: &Coordinator,
        journal: &SignerJournal,
        operation_ref: [u8; 32],
        key: &TransactionKey,
        now: u64,
        fees: Eip1559Fees,
        gas_limit: u64,
    ) -> Result<Vec<u8>, ShieldedChainError> {
        let prepared = coordinator.prepared_settlement(operation_ref)?;
        let (target, calldata) = self.checked_call(&prepared)?;
        let plan = self
            .evm
            .reserve_nonce_for_call(journal, &prepared, target, calldata, fees, gas_limit)
            .await?;
        let chain_id = self.evm.deployment().chain_id;
        let raw = coordinator.sign_transaction(
            operation_ref,
            now,
            &plan.encode(),
            |saved, bytes| {
                validate_prepared(&self.context, saved)
                    .map_err(|_| EvmError::SignedIntentMismatch)?;
                let plan = SigningPlan::decode(bytes)?;
                Ok::<_, EvmError>(
                    plan.sign_call(chain_id, target, saved.backend_evidence.clone(), key)?
                        .raw()
                        .to_vec(),
                )
            },
            |saved, bytes, raw| {
                validate_prepared(&self.context, saved)
                    .map_err(|_| EvmError::SignedIntentMismatch)?;
                SigningPlan::decode(bytes)?.validate_call(
                    chain_id,
                    target,
                    &saved.backend_evidence,
                    raw,
                )
            },
        )?;
        Ok(raw)
    }

    /// Sends the coordinator's persisted bytes after rechecking their pool call.
    /// An acknowledgement is not payment evidence.
    pub async fn broadcast(
        &self,
        coordinator: &Coordinator,
        operation_ref: [u8; 32],
        now: u64,
    ) -> Result<Broadcast, ShieldedChainError> {
        let prepared = coordinator.prepared_settlement(operation_ref)?;
        let (target, calldata) = self.checked_call(&prepared)?;
        Ok(self
            .evm
            .broadcast_journaled_call(coordinator, operation_ref, now, &prepared, target, calldata)
            .await?)
    }

    /// Appends a same-nonce fee replacement without changing the pool call.
    /// The coordinator retains every prior signed transaction for recovery.
    pub fn replace(
        &self,
        coordinator: &Coordinator,
        operation_ref: [u8; 32],
        key: &TransactionKey,
        now: u64,
        fees: Eip1559Fees,
    ) -> Result<usize, ShieldedChainError> {
        let prepared = coordinator.prepared_settlement(operation_ref)?;
        let (target, calldata) = self.checked_call(&prepared)?;
        let original = coordinator
            .transaction_plan(operation_ref)?
            .ok_or(erebus_coordinator::Error::Stage)?;
        let original = SigningPlan::decode(&original)?;
        let plan = SigningPlan::new(
            original.sender(),
            TransactionParams {
                nonce: original.params().nonce,
                gas_limit: original.params().gas_limit,
                fees,
            },
        )?;
        let chain_id = self.evm.deployment().chain_id;
        let index = coordinator.sign_replacement(
            operation_ref,
            now,
            &plan.encode(),
            |previous, bytes| {
                if previous.prepared() != &prepared {
                    return Err(EvmError::SignedIntentMismatch);
                }
                let plan = SigningPlan::decode(bytes)?;
                Ok::<_, EvmError>(
                    plan.sign_call(chain_id, target, calldata.to_vec(), key)?
                        .raw()
                        .to_vec(),
                )
            },
            |previous, bytes, raw| {
                if previous.prepared() != &prepared {
                    return Err(EvmError::SignedIntentMismatch);
                }
                let prior_plan = SigningPlan::decode(previous.plan())?;
                prior_plan.validate_call(chain_id, target, calldata, previous.raw())?;
                let plan = SigningPlan::decode(bytes)?;
                plan.validate_call(chain_id, target, calldata, raw)?;
                SignedTransaction::from_raw(raw)?
                    .validate_replacement(&SignedTransaction::from_raw(previous.raw())?)
            },
        )?;
        Ok(index)
    }

    /// Explicit low-level single-provider recovery for trusted-RPC experiments only.
    /// This is not the supported operator default. Use `reconcile` for paired verification.
    #[allow(clippy::too_many_arguments)]
    pub async fn reconcile_single_provider(
        &self,
        coordinator: &Coordinator,
        journal: &SignerJournal,
        pool_rpc: &PoolRpc,
        index_store: &IndexStore,
        wallet_store: &WalletStore,
        operation_ref: [u8; 32],
        now: u64,
    ) -> Result<erebus_core::deal_state::DealAssessment, ShieldedChainError> {
        let (context, revisions) = coordinator.recorded_deal(operation_ref)?;
        if context != self.context {
            return Err(ShieldedChainError::Context);
        }
        let nullifier = revisions
            .first()
            .ok_or(ShieldedChainError::Context)?
            .deal_nullifier();
        let evidence =
            observe_shielded_deal(pool_rpc, index_store, &context, &nullifier, &revisions).await?;
        recover_finalized_wallet(pool_rpc, index_store, wallet_store).await?;
        let assessment = coordinator.reconcile(operation_ref, &evidence, now)?;
        let finalized_nonce = self
            .evm
            .verified_finalized_nonce(journal.account_address())
            .await?;
        match journal.release(&finalized_nonce) {
            Ok(()) | Err(NonceError::NotConsumed) => {}
            Err(error) => return Err(error.into()),
        }
        Ok(assessment)
    }
}
