//! End-to-end evidence for Metropolis M2.
//!
//! Two independent participants, each with its own identity, transport session, and durable
//! transcript store, negotiate over the ciphertext relay and agree on one transcript root. The
//! tests then exercise the failure modes the threat model names: replayed ciphertext, a message
//! from another session, and a restart that must re-handshake before continuing.

use erebus_core::auth::Role;
use erebus_core::ids::{AssetId, ChainNamespace};
use erebus_core::terms::{GuaranteeSet, SettlementMode};
use erebus_transport::descriptor::{self, Directory, DiscoveryFilter, ServiceDescriptor};
use erebus_transport::identity::{AuthorizationIdentity, TransportIdentity};
use erebus_transport::message::{Message, MessageType};
use erebus_transport::relay::{MailboxId, MemoryRelay, Relay};
use erebus_transport::session::{self, Session};
use erebus_transport::store::{FileTranscriptStore, TranscriptStore};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

const DEAL: [u8; 16] = [0x42; 16];
const SUITE: u16 = 1;
const NAMESPACE: &str = "testnet.metropolis";
const NOW: u64 = 1_700_000_000;

struct Participant {
    transport: TransportIdentity,
    store: FileTranscriptStore,
    descriptor: ServiceDescriptor,
    _directory: tempfile::TempDir,
}

fn descriptor_for(
    authorization: &AuthorizationIdentity,
    transport: &TransportIdentity,
    mode: SettlementMode,
) -> ServiceDescriptor {
    let namespace = ChainNamespace::new("eip155", "10143").expect("namespace");
    let asset = AssetId::new(namespace.clone(), "erc20", "0xabc").expect("asset");
    let guarantees = GuaranteeSet::empty();
    let mut descriptor = ServiceDescriptor::new(
        authorization.address(),
        transport.public_key(),
        vec!["https://peer.example/eleusis".to_owned()],
        namespace,
        vec![asset],
        vec![SUITE],
        match mode {
            SettlementMode::PublicBound => descriptor::MODE_PUBLIC_BOUND,
            SettlementMode::Shielded => descriptor::MODE_SHIELDED,
        },
        guarantees,
        NOW,
        NOW + 86_400,
    )
    .expect("descriptor");
    descriptor.sign(authorization).expect("sign");
    descriptor
}

fn participant(seed: u8) -> Participant {
    let directory = tempfile::tempdir().expect("temp dir");
    let store = FileTranscriptStore::open(directory.path()).expect("store");
    let transport = TransportIdentity::generate().expect("transport key");
    let mut key = [0u8; 32];
    key[0] = seed;
    let authorization = AuthorizationIdentity::from_bytes(&key).expect("authorization key");
    let descriptor = descriptor_for(&authorization, &transport, SettlementMode::PublicBound);
    Participant {
        transport,
        store,
        descriptor,
        _directory: directory,
    }
}

fn handshake(buyer: &Participant, seller: &Participant) -> (Session, Session) {
    let prologue = session::prologue(
        &buyer.descriptor.digest().expect("buyer digest"),
        &seller.descriptor.digest().expect("seller digest"),
    );
    let mut initiator = session::Handshake::initiator(
        &buyer.transport,
        Role::Buyer,
        Role::Seller,
        &prologue,
        seller.transport.public_key(),
    )
    .expect("initiator");
    let mut responder = session::Handshake::responder(
        &seller.transport,
        Role::Seller,
        Role::Buyer,
        &prologue,
        buyer.transport.public_key(),
    )
    .expect("responder");
    let first = initiator.write().expect("one");
    responder.read(&first).expect("read one");
    let second = responder.write().expect("two");
    initiator.read(&second).expect("read two");
    let third = initiator.write().expect("three");
    responder.read(&third).expect("read three");
    (
        initiator.finish().expect("buyer session"),
        responder.finish().expect("seller session"),
    )
}

fn message(
    session: &Session,
    sequence: u64,
    parent: [u8; 32],
    message_type: MessageType,
    body: &[u8],
) -> Message {
    Message::new(
        session.session_id(),
        DEAL,
        1,
        session.local_role(),
        sequence,
        parent,
        message_type,
        body.to_vec(),
    )
    .expect("message")
}

