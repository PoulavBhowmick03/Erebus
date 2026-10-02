//! Admission policy for the public-bound transaction relayer.
//! The relayer receives the agreement opening: it is not a private-payment service.

use crate::{
    backend::{validate_prepared, EvmSettlementBackend, FundingDiagnostics},
    chain::{Broadcast, BroadcastOutcome, EvmChain, SignedTransaction, SigningPlan},
    deployment::EvmDeployment,
    error::EvmError,
    evidence::SettlementEvidence,
};
use alloy::primitives::keccak256;
use erebus_core::{
    commitment::{commit_agreement, deal_nullifier, CommitmentBlinding},
    settlement::PreparedSettlement,
    terms::AgreementTerms,
};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

mod operator;
pub use operator::{DurableRelayer, RelayOperation, RelayOperationError};

/// A durable relay failure is never evidence that the deal was unpaid.
#[derive(Debug, thiserror::Error)]
pub enum DurableRelayError {
    /// Admission failed before any network send.
    #[error(transparent)]
    Admission(#[from] RelayError),
    /// Keep reservations and recover the durable record.
    #[error(transparent)]
    Journal(#[from] erebus_coordinator::Error),
    /// Stored transaction or configured deployment does not match.
    #[error(transparent)]
    Transaction(#[from] EvmError),
}

/// Maximum decoded evidence accepted before parsing or signature verification.
pub const MAX_RELAY_EVIDENCE_BYTES: usize = 64 * 1024;

/// Fixed-fee policy published before the agents authorize their deal.
/// Values are token base units, not gas estimates or exchange-rate quotes.
#[derive(Clone)]
pub struct RelayPolicy {
    deployment: EvmDeployment,
    fee_recipient: [u8; 20],
    token_fees: BTreeMap<[u8; 20], u128>,
    max_lifetime: u64,
}
impl std::fmt::Debug for RelayPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelayPolicy")
            .field("chain_id", &self.deployment.chain_id)
            .field("asset_count", &self.token_fees.len())
            .finish_non_exhaustive()
    }
}

/// Redacted admission errors; no terms, signatures, or upstream response text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RelayError {
    /// Empty or unsafe service configuration.
    #[error("invalid relayer policy")]
    Configuration,
    /// Malformed, oversized, wrong-domain, or unauthorized agreement evidence.
    #[error("invalid relay agreement")]
    Agreement,
    /// The agreement is expired or outlives the configured admission window.
    #[error("agreement outside relayer lifetime window")]
    Expiry,
    /// The signed token, fee amount, or fee recipient differs from the published policy.
    #[error("agreement does not match relayer fee policy")]
    Fee,
    /// The client exceeded its configured access limit.
    #[error("relayer access limit exceeded")]
    RateLimited,
}

/// Access limits for one relayer service, applied per opaque client identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessLimits {
    /// Requests allowed per window.
    pub max_requests: u32,
    /// Window length in seconds.
    pub window_seconds: u64,
}

impl AccessLimits {
    /// A limit of `max_requests` per `window_seconds`.
    #[must_use]
    pub const fn new(max_requests: u32, window_seconds: u64) -> Self {
        Self {
            max_requests,
            window_seconds,
        }
    }
}

impl Default for AccessLimits {
    fn default() -> Self {
        Self::new(60, 60)
    }
}

/// Redacted relayer counters. They contain no agreement, signature, or client data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RelayMetricsSnapshot {
    /// Agreements admitted for submission.
    pub admitted: u64,
    /// Agreements rejected by admission, for any reason.
    pub rejected: u64,
    /// Requests refused by the access limit.
    pub rate_limited: u64,
    /// Submissions acknowledged by a provider.
    pub submitted: u64,
    /// Submissions that failed on every configured provider.
    pub submit_failed: u64,
}

#[derive(Default)]
struct Counters {
    admitted: AtomicU64,
    rejected: AtomicU64,
    rate_limited: AtomicU64,
    submitted: AtomicU64,
    submit_failed: AtomicU64,
}

