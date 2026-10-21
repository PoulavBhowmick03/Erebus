//! Many-buyer sellers: the operator configuration, the configured peer table, and the seller's
//! accept loop.
//!
//! A version 1 configuration names exactly one counterparty and is unchanged. A version 2
//! seller configuration names a `peers_root` instead, holding one `<id>/peer.json` per buyer.
//! The seller still negotiates one operation at a time under the identity lease; it accepts
//! connections until one configured buyer authenticates, and that buyer binds the operation.

use super::*;
use erebus_transport::socket::SocketError;

/// Most enabled peers one seller configuration may hold.
pub(super) const MAX_PEERS: usize = 256;
/// Most entries a peers root may hold, enabled or not, so loading stays bounded.
const MAX_PEER_ENTRIES: usize = 1024;
/// Longest one accepted connection may take to finish its handshake.
pub(super) const HANDSHAKE_BUDGET: Duration = Duration::from_secs(5);
/// File holding one peer's settings inside its directory.
pub(super) const PEER_FILE: &str = "peer.json";

pub(super) const PEER_NOT_CONFIGURED: &str = "negotiation peer is not configured";
pub(super) const PEER_EXPIRED: &str = "negotiation peer is expired or disabled";
pub(super) const BOUND_TO_OTHER_PEER: &str = "operation is bound to another negotiation peer";
pub(super) const NO_PEER_CONNECTED: &str = "no configured peer connected before the deadline";
/// The version 1 deadline message, kept for version 1 configurations.
const DEADLINE_EXCEEDED: &str = "negotiation connection deadline exceeded";
const INVALID: &str = "invalid operator configuration";

/// A seller-chosen peer name: 1 to 32 characters of `[a-z0-9-]`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) struct PeerId(String);

impl PeerId {
    pub(super) fn parse(value: &str) -> Option<Self> {
        (!value.is_empty()
            && value.len() <= 32
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'))
        .then(|| Self(value.to_owned()))
    }

    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Version 1: one counterparty, for either role.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigV1 {
    version: u16,
    role: String,
    state_root: PathBuf,
    transport_key_file: PathBuf,
    agreement_key_file: PathBuf,
    discovery_key_file: Option<PathBuf>,
    local_descriptor_file: PathBuf,
    peer_descriptor_file: PathBuf,
    terms_template_file: PathBuf,
    endpoint: SocketAddr,
    maximum_price: String,
    minimum_price: String,
    max_deal_lifetime_seconds: u64,
    timeout_seconds: u64,
    seller_spend_secret_file: Option<PathBuf>,
    seller_wallet_file: Option<PathBuf>,
    seller_wallet_key_file: Option<PathBuf>,
    #[serde(default)]
    access_evidence_root: Option<PathBuf>,
    /// Buyer only: 2 when the seller's service record advertises routed transport.
    #[serde(default)]
    negotiation_transport: Option<u16>,
}

/// Version 2: a seller with a table of configured buyers under `peers_root`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigV2 {
    version: u16,
    role: String,
    state_root: PathBuf,
    transport_key_file: PathBuf,
    agreement_key_file: PathBuf,
    discovery_key_file: Option<PathBuf>,
    local_descriptor_file: PathBuf,
    peers_root: PathBuf,
    endpoint: SocketAddr,
    maximum_price: String,
    timeout_seconds: u64,
    seller_spend_secret_file: Option<PathBuf>,
    seller_wallet_file: Option<PathBuf>,
    seller_wallet_key_file: Option<PathBuf>,
    #[serde(default)]
    access_evidence_root: Option<PathBuf>,
}

/// One `peers/<id>/peer.json`. File names are bare names inside the peer's directory.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PeerFile {
    version: u32,
    id: String,
    descriptor_file: String,
    #[serde(default)]
    binding_file: Option<String>,
    terms_template_file: String,
    minimum_price: String,
    max_deal_lifetime_seconds: u64,
    resource: String,
    expires_at: u64,
    enabled: bool,
    legacy_transport: bool,
}

