//! Immutable snapshot access, or prepaid metered access, authenticated by the buyer and gated
//! by finalized payment.

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
    access::metered::{
        http::{FeedFile, MeteredService},
        DeploymentBinding, MeteredIssuer, MeteredPolicy, MeteredProfile,
    },
    access::x402::{payment_required_header, verify_payment_header, Facilitator},
    access::{
        read_evidence, request_shape_ok, AccessBackend, AccessError, AccessIssuer, AccessPolicy,
        AccessRequest, BuyerLimiter, MAX_RESOURCE_BYTES,
    },
    index_store::{IndexDomain, IndexStore},
    rpc::{PoolRpc, PoolRuntimePins},
};
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
    /// The one resource of a single-resource snapshot or of a metered service.
    #[serde(default)]
    resource: Option<String>,
    #[serde(default)]
    profile: Profile,
    /// Snapshot profile only: the immutable payload of `resource`.
    #[serde(default)]
    payload_file: Option<PathBuf>,
    /// Snapshot profile only, instead of `resource` and `payload_file`: several resources,
    /// each with its own payload and issuance state, routed by the agreement's resource.
    #[serde(default)]
    resources: Option<Vec<ResourceEntry>>,
    /// Metered profile only: the signed response cap of the v1 profile.
    #[serde(default)]
    max_response_bytes: Option<u32>,
    /// Metered profile only: the seller-owned feed, read when a call consumes a new index.
    #[serde(default)]
    feed_file: Option<PathBuf>,
    evidence_root: PathBuf,
    state_root: PathBuf,
    port: u16,
    backend: Backend,
}

/// One snapshot resource of a many-resource service.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceEntry {
    resource: String,
    payload_file: PathBuf,
    /// Keeps this resource's issuance records in `issuance/` itself, where a service that
    /// served only this resource kept them. At most one entry may set it.
    #[serde(default)]
    legacy_issuance: bool,
}

/// Requests the service answers at once; more are told to retry (429).
const MAX_IN_FLIGHT: usize = 16;

/// Most snapshot resources one service may serve.
const MAX_RESOURCES: usize = 64;

#[derive(Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Profile {
    #[default]
    Snapshot,
    Metered,
}

/// Rejects mixed profile fields and metered rails before any chain connection.
fn check_profile(config: &Config) -> Result<(), &'static str> {
    match config.profile {
        Profile::Snapshot => {
            match (&config.resource, &config.payload_file, &config.resources) {
                (Some(_), Some(_), None) => {}
                (None, None, Some(entries)) => {
                    let mut names: Vec<&str> = entries
                        .iter()
                        .map(|entry| entry.resource.as_str())
                        .collect();
                    names.sort_unstable();
                    names.dedup();
                    if entries.is_empty()
                        || entries.len() > MAX_RESOURCES
                        || names.len() != entries.len()
                        || entries.iter().filter(|entry| entry.legacy_issuance).count() > 1
                    {
                        return Err(
                            "resources must be 1 to 64 distinct entries, at most one legacy",
                        );
                    }
                }
                _ => {
                    return Err("snapshot profile requires resource and payload_file, or resources")
                }
            }
            if config.max_response_bytes.is_some() || config.feed_file.is_some() {
                return Err("metered fields require the metered profile");
            }
        }
        Profile::Metered => {
            if matches!(config.backend, Backend::X402Exact { .. }) {
                return Err("metered access cannot use the x402_exact rail");
            }
            if !matches!(config.backend, Backend::Shielded { .. }) {
                return Err("metered access requires the shielded backend");
            }
            if config.payload_file.is_some()
                || config.resources.is_some()
                || config.resource.is_none()
                || config.max_response_bytes.is_none()
                || config.feed_file.is_none()
            {
                return Err("metered profile requires max_response_bytes and feed_file only");
            }
        }
    }
    Ok(())
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
        runtime_keccak256: String,
        poseidon_keccak256: String,
        deposit_verifier_keccak256: String,
        transfer_verifier_keccak256: String,
        withdraw_verifier_keccak256: String,
        #[serde(default = "default_log_concurrency")]
        max_concurrent_queries: u64,
        /// Per-provider request budget, shared by verification reads and background sync.
        #[serde(default)]
        requests_per_second: Option<u32>,
    },
}