/// One participant sends one message: persist, then encrypt, then hand to the relay.
fn send(
    session: &mut Session,
    store: &FileTranscriptStore,
    relay: &dyn Relay,
    mailbox: &MailboxId,
    message: &Message,
    now: u64,
) {
    store
        .append(NAMESPACE, DEAL, SUITE, message)
        .expect("persist before send");
    let ciphertext = session.send_message(message).expect("encrypt");
    relay.put(mailbox, &ciphertext, now).expect("relay put");
}

/// One participant receives everything newer than `after`, persisting each before acknowledging.
/// Returns the messages and the new cursor.
fn receive(
    session: &mut Session,
    store: &FileTranscriptStore,
    relay: &dyn Relay,
    mailbox: &MailboxId,
    after: u64,
    now: u64,
) -> (Vec<Message>, u64) {
    let blobs = relay.get(mailbox, after, now).expect("relay get");
    let mut cursor = after;
    let mut messages = Vec::new();
    for blob in blobs {
        let message = session.receive_message(&blob.blob).expect("decrypt");
        store
            .append(NAMESPACE, DEAL, SUITE, &message)
            .expect("persist before ack");
        cursor = cursor.max(blob.cursor);
        messages.push(message);
    }
    (messages, cursor)
}

fn roots(buyer: &Participant, seller: &Participant) -> ([u8; 32], [u8; 32]) {
    (
        buyer
            .store
            .load(NAMESPACE, DEAL, SUITE)
            .expect("buyer transcript")
            .root()
            .expect("buyer root"),
        seller
            .store
            .load(NAMESPACE, DEAL, SUITE)
            .expect("seller transcript")
            .root()
            .expect("seller root"),
    )
}

#[test]
fn two_participants_agree_on_one_transcript() {
    let buyer = participant(0x11);
    let seller = participant(0x22);
    let (mut buyer_session, mut seller_session) = handshake(&buyer, &seller);
    assert_eq!(buyer_session.session_id(), seller_session.session_id());

    let relay = MemoryRelay::new();
    let to_seller = MailboxId::for_session(&buyer_session.session_id(), Role::Buyer);
    let to_buyer = MailboxId::for_session(&buyer_session.session_id(), Role::Seller);

    let offer = message(&buyer_session, 1, [0; 32], MessageType::Offer, b"100 units");
    send(
        &mut buyer_session,
        &buyer.store,
        &relay,
        &to_seller,
        &offer,
        NOW,
    );
    let (received, seller_cursor) = receive(
        &mut seller_session,
        &seller.store,
        &relay,
        &to_seller,
        0,
        NOW,
    );
    assert_eq!(received.len(), 1);

    let counter = message(
        &seller_session,
        1,
        [0; 32],
        MessageType::Counter,
        b"120 units",
    );
    send(
        &mut seller_session,
        &seller.store,
        &relay,
        &to_buyer,
        &counter,
        NOW,
    );
    let (received, buyer_cursor) =
        receive(&mut buyer_session, &buyer.store, &relay, &to_buyer, 0, NOW);
    assert_eq!(received.len(), 1);

    let accept = message(
        &buyer_session,
        2,
        buyer
            .store
            .load(NAMESPACE, DEAL, SUITE)
            .expect("buyer transcript")
            .head(Role::Buyer),
        MessageType::Authorization,
        b"accept",
    );
    send(
        &mut buyer_session,
        &buyer.store,
        &relay,
        &to_seller,
        &accept,
        NOW,
    );
    let (received, _) = receive(
        &mut seller_session,
        &seller.store,
        &relay,
        &to_seller,
        seller_cursor,
        NOW,
    );
    assert_eq!(received.len(), 1);
    assert!(buyer_cursor >= 1);

    let (buyer_root, seller_root) = roots(&buyer, &seller);
    assert_ne!(buyer_root, [0u8; 32]);
    assert_eq!(
        buyer_root, seller_root,
        "two independent participants must agree on one transcript root"
    );
}