#[derive(Debug, Clone, Copy)]
struct Window {
    started: u64,
    count: u32,
}

/// A relayer that admits against its published policy and submits over failover providers.
///
/// `admit` validates locally and counts each request; `submit_journaled` tries each provider
/// using already-signed, durable bytes. Neither method releases anything on failure: the
/// caller's coordinator keeps its reservation and nonce claim until chain evidence resolves
/// the deal.
pub struct RelayService {
    policy: RelayPolicy,
    backends: Vec<EvmSettlementBackend>,
    limits: AccessLimits,
    windows: Mutex<BTreeMap<String, Window>>,
    counters: Counters,
}

impl std::fmt::Debug for RelayService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RelayService")
            .field("policy", &self.policy)
            .field("providers", &self.backends.len())
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl RelayService {
    /// Broadcasts one already-persisted transaction over ordered providers.
    /// Every provider receives the same bytes; this method never signs or allocates a nonce.
    /// Cancellation leaves the durable attempt Unknown. An acknowledgment is not payment.
    pub async fn submit_journaled(
        &self,
        client: &str,
        coordinator: &erebus_coordinator::Coordinator,
        operation_ref: [u8; 32],
        now: u64,
        timeout: Duration,
    ) -> Result<Broadcast, DurableRelayError> {
        if timeout.is_zero() {
            return Err(RelayError::Configuration.into());
        }
        // Admission must precede the durable Unknown attempt. A rejected client or
        // agreement must not consume the operation's bounded attempt history.
        let stored = coordinator
            .signed_transaction(operation_ref)?
            .ok_or(erebus_coordinator::Error::Stage)?;
        self.admit(client, &stored.prepared().backend_evidence, now)?;
        self.submit_admitted_journaled(coordinator, operation_ref, now, timeout)
            .await
    }

    async fn submit_admitted_journaled(
        &self,
        coordinator: &erebus_coordinator::Coordinator,
        operation_ref: [u8; 32],
        now: u64,
        timeout: Duration,
    ) -> Result<Broadcast, DurableRelayError> {
        let stored = coordinator
            .signed_transaction(operation_ref)?
            .ok_or(erebus_coordinator::Error::Stage)?;
        let stored_plan = SigningPlan::decode(stored.plan())?;
        stored_plan.validate(self.policy.deployment(), stored.prepared(), stored.raw())?;
        for backend in &self.backends {
            backend
                .deployment()
                .matches_domain(&stored.prepared().domain)?;
            if backend.signer_address() != stored_plan.sender() {
                return Err(EvmError::SignedIntentMismatch.into());
            }
        }
        let attempt = coordinator.begin_broadcast_attempt(operation_ref, now)?;
        let transaction = &attempt.transaction;
        if transaction.prepared().backend_evidence != stored.prepared().backend_evidence {
            return Err(EvmError::SignedIntentMismatch.into());
        }
        let plan = SigningPlan::decode(transaction.plan())?;
        plan.validate(
            self.policy.deployment(),
            transaction.prepared(),
            transaction.raw(),
        )?;
        for backend in &self.backends {
            backend
                .deployment()
                .matches_domain(&transaction.prepared().domain)?;
            if backend.signer_address() != plan.sender() {
                return Err(EvmError::SignedIntentMismatch.into());
            }
        }
        let hash = SignedTransaction::from_raw(transaction.raw())?.hash();
        let mut outcome = BroadcastOutcome::Unknown;
        for backend in &self.backends {
            let Ok(chain) = EvmChain::connect(backend.deployment().clone(), timeout).await else {
                continue;
            };
            let result = chain
                .broadcast(transaction.prepared(), &plan, transaction.raw())
                .await;
            if matches!(
                result,
                Ok(Broadcast {
                    outcome: BroadcastOutcome::Acknowledged,
                    ..
                })
            ) {
                outcome = BroadcastOutcome::Acknowledged;
                break;
            }
        }
        // A later rejection cannot erase uncertainty from an earlier provider.
        let recorded = if outcome == BroadcastOutcome::Acknowledged {
            erebus_coordinator::BroadcastOutcome::Submitted
        } else {
            erebus_coordinator::BroadcastOutcome::Unknown
        };
        coordinator.finish_broadcast_attempt(operation_ref, &attempt.token, recorded)?;
        if outcome == BroadcastOutcome::Acknowledged {
            self.counters.submitted.fetch_add(1, Ordering::Relaxed);
        } else {
            self.counters.submit_failed.fetch_add(1, Ordering::Relaxed);
        }
        Ok(Broadcast { hash, outcome })
    }

    /// Builds a service over one or more providers. At least one is required.
    pub fn new(
        policy: RelayPolicy,
        backends: Vec<EvmSettlementBackend>,
        limits: AccessLimits,
    ) -> Result<Self, RelayError> {
        if backends.is_empty() || limits.max_requests == 0 || limits.window_seconds == 0 {
            return Err(RelayError::Configuration);
        }
        let signer = backends[0].signer_address();
        if backends.iter().any(|backend| {
            let deployment = backend.deployment();
            deployment.namespace != policy.deployment.namespace
                || deployment.chain_id != policy.deployment.chain_id
                || deployment.settlement_contract != policy.deployment.settlement_contract
                || deployment.verifier_version != policy.deployment.verifier_version
                || backend.signer_address() != signer
        }) {
            return Err(RelayError::Configuration);
        }
        Ok(Self {
            policy,
            backends,
            limits,
            windows: Mutex::new(BTreeMap::new()),
            counters: Counters::default(),
        })
    }

    /// The published policy, including the fee schedule a buyer binds before signing.
    #[must_use]
    pub fn policy(&self) -> &RelayPolicy {
        &self.policy
    }

    /// Applies the access limit and admits one agreement.
    pub fn admit(
        &self,
        client: &str,
        bytes: &[u8],
        now: u64,
    ) -> Result<PreparedSettlement, RelayError> {
        if !self.allow(client, now) {
            self.counters.rate_limited.fetch_add(1, Ordering::Relaxed);
            return Err(RelayError::RateLimited);
        }
        match self.policy.admit(bytes, now) {
            Ok(prepared) => {
                self.counters.admitted.fetch_add(1, Ordering::Relaxed);
                Ok(prepared)
            }
            Err(error) => {
                self.counters.rejected.fetch_add(1, Ordering::Relaxed);
                Err(error)
            }
        }
    }

    /// Reports gas-payer funding from the first successful configured provider.
    /// A failed read is not evidence that the account has enough gas.
    pub async fn funding(
        &self,
        prepared: &PreparedSettlement,
    ) -> Result<FundingDiagnostics, EvmError> {
        self.funding_with_timeout(prepared, Duration::from_secs(15))
            .await
    }

    /// Reads funding over ordered providers with a per-provider deadline.
    /// A zero deadline is rejected before any RPC request.
    pub async fn funding_with_timeout(
        &self,
        prepared: &PreparedSettlement,
        timeout: Duration,
    ) -> Result<FundingDiagnostics, EvmError> {
        if timeout.is_zero() {
            return Err(EvmError::InconsistentObservation("zero funding deadline"));
        }
        let mut last_error = None;
        for backend in &self.backends {
            match tokio::time::timeout(timeout, backend.funding_diagnostics(prepared)).await {
                Ok(Ok(funding)) => return Ok(funding),
                Ok(Err(error)) => last_error = Some(error),
                Err(_) => last_error = Some(EvmError::Rpc("funding read timed out".to_owned())),
            }
        }
        Err(last_error.unwrap_or(EvmError::InconsistentObservation("no funding provider")))
    }

    /// Reads the redacted counters.
    #[must_use]
    pub fn metrics(&self) -> RelayMetricsSnapshot {
        RelayMetricsSnapshot {
            admitted: self.counters.admitted.load(Ordering::Relaxed),
            rejected: self.counters.rejected.load(Ordering::Relaxed),
            rate_limited: self.counters.rate_limited.load(Ordering::Relaxed),
            submitted: self.counters.submitted.load(Ordering::Relaxed),
            submit_failed: self.counters.submit_failed.load(Ordering::Relaxed),
        }
    }

    fn allow(&self, client: &str, now: u64) -> bool {
        let Ok(mut windows) = self.windows.lock() else {
            return false;
        };
        let window = windows.entry(client.to_owned()).or_insert(Window {
            started: now,
            count: 0,
        });
        if now.saturating_sub(window.started) >= self.limits.window_seconds {
            window.started = now;
            window.count = 0;
        }
        if window.count >= self.limits.max_requests {
            return false;
        }
        window.count += 1;
        true
    }
}

