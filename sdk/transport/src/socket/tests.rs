use super::*;
use crate::{
    descriptor::MODE_PUBLIC_BOUND,
    hashing::TRANSCRIPT_HASH_VERSION,
    identity::AuthorizationIdentity,
    message::MessageType,
    store::{FileTranscriptStore, TranscriptStore},
};
use erebus_core::{
    ids::{AssetId, ChainNamespace},
    terms::GuaranteeSet,
};
use std::{net::TcpListener, thread};

const NOW: u64 = 1_700_000_000;

fn identity(seed: u8) -> (TransportIdentity, ServiceDescriptor) {
    let transport = TransportIdentity::from_private_key([seed; 32]).unwrap();
    let signer = AuthorizationIdentity::from_bytes(&[seed; 32]).unwrap();
    let namespace = ChainNamespace::parse("eip155:10143").unwrap();
    let asset = AssetId::new(namespace.clone(), "erc20", "0xabc").unwrap();
    let mut descriptor = ServiceDescriptor::new(
        signer.address(),
        transport.public_key(),
        vec!["tcp://127.0.0.1:0".into()],
        namespace,
        vec![asset],
        vec![1],
        MODE_PUBLIC_BOUND,
        GuaranteeSet::empty(),
        NOW - 1,
        NOW + 1000,
    )
    .unwrap();
    descriptor.sign(&signer).unwrap();
    (transport, descriptor)
}

fn sockets() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let buyer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let seller = listener.accept().unwrap().0;
    (buyer, seller)
}

fn channels(timeout: Duration) -> (SocketChannel, SocketChannel) {
    channels_with_mode(timeout, false)
}

fn channels_with_mode(timeout: Duration, nonblocking: bool) -> (SocketChannel, SocketChannel) {
    let (buyer, seller) = sockets();
    buyer.set_nonblocking(nonblocking).unwrap();
    seller.set_nonblocking(nonblocking).unwrap();
    let (buyer_key, buyer_descriptor) = identity(1);
    let (seller_key, seller_descriptor) = identity(2);
    let b = buyer_descriptor.clone();
    let s = seller_descriptor.clone();
    let worker = thread::spawn(move || {
        SocketChannel::seller(seller, &seller_key, &s, &b, NOW, timeout).unwrap()
    });
    let buyer = SocketChannel::buyer(
        buyer,
        &buyer_key,
        &buyer_descriptor,
        &seller_descriptor,
        NOW,
        timeout,
    )
    .unwrap();
    (buyer, worker.join().unwrap())
}

#[test]
fn inherited_nonblocking_streams_are_normalized_before_bounded_handshake_and_reads() {
    let timeout = Duration::from_millis(150);
    let (mut buyer, mut seller) = channels_with_mode(timeout, true);
    let offer = message(&buyer, 1, [0; 32], b"private offer");
    buyer.send(&offer).unwrap();
    assert_eq!(seller.receive().unwrap(), offer);
    let started = Instant::now();
    assert_eq!(seller.receive(), Err(SocketError::Timeout));
    assert!(started.elapsed() >= Duration::from_millis(120));
}

fn message(channel: &SocketChannel, sequence: u64, parent: [u8; 32], body: &[u8]) -> Message {
    Message::new(
        channel.session_id(),
        [42; 16],
        1,
        channel.local_role(),
        sequence,
        parent,
        if channel.local_role() == Role::Buyer {
            MessageType::Offer
        } else {
            MessageType::Counter
        },
        body.to_vec(),
    )
    .unwrap()
}

#[test]
fn independent_stores_exchange_authenticated_ciphertext_and_agree_on_a_root() {
    let (mut buyer, mut seller) = channels(Duration::from_secs(2));
    assert_eq!(buyer.session_id(), seller.session_id());
    let first_session = buyer.session_id();
    let b = tempfile::tempdir().unwrap();
    let s = tempfile::tempdir().unwrap();
    let buyer_store = FileTranscriptStore::open(b.path()).unwrap();
    let seller_store = FileTranscriptStore::open(s.path()).unwrap();
    let offer = message(&buyer, 1, [0; 32], b"private offer 60");
    buyer_store
        .append("agent", offer.deal_id, TRANSCRIPT_HASH_VERSION, &offer)
        .unwrap();
    buyer.send(&offer).unwrap();
    let delivered = seller.receive().unwrap();
    assert_eq!(offer, delivered);
    seller_store
        .append("agent", offer.deal_id, TRANSCRIPT_HASH_VERSION, &delivered)
        .unwrap();
    let counter = message(&seller, 1, [0; 32], b"private counter 70");
    seller_store
        .append("agent", counter.deal_id, TRANSCRIPT_HASH_VERSION, &counter)
        .unwrap();
    seller.send(&counter).unwrap();
    let delivered = buyer.receive().unwrap();
    let before = buyer_store
        .append(
            "agent",
            counter.deal_id,
            TRANSCRIPT_HASH_VERSION,
            &delivered,
        )
        .unwrap();
    assert_eq!(
        before.root().unwrap(),
        seller_store
            .load("agent", counter.deal_id, TRANSCRIPT_HASH_VERSION)
            .unwrap()
            .root()
            .unwrap()
    );
    assert_ne!(before.root().unwrap(), [0; 32]);
    drop(buyer);
    drop(seller);
    let (mut buyer, mut seller) = channels(Duration::from_secs(2));
    assert_ne!(buyer.session_id(), first_session);
    let next = message(
        &buyer,
        before.next_sequence(Role::Buyer),
        before.head(Role::Buyer),
        b"continued offer after re-handshake",
    );
    buyer_store
        .append("agent", next.deal_id, TRANSCRIPT_HASH_VERSION, &next)
        .unwrap();
    buyer.send(&next).unwrap();
    let delivered = seller.receive().unwrap();
    let remote = seller_store
        .append("agent", next.deal_id, TRANSCRIPT_HASH_VERSION, &delivered)
        .unwrap();
    assert_eq!(
        remote.root().unwrap(),
        buyer_store
            .load("agent", next.deal_id, TRANSCRIPT_HASH_VERSION)
            .unwrap()
            .root()
            .unwrap()
    );
}

