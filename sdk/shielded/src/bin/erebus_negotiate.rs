//! Native, independently keyed buyer/seller negotiation and durable agreement authorization.
//! This command never connects to an RPC, proves, signs a transaction, or submits payment.

use std::{
    fmt, fs,
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use erebus_coordinator::Coordinator;
use erebus_core::{
    auth::{authorization_digest, Authorization, Role},
    commitment::{commit_agreement, deal_nullifier},
    ids::{BaseUnits, KeyBytes, SignatureBytes},
    policy::SpendingPolicy,
    settlement::SettlementContext,
    shielded::{authorization_message, note_spend_tag},
    shielded_auth::{derive_key, sign_message},
    terms::AgreementTerms,
};
use erebus_journal::{JournalRecord, RecordId, Store, StoreError};
use erebus_shielded_prover::{
    preparation,
    wallet::{OwnedNote, WalletDomain, WalletStore},
};
use erebus_transport::{
    binding::AgreementKeyBinding,
    descriptor::ServiceDescriptor,
    disclosure::SelectedAgreement,
    hashing::TRANSCRIPT_HASH_VERSION,
    identity::{AuthorizationIdentity, TransportIdentity},
    negotiation::{bootstrap::NegotiationBootstrap, peer::NegotiationPeer, Proposal},
    socket::SocketChannel,
    store::FileTranscriptStore,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use zeroize::Zeroizing;

const MAX_REQUEST: usize = 16 * 1024;

#[derive(Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Version {},
    Negotiate {
        config_file: PathBuf,
        operation_ref: String,
        #[serde(default)]
        freeze_only: bool,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
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
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OperationId(String);
impl fmt::Debug for OperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OperationId(<redacted>)")
    }
}
impl fmt::Display for OperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl RecordId for OperationId {
    fn as_file_stem(&self) -> &str {
        &self.0
    }
    fn from_file_stem(stem: &str) -> Option<Self> {
        (stem.len() == 64
            && stem
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            && stem.bytes().any(|byte| byte != b'0'))
        .then(|| Self(stem.into()))
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    id: OperationId,
    role: u8,
    draft: Vec<u8>,
    buyer: Option<Vec<u8>>,
    seller: Option<Vec<u8>>,
    evidence: bool,
}
impl JournalRecord for Record {
    type Id = OperationId;
    const CURRENT_VERSION: u32 = 1;
    const OLDEST_READABLE_VERSION: u32 = 1;
    fn version(&self) -> u32 {
        self.version
    }
    fn record_id(&self) -> &OperationId {
        &self.id
    }
    fn attempt_count(&self) -> usize {
        1
    }
}

fn now() -> Result<u64, &'static str> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "local clock unavailable")?
        .as_secs())
}

fn read(path: &Path, limit: usize, private: bool) -> Result<Zeroizing<Vec<u8>>, &'static str> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "local input unavailable")?;
    if !metadata.file_type().is_file() || metadata.len() > limit as u64 {
        return Err("invalid local input");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if private && metadata.permissions().mode() & 0o077 != 0 {
            return Err("local input must have owner-only permissions");
        }
    }
    #[cfg(not(unix))]
    let _ = private;
    let mut bytes = Zeroizing::new(Vec::new());
    fs::File::open(path)
        .and_then(|file| file.take((limit + 1) as u64).read_to_end(&mut bytes))
        .map_err(|_| "local input unavailable")?;
    if bytes.len() > limit {
        return Err("local input exceeds limit");
    }
    Ok(bytes)
}

fn key(path: &Path) -> Result<Zeroizing<[u8; 32]>, &'static str> {
    let bytes = read(path, 32, true)?;
    Ok(Zeroizing::new(
        bytes
            .as_slice()
            .try_into()
            .map_err(|_| "key must contain exactly 32 raw bytes")?,
    ))
}

fn descriptor(path: &Path) -> Result<ServiceDescriptor, &'static str> {
    let bytes = read(path, 16 * 1024, false)?;
    ServiceDescriptor::from_json(std::str::from_utf8(&bytes).map_err(|_| "invalid descriptor")?)
        .map_err(|_| "invalid descriptor")
}

