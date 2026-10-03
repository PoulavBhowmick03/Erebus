use super::*;
use crate::{
    descriptor::{MODE_PUBLIC_BOUND, MODE_SHIELDED},
    identity::{AuthorizationIdentity, TransportIdentity},
    negotiation::tests::{proposal, signed},
    store::FileTranscriptStore,
};
use erebus_core::{ids::KeyBytes, terms::Guarantee};
use std::{
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};

const NOW: u64 = 1_700_000_000;

pub(in crate::negotiation) fn connections(
    suite: u16,
) -> (
    SocketChannel,
    SocketChannel,
    ServiceDescriptor,
    ServiceDescriptor,
) {
    let initial = proposal(suite);
    let descriptor = |role: Role, key: &TransportIdentity| {
        let signer = AuthorizationIdentity::from_bytes(&[role.tag(); 32]).unwrap();
        let mut descriptor = ServiceDescriptor::new(
            signer.address(),
            key.public_key(),
            vec!["tcp://127.0.0.1".into()],
            initial.terms().domain.namespace.clone(),
            vec![initial.terms().asset.clone()],
            vec![suite],
            if suite == 1 {
                MODE_PUBLIC_BOUND
            } else {
                MODE_SHIELDED
            },
            initial.terms().required_guarantees,
            NOW - 1,
            NOW + 1000,
        )
        .unwrap();
        descriptor.sign(&signer).unwrap();
        descriptor
    };
    let buyer_key = TransportIdentity::from_private_key([21; 32]).unwrap();
    let seller_key = TransportIdentity::from_private_key([22; 32]).unwrap();
    let buyer = descriptor(Role::Buyer, &buyer_key);
    let seller = descriptor(Role::Seller, &seller_key);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let b = buyer.clone();
    let s = seller.clone();
    let worker = thread::spawn(move || {
        SocketChannel::seller(
            listener.accept().unwrap().0,
            &seller_key,
            &s,
            &b,
            NOW,
            Duration::from_secs(2),
        )
        .unwrap()
    });
    let buyer_channel = SocketChannel::buyer(
        stream,
        &buyer_key,
        &buyer,
        &seller,
        NOW,
        Duration::from_secs(2),
    )
    .unwrap();
    (buyer_channel, worker.join().unwrap(), buyer, seller)
}

fn peers() -> (
    NegotiationPeer,
    NegotiationPeer,
    tempfile::TempDir,
    tempfile::TempDir,
) {
    let (buyer, seller, buyer_descriptor, seller_descriptor) = connections(1);
    let b = tempfile::tempdir().unwrap();
    let s = tempfile::tempdir().unwrap();
    let store = |path| {
        NegotiationStore::new(
            FileTranscriptStore::open(path).unwrap(),
            "agent".into(),
            proposal(1),
        )
        .unwrap()
    };
    let buyer = NegotiationPeer::public_bound(
        buyer,
        store(b.path()),
        &buyer_descriptor,
        &seller_descriptor,
        NOW,
    )
    .unwrap();
    let seller = NegotiationPeer::public_bound(
        seller,
        store(s.path()),
        &buyer_descriptor,
        &seller_descriptor,
        NOW,
    )
    .unwrap();
    (buyer, seller, b, s)
}

fn freeze(buyer: &mut NegotiationPeer, seller: &mut NegotiationPeer) {
    buyer
        .propose(buyer.store.initial.terms().amount, NOW)
        .unwrap();
    seller.receive(NOW).unwrap();
    seller.propose(BaseUnits::new(70), NOW).unwrap();
    buyer.receive(NOW).unwrap();
    buyer.accept(NOW).unwrap();
    seller.receive(NOW).unwrap();
    seller.accept(NOW).unwrap();
    buyer.receive(NOW).unwrap();
    assert_eq!(
        buyer.state().unwrap().agreement().unwrap(),
        seller.state().unwrap().agreement().unwrap()
    );
}

