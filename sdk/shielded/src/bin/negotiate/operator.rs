//! Operator setup for either agreement suite: fresh keys, a signed descriptor, and the canonical
//! service template both participants negotiate against. Nothing here touches a chain.

use super::*;
use chacha20poly1305::aead::{rand_core::RngCore, OsRng};
use erebus_core::{
    domain::DeploymentDomain,
    ids::{AddressBytes, AssetId, ChainNamespace},
    service::ServiceRecord,
    terms::{FeePolicy, Guarantee, GuaranteeSet, SettlementMode, CURRENT_PROTOCOL_VERSION},
};
use erebus_transport::descriptor::{MODE_PUBLIC_BOUND, MODE_SHIELDED};

/// Longest descriptor validity this setup will sign.
const MAX_DESCRIPTOR_LIFETIME: u64 = 30 * 24 * 3600;

pub(super) fn default_suite() -> u16 {
    1
}

fn guarantees(suite: u16) -> GuaranteeSet {
    if suite == 2 {
        GuaranteeSet::from_bits(7).expect("defined shielded guarantees")
    } else {
        GuaranteeSet::from_guarantee(Guarantee::AgreementBoundSettlement)
    }
}

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
/// also signs discovery, or separate suite-2/discovery keys. Returns no key values.
#[allow(clippy::too_many_arguments)]
pub(super) fn prepare_operator(
    directory: &Path,
    role: &str,
    endpoint: SocketAddr,
    namespace: &str,
    assets: &[String],
    lifetime: u64,
    suite: u16,
) -> Result<Value, &'static str> {
    if !directory.is_absolute() || !matches!(role, "buyer" | "seller") {
        return Err("use an absolute new directory and a buyer or seller role");
    }
    if !matches!(suite, 1 | 2) {
        return Err("unsupported agreement suite");
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
    if suite == 1 {
        write_new(&directory.join("agreement.key"), seed.as_ref(), true)?;
    } else {
        write_new(&directory.join("discovery.key"), seed.as_ref(), true)?;
        OsRng.fill_bytes(seed.as_mut());
        let public = derive_key(&seed).map_err(|_| "shielded key generation failed")?;
        write_new(&directory.join("agreement.key"), seed.as_ref(), true)?;
        write_new(&directory.join("agreement.public"), &public, true)?;
        OsRng.fill_bytes(seed.as_mut());
        write_new(&directory.join("wallet.key"), seed.as_ref(), true)?;
        if role == "seller" {
            loop {
                OsRng.fill_bytes(seed.as_mut());
                if note_spend_tag(&seed).is_ok() {
                    break;
                }
            }
            write_new(&directory.join("spend.key"), seed.as_ref(), true)?;
        }
    }
    let issued = now()?.saturating_sub(60);
    let mut descriptor = ServiceDescriptor::new(
        identity.address(),
        transport.public_key(),
        vec![format!("tcp://{endpoint}")],
        namespace,
        assets,
        vec![suite],
        if suite == 2 {
            MODE_SHIELDED
        } else {
            MODE_PUBLIC_BOUND
        },
        guarantees(suite),
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
    fs::File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|_| "operator directory sync failed")?;
    let mut response = json!({"protocol_version":1,"status":"ok","role":role,"suite_id":suite,
        "discovery_address":format!("0x{}", hex::encode(identity.address())),
        "transport_public_key":hex::encode(transport.public_key()),
        "agreement_key_file":directory.join("agreement.key"),"transport_key_file":directory.join("transport.key"),
        "descriptor_file":descriptor_file,"descriptor_expires_at":issued + lifetime});
    if suite == 1 {
        response["agreement_address"] = json!(format!("0x{}", hex::encode(identity.address())));
    } else {
        response["discovery_key_file"] = json!(directory.join("discovery.key"));
        response["wallet_key_file"] = json!(directory.join("wallet.key"));
        response["wallet_agreement_key_file"] = json!(directory.join("agreement.public"));
        if role == "seller" {
            response["seller_spend_secret_file"] = json!(directory.join("spend.key"));
        }
    }
    Ok(response)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BindingRequest {
    output: PathBuf,
    descriptor_file: PathBuf,
    discovery_key_file: PathBuf,
    agreement_key_file: PathBuf,
    settlement_contract: String,
    verifier_version: u32,
    expires_at: u64,
}

/// Re-signs `descriptor_file` with its own discovery key for `lifetime` seconds from now.
///
/// Every field except the validity window is kept, and the key must be the one the descriptor
/// names, so renewal can neither change an identity nor claim one. The file is replaced
/// atomically; peers must re-read it, and bindings and service terms must be re-issued.
pub(super) fn renew_descriptor(
    descriptor_file: &Path,
    discovery_key_file: &Path,
    lifetime: u64,
) -> Result<Value, &'static str> {
    if !descriptor_file.is_absolute() || !discovery_key_file.is_absolute() {
        return Err("use absolute descriptor and discovery key paths");
    }
    if !(3600..=MAX_DESCRIPTOR_LIFETIME).contains(&lifetime) {
        return Err("descriptor lifetime must be 1 hour to 30 days");
    }
    let text = fs::read_to_string(descriptor_file).map_err(|_| "descriptor unavailable")?;
    let mut descriptor = erebus_transport::descriptor::ServiceDescriptor::from_json(&text)
        .map_err(|_| "invalid descriptor")?;
    let identity = AuthorizationIdentity::from_bytes(key(discovery_key_file)?.as_ref())
        .map_err(|_| "invalid discovery key")?;
    let issued = now()?.saturating_sub(60);
    descriptor.issued_at = issued;
    descriptor.expires = issued + lifetime;
    descriptor
        .sign(&identity)
        .map_err(|_| "discovery key does not match local descriptor")?;
    descriptor
        .verify(now()?)
        .map_err(|_| "renewed descriptor failed verification")?;
    let encoded = descriptor
        .to_json()
        .map_err(|_| "descriptor encoding failed")?;
    let temporary = descriptor_file.with_extension("json.renew");
    let _ = fs::remove_file(&temporary);
    write_new(&temporary, encoded.as_bytes(), false)?;
    fs::rename(&temporary, descriptor_file).map_err(|_| "descriptor replace failed")?;
    if let Some(parent) = descriptor_file.parent() {
        fs::File::open(parent)
            .and_then(|file| file.sync_all())
            .map_err(|_| "descriptor directory sync failed")?;
    }
    Ok(
        json!({"protocol_version":1,"status":"ok","descriptor_file":descriptor_file,
        "descriptor_expires_at":issued + lifetime}),
    )
}

