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

use crate::{disclosure::verify_shielded_payment, index_store::IndexStore, rpc::PoolRpc};

pub mod client;
pub mod x402;

/// Longest resource response supported by this snapshot profile.
pub const MAX_RESOURCE_BYTES: usize = 1024 * 1024;
/// Longest lifetime of one signed retrieval request.
pub const MAX_REQUEST_LIFETIME: u64 = 300;

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
    },
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

/// A durable access issuer. The caller must control its directory and parent directories.
pub struct AccessIssuer {
    policy: AccessPolicy,
    store: Store<Issued>,
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
    let domain = evidence
        .terms
        .domain
        .encode()
        .map_err(|_| AccessError::Agreement)?;
    let expiry = expires_at.to_be_bytes();
    let encoded = payment.map(X402Payment::encode);
    let mut preimage: Vec<&[u8]> = vec![
        b"EREBUS_HTTP_ACCESS_REQUEST_V1",
        &domain,
        &service_id,
        agreement.commitment.as_bytes(),
        agreement.nullifier.as_bytes(),
        &evidence.terms.service.fulfillment_digest,
        &nonce,
        &expiry,
    ];
    if let Some(encoded) = &encoded {
        preimage[0] = b"EREBUS_HTTP_ACCESS_REQUEST_X402_V1";
        preimage.push(encoded);
    }
    let digest = suite::keccak256(&preimage);
    if evidence.terms.suite_id == SHIELDED_POSEIDON_EDDSA_SUITE_ID {
        erebus_core::shielded::access_message(&digest).map_err(|_| AccessError::Authentication)
    } else {
        Ok(digest)
    }
}

/// Stable identity for one agreement's issuance at an independently configured service.
pub fn issuance_id(service_id: &[u8; 32], commitment: &[u8; 32]) -> String {
    hex::encode(suite::keccak256(&[
        b"EREBUS_ACCESS_ISSUANCE_V1",
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
        let agreement = policy.check(&evidence)?;
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
            } => {
                let payment =
                    verify_shielded_payment(evidence.clone(), rpc, index, peer_rpc, peer_index)
                        .await
                        .map_err(|error| match error {
                            crate::disclosure::ShieldedDisclosureError::Payment => {
                                AccessError::Pending
                            }
                            crate::disclosure::ShieldedDisclosureError::Observation(
                                crate::observation::ObservationError::Recovery(
                                    crate::recovery::RecoveryError::HistoryPending { .. },
                                ),
                            ) => AccessError::Pending,
                            _ => AccessError::Observation,
                        })?;
                payment.settlement
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
        let store = Store::open_with_fault_hook(root, faults).map_err(|_| AccessError::Storage)?;
        Ok(Self { policy, store })
    }

    /// Configured immutable service policy.
    pub fn policy(&self) -> &AccessPolicy {
        &self.policy
    }

    /// Authenticates the request before any payment-observer RPC is made.
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
