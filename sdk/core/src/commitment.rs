//! Blinded deal commitments and the consumed deal identity.
//!
//! The commitment is the only value a participant signs as agreement consent, and the buyer
//! reveals the opening (terms plus blinding) only when settlement needs it. The blinding must
//! be fresh cryptographic randomness per deal: a hash of predictable prices and addresses
//! alone does not hide terms (architecture, section 4).
//!
//! The consumed deal identity follows decisions D02: it derives from a domain tag, the
//! deployment domain, the buyer's authorization key, and the fixed settlement nonce. Every
//! signed revision of one deal shares those inputs, so the first valid revision to settle
//! consumes the deal and a later counteroffer cannot be settled a second time. Pool note
//! nullifiers are separate values and do not replace this identity.

use core::fmt;

use crate::domain::DomainError;
use crate::suite::{self, SuiteError};
use crate::terms::{AgreementTerms, TermsError};

/// Domain separation for the agreement commitment.
pub const COMMITMENT_DOMAIN: &[u8] = b"EREBUS_DEAL_COMMITMENT_V1";
/// Domain separation for the consumed deal identity.
pub const DEAL_NULLIFIER_DOMAIN: &[u8] = b"EREBUS_DEAL_NULLIFIER_V1";
/// Length of a commitment blinding value.
pub const BLINDING_BYTES: usize = 32;

/// A fresh random blinding for one deal commitment.
///
/// Callers generate this with cryptographic randomness. It is treated as sensitive: anyone
/// holding it can test guesses about the terms against the commitment. Do not publish it;
/// persistence should follow key-material rules.
#[derive(Clone, PartialEq, Eq)]
pub struct CommitmentBlinding([u8; BLINDING_BYTES]);

impl CommitmentBlinding {
    /// Wraps caller-generated random bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; BLINDING_BYTES]) -> Self {
        Self(bytes)
    }

    /// Returns the raw bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; BLINDING_BYTES] {
        &self.0
    }
}

impl fmt::Debug for CommitmentBlinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CommitmentBlinding(<redacted>)")
    }
}

/// The blinded commitment both participants authorize.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DealCommitment([u8; BLINDING_BYTES]);

impl DealCommitment {
    /// Wraps an already-computed digest.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; BLINDING_BYTES]) -> Self {
        Self(bytes)
    }

    /// Returns the raw digest.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; BLINDING_BYTES] {
        &self.0
    }

    /// Lowercase hexadecimal form, used by vectors and diagnostics.
    #[must_use]
    pub fn to_hex(&self) -> String {
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            use core::fmt::Write as _;
            write!(&mut out, "{byte:02x}").expect("writing to String cannot fail");
        }
        out
    }
}

impl fmt::Display for DealCommitment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

/// The consumed deal identifier, shared by every revision of one deal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DealNullifier([u8; BLINDING_BYTES]);

impl DealNullifier {
    /// Wraps a consumed identity read from backend evidence.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; BLINDING_BYTES]) -> Self {
        Self(bytes)
    }

    /// Returns the raw digest.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; BLINDING_BYTES] {
        &self.0
    }

    /// Lowercase hexadecimal form, used by vectors and diagnostics.
    #[must_use]
    pub fn to_hex(&self) -> String {
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            use core::fmt::Write as _;
            write!(&mut out, "{byte:02x}").expect("writing to String cannot fail");
        }
        out
    }
}

