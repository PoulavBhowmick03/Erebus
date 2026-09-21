//! Settlement backend capabilities, prepared settlements, receipts, and recovery rules.
//!
//! The core does not choose a chain and does not know what a proof is. It checks that the
//! backend selected for a session declares the mode and guarantees the deal requires, and it
//! names the missing guarantee explicitly when it does not (decisions D01: never downgrade a
//! signed privacy requirement). The interface declares guarantees; it does not mandate
//! proofs, so a TEE or private-rollup backend can satisfy it without one.
//!
//! Payment status and delivery status are separate fields. A finalized payment with no
//! delivered service is representable, and it is not completion. A local timeout produces
//! [`Finality::Unknown`], which requires reconciliation, never a fresh payment.

use std::collections::BTreeSet;

use crate::commitment::{DealCommitment, DealNullifier};
use crate::domain::DeploymentDomain;
use crate::ids::AssetId;
use crate::terms::{Guarantee, GuaranteeSet, SettlementMode};

/// What one settlement backend provides.
///
/// A backend reports this honestly, including guarantees it does not have. The existing
/// STRK20 path's declaration lives in `erebus-sdk` (`sdk/rs/src/capabilities.rs`) and reports
/// client-enforced agreement checks, not proof-enforced ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendCapabilities {
    /// Canonical agreement suites supported by this backend. Empty for legacy STRK20.
    pub suites: BTreeSet<u16>,
    /// Settlement modes the backend implements.
    pub modes: BTreeSet<SettlementMode>,
    /// Guarantees the backend provides.
    pub guarantees: GuaranteeSet,
    /// Whether proving happens locally rather than through a hosted prover.
    pub local_proving: bool,
}

impl BackendCapabilities {
    /// Reports whether the backend implements a mode.
    #[must_use]
    pub fn supports_mode(&self, mode: SettlementMode) -> bool {
        self.modes.contains(&mode)
    }

    /// Reports whether the backend provides a guarantee.
    #[must_use]
    pub fn provides(&self, guarantee: Guarantee) -> bool {
        self.guarantees.contains(guarantee)
    }
}

/// The settlement context fixed before negotiation (decisions D01).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementContext {
    /// Refuse a backend that sends private witnesses to a hosted prover.
    pub require_local_proving: bool,
    /// Selected settlement mode.
    pub mode: SettlementMode,
    /// Deployment the session is bound to.
    pub domain: DeploymentDomain,
    /// Agreement suite used for commitments and authorizations.
    pub suite_id: u16,
    /// Asset the deal settles in.
    pub asset: AssetId,
    /// Guarantees the deal requires.
    pub required_guarantees: GuaranteeSet,
}

