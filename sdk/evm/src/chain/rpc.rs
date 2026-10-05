//! Chain-checked RPC writes. A broadcast response is never payment or non-payment evidence.

use super::{Eip1559Fees, NonceError, SignedTransaction, SignerJournal, SigningPlan};
use crate::{deployment::EvmDeployment, error::EvmError};
use alloy::{
    primitives::{Address, Bytes, B256, U64},
    providers::{DynProvider, Provider, ProviderBuilder},
    transports::RpcError,
};
use erebus_core::settlement::PreparedSettlement;
use std::time::Duration;

/// What the node's broadcast response establishes. Never grounds for releasing a claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BroadcastOutcome {
    /// The node acknowledged the locally computed hash. Not inclusion or finality.
    Acknowledged,
    /// The node returned a JSON-RPC error. Earlier sends may still succeed.
    Rejected {
        /// Provider error code. Untrusted messages are omitted to avoid leaking calldata.
        code: i64,
    },
    /// Timeout, transport failure, malformed response, or a different returned hash.
    Unknown,
}

/// Broadcast diagnostics with a locally computed transaction identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Broadcast {
    /// Hash computed from the exact bytes sent.
    pub hash: [u8; 32],
    /// What this attempt's response established.
    pub outcome: BroadcastOutcome,
}

/// Unsigned RPC connection. It never fills transaction fields or holds a wallet key.
/// Cloning shares the same provider handle and deployment pins; it creates no state.
#[derive(Clone)]
pub struct EvmChain {
    pub(super) deployment: EvmDeployment,
    pub(super) provider: DynProvider,
    pub(super) timeout: Duration,
}
impl std::fmt::Debug for EvmChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvmChain")
            .field("chain_id", &self.deployment.chain_id)
            .finish_non_exhaustive()
    }
}

impl EvmChain {
    /// The deployment this chain observer is bound to.
    #[must_use]
    pub const fn deployment(&self) -> &EvmDeployment {
        &self.deployment
    }

    /// Records an uncertain attempt before network I/O and saves the response afterward.
    /// Dropping this future after begin leaves a durable Unknown attempt for recovery.
    /// No journal lock crosses the await. A failed response write retains uncertainty.
    pub async fn broadcast_journaled(
        &self,
        coordinator: &erebus_coordinator::Coordinator,
        operation_ref: [u8; 32],
        now: u64,
    ) -> Result<Broadcast, JournaledBroadcastError> {
        let attempt = coordinator.begin_broadcast_attempt(operation_ref, now)?;
        let transaction = &attempt.transaction;
        let plan = SigningPlan::decode(transaction.plan())?;
        let result = self
            .broadcast(transaction.prepared(), &plan, transaction.raw())
            .await?;
        self.finish_journaled_broadcast(coordinator, operation_ref, &attempt.token, result)?;
        Ok(result)
    }

    /// Submits the initial public-bound transaction at most once through this automatic
    /// path. The coordinator atomically rejects any prior attempt or replacement before
    /// network I/O, including a concurrent caller or an interrupted Unknown fence.
    /// A returned error does not establish nonpayment. Explicit retries use separate APIs.
    pub async fn broadcast_initial_journaled(
        &self,
        coordinator: &erebus_coordinator::Coordinator,
        operation_ref: [u8; 32],
        now: u64,
    ) -> Result<Broadcast, JournaledBroadcastError> {
        let attempt = coordinator.begin_initial_broadcast_attempt(operation_ref, now)?;
        let transaction = &attempt.transaction;
        let plan = SigningPlan::decode(transaction.plan())?;
        let result = self
            .broadcast(transaction.prepared(), &plan, transaction.raw())
            .await?;
        self.finish_journaled_broadcast(coordinator, operation_ref, &attempt.token, result)?;
        Ok(result)
    }

    /// Broadcasts only an already journaled backend-validated call.
    /// `expected` must be derived from the accepted agreement, not caller-supplied RPC data.
    pub async fn broadcast_journaled_call(
        &self,
        coordinator: &erebus_coordinator::Coordinator,
        operation_ref: [u8; 32],
        now: u64,
        expected: &PreparedSettlement,
        target: [u8; 20],
        calldata: &[u8],
    ) -> Result<Broadcast, JournaledBroadcastError> {
        let attempt = coordinator.begin_broadcast_attempt(operation_ref, now)?;
        let transaction = &attempt.transaction;
        if transaction.prepared() != expected {
            return Err(EvmError::SignedIntentMismatch.into());
        }
        let plan = SigningPlan::decode(transaction.plan())?;
        let result = self
            .broadcast_call(&plan, transaction.raw(), target, calldata)
            .await?;
        self.finish_journaled_broadcast(coordinator, operation_ref, &attempt.token, result)?;
        Ok(result)
    }

