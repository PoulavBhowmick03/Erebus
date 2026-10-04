//! Shared local disclosure command protocol, with backend-specific payment verification.

use std::fs::File;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::chain::{EvmChain, ObservationJournal, ObservationLimits};
use crate::deployment::{parse_lowercase_address, EvmDeployment};
use crate::disclosure::{
    open_public_bound_disclosure, verify_public_bound_disclosure_resumable, DisclosureObservation,
};
use crate::x402::{decode_settle_call, verify_x402_exact, EXACT_PERMIT2_PROXY};
use erebus_core::ids::ChainNamespace;
use erebus_core::terms::SettlementMode;
use erebus_transport::disclosure::{
    verify_selected_agreement, DisclosureGrant, SelectedAgreement, VerifiedAgreement,
    MAX_DISCLOSURE_BYTES, MAX_GRANT_BYTES,
};
use erebus_transport::identity::{AuthorizationIdentity, DisclosureIdentity};
use erebus_transport::store::FileTranscriptStore;
use serde::Deserialize;
use serde_json::{json, Value};
use zeroize::Zeroizing;

const MAX_REQUEST_BYTES: usize = 64 * 1024;

#[derive(Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Version,
    Keygen {
        key_file: PathBuf,
    },
    KeyInfo {
        key_file: PathBuf,
    },
    Select {
        state_root: PathBuf,
        operation_ref: String,
        store_root: PathBuf,
        namespace: String,
        transcript_hash_version: u16,
        evidence_file: PathBuf,
    },
    Export {
        evidence_file: PathBuf,
        issuer_key_file: PathBuf,
        recipient_public_key: String,
        grant_file: PathBuf,
        expires_at: u64,
    },
    VerifyAgreement {
        grant_file: PathBuf,
        key_file: PathBuf,
        expected_issuer: String,
    },
    VerifyPayment {
        grant_file: PathBuf,
        key_file: PathBuf,
        expected_issuer: String,
        deployment: Value,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Deployment {
    namespace: String,
    settlement_contract: String,
    verifier_version: u32,
    rpc_url: String,
    /// First block of the settlement contract's history. Live chains are far taller than a
    /// public RPC's `eth_getLogs` range cap, so a genesis scan is not possible; pass the
    /// deployment block. Zero preserves the legacy genesis scan for local chains.
    #[serde(default)]
    from_block: u64,
    /// Blocks per `eth_getLogs` query. Some public RPCs cap the range (Monad's public
    /// endpoint allows 100); lower it when the provider rejects wider ranges.
    #[serde(default = "default_log_block_range")]
    log_block_range: u64,
    #[serde(default = "default_log_queries")]
    max_log_queries: u64,
    #[serde(default = "default_ancestry")]
    max_ancestry: u64,
    #[serde(default)]
    cache_root: Option<PathBuf>,
}

fn default_log_block_range() -> u64 {
    2_000
}

fn default_log_queries() -> u64 {
    8
}

fn default_ancestry() -> u64 {
    64
}

const HELP: &str = "erebus-disclosure: one public-bound disclosure request as JSON on stdin.
Methods: version, keygen, key_info, select, export, verify_agreement, verify_payment.
Private keys and selected evidence are read from owner-only local files.
select rebuilds canonical SelectedAgreement evidence from a participant's durable coordinator
state and transcript store; it writes a new owner-only evidence file.
export requires canonical SelectedAgreement bytes and the participant's raw 32-byte issuer key.
verify_agreement is offline and does not establish payment.
verify_payment requires an independently configured deployment and finalized RPC evidence.
Public-bound verification persists public history in deployment.cache_root (default:
public-cache beside the grant). Pending verification exits 2; repeat the same request to resume.
Output contains verification status and deal identifiers, not plaintext terms or private keys.
No method submits transactions or generates proofs. Version-2 grants use direct suite-2 signing.
Shielded payment verification requires the erebus-shielded-disclosure command.
See docs/metropolis-m7-runbook.md for request schemas and recovery boundaries.";

/// Runs the bounded JSON command protocol with an independent payment verifier.
pub async fn run<F, Fut>(verify_payment: F)
where
    F: FnOnce(PaymentRequest) -> Fut,
    Fut: std::future::Future<Output = Result<PaymentVerification, &'static str>>,
{
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if matches!(arguments.as_slice(), [argument] if argument == "--help") {
        println!("{HELP}");
        return;
    }
    if !arguments.is_empty() {
        finish(Err("invalid arguments"));
    }
    let mut input = Zeroizing::new(Vec::new());
    if std::io::stdin()
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut input)
        .is_err()
    {
        finish(Err("cannot read request"));
    }
    if input.len() > MAX_REQUEST_BYTES {
        finish(Err("request exceeds size limit"));
    }
    let request = match serde_json::from_slice(&input) {
        Ok(request) => request,
        Err(_) => finish(Err("invalid request")),
    };
    finish(handle(request, verify_payment).await);
}