/// A backend cannot serve the session's requirements.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SelectionError {
    /// The backend cannot execute this canonical agreement suite.
    #[error("backend does not support agreement suite {suite_id}")]
    SuiteUnsupported {
        /// Unsupported suite.
        suite_id: u16,
    },
    /// The selected suite cannot provide this settlement mode.
    #[error(transparent)]
    Suite(#[from] crate::suite::SuiteError),
    /// The deployment domain is malformed.
    #[error(transparent)]
    Domain(#[from] crate::domain::DomainError),
    /// The asset is not on the selected deployment chain.
    #[error("asset is not on the selected deployment chain")]
    AssetDomainMismatch,
    /// The request requires local proving but the backend uses a hosted prover.
    #[error("backend does not provide local proving")]
    LocalProvingRequired,
    /// A transparent mode cannot satisfy a hidden-payment requirement.
    #[error("public-bound settlement cannot provide hidden amount or recipient")]
    ContradictoryPrivacy,
    /// Public-bound canonical agreements require a settlement contract.
    #[error("public-bound context requires a settlement contract")]
    MissingSettlementContract,
    /// The backend does not implement the selected mode.
    #[error("backend does not support settlement mode {mode}")]
    ModeUnsupported {
        /// The unsupported mode.
        mode: SettlementMode,
    },
    /// The backend does not provide a required guarantee.
    #[error("backend does not provide required guarantee `{guarantee}`")]
    MissingGuarantee {
        /// The missing guarantee.
        guarantee: Guarantee,
    },
}

/// Checks a backend's declaration against the session's requirements.
///
/// This is the explicit-failure point: a caller must not proceed by weakening the
/// requirements when this returns an error.
pub fn check_capabilities(
    context: &SettlementContext,
    capabilities: &BackendCapabilities,
) -> Result<(), SelectionError> {
    check_requirements(
        context.mode,
        context.required_guarantees,
        context.require_local_proving,
        capabilities,
    )?;
    if !capabilities.suites.contains(&context.suite_id) {
        return Err(SelectionError::SuiteUnsupported {
            suite_id: context.suite_id,
        });
    }
    crate::suite::check_mode(context.suite_id, context.mode)?;
    context.domain.validate()?;
    if context.mode == SettlementMode::PublicBound && context.domain.settlement_contract.is_none() {
        return Err(SelectionError::MissingSettlementContract);
    }
    if context.asset.namespace() != &context.domain.namespace {
        return Err(SelectionError::AssetDomainMismatch);
    }
    Ok(())
}

/// Checks guarantees for either a canonical backend or a legacy adapter.
///
/// This does not establish canonical suite compatibility. Canonical requests must also
/// pass `check_capabilities`; legacy requests must retain their own wire and domain checks.
pub fn check_requirements(
    mode: SettlementMode,
    required_guarantees: GuaranteeSet,
    require_local_proving: bool,
    capabilities: &BackendCapabilities,
) -> Result<(), SelectionError> {
    if !capabilities.supports_mode(mode) {
        return Err(SelectionError::ModeUnsupported { mode });
    }
    if mode == SettlementMode::PublicBound
        && (required_guarantees.contains(Guarantee::HiddenAmount)
            || required_guarantees.contains(Guarantee::HiddenRecipient))
    {
        return Err(SelectionError::ContradictoryPrivacy);
    }
    for guarantee in required_guarantees.iter() {
        if !capabilities.provides(guarantee) {
            return Err(SelectionError::MissingGuarantee { guarantee });
        }
    }
    if require_local_proving && !capabilities.local_proving {
        return Err(SelectionError::LocalProvingRequired);
    }
    Ok(())
}

/// A backend-specific prepared transition for one authorized deal.
///
/// `backend_evidence` is opaque to the core: it holds whatever the backend must carry from
/// preparation to submission (a proof and public inputs, or a signed envelope). No spending
/// secret or witness belongs in it once it leaves the local environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedSettlement {
    /// Caller-supplied operation reference, bound to the durable journal before preparation.
    pub operation_ref: [u8; 32],
    /// The authorized commitment this transition settles.
    pub deal_commitment: DealCommitment,
    /// Consumed identity shared by signed revisions of the deal.
    pub deal_nullifier: DealNullifier,
    /// Deployment the transition targets.
    pub domain: DeploymentDomain,
    /// Selected settlement mode.
    pub mode: SettlementMode,
    /// Guarantees the preparation was built to satisfy.
    pub required_guarantees: GuaranteeSet,
    /// Backend-internal evidence.
    pub backend_evidence: Vec<u8>,
}

/// How far a submitted transition has progressed on chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Finality {
    /// No chain evidence establishes the outcome; reconciliation is required.
    Unknown,
    /// Submitted and not yet included.
    Pending,
    /// Included in a block that could still be reorganized.
    Included,
    /// Final under the backend's finality rule.
    Finalized,
    /// Included and reverted.
    Reverted,
    /// The authorized expiry passed before settlement.
    Expired,
}

/// The payment half of a receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaymentStatus {
    /// No submission was made.
    NotSubmitted,
    /// Submitted and awaiting chain evidence.
    Pending,
    /// Chain evidence shows the payment executed.
    Settled,
    /// Chain evidence shows the payment reverted.
    Reverted,
    /// No local data establishes the outcome.
    Unknown,
}

/// The delivery half of a receipt, kept separate from payment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeliveryStatus {
    /// The service has not been delivered.
    NotStarted,
    /// Access was issued; the digest identifies the fulfillment evidence.
    Issued {
        /// Digest of the issuer's fulfillment evidence.
        evidence_digest: [u8; 32],
    },
    /// Delivery failed or was refused.
    Failed,
    /// No local data establishes the outcome.
    Unknown,
}

/// The normalized result of one settlement attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementReceipt {
    /// The operation reference preparation bound.
    pub operation_ref: [u8; 32],
    /// The commitment the settlement covers.
    pub deal_commitment: DealCommitment,
    /// Consumed identity established by the settlement evidence.
    pub deal_nullifier: DealNullifier,
    /// Deployment the evidence is from.
    pub domain: DeploymentDomain,
    /// Settlement mode that produced it.
    pub mode: SettlementMode,
    /// Chain-specific transaction identity.
    pub transaction_ref: Vec<u8>,
    /// Chain-specific block identity, when known.
    pub block_ref: Option<Vec<u8>>,
    /// Finality state of the transaction.
    pub finality: Finality,
    /// Guarantees the backend verified for this receipt.
    pub verified_guarantees: GuaranteeSet,
    /// Payment state.
    pub payment: PaymentStatus,
    /// Delivery state.
    pub delivery: DeliveryStatus,
    /// When the receipt was observed.
    pub observed_at: u64,
}

