//! Independent public-bound disclosure verification against finalized EVM state.

pub mod cli;

use erebus_core::deal_state::{DealEvidence, DealReads};
use erebus_core::terms::SettlementMode;
use erebus_transport::disclosure::{
    DisclosureGrant, GrantError, SelectedAgreement, VerifiedAgreement,
};
use erebus_transport::identity::DisclosureIdentity;

use crate::chain::{EvmChain, ObservationLimits};
use crate::error::EvmError;

/// A selected agreement and its verified final public-bound payment.
pub struct FinalizedDisclosure {
    /// Private agreement evidence, opened only by the intended recipient.
    pub evidence: SelectedAgreement,
    /// Facts verified from the selected transcript and both authorizations.
    pub agreement: VerifiedAgreement,
    /// Independently observed finalized payment evidence, not an issuer-supplied receipt.
    pub settlement: DealReads,
}

impl core::fmt::Debug for FinalizedDisclosure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("FinalizedDisclosure { <redacted> }")
    }
}

/// The grant or final payment could not be independently verified.
#[derive(Debug, thiserror::Error)]
pub enum DisclosureVerificationError {
    /// The recipient could not open and verify the grant.
    #[error(transparent)]
    Grant(#[from] GrantError),
    /// The chain observer failed or its deployment differs from the agreement.
    #[error(transparent)]
    Chain(#[from] EvmError),
    /// No finalized matching payment was found.
    #[error("the disclosed agreement has no verified final matching payment")]
    Payment,
}

/// Opens one grant and checks that its issuer is an authorized public-bound participant.
/// This verifies the selected agreement only, not payment or service delivery.
pub fn open_public_bound_disclosure(
    grant: &DisclosureGrant,
    recipient: &DisclosureIdentity,
    expected_issuer: [u8; 20],
    now: u64,
) -> Result<(SelectedAgreement, VerifiedAgreement), DisclosureVerificationError> {
    let (evidence, agreement) = grant.open(recipient, expected_issuer, now)?;
    if evidence.terms.settlement_mode != SettlementMode::PublicBound {
        return Err(DisclosureVerificationError::Payment);
    }
    if evidence.terms.buyer_authorization_key.as_bytes() != expected_issuer
        && evidence.terms.seller_authorization_key.as_bytes() != expected_issuer
    {
        return Err(DisclosureVerificationError::Payment);
    }
    Ok((evidence, agreement))
}

/// Opens one grant and queries the configured chain for final settlement evidence.
/// The caller supplies the expected issuer identity and its own trusted RPC configuration.
pub async fn verify_public_bound_disclosure(
    grant: &DisclosureGrant,
    recipient: &DisclosureIdentity,
    expected_issuer: [u8; 20],
    now: u64,
    chain: &EvmChain,
    limits: ObservationLimits,
) -> Result<FinalizedDisclosure, DisclosureVerificationError> {
    let (evidence, agreement) =
        open_public_bound_disclosure(grant, recipient, expected_issuer, now)?;
    chain.deployment().matches_domain(&evidence.terms.domain)?;
    let DealEvidence::Observed(reads) = chain
        .finalized_deal_evidence(&agreement.nullifier, limits)
        .await?
    else {
        return Err(DisclosureVerificationError::Payment);
    };
    let Some(winner) = &reads.winner else {
        return Err(DisclosureVerificationError::Payment);
    };
    if reads.deal_nullifier != agreement.nullifier
        || !reads.consumed_at_final
        || !reads.consumed_at_head
        || !winner.is_final
        || winner.commitment != agreement.commitment
        || winner.amount != evidence.terms.amount
        || winner.fee != evidence.terms.fee_policy.fee
    {
        return Err(DisclosureVerificationError::Payment);
    }
    Ok(FinalizedDisclosure {
        evidence,
        agreement,
        settlement: reads,
    })
}