#[test]
fn failed_or_partial_io_never_reuses_a_noise_connection() {
    let (mut buyer, mut seller) = channels(Duration::from_millis(80));
    assert_eq!(buyer.receive(), Err(SocketError::Timeout));
    assert_eq!(buyer.receive(), Err(SocketError::Closed));
    assert_eq!(
        buyer.send(&message(&buyer, 1, [0; 32], b"never send after timeout")),
        Err(SocketError::Closed)
    );
    assert!(seller.receive().is_err());
    assert_eq!(seller.receive(), Err(SocketError::Closed));
}

#[test]
fn malformed_frames_close_before_allocation_and_wrong_authentication_closes() {
    for length in [0, (MAX_MESSAGE_BYTES + 1) as u32] {
        let (mut buyer, mut seller) = channels(Duration::from_secs(2));
        seller.stream.write_all(&length.to_be_bytes()).unwrap();
        assert_eq!(buyer.receive(), Err(SocketError::Frame));
        assert_eq!(buyer.receive(), Err(SocketError::Closed));
    }
    let (mut buyer, mut seller) = channels(Duration::from_secs(2));
    write_frame(
        &mut seller.stream,
        &[0; 16],
        MAX_MESSAGE_BYTES,
        Instant::now() + Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(buyer.receive(), Err(SocketError::Authentication));
    assert_eq!(buyer.receive(), Err(SocketError::Closed));
}

#[test]
fn valid_but_different_descriptors_cannot_downgrade_the_prologue() {
    let (buyer, seller) = sockets();
    let (buyer_key, b) = identity(1);
    let (seller_key, s) = identity(2);
    let mut changed = s.clone();
    changed.endpoints = vec!["tcp://different.example:5555".into()];
    changed
        .sign(&AuthorizationIdentity::from_bytes(&[2; 32]).unwrap())
        .unwrap();
    let peer_b = b.clone();
    let worker = thread::spawn(move || {
        SocketChannel::seller(
            seller,
            &seller_key,
            &s,
            &peer_b,
            NOW,
            Duration::from_secs(1),
        )
    });
    assert!(matches!(
        SocketChannel::buyer(buyer, &buyer_key, &b, &changed, NOW, Duration::from_secs(1)),
        Err(SocketError::Authentication)
    ));
    assert!(worker.join().unwrap().is_err());
}

#[test]
fn invalid_identity_expiry_and_deadline_fail_before_handshake() {
    let (buyer_key, b) = identity(1);
    let (_, s) = identity(2);
    for (now, timeout) in [
        (NOW, Duration::ZERO),
        (NOW, Duration::from_secs(301)),
        (NOW + 1000, Duration::from_secs(1)),
    ] {
        let (buyer, _seller) = sockets();
        assert!(matches!(
            SocketChannel::buyer(buyer, &buyer_key, &b, &s, now, timeout),
            Err(SocketError::Configuration)
        ));
    }
    let (buyer, _seller) = sockets();
    assert!(matches!(
        SocketChannel::buyer(
            buyer,
            &TransportIdentity::from_private_key([7; 32]).unwrap(),
            &b,
            &s,
            NOW,
            Duration::from_secs(1)
        ),
        Err(SocketError::Configuration)
    ));
}

#[test]
fn incremental_progress_does_not_reset_the_frame_deadline() {
    let (mut buyer, mut seller) = sockets();
    let worker = thread::spawn(move || {
        for byte in [0, 0, 0, 3, 1, 2, 3] {
            if seller.write_all(&[byte]).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(40));
        }
    });
    let start = Instant::now();
    assert_eq!(
        read_frame(&mut buyer, 10, start + Duration::from_millis(100)),
        Err(SocketError::Timeout)
    );
    assert!(start.elapsed() < Duration::from_millis(240));
    drop(buyer);
    worker.join().unwrap();
}

#[test]
fn ciphertext_does_not_contain_offer_and_replay_closes_the_session() {
    let (mut buyer, mut seller) = channels(Duration::from_secs(2));
    let offer = message(&buyer, 1, [0; 32], b"CONFIDENTIAL_RESERVATION_PRICE");
    let ciphertext = buyer.session.send_message(&offer).unwrap();
    assert!(!ciphertext
        .windows(offer.body.len())
        .any(|window| window == offer.body));
    write_frame(
        &mut buyer.stream,
        &ciphertext,
        MAX_MESSAGE_BYTES,
        Instant::now() + Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(seller.receive().unwrap(), offer);
    write_frame(
        &mut buyer.stream,
        &ciphertext,
        MAX_MESSAGE_BYTES,
        Instant::now() + Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(seller.receive(), Err(SocketError::Authentication));
    assert_eq!(seller.receive(), Err(SocketError::Closed));
}