fn finish(result: Result<Value, &'static str>) -> ! {
    let failed = result.is_err();
    let response = result.unwrap_or_else(|error| json!({"status": "error", "error": error}));
    let pending = response["status"] == "pending";
    println!("{response}");
    if std::io::stdout().flush().is_err() {
        std::process::exit(1);
    }
    std::process::exit(if pending { 2 } else { i32::from(failed) });
}

async fn handle<F, Fut>(request: Request, verify_payment: F) -> Result<Value, &'static str>
where
    F: FnOnce(PaymentRequest) -> Fut,
    Fut: std::future::Future<Output = Result<PaymentVerification, &'static str>>,
{
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock unavailable")?
        .as_secs();
    match request {
        Request::Version => Ok(json!({
            "status": "ok", "protocol": 1,
            "methods": ["version", "keygen", "key_info", "select", "export", "verify_agreement", "verify_payment"],
        })),
        Request::Keygen { key_file } => {
            let recipient = DisclosureIdentity::generate_and_store(&key_file)
                .map_err(|_| "cannot create disclosure key")?;
            let parent = key_file
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|_| "cannot persist disclosure key directory")?;
            Ok(json!({"status": "ok", "recipient_public_key": hex::encode(recipient.public_key())}))
        }
        Request::KeyInfo { key_file } => {
            let recipient = load_recipient(&key_file)?;
            Ok(json!({"status": "ok", "recipient_public_key": hex::encode(recipient.public_key())}))
        }
        Request::Select {
            state_root,
            operation_ref,
            store_root,
            namespace,
            transcript_hash_version,
            evidence_file,
        } => {
            let operation_ref = parse_operation_ref(&operation_ref)?;
            let (terms, blinding, buyer, seller) =
                erebus_coordinator::read_disclosure_opening(&state_root, operation_ref)
                    .map_err(|_| "cannot read durable agreement opening")?;
            let store = FileTranscriptStore::open(&store_root)
                .map_err(|_| "cannot open participant transcript store")?;
            let selected = SelectedAgreement::from_store(
                terms,
                blinding,
                buyer,
                seller,
                transcript_hash_version,
                &store,
                &namespace,
            )
            .map_err(|_| "cannot select a verified agreement from participant storage")?;
            let agreement = verify_selected_agreement(&selected)
                .map_err(|_| "selected agreement did not verify")?;
            let bytes = Zeroizing::new(
                selected
                    .encode()
                    .map_err(|_| "cannot encode selected evidence")?,
            );
            write_owner_only_new(&evidence_file, &bytes)?;
            Ok(json!({
                "status": "ok",
                "evidence_saved": true,
                "deal_id": hex::encode(agreement.deal_id),
                "revision": agreement.revision,
                "deal_commitment": agreement.commitment.to_hex(),
                "deal_nullifier": agreement.nullifier.to_hex(),
            }))
        }
        Request::Export {
            evidence_file,
            issuer_key_file,
            recipient_public_key,
            grant_file,
            expires_at,
        } => {
            let bytes = read_private_file(&evidence_file, MAX_DISCLOSURE_BYTES)?;
            let evidence =
                SelectedAgreement::decode(&bytes).map_err(|_| "invalid selected evidence")?;
            let agreement =
                verify_selected_agreement(&evidence).map_err(|_| "invalid selected evidence")?;
            let key = read_private_file(&issuer_key_file, 32)?;
            let recipient = public_key(&recipient_public_key)?;
            let grant = match (evidence.terms.suite_id, evidence.terms.settlement_mode) {
                (1, SettlementMode::PublicBound) => {
                    let issuer = AuthorizationIdentity::from_bytes(&key)
                        .map_err(|_| "invalid issuer key")?;
                    if evidence.terms.buyer_authorization_key.as_bytes() != issuer.address()
                        && evidence.terms.seller_authorization_key.as_bytes() != issuer.address()
                    {
                        return Err("issuer is not an agreement participant");
                    }
                    DisclosureGrant::seal(&evidence, &issuer, recipient, expires_at, now)
                }
                (2, SettlementMode::Shielded) => {
                    let seed = Zeroizing::new(
                        <[u8; 32]>::try_from(key.as_slice()).map_err(|_| "invalid issuer seed")?,
                    );
                    DisclosureGrant::seal_shielded(&evidence, &seed, recipient, expires_at, now)
                }
                _ => return Err("unsupported disclosure suite or mode"),
            }
            .map_err(|_| "cannot create participant disclosure grant")?;
            grant
                .write_backup(&grant_file)
                .map_err(|_| "cannot save disclosure grant")?;
            Ok(json!({
                "status": "ok",
                "grant_saved": true,
                "deal_id": hex::encode(agreement.deal_id),
                "issuer": format!("0x{}", hex::encode(&grant.issuer)),
            }))
        }
        Request::VerifyAgreement {
            grant_file,
            key_file,
            expected_issuer,
        } => {
            let (grant, recipient, issuer) = load_grant(&grant_file, &key_file, &expected_issuer)?;
            let (evidence, agreement) = open_checked(&grant, &recipient, &issuer, now)?;
            Ok(verification_response(
                agreement,
                evidence.terms.settlement_mode,
                false,
            ))
        }
        Request::VerifyPayment {
            grant_file,
            key_file,
            expected_issuer,
            deployment,
        } => {
            let (grant, recipient, issuer) = load_grant(&grant_file, &key_file, &expected_issuer)?;
            let (evidence, _) = open_checked(&grant, &recipient, &issuer, now)?;
            let mode = evidence.terms.settlement_mode;
            let verified = verify_payment(PaymentRequest {
                grant,
                recipient,
                issuer,
                now,
                deployment,
                evidence,
                default_cache_root: grant_file
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join("public-cache"),
            })
            .await?;
            match verified {
                PaymentVerification::Finalized(agreement) => {
                    Ok(verification_response(agreement, mode, true))
                }
                PaymentVerification::Pending {
                    agreement,
                    next_log_block,
                    ancestry_block,
                } => {
                    let mut response = verification_response(agreement, mode, false);
                    response["status"] = json!("pending");
                    response["ancestry_block"] = json!(ancestry_block);
                    if let Some(block) = next_log_block {
                        response["next_log_block"] = json!(block);
                    }
                    Ok(response)
                }
            }
        }
    }
}