/// Parses one lowercase-hex 32-byte deployment pin. A zero pin is never valid.
fn digest(value: &str) -> Result<[u8; 32], &'static str> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("invalid deployment pin");
    }
    let bytes: [u8; 32] = hex::decode(value)
        .map_err(|_| "invalid deployment pin")?
        .try_into()
        .map_err(|_| "invalid deployment pin")?;
    if bytes == [0; 32] {
        return Err("zero deployment pin");
    }
    Ok(bytes)
}

fn default_log_concurrency() -> u64 {
    8
}

#[derive(Clone)]
struct App {
    /// One issuer per served resource, each with its own payload and issuance directory.
    issuers: Arc<Vec<AccessIssuer>>,
    buyers: Arc<BuyerLimiter>,
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
        println!("erebus_access_service: owner-only JSON configuration in EREBUS_ACCESS_CONFIG.\nGET /healthz; POST /v1/access (profile snapshot, the default) or POST /v1/metered (profile metered, shielded backend only) with a short-lived signature from the buyer agreement key.\nListen is loopback-only; place an authenticated TLS gateway in front for remote clients.\nObservation backends cannot submit payments. Explicit x402_exact mode signs and fences one seller-funded transaction for a buyer-authorized permit. Retries observe the persisted transaction without resubmitting.\nStore state persistently for payment and delivery recovery.");
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
    check_profile(&config)?;
    // Requests in flight across both routes; background history sync yields while any are.
    let slots = Arc::new(Semaphore::new(MAX_IN_FLIGHT));
    let seller = KeyBytes::new(config.seller_key).map_err(|_| "invalid seller identity")?;
    let policy = |resource: &str, payload_file: &Path| {
        Ok::<_, &'static str>(AccessPolicy {
            service_id: config.service_id,
            seller: seller.clone(),
            suite_id: config.suite_id,
            resource: resource.to_owned(),
            payload: bounded_file(payload_file, MAX_RESOURCE_BYTES, false)?.to_vec(),
        })
    };
    let open = |root: PathBuf, policy: AccessPolicy| {
        AccessIssuer::open(root, policy).map_err(|_| "access policy or storage unavailable")
    };
    let issuers = match (&config.resource, &config.payload_file, &config.resources) {
        // The single-resource form keeps its issuance records where it always has.
        (Some(resource), Some(payload_file), None) => Some(vec![open(
            config.state_root.join("issuance"),
            policy(resource, payload_file)?,
        )?]),
        (None, None, Some(entries)) => Some(
            entries
                .iter()
                .map(|entry| {
                    let root = if entry.legacy_issuance {
                        config.state_root.join("issuance")
                    } else {
                        config.state_root.join("issuance").join(hex::encode(
                            <sha2::Sha256 as sha2::Digest>::digest(entry.resource.as_bytes()),
                        ))
                    };
                    open(root, policy(&entry.resource, &entry.payload_file)?)
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        _ => None,
    };
    let mut facilitator = None;
    let mut deployment = None;
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
            runtime_keccak256,
            poseidon_keccak256,
            deposit_verifier_keccak256,
            transfer_verifier_keccak256,
            withdraw_verifier_keccak256,
            max_concurrent_queries,
            requests_per_second,
        } => {
            let pool = parse_lowercase_address(&pool).ok_or("invalid pool")?;
            let rate = requests_per_second
                .unwrap_or(erebus_shielded_prover::rpc::DEFAULT_MAX_REQUESTS_PER_SECOND);
            let runtime = PoolRuntimePins {
                pool: digest(&runtime_keccak256)?,
                poseidon: digest(&poseidon_keccak256)?,
                deposit: digest(&deposit_verifier_keccak256)?,
                transfer: digest(&transfer_verifier_keccak256)?,
                withdraw: digest(&withdraw_verifier_keccak256)?,
            };
            deployment = Some(DeploymentBinding {
                chain_id,
                pool,
                first_block,
                first_hash,
                runtime,
            });
            let domain = IndexDomain {
                chain_id,
                pool,
                first_block,
                first_hash,
            };
            let observer = |url: &str, rate: u32| {
                PoolRpc::new(url, chain_id, pool)
                    .and_then(|rpc| rpc.with_max_concurrent_reads(max_concurrent_queries))
                    .and_then(|rpc| rpc.with_max_requests_per_second(rate))
                    .map_err(|_| "invalid pool RPC")
            };
            let rpc = observer(&rpc_url, rate)?;
            let peer_rpc = observer(&peer_rpc_url, rate)?;
            if rpc.shares_endpoint(&peer_rpc) {
                return Err("configure distinct pool observers");
            }
            let checkpoint = erebus_shielded_prover::checkpoint::load(&config.state_root, domain)
                .map_err(|_| "history checkpoint record unavailable")?;
            let store = |name: &str| {
                IndexStore::with_checkpoint(config.state_root.join(name), domain, checkpoint)
                    .map_err(|_| "invalid pool history")
            };
            // Keep both histories current between requests, so a first delivery after a
            // quiet period does not replay the gap inside the request. Each observer catches
            // up on its own task, through its provider's one request budget (shared with the
            // request path), and yields while any request is in flight.
            for (slot, (source, name)) in [
                (rpc.sharing_budget(), "pool-history.json"),
                (peer_rpc.sharing_budget(), "pool-peer-history.json"),
            ]
            .into_iter()
            .enumerate()
            {
                let history = store(name)?;
                let requests = Arc::clone(&slots);
                tokio::spawn(async move {
                    let mut reported = None::<std::time::Instant>;
                    erebus_shielded_prover::recovery::keep_history_current(
                        &source,
                        &history,
                        || requests.available_permits() < MAX_IN_FLIGHT,
                        |error| {
                            // Public diagnostics only (no URL or key is part of these errors),
                            // once a minute per provider, so a stalled sync is visible.
                            if reported.is_none_or(|at| at.elapsed() >= Duration::from_secs(60)) {
                                eprintln!(
                                    "{}",
                                    json!({"event":"history_sync_failed","observer":if slot == 0 { "primary" } else { "peer" },
                                        "error":error.to_string()})
                                );
                                reported = Some(std::time::Instant::now());
                            }
                        },
                    )
                    .await;
                });
            }
            Some(AccessBackend::Shielded {
                rpc: Box::new(rpc),
                peer_rpc: Box::new(peer_rpc),
                index: IndexStore::with_checkpoint(
                    config.state_root.join("pool-history.json"),
                    domain,
                    checkpoint,
                )
                .map_err(|_| "invalid pool history")?,
                peer_index: IndexStore::with_checkpoint(
                    config.state_root.join("pool-peer-history.json"),
                    domain,
                    checkpoint,
                )
                .map_err(|_| "invalid peer pool history")?,
                runtime: Box::new(runtime),
            })
        }
    };
    let submits = facilitator.is_some();
    let routes = match (config.profile, issuers) {
        (Profile::Snapshot, Some(issuers)) => Router::new()
            .route("/v1/access", post(access))
            .with_state(App {
                issuers: Arc::new(issuers),
                buyers: Arc::new(BuyerLimiter::default()),
                backend: backend.map(Arc::new),
                facilitator,
                evidence_root: config.evidence_root,
                slots: Arc::clone(&slots),
            }),
        (Profile::Metered, None) => {
            let backend = backend.ok_or("metered access requires the shielded backend")?;
            let observers = backend
                .shielded_observers()
                .ok_or("metered access requires the shielded backend")?;
            let profile = MeteredProfile::new(config.max_response_bytes.unwrap_or(0))
                .map_err(|_| "invalid metered response cap")?;
            let feed = FeedFile::new(
                config
                    .feed_file
                    .ok_or("metered profile requires feed_file")?,
                &profile,
            );
            let deployment = deployment.ok_or("metered access requires the shielded backend")?;
            let issuer = MeteredIssuer::open(
                config.state_root.join("metered"),
                MeteredPolicy {
                    service_id: config.service_id,
                    seller,
                    suite_id: config.suite_id,
                    resource: config.resource.ok_or("metered profile requires resource")?,
                    profile,
                },
                observers,
                deployment,
            )
            .map_err(|_| "metered policy or storage unavailable")?;
            MeteredService::with_slots(
                issuer,
                Arc::new(backend),
                feed,
                config.evidence_root,
                Arc::clone(&slots),
            )
            .router()
        }
        _ => return Err("invalid access profile"),
    };
    let profile = match config.profile {
        Profile::Snapshot => "snapshot",
        Profile::Metered => "metered",
    };
    let router = Router::new()
        .route(
            "/healthz",
            get(move || async move {
                Json(json!({"status":"ok","profile":profile,"payment_submission":submits}))
            }),
        )
        .merge(routes)
        .layer(DefaultBodyLimit::max(8192));
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
    // Before authentication every failure is one answer: this route is not a commitment
    // oracle. `read_evidence` refuses a commitment that is not lowercase hex before it names
    // a file, and a miss spends decoy work comparable to a real bad signature.
    // The agreement's signed resource selects the issuer. Evidence for a resource this
    // service does not serve is a miss like any other, answered by the decoy.
    let loader = app.clone();
    let checked_request = request.clone();
    let selected = tokio::task::spawn_blocking(move || {
        let now = now()?;
        // Request-only checks before the evidence lookup: their answer and cost never depend on
        // whether the named commitment exists.
        if !request_shape_ok(&checked_request.nonce, checked_request.expires_at, now) {
            return Err(AccessError::Authentication);
        }
        let evidence = read_evidence(&loader.evidence_root, &checked_request.deal_commitment);
        let index = evidence.as_ref().and_then(|evidence| {
            loader
                .issuers
                .iter()
                .position(|issuer| issuer.policy().resource == evidence.terms.service.resource)
        });
        let issuer = &loader.issuers[index.unwrap_or(0)];
        let evidence = evidence.filter(|_| index.is_some());
        issuer.authenticate_loaded(evidence.as_ref(), &checked_request, now)?;
        Ok::<_, AccessError>((
            evidence.ok_or(AccessError::Authentication)?,
            index.unwrap_or(0),
        ))
    })
    .await;
    let (evidence, index) = match selected {
        Ok(Ok(selected)) => selected,
        Ok(Err(failure)) => return error(failure, false),
        Err(_) => return error(AccessError::Storage, false),
    };
    // Per-buyer admission, keyed by the key that just authenticated, before any RPC.
    let Some(_buyer_slot) = app.buyers.admit(
        evidence.terms.buyer_authorization_key.as_bytes(),
        std::time::Instant::now(),
    ) else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            HeaderMap::new(),
            Json(json!({"status":"busy","retry_without_payment":true})),
        );
    };
    let issuer = Arc::clone(&app.issuers);
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
                .settle_and_verify(
                    issuer[index].policy(),
                    evidence,
                    payment,
                    now().unwrap_or(0),
                )
                .await
        }
        (None, Some(backend), None) => backend.verify(issuer[index].policy(), evidence).await,
        _ => Err(AccessError::Agreement),
    };
    let paid = match verified {
        Ok(paid) => paid,
        Err(failure) => return error(failure, false),
    };
    let issuers = Arc::clone(&issuer);
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
    let issued =
        tokio::task::spawn_blocking(move || issuers[index].issue(&paid, &request, now()?)).await;
    match issued {
        Ok(Ok(issuance)) => (
            StatusCode::OK,
            response_headers,
            Json(
                json!({"status":"issued","issuance":issuance,"payload_hex":hex::encode(&issuer[index].policy().payload),"payment_verified":true,"delivery_verified":false}),
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
