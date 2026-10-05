//! Relayer-owned recovery state. This is not the buyer's authoritative policy ledger.

use super::{RelayError, RelayService};
use crate::{
    chain::{
        Eip1559Fees, EvmChain, HistoricalObservation, NonceError, ObservationJournal,
        ObservationLimits, SignerJournal, SigningPlan, TransactionKey,
    },
    evidence::SettlementEvidence,
};
use erebus_coordinator::{Coordinator, Stage};
use erebus_core::{
    auth::{Authorization, Role},
    commitment::CommitmentBlinding,
    deal_state::DealEvidence,
    ids::{BaseUnits, SignatureBytes},
    policy::SpendingPolicy,
    settlement::{PreparedSettlement, SettlementContext},
    terms::AgreementTerms,
};
use std::{
    convert::Infallible,
    path::{Path, PathBuf},
    time::Duration,
};

/// Redacted result. A submitted stage does not prove payment.
#[derive(Debug, Clone, Copy)]
pub struct RelayOperation {
    /// Deterministic identity derived from the accepted agreement.
    pub operation_ref: [u8; 32],
    /// Durable progress from the relayer's own coordinator.
    pub stage: Stage,
    /// Transaction hash, if signed bytes were persisted.
    pub transaction_hash: Option<[u8; 32]>,
    /// Whether this gas account still has a durable unresolved nonce claim.
    pub gas_account_busy: bool,
}

/// Redacted operator errors. None establishes that an earlier payment did not occur.
#[derive(Debug, thiserror::Error)]
pub enum RelayOperationError {
    /// Configuration is invalid.
    #[error("invalid durable relayer configuration")]
    Configuration,
    /// Request admission failed.
    #[error(transparent)]
    Admission(#[from] RelayError),
    /// Preserve state and repair or restore the private relayer store.
    #[error("relayer recovery state unavailable; retain the operation")]
    Storage,
    /// Evidence did not establish a canonical final state.
    #[error("relayer chain observation unavailable; retain the operation")]
    Observation,
    /// Repeat recovery to continue the persisted scan. No financial state is released.
    #[error("relayer history scan pending; repeat recovery with the original request")]
    HistoryPending,
    /// Another unresolved operation owns the gas account's nonce lane.
    #[error("relayer gas account has an unresolved operation")]
    Busy,
    /// Funding is insufficient or cannot be verified.
    #[error("relayer funding unavailable or insufficient")]
    Funding,
    /// Signing or broadcast failed. Retry by recovering stored state, not by resetting it.
    #[error("relayer transaction unavailable; retain the operation")]
    Transaction,
}

/// Independently hosted relayer using the existing coordinator and account-wide nonce lane.
/// Each operation has a relayer-owned replica of its authorized public-bound terms.
/// These replicas track delivery, not the buyer's budgets. Never use them instead of
/// client-side policy enforcement. All processes using the gas key must share `root`.
pub struct DurableRelayer {
    root: PathBuf,
    key: TransactionKey,
    fees: Eip1559Fees,
    gas_limit: u64,
    timeout: Duration,
    observation_budget: ObservationLimits,
    verification_rpc_url: String,
}

impl DurableRelayer {
    /// Configures persistent state, gas caps, and a mandatory second RPC for verification.
    pub fn new(
        root: impl AsRef<Path>,
        key: &[u8; 32],
        fees: Eip1559Fees,
        gas_limit: u64,
        timeout: Duration,
        verification_rpc_url: &str,
    ) -> Result<Self, RelayOperationError> {
        if root.as_ref().as_os_str().is_empty() || gas_limit == 0 || timeout.is_zero() {
            return Err(RelayOperationError::Configuration);
        }
        Ok(Self {
            root: root.as_ref().to_path_buf(),
            key: TransactionKey::from_bytes(key).map_err(|_| RelayOperationError::Configuration)?,
            fees,
            gas_limit,
            timeout,
            observation_budget: ObservationLimits::default(),
            verification_rpc_url: crate::deployment::normalized_rpc_url(verification_rpc_url)
                .map_err(|_| RelayOperationError::Configuration)?,
        })
    }