impl RelayPolicy {
    /// Configures a paid public-bound service. No wildcard token or zero-fee subsidy.
    pub fn new(
        deployment: EvmDeployment,
        fee_recipient: [u8; 20],
        token_fees: BTreeMap<[u8; 20], u128>,
        max_lifetime: u64,
    ) -> Result<Self, RelayError> {
        if fee_recipient == [0; 20]
            || token_fees.is_empty()
            || token_fees.len() > 64
            || token_fees
                .iter()
                .any(|(token, fee)| *token == [0; 20] || *fee == 0)
            || max_lifetime == 0
            || deployment.namespace.to_string() != format!("eip155:{}", deployment.chain_id)
        {
            return Err(RelayError::Configuration);
        }
        Ok(Self {
            deployment,
            fee_recipient,
            token_fees,
            max_lifetime,
        })
    }

    /// The deployment this relayer serves.
    #[must_use]
    pub const fn deployment(&self) -> &EvmDeployment {
        &self.deployment
    }

    /// Published fee recipient, to be bound into both parties' authorization.
    #[must_use]
    pub const fn fee_recipient(&self) -> [u8; 20] {
        self.fee_recipient
    }

    /// Exact fixed fees by supported ERC-20 address.
    #[must_use]
    pub fn token_fees(&self) -> &BTreeMap<[u8; 20], u128> {
        &self.token_fees
    }

