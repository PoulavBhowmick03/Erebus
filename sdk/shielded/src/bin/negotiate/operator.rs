//! Operator setup for a suite-1 participant: fresh keys, a signed descriptor, and the canonical
//! service template both participants negotiate against. Nothing here touches a chain.

use super::*;
use chacha20poly1305::aead::{rand_core::RngCore, OsRng};
use erebus_core::{
    domain::DeploymentDomain,
    ids::{AddressBytes, AssetId, ChainNamespace},
    service::ServiceRecord,
    terms::{FeePolicy, Guarantee, GuaranteeSet, SettlementMode, CURRENT_PROTOCOL_VERSION},
};
use erebus_transport::descriptor::MODE_PUBLIC_BOUND;

/// Longest descriptor validity this setup will sign.
const MAX_DESCRIPTOR_LIFETIME: u64 = 30 * 24 * 3600;

fn write_new(path: &Path, bytes: &[u8], private: bool) -> Result<(), &'static str> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if private { 0o600 } else { 0o644 });
    }
    #[cfg(not(unix))]
    let _ = private;
    let mut file = options
        .open(path)
        .map_err(|_| "refusing to overwrite an existing file")?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| "operator file write failed")
}

fn address(value: &str) -> Result<[u8; 20], &'static str> {
    erebus_evm::deployment::parse_lowercase_address(value)
        .ok_or("addresses must be lowercase 0x hex")
}

/// Creates `directory` (which must not exist) with a transport key, a suite-1 agreement key that
/// also signs discovery, and a signed public descriptor. Returns public values only.
pub(super) fn prepare_operator(
    directory: &Path,
    role: &str,
    endpoint: SocketAddr,
    namespace: &str,
    assets: &[String],
    lifetime: u64,
) -> Result<Value, &'static str> {
    if !directory.is_absolute() || !matches!(role, "buyer" | "seller") {
        return Err("use an absolute new directory and a buyer or seller role");
    }
    if !(3600..=MAX_DESCRIPTOR_LIFETIME).contains(&lifetime)
        || assets.is_empty()
        || assets.len() > 8
    {
        return Err("descriptor lifetime must be 1 hour to 30 days with 1 to 8 assets");
    }
    let namespace = ChainNamespace::parse(namespace).map_err(|_| "invalid chain namespace")?;
    let assets = assets
        .iter()
        .map(|asset| AssetId::parse(asset).map_err(|_| "invalid asset"))
        .collect::<Result<Vec<_>, _>>()?;
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(directory)
        .map_err(|_| "operator directory must not already exist")?;
    let transport = TransportIdentity::generate().map_err(|_| "transport key generation failed")?;
    transport
        .store(directory.join("transport.key"))
        .map_err(|_| "transport key write failed")?;
    let mut seed = Zeroizing::new([0u8; 32]);
    let identity = loop {
        OsRng.fill_bytes(seed.as_mut());
        if let Ok(identity) = AuthorizationIdentity::from_bytes(seed.as_ref()) {
            break identity;
        }
    };
    write_new(&directory.join("agreement.key"), seed.as_ref(), true)?;
    let issued = now()?.saturating_sub(60);
    let mut descriptor = ServiceDescriptor::new(
        identity.address(),
        transport.public_key(),
        vec![format!("tcp://{endpoint}")],
        namespace,
        assets,
        vec![1],
        MODE_PUBLIC_BOUND,
        GuaranteeSet::from_guarantee(Guarantee::AgreementBoundSettlement),
        issued,
        issued + lifetime,
    )
    .map_err(|_| "invalid descriptor fields")?;
    descriptor
        .sign(&identity)
        .map_err(|_| "descriptor signing failed")?;
    let descriptor_file = directory.join(format!("{role}.descriptor.json"));
    write_new(
        &descriptor_file,
        descriptor
            .to_json()
            .map_err(|_| "descriptor encoding failed")?
            .as_bytes(),
        false,
    )?;
    Ok(json!({"protocol_version":1,"status":"ok","role":role,
        "agreement_address":format!("0x{}", hex::encode(identity.address())),
        "transport_public_key":hex::encode(transport.public_key()),
        "agreement_key_file":directory.join("agreement.key"),"transport_key_file":directory.join("transport.key"),
        "descriptor_file":descriptor_file,"descriptor_expires_at":issued + lifetime}))
}