    /// Sets per-call history work, without limiting the total recoverable chain age.
    /// Pending work survives restart in the operator's history journal.
    pub fn with_observation_budget(
        mut self,
        budget: ObservationLimits,
    ) -> Result<Self, RelayOperationError> {
        if budget.log_block_range == 0
            || budget.max_log_queries == 0
            || budget.max_ancestry == 0
            || budget.max_concurrent_queries == 0
            || budget.max_concurrent_queries > crate::chain::MAX_CONCURRENT_LOG_QUERIES
        {
            return Err(RelayOperationError::Configuration);
        }
        self.observation_budget = budget;
        Ok(self)
    }

    /// Persists an already-authorized request, observes before retrying, and submits only
    /// durable bytes. No buyer or seller authorization is generated here.
    pub async fn relay(
        &self,
        service: &RelayService,
        client: &str,
        bytes: &[u8],
        now: u64,
    ) -> Result<RelayOperation, RelayOperationError> {
        self.check_configuration(service)?;
        let prepared = service.admit(client, bytes, now)?;
        let (coordinator, journal) = self.open(service, &prepared, false)?;
        let evidence = SettlementEvidence::decode(bytes).map_err(|_| RelayError::Agreement)?;
        let terms = AgreementTerms::decode(&evidence.terms).map_err(|_| RelayError::Agreement)?;
        let blinding = CommitmentBlinding::from_bytes(evidence.blinding);
        coordinator
            .record_intent(prepared.operation_ref, &terms, &blinding, now)
            .map_err(|_| RelayOperationError::Storage)?;
        // Attach existing signatures; these callbacks hold no participant signing key.
        let buyer = authorization(
            &prepared,
            terms.suite_id,
            Role::Buyer,
            &evidence.buyer_signature,
        )?;
        let seller = authorization(
            &prepared,
            terms.suite_id,
            Role::Seller,
            &evidence.seller_signature,
        )?;
        let current = result(&coordinator, prepared.operation_ref)?;
        if matches!(current.stage, Stage::Finalized | Stage::ClosedUnpaid) {
            self.observe(service, &coordinator, &journal, &prepared, now)
                .await?;
            return self.operation_result(service, &coordinator, &journal, &prepared);
        }
        coordinator
            .authorize_buyer(prepared.operation_ref, now, |_, _| {
                Ok::<_, Infallible>(buyer)
            })
            .map_err(|_| RelayOperationError::Storage)?;
        coordinator
            .accept_seller(prepared.operation_ref, &seller)
            .map_err(|_| RelayOperationError::Storage)?;
        let stored = coordinator
            .prepare(prepared.operation_ref, now, |_, _, _, _| {
                Ok::<_, Infallible>(prepared.clone())
            })
            .map_err(|_| RelayOperationError::Storage)?;
        if stored.backend_evidence != bytes {
            return Err(RelayOperationError::Storage);
        }
        let (chain, consumed) = self
            .observe(service, &coordinator, &journal, &prepared, now)
            .await?;
        let current = result(&coordinator, prepared.operation_ref)?;
        if consumed || matches!(current.stage, Stage::Finalized | Stage::ClosedUnpaid) {
            return self.operation_result(service, &coordinator, &journal, &prepared);
        }

        let deployment = chain.deployment();
        let plan = if let Some(plan) = coordinator
            .transaction_plan(prepared.operation_ref)
            .map_err(|_| RelayOperationError::Storage)?
        {
            let plan = SigningPlan::decode(&plan).map_err(|_| RelayOperationError::Transaction)?;
            if journal.resume(deployment, &prepared).map_err(nonce_error)? != Some(plan.clone()) {
                return Err(RelayOperationError::Storage);
            }
            plan
        } else {
            let funding = service
                .funding_with_timeout(&prepared, self.timeout)
                .await
                .map_err(|_| RelayOperationError::Funding)?;
            let required = u128::from(self.gas_limit)
                .checked_mul(self.fees.max_fee_per_gas())
                .ok_or(RelayOperationError::Configuration)?;
            if funding.balance < required || funding.gas > self.gas_limit {
                return Err(RelayOperationError::Funding);
            }
            chain
                .reserve_nonce(&journal, &prepared, self.fees, self.gas_limit)
                .await
                .map_err(nonce_error)?
        };
        coordinator
            .sign_transaction(
                prepared.operation_ref,
                now,
                &plan.encode(),
                |prepared, encoded| {
                    SigningPlan::decode(encoded)?
                        .sign(deployment, prepared, &self.key)
                        .map(|tx| tx.raw().to_vec())
                },
                |prepared, encoded, raw| {
                    SigningPlan::decode(encoded)?.validate(deployment, prepared, raw)
                },
            )
            .map_err(|_| RelayOperationError::Transaction)?;
        service
            .submit_admitted_journaled(&coordinator, prepared.operation_ref, now, self.timeout)
            .await
            .map_err(|_| RelayOperationError::Transaction)?;
        self.operation_result(service, &coordinator, &journal, &prepared)
    }