    /// Validates both signatures and all settlement fields before any gas can be spent.
    /// Derives a deterministic local operation ID; callers cannot choose colliding IDs.
    /// An accepted request still requires durable journaling, funding checks, and rate limits.
    pub fn admit(&self, bytes: &[u8], now: u64) -> Result<PreparedSettlement, RelayError> {
        let prepared = self.validate_evidence(bytes)?;
        let evidence = SettlementEvidence::decode(bytes).map_err(|_| RelayError::Agreement)?;
        let terms = AgreementTerms::decode(&evidence.terms).map_err(|_| RelayError::Agreement)?;
        terms
            .expiry
            .checked_sub(now)
            .filter(|remaining| *remaining > 0 && *remaining <= self.max_lifetime)
            .ok_or(RelayError::Expiry)?;
        Ok(prepared)
    }

    // Recovery validates the original authorization even after the agreement expires.
    fn validate_evidence(&self, bytes: &[u8]) -> Result<PreparedSettlement, RelayError> {
        self.validate_evidence_inner(bytes, true)
    }

    // A fee-schedule change must not prevent observation of an already persisted request.
    fn validate_recovery(&self, bytes: &[u8]) -> Result<PreparedSettlement, RelayError> {
        self.validate_evidence_inner(bytes, false)
    }

