//! Prepaid metered access: profile, per-call request digest, and request authentication.
//!
//! Candidate profile, pinned by `tests/fixtures/metered-v1-vectors.json`. One finalized settlement buys N calls to one
//! seller-configured resource. This module holds only pure types and derivations; it keeps no
//! ledger, observes no chain, and serves no HTTP. A signed metered request authorizes consuming
//! at most one prepaid unit. It is not a payment, snapshot access, or disclosure authorization.

use std::fmt;

use erebus_core::{
    ids::KeyBytes,
    shielded_auth,
    suite::{self, SHIELDED_POSEIDON_EDDSA_SUITE_ID},
};
use erebus_transport::{
    disclosure::{verify_selected_agreement, SelectedAgreement, VerifiedAgreement},
    identity::AuthorizationIdentity,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{AccessError, CheckedAgreement, MAX_REQUEST_LIFETIME};

pub mod client;
pub mod http;
pub mod issuer;

pub use client::{
    CachedCall, Cursor, MeteredCallReceipt, MeteredClient, MeteredClientError, MeteredStatusReceipt,
};
pub use issuer::{
    AuthorizedRequest, CallSummary, DeploymentBinding, MeteredCall, MeteredError, MeteredIssuer,
    MeteredReply, MeteredStatus, PaymentCertifier, ResourceUnavailable, ResponseSource,
    METERED_VERIFIER_VERSION,
};

/// Byte prefix of the metered profile, whose SHA-256 is the signed fulfillment digest.
pub const METERED_PROFILE_TAG: &[u8] = b"EREBUS_HTTP_METERED_PROFILE_V1";
/// Keccak-256 domain tag of a metered call or status request.
pub const METERED_REQUEST_TAG: &[u8] = b"EREBUS_HTTP_METERED_REQUEST_V1";
/// Keccak-256 domain tag of an entitlement record id.
pub const METERED_ENTITLEMENT_TAG: &[u8] = b"EREBUS_METERED_ENTITLEMENT_V1";
/// Keccak-256 domain tag of a per-call record id.
pub const METERED_CALL_TAG: &[u8] = b"EREBUS_METERED_CALL_V1";
/// Keccak-256 domain tag of a seller policy fingerprint stored with an entitlement.
pub const METERED_POLICY_TAG: &[u8] = b"EREBUS_METERED_POLICY_V1";
/// Version encoded in the profile bytes.
pub const METERED_PROFILE_VERSION: u16 = 1;
/// Signed service unit of a metered agreement.
pub const METERED_UNIT: &str = "call";
/// Signed fulfillment method of a metered agreement.
pub const METERED_FULFILLMENT_METHOD: &str = "http-metered-v1";
/// Largest prepaid call count (provisional cap P5).
pub const MAX_METERED_QUANTITY: u128 = 4096;
/// Largest response a v1 profile may declare (provisional cap P6).
pub const MAX_METERED_RESPONSE_BYTES: u32 = 65536;
/// Request parameter digest in v1: calls take no buyer input, so it is all zero.
pub const PARAMS_DIGEST_V1: [u8; 32] = [0; 32];

/// How a call carries buyer input. Version 1 admits none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamsMode {
    /// The call takes no buyer input; the request's parameter digest is all zero.
    None,
}

impl ParamsMode {
    /// Wire byte in the profile encoding.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::None => 0,
        }
    }
}

/// Seller-configured metered profile. Only valid v1 profiles can be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeteredProfile {
    max_response_bytes: u32,
    params_mode: ParamsMode,
}

impl MeteredProfile {
    /// A v1 profile with no call parameters. The response cap must be in `1..=65536`.
    pub fn new(max_response_bytes: u32) -> Result<Self, AccessError> {
        if max_response_bytes == 0 || max_response_bytes > MAX_METERED_RESPONSE_BYTES {
            return Err(AccessError::Agreement);
        }
        Ok(Self {
            max_response_bytes,
            params_mode: ParamsMode::None,
        })
    }

    /// Largest response body the seller may return for one call.
    #[must_use]
    pub const fn max_response_bytes(&self) -> u32 {
        self.max_response_bytes
    }

    /// How a call carries buyer input.
    #[must_use]
    pub const fn params_mode(&self) -> ParamsMode {
        self.params_mode
    }

    /// Profile bytes: tag, `u16` version, `u32` response cap, `u8` parameter mode, big-endian.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(METERED_PROFILE_TAG.len() + 7);
        bytes.extend_from_slice(METERED_PROFILE_TAG);
        bytes.extend_from_slice(&METERED_PROFILE_VERSION.to_be_bytes());
        bytes.extend_from_slice(&self.max_response_bytes.to_be_bytes());
        bytes.push(self.params_mode.code());
        bytes
    }

    /// SHA-256 of the profile bytes, bound into the agreement's fulfillment digest.
    #[must_use]
    pub fn profile_digest(&self) -> [u8; 32] {
        Sha256::digest(self.encode()).into()
    }
}

