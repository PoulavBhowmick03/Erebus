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

/// A descriptor of `seed`'s identity valid until `expires`, so two of them differ in digest only.
fn identity_until(seed: u8, expires: u64) -> (TransportIdentity, ServiceDescriptor) {
    let (transport, mut descriptor) = identity(seed);
    let signer = AuthorizationIdentity::from_bytes(&[seed; 32]).unwrap();
    descriptor.expires = expires;
    descriptor.sign(&signer).unwrap();
    (transport, descriptor)
}

/// The buyer's and the routed seller's handshake results.
type RoutedPair = (
    Result<SocketChannel, SocketError>,
    Result<(SocketChannel, ServiceDescriptor, &'static str), SocketError>,
);

/// Runs `seller_routed` on one end of a socket pair with peers `table` and returns its result
/// next to the buyer's routed or direct result.
fn routed_pair(
    routed_buyer: bool,
    buyer_seed: u8,
    buyer_descriptor: ServiceDescriptor,
    legacy: Option<ServiceDescriptor>,
    table: Vec<(ServiceDescriptor, &'static str)>,
    timeout: Duration,
) -> RoutedPair {
    let (buyer, seller) = sockets();
    let (buyer_key, _) = identity(buyer_seed);
    let (seller_key, seller_descriptor) = identity(2);
    let s = seller_descriptor.clone();
    let worker = thread::spawn(move || {
        SocketChannel::seller_routed(
            seller,
            &seller_key,
            &s,
            NOW,
            timeout,
            timeout,
            legacy.map(|descriptor| (descriptor, "legacy")),
            |key| {
                table
                    .iter()
                    .find(|(descriptor, _)| descriptor.transport_key == *key)
                    .cloned()
            },
        )
    });
    let buyer = if routed_buyer {
        SocketChannel::buyer_routed(
            buyer,
            &buyer_key,
            &buyer_descriptor,
            &seller_descriptor,
            NOW,
            timeout,
        )
    } else {
        SocketChannel::buyer(
            buyer,
            &buyer_key,
            &buyer_descriptor,
            &seller_descriptor,
            NOW,
            timeout,
        )
    };
    (buyer, worker.join().unwrap())
}

#[test]
fn a_routed_seller_selects_each_configured_buyer_and_reports_both_digests() {
    let (_, first) = identity(1);
    let (_, second) = identity(3);
    let (_, seller) = identity(2);
    for (seed, descriptor, expected) in [(1, &first, "first"), (3, &second, "second")] {
        let table = vec![(first.clone(), "first"), (second.clone(), "second")];
        let (buyer, selected) = routed_pair(
            true,
            seed,
            descriptor.clone(),
            None,
            table,
            Duration::from_secs(2),
        );
        let mut buyer = buyer.unwrap();
        let (mut channel, authenticated, tag) = selected.unwrap();
        assert_eq!(tag, expected);
        assert_eq!(&authenticated, descriptor);
        let digests = [descriptor.digest().unwrap(), seller.digest().unwrap()];
        assert_eq!(channel.descriptor_digests(), digests);
        assert_eq!(buyer.descriptor_digests(), digests);
        assert_eq!(buyer.session_id(), channel.session_id());
        let offer = message(&buyer, 1, [0; 32], b"routed offer");
        buyer.send(&offer).unwrap();
        assert_eq!(channel.receive().unwrap(), offer);
    }
}

#[test]
fn a_routed_seller_rejects_an_unknown_key_and_a_stale_descriptor() {
    let (_, configured) = identity(1);
    let (_, stranger) = identity(4);
    let (buyer, selected) = routed_pair(
        true,
        4,
        stranger,
        None,
        vec![(configured.clone(), "configured")],
        Duration::from_secs(2),
    );
    assert_eq!(selected.err(), Some(SocketError::Authentication));
    // The buyer finished its side; its first use finds the connection closed.
    if let Ok(mut buyer) = buyer {
        assert!(buyer.receive().is_err());
    }
    // Same key, but the seller holds a different signed descriptor than the one the buyer
    // presents: the digest sent in message 3 does not match the configured descriptor.
    let (_, renewed) = identity_until(1, NOW + 2000);
    assert_ne!(renewed.digest().unwrap(), configured.digest().unwrap());
    let (_, selected) = routed_pair(
        true,
        1,
        renewed,
        None,
        vec![(configured, "configured")],
        Duration::from_secs(2),
    );
    assert_eq!(selected.err(), Some(SocketError::Authentication));
}

#[test]
fn a_configured_descriptor_must_advertise_the_authenticated_key() {
    // A table that maps the buyer's key to someone else's descriptor is refused.
    let (_, buyer) = identity(1);
    let (_, other) = identity(5);
    let (buyer_key, _) = identity(1);
    let (seller_stream, seller_key, seller) = {
        let (b, s) = sockets();
        let (key, descriptor) = identity(2);
        let worker_descriptor = descriptor.clone();
        let other = other.clone();
        let worker = thread::spawn(move || {
            SocketChannel::seller_routed(
                s,
                &key,
                &worker_descriptor,
                NOW,
                Duration::from_secs(2),
                Duration::from_secs(2),
                None,
                |_| Some((other, "wrong")),
            )
            .map(|_| ())
        });
        (b, worker, descriptor)
    };
    let _ = SocketChannel::buyer_routed(
        seller_stream,
        &buyer_key,
        &buyer,
        &seller,
        NOW,
        Duration::from_secs(2),
    );
    assert_eq!(seller_key.join().unwrap(), Err(SocketError::Authentication));
}

#[test]
fn a_direct_buyer_is_accepted_only_as_the_legacy_peer() {
    let (_, legacy) = identity(1);
    let (_, seller) = identity(2);
    let (buyer, selected) = routed_pair(
        false,
        1,
        legacy.clone(),
        Some(legacy.clone()),
        Vec::new(),
        Duration::from_secs(2),
    );
    let mut buyer = buyer.unwrap();
    let (mut channel, authenticated, tag) = selected.unwrap();
    assert_eq!((tag, &authenticated), ("legacy", &legacy));
    assert_eq!(
        channel.descriptor_digests(),
        [legacy.digest().unwrap(), seller.digest().unwrap()]
    );
    let offer = message(&buyer, 1, [0; 32], b"legacy offer");
    buyer.send(&offer).unwrap();
    assert_eq!(channel.receive().unwrap(), offer);

    // Without a legacy peer, a direct first message is refused before any reply.
    let (buyer, selected) = routed_pair(
        false,
        1,
        legacy.clone(),
        None,
        vec![(legacy.clone(), "routed")],
        Duration::from_secs(2),
    );
    assert_eq!(selected.err(), Some(SocketError::Authentication));
    assert!(buyer.is_err());

    // A direct buyer that is not the legacy peer fails the version 1 key check.
    let (_, other) = identity(6);
    let (buyer, selected) = routed_pair(
        false,
        6,
        other,
        Some(legacy),
        Vec::new(),
        Duration::from_secs(2),
    );
    assert!(selected.is_err());
    assert!(buyer.is_err());
}

#[test]
fn a_routed_buyer_and_a_direct_seller_never_connect() {
    let (buyer, seller) = sockets();
    let (buyer_key, buyer_descriptor) = identity(1);
    let (seller_key, seller_descriptor) = identity(2);
    let (b, s) = (buyer_descriptor.clone(), seller_descriptor.clone());
    let worker = thread::spawn(move || {
        SocketChannel::seller(seller, &seller_key, &s, &b, NOW, Duration::from_secs(2)).map(|_| ())
    });
    let routed = SocketChannel::buyer_routed(
        buyer,
        &buyer_key,
        &buyer_descriptor,
        &seller_descriptor,
        NOW,
        Duration::from_secs(2),
    );
    assert!(routed.is_err());
    let _ = worker.join().unwrap();
}

/// Forwards one connection to `target`, applying `rewrite` to the first frame only.
fn tampering_proxy(
    listener: TcpListener,
    target: std::net::SocketAddr,
    rewrite: fn(Vec<u8>) -> Vec<u8>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let (mut client, _) = listener.accept().unwrap();
        let mut server = TcpStream::connect(target).unwrap();
        let mut prefix = [0u8; 4];
        client.read_exact(&mut prefix).unwrap();
        let mut frame = vec![0u8; u32::from_be_bytes(prefix) as usize];
        client.read_exact(&mut frame).unwrap();
        let frame = rewrite(frame);
        server
            .write_all(&(frame.len() as u32).to_be_bytes())
            .unwrap();
        server.write_all(&frame).unwrap();
        let (mut client_reader, mut server_writer) =
            (client.try_clone().unwrap(), server.try_clone().unwrap());
        let upstream = thread::spawn(move || {
            let _ = std::io::copy(&mut client_reader, &mut server_writer);
            let _ = server_writer.shutdown(Shutdown::Write);
        });
        let _ = std::io::copy(&mut server, &mut client);
        let _ = client.shutdown(Shutdown::Write);
        let _ = upstream.join();
    })
}

