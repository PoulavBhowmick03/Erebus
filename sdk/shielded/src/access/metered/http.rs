//! HTTP route for prepaid metered access (`docs/metropolis-operations.md`).
//!
//! `POST /v1/metered` takes one JSON [`MeteredRequest`]. Every issuer step that reads or writes
//! the ledger, waits on a file lock, or reads the feed runs on a blocking thread; only payment
//! certification awaits on the runtime. Nothing here signs, submits, or challenges for payment,
//! so no response is ever 402. The route is loopback-only like the snapshot service; remote
//! clients need an authenticated TLS gateway in front of it.

use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{body::Bytes, extract::State, http::StatusCode, routing::post, Json, Router};
use serde_json::{json, Value};
use tokio::sync::Semaphore;
use zeroize::Zeroizing;

use super::{
    AuthorizedRequest, MeteredError, MeteredIssuer, MeteredProfile, MeteredReply, MeteredRequest,
    PaymentCertifier, ResourceUnavailable, ResponseSource,
};
use crate::access::{read_evidence, AccessError, BuyerLimiter};

/// Path of the metered route.
pub const METERED_PATH: &str = "/v1/metered";

/// The seller-owned feed file, read when a call consumes a new index.
///
/// The file is read at call time, never cached, and bounded by the profile's response cap. A
/// missing, non-regular, unreadable, or oversize file is [`ResourceUnavailable`], so the issuer
/// answers 503 and consumes nothing. Replacing the file changes only calls not yet committed.
#[derive(Debug, Clone)]
pub struct FeedFile {
    path: PathBuf,
    max_bytes: usize,
}

impl FeedFile {
    /// A feed bounded by the profile's `max_response_bytes`.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>, profile: &MeteredProfile) -> Self {
        Self {
            path: path.into(),
            max_bytes: profile.max_response_bytes() as usize,
        }
    }
}

impl ResponseSource for FeedFile {
    fn fetch(&self, _call_index: u64) -> Result<Vec<u8>, ResourceUnavailable> {
        read_bounded(&self.path, self.max_bytes, false)
            .map(|bytes| bytes.to_vec())
            .ok_or(ResourceUnavailable)
    }
}

/// Reads a regular, non-symlink file of at most `limit` bytes. With `owner_only`, a file that
/// group or others can access is refused.
fn read_bounded(path: &Path, limit: usize, owner_only: bool) -> Option<Zeroizing<Vec<u8>>> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return None;
    }
    #[cfg(unix)]
    if owner_only {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return None;
        }
    }
    #[cfg(not(unix))]
    let _ = owner_only;
    let mut bytes = Zeroizing::new(Vec::new());
    std::fs::File::open(path)
        .ok()?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= limit).then_some(bytes)
}

/// Shared state of the metered route: one issuer, its payment certifier, and the response
/// source. Every worker serving this service must share the issuer's directory.
pub struct MeteredService<C, S> {
    issuer: Arc<MeteredIssuer>,
    certifier: Arc<C>,
    source: Arc<S>,
    evidence_root: PathBuf,
    slots: Arc<Semaphore>,
    buyers: Arc<BuyerLimiter>,
}

impl<C, S> Clone for MeteredService<C, S> {
    fn clone(&self) -> Self {
        Self {
            issuer: Arc::clone(&self.issuer),
            certifier: Arc::clone(&self.certifier),
            source: Arc::clone(&self.source),
            evidence_root: self.evidence_root.clone(),
            slots: Arc::clone(&self.slots),
            buyers: Arc::clone(&self.buyers),
        }
    }
}

impl<C, S> MeteredService<C, S>
where
    C: PaymentCertifier + Send + Sync + 'static,
    S: ResponseSource + Send + Sync + 'static,
{
    /// A service reading `<evidence_root>/<deal_commitment>.evidence`, owner-only, and
    /// answering at most `concurrency` requests at once; more are told to retry (429).
    pub fn new(
        issuer: MeteredIssuer,
        certifier: Arc<C>,
        source: S,
        evidence_root: impl Into<PathBuf>,
        concurrency: usize,
    ) -> Self {
        Self {
            issuer: Arc::new(issuer),
            certifier,
            source: Arc::new(source),
            evidence_root: evidence_root.into(),
            slots: Arc::new(Semaphore::new(concurrency)),
            buyers: Arc::new(BuyerLimiter::default()),
        }
    }

    /// A router serving only [`METERED_PATH`]. The caller adds the body limit and binds it.
    pub fn router(self) -> Router {
        Router::new()
            .route(METERED_PATH, post(metered::<C, S>))
            .with_state(self)
    }
}

fn now() -> Result<u64, MeteredError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs())
        .map_err(|_| AccessError::Authentication.into())
}