#[test]
fn synchronization_rejects_unbounded_counts_and_closes_the_connection() {
    let (mut buyer, mut seller, _b, _s) = peers();
    buyer
        .propose(buyer.store.initial.terms().amount, NOW)
        .unwrap();
    seller.receive(NOW).unwrap();
    let mut writer = erebus_core::encoding::Writer::new();
    writer.u64(MAX_MESSAGES_PER_DEAL as u64 + 1);
    seller.send_control(6, &writer.finish()).unwrap();
    assert!(matches!(buyer.synchronize(NOW), Err(PeerError::Binding)));
    assert!(matches!(
        buyer.send_control(6, &[0; 8]),
        Err(PeerError::Socket(SocketError::Closed))
    ));
}

#[test]
fn synchronization_rejects_a_mismatched_root_without_losing_the_retained_offer() {
    let (mut buyer, mut seller, _b, _s) = peers();
    buyer
        .propose(buyer.store.initial.terms().amount, NOW)
        .unwrap();
    seller.receive(NOW).unwrap();
    let retained = buyer.state().unwrap().transcript().root().unwrap();
    let worker = thread::spawn(move || {
        seller.receive_control(6).unwrap();
        seller.send_control(6, &[0; 8]).unwrap();
        seller.receive(NOW).unwrap();
        seller.receive_control(7).unwrap();
        seller.send_control(7, &[8; 32]).unwrap();
    });
    assert!(matches!(buyer.synchronize(NOW), Err(PeerError::Binding)));
    worker.join().unwrap();
    assert_eq!(
        buyer.state().unwrap().transcript().root().unwrap(),
        retained
    );
    assert!(matches!(
        buyer.send_control(6, &[0; 8]),
        Err(PeerError::Socket(SocketError::Closed))
    ));
}

#[test]
fn descriptor_identity_capability_and_handshake_substitution_fail_closed() {
    for case in 0..4 {
        let (buyer, _seller, b, mut s) = connections(1);
        let directory = tempfile::tempdir().unwrap();
        let mut initial = proposal(1);
        match case {
            0 => initial.terms.buyer_authorization_key = KeyBytes::new(vec![9; 20]).unwrap(),
            1 => initial
                .terms
                .required_guarantees
                .insert(Guarantee::ScopedDisclosure),
            2 => {
                s.endpoints[0] = "tcp://another.example".into();
                s.sign(&AuthorizationIdentity::from_bytes(&[2; 32]).unwrap())
                    .unwrap();
            }
            3 => initial = proposal(2),
            _ => unreachable!(),
        }
        let store = NegotiationStore::new(
            FileTranscriptStore::open(directory.path()).unwrap(),
            "agent".into(),
            initial,
        )
        .unwrap();
        assert!(matches!(
            NegotiationPeer::public_bound(buyer, store, &b, &s, NOW),
            Err(PeerError::Binding)
        ));
    }
}