#[test]
fn stripping_the_routed_announcement_in_flight_fails_both_sides() {
    let (_, buyer_descriptor) = identity(1);
    let (seller_key, seller_descriptor) = identity(2);
    let (buyer_key, _) = identity(1);
    let seller_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let seller_address = seller_listener.local_addr().unwrap();
    let proxy_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_address = proxy_listener.local_addr().unwrap();
    // The proxy turns the routed first message into a direct one.
    let proxy = tampering_proxy(proxy_listener, seller_address, |mut frame| {
        frame.truncate(32);
        frame
    });
    let (legacy, s) = (buyer_descriptor.clone(), seller_descriptor.clone());
    let seller = thread::spawn(move || {
        let stream = seller_listener.accept().unwrap().0;
        // Even with this very buyer configured as the legacy peer, the downgrade fails.
        SocketChannel::seller_routed(
            stream,
            &seller_key,
            &s,
            NOW,
            Duration::from_secs(2),
            Duration::from_secs(2),
            Some((legacy, "legacy")),
            |_| None,
        )
        .map(|_| ())
    });
    let buyer = SocketChannel::buyer_routed(
        TcpStream::connect(proxy_address).unwrap(),
        &buyer_key,
        &buyer_descriptor,
        &seller_descriptor,
        NOW,
        Duration::from_secs(2),
    );
    assert_eq!(buyer.err(), Some(SocketError::Authentication));
    assert!(seller.join().unwrap().is_err());
    proxy.join().unwrap();
}