fn sign_commitment(
    terms: &AgreementTerms,
    commitment: erebus_core::commitment::DealCommitment,
    seed: &[u8; 32],
    expected: &KeyBytes,
    role: Role,
) -> Result<Authorization, &'static str> {
    let bytes = if terms.suite_id == 1 {
        let signer =
            AuthorizationIdentity::from_bytes(seed).map_err(|_| "invalid agreement key")?;
        if signer.address().as_slice() != expected.as_bytes() {
            return Err("agreement key does not match identity");
        }
        let digest = authorization_digest(&terms.domain, role, &commitment, 1)
            .map_err(|_| "invalid signature domain")?;
        signer.sign_digest(&digest).to_vec()
    } else {
        if derive_key(seed)
            .map_err(|_| "invalid agreement key")?
            .as_slice()
            != expected.as_bytes()
        {
            return Err("agreement key does not match identity");
        }
        let message = authorization_message(&terms.domain, role, &commitment)
            .map_err(|_| "invalid signature domain")?;
        sign_message(seed, &message)
            .map_err(|_| "agreement signing failed")?
            .1
            .to_vec()
    };
    Ok(Authorization {
        role,
        suite_id: terms.suite_id,
        commitment,
        signature: SignatureBytes::new(bytes).map_err(|_| "invalid signature")?,
    })
}

fn persist_auth(
    store: &Store<Record>,
    record: &mut Record,
    auth: &Authorization,
) -> Result<(), &'static str> {
    let bytes = auth.encode().map_err(|_| "invalid authorization")?;
    let slot = if auth.role == Role::Buyer {
        &mut record.buyer
    } else {
        &mut record.seller
    };
    if slot.as_ref().is_some_and(|retained| retained != &bytes) {
        return Err("authorization conflicts with durable state");
    }
    *slot = Some(bytes);
    store
        .write(record)
        .map_err(|_| "authorization persistence failed")
}

fn check_service(
    received: &Proposal,
    template: &AgreementTerms,
    lifetime: u64,
) -> Result<(), &'static str> {
    let terms = received.terms();
    let current = now()?;
    if terms.expiry <= current
        || terms.expiry > current.checked_add(lifetime).ok_or("invalid deadline")?
    {
        return Err("offer deadline exceeds service policy");
    }
    let mut expected = template.clone();
    expected.deal_id = terms.deal_id;
    expected.settlement_nonce = terms.settlement_nonce;
    expected.amount = terms.amount;
    expected.expiry = terms.expiry;
    expected.revision = 1;
    expected.transcript_root = [0; 32];
    expected.buyer_authorization_key = terms.buyer_authorization_key.clone();
    expected.seller_authorization_key = terms.seller_authorization_key.clone();
    expected.payment_recipient = terms.payment_recipient.clone();
    expected.service.access_recipient = terms.buyer_authorization_key.clone();
    if expected != *terms {
        return Err("offer does not match the configured service promise");
    }
    Ok(())
}

fn connect(config: &Config, role: Role, timeout: Duration) -> Result<TcpStream, &'static str> {
    if role == Role::Buyer {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("negotiation endpoint unavailable");
            }
            match TcpStream::connect_timeout(
                &config.endpoint,
                remaining.min(Duration::from_millis(100)),
            ) {
                Ok(stream) => return Ok(stream),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    thread::sleep(remaining.min(Duration::from_millis(25)));
                }
                Err(_) => return Err("negotiation endpoint unavailable"),
            }
        }
    }
    let listener =
        TcpListener::bind(config.endpoint).map_err(|_| "negotiation listener unavailable")?;
    listener
        .set_nonblocking(true)
        .map_err(|_| "negotiation listener unavailable")?;
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((stream, _)) => return Ok(stream),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err("negotiation connection deadline exceeded");
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return Err("negotiation listener unavailable"),
        }
    }
}

