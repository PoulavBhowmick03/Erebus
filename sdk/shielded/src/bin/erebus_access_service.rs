//! Immutable snapshot access, authenticated by the buyer and gated by finalized payment.

use std::{
    io::Read,
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use erebus_core::ids::{ChainNamespace, KeyBytes};
use erebus_evm::{
    chain::{EvmChain, ObservationJournal, ObservationLimits},
    deployment::{parse_lowercase_address, EvmDeployment},
};
use erebus_shielded_prover::{
    access::{
        AccessBackend, AccessError, AccessIssuer, AccessPolicy, AccessRequest, MAX_RESOURCE_BYTES,
    },
    index_store::{IndexDomain, IndexStore},
    rpc::PoolRpc,
};
use erebus_transport::disclosure::{SelectedAgreement, MAX_DISCLOSURE_BYTES};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Semaphore;
use zeroize::Zeroizing;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    service_id: [u8; 32],
    seller_key: Vec<u8>,
    suite_id: u16,
    resource: String,
    payload_file: PathBuf,
    evidence_root: PathBuf,
    state_root: PathBuf,
    port: u16,
    backend: Backend,
}

#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum Backend {
    PublicBound {
        namespace: String,
        settlement_contract: String,
        verifier_version: u32,
        rpc_url: String,
        from_block: u64,
        log_block_range: u64,
        max_log_queries: u64,
        max_ancestry: u64,
    },
    Shielded {
        chain_id: u64,
        pool: String,
        rpc_url: String,
        peer_rpc_url: String,
        first_block: u64,
        first_hash: [u8; 32],
    },
}

#[derive(Clone)]
struct App {
    issuer: Arc<AccessIssuer>,
    backend: Arc<AccessBackend>,
    evidence_root: PathBuf,
    slots: Arc<Semaphore>,
}

fn bounded_file(
    path: &Path,
    limit: usize,
    private: bool,
) -> Result<Zeroizing<Vec<u8>>, &'static str> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| "local input unavailable")?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return Err("local input unavailable");
    }
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("private input must be owner-only");
        }
    }
    let mut bytes = Zeroizing::new(Vec::new());
    std::fs::File::open(path)
        .map_err(|_| "local input unavailable")?
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "local input unavailable")?;
    if bytes.len() > limit {
        return Err("local input unavailable");
    }
    Ok(bytes)
}

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("--help") {
        println!("erebus_access_service: owner-only JSON configuration in EREBUS_ACCESS_CONFIG.\nGET /healthz; POST /v1/access with a short-lived signature from the buyer agreement key.\nListen is loopback-only; place an authenticated TLS gateway in front for remote clients.\nNo endpoint signs, submits, or repeats payments. Store state persistently for delivery recovery.");
        return;
    }
    if let Err(error) = run().await {
        eprintln!("erebus_access_service: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), &'static str> {
    let path = std::env::var_os("EREBUS_ACCESS_CONFIG").ok_or("configure EREBUS_ACCESS_CONFIG")?;
    let bytes = bounded_file(Path::new(&path), 16384, true)?;
    let config: Config =
        serde_json::from_slice(&bytes).map_err(|_| "invalid access configuration")?;
    let metadata = std::fs::symlink_metadata(&config.evidence_root)
        .map_err(|_| "evidence directory unavailable")?;
    if !metadata.is_dir() {
        return Err("evidence directory must be a real directory");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("evidence directory must be owner-only");
        }
    }
    let payload = bounded_file(&config.payload_file, MAX_RESOURCE_BYTES, false)?.to_vec();
    let issuer = AccessIssuer::open(
        config.state_root.join("issuance"),
        AccessPolicy {
            service_id: config.service_id,
            seller: KeyBytes::new(config.seller_key).map_err(|_| "invalid seller identity")?,
            suite_id: config.suite_id,
            resource: config.resource,
            payload,
        },
    )
    .map_err(|_| "access policy or storage unavailable")?;
    let backend = match config.backend {
        Backend::PublicBound {
            namespace,
            settlement_contract,
            verifier_version,
            rpc_url,
            from_block,
            log_block_range,
            max_log_queries,
            max_ancestry,
        } => {
            if log_block_range == 0
                || log_block_range > 2000
                || max_log_queries == 0
                || max_log_queries > 1024
                || max_ancestry == 0
                || max_ancestry > 8192
            {
                return Err("invalid observer budget");
            }
            let deployment = EvmDeployment::new(
                ChainNamespace::parse(&namespace).map_err(|_| "invalid chain")?,
                parse_lowercase_address(&settlement_contract)
                    .ok_or("invalid settlement contract")?,
                verifier_version,
                rpc_url,
            )
            .map_err(|_| "invalid public deployment")?;
            AccessBackend::PublicBound {
                chain: EvmChain::connect(deployment, Duration::from_secs(15))
                    .await
                    .map_err(|_| "chain unavailable")?,
                journal: ObservationJournal::open(config.state_root.join("public-history"))
                    .map_err(|_| "history cache unavailable")?,
                from_block,
                limits: ObservationLimits {
                    log_block_range,
                    max_log_queries,
                    max_ancestry,
                },
            }
        }
        Backend::Shielded {
            chain_id,
            pool,
            rpc_url,
            peer_rpc_url,
            first_block,
            first_hash,
        } => {
            let pool = parse_lowercase_address(&pool).ok_or("invalid pool")?;
            let domain = IndexDomain {
                chain_id,
                pool,
                first_block,
                first_hash,
            };
            let rpc = PoolRpc::new(&rpc_url, chain_id, pool).map_err(|_| "invalid pool RPC")?;
            let peer_rpc =
                PoolRpc::new(&peer_rpc_url, chain_id, pool).map_err(|_| "invalid peer pool RPC")?;
            if rpc.shares_endpoint(&peer_rpc) {
                return Err("configure distinct pool observers");
            }
            AccessBackend::Shielded {
                rpc: Box::new(rpc),
                peer_rpc: Box::new(peer_rpc),
                index: IndexStore::new(config.state_root.join("pool-history.json"), domain)
                    .map_err(|_| "invalid pool history")?,
                peer_index: IndexStore::new(
                    config.state_root.join("pool-peer-history.json"),
                    domain,
                )
                .map_err(|_| "invalid peer pool history")?,
            }
        }
    };
    let app = App {
        issuer: Arc::new(issuer),
        backend: Arc::new(backend),
        evidence_root: config.evidence_root,
        slots: Arc::new(Semaphore::new(16)),
    };
    let router = Router::new()
        .route(
            "/healthz",
            get(|| async { Json(json!({"status":"ok","payment_submission":false})) }),
        )
        .route("/v1/access", post(access))
        .layer(DefaultBodyLimit::max(8192))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind((IpAddr::V4(Ipv4Addr::LOCALHOST), config.port))
        .await
        .map_err(|_| "cannot bind access service")?;
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(|_| "access service stopped")
}