/// An authenticated grant and the operator's payment observation configuration.
/// Private evidence must stay local; do not log this request.
pub struct PaymentRequest {
    /// Encrypted, participant-signed grant.
    pub grant: DisclosureGrant,
    /// Local recipient identity, never a participant spending key.
    pub recipient: DisclosureIdentity,
    /// Independently supplied participant identity.
    pub issuer: Vec<u8>,
    /// Current time for grant policy checks.
    pub now: u64,
    /// Backend-specific trusted deployment configuration.
    pub deployment: Value,
    /// Already verified selected agreement; its opening is private.
    pub evidence: SelectedAgreement,
    /// Default public observation cache beside the grant. Contains no participant secrets.
    pub default_cache_root: PathBuf,
}

/// An independent payment verifier's result. Pending is never a payment claim.
pub enum PaymentVerification {
    /// Matching finalized payment was independently verified.
    Finalized(VerifiedAgreement),
    /// Authenticated agreement, but public history verification is incomplete.
    Pending {
        /// Agreement identity only.
        agreement: VerifiedAgreement,
        /// Next log range, if unfinished.
        next_log_block: Option<u64>,
        /// Saved ancestry checkpoint.
        ancestry_block: u64,
    },
}

fn open_checked(
    grant: &DisclosureGrant,
    recipient: &DisclosureIdentity,
    issuer: &[u8],
    now: u64,
) -> Result<(SelectedAgreement, VerifiedAgreement), &'static str> {
    let (evidence, agreement) = grant
        .open(recipient, issuer, now)
        .map_err(|_| "disclosure agreement not verified")?;
    if evidence.terms.buyer_authorization_key.as_bytes() != issuer
        && evidence.terms.seller_authorization_key.as_bytes() != issuer
    {
        return Err("issuer is not an agreement participant");
    }
    Ok((evidence, agreement))
}