/// One configured counterparty and the service promise negotiated with it.
pub(super) struct Peer {
    /// `None` only for the implicit counterparty of a version 1 configuration.
    pub(super) id: Option<PeerId>,
    pub(super) descriptor: ServiceDescriptor,
    pub(super) template: AgreementTerms,
    pub(super) minimum: BaseUnits,
    pub(super) lifetime: u64,
    pub(super) resource: String,
    pub(super) expires_at: Option<u64>,
    pub(super) enabled: bool,
    /// Its descriptor verified at load time. Its own `expires_at` is checked against the clock
    /// whenever it is selected, see [`Peer::live_at`].
    pub(super) current: bool,
    /// May connect with the version 1 handshake. Its transcripts keep the `agent` namespace.
    pub(super) legacy: bool,
}

impl Peer {
    /// Its configured expiry has not passed at `now`.
    pub(super) fn unexpired(&self, now: u64) -> bool {
        self.expires_at.is_none_or(|expires_at| now < expires_at)
    }

    /// Enabled, its descriptor valid at load, and unexpired at `now`. The handshake checks the
    /// descriptor itself against the clock again.
    pub(super) fn live_at(&self, now: u64) -> bool {
        self.enabled && self.current && self.unexpired(now)
    }

    /// The transcript namespace of this peer's operations.
    pub(super) fn namespace(&self) -> String {
        match (&self.id, self.legacy) {
            (Some(id), false) => format!("peer-{}", id.as_str()),
            _ => "agent".to_owned(),
        }
    }
}

/// The operator configuration, normalized across versions.
pub(super) struct Operator {
    pub(super) version: u16,
    pub(super) role: Role,
    pub(super) state_root: PathBuf,
    pub(super) transport_key_file: PathBuf,
    pub(super) agreement_key_file: PathBuf,
    pub(super) discovery_key_file: Option<PathBuf>,
    pub(super) local_descriptor_file: PathBuf,
    pub(super) endpoint: SocketAddr,
    pub(super) maximum: BaseUnits,
    pub(super) timeout_seconds: u64,
    pub(super) seller_spend_secret_file: Option<PathBuf>,
    pub(super) seller_wallet_file: Option<PathBuf>,
    pub(super) seller_wallet_key_file: Option<PathBuf>,
    pub(super) access_evidence_root: Option<PathBuf>,
    /// The buyer dials with the routed handshake.
    pub(super) routed: bool,
    /// The buyer's one seller, or the seller's configured buyers.
    pub(super) peers: Vec<Peer>,
}

fn price(value: &str) -> Result<BaseUnits, &'static str> {
    Ok(BaseUnits::new(
        value.parse().map_err(|_| "invalid price policy")?,
    ))
}

fn role(value: &str) -> Result<Role, &'static str> {
    match value {
        "buyer" => Ok(Role::Buyer),
        "seller" => Ok(Role::Seller),
        _ => Err("invalid configured role"),
    }
}

/// Checks one peer's price and lifetime policy against the operator's maximum.
fn check_policy(peer: &Peer, role: Role, maximum: BaseUnits) -> Result<(), &'static str> {
    if !(1..=86400).contains(&peer.lifetime) {
        return Err(INVALID);
    }
    if maximum.is_zero()
        || peer.minimum > maximum
        || peer.template.amount > maximum
        || (role == Role::Seller && peer.template.amount < peer.minimum)
    {
        return Err("invalid price policy");
    }
    if peer.template.service.resource != peer.resource {
        return Err(INVALID);
    }
    Ok(())
}

/// Reads and normalizes `negotiate.json` of either version.
pub(super) fn load_operator(bytes: &[u8], now: u64) -> Result<Operator, &'static str> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| INVALID)?;
    match value.get("version").and_then(Value::as_u64) {
        Some(1) => load_v1(serde_json::from_value(value).map_err(|_| INVALID)?, now),
        Some(2) => load_v2(serde_json::from_value(value).map_err(|_| INVALID)?, now),
        _ => Err(INVALID),
    }
}