    /// Atomically fences the first backend-validated call before network I/O.
    /// Any retained attempt or replacement prevents automatic resubmission.
    #[allow(clippy::too_many_arguments)]
    pub async fn broadcast_initial_journaled_call(
        &self,
        coordinator: &erebus_coordinator::Coordinator,
        operation_ref: [u8; 32],
        now: u64,
        expected: &PreparedSettlement,
        target: [u8; 20],
        calldata: &[u8],
    ) -> Result<Broadcast, JournaledBroadcastError> {
        let attempt = coordinator.begin_initial_broadcast_attempt(operation_ref, now)?;
        let transaction = &attempt.transaction;
        if transaction.prepared() != expected {
            return Err(EvmError::SignedIntentMismatch.into());
        }
        let plan = SigningPlan::decode(transaction.plan())?;
        let result = self
            .broadcast_call(&plan, transaction.raw(), target, calldata)
            .await?;
        self.finish_journaled_broadcast(coordinator, operation_ref, &attempt.token, result)?;
        Ok(result)
    }

    /// Reads the gas payer's balance after checking the configured chain.
    /// No allowance, payment authorization, or transaction is created.
    pub async fn gas_payer_balance(&self, sender: [u8; 20]) -> Result<u128, EvmError> {
        self.check_chain().await?;
        let balance = tokio::time::timeout(
            self.timeout,
            self.provider.get_balance(Address::from(sender)),
        )
        .await
        .map_err(|_| EvmError::Rpc("gas funding check timed out".into()))?
        .map_err(|_| EvmError::Rpc("gas funding check failed".into()))?;
        balance
            .try_into()
            .map_err(|_| EvmError::SignedIntentMismatch)
    }

    /// Estimates a backend-derived call without filling or signing transaction fields.
    pub async fn estimate_call_gas(
        &self,
        sender: [u8; 20],
        target: [u8; 20],
        calldata: &[u8],
    ) -> Result<u64, EvmError> {
        use alloy::rpc::types::TransactionRequest;
        self.check_chain().await?;
        let request = TransactionRequest::default()
            .from(Address::from(sender))
            .to(Address::from(target))
            .input(Bytes::copy_from_slice(calldata).into());
        tokio::time::timeout(self.timeout, self.provider.estimate_gas(request))
            .await
            .map_err(|_| EvmError::Rpc("gas estimate timed out".into()))?
            .map_err(|_| EvmError::Rpc("gas estimate failed".into()))
    }

    fn finish_journaled_broadcast(
        &self,
        coordinator: &erebus_coordinator::Coordinator,
        operation_ref: [u8; 32],
        token: &erebus_coordinator::AttemptToken,
        result: Broadcast,
    ) -> Result<(), JournaledBroadcastError> {
        let outcome = match result.outcome {
            BroadcastOutcome::Acknowledged => erebus_coordinator::BroadcastOutcome::Submitted,
            BroadcastOutcome::Rejected { .. } => erebus_coordinator::BroadcastOutcome::Rejected,
            BroadcastOutcome::Unknown => erebus_coordinator::BroadcastOutcome::Unknown,
        };
        coordinator.finish_broadcast_attempt(operation_ref, token, outcome)?;
        Ok(())
    }

    /// Connects with a bounded request timeout and checks `eth_chainId`.
    pub async fn connect(deployment: EvmDeployment, timeout: Duration) -> Result<Self, EvmError> {
        let url = deployment
            .rpc_url
            .parse()
            .map_err(|_| EvmError::Rpc("invalid RPC URL".into()))?;
        let provider = ProviderBuilder::default().connect_http(url).erased();
        Self::from_provider(deployment, provider, timeout).await
    }

    /// Uses a caller-selected provider, including an in-memory test transport.
    /// RPC consistency checks do not authenticate a malicious provider.
    pub async fn from_provider(
        deployment: EvmDeployment,
        provider: DynProvider,
        timeout: Duration,
    ) -> Result<Self, EvmError> {
        if timeout.is_zero()
            || deployment.namespace.to_string() != format!("eip155:{}", deployment.chain_id)
        {
            return Err(EvmError::DeploymentMismatch);
        }
        let this = Self {
            deployment,
            provider,
            timeout,
        };
        this.check_chain().await?;
        Ok(this)
    }

    /// Checks the current RPC chain. Errors contain no provider URL or response text.
    pub async fn check_chain(&self) -> Result<(), EvmError> {
        let found = tokio::time::timeout(self.timeout, self.provider.get_chain_id())
            .await
            .map_err(|_| EvmError::Rpc("chain check timed out".into()))?
            .map_err(|_| EvmError::Rpc("chain check failed".into()))?;
        if found != self.deployment.chain_id {
            return Err(EvmError::ChainIdMismatch {
                expected: self.deployment.chain_id,
                found,
            });
        }
        Ok(())
    }