#[test]
fn a_replayed_ciphertext_is_rejected() {
    let buyer = participant(0x31);
    let seller = participant(0x32);
    let (mut buyer_session, mut seller_session) = handshake(&buyer, &seller);

    let offer = message(&buyer_session, 1, [0; 32], MessageType::Offer, b"offer");
    let ciphertext = buyer_session.send_message(&offer).expect("encrypt");
    assert!(seller_session.receive_message(&ciphertext).is_ok());
    assert!(
        seller_session.receive_message(&ciphertext).is_err(),
        "a replayed ciphertext must not decrypt twice"
    );
}

#[test]
fn a_message_from_another_session_is_rejected() {
    let buyer = participant(0x41);
    let seller = participant(0x42);
    let third = participant(0x43);

    let (mut buyer_session, mut seller_session) = handshake(&buyer, &seller);
    let (mut third_session, _) = handshake(&third, &seller);

    let foreign = message(&third_session, 1, [0; 32], MessageType::Offer, b"foreign");
    let ciphertext = third_session.send_message(&foreign).expect("encrypt");
    assert!(
        seller_session.receive_message(&ciphertext).is_err(),
        "a ciphertext from another session must not authenticate"
    );

    let legitimate = message(
        &buyer_session,
        1,
        [0; 32],
        MessageType::Offer,
        b"legitimate",
    );
    let ciphertext = buyer_session.send_message(&legitimate).expect("encrypt");
    assert!(seller_session.receive_message(&ciphertext).is_ok());
}

#[test]
fn a_restart_rehandshakes_and_the_transcript_continues() {
    let buyer = participant(0x51);
    let seller = participant(0x52);
    let relay = MemoryRelay::new();
    let (mut buyer_session, mut seller_session) = handshake(&buyer, &seller);
    let to_seller = MailboxId::for_session(&buyer_session.session_id(), Role::Buyer);
    let first_session_id = buyer_session.session_id();

    let offer = message(&buyer_session, 1, [0; 32], MessageType::Offer, b"offer");
    send(
        &mut buyer_session,
        &buyer.store,
        &relay,
        &to_seller,
        &offer,
        NOW,
    );
    let (_, first_session_cursor) = receive(
        &mut seller_session,
        &seller.store,
        &relay,
        &to_seller,
        0,
        NOW,
    );
    assert!(first_session_cursor >= 1);

    // The buyer's process dies. The session is dropped and must never be resumed; a restart
    // performs a fresh handshake (a new session id) and continues the same deal transcript.
    drop(buyer_session);
    let (mut restarted_buyer, mut restarted_seller) = handshake(&buyer, &seller);
    let second_session_id = restarted_buyer.session_id();
    assert_ne!(
        first_session_id, second_session_id,
        "a restart must not reuse the dead session"
    );
    assert_eq!(second_session_id, restarted_seller.session_id());
    let restarted_to_seller = MailboxId::for_session(&second_session_id, Role::Buyer);
    let restarted_to_buyer = MailboxId::for_session(&second_session_id, Role::Seller);
    assert_ne!(to_seller, restarted_to_seller);

    // Reload the transcript from disk and continue the chain from the persisted head.
    let buyer_transcript = buyer
        .store
        .load(NAMESPACE, DEAL, SUITE)
        .expect("reload buyer transcript");
    let seller_transcript = seller
        .store
        .load(NAMESPACE, DEAL, SUITE)
        .expect("reload seller transcript");

    let counter = message(
        &restarted_seller,
        1,
        seller_transcript.head(Role::Seller),
        MessageType::Counter,
        b"counter",
    );
    send(
        &mut restarted_seller,
        &seller.store,
        &relay,
        &restarted_to_buyer,
        &counter,
        NOW,
    );
    let (_, buyer_cursor) = receive(
        &mut restarted_buyer,
        &buyer.store,
        &relay,
        &restarted_to_buyer,
        0,
        NOW,
    );

    let follow_up = message(
        &restarted_buyer,
        2,
        buyer_transcript.head(Role::Buyer),
        MessageType::Authorization,
        b"accept",
    );
    send(
        &mut restarted_buyer,
        &buyer.store,
        &relay,
        &restarted_to_seller,
        &follow_up,
        NOW,
    );
    receive(
        &mut restarted_seller,
        &seller.store,
        &relay,
        &restarted_to_seller,
        0,
        NOW,
    );
    assert!(buyer_cursor >= 1);

    let (buyer_root, seller_root) = roots(&buyer, &seller);
    assert_ne!(buyer_root, [0u8; 32]);
    assert_eq!(buyer_root, seller_root);
}