/// What a metered request asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MeteredOperation {
    /// Consume, or replay, the call at `call_index`.
    Call,
    /// Report the allowance without consuming. Requires `call_index == 0`.
    Status,
}

impl MeteredOperation {
    /// Signed operation byte.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Call => 1,
            Self::Status => 2,
        }
    }
}

/// A short-lived, buyer-signed request against a prepaid metered agreement.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MeteredRequest {
    /// Exact paid agreement commitment, lowercase hexadecimal.
    pub deal_commitment: String,
    /// Call or status.
    pub operation: MeteredOperation,
    /// Zero-based call index; the idempotency key of a call. Zero for status.
    pub call_index: u64,
    /// Fresh nonzero nonce.
    pub nonce: [u8; 32],
    /// Unix request expiry, at most five minutes from verification time.
    pub expires_at: u64,
    /// Suite-specific buyer signature over [`metered_request_digest`].
    pub signature: Vec<u8>,
}

impl fmt::Debug for MeteredRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MeteredRequest { <redacted> }")
    }
}

/// Seller-controlled metered service and independently trusted service identity.
#[derive(Clone)]
pub struct MeteredPolicy {
    /// Hash-pinned service identity, obtained independently of a request or response.
    pub service_id: [u8; 32],
    /// Seller authorization key named in every accepted agreement.
    pub seller: KeyBytes,
    /// Agreement and request-signature suite.
    pub suite_id: u16,
    /// Exact signed service resource identifier.
    pub resource: String,
    /// Profile whose digest must equal the signed fulfillment digest.
    pub profile: MeteredProfile,
}

impl fmt::Debug for MeteredPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MeteredPolicy { <redacted> }")
    }
}

impl MeteredPolicy {
    fn validate(&self) -> Result<(), AccessError> {
        let suite = suite::suite(self.suite_id).map_err(|_| AccessError::Agreement)?;
        if self.service_id == [0; 32]
            || self.resource.is_empty()
            || self.resource.len() > 256
            || self.seller.as_bytes().len() != suite.authorization_key_length()
        {
            return Err(AccessError::Agreement);
        }
        Ok(())
    }

    /// Checks the agreement against this service. Snapshot terms never pass.
    pub fn check(&self, evidence: &SelectedAgreement) -> Result<VerifiedAgreement, AccessError> {
        self.validate()?;
        let terms = &evidence.terms;
        let service = &terms.service;
        if terms.suite_id != self.suite_id
            || terms.seller_authorization_key != self.seller
            || service.access_recipient != terms.buyer_authorization_key
            || service.resource != self.resource
            || !(1..=MAX_METERED_QUANTITY).contains(&service.quantity.get())
            || service.unit != METERED_UNIT
            || service.fulfillment_method != METERED_FULFILLMENT_METHOD
            || service.fulfillment_digest != self.profile.profile_digest()
        {
            return Err(AccessError::Agreement);
        }
        verify_selected_agreement(evidence).map_err(|_| AccessError::Agreement)
    }

    /// Checks the agreement against this metered profile and keeps it for payment checks.
    pub fn check_agreement(
        &self,
        evidence: SelectedAgreement,
    ) -> Result<CheckedAgreement, AccessError> {
        let agreement = self.check(&evidence)?;
        Ok(CheckedAgreement {
            evidence,
            agreement,
        })
    }