/// Independently verifies a public-bound payment; shielded requests fail without RPC access.
pub async fn verify_public_payment(
    request: PaymentRequest,
) -> Result<PaymentVerification, &'static str> {
    let PaymentRequest {
        grant,
        recipient,
        issuer,
        now,
        deployment,
        evidence,
        default_cache_root,
    } = request;
    let issuer: [u8; 20] = issuer
        .try_into()
        .map_err(|_| "public-bound participant required")?;
    open_public_bound_disclosure(&grant, &recipient, issuer, now)
        .map_err(|_| "public-bound disclosure required")?;
    let deployment: Deployment =
        serde_json::from_value(deployment).map_err(|_| "invalid public-bound deployment")?;
    if deployment.log_block_range == 0
        || deployment.log_block_range > 2_000
        || deployment.max_log_queries == 0
        || deployment.max_log_queries > 1_024
        || deployment.max_ancestry == 0
        || deployment.max_ancestry > 8_192
    {
        return Err("invalid observation budget");
    }
    let namespace =
        ChainNamespace::parse(&deployment.namespace).map_err(|_| "invalid deployment")?;
    let contract =
        parse_lowercase_address(&deployment.settlement_contract).ok_or("invalid deployment")?;
    if contract == [0; 20] || deployment.verifier_version == 0 {
        return Err("invalid deployment");
    }
    let configured = EvmDeployment::new(
        namespace,
        contract,
        deployment.verifier_version,
        deployment.rpc_url,
    )
    .map_err(|_| "invalid deployment")?;
    if configured.chain_id == 0 {
        return Err("invalid deployment");
    }
    configured
        .matches_domain(&evidence.terms.domain)
        .map_err(|_| "deployment does not match agreement")?;
    let chain = EvmChain::connect(configured, Duration::from_secs(15))
        .await
        .map_err(|_| "payment verification unavailable")?;
    let limits = ObservationLimits {
        log_block_range: deployment.log_block_range,
        max_log_queries: deployment.max_log_queries,
        max_ancestry: deployment.max_ancestry,
    };
    let journal = ObservationJournal::open(deployment.cache_root.unwrap_or(default_cache_root))
        .map_err(|_| "public history cache unavailable")?;
    let observed = verify_public_bound_disclosure_resumable(
        &grant,
        &recipient,
        issuer,
        now,
        &chain,
        &journal,
        deployment.from_block,
        limits,
    )
    .await
    .map_err(|_| "payment not independently verified")?;
    match observed {
        DisclosureObservation::Finalized(verified) => {
            Ok(PaymentVerification::Finalized(verified.agreement))
        }
        DisclosureObservation::Pending {
            agreement,
            next_log_block,
            ancestry_block,
        } => Ok(PaymentVerification::Pending {
            agreement,
            next_log_block,
            ancestry_block,
        }),
    }
}