/// The service promise both participants must configure identically, apart from price.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TermsRequest {
    pub(super) output: PathBuf,
    pub(super) role: String,
    pub(super) local_descriptor_file: PathBuf,
    pub(super) peer_descriptor_file: PathBuf,
    /// `ErebusSettlement` for public-bound payment, or the canonical exact proxy for x402.
    pub(super) settlement_contract: String,
    pub(super) verifier_version: u32,
    pub(super) asset: String,
    /// This participant's starting price in base units.
    pub(super) amount: String,
    pub(super) resource: String,
    pub(super) unit: String,
    pub(super) quantity: String,
    pub(super) fulfillment_method: String,
    /// SHA-256 of the exact resource bytes, lowercase hex.
    pub(super) fulfillment_digest: String,
    /// Absolute unix delivery deadline; both participants must use the same value.
    pub(super) delivery_deadline: u64,
}

/// Writes the canonical suite-1 template. Deal identity, nonce, expiry, and transcript are
/// replaced per deal by negotiation; every other field is the promise the peer must match.
pub(super) fn prepare_terms(request: TermsRequest) -> Result<Value, &'static str> {
    if !request.output.is_absolute() || !matches!(request.role.as_str(), "buyer" | "seller") {
        return Err("use an absolute output path and a buyer or seller role");
    }
    let current = now()?;
    let local = descriptor(&request.local_descriptor_file)?;
    let peer = descriptor(&request.peer_descriptor_file)?;
    for descriptor in [&local, &peer] {
        descriptor
            .verify(current)
            .map_err(|_| "descriptor signature or validity failed")?;
    }
    if local.seller_address == peer.seller_address {
        return Err("buyer and seller must use different agreement keys");
    }
    let (buyer, seller) = if request.role == "buyer" {
        (local.seller_address, peer.seller_address)
    } else {
        (peer.seller_address, local.seller_address)
    };
    let asset = AssetId::parse(&request.asset).map_err(|_| "invalid asset")?;
    if !local.assets.contains(&asset) || !peer.assets.contains(&asset) {
        return Err("both descriptors must offer the agreement asset");
    }
    let digest: [u8; 32] = hex::decode(&request.fulfillment_digest)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or("fulfillment digest must be 64 hex digits")?;
    let decimal = |value: &str| -> Result<u128, &'static str> {
        (!value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()))
            .then(|| value.parse().ok())
            .flatten()
            .ok_or("amounts must be decimal base units")
    };
    let key = |bytes: [u8; 20]| KeyBytes::new(bytes.to_vec()).map_err(|_| "invalid key");
    let terms = AgreementTerms {
        protocol_version: CURRENT_PROTOCOL_VERSION,
        suite_id: 1,
        domain: DeploymentDomain {
            namespace: asset.namespace().clone(),
            settlement_contract: Some(
                AddressBytes::new(address(&request.settlement_contract)?.to_vec())
                    .map_err(|_| "invalid settlement contract")?,
            ),
            pool: None,
            verifier_version: request.verifier_version,
        },
        deal_id: [0; 16],
        revision: 1,
        transcript_root: [0; 32],
        buyer_authorization_key: key(buyer)?,
        seller_authorization_key: key(seller)?,
        payment_recipient: key(seller)?,
        asset,
        amount: BaseUnits::new(decimal(&request.amount)?),
        expiry: request.delivery_deadline,
        fee_policy: FeePolicy::none(),
        settlement_mode: SettlementMode::PublicBound,
        required_guarantees: GuaranteeSet::from_guarantee(Guarantee::AgreementBoundSettlement),
        settlement_nonce: [0; 32],
        service: ServiceRecord {
            resource: request.resource,
            quantity: BaseUnits::new(decimal(&request.quantity)?),
            unit: request.unit,
            access_recipient: key(buyer)?,
            delivery_deadline: request.delivery_deadline,
            fulfillment_method: request.fulfillment_method,
            fulfillment_digest: digest,
        },
    };
    if request.delivery_deadline <= current {
        return Err("delivery deadline must be in the future");
    }
    let encoded = terms
        .encode()
        .map_err(|_| "terms do not encode canonically")?;
    write_new(&request.output, &encoded, true)?;
    Ok(
        json!({"protocol_version":1,"status":"ok","terms_template_file":request.output,
        "buyer":format!("0x{}", hex::encode(buyer)),"seller":format!("0x{}", hex::encode(seller)),
        "payment_recipient":format!("0x{}", hex::encode(seller))}),
    )
}