    /// Domain-separated hash of every policy field, stored with an entitlement so a changed
    /// policy is detected. Variable-length fields are length-prefixed.
    #[must_use]
    pub fn fingerprint(&self) -> [u8; 32] {
        let seller = self.seller.as_bytes();
        let resource = self.resource.as_bytes();
        let seller_len = u64::try_from(seller.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes();
        let resource_len = u64::try_from(resource.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes();
        suite::keccak256(&[
            METERED_POLICY_TAG,
            &self.service_id,
            &self.suite_id.to_be_bytes(),
            &seller_len,
            seller,
            &resource_len,
            resource,
            &self.profile.encode(),
        ])
    }

    /// Authenticates a metered request before any payment-observer RPC or ledger access.
    /// It does not compare `call_index` with the allowance: exhaustion is a ledger answer.
    pub fn authenticate(
        &self,
        evidence: &SelectedAgreement,
        request: &MeteredRequest,
        now: u64,
    ) -> Result<(), AccessError> {
        self.authenticate_checked(&self.check_agreement(evidence.clone())?, request, now)
    }

    /// [`Self::authenticate_checked`] without early exits: the request signature is verified
    /// whatever else fails, so a mismatched commitment costs what a bad signature costs.
    pub(crate) fn authenticate_uniformly(
        &self,
        checked: &CheckedAgreement,
        request: &MeteredRequest,
        now: u64,
    ) -> bool {
        let (evidence, agreement) = (checked.evidence(), checked.agreement());
        let signed = agreement_digest(
            evidence,
            agreement,
            self.service_id,
            request.operation,
            request.call_index,
            &PARAMS_DIGEST_V1,
            request.nonce,
            request.expires_at,
        )
        .and_then(|digest| {
            crate::access::verify_request_signature(
                evidence.terms.suite_id,
                evidence.terms.buyer_authorization_key.as_bytes(),
                &digest,
                &request.signature,
            )
        });
        signed.is_ok()
            && metered_request_shape_ok(request, now)
            && request.deal_commitment == agreement.commitment.to_hex()
    }

    /// [`Self::authenticate`] for an agreement this policy already checked, without verifying
    /// the agreement's participant signatures a second time. Freshness and the request
    /// signature are checked exactly as there.
    pub(crate) fn authenticate_checked(
        &self,
        checked: &CheckedAgreement,
        request: &MeteredRequest,
        now: u64,
    ) -> Result<(), AccessError> {
        let (evidence, agreement) = (checked.evidence(), checked.agreement());
        if request.deal_commitment != agreement.commitment.to_hex()
            || request.nonce == [0; 32]
            || request.expires_at <= now
            || request.expires_at.saturating_sub(now) > MAX_REQUEST_LIFETIME
        {
            return Err(AccessError::Authentication);
        }
        let digest = agreement_digest(
            evidence,
            agreement,
            self.service_id,
            request.operation,
            request.call_index,
            &PARAMS_DIGEST_V1,
            request.nonce,
            request.expires_at,
        )?;
        verify_buyer_signature(evidence, &digest, &request.signature)
    }
}

/// The signed fields of one request, in preimage order.
struct RequestBinding<'a> {
    domain: &'a [u8],
    service_id: &'a [u8; 32],
    commitment: &'a [u8; 32],
    nullifier: &'a [u8; 32],
    fulfillment_digest: &'a [u8; 32],
    operation: MeteredOperation,
    call_index: u64,
    params_digest: &'a [u8; 32],
    nonce: &'a [u8; 32],
    expires_at: u64,
}

impl RequestBinding<'_> {
    fn keccak(&self) -> [u8; 32] {
        suite::keccak256(&[
            METERED_REQUEST_TAG,
            self.domain,
            self.service_id,
            self.commitment,
            self.nullifier,
            self.fulfillment_digest,
            &[self.operation.code()],
            &self.call_index.to_be_bytes(),
            self.params_digest,
            self.nonce,
            &self.expires_at.to_be_bytes(),
        ])
    }
}

fn digest_with_params(
    evidence: &SelectedAgreement,
    service_id: [u8; 32],
    operation: MeteredOperation,
    call_index: u64,
    params_digest: &[u8; 32],
    nonce: [u8; 32],
    expires_at: u64,
) -> Result<[u8; 32], AccessError> {
    if operation == MeteredOperation::Status && call_index != 0 {
        return Err(AccessError::Authentication);
    }
    let agreement = verify_selected_agreement(evidence).map_err(|_| AccessError::Agreement)?;
    agreement_digest(
        evidence,
        &agreement,
        service_id,
        operation,
        call_index,
        params_digest,
        nonce,
        expires_at,
    )
}

/// The request digest over an agreement whose opening and authorizations were verified into
/// `agreement`. The caller must have obtained `agreement` from this `evidence`.
#[allow(clippy::too_many_arguments)]
fn agreement_digest(
    evidence: &SelectedAgreement,
    agreement: &VerifiedAgreement,
    service_id: [u8; 32],
    operation: MeteredOperation,
    call_index: u64,
    params_digest: &[u8; 32],
    nonce: [u8; 32],
    expires_at: u64,
) -> Result<[u8; 32], AccessError> {
    if operation == MeteredOperation::Status && call_index != 0 {
        return Err(AccessError::Authentication);
    }
    let domain = evidence
        .terms
        .domain
        .encode()
        .map_err(|_| AccessError::Agreement)?;
    let digest = RequestBinding {
        domain: &domain,
        service_id: &service_id,
        commitment: agreement.commitment.as_bytes(),
        nullifier: agreement.nullifier.as_bytes(),
        fulfillment_digest: &evidence.terms.service.fulfillment_digest,
        operation,
        call_index,
        params_digest,
        nonce: &nonce,
        expires_at,
    }
    .keccak();
    if evidence.terms.suite_id == SHIELDED_POSEIDON_EDDSA_SUITE_ID {
        erebus_core::shielded::metered_access_message(&digest)
            .map_err(|_| AccessError::Authentication)
    } else {
        Ok(digest)
    }
}