    /// Reconciles an existing request, including after expiry. Never creates an operation,
    /// signs, or broadcasts. The caller can supply the original authorized request.
    pub async fn recover(
        &self,
        service: &RelayService,
        client: &str,
        bytes: &[u8],
        now: u64,
    ) -> Result<RelayOperation, RelayOperationError> {
        self.check_configuration(service)?;
        if !service.allow(client, now) {
            service
                .counters
                .rate_limited
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Err(RelayError::RateLimited.into());
        }
        let prepared = service.policy.validate_recovery(bytes)?;
        let (coordinator, journal) = self.open(service, &prepared, true)?;
        match coordinator.prepared_settlement(prepared.operation_ref) {
            Ok(stored) if stored.backend_evidence == bytes => {}
            Err(erebus_coordinator::Error::Stage) => {}
            _ => return Err(RelayOperationError::Storage),
        }
        self.observe(service, &coordinator, &journal, &prepared, now)
            .await?;
        self.operation_result(service, &coordinator, &journal, &prepared)
    }

    fn operation_result(
        &self,
        service: &RelayService,
        coordinator: &Coordinator,
        journal: &SignerJournal,
        prepared: &PreparedSettlement,
    ) -> Result<RelayOperation, RelayOperationError> {
        let mut operation = result(coordinator, prepared.operation_ref)?;
        operation.gas_account_busy = match journal.resume(service.policy.deployment(), prepared) {
            Ok(Some(_)) | Err(NonceError::Busy) => true,
            Ok(None) => false,
            Err(_) => return Err(RelayOperationError::Storage),
        };
        Ok(operation)
    }

    /// Rejects a signer mismatch or a verification endpoint reused for broadcast.
    /// This local check performs no RPC calls and creates no recovery state.
    pub fn check_configuration(&self, service: &RelayService) -> Result<(), RelayOperationError> {
        if service
            .backends
            .iter()
            .any(|backend| backend.signer_address() != self.key.address())
        {
            return Err(RelayOperationError::Configuration);
        }
        for backend in &service.backends {
            let endpoint = crate::deployment::normalized_rpc_url(&backend.deployment().rpc_url)
                .map_err(|_| RelayOperationError::Configuration)?;
            if endpoint == self.verification_rpc_url {
                return Err(RelayOperationError::Configuration);
            }
        }
        Ok(())
    }

    fn open(
        &self,
        service: &RelayService,
        prepared: &PreparedSettlement,
        existing: bool,
    ) -> Result<(Coordinator, SignerJournal), RelayOperationError> {
        let evidence = SettlementEvidence::decode(&prepared.backend_evidence)
            .map_err(|_| RelayError::Agreement)?;
        let terms = AgreementTerms::decode(&evidence.terms).map_err(|_| RelayError::Agreement)?;
        let root = self
            .root
            .join("operations")
            .join(hex::encode(prepared.operation_ref));
        if existing
            && !root
                .try_exists()
                .map_err(|_| RelayOperationError::Storage)?
        {
            return Err(RelayOperationError::Storage);
        }
        let context = SettlementContext {
            require_local_proving: true,
            mode: terms.settlement_mode,
            domain: terms.domain.clone(),
            suite_id: terms.suite_id,
            asset: terms.asset.clone(),
            required_guarantees: terms.required_guarantees,
        };
        let coordinator = Coordinator::open(
            root,
            terms.buyer_authorization_key.clone(),
            context.clone(),
            &context,
            &service.backends[0].capabilities(),
            SpendingPolicy {
                per_deal_max: BaseUnits::new(u128::MAX),
                allowed_assets: [terms.asset].into_iter().collect(),
                ..SpendingPolicy::default()
            },
        )
        .map_err(|_| RelayOperationError::Storage)?;
        let journal = SignerJournal::open(
            self.root.join("signer"),
            service.policy.deployment.chain_id,
            self.key.address(),
        )
        .map_err(nonce_error)?;
        Ok((coordinator, journal))
    }

