//! Finalized-payment-gated access to one immutable resource snapshot.
//!
//! The first access profile uses the existing buyer agreement key, not a delegated key.
//! Requests require TLS: a stolen signed request can be replayed until its short expiry.
//! Repeating a request returns the same durable issuance. Nothing here signs or sends payments.
//! Access issuance is a seller assertion, not an independent proof of delivery.

use std::{fmt, path::PathBuf, sync::Arc};

use erebus_core::{
    commitment::DealNullifier,
    deal_state::DealEvidence,
    ids::KeyBytes,
    suite::{self, SHIELDED_POSEIDON_EDDSA_SUITE_ID},
    terms::SettlementMode,
};
use erebus_evm::chain::{EvmChain, HistoricalObservation, ObservationJournal, ObservationLimits};
use erebus_evm::x402::{
    validate_permit_fields, verify_permit_signature, verify_x402_exact, DealPermit,
    X402VerificationError,
};
use erebus_journal::{FaultHook, JournalRecord, NoFaults, RecordId, Store};
use erebus_transport::disclosure::{
    verify_selected_agreement, SelectedAgreement, VerifiedAgreement,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    disclosure::verify_shielded_payment_certified,
    index_store::IndexStore,
    rpc::{PoolRpc, PoolRuntimePins},
};

pub mod client;
pub mod metered;
pub mod x402;

/// Longest resource response supported by this snapshot profile.
pub const MAX_RESOURCE_BYTES: usize = 1024 * 1024;
/// Longest lifetime of one signed retrieval request.
pub const MAX_REQUEST_LIFETIME: u64 = 300;
/// Keccak-256 domain tag of a snapshot retrieval request.
pub const ACCESS_REQUEST_TAG: &[u8] = b"EREBUS_HTTP_ACCESS_REQUEST_V1";
/// Keccak-256 domain tag of a snapshot retrieval request carrying an x402 authorization.
pub const ACCESS_REQUEST_X402_TAG: &[u8] = b"EREBUS_HTTP_ACCESS_REQUEST_X402_V1";
/// Keccak-256 domain tag of a snapshot issuance identity.
pub const ACCESS_ISSUANCE_TAG: &[u8] = b"EREBUS_ACCESS_ISSUANCE_V1";

/// A short-lived proof of possession of the buyer's agreement key.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccessRequest {
    /// Exact paid agreement commitment, lowercase hexadecimal.
    pub deal_commitment: String,
    /// Fresh nonzero nonce. Retries can reuse it or sign a fresh nonce.
    pub nonce: [u8; 32],
    /// Unix request expiry, at most five minutes from verification time.
    pub expires_at: u64,
    /// Suite-specific buyer signature over [`request_digest`].
    pub signature: Vec<u8>,
    /// Optional x402 exact payment authorization, covered by the request signature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment: Option<X402Payment>,
}

/// One buyer-signed x402 exact authorization bound to the agreement's deal nullifier.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct X402Payment {
    /// Token the agreement is denominated in.
    pub token: [u8; 20],
    /// Exact agreed amount in base units.
    pub amount: u128,
    /// Permit2 deadline, unix seconds.
    pub deadline: u64,
    /// Recipient fixed by the permit witness.
    pub to: [u8; 20],
    /// Earliest settlement time, unix seconds.
    pub valid_after: u64,
    /// 65-byte `r || s || v` permit signature by the buyer agreement key.
    pub signature: Vec<u8>,
}

impl fmt::Debug for X402Payment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("X402Payment { <redacted> }")
    }
}

impl X402Payment {
    /// Canonical bytes covered by the retrieval signature.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(72 + self.signature.len());
        bytes.extend_from_slice(&self.token);
        bytes.extend_from_slice(&self.amount.to_be_bytes());
        bytes.extend_from_slice(&self.deadline.to_be_bytes());
        bytes.extend_from_slice(&self.to);
        bytes.extend_from_slice(&self.valid_after.to_be_bytes());
        bytes.extend_from_slice(&self.signature);
        bytes
    }

    /// The permit this authorization signs for `deal`.
    #[must_use]
    pub fn permit(&self, deal: DealNullifier) -> DealPermit {
        DealPermit {
            token: self.token,
            amount: self.amount,
            deal,
            deadline: self.deadline,
            to: self.to,
            valid_after: self.valid_after,
        }
    }

    /// The 65-byte signature, rejecting any other length.
    pub fn signature(&self) -> Result<[u8; 65], AccessError> {
        self.signature
            .as_slice()
            .try_into()
            .map_err(|_| AccessError::Authentication)
    }
}

