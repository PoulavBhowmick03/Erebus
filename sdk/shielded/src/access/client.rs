//! Buyer-local retrieval with an explicit, durable x402 permit mode.
//! The buyer never submits a transaction or renews a permit after an uncertain payment.

use std::{
    fmt,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use ark_std::rand::{rngs::OsRng, RngCore};
use erebus_core::commitment::deal_nullifier;
use erebus_core::{shielded_auth, suite::SHIELDED_POSEIDON_EDDSA_SUITE_ID};
use erebus_evm::{
    deployment::EvmDeployment,
    x402::{validate_permit_fields, verify_permit_signature, DealPermit, EXACT_PERMIT2_PROXY},
};
use erebus_journal::{FaultHook, JournalRecord, NoFaults, Store};
use erebus_transport::{
    disclosure::{verify_selected_agreement, SelectedAgreement},
    identity::AuthorizationIdentity,
};
use reqwest::{redirect::Policy, Client, Url};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use super::{
    issuance_id, request_digest, AccessRequest, Issuance, IssuanceId, X402Payment,
    MAX_RESOURCE_BYTES,
};

const MAX_RESPONSE_BYTES: usize = MAX_RESOURCE_BYTES * 2 + 4096;

/// Retrieval failures never authorize another payment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RetrievalError {
    /// Endpoint or local cache configuration is invalid.
    #[error("configure a trusted HTTPS access endpoint and private local cache")]
    Configuration,
    /// Invalid agreement or unsupported snapshot profile.
    #[error("agreement does not support this retrieval profile")]
    Agreement,
    /// The local key does not match the agreement buyer.
    #[error("configure the existing buyer agreement key for access")]
    Authentication,
    /// The seller is still observing finalized payment.
    #[error("access is pending; retry retrieval without another payment")]
    Pending,
    /// The authenticated endpoint reports payment final but cannot issue access.
    /// This is a seller claim, not independently verified chain evidence.
    #[error("seller reports paid but undelivered; retain state and retry retrieval without another payment")]
    SellerReportedUndelivered,
    /// Service unavailable, rejected, or timed out. Payment status is unknown here.
    #[error("access service unavailable; retain state and retry without another payment")]
    Unavailable,
    /// Seller response fails strict content and issuance checks.
    #[error("access response does not match the signed resource; do not pay again")]
    Response,
    /// Local durable storage is unavailable or corrupt.
    #[error("local access cache unavailable; retain state and retry without another payment")]
    Storage,
}

/// Public local-content receipt. This does not independently verify chain payment or delivery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RetrievalReceipt {
    /// Commitment of the signed agreement used to verify the resource.
    pub deal_commitment: String,
    /// Stable service issuance identity.
    pub issuance_id: String,
    /// SHA-256 independently computed from the returned bytes.
    pub resource_sha256: String,
    /// Verified payload length.
    pub resource_bytes: usize,
    /// Owner-only durable file, never resource bytes in model-visible output.
    pub resource_file: PathBuf,
    /// Whether retrieval came from already verified local storage without HTTP.
    pub cached: bool,
    /// Seller claimed finalized payment; this client does not verify that claim through RPC.
    pub seller_reported_payment_finalized: bool,
    /// Always false: content retrieval alone does not independently establish payment.
    pub payment_verified: bool,
    /// True after the local SHA-256 matches the signed fulfillment_digest.
    pub resource_verified: bool,
    /// Always false: local content checking is not an independent delivery audit.
    pub delivery_verified: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    status: String,
    issuance: Issuance,
    payload_hex: String,
    payment_verified: bool,
    delivery_verified: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ErrorResponse {
    status: String,
    payment_verified: bool,
    delivery_verified: bool,
    retry_without_payment: bool,
    error: String,
}

impl Drop for ErrorResponse {
    fn drop(&mut self) {
        self.error.zeroize();
    }
}