    async fn observe(
        &self,
        service: &RelayService,
        coordinator: &Coordinator,
        journal: &SignerJournal,
        prepared: &PreparedSettlement,
        now: u64,
    ) -> Result<(EvmChain, bool), RelayOperationError> {
        let mut peer_deployment = service.policy.deployment().clone();
        peer_deployment.rpc_url = self.verification_rpc_url.clone();
        let peer = EvmChain::connect(peer_deployment, self.timeout)
            .await
            .map_err(|_| RelayOperationError::Observation)?;
        let peer_history = ObservationJournal::open(self.history_root(&self.verification_rpc_url))
            .map_err(|_| RelayOperationError::Storage)?;
        for backend in &service.backends {
            let Ok(chain) = EvmChain::connect(backend.deployment().clone(), self.timeout).await
            else {
                continue;
            };
            let endpoint = crate::deployment::normalized_rpc_url(&backend.deployment().rpc_url)
                .map_err(|_| RelayOperationError::Configuration)?;
            let history = ObservationJournal::open(self.history_root(&endpoint))
                .map_err(|_| RelayOperationError::Storage)?;
            let Ok(observation) = chain
                .finalized_deal_evidence_resumable_agreed(
                    &history,
                    &peer,
                    &peer_history,
                    &prepared.deal_nullifier,
                    self.observation_budget,
                )
                .await
            else {
                continue;
            };
            let evidence = match observation {
                HistoricalObservation::Complete { evidence, .. } => evidence,
                HistoricalObservation::Pending { .. } => {
                    return Err(RelayOperationError::HistoryPending)
                }
            };
            let Ok(nonce) = chain
                .verified_finalized_nonce_agreed(&peer, self.key.address())
                .await
            else {
                continue;
            };
            coordinator
                .reconcile(prepared.operation_ref, &evidence, now)
                .map_err(|_| RelayOperationError::Storage)?;
            match journal.release(&nonce) {
                Ok(()) | Err(NonceError::NotConsumed) => {}
                Err(error) => return Err(nonce_error(error)),
            }
            let consumed =
                matches!(evidence, DealEvidence::Observed(ref reads) if reads.consumed_at_head);
            return Ok((chain, consumed));
        }
        Err(RelayOperationError::Observation)
    }

    fn history_root(&self, endpoint: &str) -> PathBuf {
        self.root
            .join("history")
            .join(hex::encode(alloy::primitives::keccak256(
                endpoint.as_bytes(),
            )))
    }
}

fn authorization(
    prepared: &PreparedSettlement,
    suite_id: u16,
    role: Role,
    signature: &[u8],
) -> Result<Authorization, RelayOperationError> {
    Ok(Authorization {
        role,
        suite_id,
        commitment: prepared.deal_commitment,
        signature: SignatureBytes::new(signature.to_vec()).map_err(|_| RelayError::Agreement)?,
    })
}

fn nonce_error(error: NonceError) -> RelayOperationError {
    match error {
        NonceError::Busy => RelayOperationError::Busy,
        _ => RelayOperationError::Storage,
    }
}

fn result(
    coordinator: &Coordinator,
    operation_ref: [u8; 32],
) -> Result<RelayOperation, RelayOperationError> {
    let diagnostic = coordinator
        .diagnostics()
        .map_err(|_| RelayOperationError::Storage)?
        .into_iter()
        .find(|entry| entry.operation_ref == operation_ref)
        .ok_or(RelayOperationError::Storage)?;
    let transaction_hash = coordinator
        .signed_transaction(operation_ref)
        .map_err(|_| RelayOperationError::Storage)?
        .map(|stored| crate::chain::SignedTransaction::from_raw(stored.raw()).map(|tx| tx.hash()))
        .transpose()
        .map_err(|_| RelayOperationError::Transaction)?;
    Ok(RelayOperation {
        operation_ref,
        stage: diagnostic.stage,
        transaction_hash,
        gas_account_busy: false,
    })
}