impl fmt::Debug for AccessRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AccessRequest { <redacted> }")
    }
}

/// Seller-controlled immutable snapshot and independently trusted service identity.
#[derive(Clone)]
pub struct AccessPolicy {
    /// Hash-pinned service identity, obtained independently of a request or response.
    pub service_id: [u8; 32],
    /// Seller authorization key named in every accepted agreement.
    pub seller: KeyBytes,
    /// Agreement and retrieval-signature suite.
    pub suite_id: u16,
    /// Exact signed service resource identifier.
    pub resource: String,
    /// Immutable payload. Its SHA-256 must equal the signed fulfillment_digest.
    pub payload: Vec<u8>,
}

impl fmt::Debug for AccessPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AccessPolicy { <redacted> }")
    }
}

/// Configured observers, never supplied by an HTTP caller.
pub enum AccessBackend {
    /// Public payment on a trusted EVM deployment, with durable bounded history.
    PublicBound {
        /// Independently configured chain observer.
        chain: EvmChain,
        /// Persistent public history cache.
        journal: ObservationJournal,
        /// Deployment block or an earlier block; must not omit a possible settlement.
        from_block: u64,
        /// Bounded query budget.
        limits: ObservationLimits,
    },
    /// x402 exact payment settled through the canonical Permit2 proxy on a trusted deployment.
    X402Exact {
        /// Independently configured chain observer for the settlement deployment.
        chain: EvmChain,
    },
    /// Shielded payment checked by the existing paired pool observer.
    Shielded {
        /// First configured RPC.
        rpc: Box<PoolRpc>,
        /// First public history cache.
        index: IndexStore,
        /// Independent second configured RPC.
        peer_rpc: Box<PoolRpc>,
        /// Distinct second public history cache.
        peer_index: IndexStore,
        /// Reviewed pool and dependency runtime pins both observers must run. Boxed to keep
        /// this startup-configured enum from carrying the largest inline variant.
        runtime: Box<PoolRuntimePins>,
    },
}

/// An agreement already checked against one access profile's terms.
///
/// Only a profile check in this module constructs it, so a backend that accepts one never
/// re-derives which profile it was checked for. It says nothing about payment.
pub struct CheckedAgreement {
    evidence: SelectedAgreement,
    agreement: VerifiedAgreement,
}

impl CheckedAgreement {
    /// The selected agreement evidence that passed the profile check.
    pub fn evidence(&self) -> &SelectedAgreement {
        &self.evidence
    }

    /// Facts established by transcript replay and both participant authorizations.
    pub fn agreement(&self) -> &VerifiedAgreement {
        &self.agreement
    }
}

impl fmt::Debug for CheckedAgreement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CheckedAgreement { <redacted> }")
    }
}

/// Agreement checked against matching finalized payment. Only the observers can construct it.
pub struct PaidAccess {
    evidence: SelectedAgreement,
    agreement: VerifiedAgreement,
}

#[cfg(test)]
mod tests;

impl fmt::Debug for PaidAccess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PaidAccess { <redacted> }")
    }
}

/// Access can fail while payment remains final. Never recharge on these errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AccessError {
    /// The service record does not match the configured seller, resource, or buyer key.
    #[error("agreement does not match this access service")]
    Agreement,
    /// Buyer request is expired, malformed, or not signed by its agreement key.
    #[error("access request is not authenticated")]
    Authentication,
    /// History remains pending or no finalized matching payment exists yet.
    #[error("payment verification is pending; retain state and retry without another payment")]
    Pending,
    /// RPC failed. This is not evidence of non-payment.
    #[error("payment observation is unavailable; retry without another payment")]
    Observation,
    /// Durable issuance storage failed or was corrupt.
    #[error("access storage unavailable; payment may be final, retry delivery without recharging")]
    Storage,
}

/// Delivery metadata. A client must hash the returned bytes itself to verify content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Issuance {
    /// Stable issuance identity shared by all retries of this agreement at this service.
    pub issuance_id: String,
    /// SHA-256 of the expected immutable payload.
    pub resource_sha256: String,
    /// Original durable issuance time; never reset by retries.
    pub issued_at: u64,
    /// Issuance happened after the signed delivery deadline; payment is not voided.
    pub late: bool,
    /// A previously persisted issuance was reused.
    pub repeated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct IssuanceId(String);