/// Whether a `verify_payment` deployment selects the x402 exact rail.
#[must_use]
pub fn is_x402_request(deployment: &Value) -> bool {
    deployment.get("rail").and_then(Value::as_str) == Some("x402_exact")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct X402Deployment {
    rail: String,
    namespace: String,
    rpc_url: String,
    peer_rpc_url: String,
    permit2_runtime_hash: String,
    proxy_runtime_hash: String,
    transaction_hash: String,
}

fn lowercase_hash(text: &str, label: &'static str) -> Result<[u8; 32], &'static str> {
    let digits = text.strip_prefix("0x").unwrap_or(text);
    if digits.len() != 64
        || !digits
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(label);
    }
    hex::decode(digits)
        .map_err(|_| label)?
        .try_into()
        .map_err(|_| label)
}

/// Independently verifies a finalized x402 exact payment from the encrypted grant alone.
///
/// The auditor needs no participant state and no spending key. It authenticates the pinned
/// canonical Permit2 and exact-proxy runtimes at two configured RPCs, decodes the permit and
/// signature from the finalized transaction input, and requires matching finalized evidence
/// at both observers. A pending observation is never a payment claim.
pub async fn verify_x402_payment(
    request: PaymentRequest,
) -> Result<PaymentVerification, &'static str> {
    let PaymentRequest {
        grant,
        recipient,
        issuer,
        now,
        deployment,
        ..
    } = request;
    let issuer: [u8; 20] = issuer
        .try_into()
        .map_err(|_| "public-bound participant required")?;
    let (evidence, agreement) = open_checked(&grant, &recipient, &issuer, now)?;
    if evidence.terms.settlement_mode != SettlementMode::PublicBound
        || evidence.terms.suite_id != 1
        || evidence.terms.fee_policy.fee.get() != 0
    {
        return Err("x402 exact disclosure required");
    }
    let deployment: X402Deployment =
        serde_json::from_value(deployment).map_err(|_| "invalid x402 deployment")?;
    if deployment.rail != "x402_exact" {
        return Err("invalid x402 deployment");
    }
    let permit2 = lowercase_hash(&deployment.permit2_runtime_hash, "invalid x402 deployment")?;
    let proxy = lowercase_hash(&deployment.proxy_runtime_hash, "invalid x402 deployment")?;
    let transaction_hash =
        lowercase_hash(&deployment.transaction_hash, "invalid x402 transaction")?;
    let namespace =
        ChainNamespace::parse(&deployment.namespace).map_err(|_| "invalid deployment")?;
    let configured = EvmDeployment::new(namespace, EXACT_PERMIT2_PROXY, 1, &deployment.rpc_url)
        .map_err(|_| "invalid deployment")?;
    configured
        .matches_domain(&evidence.terms.domain)
        .map_err(|_| "deployment does not match agreement")?;
    let peer_deployment = EvmDeployment::new(
        configured.namespace.clone(),
        EXACT_PERMIT2_PROXY,
        1,
        &deployment.peer_rpc_url,
    )
    .map_err(|_| "invalid deployment")?;
    let primary = EvmChain::connect(configured, Duration::from_secs(15))
        .await
        .map_err(|_| "payment verification unavailable")?;
    let peer = EvmChain::connect(peer_deployment, Duration::from_secs(15))
        .await
        .map_err(|_| "payment verification unavailable")?;
    primary
        .check_peer(&peer)
        .map_err(|_| "inconsistent x402 observers")?;
    primary
        .authenticate_x402_runtimes(permit2, proxy)
        .await
        .map_err(|_| "x402 runtime authentication failed")?;
    peer.authenticate_x402_runtimes(permit2, proxy)
        .await
        .map_err(|_| "peer x402 runtime authentication failed")?;
    let owner: [u8; 20] = evidence
        .terms
        .buyer_authorization_key
        .as_bytes()
        .try_into()
        .map_err(|_| "public-bound participant required")?;
    let peer_evidence = peer
        .finalized_x402_evidence(transaction_hash, owner, &agreement.nullifier)
        .await
        .map_err(|_| "payment verification unavailable")?;
    let primary_evidence = primary
        .finalized_x402_evidence(transaction_hash, owner, &agreement.nullifier)
        .await
        .map_err(|_| "payment verification unavailable")?;
    let (Some(peer_evidence), Some(primary_evidence)) = (peer_evidence, primary_evidence) else {
        return Ok(PaymentVerification::Pending {
            agreement,
            next_log_block: None,
            ancestry_block: 0,
        });
    };
    let (permit, calldata_owner, signature) = decode_settle_call(&primary_evidence.calldata)
        .ok_or("payment not independently verified")?;
    if calldata_owner != owner {
        return Err("payment not independently verified");
    }
    verify_x402_exact(
        primary.deployment(),
        &evidence.terms,
        &permit,
        &signature,
        &primary_evidence,
    )
    .map_err(|_| "payment not independently verified")?;
    verify_x402_exact(
        peer.deployment(),
        &evidence.terms,
        &permit,
        &signature,
        &peer_evidence,
    )
    .map_err(|_| "peer payment not independently verified")?;
    Ok(PaymentVerification::Finalized(agreement))
}