impl Drop for Response {
    fn drop(&mut self) {
        self.payload_hex.zeroize();
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CachedResource {
    version: u32,
    id: IssuanceId,
    commitment: String,
    service_id: [u8; 32],
    resource_hash: [u8; 32],
    resource_bytes: usize,
}

impl JournalRecord for CachedResource {
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

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedPermit {
    version: u32,
    id: IssuanceId,
    payment: X402Payment,
}

impl JournalRecord for SavedPermit {
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

/// Trusted endpoint, service identity, and durable buyer cache.
/// Parent directories must remain under operator control.
#[derive(Clone)]
pub struct AccessClient {
    client: Client,
    endpoint: Url,
    service_id: [u8; 32],
    store: Arc<Store<CachedResource>>,
    permits: Arc<Store<SavedPermit>>,
}

impl fmt::Debug for AccessClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AccessClient { <redacted> }")
    }
}

impl AccessClient {
    /// Open the client. HTTP is permitted only for explicitly selected loopback development.
    pub fn open(
        endpoint: &str,
        service_id: [u8; 32],
        cache: impl Into<PathBuf>,
        allow_loopback_http: bool,
    ) -> Result<Self, RetrievalError> {
        Self::with_faults(
            endpoint,
            service_id,
            cache,
            allow_loopback_http,
            Arc::new(NoFaults),
        )
    }

    /// Open with injected durable-write faults for crash tests.
    pub fn with_faults(
        endpoint: &str,
        service_id: [u8; 32],
        cache: impl Into<PathBuf>,
        allow_loopback_http: bool,
        faults: Arc<dyn FaultHook>,
    ) -> Result<Self, RetrievalError> {
        let endpoint = Url::parse(endpoint).map_err(|_| RetrievalError::Configuration)?;
        let local = endpoint.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
                || host == "[::1]"
        });
        if service_id == [0; 32]
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || endpoint.path() != "/v1/access"
            || !(endpoint.scheme() == "https"
                || (allow_loopback_http && endpoint.scheme() == "http" && local))
        {
            return Err(RetrievalError::Configuration);
        }
        let cache = cache.into();
        if let Ok(metadata) = std::fs::symlink_metadata(&cache) {
            if !metadata.is_dir() {
                return Err(RetrievalError::Storage);
            }
        }
        let store = Store::open_with_fault_hook(cache.clone(), faults.clone())
            .map_err(|_| RetrievalError::Storage)?;
        let permits = Store::open_with_fault_hook(cache.join("x402-authorizations"), faults)
            .map_err(|_| RetrievalError::Storage)?;
        let client = Client::builder()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| RetrievalError::Configuration)?;
        Ok(Self {
            client,
            endpoint,
            service_id,
            store: Arc::new(store),
            permits: Arc::new(permits),
        })
    }

    /// Return the cached resource after verifying its bytes and signed agreement binding.
    /// A cache hit requires no signing key, chain provider, or seller availability.
    pub fn cached(
        &self,
        evidence: &SelectedAgreement,
    ) -> Result<Option<RetrievalReceipt>, RetrievalError> {
        let commitment = check_agreement(evidence)?;
        let id = IssuanceId(issuance_id(&self.service_id, &commitment));
        let _lock = self
            .store
            .lock_record(&id)
            .map_err(|_| RetrievalError::Storage)?;
        let path = self.store.record_path(&id);
        match std::fs::symlink_metadata(&path) {
            Ok(info) if !info.is_file() || info.len() > 4096 => {
                return Err(RetrievalError::Storage)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(RetrievalError::Storage),
            _ => {}
        }
        let Some(record) = self.store.read(&id).map_err(|_| RetrievalError::Storage)? else {
            return Err(RetrievalError::Storage);
        };
        private_read(&path, 4096)?;
        if record.commitment != hex::encode(commitment)
            || record.service_id != self.service_id
            || record.resource_hash != evidence.terms.service.fulfillment_digest
            || record.resource_bytes > MAX_RESOURCE_BYTES
        {
            return Err(RetrievalError::Storage);
        }
        let bytes = private_read(&self.store.blob_path(&id, 0), MAX_RESOURCE_BYTES)?;
        if bytes.len() != record.resource_bytes
            || <[u8; 32]>::from(Sha256::digest(&bytes)) != record.resource_hash
        {
            return Err(RetrievalError::Storage);
        }
        Ok(Some(self.receipt(record, true)))
    }

    /// Sign and retrieve once. Neither a timeout nor pending response submits a payment.
    /// Callers must keep the seed private and erase their copy after use.
    pub async fn retrieve(
        &self,
        evidence: &SelectedAgreement,
        seed: &[u8; 32],
        now: u64,
    ) -> Result<RetrievalReceipt, RetrievalError> {
        if evidence
            .terms
            .domain
            .settlement_contract
            .as_ref()
            .is_some_and(|address| address.as_bytes() == EXACT_PERMIT2_PROXY)
        {
            return Err(RetrievalError::Configuration);
        }
        self.retrieve_inner(evidence, seed, now, None).await
    }

    /// Persist one exact authorization before contacting the seller facilitator.
    /// Repeated calls reuse the same bytes even after expiry; they never extend permission.
    pub fn prepare_x402_payment(
        &self,
        evidence: &SelectedAgreement,
        seed: &[u8; 32],
    ) -> Result<X402Payment, RetrievalError> {
        let commitment = check_agreement(evidence)?;
        let deployment = EvmDeployment::new(
            evidence.terms.domain.namespace.clone(),
            EXACT_PERMIT2_PROXY,
            evidence.terms.domain.verifier_version,
            "http://127.0.0.1:1",
        )
        .map_err(|_| RetrievalError::Agreement)?;
        deployment
            .matches_domain(&evidence.terms.domain)
            .map_err(|_| RetrievalError::Agreement)?;
        let identity =
            AuthorizationIdentity::from_bytes(seed).map_err(|_| RetrievalError::Authentication)?;
        if identity.address() != evidence.terms.buyer_authorization_key.as_bytes() {
            return Err(RetrievalError::Authentication);
        }
        let id = IssuanceId(issuance_id(&self.service_id, &commitment));
        let _lock = self
            .permits
            .lock_record(&id)
            .map_err(|_| RetrievalError::Storage)?;
        let saved = self
            .permits
            .read(&id)
            .map_err(|_| RetrievalError::Storage)?;
        let payment = if let Some(saved) = saved {
            saved.payment
        } else {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| RetrievalError::Authentication)?
                .as_secs();
            if now >= evidence.terms.expiry {
                return Err(RetrievalError::Pending);
            }
            let deal = deal_nullifier(&evidence.terms).map_err(|_| RetrievalError::Agreement)?;
            let permit = DealPermit {
                token: deployment
                    .token_address(&evidence.terms.asset)
                    .map_err(|_| RetrievalError::Agreement)?,
                amount: evidence.terms.amount.get(),
                deal,
                deadline: evidence.terms.expiry.min(now.saturating_add(120)),
                to: evidence
                    .terms
                    .payment_recipient
                    .as_bytes()
                    .try_into()
                    .map_err(|_| RetrievalError::Agreement)?,
                valid_after: 0,
            };
            validate_permit_fields(&deployment, &evidence.terms, &permit)
                .map_err(|_| RetrievalError::Agreement)?;
            X402Payment {
                token: permit.token,
                amount: permit.amount,
                deadline: permit.deadline,
                to: permit.to,
                valid_after: permit.valid_after,
                signature: permit.sign(deployment.chain_id, &identity).to_vec(),
            }
        };
        let permit =
            payment.permit(deal_nullifier(&evidence.terms).map_err(|_| RetrievalError::Agreement)?);
        validate_permit_fields(&deployment, &evidence.terms, &permit)
            .map_err(|_| RetrievalError::Agreement)?;
        verify_permit_signature(
            &deployment,
            &evidence.terms,
            &permit,
            &payment
                .signature()
                .map_err(|_| RetrievalError::Authentication)?,
        )
        .map_err(|_| RetrievalError::Authentication)?;
        self.permits
            .write(&SavedPermit {
                version: 1,
                id,
                payment: payment.clone(),
            })
            .map_err(|_| RetrievalError::Storage)?;
        Ok(payment)
    }

    /// Retrieve through the operator-selected exact rail. Signing and persistence precede HTTP.
    /// Pending or failed retrieval never creates another permit or submits a buyer transaction.
    pub async fn retrieve_x402(
        &self,
        evidence: &SelectedAgreement,
        seed: &[u8; 32],
        now: u64,
    ) -> Result<RetrievalReceipt, RetrievalError> {
        if let Some(cached) = self.cached(evidence)? {
            return Ok(cached);
        }
        let client = self.clone();
        let selected = evidence.clone();
        let secret = Zeroizing::new(*seed);
        let payment =
            tokio::task::spawn_blocking(move || client.prepare_x402_payment(&selected, &secret))
                .await
                .map_err(|_| RetrievalError::Storage)??;
        self.retrieve_inner(evidence, seed, now, Some(payment))
            .await
    }

    async fn retrieve_inner(
        &self,
        evidence: &SelectedAgreement,
        seed: &[u8; 32],
        now: u64,
        payment: Option<X402Payment>,
    ) -> Result<RetrievalReceipt, RetrievalError> {
        let cached_client = self.clone();
        let cached_evidence = evidence.clone();
        if let Some(cached) =
            tokio::task::spawn_blocking(move || cached_client.cached(&cached_evidence))
                .await
                .map_err(|_| RetrievalError::Storage)??
        {
            return Ok(cached);
        }
        let selected = evidence.clone();
        let seed = Zeroizing::new(*seed);
        let service_id = self.service_id;
        let (commitment, request) = tokio::task::spawn_blocking(move || {
            let commitment = check_agreement(&selected)?;
            let mut nonce = [0; 32];
            OsRng
                .try_fill_bytes(&mut nonce)
                .map_err(|_| RetrievalError::Authentication)?;
            let expires_at = now.checked_add(120).ok_or(RetrievalError::Authentication)?;
            let digest = request_digest(&selected, service_id, nonce, expires_at, payment.as_ref())
                .map_err(|_| RetrievalError::Agreement)?;
            let (buyer, signature) = if selected.terms.suite_id == SHIELDED_POSEIDON_EDDSA_SUITE_ID
            {
                let (key, signature) = shielded_auth::sign_message(&seed, &digest)
                    .map_err(|_| RetrievalError::Authentication)?;
                (key.to_vec(), signature.to_vec())
            } else {
                let identity = AuthorizationIdentity::from_bytes(seed.as_ref())
                    .map_err(|_| RetrievalError::Authentication)?;
                (
                    identity.address().to_vec(),
                    identity.sign_digest(&digest).to_vec(),
                )
            };
            if buyer != selected.terms.buyer_authorization_key.as_bytes() {
                return Err(RetrievalError::Authentication);
            }
            let request = AccessRequest {
                deal_commitment: hex::encode(commitment),
                nonce,
                expires_at,
                signature,
                payment,
            };
            Ok::<_, RetrievalError>((commitment, request))
        })
        .await
        .map_err(|_| RetrievalError::Authentication)??;
        let mut outgoing = self.client.post(self.endpoint.clone());
        if let Some(payment) = &request.payment {
            outgoing = outgoing.header(
                "PAYMENT-SIGNATURE",
                super::x402::payment_signature_header(&evidence.terms, payment)
                    .map_err(|_| RetrievalError::Agreement)?,
            );
        }
        let mut response = outgoing
            .json(&request)
            .send()
            .await
            .map_err(|_| RetrievalError::Unavailable)?;
        if response.status() == 202 {
            return Err(RetrievalError::Pending);
        }
        let status = response.status();
        if status != 200 && status != 503 {
            return Err(RetrievalError::Unavailable);
        }
        let mut body = Zeroizing::new(Vec::new());
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| RetrievalError::Unavailable)?
        {
            if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(RetrievalError::Response);
            }
            body.extend_from_slice(&chunk);
        }
        if status == 503 {
            if let Ok(failure) = serde_json::from_slice::<ErrorResponse>(&body) {
                if failure.status == "paid_but_undelivered"
                    && failure.payment_verified
                    && !failure.delivery_verified
                    && failure.retry_without_payment
                {
                    return Err(RetrievalError::SellerReportedUndelivered);
                }
            }
            return Err(RetrievalError::Unavailable);
        }
        let received: Response =
            serde_json::from_slice(&body).map_err(|_| RetrievalError::Response)?;
        let id = IssuanceId(issuance_id(&self.service_id, &commitment));
        if received.status != "issued"
            || !received.payment_verified
            || received.delivery_verified
            || received.issuance.issuance_id != id.0
            || received.issuance.resource_sha256
                != hex::encode(evidence.terms.service.fulfillment_digest)
            || received.issuance.issued_at > now.saturating_add(120)
            || received.issuance.late
                != (received.issuance.issued_at > evidence.terms.service.delivery_deadline)
            || received.payload_hex.len() > MAX_RESOURCE_BYTES * 2
        {
            return Err(RetrievalError::Response);
        }
        let bytes = Zeroizing::new(
            hex::decode(&received.payload_hex).map_err(|_| RetrievalError::Response)?,
        );
        if <[u8; 32]>::from(Sha256::digest(&bytes)) != evidence.terms.service.fulfillment_digest {
            return Err(RetrievalError::Response);
        }
        let mut record = CachedResource {
            version: 1,
            id,
            commitment: hex::encode(commitment),
            service_id: self.service_id,
            resource_hash: evidence.terms.service.fulfillment_digest,
            resource_bytes: bytes.len(),
        };
        let storage = self.clone();
        tokio::task::spawn_blocking(move || {
            let _identity = storage
                .store
                .lock_identity()
                .map_err(|_| RetrievalError::Storage)?;
            let _lock = storage
                .store
                .lock_record(&record.id)
                .map_err(|_| RetrievalError::Storage)?;
            storage
                .store
                .write_blob_then_record(&mut record, 0, &bytes, |_| {
                    Ok::<_, erebus_journal::StoreError<IssuanceId>>(())
                })
                .map_err(|_| RetrievalError::Storage)?;
            let receipt = storage.receipt(record, false);
            let written = private_read(&receipt.resource_file, MAX_RESOURCE_BYTES)?;
            if written.len() != receipt.resource_bytes
                || hex::encode(Sha256::digest(&written)) != receipt.resource_sha256
            {
                return Err(RetrievalError::Storage);
            }
            Ok(receipt)
        })
        .await
        .map_err(|_| RetrievalError::Storage)?
    }

    fn receipt(&self, record: CachedResource, cached: bool) -> RetrievalReceipt {
        RetrievalReceipt {
            deal_commitment: record.commitment,
            issuance_id: record.id.0.clone(),
            resource_sha256: hex::encode(record.resource_hash),
            resource_bytes: record.resource_bytes,
            resource_file: self.store.blob_path(&record.id, 0),
            cached,
            seller_reported_payment_finalized: true,
            payment_verified: false,
            resource_verified: true,
            delivery_verified: false,
        }
    }
}

fn check_agreement(evidence: &SelectedAgreement) -> Result<[u8; 32], RetrievalError> {
    let agreement = verify_selected_agreement(evidence).map_err(|_| RetrievalError::Agreement)?;
    let service = &evidence.terms.service;
    if service.access_recipient != evidence.terms.buyer_authorization_key
        || service.unit != "snapshot"
        || service.quantity.get() != 1
        || service.fulfillment_method != "http-access-v1"
    {
        return Err(RetrievalError::Agreement);
    }
    Ok(*agreement.commitment.as_bytes())
}

fn private_read(path: &Path, limit: usize) -> Result<Vec<u8>, RetrievalError> {
    let info = std::fs::symlink_metadata(path).map_err(|_| RetrievalError::Storage)?;
    if !info.is_file() || info.len() > limit as u64 {
        return Err(RetrievalError::Storage);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if info.permissions().mode() & 0o077 != 0 {
            return Err(RetrievalError::Storage);
        }
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| RetrievalError::Storage)?
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| RetrievalError::Storage)?;
    if bytes.len() > limit {
        return Err(RetrievalError::Storage);
    }
    Ok(bytes)
}