fn load_v1(config: ConfigV1, now: u64) -> Result<Operator, &'static str> {
    if config.version != 1
        || !(1..=300).contains(&config.timeout_seconds)
        || !(1..=86400).contains(&config.max_deal_lifetime_seconds)
    {
        return Err(INVALID);
    }
    let role = role(&config.role)?;
    let routed = match (role, config.negotiation_transport) {
        (_, None | Some(1)) => false,
        (Role::Buyer, Some(2)) => true,
        _ => return Err(INVALID),
    };
    let maximum = price(&config.maximum_price)?;
    let template = AgreementTerms::decode(&read(&config.terms_template_file, 4096, true)?)
        .map_err(|_| "invalid service template")?;
    let descriptor = descriptor(&config.peer_descriptor_file)?;
    let current = descriptor.verify(now).is_ok();
    let peer = Peer {
        id: None,
        resource: template.service.resource.clone(),
        descriptor,
        template,
        minimum: price(&config.minimum_price)?,
        lifetime: config.max_deal_lifetime_seconds,
        expires_at: None,
        enabled: true,
        current,
        legacy: true,
    };
    check_policy(&peer, role, maximum)?;
    Ok(Operator {
        version: 1,
        role,
        state_root: config.state_root,
        transport_key_file: config.transport_key_file,
        agreement_key_file: config.agreement_key_file,
        discovery_key_file: config.discovery_key_file,
        local_descriptor_file: config.local_descriptor_file,
        endpoint: config.endpoint,
        maximum,
        timeout_seconds: config.timeout_seconds,
        seller_spend_secret_file: config.seller_spend_secret_file,
        seller_wallet_file: config.seller_wallet_file,
        seller_wallet_key_file: config.seller_wallet_key_file,
        access_evidence_root: config.access_evidence_root,
        routed,
        peers: vec![peer],
    })
}

fn load_v2(config: ConfigV2, now: u64) -> Result<Operator, &'static str> {
    if config.version != 2 || !(1..=300).contains(&config.timeout_seconds) {
        return Err(INVALID);
    }
    if role(&config.role)? != Role::Seller {
        return Err("only a seller configures many peers");
    }
    let maximum = price(&config.maximum_price)?;
    let peers = load_peers(&config.peers_root, now, maximum)?;
    Ok(Operator {
        version: 2,
        role: Role::Seller,
        state_root: config.state_root,
        transport_key_file: config.transport_key_file,
        agreement_key_file: config.agreement_key_file,
        discovery_key_file: config.discovery_key_file,
        local_descriptor_file: config.local_descriptor_file,
        endpoint: config.endpoint,
        maximum,
        timeout_seconds: config.timeout_seconds,
        seller_spend_secret_file: config.seller_spend_secret_file,
        seller_wallet_file: config.seller_wallet_file,
        seller_wallet_key_file: config.seller_wallet_key_file,
        access_evidence_root: config.access_evidence_root,
        routed: false,
        peers,
    })
}

/// A bare file name inside a peer directory: no separators, no dot names.
fn bare(directory: &Path, name: &str) -> Result<PathBuf, &'static str> {
    if name.is_empty()
        || name.len() > 64
        || name.starts_with('.')
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(INVALID);
    }
    Ok(directory.join(name))
}