#[test]
fn a_silent_connection_times_out_within_the_handshake_budget() {
    let (_, buyer_descriptor) = identity(1);
    let (_silent, seller_stream) = sockets();
    let (seller_key, seller_descriptor) = identity(2);
    let started = Instant::now();
    let result = SocketChannel::seller_routed(
        seller_stream,
        &seller_key,
        &seller_descriptor,
        NOW,
        Duration::from_secs(30),
        Duration::from_millis(200),
        None,
        |_| Some((buyer_descriptor.clone(), "never")),
    );
    assert_eq!(result.err(), Some(SocketError::Timeout));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn a_stalled_routed_buyer_times_out_after_message_one() {
    use std::io::Write as _;
    let (mut buyer, seller_stream) = sockets();
    let (buyer_key, buyer_descriptor) = identity(1);
    let (seller_key, seller_descriptor) = identity(2);
    let mut handshake = Handshake::routed_initiator(
        &buyer_key,
        Role::Buyer,
        Role::Seller,
        &seller_descriptor.digest().unwrap(),
        seller_descriptor.transport_key,
        buyer_descriptor.digest().unwrap(),
    )
    .unwrap();
    let first = handshake.write().unwrap();
    buyer
        .write_all(&(first.len() as u32).to_be_bytes())
        .unwrap();
    buyer.write_all(&first).unwrap();
    let started = Instant::now();
    let result = SocketChannel::seller_routed(
        seller_stream,
        &seller_key,
        &seller_descriptor,
        NOW,
        Duration::from_secs(30),
        Duration::from_millis(200),
        None,
        |_| Some((buyer_descriptor.clone(), "stalled")),
    );
    assert_eq!(result.err(), Some(SocketError::Timeout));
    assert!(started.elapsed() < Duration::from_secs(2));
}
