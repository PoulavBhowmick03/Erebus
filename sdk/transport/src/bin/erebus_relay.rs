//! `erebus-relay`: a minimal ciphertext mailbox service (decision DM2-5).
//!
//! The relay stores opaque Noise ciphertext for a mailbox id. It never holds a session key and
//! cannot read a message. Hosted and self-hosted deployments run this same binary with different
//! configuration (decision D05).
//!
//! Configuration is read from the environment:
//!
//! | Variable | Default | Meaning |
//! |---|---|---|
//! | `EREBUS_RELAY_ROOT` | `./relay-data` | Persistent storage directory |
//! | `EREBUS_RELAY_PORT` | `8080` | Listen port |
//! | `EREBUS_RELAY_TOKEN` | unset | Required bearer token; unset refuses to start unless `EREBUS_RELAY_INSECURE=1` |
//! | `EREBUS_RELAY_RETENTION` | 7 days | Retention in seconds |
//!
//! Endpoints: `GET /healthz`, `POST /v1/mailbox/{id}`, `GET /v1/mailbox/{id}?after=<cursor>`.
//! Operational logs never contain a blob or a mailbox id.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use erebus_transport::limits::{MAX_BLOB_BYTES, RELAY_RETENTION_SECONDS};
use erebus_transport::relay::{FileRelay, MailboxId, Relay, RelayError};
use serde::Deserialize;
use serde_json::json;

#[derive(Clone)]
struct AppState {
    relay: Arc<FileRelay>,
    token: Option<String>,
}

#[tokio::main]
async fn main() {
    let root = std::env::var("EREBUS_RELAY_ROOT").unwrap_or_else(|_| "./relay-data".to_owned());
    let port: u16 = std::env::var("EREBUS_RELAY_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(8080);
    let retention = std::env::var("EREBUS_RELAY_RETENTION")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(RELAY_RETENTION_SECONDS);
    let token = std::env::var("EREBUS_RELAY_TOKEN").ok();
    let auth_enabled = token.is_some();
    let insecure = std::env::var("EREBUS_RELAY_INSECURE").is_ok_and(|value| value == "1");

    if token.is_none() && !insecure {
        eprintln!(
            "erebus-relay: refusing to start without EREBUS_RELAY_TOKEN. \
             Set a token, or set EREBUS_RELAY_INSECURE=1 for a local-only run."
        );
        std::process::exit(2);
    }
    if token.is_none() {
        eprintln!("erebus-relay: WARNING running without authentication (EREBUS_RELAY_INSECURE=1)");
    }

    let relay = match FileRelay::with_retention(&root, retention) {
        Ok(relay) => Arc::new(relay),
        Err(error) => {
            eprintln!("erebus-relay: cannot open storage at {root}: {error}");
            std::process::exit(1);
        }
    };

    let state = AppState {
        relay: Arc::clone(&relay),
        token,
    };
    let app = Router::new()
        .route("/healthz", get(health))
        .route("/v1/mailbox/{id}", post(put_blob).get(get_blobs))
        .route_layer(middleware::from_fn_with_state(state.clone(), authorize))
        .layer(DefaultBodyLimit::max(MAX_BLOB_BYTES))
        .with_state(state);

    let address = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = match tokio::net::TcpListener::bind(address).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("erebus-relay: cannot bind {address}: {error}");
            std::process::exit(1);
        }
    };
    eprintln!(
        "erebus-relay: listening on {address}, retention {retention}s, storage {root}, auth {}",
        if auth_enabled { "on" } else { "off" }
    );
    if let Err(error) = axum::serve(listener, app).await {
        eprintln!("erebus-relay: server error: {error}");
        std::process::exit(1);
    }
}

async fn authorize(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if let Some(token) = &state.token {
        let expected = format!("Bearer {token}");
        let presented = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok());
        if presented != Some(expected.as_str()) {
            return Err(StatusCode::UNAUTHORIZED);
        }
    }
    Ok(next.run(request).await)
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "retention_seconds": state.relay.retention_seconds(),
    }))
}

async fn put_blob(
    State(state): State<AppState>,
    Path(mailbox): Path<String>,
    body: Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mailbox = MailboxId::from_hex(&mailbox)?;
    let receipt = state.relay.put(&mailbox, &body, now())?;
    Ok(Json(json!({
        "cursor": receipt.cursor,
        "expires_at": receipt.expires_at,
        "duplicate": receipt.duplicate,
    })))
}

#[derive(Deserialize)]
struct ReadQuery {
    #[serde(default)]
    after: u64,
}

async fn get_blobs(
    State(state): State<AppState>,
    Path(mailbox): Path<String>,
    Query(query): Query<ReadQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mailbox = MailboxId::from_hex(&mailbox)?;
    let blobs = state.relay.get(&mailbox, query.after, now())?;
    let blobs: Vec<serde_json::Value> = blobs
        .into_iter()
        .map(|blob| {
            json!({
                "cursor": blob.cursor,
                "expires_at": blob.expires_at,
                "blob": hex::encode(blob.blob),
            })
        })
        .collect();
    Ok(Json(json!({ "blobs": blobs })))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

struct ApiError {
    status: StatusCode,
    message: String,
}

impl From<RelayError> for ApiError {
    fn from(error: RelayError) -> Self {
        let status = match error {
            RelayError::BlobTooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            RelayError::MailboxFull(_) => StatusCode::INSUFFICIENT_STORAGE,
            RelayError::InvalidMailbox => StatusCode::BAD_REQUEST,
            RelayError::Io(_) | RelayError::Corrupt(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self {
            status,
            message: error.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}
