use super::*;
use crate::{
    disclosure::{verify_selected_agreement, SelectedAgreement},
    identity::AuthorizationIdentity,
};
use erebus_core::{
    auth::authorization_digest,
    commitment::deal_nullifier,
    ids::{KeyBytes, SignatureBytes},
    shielded::{note_spend_tag, ShieldedDeal},
    shielded_auth::{derive_key, sign_message},
    terms::{Guarantee, GuaranteeSet, SettlementMode},
};
use std::{
    fs,
    sync::{Arc, Barrier},
    thread,
};

const NOW: u64 = 1_700_000_000;

pub(crate) fn proposal(suite: u16) -> Proposal {
    let vector: serde_json::Value = serde_json::from_str(include_str!(
        "../../../core/tests/fixtures/agreement-v1-vectors.json"
    ))
    .unwrap();
    let bytes = hex::decode(
        vector["vectors"][0]["expected"]["canonicalHex"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let mut terms = AgreementTerms::decode(&bytes).unwrap();
    terms.transcript_root = [0; 32];
    if suite == 2 {
        terms.suite_id = 2;
        terms.domain.pool = terms.domain.settlement_contract.clone();
        terms.settlement_mode = SettlementMode::Shielded;
        terms.required_guarantees = GuaranteeSet::empty();
        for guarantee in [
            Guarantee::HiddenAmount,
            Guarantee::HiddenRecipient,
            Guarantee::AgreementBoundSettlement,
        ] {
            terms.required_guarantees.insert(guarantee);
        }
        terms.buyer_authorization_key =
            KeyBytes::new(derive_key(&[1; 32]).unwrap().to_vec()).unwrap();
        terms.seller_authorization_key =
            KeyBytes::new(derive_key(&[2; 32]).unwrap().to_vec()).unwrap();
        terms.service.access_recipient = terms.buyer_authorization_key.clone();
        terms.payment_recipient =
            KeyBytes::new(note_spend_tag(&[3; 32]).unwrap().to_vec()).unwrap();
    }
    Proposal::new(terms, CommitmentBlinding::from_bytes([3; 32])).unwrap()
}

pub(super) fn signed(state: &Negotiation, role: Role) -> Authorization {
    let (terms, _, commitment) = state.agreement().unwrap();
    let seed = [role.tag(); 32];
    let signature = if terms.suite_id == 1 {
        let identity = AuthorizationIdentity::from_bytes(&seed).unwrap();
        let digest = authorization_digest(&terms.domain, role, &commitment, 1).unwrap();
        identity.sign_digest(&digest).to_vec()
    } else {
        let message = ShieldedDeal::from_terms(&terms)
            .unwrap()
            .authorization_message(role, &commitment)
            .unwrap();
        sign_message(&seed, &message).unwrap().1.to_vec()
    };
    Authorization {
        role,
        suite_id: terms.suite_id,
        commitment,
        signature: SignatureBytes::new(signature).unwrap(),
    }
}

fn complete(state: &mut Negotiation) -> Vec<Message> {
    let offer = state
        .propose([1; 32], Role::Buyer, state.initial.terms.amount, NOW)
        .unwrap();
    state.append(&offer).unwrap();
    let counter = state
        .propose([1; 32], Role::Seller, BaseUnits::new(70), NOW)
        .unwrap();
    state.append(&counter).unwrap();
    let buyer = state.accept([1; 32], Role::Buyer, NOW).unwrap();
    state.append(&buyer).unwrap();
    let seller = state.accept([1; 32], Role::Seller, NOW).unwrap();
    state.append(&seller).unwrap();
    vec![offer, counter, buyer, seller]
}

#[test]
fn both_suites_freeze_before_signatures_without_changing_m1_or_m2_hashes() {
    for suite in [1, 2] {
        let initial = proposal(suite);
        let mut buyer = Negotiation::new(initial.clone()).unwrap();
        assert!(matches!(
            buyer.agreement(),
            Err(NegotiationError::NotFrozen)
        ));
        let messages = complete(&mut buyer);
        let seller = Negotiation::replay(initial.clone(), &messages).unwrap();
        let (terms, blinding, commitment) = buyer.agreement().unwrap();
        assert_ne!(terms.transcript_root, [0; 32]);
        assert_eq!(terms.amount.get(), 70);
        assert_eq!(terms.revision, 2);
        assert_eq!(
            seller.agreement().unwrap(),
            (terms.clone(), blinding.clone(), commitment)
        );
        assert_eq!(
            deal_nullifier(&initial.terms).unwrap(),
            deal_nullifier(&terms).unwrap()
        );
        assert_eq!(
            Transcript::replay(terms.deal_id, TRANSCRIPT_HASH_VERSION, &messages)
                .unwrap()
                .root()
                .unwrap(),
            terms.transcript_root
        );
        assert_eq!(commit_agreement(&terms, &blinding).unwrap(), commitment);
        assert_ne!(initial.digest().unwrap(), *commitment.as_bytes());
        let buyer_auth = signed(&buyer, Role::Buyer);
        let seller_auth = signed(&seller, Role::Seller);
        for authorization in [&buyer_auth, &seller_auth] {
            let envelope = buyer
                .authorization_message([2; 32], authorization, NOW)
                .unwrap();
            assert_eq!(
                seller.verify_final_authorization(&envelope, NOW).unwrap(),
                *authorization
            );
            assert!(buyer.append(&envelope).is_err());
            assert_eq!(buyer.agreement().unwrap().2, commitment);
        }
        let evidence = SelectedAgreement {
            terms,
            blinding,
            buyer: buyer_auth,
            seller: seller_auth,
            transcript_hash_version: TRANSCRIPT_HASH_VERSION,
            messages,
        };
        assert_eq!(
            verify_selected_agreement(&evidence).unwrap().commitment,
            commitment
        );
    }
}

#[test]
fn bad_or_noncanonical_drafts_are_rejected_and_debug_is_redacted() {
    for suite in [1, 2] {
        let initial = proposal(suite);
        assert_eq!(
            Proposal::decode(&initial.encode().unwrap()).unwrap(),
            initial
        );
        let encoded = initial.encode().unwrap();
        for end in [0, 1, 2, encoded.len() - 1] {
            assert!(Proposal::decode(&encoded[..end]).is_err());
        }
        let mut trailing = encoded.clone();
        trailing.push(0);
        assert!(Proposal::decode(&trailing).is_err());
        let mut nonzero_root = initial.terms.clone();
        nonzero_root.transcript_root = [1; 32];
        assert!(Proposal::new(nonzero_root, initial.blinding.clone()).is_err());
        assert_eq!(format!("{initial:?}"), "Proposal(<redacted>)");
        assert_eq!(
            format!("{:?}", Negotiation::new(initial.clone()).unwrap()),
            "Negotiation(<redacted>)"
        );
        assert!(initial.counter(BaseUnits::new(0)).is_err());
    }
}

#[test]
fn wrong_turn_revision_parent_and_changed_context_do_not_mutate_state() {
    let initial = proposal(1);
    let mut state = Negotiation::new(initial).unwrap();
    assert!(state
        .propose([1; 32], Role::Seller, state.initial.terms.amount, NOW)
        .is_err());
    assert!(state.accept([1; 32], Role::Buyer, NOW).is_err());
    let offer = state
        .propose([1; 32], Role::Buyer, state.initial.terms.amount, NOW)
        .unwrap();
    state.append(&offer).unwrap();
    assert!(state
        .propose([1; 32], Role::Buyer, BaseUnits::new(70), NOW)
        .is_err());
    assert!(state.accept([1; 32], Role::Buyer, NOW).is_err());
    let counter = state
        .propose([1; 32], Role::Seller, BaseUnits::new(70), NOW)
        .unwrap();
    let before = state.transcript.clone();
    for mutation in 0..8 {
        let mut bad = counter.clone();
        let Event::Proposal {
            mut parent,
            mut proposal,
        } = Event::decode(&bad.body).unwrap()
        else {
            panic!("proposal")
        };
        match mutation {
            0 => {
                proposal.terms.domain.verifier_version += 1;
            }
            1 => {
                proposal.terms.settlement_nonce[0] ^= 1;
            }
            2 => {
                proposal.terms.buyer_authorization_key = KeyBytes::new(vec![9; 20]).unwrap();
            }
            3 => {
                proposal.terms.expiry += 1;
            }
            4 => {
                proposal.terms.service.resource = "another.resource".into();
            }
            5 => {
                proposal.blinding = CommitmentBlinding::from_bytes([4; 32]);
            }
            6 => {
                parent[0] ^= 1;
            }
            7 => {
                bad.revision += 1;
            }
            _ => unreachable!(),
        }
        bad.body = Event::Proposal { parent, proposal }.encode().unwrap();
        assert!(state.append(&bad).is_err(), "mutation {mutation}");
        assert_eq!(state.transcript, before);
    }
    let mut bad = counter.clone();
    bad.body[1] = 2;
    assert!(state.append(&bad).is_err());
    state.append(&counter).unwrap();
    let accept = state.accept([1; 32], Role::Buyer, NOW).unwrap();
    let mut stale_accept = accept.clone();
    stale_accept.body = Event::Accept([9; 32]).encode().unwrap();
    assert!(state.append(&stale_accept).is_err());
    state.append(&accept).unwrap();
    assert!(state
        .propose([1; 32], Role::Seller, BaseUnits::new(80), NOW)
        .is_err());
    assert!(state.accept([1; 32], Role::Buyer, NOW).is_err());
    assert!(matches!(
        state.agreement(),
        Err(NegotiationError::NotFrozen)
    ));
    let accept = state.accept([1; 32], Role::Seller, NOW).unwrap();
    state.append(&accept).unwrap();
    assert!(state.accept([1; 32], Role::Seller, NOW).is_err());
}

#[test]
fn live_transitions_and_authorizations_reject_expiry_at_the_boundary() {
    let mut state = Negotiation::new(proposal(1)).unwrap();
    let expiry = state.initial.terms.expiry;
    assert!(matches!(
        state.propose([1; 32], Role::Buyer, state.initial.terms.amount, expiry),
        Err(NegotiationError::Expired)
    ));
    complete(&mut state);
    let authorization = signed(&state, Role::Seller);
    let envelope = state
        .authorization_message([2; 32], &authorization, expiry - 1)
        .unwrap();
    assert!(matches!(
        state.verify_final_authorization(&envelope, expiry),
        Err(NegotiationError::Expired)
    ));
    let mut wrong_role = envelope.clone();
    wrong_role.author = Role::Buyer;
    assert!(state.verify_final_authorization(&wrong_role, NOW).is_err());
    let mut bad = authorization;
    bad.commitment = DealCommitment::from_bytes([1; 32]);
    assert!(state.authorization_message([2; 32], &bad, NOW).is_err());
}

#[test]
fn durable_freeze_survives_restart_and_lost_delivery_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let file = FileTranscriptStore::open(dir.path()).unwrap();
    let initial = proposal(1);
    let store = NegotiationStore::new(file.clone(), "agent".into(), initial.clone()).unwrap();
    let mut state = store.load().unwrap();
    let offer = state
        .propose([1; 32], Role::Buyer, initial.terms.amount, NOW)
        .unwrap();
    state = store.append(&offer).unwrap();
    let counter = state
        .propose([1; 32], Role::Seller, BaseUnits::new(70), NOW)
        .unwrap();
    state = store.append(&counter).unwrap();
    let buyer = state.accept([1; 32], Role::Buyer, NOW).unwrap();
    store.append(&buyer).unwrap();
    drop(store);
    let reopened = NegotiationStore::new(file.clone(), "agent".into(), initial.clone()).unwrap();
    state = reopened.load().unwrap();
    assert!(!state.is_frozen());
    let seller = state.accept([2; 32], Role::Seller, NOW).unwrap();
    state = reopened.append(&seller).unwrap();
    let final_agreement = state.agreement().unwrap();
    for mut previous in [offer, counter, buyer, seller] {
        previous.session_id = [3; 32];
        assert_eq!(
            reopened.append(&previous).unwrap().agreement().unwrap(),
            final_agreement
        );
        previous.body.push(1);
        assert!(reopened.append(&previous).is_err());
    }
    let final_envelope = state
        .authorization_message([3; 32], &signed(&state, Role::Buyer), NOW)
        .unwrap();
    assert!(matches!(
        file.append("agent", initial.terms.deal_id, 1, &final_envelope),
        Err(StoreError::Frozen)
    ));
    assert!(reopened.append(&final_envelope).is_err());
    assert_eq!(
        file.messages("agent", initial.terms.deal_id).unwrap().len(),
        4
    );
    assert_eq!(
        file.frozen("agent", initial.terms.deal_id, 1)
            .unwrap()
            .unwrap()
            .root(),
        final_agreement.0.transcript_root
    );
    assert_eq!(
        NegotiationStore::new(file, "agent".into(), initial)
            .unwrap()
            .load()
            .unwrap()
            .agreement()
            .unwrap(),
        final_agreement
    );
}

#[test]
fn committed_acceptance_repairs_a_missing_freeze_but_corrupt_records_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let file = FileTranscriptStore::open(dir.path()).unwrap();
    let initial = proposal(1);
    let store = NegotiationStore::new(file.clone(), "agent".into(), initial.clone()).unwrap();
    let mut state = Negotiation::new(initial.clone()).unwrap();
    let messages = complete(&mut state);
    // Model a crash after the final log fsync but before publishing the freeze record.
    for message in &messages {
        file.append("agent", initial.terms.deal_id, 1, message)
            .unwrap();
    }
    let accepted = store.load().unwrap().agreement().unwrap();
    let marker = dir
        .path()
        .join("agent")
        .join(hex::encode(initial.terms.deal_id))
        .join("negotiation.freeze");
    assert!(marker.exists());
    let valid = fs::read(&marker).unwrap();
    let mut corrupt = valid.clone();
    corrupt[30] ^= 1;
    fs::write(&marker, corrupt).unwrap();
    assert!(store.load().is_err());
    assert!(file.messages("agent", initial.terms.deal_id).is_err());
    fs::write(&marker, &valid[..valid.len() - 1]).unwrap();
    assert!(store.load().is_err());
    fs::write(&marker, valid).unwrap();
    assert_eq!(store.load().unwrap().agreement().unwrap(), accepted);
    assert!(file
        .freeze(
            "agent",
            initial.terms.deal_id,
            1,
            accepted.0.transcript_root,
            [1; 32]
        )
        .is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&marker).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn early_low_level_freeze_is_not_treated_as_bilateral_acceptance() {
    let dir = tempfile::tempdir().unwrap();
    let file = FileTranscriptStore::open(dir.path()).unwrap();
    let initial = proposal(1);
    let mut state = Negotiation::new(initial.clone()).unwrap();
    let offer = state
        .propose([1; 32], Role::Buyer, initial.terms.amount, NOW)
        .unwrap();
    state.append(&offer).unwrap();
    file.append("agent", initial.terms.deal_id, 1, &offer)
        .unwrap();
    file.freeze(
        "agent",
        initial.terms.deal_id,
        1,
        state.transcript.root().unwrap(),
        initial.digest().unwrap(),
    )
    .unwrap();
    assert!(NegotiationStore::new(file, "agent".into(), initial).is_err());
}

#[test]
fn freeze_and_concurrent_append_share_the_same_lock() {
    for _ in 0..12 {
        let dir = tempfile::tempdir().unwrap();
        let file = FileTranscriptStore::open(dir.path()).unwrap();
        let initial = proposal(1);
        let mut state = Negotiation::new(initial.clone()).unwrap();
        let offer = state
            .propose([1; 32], Role::Buyer, initial.terms.amount, NOW)
            .unwrap();
        let transcript = file
            .append("agent", initial.terms.deal_id, 1, &offer)
            .unwrap();
        state.append(&offer).unwrap();
        let counter = state
            .propose([1; 32], Role::Seller, BaseUnits::new(70), NOW)
            .unwrap();
        let root = transcript.root().unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let b = barrier.clone();
        let f = file.clone();
        let deal_id = initial.terms.deal_id;
        let worker = thread::spawn(move || {
            b.wait();
            f.append("agent", deal_id, 1, &counter)
        });
        barrier.wait();
        let freeze = file.freeze("agent", deal_id, 1, root, initial.digest().unwrap());
        let appended = worker.join().unwrap();
        assert_ne!(freeze.is_ok(), appended.is_ok());
        let observed = file.load("agent", deal_id, 1).unwrap();
        assert_eq!(observed.total(), if freeze.is_ok() { 1 } else { 2 });
        assert_eq!(
            file.frozen("agent", deal_id, 1).unwrap().is_some(),
            freeze.is_ok()
        );
    }
}