fn verification_response(
    agreement: VerifiedAgreement,
    mode: SettlementMode,
    payment_verified: bool,
) -> Value {
    json!({
        "status": "ok",
        "mode": if mode == SettlementMode::Shielded { "shielded" } else { "public_bound" },
        "agreement_verified": true,
        "payment_verified": payment_verified,
        "delivery_verified": false,
        "deal_id": hex::encode(agreement.deal_id),
        "revision": agreement.revision,
        "deal_commitment": agreement.commitment.to_hex(),
        "deal_nullifier": agreement.nullifier.to_hex(),
    })
}

fn load_grant(
    grant_file: &Path,
    key_file: &Path,
    issuer: &str,
) -> Result<(DisclosureGrant, DisclosureIdentity, Vec<u8>), &'static str> {
    let digits = issuer.strip_prefix("0x").ok_or("invalid expected issuer")?;
    if !matches!(digits.len(), 40 | 128)
        || !digits
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("invalid expected issuer");
    }
    let issuer = hex::decode(digits).map_err(|_| "invalid expected issuer")?;
    let recipient = load_recipient(key_file)?;
    let bytes = read_private_file(grant_file, MAX_GRANT_BYTES)?;
    let grant = DisclosureGrant::decode(&bytes).map_err(|_| "cannot load disclosure grant")?;
    Ok((grant, recipient, issuer))
}

fn load_recipient(path: &Path) -> Result<DisclosureIdentity, &'static str> {
    let bytes = read_private_file(path, 32)?;
    let private: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| "cannot load disclosure key")?;
    let private = Zeroizing::new(private);
    DisclosureIdentity::from_private_key(*private).map_err(|_| "cannot load disclosure key")
}

fn parse_operation_ref(text: &str) -> Result<[u8; 32], &'static str> {
    let digits = text.strip_prefix("0x").unwrap_or(text);
    if digits.len() != 64
        || !digits
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("invalid operation reference");
    }
    hex::decode(digits)
        .map_err(|_| "invalid operation reference")?
        .try_into()
        .map_err(|_| "invalid operation reference")
}

/// Writes a new owner-only file, refusing to overwrite an existing one.
fn write_owner_only_new(path: &Path, bytes: &[u8]) -> Result<(), &'static str> {
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .map_err(|_| "cannot create evidence file")?;
    file.write_all(bytes)
        .map_err(|_| "cannot write evidence file")?;
    file.sync_all()
        .map_err(|_| "cannot persist evidence file")?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| "cannot persist evidence directory")
}

fn public_key(text: &str) -> Result<[u8; 32], &'static str> {
    if text.len() != 64
        || !text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("invalid recipient public key");
    }
    hex::decode(text)
        .map_err(|_| "invalid recipient public key")?
        .try_into()
        .map_err(|_| "invalid recipient public key")
}

fn read_private_file(path: &Path, maximum: usize) -> Result<Zeroizing<Vec<u8>>, &'static str> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| "private input unavailable")?;
    if !metadata.is_file() || metadata.len() > maximum as u64 {
        return Err("invalid private input");
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err("private input permissions are not owner-only");
    }
    let file = File::open(path).map_err(|_| "private input unavailable")?;
    let metadata = file.metadata().map_err(|_| "private input unavailable")?;
    if !metadata.is_file() {
        return Err("invalid private input");
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err("private input permissions are not owner-only");
    }
    let mut bytes = Zeroizing::new(Vec::new());
    file.take((maximum + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "private input unavailable")?;
    if bytes.len() > maximum {
        return Err("invalid private input");
    }
    Ok(bytes)
}