impl SettlementReceipt {
    /// Reports chain evidence that the payment executed, including non-final inclusion.
    #[must_use]
    pub fn payment_settled(&self) -> bool {
        matches!(self.finality, Finality::Included | Finality::Finalized)
            && self.payment == PaymentStatus::Settled
    }

    /// Reports reorg-stable payment finality.
    #[must_use]
    pub fn payment_finalized(&self) -> bool {
        self.finality == Finality::Finalized && self.payment == PaymentStatus::Settled
    }

    /// Reports that access was issued.
    #[must_use]
    pub fn delivery_issued(&self) -> bool {
        matches!(self.delivery, DeliveryStatus::Issued { .. })
    }

    /// Reports whether the local state cannot establish the payment outcome.
    #[must_use]
    pub fn payment_is_uncertain(&self) -> bool {
        self.payment == PaymentStatus::Unknown || self.finality == Finality::Unknown
    }

    /// The product-level completion test: finalized payment and issued delivery.
    ///
    /// A receipt that fails this is not a failure; it may be a paid deal whose service is
    /// still pending, which is why the two statuses are separate.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.payment_finalized() && self.delivery_issued()
    }
}

/// What a coordinator should do about an operation's finality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Wait for more chain evidence; resubmission is not yet justified.
    AwaitChain,
    /// Resolve the outcome from chain and journal evidence before any resubmission.
    Reconcile,
    /// The operation reached a terminal state; do not submit again.
    Stop,
}