/// A commitment or deal identity could not be derived.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommitmentError {
    /// The agreement terms were invalid.
    #[error(transparent)]
    Terms(#[from] TermsError),
    /// The selected suite is not implemented.
    #[error(transparent)]
    Suite(#[from] SuiteError),
    /// The deployment domain was invalid.
    #[error(transparent)]
    Domain(#[from] DomainError),
}

/// Computes `keccak256(COMMITMENT_DOMAIN || canonical_terms || blinding)` under the terms'
/// suite.
///
/// This is the value both roles authorize. It is not meaningful without the opening.
pub fn commit_agreement(
    terms: &AgreementTerms,
    blinding: &CommitmentBlinding,
) -> Result<DealCommitment, CommitmentError> {
    let encoded = terms.encode()?;
    let suite = suite::suite(terms.suite_id)?;
    let digest = suite.hash(&[COMMITMENT_DOMAIN, &encoded, blinding.as_bytes()]);
    Ok(DealCommitment(digest))
}

/// Derives the consumed deal identity for every revision of one deal.
///
/// The inputs are the deployment domain, the buyer's authorization key, and the settlement
/// nonce. `deal_id` and `revision` are deliberately not inputs, which is what makes all
/// signed revisions of a deal collide on one identity. The terms' suite defines the hash.
pub fn deal_nullifier(terms: &AgreementTerms) -> Result<DealNullifier, CommitmentError> {
    terms.validate()?;
    let domain = terms.domain.encode()?;
    let suite = suite::suite(terms.suite_id)?;
    let digest = suite.hash(&[
        DEAL_NULLIFIER_DOMAIN,
        &domain,
        terms.buyer_authorization_key.as_bytes(),
        &terms.settlement_nonce,
    ]);
    Ok(DealNullifier(digest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commitment_changes_with_every_field() {
        let terms = crate::terms::tests::example_terms();
        let blinding = CommitmentBlinding::from_bytes([0x99; 32]);
        let baseline = commit_agreement(&terms, &blinding).expect("commits");

        let mut mutated = terms.clone();
        mutated.amount = crate::ids::BaseUnits::new(terms.amount.get() + 1);
        assert_ne!(
            commit_agreement(&mutated, &blinding).expect("commits"),
            baseline
        );

        let mut mutated = terms.clone();
        mutated.service.resource = "gpu.a100.hour".to_owned();
        assert_ne!(
            commit_agreement(&mutated, &blinding).expect("commits"),
            baseline
        );
    }

    #[test]
    fn blinding_changes_the_commitment() {
        let terms = crate::terms::tests::example_terms();
        let first =
            commit_agreement(&terms, &CommitmentBlinding::from_bytes([0x01; 32])).expect("commits");
        let second =
            commit_agreement(&terms, &CommitmentBlinding::from_bytes([0x02; 32])).expect("commits");
        assert_ne!(first, second);
    }

    #[test]
    fn revisions_share_one_deal_identity() {
        let terms = crate::terms::tests::example_terms();
        let mut counter = terms.clone();
        counter.revision = 2;
        counter.amount = crate::ids::BaseUnits::new(terms.amount.get() + 500);
        assert_eq!(
            deal_nullifier(&terms).expect("derives"),
            deal_nullifier(&counter).expect("derives"),
        );
        assert_ne!(
            commit_agreement(&terms, &CommitmentBlinding::from_bytes([0x01; 32])).expect("commits"),
            commit_agreement(&counter, &CommitmentBlinding::from_bytes([0x01; 32]))
                .expect("commits"),
        );
    }

    #[test]
    fn different_domains_and_nonces_give_different_identities() {
        let terms = crate::terms::tests::example_terms();
        let baseline = deal_nullifier(&terms).expect("derives");

        let mut other_domain = terms.clone();
        other_domain.domain.verifier_version += 1;
        assert_ne!(deal_nullifier(&other_domain).expect("derives"), baseline);

        let mut other_nonce = terms.clone();
        other_nonce.settlement_nonce = [0x77; 32];
        assert_ne!(deal_nullifier(&other_nonce).expect("derives"), baseline);

        let mut other_buyer = terms.clone();
        other_buyer.buyer_authorization_key =
            crate::ids::KeyBytes::new(vec![0xaa; 20]).expect("valid key");
        assert_ne!(deal_nullifier(&other_buyer).expect("derives"), baseline);
    }

    #[test]
    fn blindings_do_not_print() {
        let debt = CommitmentBlinding::from_bytes([0x42; 32]);
        assert_eq!(format!("{debt:?}"), "CommitmentBlinding(<redacted>)");
    }
}