fn negotiate(
    config_file: &Path,
    operation: &str,
    freeze_only: bool,
) -> Result<Value, &'static str> {
    let id = OperationId::from_file_stem(operation).ok_or("invalid operation reference")?;
    let operation_ref: [u8; 32] = hex::decode(operation)
        .map_err(|_| "invalid operation reference")?
        .try_into()
        .map_err(|_| "invalid operation reference")?;
    let config: Config = serde_json::from_slice(&read(config_file, MAX_REQUEST, true)?)
        .map_err(|_| "invalid operator configuration")?;
    if config.version != 1
        || !(1..=300).contains(&config.timeout_seconds)
        || !(1..=86400).contains(&config.max_deal_lifetime_seconds)
    {
        return Err("invalid operator configuration");
    }
    let role = match config.role.as_str() {
        "buyer" => Role::Buyer,
        "seller" => Role::Seller,
        _ => return Err("invalid configured role"),
    };
    let maximum = BaseUnits::new(
        config
            .maximum_price
            .parse()
            .map_err(|_| "invalid price policy")?,
    );
    let minimum = BaseUnits::new(
        config
            .minimum_price
            .parse()
            .map_err(|_| "invalid price policy")?,
    );
    let template = AgreementTerms::decode(&read(&config.terms_template_file, 4096, true)?)
        .map_err(|_| "invalid service template")?;
    if maximum.is_zero()
        || minimum > maximum
        || template.amount > maximum
        || (role == Role::Seller && template.amount < minimum)
    {
        return Err("invalid price policy");
    }
    let local = descriptor(&config.local_descriptor_file)?;
    let remote = descriptor(&config.peer_descriptor_file)?;
    let verified_at = now()?;
    local
        .verify(verified_at)
        .map_err(|_| "local discovery descriptor is invalid or expired")?;
    remote
        .verify(verified_at)
        .map_err(|_| "peer discovery descriptor is invalid or expired")?;
    if template.suite_id == 2
        && role == Role::Seller
        && (config.seller_spend_secret_file.is_none()
            || config.seller_wallet_file.is_none()
            || config.seller_wallet_key_file.is_none())
    {
        return Err("shielded seller requires a private payment opening and wallet");
    }
    let endpoint = format!("tcp://{}", config.endpoint);
    let advertised = if role == Role::Buyer { &remote } else { &local };
    if !advertised.endpoints.contains(&endpoint) {
        return Err("endpoint not present in the authenticated descriptor");
    }
    let seed = key(&config.agreement_key_file)?;
    let discovery_seed = if template.suite_id == 1 {
        Zeroizing::new(*seed)
    } else {
        key(config
            .discovery_key_file
            .as_ref()
            .ok_or("shielded discovery identity missing")?)?
    };
    let discovery = AuthorizationIdentity::from_bytes(discovery_seed.as_ref())
        .map_err(|_| "invalid discovery key")?;
    if discovery.address() != local.seller_address {
        return Err("discovery key does not match local descriptor");
    }
    let transport = TransportIdentity::from_private_key(*key(&config.transport_key_file)?)
        .map_err(|_| "invalid transport identity")?;
    let store = Store::<Record>::open(config.state_root.join("agent"))
        .map_err(|_| "agent state unavailable")?;
    // Serializes this operator's driver calls, including reconnect and signature delivery.
    let _lease = store
        .lock_identity()
        .map_err(|_| "agent state unavailable")?;
    let _operation_lease = store
        .lock_record(&id)
        .map_err(|_| "agent state unavailable")?;
    let retained = store.read(&id).map_err(|_| "agent state unavailable")?;
    if retained
        .as_ref()
        .is_some_and(|record| record.role != role.tag())
    {
        return Err("operation role conflicts with durable state");
    }
    let timeout = Duration::from_secs(config.timeout_seconds);
    let stream = connect(&config, role, timeout)?;
    let (buyer, seller) = if role == Role::Buyer {
        (&local, &remote)
    } else {
        (&remote, &local)
    };
    let current = now()?;
    let channel = if role == Role::Buyer {
        SocketChannel::buyer(stream, &transport, buyer, seller, current, timeout)
    } else {
        SocketChannel::seller(stream, &transport, seller, buyer, current, timeout)
    }
    .map_err(|error| match error {
        erebus_transport::socket::SocketError::Authentication => {
            "peer cryptographic authentication failed"
        }
        erebus_transport::socket::SocketError::Configuration => {
            "invalid peer identity configuration"
        }
        erebus_transport::socket::SocketError::Frame => "invalid peer handshake frame",
        _ => "peer connection unavailable or timed out",
    })?;
    let context = SettlementContext {
        require_local_proving: true,
        mode: template.settlement_mode,
        domain: template.domain.clone(),
        suite_id: template.suite_id,
        asset: template.asset.clone(),
        required_guarantees: template.required_guarantees,
    };
    let mut spend_secret = None;
    let bootstrap = if template.suite_id == 1 {
        NegotiationBootstrap::public_bound(channel, context.clone(), buyer, seller, current)
    } else {
        let agreement_key = KeyBytes::new(
            derive_key(&seed)
                .map_err(|_| "invalid shielded key")?
                .to_vec(),
        )
        .map_err(|_| "invalid shielded key")?;
        let expires = local.expires.min(
            current
                .checked_add(config.max_deal_lifetime_seconds)
                .ok_or("invalid deadline")?,
        );
        let binding = AgreementKeyBinding::sign(
            &local,
            context.domain.clone(),
            agreement_key,
            &discovery,
            current,
            expires,
        )
        .map_err(|_| "cannot attest shielded identity")?;
        let recipient = if role == Role::Seller {
            let secret = key(config
                .seller_spend_secret_file
                .as_ref()
                .ok_or("seller spending secret missing")?)?;
            let tag = KeyBytes::new(
                note_spend_tag(&secret)
                    .map_err(|_| "invalid seller spending secret")?
                    .to_vec(),
            )
            .map_err(|_| "invalid seller spending tag")?;
            spend_secret = Some(secret);
            Some(tag)
        } else {
            None
        };
        NegotiationBootstrap::shielded(
            channel,
            context.clone(),
            buyer,
            seller,
            &binding,
            recipient,
            current,
        )
    }
    .map_err(|_| "negotiation identity binding failed")?;
    let transcripts = FileTranscriptStore::open(config.state_root.join("transcripts"))
        .map_err(|_| "transcript unavailable")?;
    let mut record = retained.unwrap_or(Record {
        version: 1,
        id,
        role: role.tag(),
        draft: Vec::new(),
        buyer: None,
        seller: None,
        evidence: false,
    });
    let mut peer = if role == Role::Buyer {
        let draft = if record.draft.is_empty() {
            let mut terms = template.clone();
            terms.expiry = now()?
                .checked_add(config.max_deal_lifetime_seconds)
                .ok_or("invalid deadline")?;
            let proposal = bootstrap
                .create_proposal(terms, now()?)
                .map_err(|_| "cannot create private proposal")?;
            record.draft = proposal.encode().map_err(|_| "invalid private proposal")?;
            store
                .write(&record)
                .map_err(|_| "proposal persistence failed")?;
            proposal
        } else {
            let proposal =
                Proposal::decode(&record.draft).map_err(|_| "invalid retained proposal")?;
            check_service(&proposal, &template, config.max_deal_lifetime_seconds)?;
            proposal
        };
        bootstrap
            .offer(transcripts.clone(), "agent".into(), draft, now()?)
            .map_err(|error| match error {
                erebus_transport::negotiation::peer::PeerError::Binding => {
                    "retained offer does not match authenticated identities"
                }
                erebus_transport::negotiation::peer::PeerError::Negotiation(_) => {
                    "offer transcript persistence failed; retain existing state"
                }
                _ => "offer delivery uncertain; retain this operation and reconnect",
            })?
    } else {
        bootstrap
            .receive_offer(transcripts.clone(), "agent".into(), now()?, |proposal| {
                check_service(proposal, &template, config.max_deal_lifetime_seconds)?;
                let bytes = proposal.encode().map_err(|_| "invalid received proposal")?;
                if !record.draft.is_empty() && record.draft != bytes {
                    return Err("operation conflicts with retained proposal");
                }
                record.draft = bytes;
                store
                    .write(&record)
                    .map_err(|_| "proposal persistence failed")
            })
            .map_err(|_| "offer rejected or persistence failed; retain existing state")?
    };
    let state = peer
        .synchronize(now()?)
        .map_err(|_| "transcript reconciliation failed; retain existing state")?;
    if !state.is_frozen() {
        finish_price_negotiation(&mut peer, role, &template, minimum, maximum)?;
    }
    let (terms, blinding, commitment) = peer
        .state()
        .map_err(|_| "transcript unavailable")?
        .agreement()
        .map_err(|_| "bilateral acceptance incomplete")?;
    let nullifier = deal_nullifier(&terms).map_err(|_| "invalid agreed deal")?;
    if freeze_only {
        return Ok(
            json!({"protocol_version":1,"status":"frozen","operation_ref":operation,"deal_id":hex::encode(terms.deal_id),"deal_commitment":commitment.to_hex(),"deal_nullifier":nullifier.to_hex(),"payment_verified":false,"delivery_verified":false}),
        );
    }
    if role == Role::Buyer {
        let capabilities = if terms.suite_id == 1 {
            erebus_evm::backend::public_bound_capabilities()
        } else {
            preparation::capabilities()
        };
        let coordinator = Coordinator::open(
            config.state_root.join("coordinator"),
            terms.buyer_authorization_key.clone(),
            context.clone(),
            &context,
            &capabilities,
            SpendingPolicy {
                per_deal_max: maximum,
                allowed_assets: [terms.asset.clone()].into(),
                ..Default::default()
            },
        )
        .map_err(|_| "coordinator policy or identity mismatch")?;
        coordinator
            .record_intent(operation_ref, &terms, &blinding, now()?)
            .map_err(|_| "spending reservation denied; retain existing state")?;
        let auth = coordinator
            .authorize_buyer(operation_ref, now()?, |agreed, opening| {
                let commitment =
                    commit_agreement(agreed, opening).map_err(|_| "invalid agreement")?;
                sign_commitment(
                    agreed,
                    commitment,
                    &seed,
                    &agreed.buyer_authorization_key,
                    Role::Buyer,
                )
            })
            .map_err(|_| "buyer signing failed; reservation retained")?;
        peer.send_authorization(&auth, now()?, |auth| {
            persist_auth(&store, &mut record, auth)
        })
        .map_err(|_| "buyer authorization delivery uncertain; reservation retained")?;
        peer.receive_authorization(now()?, |auth| {
            coordinator
                .accept_seller(operation_ref, auth)
                .map_err(|_| "seller authorization rejected")?;
            persist_auth(&store, &mut record, auth)
        })
        .map_err(|_| "seller authorization unavailable; reservation retained")?;
    } else {
        peer.receive_authorization(now()?, |auth| persist_auth(&store, &mut record, auth))
            .map_err(|_| "buyer authorization unavailable")?;
        if let Some(secret) = spend_secret {
            let wallet_file = config
                .seller_wallet_file
                .as_ref()
                .ok_or("seller wallet missing")?;
            let encryption_key = key(config
                .seller_wallet_key_file
                .as_ref()
                .ok_or("seller wallet key missing")?)?;
            let pool = context
                .domain
                .pool
                .as_ref()
                .ok_or("invalid pool domain")?
                .as_bytes()
                .try_into()
                .map_err(|_| "invalid pool domain")?;
            let chain_id = context
                .domain
                .namespace
                .reference()
                .parse()
                .map_err(|_| "invalid chain id")?;
            let wallet = WalletStore::new(
                wallet_file,
                WalletDomain { chain_id, pool },
                *encryption_key,
            )
            .map_err(|_| "seller wallet unavailable")?;
            let expected = OwnedNote::expected_payment(&terms, *secret)
                .map_err(|_| "seller cannot recover the agreed payment")?;
            wallet
                .update(|snapshot| {
                    if snapshot
                        .notes()
                        .iter()
                        .any(|note| note.commitment() == expected.commitment())
                    {
                        return Ok(());
                    }
                    snapshot.add(expected)
                })
                .map_err(|_| "payment opening persistence failed; seller consent withheld")?;
        }
        let auth = if let Some(bytes) = &record.seller {
            Authorization::decode(bytes).map_err(|_| "invalid retained seller authorization")?
        } else {
            sign_commitment(
                &terms,
                commitment,
                &seed,
                &terms.seller_authorization_key,
                Role::Seller,
            )?
        };
        peer.send_authorization(&auth, now()?, |auth| {
            persist_auth(&store, &mut record, auth)
        })
        .map_err(|_| "seller authorization delivery uncertain; retain this operation")?;
    }
    let buyer_auth =
        Authorization::decode(record.buyer.as_deref().ok_or("buyer consent not durable")?)
            .map_err(|_| "invalid retained buyer consent")?;
    let seller_auth = Authorization::decode(
        record
            .seller
            .as_deref()
            .ok_or("seller consent not durable")?,
    )
    .map_err(|_| "invalid retained seller consent")?;
    let evidence = SelectedAgreement::from_store(
        terms.clone(),
        blinding,
        buyer_auth,
        seller_auth,
        TRANSCRIPT_HASH_VERSION,
        &transcripts,
        "agent",
    )
    .map_err(|_| "durable agreement verification failed")?;
    let encoded = evidence
        .encode()
        .map_err(|_| "disclosure evidence encoding failed")?;
    store
        .write_blob_then_record(&mut record, 0, &encoded, |record| {
            record.evidence = true;
            Ok::<_, StoreError<OperationId>>(())
        })
        .map_err(|_| "disclosure evidence persistence failed")?;
    Ok(
        json!({"protocol_version":1,"status":"authorized","operation_ref":operation,"deal_id":hex::encode(terms.deal_id),"deal_commitment":commitment.to_hex(),"deal_nullifier":nullifier.to_hex(),"agreement_verified":true,"evidence_file":store.blob_path(&record.id,0),"payment_verified":false,"delivery_verified":false}),
    )
}