fn now() -> Result<u64, AccessError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs())
        .map_err(|_| AccessError::Authentication)
}

async fn access(
    State(app): State<App>,
    Json(request): Json<AccessRequest>,
) -> (StatusCode, Json<Value>) {
    let Ok(_slot) = app.slots.try_acquire() else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"status":"busy","retry_without_payment":true})),
        );
    };
    if request.deal_commitment.len() != 64
        || !request
            .deal_commitment
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return error(AccessError::Authentication, false);
    }
    let loader = app.clone();
    let checked_request = request.clone();
    let selected = tokio::task::spawn_blocking(move || {
        let bytes = bounded_file(
            &loader
                .evidence_root
                .join(format!("{}.evidence", checked_request.deal_commitment)),
            MAX_DISCLOSURE_BYTES,
            true,
        )
        .map_err(|_| AccessError::Agreement)?;
        let evidence = SelectedAgreement::decode(&bytes).map_err(|_| AccessError::Agreement)?;
        loader
            .issuer
            .authenticate(&evidence, &checked_request, now()?)?;
        Ok::<_, AccessError>(evidence)
    })
    .await;
    let evidence = match selected {
        Ok(Ok(evidence)) => evidence,
        Ok(Err(failure)) => return error(failure, false),
        Err(_) => return error(AccessError::Storage, false),
    };
    let paid = match app.backend.verify(app.issuer.policy(), evidence).await {
        Ok(paid) => paid,
        Err(failure) => return error(failure, false),
    };
    let issuer = app.issuer.clone();
    let issued = tokio::task::spawn_blocking(move || issuer.issue(&paid, &request, now()?)).await;
    match issued {
        Ok(Ok(issuance)) => (
            StatusCode::OK,
            Json(
                json!({"status":"issued","issuance":issuance,"payload_hex":hex::encode(&app.issuer.policy().payload),"payment_verified":true,"delivery_verified":false}),
            ),
        ),
        Ok(Err(failure)) => error(failure, true),
        Err(_) => error(AccessError::Storage, true),
    }
}

fn error(failure: AccessError, paid: bool) -> (StatusCode, Json<Value>) {
    let status = match failure {
        AccessError::Authentication => StatusCode::UNAUTHORIZED,
        AccessError::Agreement => StatusCode::BAD_REQUEST,
        AccessError::Pending => StatusCode::ACCEPTED,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    (
        status,
        Json(
            json!({"status":if paid {"paid_but_undelivered"} else if failure==AccessError::Pending {"payment_pending"} else {"unavailable"},"payment_verified":paid,"delivery_verified":false,"retry_without_payment":true,"error":failure.to_string()}),
        ),
    )
}
