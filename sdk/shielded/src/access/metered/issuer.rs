//! Durable seller ledger for prepaid metered access (`docs/metropolis-operations.md`).
//!
//! One entitlement record `E` per agreement holds N, the buyer key, and the finality
//! certificate. One call record `C(k)` per consumed index holds the response hash and the
//! buyer-signed request that consumed it; its blob is the response body. `C(k)` is the only
//! consumption authority. `E.next_hint` is a cache that can lag, never lead.
//!
//! Lock order: entitlement-store identity lock, then the `E` record lock, then call-store
//! identity lock, then the `C(k)` record lock. Every path that writes takes them in this order.
//! The entitlement identity lock serializes every metered write at this service, including the
//! resource fetch; that is the price of reusing the snapshot lock order unchanged.
//!
//! Finality is verified once, by a [`PaymentCertifier`], and cached in `E`. Later requests
//! revalidate the stored certificate against the agreement and the configured observers
//! locally and make no RPC call. A changed observer configuration quarantines `E` until the
//! new observers certify the same settlement again; it never resets `next` or adds calls.
//! Nothing here signs or submits a payment.

use std::{fmt, future::Future, path::PathBuf, sync::Arc};

use erebus_core::suite::SHIELDED_POSEIDON_EDDSA_SUITE_ID;
use erebus_journal::{FaultHook, JournalRecord, NoFaults, RecordId, Store, StoreError};
use erebus_transport::disclosure::SelectedAgreement;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    call_id, entitlement_id, EntitlementId, MeteredOperation, MeteredPolicy, MeteredRequest,
};
use crate::{
    access::{shielded_access_error, AccessBackend, AccessError, CheckedAgreement, MissDecoy},
    disclosure::{verify_shielded_payment_certified, PaymentCertificate},
    rpc::PoolRuntimePins,
};

/// Version of the finality-once rule an entitlement was certified under.
pub const METERED_VERIFIER_VERSION: u32 = 1;

const ENTITLEMENT_DIR: &str = "entitlements";
const CALL_DIR: &str = "calls";
/// The single stored response of a call record is attempt 0 of its blob.
const RESPONSE_ATTEMPT: usize = 0;

/// A metered request failed. Payment may be final; never answer any of these with a payment
/// challenge, and never recharge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MeteredError {
    /// Agreement, authentication, pending payment, observation, or storage failure.
    #[error(transparent)]
    Access(#[from] AccessError),
    /// The call index is ahead of the next unconsumed index. Nothing was consumed.
    #[error("call index is ahead of the next unconsumed call")]
    OutOfOrder,
    /// Every prepaid call has been consumed. Earlier indices remain retrievable.
    #[error("prepaid allowance is exhausted")]
    AllowanceExhausted,
    /// The resource failed or returned an oversize response. Nothing was consumed.
    #[error("resource is unavailable; nothing was consumed")]
    Resource,
    /// A payment certificate does not describe this agreement.
    #[error("payment certificate does not match this agreement")]
    CertificateMismatch,
    /// This buyer has too many requests in flight or exceeded its rate. Retry later.
    #[error("too many requests from this buyer; retry later without paying again")]
    Busy,
}

impl MeteredError {
    /// HTTP status for this failure. Exhaustion is 403, never 402.
    #[must_use]
    pub const fn status(self) -> u16 {
        match self {
            Self::Access(AccessError::Authentication) => 401,
            Self::Access(AccessError::Agreement) | Self::CertificateMismatch => 400,
            Self::Access(AccessError::Pending) => 202,
            Self::Access(AccessError::Observation | AccessError::Storage) | Self::Resource => 503,
            Self::OutOfOrder => 409,
            Self::AllowanceExhausted => 403,
            Self::Busy => 429,
        }
    }

    /// Stable machine-readable error code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Access(AccessError::Authentication) => "unauthenticated",
            Self::Access(AccessError::Agreement) => "agreement_mismatch",
            Self::Access(AccessError::Pending) => "payment_pending",
            Self::Access(AccessError::Observation) => "observation_unavailable",
            Self::Access(AccessError::Storage) => "storage_unavailable",
            Self::OutOfOrder => "out_of_order",
            Self::AllowanceExhausted => "allowance_exhausted",
            Self::Resource => "resource_unavailable",
            Self::CertificateMismatch => "certificate_mismatch",
            Self::Busy => "busy",
        }
    }
}