fn finish_price_negotiation(
    peer: &mut NegotiationPeer,
    role: Role,
    template: &AgreementTerms,
    minimum: BaseUnits,
    maximum: BaseUnits,
) -> Result<(), &'static str> {
    let state = peer.state().map_err(|_| "transcript unavailable")?;
    let latest = state.latest().ok_or("initial offer unavailable")?;
    if role == Role::Seller {
        if latest.terms().revision == 1 {
            if state.transcript().next_sequence(Role::Seller) == 1 {
                if latest.terms().amount < minimum {
                    peer.propose(template.amount, now()?)
                        .map_err(|_| "counter delivery uncertain")?;
                } else {
                    peer.accept(now()?)
                        .map_err(|_| "acceptance delivery uncertain")?;
                }
            }
            peer.receive(now()?)
                .map_err(|_| "buyer acceptance unavailable")?;
        } else if state.transcript().next_sequence(Role::Buyer) < 3 {
            peer.receive(now()?)
                .map_err(|_| "buyer acceptance unavailable")?;
        }
        if !peer
            .state()
            .map_err(|_| "transcript unavailable")?
            .is_frozen()
        {
            peer.accept(now()?)
                .map_err(|_| "acceptance delivery uncertain")?;
        }
    } else {
        if latest.terms().revision == 1 && state.transcript().next_sequence(Role::Seller) == 1 {
            peer.receive(now()?)
                .map_err(|_| "seller response unavailable")?;
        }
        let state = peer.state().map_err(|_| "transcript unavailable")?;
        if state
            .latest()
            .ok_or("seller response unavailable")?
            .terms()
            .amount
            > maximum
        {
            return Err("seller price exceeds buyer policy; no payment authorized");
        }
        if state.transcript().next_sequence(Role::Buyer) < 3 {
            peer.accept(now()?)
                .map_err(|_| "acceptance delivery uncertain")?;
        }
        if !peer
            .state()
            .map_err(|_| "transcript unavailable")?
            .is_frozen()
        {
            peer.receive(now()?)
                .map_err(|_| "seller acceptance unavailable")?;
        }
    }
    Ok(())
}

fn main() {
    let result = (|| {
        if std::env::args().len() != 1 {
            return Err("provide one JSON request on stdin");
        }
        let mut bytes = Zeroizing::new(Vec::new());
        std::io::stdin()
            .take((MAX_REQUEST + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| "cannot read request")?;
        if bytes.len() > MAX_REQUEST {
            return Err("request exceeds size limit");
        }
        match serde_json::from_slice(&bytes).map_err(|_| "invalid request")? {
            Request::Version {} => Ok(
                json!({"protocol_version":1,"status":"ok","service":"erebus-negotiate","settlement_submission":false}),
            ),
            Request::Negotiate {
                config_file,
                operation_ref,
                freeze_only,
            } => negotiate(&config_file, &operation_ref, freeze_only),
        }
    })();
    let failed = result.is_err();
    let response = result.unwrap_or_else(|error| json!({"protocol_version":1,"status":"error","error":error,"payment_verified":false,"delivery_verified":false}));
    println!("{response}");
    let flushed = std::io::stdout().flush().is_ok();
    std::process::exit(i32::from(failed || !flushed));
}