impl fmt::Display for IssuanceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl RecordId for IssuanceId {
    fn as_file_stem(&self) -> &str {
        &self.0
    }
    fn from_file_stem(stem: &str) -> Option<Self> {
        (stem.len() == 64
            && stem
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
        .then(|| Self(stem.into()))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Issued {
    version: u32,
    id: IssuanceId,
    commitment: String,
    nullifier: String,
    service_id: [u8; 32],
    resource_sha256: [u8; 32],
    buyer: Vec<u8>,
    issued_at: u64,
    delivery_deadline: u64,
}

impl JournalRecord for Issued {
    type Id = IssuanceId;
    const CURRENT_VERSION: u32 = 1;
    const OLDEST_READABLE_VERSION: u32 = 1;
    fn version(&self) -> u32 {
        self.version
    }
    fn record_id(&self) -> &Self::Id {
        &self.id
    }
    fn attempt_count(&self) -> usize {
        1
    }
}

/// Domain tag of the public seed behind [`MissDecoy`]. Not key material of any participant.
const MISS_DECOY_TAG: &[u8] = b"EREBUS_ACCESS_MISS_DECOY_V1";

/// Work spent on a request whose commitment names no usable evidence.
///
/// A route must not be a commitment oracle: a caller who does not hold a buyer key must not
/// learn whether some other buyer's agreement commitment exists at this service. Every
/// pre-authentication failure therefore gets the same answer, and the miss path spends
/// comparable signature work. The real path verifies the two agreement authorizations and then
/// the request signature; the decoy verifies two precomputed valid authorizations under a
/// fixed public key, then the caller's request signature against that key.
///
/// The decoy key is derived from a published constant. It authorizes nothing: its result is
/// always discarded and the miss path always answers [`AccessError::Authentication`].
/// Residual difference: a hit also reads and decodes the evidence file, replays its transcript,
/// and (snapshot) hashes the payload, so a caller timing a loopback service closely can still
/// tell the paths apart. Closing that would require authenticating before the lookup.
pub struct MissDecoy {
    suite_id: u16,
    key: Vec<u8>,
    digests: [[u8; 32]; 3],
    authorizations: [Vec<u8>; 2],
}

impl fmt::Debug for MissDecoy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MissDecoy")
            .field("suite_id", &self.suite_id)
            .finish_non_exhaustive()
    }
}

impl MissDecoy {
    /// A decoy for one agreement suite, checked to spend full verification work.
    pub fn new(suite_id: u16) -> Result<Self, AccessError> {
        let seed = suite::keccak256(&[MISS_DECOY_TAG, &suite_id.to_be_bytes()]);
        let digests = [0u8, 1, 2].map(|index| {
            let mut digest = suite::keccak256(&[MISS_DECOY_TAG, &seed, &[index]]);
            // Inside the BN254 scalar field, so the suite-2 helper parses it as a message.
            digest[0] &= 0x0f;
            digest
        });
        let sign = |digest: &[u8; 32]| -> Result<(Vec<u8>, Vec<u8>), AccessError> {
            match suite_id {
                suite::EVM_SECP256K1_KECCAK_SUITE_ID => {
                    let identity =
                        erebus_transport::identity::AuthorizationIdentity::from_bytes(&seed)
                            .map_err(|_| AccessError::Agreement)?;
                    Ok((
                        identity.address().to_vec(),
                        identity.sign_digest(digest).to_vec(),
                    ))
                }
                SHIELDED_POSEIDON_EDDSA_SUITE_ID => {
                    let (key, signature) = erebus_core::shielded_auth::sign_message(&seed, digest)
                        .map_err(|_| AccessError::Agreement)?;
                    Ok((key.to_vec(), signature.to_vec()))
                }
                _ => Err(AccessError::Agreement),
            }
        };
        let (key, first) = sign(&digests[0])?;
        let (_, second) = sign(&digests[1])?;
        let decoy = Self {
            suite_id,
            key,
            digests,
            authorizations: [first, second],
        };
        // A decoy whose signatures failed fast would not spend the real path's work.
        let verifier = suite::suite(suite_id).map_err(|_| AccessError::Agreement)?;
        for (digest, signature) in decoy.digests.iter().zip(&decoy.authorizations) {
            verifier
                .verify_authorization(&decoy.key, digest, signature)
                .map_err(|_| AccessError::Agreement)?;
        }
        Ok(decoy)
    }