#[test]
fn a_mismatched_capability_prologue_fails_the_handshake() {
    let buyer = participant(0x61);
    let seller = participant(0x62);

    // A substituted descriptor advertises an additional capability.
    let mut forged = seller.descriptor.clone();
    let mut forged_guarantees = GuaranteeSet::empty();
    forged_guarantees.insert(erebus_core::terms::Guarantee::ScopedDisclosure);
    forged.guarantees = forged_guarantees;
    let prologue = session::prologue(
        &buyer.descriptor.digest().expect("buyer digest"),
        &seller.descriptor.digest().expect("seller digest"),
    );
    let forged_prologue = session::prologue(
        &buyer.descriptor.digest().expect("buyer digest"),
        &forged.digest().expect("forged digest"),
    );
    assert_ne!(prologue, forged_prologue);

    let mut initiator = session::Handshake::initiator(
        &buyer.transport,
        Role::Buyer,
        Role::Seller,
        &prologue,
        seller.transport.public_key(),
    )
    .expect("initiator");
    let mut responder = session::Handshake::responder(
        &seller.transport,
        Role::Seller,
        Role::Buyer,
        &forged_prologue,
        buyer.transport.public_key(),
    )
    .expect("responder");
    let first = initiator.write().expect("one");
    responder.read(&first).expect("read one");
    let second = responder.write().expect("two");
    assert!(
        initiator.read(&second).is_err(),
        "a capability-binding prologue mismatch must fail the handshake"
    );
}

#[test]
fn a_descriptor_for_a_different_deployment_does_not_support_the_filter() {
    let buyer = participant(0x71);
    let namespace = ChainNamespace::new("eip155", "1").expect("namespace");
    let asset = AssetId::new(namespace.clone(), "erc20", "0xabc").expect("asset");
    let filter = DiscoveryFilter {
        chain_namespace: namespace,
        asset,
        mode: SettlementMode::PublicBound,
        required_guarantees: GuaranteeSet::empty(),
    };
    assert!(
        !buyer
            .descriptor
            .supports(&filter, NOW)
            .expect("verified descriptor"),
        "a Monad deployment must not satisfy a mainnet filter"
    );
}

#[test]
fn independent_processes_rehandshake_reopen_state_and_agree_on_the_root() {
    let directory = tempfile::tempdir().expect("temp dir");
    let seller_descriptor = deterministic_descriptor(Role::Seller);
    let directory_json = Directory::new(vec![seller_descriptor], NOW)
        .expect("verified directory")
        .to_json()
        .expect("directory json");
    fs::write(directory.path().join("directory.json"), directory_json).expect("directory file");

    run_process_phase(directory.path(), 1);
    run_process_phase(directory.path(), 2);

    let buyer_root = fs::read_to_string(directory.path().join("buyer.root")).expect("buyer root");
    let seller_root =
        fs::read_to_string(directory.path().join("seller.root")).expect("seller root");
    assert_ne!(buyer_root, hex::encode([0u8; 32]));
    assert_eq!(buyer_root, seller_root);
}