/// Maps a finality state to the required recovery action.
///
/// `Unknown` deliberately maps to [`RecoveryAction::Reconcile`]: a local timeout is never
/// evidence that an authorized deal is unpaid, so the coordinator must reconcile rather than
/// submit a second payment. `Included` may still be reorganized, so it awaits chain evidence.
#[must_use]
pub fn recovery_action(finality: Finality) -> RecoveryAction {
    match finality {
        Finality::Unknown => RecoveryAction::Reconcile,
        Finality::Pending | Finality::Included => RecoveryAction::AwaitChain,
        Finality::Finalized | Finality::Reverted | Finality::Expired => RecoveryAction::Stop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_checks_suites_modes_local_proving_and_asset_domain() {
        let mut context = context();
        let mut capabilities = capabilities();
        assert_eq!(check_capabilities(&context, &capabilities), Ok(()));
        capabilities.suites.clear();
        assert_eq!(
            check_capabilities(&context, &capabilities),
            Err(SelectionError::SuiteUnsupported { suite_id: 1 })
        );
        capabilities.suites.insert(999);
        context.suite_id = 999;
        assert_eq!(
            check_capabilities(&context, &capabilities),
            Err(SelectionError::Suite(
                crate::suite::SuiteError::Unsupported(999)
            ))
        );
        capabilities.suites.insert(1);
        context.suite_id = 1;
        capabilities.modes.insert(SettlementMode::Shielded);
        context.mode = SettlementMode::Shielded;
        assert_eq!(
            check_capabilities(&context, &capabilities),
            Err(SelectionError::Suite(
                crate::suite::SuiteError::UnsupportedMode {
                    suite_id: 1,
                    mode: SettlementMode::Shielded
                }
            ))
        );
        context.mode = SettlementMode::PublicBound;
        capabilities.local_proving = false;
        assert_eq!(
            check_capabilities(&context, &capabilities),
            Err(SelectionError::LocalProvingRequired)
        );
        context.require_local_proving = false;
        context.asset = AssetId::parse("eip155:1/erc20:0xaa").unwrap();
        assert_eq!(
            check_capabilities(&context, &capabilities),
            Err(SelectionError::AssetDomainMismatch)
        );
    }

    #[test]
    fn a_backend_declaration_cannot_override_mode_or_deployment_rules() {
        let mut context = context();
        let mut capabilities = capabilities();
        context.required_guarantees.insert(Guarantee::HiddenAmount);
        capabilities.guarantees.insert(Guarantee::HiddenAmount);
        assert_eq!(
            check_capabilities(&context, &capabilities),
            Err(SelectionError::ContradictoryPrivacy)
        );
        context.required_guarantees = GuaranteeSet::empty();
        context.domain.pool = context.domain.settlement_contract.take();
        assert_eq!(
            check_capabilities(&context, &capabilities),
            Err(SelectionError::MissingSettlementContract)
        );
    }

    fn capabilities() -> BackendCapabilities {
        let mut modes = BTreeSet::new();
        modes.insert(SettlementMode::PublicBound);
        BackendCapabilities {
            suites: [1].into_iter().collect(),
            modes,
            guarantees: GuaranteeSet::empty(),
            local_proving: true,
        }
    }

    fn context() -> SettlementContext {
        let terms = crate::terms::tests::example_terms();
        SettlementContext {
            require_local_proving: true,
            mode: SettlementMode::PublicBound,
            domain: terms.domain,
            suite_id: terms.suite_id,
            asset: terms.asset,
            required_guarantees: GuaranteeSet::empty(),
        }
    }

    #[test]
    fn selection_fails_explicitly_and_names_the_guarantee() {
        let mut context = context();
        context
            .required_guarantees
            .insert(Guarantee::AgreementBoundSettlement);
        assert_eq!(
            check_capabilities(&context, &capabilities()),
            Err(SelectionError::MissingGuarantee {
                guarantee: Guarantee::AgreementBoundSettlement
            })
        );
    }

    #[test]
    fn selection_fails_for_an_unsupported_mode() {
        let mut context = context();
        context.mode = SettlementMode::Shielded;
        assert_eq!(
            check_capabilities(&context, &capabilities()),
            Err(SelectionError::ModeUnsupported {
                mode: SettlementMode::Shielded
            })
        );
    }

    #[test]
    fn receipt_keeps_payment_and_delivery_separate() {
        let terms = crate::terms::tests::example_terms();
        let mut receipt = SettlementReceipt {
            operation_ref: [0x01; 32],
            deal_commitment: DealCommitment::from_bytes([0x02; 32]),
            deal_nullifier: crate::commitment::deal_nullifier(&terms).unwrap(),
            domain: terms.domain.clone(),
            mode: SettlementMode::PublicBound,
            transaction_ref: vec![0xaa; 32],
            block_ref: Some(vec![0xbb; 8]),
            finality: Finality::Finalized,
            verified_guarantees: GuaranteeSet::from_guarantee(Guarantee::AgreementBoundSettlement),
            payment: PaymentStatus::Settled,
            delivery: DeliveryStatus::NotStarted,
            observed_at: 1_800_000_000,
        };
        assert!(receipt.payment_finalized());
        assert!(!receipt.delivery_issued());
        assert!(!receipt.is_complete());
        receipt.delivery = DeliveryStatus::Issued {
            evidence_digest: [0xcc; 32],
        };
        assert!(receipt.is_complete());
    }

    #[test]
    fn unknown_finality_requires_reconciliation_not_resubmission() {
        assert_eq!(
            recovery_action(Finality::Unknown),
            RecoveryAction::Reconcile
        );
        assert_eq!(
            recovery_action(Finality::Pending),
            RecoveryAction::AwaitChain
        );
        assert_eq!(
            recovery_action(Finality::Included),
            RecoveryAction::AwaitChain
        );
        assert_eq!(recovery_action(Finality::Finalized), RecoveryAction::Stop);
        assert_eq!(recovery_action(Finality::Reverted), RecoveryAction::Stop);
        assert_eq!(recovery_action(Finality::Expired), RecoveryAction::Stop);
    }

    #[test]
    fn a_local_timeout_leaves_payment_uncertain() {
        let terms = crate::terms::tests::example_terms();
        let receipt = SettlementReceipt {
            operation_ref: [0x01; 32],
            deal_commitment: DealCommitment::from_bytes([0x02; 32]),
            deal_nullifier: crate::commitment::deal_nullifier(&terms).unwrap(),
            domain: terms.domain,
            mode: SettlementMode::PublicBound,
            transaction_ref: Vec::new(),
            block_ref: None,
            finality: Finality::Unknown,
            verified_guarantees: GuaranteeSet::empty(),
            payment: PaymentStatus::Unknown,
            delivery: DeliveryStatus::NotStarted,
            observed_at: 1_800_000_000,
        };
        assert!(receipt.payment_is_uncertain());
        assert!(!receipt.payment_settled());
    }
}