#[test]
fn shielded_identity_bindings_stay_outside_the_frozen_negotiation_and_payment_signatures_verify() {
    let (buyer_channel, seller_channel, buyer_descriptor, seller_descriptor) = connections(2);
    let b = tempfile::tempdir().unwrap();
    let s = tempfile::tempdir().unwrap();
    let initial = proposal(2);
    let buyer_store = NegotiationStore::new(
        FileTranscriptStore::open(b.path()).unwrap(),
        "agent".into(),
        initial.clone(),
    )
    .unwrap();
    let seller_store = NegotiationStore::new(
        FileTranscriptStore::open(s.path()).unwrap(),
        "agent".into(),
        initial.clone(),
    )
    .unwrap();
    let binding = |descriptor: &ServiceDescriptor, role: Role| {
        AgreementKeyBinding::sign(
            descriptor,
            initial.terms().domain.clone(),
            if role == Role::Buyer {
                initial.terms().buyer_authorization_key.clone()
            } else {
                initial.terms().seller_authorization_key.clone()
            },
            &AuthorizationIdentity::from_bytes(&[role.tag(); 32]).unwrap(),
            NOW,
            NOW + 300,
        )
        .unwrap()
    };
    let buyer_binding = binding(&buyer_descriptor, Role::Buyer);
    let seller_binding = binding(&seller_descriptor, Role::Seller);
    let buyer_descriptor_copy = buyer_descriptor.clone();
    let seller_descriptor_copy = seller_descriptor.clone();
    let worker = thread::spawn(move || {
        NegotiationPeer::shielded(
            seller_channel,
            seller_store,
            &buyer_descriptor_copy,
            &seller_descriptor_copy,
            &seller_binding,
            NOW,
        )
        .unwrap()
    });
    let mut buyer = NegotiationPeer::shielded(
        buyer_channel,
        buyer_store,
        &buyer_descriptor,
        &seller_descriptor,
        &buyer_binding,
        NOW,
    )
    .unwrap();
    let mut seller = worker.join().unwrap();
    assert!(buyer.state().unwrap().transcript().is_empty());
    assert!(seller.state().unwrap().transcript().is_empty());
    freeze(&mut buyer, &mut seller);
    let state = buyer.state().unwrap();
    let buyer_auth = signed(&state, Role::Buyer);
    let seller_auth = signed(&state, Role::Seller);
    buyer
        .send_authorization(&buyer_auth, NOW, |_| Ok::<_, ()>(()))
        .unwrap();
    assert_eq!(
        seller
            .receive_authorization(NOW, |_| Ok::<_, ()>(()))
            .unwrap(),
        buyer_auth
    );
    seller
        .send_authorization(&seller_auth, NOW, |_| Ok::<_, ()>(()))
        .unwrap();
    assert_eq!(
        buyer
            .receive_authorization(NOW, |_| Ok::<_, ()>(()))
            .unwrap(),
        seller_auth
    );
    assert_eq!(buyer.state().unwrap().transcript().total(), 4);
    assert_eq!(seller.state().unwrap().transcript().total(), 4);
    assert_eq!(
        buyer.state().unwrap().agreement().unwrap(),
        state.agreement().unwrap()
    );
}

#[test]
fn a_descriptor_signed_substituted_shielded_key_is_rejected_before_negotiation() {
    let (mut buyer_channel, seller_channel, buyer_descriptor, seller_descriptor) = connections(2);
    let s = tempfile::tempdir().unwrap();
    let initial = proposal(2);
    let store = NegotiationStore::new(
        FileTranscriptStore::open(s.path()).unwrap(),
        "agent".into(),
        initial.clone(),
    )
    .unwrap();
    let substituted = AgreementKeyBinding::sign(
        &buyer_descriptor,
        initial.terms().domain.clone(),
        initial.terms().seller_authorization_key.clone(),
        &AuthorizationIdentity::from_bytes(&[1; 32]).unwrap(),
        NOW,
        NOW + 300,
    )
    .unwrap();
    let seller_binding = AgreementKeyBinding::sign(
        &seller_descriptor,
        initial.terms().domain.clone(),
        initial.terms().seller_authorization_key.clone(),
        &AuthorizationIdentity::from_bytes(&[2; 32]).unwrap(),
        NOW,
        NOW + 300,
    )
    .unwrap();
    let worker = thread::spawn(move || {
        NegotiationPeer::shielded(
            seller_channel,
            store,
            &buyer_descriptor,
            &seller_descriptor,
            &seller_binding,
            NOW,
        )
    });
    let mut body = erebus_core::encoding::Writer::new();
    body.u16(1);
    body.u8(4);
    body.bytes(&substituted.encode().unwrap());
    let message = Message::new(
        buyer_channel.session_id(),
        initial.terms().deal_id,
        1,
        Role::Buyer,
        1,
        [0; 32],
        MessageType::Authorization,
        body.finish(),
    )
    .unwrap();
    buyer_channel.send(&message).unwrap();
    assert!(matches!(worker.join().unwrap(), Err(PeerError::Binding)));
    assert!(buyer_channel.receive().is_err());
    assert!(FileTranscriptStore::open(s.path())
        .unwrap()
        .messages("agent", initial.terms().deal_id)
        .unwrap()
        .is_empty());
}