fn shielded_domain(
    namespace: ChainNamespace,
    contract: &str,
    verifier_version: u32,
) -> Result<DeploymentDomain, &'static str> {
    let contract =
        AddressBytes::new(address(contract)?.to_vec()).map_err(|_| "invalid pool address")?;
    Ok(DeploymentDomain {
        namespace,
        settlement_contract: Some(contract.clone()),
        pool: Some(contract),
        verifier_version,
    })
}

/// Writes an owner-only attestation. Exchange it privately; never publish it with discovery.
pub(super) fn prepare_binding(request: BindingRequest) -> Result<Value, &'static str> {
    if !request.output.is_absolute() {
        return Err("use an absolute new binding path");
    }
    let descriptor = descriptor(&request.descriptor_file)?;
    let current = now()?;
    let domain = shielded_domain(
        descriptor.chain_namespace.clone(),
        &request.settlement_contract,
        request.verifier_version,
    )?;
    let discovery = AuthorizationIdentity::from_bytes(key(&request.discovery_key_file)?.as_ref())
        .map_err(|_| "invalid discovery key")?;
    let seed = key(&request.agreement_key_file)?;
    let agreement = KeyBytes::new(
        derive_key(&seed)
            .map_err(|_| "invalid shielded key")?
            .to_vec(),
    )
    .map_err(|_| "invalid shielded key")?;
    let binding = AgreementKeyBinding::sign(
        &descriptor,
        domain,
        agreement,
        &discovery,
        current,
        request.expires_at,
    )
    .map_err(|_| "identity binding context or validity failed")?;
    write_new(
        &request.output,
        &binding.encode().map_err(|_| "binding encoding failed")?,
        true,
    )?;
    fs::File::open(
        request
            .output
            .parent()
            .ok_or("binding parent unavailable")?,
    )
    .and_then(|file| file.sync_all())
    .map_err(|_| "binding directory sync failed")?;
    Ok(
        json!({"protocol_version":1,"status":"ok","binding_file":request.output,
        "expires_at":request.expires_at,"private_counterparty_exchange":true}),
    )
}

/// The service promise both participants must configure identically, apart from price.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TermsRequest {
    #[serde(default = "default_suite")]
    pub(super) suite_id: u16,
    #[serde(default)]
    pub(super) local_binding_file: Option<PathBuf>,
    #[serde(default)]
    pub(super) peer_binding_file: Option<PathBuf>,
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

