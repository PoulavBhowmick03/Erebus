//! Role-specific authorizations over a deal commitment.
//!
//! Both participants authorize the same [`DealCommitment`] with their role's key. The signed
//! digest binds the role tag, the deployment domain, and the commitment, which is what makes
//! three attacks fail:
//!
//! - **Cross-domain replay**: a signature made for one deployment verifies under no other,
//!   because the domain is inside the digest.
//! - **Role substitution**: a seller signature does not verify as a buyer authorization,
//!   because the role tag is inside the digest and the key is looked up by role.
//! - **Terms substitution**: changed terms produce a different commitment, so no previous
//!   authorization covers them.
//!
//! Verification takes `now` as a caller-supplied timestamp; the core never reads a clock.
//! Rejection is at `now >= expiry`. See decisions D02 for what cancellation does and does
//! not promise: local cancellation stops local work only, and there is no onchain revocation
//! in the initial contract.

use crate::commitment::{commit_agreement, CommitmentBlinding, CommitmentError, DealCommitment};
use crate::domain::{DeploymentDomain, DomainError};
use crate::encoding::{EncodingError, Reader, Writer};
use crate::ids::{IdError, SignatureBytes, MAX_SIGNATURE_BYTES};
use crate::suite::{self, SuiteError};
use crate::terms::AgreementTerms;

/// Domain separation for authorization digests.
pub const AUTHORIZATION_DOMAIN: &[u8] = b"EREBUS_DEAL_AUTHORIZATION_V1";

/// Which participant an authorization belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// The party paying; its authorization covers the buyer key in the terms.
    Buyer,
    /// The party being paid; its authorization covers the seller key in the terms.
    Seller,
}

impl Role {
    /// Stable encoding tag.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::Buyer => 1,
            Self::Seller => 2,
        }
    }

    /// Parses a stable encoding tag.
    pub fn from_tag(tag: u8) -> Result<Self, EncodingError> {
        match tag {
            1 => Ok(Self::Buyer),
            2 => Ok(Self::Seller),
            _ => Err(EncodingError::UnknownTag("authorization_role", tag)),
        }
    }

    /// Human-readable name used in diagnostics.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Buyer => "buyer",
            Self::Seller => "seller",
        }
    }
}

/// One party's signature over the deal commitment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authorization {
    /// Which party signed.
    pub role: Role,
    /// The suite that verifies [`Self::signature`]; must equal the terms' suite.
    pub suite_id: u16,
    /// The commitment this signature covers.
    pub commitment: DealCommitment,
    /// Suite-encoded signature bytes.
    pub signature: SignatureBytes,
}

impl Authorization {
    /// Encodes the authorization canonically.
    pub fn encode(&self) -> Result<Vec<u8>, AuthError> {
        let mut writer = Writer::new();
        writer.u16(self.suite_id);
        writer.u8(self.role.tag());
        writer.fixed(self.commitment.as_bytes());
        writer.bytes(self.signature.as_bytes());
        Ok(writer.finish())
    }

    /// Decodes a complete authorization.
    pub fn decode(bytes: &[u8]) -> Result<Self, AuthError> {
        let mut reader = Reader::new(bytes);
        let suite_id = reader.u16("suite_id")?;
        let role = Role::from_tag(reader.u8("authorization_role")?)?;
        let commitment = DealCommitment::from_bytes(reader.fixed("commitment")?);
        let signature = SignatureBytes::from_validated(
            reader.bytes("signature", 1, MAX_SIGNATURE_BYTES)?.to_vec(),
        );
        reader.finish()?;
        Ok(Self {
            role,
            suite_id,
            commitment,
            signature,
        })
    }
}

/// Computes the digest a role signs for one commitment under one domain.
pub fn authorization_digest(
    domain: &DeploymentDomain,
    role: Role,
    commitment: &DealCommitment,
    suite_id: u16,
) -> Result<[u8; 32], AuthError> {
    let suite = suite::suite(suite_id)?;
    let domain = domain.encode()?;
    Ok(suite.hash(&[
        AUTHORIZATION_DOMAIN,
        &domain,
        &[role.tag()],
        commitment.as_bytes(),
    ]))
}

/// Checks the commitment opening and signature, without an expiry check.
///
/// This is the right entry point for disclosure and audit, where an expired authorization is
/// still evidence that it was issued. Settlement must use [`verify_authorization`].
pub fn verify_authorization_signature(
    terms: &AgreementTerms,
    commitment: &DealCommitment,
    blinding: &CommitmentBlinding,
    authorization: &Authorization,
) -> Result<(), AuthError> {
    if authorization.suite_id != terms.suite_id {
        return Err(AuthError::SuiteMismatch {
            terms: terms.suite_id,
            authorization: authorization.suite_id,
        });
    }
    if &authorization.commitment != commitment {
        return Err(AuthError::CommitmentMismatch);
    }
    if commit_agreement(terms, blinding)? != *commitment {
        return Err(AuthError::OpeningMismatch);
    }
    let key = match authorization.role {
        Role::Buyer => &terms.buyer_authorization_key,
        Role::Seller => &terms.seller_authorization_key,
    };
    let digest = authorization_digest(
        &terms.domain,
        authorization.role,
        commitment,
        terms.suite_id,
    )?;
    let suite = suite::suite(terms.suite_id)?;
    suite.verify_authorization(key.as_bytes(), &digest, authorization.signature.as_bytes())?;
    Ok(())
}

/// Checks the opening and signature at settlement time, then checks expiry.
///
/// A failed signature is reported even when the agreement is also expired, because the
/// signature failure is the more fundamental problem.
pub fn verify_authorization(
    terms: &AgreementTerms,
    commitment: &DealCommitment,
    blinding: &CommitmentBlinding,
    authorization: &Authorization,
    now: u64,
) -> Result<(), AuthError> {
    verify_authorization_signature(terms, commitment, blinding, authorization)?;
    if now >= terms.expiry {
        return Err(AuthError::Expired {
            expiry: terms.expiry,
            now,
        });
    }
    Ok(())
}

/// An authorization could not be encoded or verified.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    /// The authorization's suite is not the agreement's suite.
    #[error("authorization suite {authorization} does not match agreement suite {terms}")]
    SuiteMismatch {
        /// The terms' suite id.
        terms: u16,
        /// The authorization's suite id.
        authorization: u16,
    },
    /// The authorization covers a different commitment.
    #[error("authorization does not cover this agreement commitment")]
    CommitmentMismatch,
    /// Supplied terms and blinding do not open the signed commitment.
    #[error("agreement terms and blinding do not open the signed commitment")]
    OpeningMismatch,
    /// The supplied opening contains invalid terms.
    #[error(transparent)]
    Opening(#[from] CommitmentError),
    /// The agreement is past its authorization expiry.
    #[error("agreement expired at {expiry}; verification time is {now}")]
    Expired {
        /// The terms' expiry.
        expiry: u64,
        /// The caller-supplied verification time.
        now: u64,
    },
    /// The suite rejected the signature.
    #[error(transparent)]
    Suite(#[from] SuiteError),
    /// The deployment domain was invalid.
    #[error(transparent)]
    Domain(#[from] DomainError),
    /// A canonical encoding could not be read.
    #[error(transparent)]
    Encoding(#[from] EncodingError),
    /// An identifier was invalid.
    #[error(transparent)]
    Id(#[from] IdError),
}