impl From<StoreError<LedgerId>> for MeteredError {
    fn from(_: StoreError<LedgerId>) -> Self {
        Self::Access(AccessError::Storage)
    }
}

/// Verifies finalized payment for a checked agreement and certifies what it observed.
///
/// This is the trust root of the finality-once rule: the issuer stores whatever certificate
/// it returns, after checking only that it binds the agreement and names the configured
/// observers. Production uses [`AccessBackend::Shielded`].
pub trait PaymentCertifier {
    /// Observes finalized payment. Pending or unavailable observation must be an error.
    fn certify(
        &self,
        checked: &CheckedAgreement,
    ) -> impl Future<Output = Result<PaymentCertificate, AccessError>> + Send;
}

impl PaymentCertifier for AccessBackend {
    async fn certify(&self, checked: &CheckedAgreement) -> Result<PaymentCertificate, AccessError> {
        let Self::Shielded {
            rpc,
            index,
            peer_rpc,
            peer_index,
            runtime,
        } = self
        else {
            return Err(AccessError::Agreement);
        };
        verify_shielded_payment_certified(
            checked.evidence().clone(),
            rpc,
            index,
            peer_rpc,
            peer_index,
            **runtime,
        )
        .await
        .map(|certified| certified.certificate)
        .map_err(shielded_access_error)
    }
}

impl AccessBackend {
    /// Endpoint fingerprints of the two configured shielded observers, in certificate order.
    #[must_use]
    pub fn shielded_observers(&self) -> Option<[[u8; 32]; 2]> {
        match self {
            Self::Shielded { rpc, peer_rpc, .. } => {
                Some([rpc.endpoint_fingerprint(), peer_rpc.endpoint_fingerprint()])
            }
            _ => None,
        }
    }
}

/// The resource could not produce a response for this call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceUnavailable;

/// Produces the response for one call. Called at most once per committed index, under the
/// entitlement lock. A failed or retried call may call it again for the same index, so it must
/// not have side effects the seller cannot repeat.
pub trait ResponseSource {
    /// The response body for `call_index`.
    fn fetch(&self, call_index: u64) -> Result<Vec<u8>, ResourceUnavailable>;
}

impl<F> ResponseSource for F
where
    F: Fn(u64) -> Result<Vec<u8>, ResourceUnavailable>,
{
    fn fetch(&self, call_index: u64) -> Result<Vec<u8>, ResourceUnavailable> {
        self(call_index)
    }
}

/// One durably committed call. Every field but `repeated` is read from its call record.
#[derive(Clone, PartialEq, Eq)]
pub struct MeteredCall {
    /// Zero-based call index.
    pub call_index: u64,
    /// Exact stored response bytes.
    pub body: Vec<u8>,
    /// SHA-256 of `body`.
    pub response_sha256: [u8; 32],
    /// Original commit time; never reset by retries.
    pub committed_at: u64,
    /// Committed after the signed delivery deadline; the allowance is not voided.
    pub late: bool,
    /// The call was already committed before this request.
    pub repeated: bool,
    /// Prepaid calls N.
    pub quantity: u64,
    /// Unconsumed calls after this request was served.
    pub remaining: u64,
}

impl fmt::Debug for MeteredCall {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MeteredCall")
            .field("call_index", &self.call_index)
            .field("body", &"<redacted>")
            .field("committed_at", &self.committed_at)
            .field("late", &self.late)
            .field("repeated", &self.repeated)
            .field("quantity", &self.quantity)
            .field("remaining", &self.remaining)
            .finish()
    }
}

/// Allowance report. Consumes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeteredStatus {
    /// Prepaid calls N.
    pub quantity: u64,
    /// Next unconsumed index.
    pub next: u64,
    /// `quantity - next`.
    pub remaining: u64,
    /// Every committed call below `next`, in index order. Bodies are not included.
    pub calls: Vec<CallSummary>,
}

/// What the ledger records about one committed call, without its body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallSummary {
    /// Zero-based call index.
    pub call_index: u64,
    /// Stored response length in bytes.
    pub response_len: u64,
    /// SHA-256 of the stored response.
    pub response_sha256: [u8; 32],
    /// Original commit time.
    pub committed_at: u64,
    /// Committed after the signed delivery deadline.
    pub late: bool,
}