    /// Spends the miss path's work for one request signature and returns the single
    /// pre-authentication answer, [`AccessError::Authentication`].
    pub fn reject(&self, request_signature: &[u8]) -> AccessError {
        if let Ok(verifier) = suite::suite(self.suite_id) {
            for (digest, signature) in self.digests.iter().zip(&self.authorizations) {
                let _ = std::hint::black_box(verifier.verify_authorization(
                    &self.key,
                    std::hint::black_box(digest),
                    signature,
                ));
            }
        }
        let _ = std::hint::black_box(verify_request_signature(
            self.suite_id,
            &self.key,
            &self.digests[2],
            request_signature,
        ));
        AccessError::Authentication
    }
}

/// Most requests one authenticated buyer may have in flight at one service.
pub const MAX_BUYER_IN_FLIGHT: u32 = 2;
/// Requests one buyer may start in a burst before the refill rate applies.
pub const BUYER_BURST: u32 = 6;
/// Requests per second one buyer regains, up to [`BUYER_BURST`].
pub const BUYER_REFILL_PER_SECOND: u32 = 1;
/// Most buyers tracked at once; beyond it, idle full buckets are forgotten first.
const MAX_TRACKED_BUYERS: usize = 4096;

#[derive(Debug)]
struct BuyerBucket {
    in_flight: u32,
    tokens: u32,
    refilled: std::time::Instant,
}

/// Per-buyer admission, applied after authentication and before any payment-observer RPC.
///
/// It is keyed by the authenticated buyer agreement key, so one buyer cannot spend another's
/// share of the global request slots, and it runs on top of the service's global limits. A
/// refusal is "busy": retry later, never pay again. Only an authenticated buyer reaches it,
/// so it reveals nothing to a caller without a buyer key.
#[derive(Debug, Default)]
pub struct BuyerLimiter {
    buckets: std::sync::Mutex<std::collections::HashMap<Vec<u8>, BuyerBucket>>,
}

/// One admitted request; dropping it frees the buyer's in-flight slot.
#[derive(Debug)]
pub struct BuyerSlot {
    limiter: Arc<BuyerLimiter>,
    buyer: Vec<u8>,
}

impl Drop for BuyerSlot {
    fn drop(&mut self) {
        if let Ok(mut buckets) = self.limiter.buckets.lock() {
            if let Some(bucket) = buckets.get_mut(&self.buyer) {
                bucket.in_flight = bucket.in_flight.saturating_sub(1);
            }
        }
    }
}

impl BuyerLimiter {
    /// Admits one request for `buyer` at `now`, or refuses it as busy.
    pub fn admit(self: &Arc<Self>, buyer: &[u8], now: std::time::Instant) -> Option<BuyerSlot> {
        let mut buckets = self.buckets.lock().ok()?;
        if !buckets.contains_key(buyer) && buckets.len() >= MAX_TRACKED_BUYERS {
            let refill = |bucket: &BuyerBucket| {
                now.saturating_duration_since(bucket.refilled).as_secs()
                    * u64::from(BUYER_REFILL_PER_SECOND)
                    + u64::from(bucket.tokens)
                    >= u64::from(BUYER_BURST)
            };
            buckets.retain(|_, bucket| bucket.in_flight > 0 || !refill(bucket));
            if buckets.len() >= MAX_TRACKED_BUYERS {
                return None;
            }
        }
        let bucket = buckets.entry(buyer.to_vec()).or_insert(BuyerBucket {
            in_flight: 0,
            tokens: BUYER_BURST,
            refilled: now,
        });
        let elapsed = now.saturating_duration_since(bucket.refilled).as_secs();
        if elapsed > 0 {
            let regained = elapsed.saturating_mul(u64::from(BUYER_REFILL_PER_SECOND));
            bucket.tokens = u64::from(bucket.tokens)
                .saturating_add(regained)
                .min(u64::from(BUYER_BURST)) as u32;
            bucket.refilled = now;
        }
        if bucket.in_flight >= MAX_BUYER_IN_FLIGHT || bucket.tokens == 0 {
            return None;
        }
        bucket.in_flight += 1;
        bucket.tokens -= 1;
        Some(BuyerSlot {
            limiter: Arc::clone(self),
            buyer: buyer.to_vec(),
        })
    }
}

/// Reads `<evidence_root>/<deal_commitment>.evidence`: lowercase hex name, regular owner-only
/// file, bounded, decodable. Any failure is `None`, so callers cannot tell the causes apart.
pub fn read_evidence(
    evidence_root: &std::path::Path,
    deal_commitment: &str,
) -> Option<SelectedAgreement> {
    use std::io::Read;
    if deal_commitment.len() != 64
        || !deal_commitment
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let limit = erebus_transport::disclosure::MAX_DISCLOSURE_BYTES;
    let path = evidence_root.join(format!("{deal_commitment}.evidence"));
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return None;
        }
    }
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
    std::fs::File::open(&path)
        .ok()?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > limit {
        return None;
    }
    SelectedAgreement::decode(&bytes).ok()
}