fn verify_buyer_signature(
    evidence: &SelectedAgreement,
    digest: &[u8; 32],
    signature: &[u8],
) -> Result<(), AccessError> {
    suite::suite(evidence.terms.suite_id)
        .map_err(|_| AccessError::Authentication)?
        .verify_authorization(
            evidence.terms.buyer_authorization_key.as_bytes(),
            digest,
            signature,
        )
        .map_err(|_| AccessError::Authentication)
}

/// Request fields a route checks before any evidence lookup: [`crate::access::request_shape_ok`],
/// and a status request names call index zero.
#[must_use]
pub fn metered_request_shape_ok(request: &MeteredRequest, now: u64) -> bool {
    crate::access::request_shape_ok(&request.nonce, request.expires_at, now)
        && (request.operation != MeteredOperation::Status || request.call_index == 0)
}

/// Computes the domain-separated metered request digest, not a payment authorization.
/// Suite 1 signs the Keccak-256 digest; suite 2 signs its tag-3003 Poseidon message.
/// The caller must trust `service_id` independently. Status requires `call_index == 0`.
pub fn metered_request_digest(
    evidence: &SelectedAgreement,
    service_id: [u8; 32],
    operation: MeteredOperation,
    call_index: u64,
    nonce: [u8; 32],
    expires_at: u64,
) -> Result<[u8; 32], AccessError> {
    digest_with_params(
        evidence,
        service_id,
        operation,
        call_index,
        &PARAMS_DIGEST_V1,
        nonce,
        expires_at,
    )
}

/// Signs a metered request with the buyer's existing agreement key seed.
/// Fails unless the seed derives the agreement's buyer authorization key.
pub fn sign_metered_request(
    evidence: &SelectedAgreement,
    service_id: [u8; 32],
    operation: MeteredOperation,
    call_index: u64,
    nonce: [u8; 32],
    expires_at: u64,
    seed: &[u8; 32],
) -> Result<MeteredRequest, AccessError> {
    if nonce == [0; 32] {
        return Err(AccessError::Authentication);
    }
    let agreement = verify_selected_agreement(evidence).map_err(|_| AccessError::Agreement)?;
    let digest = metered_request_digest(
        evidence, service_id, operation, call_index, nonce, expires_at,
    )?;
    let (buyer, signature) = if evidence.terms.suite_id == SHIELDED_POSEIDON_EDDSA_SUITE_ID {
        let (key, signature) =
            shielded_auth::sign_message(seed, &digest).map_err(|_| AccessError::Authentication)?;
        (key.to_vec(), signature.to_vec())
    } else {
        let identity =
            AuthorizationIdentity::from_bytes(seed).map_err(|_| AccessError::Authentication)?;
        (
            identity.address().to_vec(),
            identity.sign_digest(&digest).to_vec(),
        )
    };
    if buyer != evidence.terms.buyer_authorization_key.as_bytes() {
        return Err(AccessError::Authentication);
    }
    Ok(MeteredRequest {
        deal_commitment: agreement.commitment.to_hex(),
        operation,
        call_index,
        nonce,
        expires_at,
        signature,
    })
}

/// Verifies the request signature against the agreement's buyer authorization key.
/// This checks only the signature; [`MeteredPolicy::authenticate`] adds terms and freshness.
pub fn verify_metered_request(
    evidence: &SelectedAgreement,
    service_id: [u8; 32],
    request: &MeteredRequest,
) -> Result<(), AccessError> {
    let digest = metered_request_digest(
        evidence,
        service_id,
        request.operation,
        request.call_index,
        request.nonce,
        request.expires_at,
    )?;
    verify_buyer_signature(evidence, &digest, &request.signature)
}

/// Entitlement record id `E`: one per agreement at an independently configured service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntitlementId([u8; 32]);

impl EntitlementId {
    /// Raw id bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Lowercase hexadecimal form.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

/// Call record id `C(k)`: the only consumption authority for call index `k`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallId([u8; 32]);

impl CallId {
    /// Raw id bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Lowercase hexadecimal form.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

/// `E = keccak256("EREBUS_METERED_ENTITLEMENT_V1" || service_id || commitment)`.
#[must_use]
pub fn entitlement_id(service_id: &[u8; 32], commitment: &[u8; 32]) -> EntitlementId {
    EntitlementId(suite::keccak256(&[
        METERED_ENTITLEMENT_TAG,
        service_id,
        commitment,
    ]))
}

/// `C(k) = keccak256("EREBUS_METERED_CALL_V1" || E || u64(k))`.
#[must_use]
pub fn call_id(entitlement: &EntitlementId, index: u64) -> CallId {
    CallId(suite::keccak256(&[
        METERED_CALL_TAG,
        entitlement.as_bytes(),
        &index.to_be_bytes(),
    ]))
}

#[cfg(test)]
mod tests;