/// What a served request returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeteredReply {
    /// A committed call, new or repeated.
    Call(MeteredCall),
    /// The allowance.
    Status(MeteredStatus),
}

/// A request authenticated against a checked agreement. Only [`MeteredIssuer::authorize`]
/// constructs it.
pub struct AuthorizedRequest {
    checked: CheckedAgreement,
    request: MeteredRequest,
    entitlement: EntitlementId,
    needs_certificate: bool,
}

impl AuthorizedRequest {
    /// No current entitlement exists: certify payment and [`MeteredIssuer::activate`] first.
    #[must_use]
    pub fn needs_certificate(&self) -> bool {
        self.needs_certificate
    }

    /// The checked agreement a certifier must observe.
    #[must_use]
    pub fn checked(&self) -> &CheckedAgreement {
        &self.checked
    }
}

impl fmt::Debug for AuthorizedRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuthorizedRequest { <redacted> }")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LedgerId(String);

impl LedgerId {
    fn new(bytes: &[u8; 32]) -> Self {
        Self(hex::encode(bytes))
    }
}

impl fmt::Display for LedgerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl RecordId for LedgerId {
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

/// Deployment facts the seller's verification configuration pins for one metered service.
///
/// These are the inputs that decide which pool state is accepted: the chain and pool identity,
/// the deployment origin whose history is trusted, and the reviewed runtime pins. The endpoint
/// fingerprints are bound beside them in [`ObserverBinding`]. Changing any field quarantines
/// every existing entitlement for revalidation; consumption state is never reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentBinding {
    /// EVM chain ID the pool observers are configured for.
    pub chain_id: u64,
    /// Pool contract address the observers filter for.
    pub pool: [u8; 20],
    /// Block in which the pool was deployed.
    pub first_block: u64,
    /// Trusted hash of the deployment block.
    pub first_hash: [u8; 32],
    /// Reviewed runtime pins of the pool and its four immutable dependencies.
    pub runtime: PoolRuntimePins,
}

/// Seller configuration an entitlement was certified under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObserverBinding {
    verifier_version: u32,
    policy_fingerprint: [u8; 32],
    observers: [[u8; 32]; 2],
    deployment: DeploymentBinding,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entitlement {
    version: u32,
    id: LedgerId,
    service_id: [u8; 32],
    commitment: [u8; 32],
    nullifier: [u8; 32],
    buyer: Vec<u8>,
    quantity: u64,
    profile_digest: [u8; 32],
    delivery_deadline: u64,
    binding: ObserverBinding,
    certificate: PaymentCertificate,
    verified_at: u64,
    next_hint: u64,
}

impl JournalRecord for Entitlement {
    type Id = LedgerId;
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

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CallRecord {
    version: u32,
    id: LedgerId,
    entitlement: [u8; 32],
    index: u64,
    response_len: u64,
    response_sha256: [u8; 32],
    committed_at: u64,
    late: bool,
    /// The buyer-signed request that consumed this index: evidence of consent to it.
    request: MeteredRequest,
}

impl JournalRecord for CallRecord {
    type Id = LedgerId;
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

/// A durable prepaid metered issuer. The caller must control its directory and parents, and
/// every worker serving this service must share this directory and its locks.
pub struct MeteredIssuer {
    policy: MeteredPolicy,
    binding: ObserverBinding,
    entitlements: Store<Entitlement>,
    calls: Store<CallRecord>,
    decoy: MissDecoy,
}

impl fmt::Debug for MeteredIssuer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MeteredIssuer { <redacted> }")
    }
}

/// What an existing entitlement says about the current configuration.
enum Standing {
    Current,
    Quarantined,
}

impl MeteredIssuer {
    /// Creates or opens durable metered state for suite-2 shielded agreements.
    /// `observers` are the configured RPC endpoint fingerprints, in certificate order.
    /// `deployment` pins the chain, pool, deployment origin, and runtime code.
    pub fn open(
        root: impl Into<PathBuf>,
        policy: MeteredPolicy,
        observers: [[u8; 32]; 2],
        deployment: DeploymentBinding,
    ) -> Result<Self, MeteredError> {
        Self::with_faults(root, policy, observers, deployment, Arc::new(NoFaults))
    }