    fn validate_evidence_inner(
        &self,
        bytes: &[u8],
        check_fee: bool,
    ) -> Result<PreparedSettlement, RelayError> {
        if bytes.is_empty() || bytes.len() > MAX_RELAY_EVIDENCE_BYTES {
            return Err(RelayError::Agreement);
        }
        let evidence = SettlementEvidence::decode(bytes).map_err(|_| RelayError::Agreement)?;
        let terms = AgreementTerms::decode(&evidence.terms).map_err(|_| RelayError::Agreement)?;
        let commitment =
            commit_agreement(&terms, &CommitmentBlinding::from_bytes(evidence.blinding))
                .map_err(|_| RelayError::Agreement)?;
        let nullifier = deal_nullifier(&terms).map_err(|_| RelayError::Agreement)?;
        let mut identity = b"EREBUS_RELAY_OPERATION_V1".to_vec();
        identity.extend_from_slice(commitment.as_bytes());
        identity.extend_from_slice(nullifier.as_bytes());
        let prepared = PreparedSettlement {
            operation_ref: keccak256(identity).0,
            deal_commitment: commitment,
            deal_nullifier: nullifier,
            domain: terms.domain.clone(),
            mode: terms.settlement_mode,
            required_guarantees: terms.required_guarantees,
            backend_evidence: bytes.to_vec(),
        };
        let checked =
            validate_prepared(&self.deployment, &prepared).map_err(|_| RelayError::Agreement)?;
        if check_fee
            && (self.token_fees.get(&checked.token).copied() != Some(terms.fee_policy.fee.get())
                || terms.fee_policy.recipient.as_ref().map(|r| r.as_bytes())
                    != Some(self.fee_recipient.as_slice()))
        {
            return Err(RelayError::Fee);
        }
        Ok(prepared)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (RelayPolicy, Vec<u8>, u64) {
        let vectors: serde_json::Value = serde_json::from_str(include_str!(
            "../../core/tests/fixtures/agreement-v1-vectors.json"
        ))
        .unwrap();
        let v = vectors["vectors"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["terms"]["fee"].as_str().unwrap() != "0" && v["terms"]["suiteId"] == 1)
            .unwrap();
        let decode = |v: &serde_json::Value| hex::decode(v.as_str().unwrap()).unwrap();
        let evidence = SettlementEvidence {
            terms: decode(&v["expected"]["canonicalHex"]),
            blinding: decode(&v["blindingHex"]).try_into().unwrap(),
            buyer_signature: decode(&v["expected"]["buyerSignatureHex"])
                .try_into()
                .unwrap(),
            seller_signature: decode(&v["expected"]["sellerSignatureHex"])
                .try_into()
                .unwrap(),
        };
        let t = AgreementTerms::decode(&evidence.terms).unwrap();
        let deployment = EvmDeployment::new(
            t.domain.namespace.clone(),
            t.domain
                .settlement_contract
                .as_ref()
                .unwrap()
                .as_bytes()
                .try_into()
                .unwrap(),
            t.domain.verifier_version,
            "http://localhost:1",
        )
        .unwrap();
        let recipient = t
            .fee_policy
            .recipient
            .as_ref()
            .unwrap()
            .as_bytes()
            .try_into()
            .unwrap();
        let token = deployment.token_address(&t.asset).unwrap();
        let policy = RelayPolicy::new(
            deployment,
            recipient,
            [(token, t.fee_policy.fee.get())].into_iter().collect(),
            3600,
        )
        .unwrap();
        (policy, evidence.encode(), t.expiry - 100)
    }
    #[test]
    fn valid_request_has_stable_identity_and_exact_fee_policy() {
        let (policy, bytes, now) = fixture();
        let first = policy.admit(&bytes, now).unwrap();
        assert_eq!(first, policy.admit(&bytes, now + 1).unwrap());
        let mut wrong = policy.clone();
        wrong.fee_recipient = [9; 20];
        assert_eq!(wrong.admit(&bytes, now), Err(RelayError::Fee));
        let mut wrong = policy;
        *wrong.token_fees.values_mut().next().unwrap() += 1;
        assert_eq!(wrong.admit(&bytes, now), Err(RelayError::Fee));
    }
    fn service() -> (RelayService, Vec<u8>, u64) {
        let (policy, bytes, now) = fixture();
        let backend = EvmSettlementBackend::connect(policy.deployment().clone(), &[0x11; 32])
            .expect("backend");
        let service =
            RelayService::new(policy, vec![backend], AccessLimits::new(2, 60)).expect("service");
        (service, bytes, now)
    }

    #[test]
    fn access_limits_are_per_client_and_reset_with_the_window() {
        let (service, bytes, now) = service();
        assert!(service.admit("buyer", &bytes, now).is_ok());
        assert!(service.admit("buyer", &bytes, now).is_ok());
        assert_eq!(
            service.admit("buyer", &bytes, now),
            Err(RelayError::RateLimited)
        );
        // A different client has its own window.
        assert!(service.admit("other", &bytes, now).is_ok());
        // The next window resets the count.
        assert!(service.admit("buyer", &bytes, now + 60).is_ok());
    }

    #[test]
    fn metrics_count_admissions_rejections_and_limits_without_payloads() {
        let (policy, bytes, now) = fixture();
        let backend = EvmSettlementBackend::connect(policy.deployment().clone(), &[0x11; 32])
            .expect("backend");
        // Rejected requests still consume an access slot, so allow one more than the sequence.
        let service =
            RelayService::new(policy, vec![backend], AccessLimits::new(3, 60)).expect("service");
        service.admit("buyer", &bytes, now).expect("first");
        service
            .admit("buyer", &[], now)
            .expect_err("empty evidence is rejected");
        service.admit("buyer", &bytes, now).expect("second");
        service
            .admit("buyer", &bytes, now)
            .expect_err("rate limited");
        let metrics = service.metrics();
        assert_eq!(metrics.admitted, 2);
        assert_eq!(metrics.rejected, 1);
        assert_eq!(metrics.rate_limited, 1);
        assert_eq!(metrics.submitted, 0);
        assert_eq!(metrics.submit_failed, 0);
    }

    #[test]
    fn a_service_requires_a_provider_and_positive_limits() {
        let (policy, _, _) = fixture();
        assert!(matches!(
            RelayService::new(policy.clone(), Vec::new(), AccessLimits::default()),
            Err(RelayError::Configuration)
        ));
        let backend = EvmSettlementBackend::connect(policy.deployment().clone(), &[0x11; 32])
            .expect("backend");
        assert!(matches!(
            RelayService::new(policy, vec![backend], AccessLimits::new(0, 60)),
            Err(RelayError::Configuration)
        ));
    }

    #[test]
    fn failover_providers_must_share_deployment_and_signer() {
        let (policy, _, _) = fixture();
        let primary = EvmSettlementBackend::connect(policy.deployment().clone(), &[0x11; 32])
            .expect("primary");
        let mut other_endpoint = policy.deployment().clone();
        other_endpoint.rpc_url = "http://localhost:2".to_owned();
        let fallback =
            EvmSettlementBackend::connect(other_endpoint.clone(), &[0x11; 32]).expect("fallback");
        assert!(RelayService::new(
            policy.clone(),
            vec![primary, fallback],
            AccessLimits::default(),
        )
        .is_ok());

        let other_signer = EvmSettlementBackend::connect(other_endpoint.clone(), &[0x22; 32])
            .expect("other signer");
        let primary = EvmSettlementBackend::connect(policy.deployment().clone(), &[0x11; 32])
            .expect("primary");
        assert!(matches!(
            RelayService::new(
                policy.clone(),
                vec![primary, other_signer],
                AccessLimits::default()
            ),
            Err(RelayError::Configuration)
        ));

        other_endpoint.settlement_contract[0] ^= 1;
        let other_contract =
            EvmSettlementBackend::connect(other_endpoint, &[0x11; 32]).expect("other contract");
        assert!(matches!(
            RelayService::new(policy, vec![other_contract], AccessLimits::default()),
            Err(RelayError::Configuration)
        ));
    }

    #[test]
    fn invalid_or_expired_requests_never_reach_submission() {
        let (policy, bytes, now) = fixture();
        assert_eq!(policy.admit(&bytes, now + 100), Err(RelayError::Expiry));
        assert_eq!(policy.admit(&bytes, now - 4000), Err(RelayError::Expiry));
        let mut evidence = SettlementEvidence::decode(&bytes).unwrap();
        evidence.buyer_signature = [0; 65];
        assert_eq!(
            policy.admit(&evidence.encode(), now),
            Err(RelayError::Agreement)
        );
        assert_eq!(
            policy.admit(&vec![0; MAX_RELAY_EVIDENCE_BYTES + 1], now),
            Err(RelayError::Agreement)
        );
    }
}
