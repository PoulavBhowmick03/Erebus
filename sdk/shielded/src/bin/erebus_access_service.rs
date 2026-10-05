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
    http::{HeaderMap, HeaderValue, StatusCode},
    routing::{get, post},
    Json, Router,
};
use erebus_core::ids::{ChainNamespace, KeyBytes};
use erebus_evm::{
    chain::{Eip1559Fees, EvmChain, ObservationJournal, ObservationLimits, TransactionKey},
    deployment::{parse_lowercase_address, EvmDeployment},
    x402::EXACT_PERMIT2_PROXY,
};
use erebus_shielded_prover::{
    access::x402::{payment_required_header, verify_payment_header, Facilitator},
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
    X402Exact {
        namespace: String,
        rpc_url: String,
        peer_rpc_url: String,
        permit2_runtime_hash: [u8; 32],
        proxy_runtime_hash: [u8; 32],
        transaction_key_file: PathBuf,
        signer_journal_root: PathBuf,
        gas_limit: u64,
        max_fee_per_gas: String,
        max_priority_fee_per_gas: String,
    },
    PublicBound {
        namespace: String,
        settlement_contract: String,
        verifier_version: u32,
        rpc_url: String,
        from_block: u64,
        log_block_range: u64,
        max_log_queries: u64,
        max_ancestry: u64,
        #[serde(default = "default_log_concurrency")]
        max_concurrent_queries: u64,
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

fn default_log_concurrency() -> u64 {
    8
}

#[derive(Clone)]
struct App {
    issuer: Arc<AccessIssuer>,
    backend: Option<Arc<AccessBackend>>,
    facilitator: Option<Arc<Facilitator>>,
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
        println!("erebus_access_service: owner-only JSON configuration in EREBUS_ACCESS_CONFIG.\nGET /healthz; POST /v1/access with a short-lived signature from the buyer agreement key.\nListen is loopback-only; place an authenticated TLS gateway in front for remote clients.\nObservation backends cannot submit payments. Explicit x402_exact mode signs and fences one seller-funded transaction for a buyer-authorized permit. Retries observe the persisted transaction without resubmitting.\nStore state persistently for payment and delivery recovery.");
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
    let mut facilitator = None;
    let backend = match config.backend {
        Backend::X402Exact {
            namespace,
            rpc_url,
            peer_rpc_url,
            permit2_runtime_hash,
            proxy_runtime_hash,
            transaction_key_file,
            signer_journal_root,
            gas_limit,
            max_fee_per_gas,
            max_priority_fee_per_gas,
        } => {
            let deployment = |url| {
                EvmDeployment::new(
                    ChainNamespace::parse(&namespace).map_err(|_| "invalid chain")?,
                    EXACT_PERMIT2_PROXY,
                    1,
                    url,
                )
                .map_err(|_| "invalid x402 deployment")
            };
            let primary = EvmChain::connect(deployment(rpc_url)?, Duration::from_secs(15))
                .await
                .map_err(|_| "chain unavailable")?;
            let peer = EvmChain::connect(deployment(peer_rpc_url)?, Duration::from_secs(15))
                .await
                .map_err(|_| "peer unavailable")?;
            // Distinct URLs are configuration hygiene, not proof of independent providers.
            primary
                .verified_finalized_nonce_agreed(&peer, [0; 20])
                .await
                .map_err(|_| "configure consistent distinct observers")?;
            primary
                .authenticate_x402_runtimes(permit2_runtime_hash, proxy_runtime_hash)
                .await
                .map_err(|_| "x402 runtime authentication failed")?;
            peer.authenticate_x402_runtimes(permit2_runtime_hash, proxy_runtime_hash)
                .await
                .map_err(|_| "peer runtime authentication failed")?;
            let bytes = bounded_file(&transaction_key_file, 32, true)?;
            let key = TransactionKey::from_bytes(
                bytes.as_slice().try_into().map_err(|_| "invalid gas key")?,
            )
            .map_err(|_| "invalid gas key")?;
            let fees = Eip1559Fees::new(
                max_fee_per_gas.parse().map_err(|_| "invalid fee cap")?,
                max_priority_fee_per_gas
                    .parse()
                    .map_err(|_| "invalid priority fee")?,
            )
            .map_err(|_| "invalid fee policy")?;
            facilitator = Some(Arc::new(
                Facilitator::open(
                    primary,
                    peer,
                    key,
                    config.state_root.join("x402-payments"),
                    signer_journal_root,
                    fees,
                    gas_limit,
                )
                .map_err(|_| "x402 state unavailable")?,
            ));
            None
        }
        Backend::PublicBound {
            namespace,
            settlement_contract,
            verifier_version,
            rpc_url,
            from_block,
            log_block_range,
            max_log_queries,
            max_ancestry,
            max_concurrent_queries,
        } => {
            if log_block_range == 0
                || log_block_range > 2000
                || max_log_queries == 0
                || max_log_queries > 1024
                || max_ancestry == 0
                || max_ancestry > 8192
                || max_concurrent_queries == 0
                || max_concurrent_queries > erebus_evm::chain::MAX_CONCURRENT_LOG_QUERIES
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
            Some(AccessBackend::PublicBound {
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
                    max_concurrent_queries,
                },
            })
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
            Some(AccessBackend::Shielded {
                rpc: Box::new(rpc),
                peer_rpc: Box::new(peer_rpc),
                index: IndexStore::new(config.state_root.join("pool-history.json"), domain)
                    .map_err(|_| "invalid pool history")?,
                peer_index: IndexStore::new(
                    config.state_root.join("pool-peer-history.json"),
                    domain,
                )
                .map_err(|_| "invalid peer pool history")?,
            })
        }
    };
    let app = App {
        issuer: Arc::new(issuer),
        backend: backend.map(Arc::new),
        facilitator,
        evidence_root: config.evidence_root,
        slots: Arc::new(Semaphore::new(16)),
    };
    let router = Router::new()
        .route(
            "/healthz",
            get({
                let submits = app.facilitator.is_some();
                move || async move { Json(json!({"status":"ok","payment_submission":submits})) }
            }),
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
    headers: HeaderMap,
    Json(request): Json<AccessRequest>,
) -> (StatusCode, HeaderMap, Json<Value>) {
    let Ok(_slot) = app.slots.try_acquire() else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            HeaderMap::new(),
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
    let network = evidence.terms.domain.namespace.to_string();
    let payer = hex::encode(evidence.terms.buyer_authorization_key.as_bytes());
    if app.facilitator.is_some() {
        if let Some(payment) = &request.payment {
            let header = headers
                .get("PAYMENT-SIGNATURE")
                .and_then(|value| value.to_str().ok());
            if header
                .is_none_or(|value| verify_payment_header(value, &evidence.terms, payment).is_err())
            {
                return error(AccessError::Authentication, false);
            }
        } else {
            let Ok(required) = payment_required_header(&evidence.terms) else {
                return error(AccessError::Agreement, false);
            };
            let mut response_headers = HeaderMap::new();
            let Ok(required) = HeaderValue::from_str(&required) else {
                return error(AccessError::Agreement, false);
            };
            response_headers.insert("PAYMENT-REQUIRED", required);
            return (
                StatusCode::PAYMENT_REQUIRED,
                response_headers,
                Json(
                    json!({"x402Version":2,"error":"payment_required","retry_without_new_payment":true}),
                ),
            );
        }
    } else if headers.contains_key("PAYMENT-SIGNATURE") || request.payment.is_some() {
        return error(AccessError::Agreement, false);
    }
    let verified = match (&app.facilitator, &app.backend, &request.payment) {
        (Some(facilitator), None, Some(payment)) => {
            facilitator
                .settle_and_verify(app.issuer.policy(), evidence, payment, now().unwrap_or(0))
                .await
        }
        (None, Some(backend), None) => backend.verify(app.issuer.policy(), evidence).await,
        _ => Err(AccessError::Agreement),
    };
    let paid = match verified {
        Ok(paid) => paid,
        Err(failure) => return error(failure, false),
    };
    let issuer = app.issuer.clone();
    let mut response_headers = HeaderMap::new();
    if let Some(facilitator) = &app.facilitator {
        let Ok(hash) = facilitator.transaction_hash(&request.deal_commitment) else {
            return error(AccessError::Storage, true);
        };
        use base64::{engine::general_purpose::STANDARD, Engine};
        let encoded = STANDARD.encode(
            json!({"success":true,"transaction":format!("0x{}",hex::encode(hash)),
            "network":network,"payer":format!("0x{payer}")})
            .to_string(),
        );
        let Ok(value) = HeaderValue::from_str(&encoded) else {
            return error(AccessError::Storage, true);
        };
        response_headers.insert("PAYMENT-RESPONSE", value);
    }
    let issued = tokio::task::spawn_blocking(move || issuer.issue(&paid, &request, now()?)).await;
    match issued {
        Ok(Ok(issuance)) => (
            StatusCode::OK,
            response_headers,
            Json(
                json!({"status":"issued","issuance":issuance,"payload_hex":hex::encode(&app.issuer.policy().payload),"payment_verified":true,"delivery_verified":false}),
            ),
        ),
        Ok(Err(failure)) => error(failure, true),
        Err(_) => error(AccessError::Storage, true),
    }
}

fn error(failure: AccessError, paid: bool) -> (StatusCode, HeaderMap, Json<Value>) {
    let status = match failure {
        AccessError::Authentication => StatusCode::UNAUTHORIZED,
        AccessError::Agreement => StatusCode::BAD_REQUEST,
        AccessError::Pending => StatusCode::ACCEPTED,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    (
        status,
        HeaderMap::new(),
        Json(
            json!({"status":if paid {"paid_but_undelivered"} else if failure==AccessError::Pending {"payment_pending"} else {"unavailable"},"payment_verified":paid,"delivery_verified":false,"retry_without_payment":true,"error":failure.to_string()}),
        ),
    )
}
