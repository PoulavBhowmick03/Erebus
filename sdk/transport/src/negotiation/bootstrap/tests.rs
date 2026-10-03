use super::*;
use crate::{
    identity::AuthorizationIdentity,
    negotiation::{peer::tests::connections, tests::proposal, tests::signed},
    store::TranscriptStore,
};
use erebus_core::{commitment::CommitmentBlinding, ids::BaseUnits};
use std::thread;

const NOW: u64 = 1_700_000_000;

fn context(suite: u16) -> SettlementContext {
    let draft = proposal(suite);
    let terms = draft.terms();
    SettlementContext {
        require_local_proving: suite == 2,
        mode: terms.settlement_mode,
        domain: terms.domain.clone(),
        suite_id: suite,
        asset: terms.asset.clone(),
        required_guarantees: terms.required_guarantees,
    }
}

fn bootstraps(suite: u16) -> (NegotiationBootstrap, NegotiationBootstrap) {
    let (buyer, seller, buyer_descriptor, seller_descriptor) = connections(suite);
    if suite == 1 {
        return (
            NegotiationBootstrap::public_bound(
                buyer,
                context(suite),
                &buyer_descriptor,
                &seller_descriptor,
                NOW,
            )
            .unwrap(),
            NegotiationBootstrap::public_bound(
                seller,
                context(suite),
                &buyer_descriptor,
                &seller_descriptor,
                NOW,
            )
            .unwrap(),
        );
    }
    let bind = |descriptor: &ServiceDescriptor, role: Role| {
        let initial = proposal(2);
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
    let buyer_binding = bind(&buyer_descriptor, Role::Buyer);
    let seller_binding = bind(&seller_descriptor, Role::Seller);
    let buyer_descriptor_copy = buyer_descriptor.clone();
    let seller_descriptor_copy = seller_descriptor.clone();
    let seller_worker = thread::spawn(move || {
        NegotiationBootstrap::shielded(
            seller,
            context(2),
            &buyer_descriptor_copy,
            &seller_descriptor_copy,
            &seller_binding,
            Some(proposal(2).terms().payment_recipient.clone()),
            NOW,
        )
        .unwrap()
    });
    let buyer = NegotiationBootstrap::shielded(
        buyer,
        context(2),
        &buyer_descriptor,
        &seller_descriptor,
        &buyer_binding,
        None,
        NOW,
    )
    .unwrap();
    (buyer, seller_worker.join().unwrap())
}

fn private_draft(bootstrap: &NegotiationBootstrap, suite: u16) -> Proposal {
    let mut terms = proposal(suite).terms().clone();
    terms.buyer_authorization_key = bootstrap.buyer_key().clone();
    terms.seller_authorization_key = bootstrap.seller_key().clone();
    terms.payment_recipient = bootstrap.payment_recipient().clone();
    terms.service.access_recipient = bootstrap.buyer_key().clone();
    terms.deal_id = [81; 16];
    terms.settlement_nonce = [82; 32];
    Proposal::new(terms, CommitmentBlinding::from_bytes([7; 32])).unwrap()
}

fn finish(buyer: &mut NegotiationPeer, seller: &mut NegotiationPeer) {
    seller.propose(BaseUnits::new(70), NOW).unwrap();
    buyer.receive(NOW).unwrap();
    buyer.accept(NOW).unwrap();
    seller.receive(NOW).unwrap();
    seller.accept(NOW).unwrap();
    buyer.receive(NOW).unwrap();
    let state = buyer.state().unwrap();
    buyer
        .send_authorization(&signed(&state, Role::Buyer), NOW, |_| Ok::<_, ()>(()))
        .unwrap();
    seller
        .receive_authorization(NOW, |_| Ok::<_, ()>(()))
        .unwrap();
    seller
        .send_authorization(&signed(&state, Role::Seller), NOW, |_| Ok::<_, ()>(()))
        .unwrap();
    buyer
        .receive_authorization(NOW, |_| Ok::<_, ()>(()))
        .unwrap();
}

#[test]
fn seller_learns_offer_only_over_noise_and_both_suites_authorize_the_frozen_agreement() {
    for suite in [1, 2] {
        let (buyer, seller) = bootstraps(suite);
        let draft = private_draft(&buyer, suite);
        let buyer_root = tempfile::tempdir().unwrap();
        let seller_root = tempfile::tempdir().unwrap();
        let mut buyer = buyer
            .offer(
                FileTranscriptStore::open(buyer_root.path()).unwrap(),
                "agent".into(),
                draft,
                NOW,
            )
            .unwrap();
        // No draft, nonce, deal ID, or blinding is passed to the seller API.
        let mut seller = seller
            .receive_offer(
                FileTranscriptStore::open(seller_root.path()).unwrap(),
                "agent".into(),
                NOW,
                |received| {
                    assert_eq!(received.terms().deal_id, [81; 16]);
                    assert_eq!(received.terms().settlement_nonce, [82; 32]);
                    assert_eq!(received.blinding().as_bytes(), &[7; 32]);
                    Ok::<_, ()>(())
                },
            )
            .unwrap();
        finish(&mut buyer, &mut seller);
        assert_eq!(
            buyer.state().unwrap().agreement().unwrap(),
            seller.state().unwrap().agreement().unwrap()
        );
        assert_eq!(
            seller
                .state()
                .unwrap()
                .transcript()
                .next_sequence(Role::Seller),
            3,
            "identity bindings and final authorizations must stay outside the transcript"
        );
    }
}

#[test]
fn restarted_bootstrap_redelivers_the_retained_offer_without_changing_a_frozen_root() {
    for suite in [1, 2] {
        let buyer_root = tempfile::tempdir().unwrap();
        let seller_root = tempfile::tempdir().unwrap();
        let buyer_store = FileTranscriptStore::open(buyer_root.path()).unwrap();
        let seller_store = FileTranscriptStore::open(seller_root.path()).unwrap();
        let (buyer, seller) = bootstraps(suite);
        let draft = private_draft(&buyer, suite);
        let mut buyer = buyer
            .offer(buyer_store.clone(), "agent".into(), draft.clone(), NOW)
            .unwrap();
        let mut seller = seller
            .receive_offer(seller_store.clone(), "agent".into(), NOW, |_| {
                Ok::<_, ()>(())
            })
            .unwrap();
        finish(&mut buyer, &mut seller);
        let frozen = buyer.state().unwrap().agreement().unwrap();
        drop(buyer);
        drop(seller);
        let (buyer, seller) = bootstraps(suite);
        // A fresh buyer reconstructs the original private draft from its own retained offer.
        let retained = buyer_store.messages("agent", [81; 16]).unwrap();
        let recovered = Proposal::from_initial_offer(&retained[0], NOW).unwrap();
        assert_eq!(recovered, draft);
        let buyer = buyer
            .offer(buyer_store.clone(), "agent".into(), recovered, NOW)
            .unwrap();
        let seller = seller
            .receive_offer(seller_store.clone(), "agent".into(), NOW, |_| {
                Ok::<_, ()>(())
            })
            .unwrap();
        assert_eq!(buyer.state().unwrap().agreement().unwrap(), frozen);
        assert_eq!(seller.state().unwrap().agreement().unwrap(), frozen);
        assert_eq!(buyer_store.messages("agent", [81; 16]).unwrap().len(), 4);
        assert_eq!(seller_store.messages("agent", [81; 16]).unwrap().len(), 4);
    }
}

#[test]
fn rejected_service_policy_leaves_no_incoming_deal_record() {
    for suite in [1, 2] {
        let (buyer, seller) = bootstraps(suite);
        let draft = private_draft(&buyer, suite);
        let buyer_root = tempfile::tempdir().unwrap();
        let seller_root = tempfile::tempdir().unwrap();
        let seller_store = FileTranscriptStore::open(seller_root.path()).unwrap();
        let _buyer = buyer
            .offer(
                FileTranscriptStore::open(buyer_root.path()).unwrap(),
                "agent".into(),
                draft,
                NOW,
            )
            .unwrap();
        assert!(matches!(
            seller.receive_offer(seller_store.clone(), "agent".into(), NOW, |_| Err::<(), _>(
                ()
            )),
            Err(PeerError::Policy)
        ));
        assert!(seller_store.messages("agent", [81; 16]).unwrap().is_empty());
    }
}

#[test]
fn wrong_role_or_substituted_deployment_key_or_recipient_cannot_send_an_offer() {
    for suite in [1, 2] {
        for field in 0..4 {
            let (buyer, _seller) = bootstraps(suite);
            let draft = private_draft(&buyer, suite);
            let mut terms = draft.terms().clone();
            match field {
                0 => terms.domain.verifier_version += 1,
                1 => {
                    terms.payment_recipient =
                        proposal(suite).terms().buyer_authorization_key.clone()
                }
                2 => terms.seller_authorization_key = terms.buyer_authorization_key.clone(),
                3 => {
                    terms.asset = erebus_core::ids::AssetId::parse(
                        "eip155:10143/erc20:0x4444444444444444444444444444444444444444",
                    )
                    .unwrap()
                }
                _ => unreachable!(),
            }
            // Suite-2 recipient-width errors can already be rejected by canonical mapping.
            if let Ok(changed) = Proposal::new(terms, draft.blinding().clone()) {
                let root = tempfile::tempdir().unwrap();
                let store = FileTranscriptStore::open(root.path()).unwrap();
                assert!(buyer
                    .offer(store.clone(), "agent".into(), changed, NOW)
                    .is_err());
                assert!(store.messages("agent", [81; 16]).unwrap().is_empty());
            }
        }
        let (_buyer, seller) = bootstraps(suite);
        let root = tempfile::tempdir().unwrap();
        assert!(seller
            .offer(
                FileTranscriptStore::open(root.path()).unwrap(),
                "agent".into(),
                proposal(suite),
                NOW,
            )
            .is_err());
    }
}

#[test]
fn first_offer_decoder_rejects_noninitial_envelopes_and_expiry() {
    let draft = proposal(1);
    let state = super::super::Negotiation::new(draft.clone()).unwrap();
    let initial = state
        .propose([3; 32], Role::Buyer, draft.terms().amount, NOW)
        .unwrap();
    assert_eq!(Proposal::from_initial_offer(&initial, NOW).unwrap(), draft);
    for field in 0..7 {
        let mut changed = initial.clone();
        match field {
            0 => changed.author = Role::Seller,
            1 => changed.deal_id = [9; 16],
            2 => changed.revision = 2,
            3 => changed.sequence = 2,
            4 => changed.parent_hash = [8; 32],
            5 => changed.message_type = MessageType::Counter,
            6 => changed.body.push(0),
            _ => unreachable!(),
        }
        assert!(Proposal::from_initial_offer(&changed, NOW).is_err());
    }
    assert!(Proposal::from_initial_offer(&initial, draft.terms().expiry).is_err());
}

#[test]
fn shielded_context_substitution_rejects_identity_exchange_before_offering() {
    let (buyer, seller, buyer_descriptor, seller_descriptor) = connections(2);
    let binding = |descriptor: &ServiceDescriptor, role: Role| {
        let terms = proposal(2).terms().clone();
        AgreementKeyBinding::sign(
            descriptor,
            terms.domain,
            if role == Role::Buyer {
                terms.buyer_authorization_key
            } else {
                terms.seller_authorization_key
            },
            &AuthorizationIdentity::from_bytes(&[role.tag(); 32]).unwrap(),
            NOW,
            NOW + 300,
        )
        .unwrap()
    };
    let buyer_binding = binding(&buyer_descriptor, Role::Buyer);
    let seller_binding = binding(&seller_descriptor, Role::Seller);
    let b = buyer_descriptor.clone();
    let s = seller_descriptor.clone();
    let worker = thread::spawn(move || {
        let mut changed = context(2);
        changed.domain.verifier_version += 1;
        let seller_binding = AgreementKeyBinding::sign(
            &s,
            changed.domain.clone(),
            seller_binding
                .attested_key(&s, &context(2).domain, NOW)
                .unwrap(),
            &AuthorizationIdentity::from_bytes(&[Role::Seller.tag(); 32]).unwrap(),
            NOW,
            NOW + 300,
        )
        .unwrap();
        NegotiationBootstrap::shielded(
            seller,
            changed,
            &b,
            &s,
            &seller_binding,
            Some(proposal(2).terms().payment_recipient.clone()),
            NOW,
        )
    });
    assert!(NegotiationBootstrap::shielded(
        buyer,
        context(2),
        &buyer_descriptor,
        &seller_descriptor,
        &buyer_binding,
        None,
        NOW
    )
    .is_err());
    assert!(worker.join().unwrap().is_err());
}

#[test]
fn new_drafts_use_fresh_entropy_but_do_not_weaken_the_selected_context() {
    for suite in [1, 2] {
        let (buyer, _seller) = bootstraps(suite);
        let template = proposal(suite);
        let first = buyer
            .create_proposal(template.terms().clone(), NOW)
            .unwrap();
        let second = buyer
            .create_proposal(template.terms().clone(), NOW)
            .unwrap();
        assert_ne!(first.terms().deal_id, second.terms().deal_id);
        assert_ne!(
            first.terms().settlement_nonce,
            second.terms().settlement_nonce
        );
        assert_ne!(first.blinding(), second.blinding());
        assert_eq!(first.terms().service, template.terms().service);
        assert_eq!(first.terms().expiry, template.terms().expiry);
        assert_eq!(first.terms().fee_policy, template.terms().fee_policy);
        let mut changed = template.terms().clone();
        changed.domain.verifier_version += 1;
        assert!(buyer.create_proposal(changed, NOW).is_err());
        let mut changed = template.terms().clone();
        changed.amount = BaseUnits::new(0);
        assert!(buyer.create_proposal(changed, NOW).is_err());
        assert!(buyer
            .create_proposal(template.terms().clone(), template.terms().expiry)
            .is_err());
    }
}

#[test]
fn interrupted_delivery_recovers_the_same_prefix_before_any_final_signing() {
    for suite in [1, 2] {
        for lost_event in 0..3 {
            let buyer_root = tempfile::tempdir().unwrap();
            let seller_root = tempfile::tempdir().unwrap();
            let buyer_store = FileTranscriptStore::open(buyer_root.path()).unwrap();
            let seller_store = FileTranscriptStore::open(seller_root.path()).unwrap();
            let (buyer, seller) = bootstraps(suite);
            let draft = private_draft(&buyer, suite);
            let mut buyer = buyer
                .offer(buyer_store.clone(), "agent".into(), draft.clone(), NOW)
                .unwrap();
            let mut seller = seller
                .receive_offer(seller_store.clone(), "agent".into(), NOW, |_| {
                    Ok::<_, ()>(())
                })
                .unwrap();
            seller.propose(BaseUnits::new(70), NOW).unwrap();
            if lost_event >= 1 {
                buyer.receive(NOW).unwrap();
                buyer.accept(NOW).unwrap();
            }
            if lost_event >= 2 {
                seller.receive(NOW).unwrap();
                seller.accept(NOW).unwrap();
            }
            // Recipient exits without reading the last already-durable encrypted event.
            drop(buyer);
            drop(seller);
            let (buyer, seller) = bootstraps(suite);
            let mut buyer = buyer
                .offer(buyer_store.clone(), "agent".into(), draft, NOW)
                .unwrap();
            let mut seller = seller
                .receive_offer(seller_store.clone(), "agent".into(), NOW, |_| {
                    Ok::<_, ()>(())
                })
                .unwrap();
            let worker = thread::spawn(move || seller.synchronize(NOW));
            let recovered = buyer.synchronize(NOW).unwrap();
            let remote = worker.join().unwrap().unwrap();
            assert_eq!(
                recovered.transcript().root().unwrap(),
                remote.transcript().root().unwrap()
            );
            assert_eq!(recovered.is_frozen(), lost_event == 2);
            assert_eq!(
                buyer_store.messages("agent", [81; 16]).unwrap().len(),
                lost_event + 2
            );
            assert_eq!(
                seller_store.messages("agent", [81; 16]).unwrap().len(),
                lost_event + 2
            );
        }
    }
}