/// Runs one blocking issuer step off the async runtime. A panicked or cancelled step is a
/// storage failure: the ledger's durable state, not this task, decides what was consumed.
async fn blocking<T: Send + 'static>(
    step: impl FnOnce() -> Result<T, MeteredError> + Send + 'static,
) -> Result<T, MeteredError> {
    tokio::task::spawn_blocking(step)
        .await
        .unwrap_or(Err(AccessError::Storage.into()))
}

fn is_commitment_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

async fn metered<C, S>(
    State(service): State<MeteredService<C, S>>,
    body: Bytes,
) -> (StatusCode, Json<Value>)
where
    C: PaymentCertifier + Send + Sync + 'static,
    S: ResponseSource + Send + Sync + 'static,
{
    let Ok(_slot) = Arc::clone(&service.slots).try_acquire_owned() else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(
                json!({"status":"busy","error":"busy","retry_without_payment":true,"delivery_verified":false}),
            ),
        );
    };
    let Ok(request) = serde_json::from_slice::<MeteredRequest>(&body) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"status":"rejected","error":"invalid_request","delivery_verified":false})),
        );
    };
    // Checked before the commitment names a file.
    if !is_commitment_hex(&request.deal_commitment) {
        return failure(AccessError::Authentication.into());
    }
    match serve(&service, request).await {
        Ok(MeteredReply::Call(call)) => (
            StatusCode::OK,
            Json(json!({
                "status": "issued",
                "call_index": call.call_index,
                "quantity": call.quantity,
                "remaining": call.remaining,
                "repeated": call.repeated,
                "late": call.late,
                "response_hex": hex::encode(&call.body),
                "response_sha256": hex::encode(call.response_sha256),
                "payment_verified": true,
                "delivery_verified": false,
            })),
        ),
        Ok(MeteredReply::Status(status)) => (
            StatusCode::OK,
            Json(json!({
                "status": "allowance",
                "quantity": status.quantity,
                "next_call_index": status.next,
                "remaining": status.remaining,
                "calls": status.calls.iter().map(|call| json!({
                    "call_index": call.call_index,
                    "response_len": call.response_len,
                    "response_sha256": hex::encode(call.response_sha256),
                    "committed_at": call.committed_at,
                    "late": call.late,
                })).collect::<Vec<_>>(),
                "payment_verified": true,
                "delivery_verified": false,
            })),
        ),
        Err(error) => failure(error),
    }
}

/// Authorize, certify finality once if needed, activate, and serve: the steps of
/// [`MeteredIssuer::handle`], with every blocking step off the runtime.
async fn serve<C, S>(
    service: &MeteredService<C, S>,
    request: MeteredRequest,
) -> Result<MeteredReply, MeteredError>
where
    C: PaymentCertifier + Send + Sync + 'static,
    S: ResponseSource + Send + Sync + 'static,
{
    let now = now()?;
    let issuer = Arc::clone(&service.issuer);
    // Request-only checks before the evidence lookup, so their answer and cost never depend on
    // whether the named commitment exists.
    if !super::metered_request_shape_ok(&request, now) {
        return Err(AccessError::Authentication.into());
    }
    let root = service.evidence_root.clone();
    // One answer for every pre-authentication failure: the route is not a commitment oracle.
    let mut authorized: AuthorizedRequest = blocking(move || {
        let evidence = read_evidence(&root, &request.deal_commitment);
        issuer.authorize_loaded(evidence.as_ref(), &request, now)
    })
    .await?;
    // Per-buyer admission, keyed by the key that just authenticated, before any RPC.
    let _buyer_slot = service
        .buyers
        .admit(
            authorized
                .checked()
                .evidence()
                .terms
                .buyer_authorization_key
                .as_bytes(),
            std::time::Instant::now(),
        )
        .ok_or(MeteredError::Busy)?;
    if authorized.needs_certificate() {
        // Pending or unavailable observation returns here, before anything is written.
        let certificate = service.certifier.certify(authorized.checked()).await?;
        let issuer = Arc::clone(&service.issuer);
        authorized = blocking(move || {
            issuer.activate(&authorized, certificate, now)?;
            Ok(authorized)
        })
        .await?;
    }
    let issuer = Arc::clone(&service.issuer);
    let source = Arc::clone(&service.source);
    blocking(move || issuer.serve(&authorized, source.as_ref(), now)).await
}

fn failure(error: MeteredError) -> (StatusCode, Json<Value>) {
    let status = StatusCode::from_u16(error.status()).unwrap_or(StatusCode::SERVICE_UNAVAILABLE);
    let retryable = matches!(error.status(), 202 | 429 | 503);
    let mut body = json!({
        "status": if error == MeteredError::Access(AccessError::Pending) {
            "payment_pending"
        } else if retryable {
            "unavailable"
        } else {
            "rejected"
        },
        "error": error.code(),
        "delivery_verified": false,
    });
    if retryable {
        body["retry_without_payment"] = json!(true);
    }
    (status, Json(body))
}