/// Loads every `<id>/peer.json` under an owner-only `root`, in name order.
///
/// Entries whose name starts with `.` (staging directories) are skipped; any other entry must
/// be a peer directory with a valid id. At most [`MAX_PEERS`] peers may be enabled, enabled
/// peers must have distinct transport keys, and at most one enabled peer may be legacy.
pub(super) fn load_peers(
    root: &Path,
    now: u64,
    maximum: BaseUnits,
) -> Result<Vec<Peer>, &'static str> {
    let metadata = fs::symlink_metadata(root).map_err(|_| "peer directory unavailable")?;
    if !root.is_absolute() || !metadata.is_dir() {
        return Err(INVALID);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("local input must have owner-only permissions");
        }
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(root).map_err(|_| "peer directory unavailable")? {
        let entry = entry.map_err(|_| "peer directory unavailable")?;
        let name = entry.file_name().into_string().map_err(|_| INVALID)?;
        if name.starts_with('.') {
            continue;
        }
        if names.len() == MAX_PEER_ENTRIES {
            return Err(INVALID);
        }
        names.push(name);
    }
    names.sort();
    let mut peers = Vec::with_capacity(names.len());
    for name in names {
        let id = PeerId::parse(&name).ok_or(INVALID)?;
        let directory = root.join(&name);
        if !fs::symlink_metadata(&directory)
            .map_err(|_| "peer directory unavailable")?
            .is_dir()
        {
            return Err(INVALID);
        }
        let file: PeerFile =
            serde_json::from_slice(&read(&directory.join(PEER_FILE), MAX_REQUEST, true)?)
                .map_err(|_| INVALID)?;
        if file.version != 1 || file.id != name {
            return Err(INVALID);
        }
        if let Some(binding) = &file.binding_file {
            bare(&directory, binding)?;
        }
        let descriptor = descriptor(&bare(&directory, &file.descriptor_file)?)?;
        let current = match descriptor.verify(now) {
            Ok(()) => true,
            Err(
                erebus_transport::descriptor::DescriptorError::Expired { .. }
                | erebus_transport::descriptor::DescriptorError::NotYetValid { .. },
            ) => false,
            Err(_) => return Err("peer discovery descriptor is invalid"),
        };
        let template = AgreementTerms::decode(&read(
            &bare(&directory, &file.terms_template_file)?,
            4096,
            true,
        )?)
        .map_err(|_| "invalid service template")?;
        let peer = Peer {
            id: Some(id),
            descriptor,
            template,
            minimum: price(&file.minimum_price)?,
            lifetime: file.max_deal_lifetime_seconds,
            resource: file.resource,
            expires_at: Some(file.expires_at),
            enabled: file.enabled,
            current,
            legacy: file.legacy_transport,
        };
        check_policy(&peer, Role::Seller, maximum)?;
        peers.push(peer);
    }
    let enabled: Vec<&Peer> = peers.iter().filter(|peer| peer.enabled).collect();
    let mut keys: Vec<[u8; 32]> = enabled
        .iter()
        .map(|peer| peer.descriptor.transport_key)
        .collect();
    keys.sort_unstable();
    keys.dedup();
    if enabled.len() > MAX_PEERS
        || keys.len() != enabled.len()
        || enabled.iter().filter(|peer| peer.legacy).count() > 1
    {
        return Err(INVALID);
    }
    Ok(peers)
}

/// The peers one seller call may accept: every live peer, or only the one an operation or the
/// caller names.
pub(super) fn allowed_peer(
    operator: &Operator,
    bound: Option<Option<&str>>,
    requested: Option<&str>,
    now: u64,
) -> Result<Option<usize>, &'static str> {
    let find = |id: &str| {
        operator
            .peers
            .iter()
            .position(|peer| {
                peer.id
                    .as_ref()
                    .is_some_and(|peer_id| peer_id.as_str() == id)
            })
            .ok_or(PEER_NOT_CONFIGURED)
    };
    let index = match bound {
        // A retained operation is bound: to a named peer, or (a version 1 record) to the
        // legacy peer.
        Some(binding) => {
            let index = match binding {
                Some(id) => find(id)?,
                None => operator
                    .peers
                    .iter()
                    .position(|peer| peer.legacy)
                    .ok_or(PEER_NOT_CONFIGURED)?,
            };
            if let Some(requested) = requested {
                if operator.peers[index]
                    .id
                    .as_ref()
                    .is_none_or(|id| id.as_str() != requested)
                {
                    return Err(BOUND_TO_OTHER_PEER);
                }
            }
            Some(index)
        }
        None => match requested {
            Some(id) => Some(find(id)?),
            None => None,
        },
    };
    if let Some(index) = index {
        if !operator.peers[index].live_at(now) {
            return Err(if operator.version == 1 {
                "peer discovery descriptor is invalid or expired"
            } else {
                PEER_EXPIRED
            });
        }
    } else if !operator.peers.iter().any(|peer| peer.live_at(now)) {
        return Err(if operator.version == 1 {
            "peer discovery descriptor is invalid or expired"
        } else {
            PEER_EXPIRED
        });
    }
    Ok(index)
}