    /// Recovers an existing claim locally, or reads account nonces and reserves a new claim.
    /// A new claim requires matching latest and pending counts on this RPC. A difference
    /// is an unresolved account transaction, not permission to skip ahead.
    /// All callers must share the journal and use a dedicated signer.
    pub async fn reserve_nonce(
        &self,
        journal: &SignerJournal,
        prepared: &PreparedSettlement,
        fees: Eip1559Fees,
        gas_limit: u64,
    ) -> Result<SigningPlan, NonceError> {
        let calldata = super::settlement_calldata(&self.deployment, prepared)?;
        self.reserve_nonce_for_call(
            journal,
            prepared,
            self.deployment.settlement_contract,
            &calldata,
            fees,
            gas_limit,
        )
        .await
    }

    /// Reserves the same durable nonce lane for a backend-validated contract call.
    pub async fn reserve_nonce_for_call(
        &self,
        journal: &SignerJournal,
        prepared: &PreparedSettlement,
        target: [u8; 20],
        calldata: &[u8],
        fees: Eip1559Fees,
        gas_limit: u64,
    ) -> Result<SigningPlan, NonceError> {
        let (chain_id, sender) = journal.account();
        if chain_id != self.deployment.chain_id {
            return Err(NonceError::Conflict);
        }
        if let Some(plan) = journal.resume_call(&self.deployment, prepared, target, calldata)? {
            if plan.params().fees != fees || plan.params().gas_limit != gas_limit {
                return Err(NonceError::Conflict);
            }
            return Ok(plan);
        }
        self.check_chain().await?;
        let latest = self.account_nonce(sender, "latest").await?;
        let pending = self.account_nonce(sender, "pending").await?;
        self.check_chain().await?;
        if latest != pending {
            return Err(NonceError::Busy);
        }
        journal.reserve_call(
            &self.deployment,
            prepared,
            target,
            calldata,
            latest,
            fees,
            gas_limit,
        )
    }

    async fn account_nonce(&self, sender: [u8; 20], tag: &'static str) -> Result<u64, EvmError> {
        let request = self
            .provider
            .client()
            .request::<_, U64>("eth_getTransactionCount", (Address::from(sender), tag));
        tokio::time::timeout(self.timeout, request)
            .await
            .map_err(|_| EvmError::Rpc("nonce read timed out".into()))?
            .map(|n| n.to::<u64>())
            .map_err(|_| EvmError::Rpc("nonce read failed".into()))
    }

    /// Validates and broadcasts the exact bytes already persisted by the coordinator.
    /// The caller must persist before invoking this method; this adapter cannot prove storage.
    /// An `Err` means this call stopped before sending, not that an earlier send cannot pay.
    /// After sending begins, all failures produce an outcome that retains uncertainty.
    pub async fn broadcast(
        &self,
        prepared: &PreparedSettlement,
        plan: &SigningPlan,
        raw: &[u8],
    ) -> Result<Broadcast, EvmError> {
        plan.validate(&self.deployment, prepared, raw)?;
        self.send_validated(raw).await
    }

    /// Validates the exact stored bytes against a backend-validated call before sending.
    pub async fn broadcast_call(
        &self,
        plan: &SigningPlan,
        raw: &[u8],
        target: [u8; 20],
        calldata: &[u8],
    ) -> Result<Broadcast, EvmError> {
        if target != self.deployment.settlement_contract {
            return Err(EvmError::SignedIntentMismatch);
        }
        plan.validate_call(self.deployment.chain_id, target, calldata, raw)?;
        self.send_validated(raw).await
    }

    async fn send_validated(&self, raw: &[u8]) -> Result<Broadcast, EvmError> {
        let hash = SignedTransaction::from_raw(raw)?.hash();
        self.check_chain().await?;
        let request = self
            .provider
            .client()
            .request::<_, B256>("eth_sendRawTransaction", (Bytes::copy_from_slice(raw),));
        let outcome = match tokio::time::timeout(self.timeout, request).await {
            Ok(Ok(returned)) if returned.0 == hash => BroadcastOutcome::Acknowledged,
            Ok(Err(RpcError::ErrorResp(error))) => BroadcastOutcome::Rejected { code: error.code },
            _ => BroadcastOutcome::Unknown,
        };
        Ok(Broadcast { hash, outcome })
    }
}

/// Journaling or RPC preflight failed. Neither variant establishes non-payment.
#[derive(Debug, thiserror::Error)]
pub enum JournaledBroadcastError {
    /// The record remains held for reconciliation.
    #[error(transparent)]
    Coordinator(#[from] erebus_coordinator::Error),
    /// Validation or chain preflight failed; earlier attempts may still pay.
    #[error(transparent)]
    Evm(#[from] EvmError),
}