/// A durable access issuer. The caller must control its directory and parent directories.
pub struct AccessIssuer {
    policy: AccessPolicy,
    store: Store<Issued>,
    decoy: MissDecoy,
}

impl AccessPolicy {
    /// Hash bound into the canonical agreement's fulfillment_digest.
    pub fn resource_hash(&self) -> [u8; 32] {
        Sha256::digest(&self.payload).into()
    }

    fn validate(&self) -> Result<(), AccessError> {
        let suite = suite::suite(self.suite_id).map_err(|_| AccessError::Agreement)?;
        if self.service_id == [0; 32]
            || self.payload.is_empty()
            || self.payload.len() > MAX_RESOURCE_BYTES
            || self.resource.is_empty()
            || self.resource.len() > 256
            || self.seller.as_bytes().len() != suite.authorization_key_length()
        {
            return Err(AccessError::Agreement);
        }
        Ok(())
    }

    fn check(&self, evidence: &SelectedAgreement) -> Result<VerifiedAgreement, AccessError> {
        self.validate()?;
        let terms = &evidence.terms;
        let service = &terms.service;
        if terms.suite_id != self.suite_id
            || terms.seller_authorization_key != self.seller
            || service.access_recipient != terms.buyer_authorization_key
            || service.resource != self.resource
            || service.quantity.get() != 1
            || service.unit != "snapshot"
            || service.fulfillment_method != "http-access-v1"
            || service.fulfillment_digest != self.resource_hash()
        {
            return Err(AccessError::Agreement);
        }
        verify_selected_agreement(evidence).map_err(|_| AccessError::Agreement)
    }

    /// Checks the agreement against this snapshot profile and keeps it for payment checks.
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
}

/// Maps a shielded verification failure to an access answer. Never an unpaid verdict.
fn shielded_access_error(error: crate::disclosure::ShieldedDisclosureError) -> AccessError {
    match error {
        crate::disclosure::ShieldedDisclosureError::Payment => AccessError::Pending,
        crate::disclosure::ShieldedDisclosureError::Observation(
            crate::observation::ObservationError::Recovery(
                crate::recovery::RecoveryError::HistoryPending { .. },
            ),
        )
        | crate::disclosure::ShieldedDisclosureError::Observation(
            crate::observation::ObservationError::FinalityPending,
        ) => AccessError::Pending,
        _ => AccessError::Observation,
    }
}

/// Computes a domain-separated retrieval digest, not a payment authorization.
/// The caller must trust service_id independently and sign using its existing buyer key.
pub fn request_digest(
    evidence: &SelectedAgreement,
    service_id: [u8; 32],
    nonce: [u8; 32],
    expires_at: u64,
    payment: Option<&X402Payment>,
) -> Result<[u8; 32], AccessError> {
    let agreement = verify_selected_agreement(evidence).map_err(|_| AccessError::Agreement)?;
    request_digest_for(evidence, &agreement, service_id, nonce, expires_at, payment)
}

/// [`request_digest`] for an agreement already verified, without replaying it a second time.
fn request_digest_for(
    evidence: &SelectedAgreement,
    agreement: &VerifiedAgreement,
    service_id: [u8; 32],
    nonce: [u8; 32],
    expires_at: u64,
    payment: Option<&X402Payment>,
) -> Result<[u8; 32], AccessError> {
    let domain = evidence
        .terms
        .domain
        .encode()
        .map_err(|_| AccessError::Agreement)?;
    let expiry = expires_at.to_be_bytes();
    let encoded = payment.map(X402Payment::encode);
    let mut preimage: Vec<&[u8]> = vec![
        ACCESS_REQUEST_TAG,
        &domain,
        &service_id,
        agreement.commitment.as_bytes(),
        agreement.nullifier.as_bytes(),
        &evidence.terms.service.fulfillment_digest,
        &nonce,
        &expiry,
    ];
    if let Some(encoded) = &encoded {
        preimage[0] = ACCESS_REQUEST_X402_TAG;
        preimage.push(encoded);
    }
    let digest = suite::keccak256(&preimage);
    if evidence.terms.suite_id == SHIELDED_POSEIDON_EDDSA_SUITE_ID {
        erebus_core::shielded::access_message(&digest).map_err(|_| AccessError::Authentication)
    } else {
        Ok(digest)
    }
}