/// Writes the canonical template. Deal identity, nonce, expiry, and transcript are
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
    let mode = match request.suite_id {
        1 => MODE_PUBLIC_BOUND,
        2 => MODE_SHIELDED,
        _ => return Err("unsupported agreement suite"),
    };
    if [&local, &peer].iter().any(|descriptor| {
        !descriptor.assets.contains(&asset)
            || descriptor.chain_namespace != *asset.namespace()
            || !descriptor.suites.contains(&request.suite_id)
            || descriptor.settlement_modes & mode == 0
            || descriptor.guarantees.bits() & guarantees(request.suite_id).bits()
                != guarantees(request.suite_id).bits()
    }) {
        return Err("both descriptors must support the agreement asset, chain, suite and mode");
    }
    let domain = if request.suite_id == 2 {
        shielded_domain(
            asset.namespace().clone(),
            &request.settlement_contract,
            request.verifier_version,
        )?
    } else {
        DeploymentDomain {
            namespace: asset.namespace().clone(),
            settlement_contract: Some(
                AddressBytes::new(address(&request.settlement_contract)?.to_vec())
                    .map_err(|_| "invalid settlement contract")?,
            ),
            pool: None,
            verifier_version: request.verifier_version,
        }
    };
    let (buyer_key, seller_key) = if request.suite_id == 2 {
        let binding_key = |path: &Option<PathBuf>, descriptor: &ServiceDescriptor| {
            let path = path
                .as_ref()
                .ok_or("suite-2 requires private identity bindings")?;
            let bytes = read(path, erebus_transport::binding::MAX_BINDING_BYTES, true)?;
            AgreementKeyBinding::decode(&bytes)
                .and_then(|binding| binding.attested_key(descriptor, &domain, current))
                .map_err(|_| "identity binding signature, domain or validity failed")
        };
        let local_key = binding_key(&request.local_binding_file, &local)?;
        let peer_key = binding_key(&request.peer_binding_file, &peer)?;
        if local_key == peer_key {
            return Err("buyer and seller must use different agreement keys");
        }
        if request.role == "buyer" {
            (local_key, peer_key)
        } else {
            (peer_key, local_key)
        }
    } else {
        if request.local_binding_file.is_some() || request.peer_binding_file.is_some() {
            return Err("public-bound templates must not carry shielded bindings");
        }
        (
            KeyBytes::new(buyer.to_vec()).map_err(|_| "invalid key")?,
            KeyBytes::new(seller.to_vec()).map_err(|_| "invalid key")?,
        )
    };
    // The encrypted bootstrap replaces this template-only spend tag with the seller's tag.
    let recipient = if request.suite_id == 2 {
        let mut placeholder = [0u8; 32];
        placeholder[31] = 1;
        KeyBytes::new(placeholder.to_vec()).map_err(|_| "invalid template tag")?
    } else {
        seller_key.clone()
    };
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
    let terms = AgreementTerms {
        protocol_version: CURRENT_PROTOCOL_VERSION,
        suite_id: request.suite_id,
        domain,
        deal_id: [0; 16],
        revision: 1,
        transcript_root: [0; 32],
        buyer_authorization_key: buyer_key.clone(),
        seller_authorization_key: seller_key,
        payment_recipient: recipient,
        asset,
        amount: BaseUnits::new(decimal(&request.amount)?),
        expiry: request.delivery_deadline,
        fee_policy: FeePolicy::none(),
        settlement_mode: if request.suite_id == 2 {
            SettlementMode::Shielded
        } else {
            SettlementMode::PublicBound
        },
        required_guarantees: guarantees(request.suite_id),
        settlement_nonce: [0; 32],
        service: ServiceRecord {
            resource: request.resource,
            quantity: BaseUnits::new(decimal(&request.quantity)?),
            unit: request.unit,
            access_recipient: buyer_key,
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
    fs::File::open(request.output.parent().ok_or("terms parent unavailable")?)
        .and_then(|file| file.sync_all())
        .map_err(|_| "terms directory sync failed")?;
    if request.suite_id == 2 {
        return Ok(json!({"protocol_version":1,"status":"ok","suite_id":2,
            "terms_template_file":request.output,"payment_recipient_finalized":false}));
    }
    Ok(
        json!({"protocol_version":1,"status":"ok","terms_template_file":request.output,
        "buyer":format!("0x{}", hex::encode(buyer)),"seller":format!("0x{}", hex::encode(seller)),
        "payment_recipient":format!("0x{}", hex::encode(seller))}),
    )
}