/// Accepts connections on the seller's endpoint until one allowed peer authenticates.
///
/// Every accepted connection gets at most [`HANDSHAKE_BUDGET`]. A connection that fails, names
/// an unknown, expired, or disabled peer, or a peer other than `allowed`, is closed and the loop
/// keeps accepting until the call's deadline. One handshake runs at a time.
pub(super) fn accept(
    operator: &Operator,
    transport: &TransportIdentity,
    local: &ServiceDescriptor,
    allowed: Option<usize>,
    timeout: Duration,
) -> Result<(SocketChannel, usize), &'static str> {
    let listener =
        TcpListener::bind(operator.endpoint).map_err(|_| "negotiation listener unavailable")?;
    listener
        .set_nonblocking(true)
        .map_err(|_| "negotiation listener unavailable")?;
    let deadline = Instant::now() + timeout;
    let expired = || {
        Err(if operator.version == 1 {
            DEADLINE_EXCEEDED
        } else {
            NO_PEER_CONNECTED
        })
    };
    // Expiry is evaluated against the clock each time a peer can be selected.
    let candidate = |index: usize, now: u64| {
        operator.peers[index].live_at(now) && allowed.is_none_or(|allowed| allowed == index)
    };
    loop {
        // The absolute deadline bounds the call, including the operator's identity lease, however
        // many connections keep arriving.
        if Instant::now() >= deadline {
            return expired();
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    drop(stream);
                    return expired();
                }
                let budget = HANDSHAKE_BUDGET.min(remaining);
                let started = now()?;
                let legacy = (0..operator.peers.len())
                    .find(|index| candidate(*index, started) && operator.peers[*index].legacy)
                    .map(|index| (operator.peers[index].descriptor.clone(), index));
                let accepted = SocketChannel::seller_routed(
                    stream,
                    transport,
                    local,
                    started,
                    timeout,
                    budget,
                    legacy,
                    |key| {
                        let selected = now().ok()?;
                        (0..operator.peers.len())
                            .find(|index| {
                                candidate(*index, selected)
                                    && operator.peers[*index].descriptor.transport_key == *key
                            })
                            .map(|index| (operator.peers[index].descriptor.clone(), index))
                    },
                );
                match accepted {
                    // Authenticated: check the selected peer's expiry once more, so a legacy peer
                    // chosen before its handshake cannot finish after expiring.
                    Ok((channel, _, index)) if candidate(index, now()?) => {
                        return Ok((channel, index))
                    }
                    Ok(_) => {}
                    // The local descriptor or identity is wrong: no connection can succeed.
                    Err(SocketError::Configuration) => {
                        return Err("invalid peer identity configuration")
                    }
                    // Unknown, unauthenticated, slow, or vanished: close it and keep accepting.
                    Err(_) => {}
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return Err("negotiation listener unavailable"),
        }
    }
}

/// The seller's configured peers, without key material: what `list_peers` reports.
pub(super) fn list(config_file: &Path) -> Result<Value, &'static str> {
    let current = now()?;
    let operator = load_operator(&read(config_file, MAX_REQUEST, true)?, current)?;
    if operator.role != Role::Seller {
        return Err("only a seller lists negotiation peers");
    }
    let peers: Vec<Value> = operator
        .peers
        .iter()
        .map(|peer| {
            json!({
                "peer": peer.id.as_ref().map(PeerId::as_str),
                "enabled": peer.enabled,
                "expired": !peer.current || !peer.unexpired(current),
                "live": peer.live_at(current),
                "resource": peer.resource,
                "price": peer.template.amount.get().to_string(),
                "minimum_price": peer.minimum.get().to_string(),
                "expires_at": peer.expires_at,
                "legacy_transport": peer.legacy,
            })
        })
        .collect();
    Ok(json!({"protocol_version":1,"status":"ok","config_version":operator.version,"peers":peers}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_ids_are_short_lowercase_names() {
        for valid in ["default", "b1", "buyer-2", &"a".repeat(32)] {
            assert!(PeerId::parse(valid).is_some(), "{valid}");
        }
        for invalid in ["", "B1", "b_1", "b.1", "../x", &"a".repeat(33), "b 1"] {
            assert!(PeerId::parse(invalid).is_none(), "{invalid}");
        }
    }

    #[test]
    fn peer_file_names_stay_inside_the_peer_directory() {
        let root = Path::new("/peers/b1");
        assert!(bare(root, "descriptor.json").is_ok());
        for name in ["", "../x", "a/b", ".hidden", "a\\b"] {
            assert!(bare(root, name).is_err(), "{name}");
        }
    }
}