/// Request fields a route checks before it looks up any evidence: a nonzero nonce and an
/// expiry within the next [`MAX_REQUEST_LIFETIME`] seconds. Failing them never touches the
/// evidence directory, so their answer and cost cannot depend on whether a commitment exists.
#[must_use]
pub fn request_shape_ok(nonce: &[u8; 32], expires_at: u64, now: u64) -> bool {
    *nonce != [0; 32] && expires_at > now && expires_at.saturating_sub(now) <= MAX_REQUEST_LIFETIME
}

#[cfg(test)]
thread_local! {
    /// Request-signature verifications run on this thread, real or decoy.
    pub(crate) static REQUEST_VERIFICATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Verifies one request signature; the one place either path does, so tests can count them.
pub(crate) fn verify_request_signature(
    suite_id: u16,
    key: &[u8],
    digest: &[u8; 32],
    signature: &[u8],
) -> Result<(), AccessError> {
    #[cfg(test)]
    REQUEST_VERIFICATIONS.with(|count| count.set(count.get() + 1));
    suite::suite(suite_id)
        .map_err(|_| AccessError::Authentication)?
        .verify_authorization(key, digest, signature)
        .map_err(|_| AccessError::Authentication)
}

/// Stable identity for one agreement's issuance at an independently configured service.
pub fn issuance_id(service_id: &[u8; 32], commitment: &[u8; 32]) -> String {
    hex::encode(suite::keccak256(&[
        ACCESS_ISSUANCE_TAG,
        service_id,
        commitment,
    ]))
}

impl AccessBackend {
    /// Checks private agreed terms against configured, independent final chain evidence.
    /// Pending and unavailable observations never produce a PaidAccess value.
    pub async fn verify(
        &self,
        policy: &AccessPolicy,
        evidence: SelectedAgreement,
    ) -> Result<PaidAccess, AccessError> {
        self.verify_checked(policy.check_agreement(evidence)?).await
    }

    /// Checks an agreement that already passed a profile check against configured, independent
    /// final chain evidence. It does not re-check profile terms; the profile's issuer must.
    /// Pending and unavailable observations never produce a PaidAccess value.
    pub async fn verify_checked(
        &self,
        checked: CheckedAgreement,
    ) -> Result<PaidAccess, AccessError> {
        let CheckedAgreement {
            evidence,
            agreement,
        } = checked;
        let reads = match self {
            Self::PublicBound {
                chain,
                journal,
                from_block,
                limits,
            } => {
                if evidence.terms.settlement_mode != SettlementMode::PublicBound {
                    return Err(AccessError::Agreement);
                }
                chain
                    .deployment()
                    .matches_domain(&evidence.terms.domain)
                    .map_err(|_| AccessError::Agreement)?;
                match chain
                    .finalized_deal_evidence_resumable_from(
                        journal,
                        &agreement.nullifier,
                        *from_block,
                        *limits,
                    )
                    .await
                    .map_err(|_| AccessError::Observation)?
                {
                    HistoricalObservation::Pending { .. } => return Err(AccessError::Pending),
                    HistoricalObservation::Complete {
                        evidence: DealEvidence::Observed(reads),
                        ..
                    } => reads,
                    _ => return Err(AccessError::Pending),
                }
            }
            Self::X402Exact { .. } => return Err(AccessError::Agreement),
            Self::Shielded {
                rpc,
                index,
                peer_rpc,
                peer_index,
                runtime,
            } => {
                // Pool code is immutable, so checking the deployment origin is as strong as
                // checking any later anchor, and it costs no observation scan.
                let origin = index.domain().first_hash;
                rpc.authenticate_runtime_at(origin, **runtime)
                    .await
                    .map_err(|_| AccessError::Observation)?;
                peer_rpc
                    .authenticate_runtime_at(origin, **runtime)
                    .await
                    .map_err(|_| AccessError::Observation)?;
                verify_shielded_payment_certified(
                    evidence.clone(),
                    rpc,
                    index,
                    peer_rpc,
                    peer_index,
                    **runtime,
                )
                .await
                .map_err(shielded_access_error)?
                .payment
                .settlement
            }
        };
        let Some(winner) = &reads.winner else {
            return Err(AccessError::Pending);
        };
        if reads.deal_nullifier != agreement.nullifier
            || !reads.consumed_at_final
            || !reads.consumed_at_head
            || !winner.is_final
            || winner.commitment != agreement.commitment
            || winner.amount != evidence.terms.amount
            || winner.fee != evidence.terms.fee_policy.fee
        {
            return Err(AccessError::Pending);
        }
        Ok(PaidAccess {
            evidence,
            agreement,
        })
    }

    /// Verifies an x402 exact payment against the configured deployment's finalized state.
    ///
    /// The transaction hash is only a lookup key: the target, calldata, token transfer, and
    /// Permit2 nonce bit are read from the configured observer at a pinned finalized anchor.
    /// Every error here retains state and never authorizes another payment.
    pub async fn verify_x402(
        &self,
        policy: &AccessPolicy,
        evidence: SelectedAgreement,
        permit: &DealPermit,
        signature: &[u8; 65],
        transaction_hash: [u8; 32],
    ) -> Result<PaidAccess, AccessError> {
        let agreement = policy.check(&evidence)?;
        let Self::X402Exact { chain } = self else {
            return Err(AccessError::Agreement);
        };
        if evidence.terms.settlement_mode != SettlementMode::PublicBound {
            return Err(AccessError::Agreement);
        }
        chain
            .deployment()
            .matches_domain(&evidence.terms.domain)
            .map_err(|_| AccessError::Agreement)?;
        if permit.deal != agreement.nullifier {
            return Err(AccessError::Authentication);
        }
        validate_permit_fields(chain.deployment(), &evidence.terms, permit)
            .map_err(|_| AccessError::Authentication)?;
        verify_permit_signature(chain.deployment(), &evidence.terms, permit, signature)
            .map_err(|_| AccessError::Authentication)?;
        let owner: [u8; 20] = evidence
            .terms
            .buyer_authorization_key
            .as_bytes()
            .try_into()
            .map_err(|_| AccessError::Authentication)?;
        let observed = chain
            .finalized_x402_evidence(transaction_hash, owner, &agreement.nullifier)
            .await
            .map_err(|_| AccessError::Observation)?;
        let Some(observed) = observed else {
            return Err(AccessError::Pending);
        };
        verify_x402_exact(
            chain.deployment(),
            &evidence.terms,
            permit,
            signature,
            &observed,
        )
        .map_err(|error| match error {
            X402VerificationError::Terms(_)
            | X402VerificationError::Commitment(_)
            | X402VerificationError::UnsupportedMode
            | X402VerificationError::NonZeroFee
            | X402VerificationError::PermitField(_) => AccessError::Agreement,
            _ => AccessError::Pending,
        })?;
        Ok(PaidAccess {
            evidence,
            agreement,
        })
    }
}

impl AccessIssuer {
    /// Creates or opens private durable issuance state. It never stores or handles payment keys.
    pub fn open(root: impl Into<PathBuf>, policy: AccessPolicy) -> Result<Self, AccessError> {
        Self::with_faults(root, policy, Arc::new(NoFaults))
    }

    /// The same issuer with an injected journal fault hook for crash tests.
    pub fn with_faults(
        root: impl Into<PathBuf>,
        policy: AccessPolicy,
        faults: Arc<dyn FaultHook>,
    ) -> Result<Self, AccessError> {
        policy.validate()?;
        let root = root.into();
        if let Ok(metadata) = std::fs::symlink_metadata(&root) {
            if !metadata.is_dir() {
                return Err(AccessError::Storage);
            }
        }
        let decoy = MissDecoy::new(policy.suite_id)?;
        let store = Store::open_with_fault_hook(root, faults).map_err(|_| AccessError::Storage)?;
        Ok(Self {
            policy,
            store,
            decoy,
        })
    }

    /// Configured immutable service policy.
    pub fn policy(&self) -> &AccessPolicy {
        &self.policy
    }

    /// Authenticates a request against the evidence its commitment named, if any, before any
    /// payment-observer RPC is made.
    ///
    /// Every failure is [`AccessError::Authentication`]: missing or undecodable evidence, an
    /// agreement this profile does not serve, an expired request, and a bad signature are one
    /// answer, and a miss spends [`MissDecoy`] work. Only the buyer key learns anything more.
    pub fn authenticate_loaded(
        &self,
        evidence: Option<&SelectedAgreement>,
        request: &AccessRequest,
        now: u64,
    ) -> Result<(), AccessError> {
        // Request-only checks first; a route runs them before its evidence lookup too.
        if !request_shape_ok(&request.nonce, request.expires_at, now) {
            return Err(AccessError::Authentication);
        }
        let Some(evidence) = evidence else {
            return Err(self.decoy.reject(&request.signature));
        };
        // A profile mismatch can stop before the agreement's signatures are checked: spend the
        // decoy's work, as a miss does.
        let Ok(agreement) = self.policy.check(evidence) else {
            return Err(self.decoy.reject(&request.signature));
        };
        // The request signature is always verified, so a commitment named under another file
        // costs what a bad signature costs.
        let signed = request_digest_for(
            evidence,
            &agreement,
            self.policy.service_id,
            request.nonce,
            request.expires_at,
            request.payment.as_ref(),
        )
        .and_then(|digest| {
            verify_request_signature(
                evidence.terms.suite_id,
                evidence.terms.buyer_authorization_key.as_bytes(),
                &digest,
                &request.signature,
            )
        });
        if signed.is_ok() && request.deal_commitment == agreement.commitment.to_hex() {
            Ok(())
        } else {
            Err(AccessError::Authentication)
        }
    }

    /// Authenticates the request before any payment-observer RPC is made.
    ///
    /// The error distinguishes a profile mismatch from a bad signature, so an HTTP route must
    /// answer through [`AccessIssuer::authenticate_loaded`] instead.
    pub fn authenticate(
        &self,
        evidence: &SelectedAgreement,
        request: &AccessRequest,
        now: u64,
    ) -> Result<(), AccessError> {
        let agreement = self.policy.check(evidence)?;
        if request.deal_commitment != agreement.commitment.to_hex()
            || request.nonce == [0; 32]
            || request.expires_at <= now
            || request.expires_at.saturating_sub(now) > MAX_REQUEST_LIFETIME
        {
            return Err(AccessError::Authentication);
        }
        let digest = request_digest(
            evidence,
            self.policy.service_id,
            request.nonce,
            request.expires_at,
            request.payment.as_ref(),
        )?;
        suite::suite(evidence.terms.suite_id)
            .map_err(|_| AccessError::Authentication)?
            .verify_authorization(
                evidence.terms.buyer_authorization_key.as_bytes(),
                &digest,
                &request.signature,
            )
            .map_err(|_| AccessError::Authentication)
    }

    /// Persists issuance before a caller can return the payload. Repetition does not recharge.
    /// A completed write followed by a lost response recovers the original issuance time and id.
    pub fn issue(
        &self,
        paid: &PaidAccess,
        request: &AccessRequest,
        now: u64,
    ) -> Result<Issuance, AccessError> {
        let agreement = self.policy.check(&paid.evidence)?;
        if agreement != paid.agreement {
            return Err(AccessError::Authentication);
        }
        self.authenticate(&paid.evidence, request, now)?;
        let resource_sha256 = self.policy.resource_hash();
        let id = IssuanceId(issuance_id(
            &self.policy.service_id,
            agreement.commitment.as_bytes(),
        ));
        let _identity = self
            .store
            .lock_identity()
            .map_err(|_| AccessError::Storage)?;
        let _lease = self
            .store
            .lock_record(&id)
            .map_err(|_| AccessError::Storage)?;
        let previous = self.store.read(&id).map_err(|_| AccessError::Storage)?;
        let repeated = previous.is_some();
        let record = previous.unwrap_or_else(|| Issued {
            version: 1,
            id: id.clone(),
            commitment: agreement.commitment.to_hex(),
            nullifier: agreement.nullifier.to_hex(),
            service_id: self.policy.service_id,
            resource_sha256,
            buyer: paid
                .evidence
                .terms
                .buyer_authorization_key
                .as_bytes()
                .to_vec(),
            issued_at: now,
            delivery_deadline: paid.evidence.terms.service.delivery_deadline,
        });
        if record.commitment != agreement.commitment.to_hex()
            || record.nullifier != agreement.nullifier.to_hex()
            || record.service_id != self.policy.service_id
            || record.resource_sha256 != resource_sha256
            || record.buyer != paid.evidence.terms.buyer_authorization_key.as_bytes()
            || record.delivery_deadline != paid.evidence.terms.service.delivery_deadline
            || record.issued_at > now
        {
            return Err(AccessError::Storage);
        }
        if !repeated {
            self.store
                .write(&record)
                .map_err(|_| AccessError::Storage)?;
        }
        Ok(Issuance {
            issuance_id: id.0,
            resource_sha256: hex::encode(resource_sha256),
            issued_at: record.issued_at,
            late: record.issued_at > record.delivery_deadline,
            repeated,
        })
    }
}