#[test]
fn packaged_http_relay_enforces_auth_and_serves_ciphertext() {
    let directory = tempfile::tempdir().expect("temp dir");
    let probe = TcpListener::bind("127.0.0.1:0").expect("reserve port");
    let port = probe.local_addr().expect("address").port();
    drop(probe);
    let mut relay = Command::new(env!("CARGO_BIN_EXE_erebus-relay"))
        .env("EREBUS_RELAY_ROOT", directory.path())
        .env("EREBUS_RELAY_PORT", port.to_string())
        .env("EREBUS_RELAY_TOKEN", "test-token")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn relay");
    wait_for_port(port, &mut relay);

    let unauthorized = http_request(port, "GET", "/healthz", None, b"");
    let health = http_request(port, "GET", "/healthz", Some("Bearer test-token"), b"");
    let mailbox = MailboxId::for_session(&[0x44; 32], Role::Buyer).to_string();
    let put = http_request(
        port,
        "POST",
        &format!("/v1/mailbox/{mailbox}"),
        Some("Bearer test-token"),
        b"ciphertext",
    );
    let get = http_request(
        port,
        "GET",
        &format!("/v1/mailbox/{mailbox}?after=0"),
        Some("Bearer test-token"),
        b"",
    );
    relay.kill().expect("stop relay");
    relay.wait().expect("reap relay");

    assert!(unauthorized.starts_with("HTTP/1.1 401"));
    assert!(health.starts_with("HTTP/1.1 200"));
    assert!(health.contains("\"status\":\"ok\""));
    assert!(put.starts_with("HTTP/1.1 200"));
    assert!(get.starts_with("HTTP/1.1 200"));
    assert!(get.contains(&hex::encode(b"ciphertext")));
}

#[test]
#[ignore = "invoked by the process-level acceptance test"]
fn subprocess_peer_worker() {
    let Some(role) = std::env::var("EREBUS_M2_WORKER_ROLE").ok() else {
        return;
    };
    let root = PathBuf::from(std::env::var("EREBUS_M2_WORKER_ROOT").expect("worker root"));
    let phase: u8 = std::env::var("EREBUS_M2_WORKER_PHASE")
        .expect("worker phase")
        .parse()
        .expect("numeric phase");
    let address = std::env::var("EREBUS_M2_WORKER_ADDRESS").expect("worker address");
    match role.as_str() {
        "buyer" => run_buyer_worker(&root, phase, &address),
        "seller" => run_seller_worker(&root, phase, &address),
        other => panic!("unknown worker role {other}"),
    }
}

fn run_process_phase(root: &Path, phase: u8) {
    let probe = TcpListener::bind("127.0.0.1:0").expect("reserve port");
    let address = probe.local_addr().expect("local address").to_string();
    drop(probe);
    let ready = root.join(format!("phase-{phase}.ready"));

    let mut seller = spawn_worker("seller", root, phase, &address);
    wait_for_file(&ready, &mut seller);
    let buyer = spawn_worker("buyer", root, phase, &address)
        .wait_with_output()
        .expect("buyer process");
    if !buyer.status.success() {
        let _ = seller.kill();
        panic!(
            "buyer subprocess failed:\n{}\n{}",
            String::from_utf8_lossy(&buyer.stdout),
            String::from_utf8_lossy(&buyer.stderr)
        );
    }
    let seller = seller.wait_with_output().expect("seller process");
    assert!(
        seller.status.success(),
        "seller subprocess failed:\n{}\n{}",
        String::from_utf8_lossy(&seller.stdout),
        String::from_utf8_lossy(&seller.stderr)
    );
}

fn spawn_worker(role: &str, root: &Path, phase: u8, address: &str) -> Child {
    Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--ignored",
            "--exact",
            "subprocess_peer_worker",
            "--nocapture",
        ])
        .env("EREBUS_M2_WORKER_ROLE", role)
        .env("EREBUS_M2_WORKER_ROOT", root)
        .env("EREBUS_M2_WORKER_PHASE", phase.to_string())
        .env("EREBUS_M2_WORKER_ADDRESS", address)
        .output_spawn()
}

trait OutputSpawn {
    fn output_spawn(&mut self) -> Child;
}

impl OutputSpawn for Command {
    fn output_spawn(&mut self) -> Child {
        self.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn worker")
    }
}

