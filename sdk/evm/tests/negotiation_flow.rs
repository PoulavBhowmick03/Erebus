//! Process-level negotiation -> policy-reserved authorization -> scoped disclosure evidence.
//! No chain payment is claimed by this test; funded settlement remains a separate gate.

use std::{
    convert::Infallible,
    fmt, fs,
    io::Write,
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use erebus_coordinator::Coordinator;
use erebus_core::{
    auth::{authorization_digest, Authorization, Role},
    commitment::{CommitmentBlinding, DealCommitment},
    ids::{BaseUnits, SignatureBytes},
    policy::{ReservationState, SpendingPolicy},
    settlement::{BackendCapabilities, SettlementContext},
    terms::AgreementTerms,
};
use erebus_journal::{JournalRecord, RecordId, Store};
use erebus_transport::{
    descriptor::{Directory, ServiceDescriptor, MODE_PUBLIC_BOUND},
    disclosure::{verify_selected_agreement, DisclosureGrant, SelectedAgreement},
    hashing::TRANSCRIPT_HASH_VERSION,
    identity::{AuthorizationIdentity, DisclosureIdentity, TransportIdentity},
    negotiation::{bootstrap::NegotiationBootstrap, Proposal},
    socket::SocketChannel,
    store::{FileTranscriptStore, TranscriptStore},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const NOW: u64 = 1_700_000_000;
const OPERATION: [u8; 32] = [11; 32];

fn initial() -> Proposal {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../core/tests/fixtures/agreement-v1-vectors.json"
    ))
    .unwrap();
    let canonical = hex::decode(
        fixture["vectors"][0]["expected"]["canonicalHex"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let mut terms = AgreementTerms::decode(&canonical).unwrap();
    terms.amount = BaseUnits::new(60);
    terms.transcript_root = [0; 32];
    Proposal::new(terms, CommitmentBlinding::from_bytes([3; 32])).unwrap()
}

fn identity(role: Role) -> AuthorizationIdentity {
    AuthorizationIdentity::from_bytes(&[role.tag(); 32]).unwrap()
}

fn signed(
    terms: &AgreementTerms,
    commitment: DealCommitment,
    role: Role,
    key: &AuthorizationIdentity,
) -> Authorization {
    let digest = authorization_digest(&terms.domain, role, &commitment, 1).unwrap();
    Authorization {
        role,
        suite_id: 1,
        commitment,
        signature: SignatureBytes::new(key.sign_digest(&digest).to_vec()).unwrap(),
    }
}

fn descriptor(role: Role, transport: &TransportIdentity, endpoint: &str) -> ServiceDescriptor {
    let draft = initial();
    let mut descriptor = ServiceDescriptor::new(
        identity(role).address(),
        transport.public_key(),
        vec![endpoint.into()],
        draft.terms().domain.namespace.clone(),
        vec![draft.terms().asset.clone()],
        vec![1],
        MODE_PUBLIC_BOUND,
        draft.terms().required_guarantees,
        NOW - 1,
        NOW + 1000,
    )
    .unwrap();
    descriptor.sign(&identity(role)).unwrap();
    descriptor
}

fn private(path: &Path, bytes: &[u8]) {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
    fs::File::open(path.parent().unwrap())
        .unwrap()
        .sync_all()
        .unwrap();
}

struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn worker(root: &Path, role: &str, phase: &str, address: &str) -> Worker {
    Worker(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "subprocess_agent_worker",
                "--nocapture",
            ])
            .env("EREBUS_M8_NEGOTIATION_ROOT", root)
            .env("EREBUS_M8_NEGOTIATION_ROLE", role)
            .env("EREBUS_M8_NEGOTIATION_PHASE", phase)
            .env("EREBUS_M8_NEGOTIATION_ADDRESS", address)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    )
}