    /// The same issuer with one journal fault hook shared by both stores, for crash tests.
    pub fn with_faults(
        root: impl Into<PathBuf>,
        policy: MeteredPolicy,
        observers: [[u8; 32]; 2],
        deployment: DeploymentBinding,
        faults: Arc<dyn FaultHook>,
    ) -> Result<Self, MeteredError> {
        policy.validate()?;
        if policy.suite_id != SHIELDED_POSEIDON_EDDSA_SUITE_ID
            || observers[0] == observers[1]
            || observers.contains(&[0; 32])
            || deployment.chain_id == 0
            || deployment.pool == [0; 20]
            || deployment.first_hash == [0; 32]
            || deployment.runtime.pool == [0; 32]
            || deployment.runtime.poseidon == [0; 32]
            || deployment.runtime.deposit == [0; 32]
            || deployment.runtime.transfer == [0; 32]
            || deployment.runtime.withdraw == [0; 32]
        {
            return Err(AccessError::Agreement.into());
        }
        let root = root.into();
        let root = if root.is_absolute() {
            root
        } else {
            std::env::current_dir()
                .map_err(|_| AccessError::Storage)?
                .join(root)
        };
        for directory in [&root, &root.join(ENTITLEMENT_DIR), &root.join(CALL_DIR)] {
            match std::fs::symlink_metadata(directory) {
                Ok(metadata) if !metadata.is_dir() => return Err(AccessError::Storage.into()),
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    return Err(AccessError::Storage.into());
                }
                _ => {}
            }
        }
        std::fs::create_dir_all(&root).map_err(|_| AccessError::Storage)?;
        let entitlements =
            Store::open_with_fault_hook(root.join(ENTITLEMENT_DIR), Arc::clone(&faults))?;
        let calls = Store::open_with_fault_hook(root.join(CALL_DIR), faults)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| AccessError::Storage)?;
            // Child fsync alone cannot persist a newly created parent's directory entry.
            for directory in [entitlements.root(), calls.root()]
                .into_iter()
                .chain(root.ancestors())
            {
                std::fs::File::open(directory)
                    .and_then(|file| file.sync_all())
                    .map_err(|_| AccessError::Storage)?;
            }
        }
        Ok(Self {
            binding: ObserverBinding {
                verifier_version: METERED_VERIFIER_VERSION,
                policy_fingerprint: policy.fingerprint(),
                observers,
                deployment,
            },
            decoy: MissDecoy::new(policy.suite_id)?,
            policy,
            entitlements,
            calls,
        })
    }

    /// Configured metered service policy.
    #[must_use]
    pub fn policy(&self) -> &MeteredPolicy {
        &self.policy
    }

    /// Checks terms and authenticates the request before any RPC or ledger write, then reads
    /// the entitlement. A corrupt or mismatched entitlement fails here and is never rebuilt.
    pub fn authorize(
        &self,
        evidence: &SelectedAgreement,
        request: &MeteredRequest,
        now: u64,
    ) -> Result<AuthorizedRequest, MeteredError> {
        let checked = self.policy.check_agreement(evidence.clone())?;
        self.policy.authenticate_checked(&checked, request, now)?;
        self.entitled(checked, request, now)
    }

    /// [`Self::authorize`] for the evidence a request's commitment named, if any, answering
    /// every pre-authentication failure with [`AccessError::Authentication`].
    ///
    /// Missing or undecodable evidence, an agreement this policy does not serve, an expired
    /// request, and a bad signature are one answer, and a miss spends [`MissDecoy`] work, so
    /// the route is not a commitment oracle. Failures after authentication keep their codes.
    pub fn authorize_loaded(
        &self,
        evidence: Option<&SelectedAgreement>,
        request: &MeteredRequest,
        now: u64,
    ) -> Result<AuthorizedRequest, MeteredError> {
        // Request-only checks first; the route runs them before its evidence lookup too.
        if !super::metered_request_shape_ok(request, now) {
            return Err(AccessError::Authentication.into());
        }
        let Some(evidence) = evidence else {
            return Err(self.decoy.reject(&request.signature).into());
        };
        // A profile mismatch can stop before the agreement's signatures: spend the decoy's work.
        let Ok(checked) = self.policy.check_agreement(evidence.clone()) else {
            return Err(self.decoy.reject(&request.signature).into());
        };
        if !self.policy.authenticate_uniformly(&checked, request, now) {
            return Err(AccessError::Authentication.into());
        }
        self.entitled(checked, request, now)
    }

    /// Reads the entitlement of an authenticated request.
    fn entitled(
        &self,
        checked: CheckedAgreement,
        request: &MeteredRequest,
        now: u64,
    ) -> Result<AuthorizedRequest, MeteredError> {
        let entitlement = entitlement_id(
            &self.policy.service_id,
            checked.agreement().commitment.as_bytes(),
        );
        let needs_certificate = match self
            .entitlements
            .read(&LedgerId::new(entitlement.as_bytes()))?
        {
            None => true,
            Some(stored) => matches!(
                self.standing(&stored, &checked, now)?,
                Standing::Quarantined
            ),
        };
        Ok(AuthorizedRequest {
            checked,
            request: request.clone(),
            entitlement,
            needs_certificate,
        })
    }

    /// Stores the certificate as the entitlement, or rebinds a quarantined entitlement to the
    /// current observers. It never resets `next`, changes N, or replaces a current certificate.
    pub fn activate(
        &self,
        authorized: &AuthorizedRequest,
        certificate: PaymentCertificate,
        now: u64,
    ) -> Result<(), MeteredError> {
        let checked = &authorized.checked;
        if !certificate.binds(checked.evidence(), checked.agreement()) {
            return Err(MeteredError::CertificateMismatch);
        }
        if certificate.observers != self.binding.observers {
            // The certifier is not the observer pair this issuer is configured with.
            return Err(AccessError::Observation.into());
        }
        if certificate.runtime != self.binding.deployment.runtime {
            // The certifier verified different runtime code than this deployment pins.
            return Err(MeteredError::CertificateMismatch);
        }
        let id = LedgerId::new(authorized.entitlement.as_bytes());
        let _identity = self.entitlements.lock_identity()?;
        let _lease = self.entitlements.lock_record(&id)?;
        let record = match self.entitlements.read(&id)? {
            None => {
                let terms = &checked.evidence().terms;
                Entitlement {
                    version: Entitlement::CURRENT_VERSION,
                    id,
                    service_id: self.policy.service_id,
                    commitment: *checked.agreement().commitment.as_bytes(),
                    nullifier: *checked.agreement().nullifier.as_bytes(),
                    buyer: terms.buyer_authorization_key.as_bytes().to_vec(),
                    quantity: quantity(checked)?,
                    profile_digest: self.policy.profile.profile_digest(),
                    delivery_deadline: terms.service.delivery_deadline,
                    binding: self.binding.clone(),
                    certificate,
                    verified_at: now,
                    next_hint: 0,
                }
            }
            Some(mut stored) => match self.standing(&stored, checked, now)? {
                // Another request activated it first; keep the stored certificate.
                Standing::Current => return Ok(()),
                Standing::Quarantined => {
                    // New observers must find the same settlement the old ones certified.
                    if stored.certificate.settlement != certificate.settlement {
                        return Err(AccessError::Observation.into());
                    }
                    stored.binding = self.binding.clone();
                    stored.certificate = certificate;
                    stored.verified_at = now;
                    stored
                }
            },
        };
        self.entitlements.write(&record)?;
        Ok(())
    }

    /// Serves one call or status under the entitlement lock. A call response is returned only
    /// after its call record is durable; a repeated index returns the stored bytes.
    pub fn serve(
        &self,
        authorized: &AuthorizedRequest,
        source: &impl ResponseSource,
        now: u64,
    ) -> Result<MeteredReply, MeteredError> {
        let id = LedgerId::new(authorized.entitlement.as_bytes());
        let _identity = self.entitlements.lock_identity()?;
        let _lease = self.entitlements.lock_record(&id)?;
        // Activation must have written it; a missing record here is never "start over".
        let mut stored = self.entitlements.read(&id)?.ok_or(AccessError::Storage)?;
        if matches!(
            self.standing(&stored, &authorized.checked, now)?,
            Standing::Quarantined
        ) {
            return Err(AccessError::Observation.into());
        }
        let next = self.next_index(&stored, now)?;
        let request = &authorized.request;
        if request.operation == MeteredOperation::Status {
            let calls = (0..next)
                .map(|index| {
                    let record = self
                        .read_call(&stored, index, now)?
                        .ok_or(AccessError::Storage)?;
                    Ok(CallSummary {
                        call_index: index,
                        response_len: record.response_len,
                        response_sha256: record.response_sha256,
                        committed_at: record.committed_at,
                        late: record.late,
                    })
                })
                .collect::<Result<_, MeteredError>>()?;
            return Ok(MeteredReply::Status(MeteredStatus {
                quantity: stored.quantity,
                next,
                remaining: stored.quantity - next,
                calls,
            }));
        }
        let index = request.call_index;
        if index < next {
            return self
                .replay(&stored, index, stored.quantity - next, now)
                .map(MeteredReply::Call);
        }
        if index >= stored.quantity {
            return Err(MeteredError::AllowanceExhausted);
        }
        if index > next {
            return Err(MeteredError::OutOfOrder);
        }
        let call = self.consume(&stored, request, source, stored.quantity - (index + 1), now)?;
        // The call record is the commit point and is already durable. The hint is a cache:
        // if this write fails, the next request probes past it and counts the unit once.
        stored.next_hint = index + 1;
        let _ = self.entitlements.write(&stored);
        Ok(MeteredReply::Call(call))
    }

    /// Authorizes, certifies finality once if needed, and serves. This performs blocking file
    /// I/O and takes blocking file locks inline; an async service should call the three steps
    /// itself and run [`Self::authorize`], [`Self::activate`], and [`Self::serve`] on a
    /// blocking thread.
    pub async fn handle<C: PaymentCertifier, S: ResponseSource>(
        &self,
        certifier: &C,
        source: &S,
        evidence: &SelectedAgreement,
        request: &MeteredRequest,
        now: u64,
    ) -> Result<MeteredReply, MeteredError> {
        let authorized = self.authorize(evidence, request, now)?;
        if authorized.needs_certificate {
            let certificate = certifier.certify(&authorized.checked).await?;
            self.activate(&authorized, certificate, now)?;
        }
        self.serve(&authorized, source, now)
    }

    /// Checks a stored entitlement against the agreement and this issuer's configuration.
    fn standing(
        &self,
        stored: &Entitlement,
        checked: &CheckedAgreement,
        now: u64,
    ) -> Result<Standing, MeteredError> {
        let terms = &checked.evidence().terms;
        if stored.verified_at > now
            || stored.next_hint > stored.quantity
            || stored.certificate.observers != stored.binding.observers
            || stored.certificate.runtime != stored.binding.deployment.runtime
        {
            return Err(AccessError::Storage.into());
        }
        if stored.service_id != self.policy.service_id
            || stored.commitment != *checked.agreement().commitment.as_bytes()
            || stored.nullifier != *checked.agreement().nullifier.as_bytes()
            || stored.buyer != terms.buyer_authorization_key.as_bytes()
            || stored.quantity != quantity(checked)?
            || stored.profile_digest != terms.service.fulfillment_digest
            || stored.delivery_deadline != terms.service.delivery_deadline
            || !stored
                .certificate
                .binds(checked.evidence(), checked.agreement())
        {
            return Err(MeteredError::CertificateMismatch);
        }
        Ok(if stored.binding == self.binding {
            Standing::Current
        } else {
            Standing::Quarantined
        })
    }

    /// Next unconsumed index: the first missing call record at or after the hint.
    ///
    /// The hint is written only after `C(k)`, so it can lag but never lead. A hint whose
    /// predecessor is missing, or a record after the first gap, means lost or restored state:
    /// fail closed rather than reissue an index the buyer may already hold.
    fn next_index(&self, stored: &Entitlement, now: u64) -> Result<u64, MeteredError> {
        let mut next = stored.next_hint;
        if next > 0 && self.read_call(stored, next - 1, now)?.is_none() {
            return Err(AccessError::Storage.into());
        }
        while next < stored.quantity && self.read_call(stored, next, now)?.is_some() {
            next += 1;
        }
        if next + 1 < stored.quantity && self.read_call(stored, next + 1, now)?.is_some() {
            return Err(AccessError::Storage.into());
        }
        Ok(next)
    }

    fn read_call(
        &self,
        stored: &Entitlement,
        index: u64,
        now: u64,
    ) -> Result<Option<CallRecord>, MeteredError> {
        let entitlement = entitlement_id(&stored.service_id, &stored.commitment);
        let id = LedgerId::new(call_id(&entitlement, index).as_bytes());
        let Some(record) = self.calls.read(&id)? else {
            return Ok(None);
        };
        if record.entitlement != *entitlement.as_bytes()
            || record.index != index
            || record.committed_at > now
            || record.late != (record.committed_at > stored.delivery_deadline)
            || record.response_len > u64::from(self.policy.profile.max_response_bytes())
            || record.request.operation != MeteredOperation::Call
            || record.request.call_index != index
        {
            return Err(AccessError::Storage.into());
        }
        // A call record is usable only while its committed response remains recoverable.
        // Check this on hint probes and status too, not only on replay of that same index.
        self.calls.sync()?;
        let body = self.calls.read_blob(&record.id, RESPONSE_ATTEMPT)?;
        if u64::try_from(body.len()).ok() != Some(record.response_len)
            || <[u8; 32]>::from(Sha256::digest(&body)) != record.response_sha256
        {
            return Err(AccessError::Storage.into());
        }
        Ok(Some(record))
    }

    fn replay(
        &self,
        stored: &Entitlement,
        index: u64,
        remaining: u64,
        now: u64,
    ) -> Result<MeteredCall, MeteredError> {
        // A previous attempt may have renamed C(k) and then failed the directory sync, which
        // reported 503 while leaving the record visible. Establish that rename's durability
        // before these bytes leave the process: otherwise a power loss could erase C(k) and a
        // later request could consume the index again with different bytes.
        self.calls.sync()?;
        let record = self
            .read_call(stored, index, now)?
            .ok_or(AccessError::Storage)?;
        let body = self.calls.read_blob(&record.id, RESPONSE_ATTEMPT)?;
        let response_sha256: [u8; 32] = Sha256::digest(&body).into();
        if u64::try_from(body.len()).ok() != Some(record.response_len)
            || response_sha256 != record.response_sha256
        {
            return Err(AccessError::Storage.into());
        }
        Ok(MeteredCall {
            call_index: index,
            body,
            response_sha256,
            committed_at: record.committed_at,
            late: record.late,
            repeated: true,
            quantity: stored.quantity,
            remaining,
        })
    }

    fn consume(
        &self,
        stored: &Entitlement,
        request: &MeteredRequest,
        source: &impl ResponseSource,
        remaining: u64,
        now: u64,
    ) -> Result<MeteredCall, MeteredError> {
        let index = request.call_index;
        let body = source.fetch(index).map_err(|_| MeteredError::Resource)?;
        let response_len = u64::try_from(body.len()).map_err(|_| MeteredError::Resource)?;
        if response_len > u64::from(self.policy.profile.max_response_bytes()) {
            return Err(MeteredError::Resource);
        }
        let response_sha256: [u8; 32] = Sha256::digest(&body).into();
        let entitlement = entitlement_id(&stored.service_id, &stored.commitment);
        let id = LedgerId::new(call_id(&entitlement, index).as_bytes());
        let mut record = CallRecord {
            version: CallRecord::CURRENT_VERSION,
            id: id.clone(),
            entitlement: *entitlement.as_bytes(),
            index,
            response_len,
            response_sha256,
            committed_at: now,
            late: now > stored.delivery_deadline,
            request: request.clone(),
        };
        let _identity = self.calls.lock_identity()?;
        let _lease = self.calls.lock_record(&id)?;
        if self.calls.read(&id)?.is_some() {
            // next_index found it missing under the same entitlement lock.
            return Err(AccessError::Storage.into());
        }
        self.calls
            .write_blob_then_record(&mut record, RESPONSE_ATTEMPT, &body, |_| {
                Ok::<(), MeteredError>(())
            })?;
        Ok(MeteredCall {
            call_index: index,
            body,
            response_sha256,
            committed_at: record.committed_at,
            late: record.late,
            repeated: false,
            quantity: stored.quantity,
            remaining,
        })
    }
}

fn quantity(checked: &CheckedAgreement) -> Result<u64, MeteredError> {
    u64::try_from(checked.evidence().terms.service.quantity.get())
        .map_err(|_| AccessError::Agreement.into())
}

#[cfg(test)]
mod tests;