#[test]
fn failed_outgoing_authorization_persistence_sends_nothing_and_closes() {
    let (mut buyer, mut seller, _b, _s) = peers();
    freeze(&mut buyer, &mut seller);
    let root = buyer
        .state()
        .unwrap()
        .agreement()
        .unwrap()
        .0
        .transcript_root;
    let authorization = signed(&buyer.state().unwrap(), Role::Buyer);
    assert!(matches!(
        buyer.send_authorization(&authorization, NOW, |_| Err::<(), ()>(())),
        Err(PeerError::Persistence)
    ));
    assert!(matches!(buyer.channel.receive(), Err(SocketError::Closed)));
    let mut called = false;
    assert!(seller
        .receive_authorization(NOW, |_| {
            called = true;
            Ok::<_, ()>(())
        })
        .is_err());
    assert!(!called);
    assert_eq!(
        buyer
            .state()
            .unwrap()
            .agreement()
            .unwrap()
            .0
            .transcript_root,
        root
    );
    assert_eq!(
        seller
            .state()
            .unwrap()
            .agreement()
            .unwrap()
            .0
            .transcript_root,
        root
    );
}

#[test]
fn failed_incoming_authorization_persistence_withholds_ack_and_preserves_frozen_root() {
    let (mut buyer, mut seller, _b, _s) = peers();
    freeze(&mut buyer, &mut seller);
    let state = buyer.state().unwrap();
    let authorization = signed(&state, Role::Buyer);
    buyer
        .send_authorization(&authorization, NOW, |_| Ok::<_, ()>(()))
        .unwrap();
    assert!(matches!(
        seller.receive_authorization(NOW, |_| Err::<(), ()>(())),
        Err(PeerError::Persistence)
    ));
    assert!(matches!(seller.channel.receive(), Err(SocketError::Closed)));
    assert_eq!(
        seller.state().unwrap().agreement().unwrap(),
        state.agreement().unwrap()
    );
}

#[test]
fn invalid_final_signature_is_rejected_before_persistence_and_never_enters_transcript() {
    let (mut buyer, mut seller, _b, _s) = peers();
    freeze(&mut buyer, &mut seller);
    let state = buyer.state().unwrap();
    let authorization = signed(&state, Role::Buyer);
    let mut message = state
        .authorization_message(buyer.channel.session_id(), &authorization, NOW)
        .unwrap();
    let last = message.body.len() - 2;
    message.body[last] ^= 1;
    buyer.channel.send(&message).unwrap();
    let mut called = false;
    assert!(seller
        .receive_authorization(NOW, |_| {
            called = true;
            Ok::<_, ()>(())
        })
        .is_err());
    assert!(!called);
    assert!(matches!(seller.channel.receive(), Err(SocketError::Closed)));
    assert_eq!(seller.state().unwrap().transcript().total(), 4);
}

#[test]
fn an_incoming_invalid_transition_closes_without_persisting() {
    let (mut buyer, mut seller, _b, _s) = peers();
    let mut message = buyer
        .state()
        .unwrap()
        .propose(
            buyer.channel.session_id(),
            Role::Buyer,
            proposal(1).terms().amount,
            NOW,
        )
        .unwrap();
    message.revision += 1;
    buyer.channel.send(&message).unwrap();
    assert!(seller.receive(NOW).is_err());
    assert!(matches!(seller.channel.receive(), Err(SocketError::Closed)));
    assert!(seller.state().unwrap().transcript().is_empty());
}