fn wait(worker: &mut Worker) {
    let started = Instant::now();
    loop {
        if let Some(status) = worker.0.try_wait().unwrap() {
            assert!(status.success());
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "negotiation worker stalled"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn run_pair(root: &Path, phase: &str) {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let address = format!("127.0.0.1:{port}");
    let mut seller = worker(root, "seller", phase, &address);
    let ready = root.join("seller").join(format!("ready-{phase}"));
    let started = Instant::now();
    while !ready.exists() {
        assert!(
            seller.0.try_wait().unwrap().is_none(),
            "seller failed before listen"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "seller did not listen"
        );
        thread::sleep(Duration::from_millis(10));
    }
    let mut buyer = worker(root, "buyer", phase, &address);
    wait(&mut buyer);
    wait(&mut seller);
}

#[test]
fn independent_agents_freeze_restart_authorize_and_disclose_without_a_signature_cycle() {
    let root = tempfile::tempdir().unwrap();
    let transport_buyer = TransportIdentity::from_private_key([21; 32]).unwrap();
    let transport_seller = TransportIdentity::from_private_key([22; 32]).unwrap();
    for (name, role, transport) in [
        ("buyer", Role::Buyer, &transport_buyer),
        ("seller", Role::Seller, &transport_seller),
    ] {
        let directory = root.path().join(name);
        fs::create_dir(&directory).unwrap();
        transport.store(directory.join("transport.key")).unwrap();
        private(&directory.join("authorization.key"), &[role.tag(); 32]);
    }
    let buyer = descriptor(
        Role::Buyer,
        &transport_buyer,
        "tcp://buyer.example/negotiation",
    );
    let seller = descriptor(
        Role::Seller,
        &transport_seller,
        "tcp://seller.example/negotiation",
    );
    let directory = Directory::new(vec![buyer, seller], NOW).unwrap();
    private(
        &root.path().join("directory.json"),
        directory.to_json().unwrap().as_bytes(),
    );
    let auditor = DisclosureIdentity::generate_and_store(root.path().join("auditor.key")).unwrap();
    private(&root.path().join("auditor.public"), &auditor.public_key());
    run_pair(root.path(), "freeze");
    run_pair(root.path(), "authorize");
    let buyer_before: Value =
        serde_json::from_slice(&fs::read(root.path().join("buyer/freeze.json")).unwrap()).unwrap();
    let seller_before: Value =
        serde_json::from_slice(&fs::read(root.path().join("seller/freeze.json")).unwrap()).unwrap();
    let buyer_after: Value =
        serde_json::from_slice(&fs::read(root.path().join("buyer/authorize.json")).unwrap())
            .unwrap();
    let seller_after: Value =
        serde_json::from_slice(&fs::read(root.path().join("seller/authorize.json")).unwrap())
            .unwrap();
    assert_eq!(buyer_before["root"], seller_before["root"]);
    assert_eq!(buyer_before["root"], buyer_after["root"]);
    assert_eq!(buyer_after["root"], seller_after["root"]);
    assert_ne!(buyer_before["session"], buyer_after["session"]);
    assert_eq!(buyer_after["session"], seller_after["session"]);
    assert_eq!(buyer_after["commitment"], seller_after["commitment"]);
    assert_eq!(buyer_after["reserved_total"], 70);
    assert_eq!(buyer_after["payment_verified"], false);
    let mut auditor_worker = worker(root.path(), "auditor", "audit", "unused");
    wait(&mut auditor_worker);
    let audited: Value =
        serde_json::from_slice(&fs::read(root.path().join("audit.json")).unwrap()).unwrap();
    assert_eq!(audited["commitment"], buyer_after["commitment"]);
    assert_eq!(audited["agreement_verified"], true);
    assert_eq!(audited["payment_verified"], false);
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct AuthId;
impl fmt::Display for AuthId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("final-authorizations")
    }
}
impl RecordId for AuthId {
    fn as_file_stem(&self) -> &str {
        "final-authorizations"
    }
    fn from_file_stem(stem: &str) -> Option<Self> {
        (stem == "final-authorizations").then_some(Self)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthRecord {
    version: u32,
    id: AuthId,
    buyer: Option<Vec<u8>>,
    seller: Option<Vec<u8>>,
}
impl JournalRecord for AuthRecord {
    type Id = AuthId;
    const CURRENT_VERSION: u32 = 1;
    const OLDEST_READABLE_VERSION: u32 = 1;
    fn version(&self) -> u32 {
        self.version
    }
    fn record_id(&self) -> &AuthId {
        &self.id
    }
    fn attempt_count(&self) -> usize {
        1
    }
}

fn persist_auth(journal: &Store<AuthRecord>, authorization: &Authorization) -> Result<(), ()> {
    let _identity = journal.lock_identity().map_err(|_| ())?;
    let _record = journal.lock_record(&AuthId).map_err(|_| ())?;
    let mut record = journal
        .read(&AuthId)
        .map_err(|_| ())?
        .unwrap_or(AuthRecord {
            version: 1,
            id: AuthId,
            buyer: None,
            seller: None,
        });
    let slot = if authorization.role == Role::Buyer {
        &mut record.buyer
    } else {
        &mut record.seller
    };
    let encoded = authorization.encode().map_err(|_| ())?;
    if slot.as_ref().is_some_and(|old| old != &encoded) {
        return Err(());
    }
    *slot = Some(encoded);
    journal.write(&record).map_err(|_| ())
}

#[test]
#[ignore = "invoked by process-level negotiation test"]
fn subprocess_agent_worker() {
    let Some(root) = std::env::var_os("EREBUS_M8_NEGOTIATION_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let role_text = std::env::var("EREBUS_M8_NEGOTIATION_ROLE").unwrap();
    if role_text == "auditor" {
        let identity = DisclosureIdentity::load(root.join("auditor.key")).unwrap();
        let grant = DisclosureGrant::load_backup(root.join("deal.grant")).unwrap();
        let (agreement, _) = grant
            .open(&identity, identity_seller_address(), NOW)
            .unwrap();
        let verified = verify_selected_agreement(&agreement).unwrap();
        private(&root.join("audit.json"), json!({"commitment": verified.commitment.to_hex(), "agreement_verified": true, "payment_verified": false}).to_string().as_bytes());
        return;
    }
    let role = if role_text == "buyer" {
        Role::Buyer
    } else {
        assert_eq!(role_text, "seller");
        Role::Seller
    };
    let phase = std::env::var("EREBUS_M8_NEGOTIATION_PHASE").unwrap();
    let address = std::env::var("EREBUS_M8_NEGOTIATION_ADDRESS").unwrap();
    let local = root.join(role_text);
    let key =
        AuthorizationIdentity::from_bytes(&fs::read(local.join("authorization.key")).unwrap())
            .unwrap();
    let transport = TransportIdentity::load(local.join("transport.key")).unwrap();
    let directory = Directory::from_json(
        &fs::read_to_string(root.join("directory.json")).unwrap(),
        NOW,
    )
    .unwrap();
    let buyer = directory
        .descriptors()
        .iter()
        .find(|entry| {
            entry.seller_address.as_slice() == initial().terms().buyer_authorization_key.as_bytes()
        })
        .unwrap();
    let seller = directory
        .descriptors()
        .iter()
        .find(|entry| entry.seller_address == identity_seller_address())
        .unwrap();
    // Peer lookup uses public agreement keys; this process loads only its own signing file.
    let stream = if role == Role::Seller {
        let listener = TcpListener::bind(&address).unwrap();
        private(&local.join(format!("ready-{phase}")), b"ready");
        listener.accept().unwrap().0
    } else {
        TcpStream::connect(address).unwrap()
    };
    let channel = if role == Role::Buyer {
        SocketChannel::buyer(
            stream,
            &transport,
            buyer,
            seller,
            NOW,
            Duration::from_secs(3),
        )
        .unwrap()
    } else {
        SocketChannel::seller(
            stream,
            &transport,
            seller,
            buyer,
            NOW,
            Duration::from_secs(3),
        )
        .unwrap()
    };
    let session = channel.session_id();
    let transcript_store = FileTranscriptStore::open(local.join("transcripts")).unwrap();
    let template = initial();
    let terms = template.terms();
    let context = SettlementContext {
        require_local_proving: false,
        mode: terms.settlement_mode,
        domain: terms.domain.clone(),
        suite_id: 1,
        asset: terms.asset.clone(),
        required_guarantees: terms.required_guarantees,
    };
    let bootstrap =
        NegotiationBootstrap::public_bound(channel, context, buyer, seller, NOW).unwrap();
    let mut peer = if role == Role::Buyer {
        let proposal = if phase == "freeze" {
            bootstrap
                .create_proposal(template.terms().clone(), NOW)
                .unwrap()
        } else {
            let before: Value =
                serde_json::from_slice(&fs::read(local.join("freeze.json")).unwrap()).unwrap();
            let deal_id = hex::decode(before["deal_id"].as_str().unwrap())
                .unwrap()
                .try_into()
                .unwrap();
            let retained = transcript_store.messages("agent", deal_id).unwrap();
            Proposal::from_initial_offer(&retained[0], NOW).unwrap()
        };
        bootstrap
            .offer(transcript_store.clone(), "agent".into(), proposal, NOW)
            .unwrap()
    } else {
        bootstrap
            .receive_offer(transcript_store.clone(), "agent".into(), NOW, |received| {
                // The seller pins the service, fees, identity, and deadline, not buyer randomness.
                let terms = received.terms();
                if terms.service != template.terms().service
                    || terms.fee_policy != template.terms().fee_policy
                    || terms.expiry != template.terms().expiry
                    || terms.amount > BaseUnits::new(75)
                {
                    return Err(());
                }
                assert_ne!(terms.deal_id, template.terms().deal_id);
                assert_ne!(terms.settlement_nonce, template.terms().settlement_nonce);
                assert_ne!(received.blinding(), template.blinding());
                Ok(())
            })
            .unwrap()
    };
    if phase == "freeze" {
        if role == Role::Buyer {
            peer.receive(NOW).unwrap();
            peer.accept(NOW).unwrap();
            peer.receive(NOW).unwrap();
        } else {
            peer.propose(BaseUnits::new(70), NOW).unwrap();
            peer.receive(NOW).unwrap();
            peer.accept(NOW).unwrap();
        }
        let state = peer.state().unwrap();
        assert!(state.is_frozen());
        private(&local.join("freeze.json"), json!({"deal_id": hex::encode(state.agreement().unwrap().0.deal_id), "root": hex::encode(state.agreement().unwrap().0.transcript_root), "session": hex::encode(session)}).to_string().as_bytes());
        return;
    }
    assert_eq!(phase, "authorize");
    // Reconcile retained events after a fresh handshake; no additional log entries.
    peer.synchronize(NOW).unwrap();
    let state = peer.state().unwrap();
    let (terms, blinding, commitment) = state.agreement().unwrap();
    let mut reserved_total = 0;
    if role == Role::Buyer {
        let context = SettlementContext {
            require_local_proving: false,
            mode: terms.settlement_mode,
            domain: terms.domain.clone(),
            suite_id: 1,
            asset: terms.asset.clone(),
            required_guarantees: terms.required_guarantees,
        };
        let capabilities = BackendCapabilities {
            suites: [1].into(),
            modes: [terms.settlement_mode].into(),
            guarantees: terms.required_guarantees,
            local_proving: false,
        };
        let coordinator = Coordinator::open(
            local.join("coordinator"),
            terms.buyer_authorization_key.clone(),
            context.clone(),
            &context,
            &capabilities,
            SpendingPolicy {
                per_deal_max: BaseUnits::new(75),
                allowed_assets: [terms.asset.clone()].into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            coordinator
                .record_intent(OPERATION, &terms, &blinding, NOW)
                .unwrap(),
            commitment
        );
        let authorization = coordinator
            .authorize_buyer(OPERATION, NOW, |terms, _| {
                Ok::<_, Infallible>(signed(terms, commitment, Role::Buyer, &key))
            })
            .unwrap();
        peer.send_authorization(&authorization, NOW, |auth| {
            assert_eq!(
                coordinator
                    .authorize_buyer(OPERATION, NOW, |_, _| Err::<Authorization, ()>(()))
                    .unwrap(),
                auth.clone()
            );
            Ok::<_, ()>(())
        })
        .unwrap();
        peer.receive_authorization(NOW, |auth| coordinator.accept_seller(OPERATION, auth))
            .unwrap();
        let ledger = coordinator.ledger().unwrap();
        reserved_total = ledger.reserved_total(&terms.asset).unwrap().get();
        assert_eq!(ledger.reservations().len(), 1);
        assert_eq!(ledger.reservations()[0].state, ReservationState::Reserved);
        assert_eq!(coordinator.diagnostics().unwrap().len(), 1);
    } else {
        let journal = Store::<AuthRecord>::open(local.join("authorizations")).unwrap();
        let buyer_auth = peer
            .receive_authorization(NOW, |auth| persist_auth(&journal, auth))
            .unwrap();
        let seller_auth = signed(&terms, commitment, Role::Seller, &key);
        peer.send_authorization(&seller_auth, NOW, |auth| persist_auth(&journal, auth))
            .unwrap();
        let evidence = SelectedAgreement::from_store(
            terms.clone(),
            blinding,
            buyer_auth,
            seller_auth,
            TRANSCRIPT_HASH_VERSION,
            &transcript_store,
            "agent",
        )
        .unwrap();
        let public: [u8; 32] = fs::read(root.join("auditor.public"))
            .unwrap()
            .try_into()
            .unwrap();
        DisclosureGrant::seal(&evidence, &key, public, NOW + 300, NOW)
            .unwrap()
            .write_backup(root.join("deal.grant"))
            .unwrap();
    }
    assert_eq!(
        peer.state().unwrap().agreement().unwrap().0.transcript_root,
        terms.transcript_root
    );
    private(&local.join("authorize.json"), json!({"root": hex::encode(terms.transcript_root), "session": hex::encode(session),
        "commitment": commitment.to_hex(), "reserved_total": reserved_total, "payment_verified": false}).to_string().as_bytes());
}

fn identity_seller_address() -> [u8; 20] {
    initial()
        .terms()
        .seller_authorization_key
        .as_bytes()
        .try_into()
        .unwrap()
}