fn wait_for_file(path: &Path, child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if path.exists() {
            return;
        }
        if let Some(status) = child.try_wait().expect("poll seller") {
            panic!("seller exited before becoming ready: {status}");
        }
        thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    panic!("seller did not become ready");
}

fn wait_for_port(port: u16, child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        if let Some(status) = child.try_wait().expect("poll relay") {
            panic!("relay exited before becoming ready: {status}");
        }
        thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    panic!("relay did not become ready");
}

fn http_request(
    port: u16,
    method: &str,
    path: &str,
    authorization: Option<&str>,
    body: &[u8],
) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect relay");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    let authorization = authorization
        .map(|value| format!("Authorization: {value}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{authorization}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(request.as_bytes()).expect("request");
    stream.write_all(body).expect("request body");
    stream.flush().expect("request flush");
    let mut response = Vec::new();
    stream.read_to_end(&mut response).expect("response");
    String::from_utf8(response).expect("HTTP response")
}

fn deterministic_transport(role: Role) -> TransportIdentity {
    let byte = match role {
        Role::Buyer => 0x81,
        Role::Seller => 0x82,
    };
    TransportIdentity::from_private_key([byte; 32]).expect("transport identity")
}

fn deterministic_descriptor(role: Role) -> ServiceDescriptor {
    let authorization_byte = match role {
        Role::Buyer => 0x71,
        Role::Seller => 0x72,
    };
    let authorization =
        AuthorizationIdentity::from_bytes(&[authorization_byte; 32]).expect("authorization");
    descriptor_for(
        &authorization,
        &deterministic_transport(role),
        SettlementMode::PublicBound,
    )
}

fn worker_handshake(stream: &mut TcpStream, role: Role, identity: &TransportIdentity) -> Session {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("write timeout");
    let buyer_descriptor = deterministic_descriptor(Role::Buyer);
    let seller_descriptor = deterministic_descriptor(Role::Seller);
    let prologue = session::prologue(
        &buyer_descriptor.digest().expect("buyer digest"),
        &seller_descriptor.digest().expect("seller digest"),
    );
    match role {
        Role::Buyer => {
            let mut handshake = session::Handshake::initiator(
                identity,
                Role::Buyer,
                Role::Seller,
                &prologue,
                deterministic_transport(Role::Seller).public_key(),
            )
            .expect("buyer handshake");
            write_frame(stream, &handshake.write().expect("handshake one"));
            handshake.read(&read_frame(stream)).expect("handshake two");
            write_frame(stream, &handshake.write().expect("handshake three"));
            handshake.finish().expect("buyer session")
        }
        Role::Seller => {
            let mut handshake = session::Handshake::responder(
                identity,
                Role::Seller,
                Role::Buyer,
                &prologue,
                deterministic_transport(Role::Buyer).public_key(),
            )
            .expect("seller handshake");
            handshake.read(&read_frame(stream)).expect("handshake one");
            write_frame(stream, &handshake.write().expect("handshake two"));
            handshake
                .read(&read_frame(stream))
                .expect("handshake three");
            handshake.finish().expect("seller session")
        }
    }
}

fn run_buyer_worker(root: &Path, phase: u8, address: &str) {
    let directory_json = fs::read_to_string(root.join("directory.json")).expect("directory json");
    let directory = erebus_transport::descriptor::Directory::from_json(&directory_json, NOW)
        .expect("verified directory");
    let namespace = ChainNamespace::new("eip155", "10143").expect("namespace");
    let filter = DiscoveryFilter {
        chain_namespace: namespace.clone(),
        asset: AssetId::new(namespace, "erc20", "0xabc").expect("asset"),
        mode: SettlementMode::PublicBound,
        required_guarantees: GuaranteeSet::empty(),
    };
    assert_eq!(
        directory
            .compatible(&filter, NOW)
            .expect("compatible")
            .len(),
        1
    );

    let mut stream = TcpStream::connect(address).expect("connect seller");
    let identity = worker_transport(root, Role::Buyer, phase);
    let mut session = worker_handshake(&mut stream, Role::Buyer, &identity);
    let store = FileTranscriptStore::open(root.join("buyer-store")).expect("buyer store");
    match phase {
        1 => {
            let offer = message(&session, 1, [0; 32], MessageType::Offer, b"offer");
            store
                .append(NAMESPACE, DEAL, SUITE, &offer)
                .expect("persist offer");
            write_frame(
                &mut stream,
                &session.send_message(&offer).expect("send offer"),
            );
        }
        2 => {
            let counter = session
                .receive_message(&read_frame(&mut stream))
                .expect("receive counter");
            let transcript = store
                .append(NAMESPACE, DEAL, SUITE, &counter)
                .expect("persist counter");
            let authorization = message(
                &session,
                2,
                transcript.head(Role::Buyer),
                MessageType::Authorization,
                b"accept",
            );
            let transcript = store
                .append(NAMESPACE, DEAL, SUITE, &authorization)
                .expect("persist authorization");
            write_frame(
                &mut stream,
                &session
                    .send_message(&authorization)
                    .expect("send authorization"),
            );
            fs::write(
                root.join("buyer.root"),
                hex::encode(transcript.root().expect("root")),
            )
            .expect("write root");
        }
        _ => panic!("unknown phase"),
    }
}

fn run_seller_worker(root: &Path, phase: u8, address: &str) {
    let listener = TcpListener::bind(address).expect("bind seller");
    fs::write(root.join(format!("phase-{phase}.ready")), b"ready").expect("ready marker");
    let (mut stream, _) = listener.accept().expect("accept buyer");
    let identity = worker_transport(root, Role::Seller, phase);
    let mut session = worker_handshake(&mut stream, Role::Seller, &identity);
    let store = FileTranscriptStore::open(root.join("seller-store")).expect("seller store");
    match phase {
        1 => {
            let offer = session
                .receive_message(&read_frame(&mut stream))
                .expect("receive offer");
            store
                .append(NAMESPACE, DEAL, SUITE, &offer)
                .expect("persist offer");
        }
        2 => {
            let transcript = store
                .load(NAMESPACE, DEAL, SUITE)
                .expect("reopen transcript");
            let counter = message(
                &session,
                1,
                transcript.head(Role::Seller),
                MessageType::Counter,
                b"counter",
            );
            store
                .append(NAMESPACE, DEAL, SUITE, &counter)
                .expect("persist counter");
            write_frame(
                &mut stream,
                &session.send_message(&counter).expect("send counter"),
            );
            let authorization = session
                .receive_message(&read_frame(&mut stream))
                .expect("receive authorization");
            let transcript = store
                .append(NAMESPACE, DEAL, SUITE, &authorization)
                .expect("persist authorization");
            fs::write(
                root.join("seller.root"),
                hex::encode(transcript.root().expect("root")),
            )
            .expect("write root");
        }
        _ => panic!("unknown phase"),
    }
}

fn write_frame(stream: &mut TcpStream, bytes: &[u8]) {
    let length = u32::try_from(bytes.len()).expect("frame length");
    stream
        .write_all(&length.to_be_bytes())
        .expect("write length");
    stream.write_all(bytes).expect("write frame");
    stream.flush().expect("flush frame");
}

fn read_frame(stream: &mut TcpStream) -> Vec<u8> {
    let mut length = [0u8; 4];
    stream.read_exact(&mut length).expect("read length");
    let length = u32::from_be_bytes(length) as usize;
    assert!(length <= 65_536, "frame exceeds transport bound");
    let mut bytes = vec![0u8; length];
    stream.read_exact(&mut bytes).expect("read frame");
    bytes
}

fn worker_transport(root: &Path, role: Role, phase: u8) -> TransportIdentity {
    let name = match role {
        Role::Buyer => "buyer.transport.key",
        Role::Seller => "seller.transport.key",
    };
    let path = root.join(name);
    match phase {
        1 => {
            let identity = deterministic_transport(role);
            identity.store(&path).expect("persist transport identity");
            identity
        }
        2 => TransportIdentity::load(&path).expect("reload transport identity"),
        _ => panic!("unknown phase"),
    }
}
