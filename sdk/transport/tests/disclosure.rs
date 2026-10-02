//! Offline verification of a selected Metropolis agreement.

use erebus_core::auth::{authorization_digest, Authorization, Role};
use erebus_core::commitment::{commit_agreement, CommitmentBlinding};
use erebus_core::domain::DeploymentDomain;
use erebus_core::ids::{
    AddressBytes, AssetId, BaseUnits, ChainNamespace, KeyBytes, SignatureBytes,
};
use erebus_core::service::ServiceRecord;
use erebus_core::terms::{
    AgreementTerms, FeePolicy, Guarantee, GuaranteeSet, SettlementMode, CURRENT_PROTOCOL_VERSION,
};
use erebus_transport::disclosure::{
    verify_selected_agreement, DisclosureError, DisclosureGrant, GrantError, SelectedAgreement,
};
use erebus_transport::hashing::TRANSCRIPT_HASH_VERSION;
use erebus_transport::identity::TransportIdentity;
use erebus_transport::identity::{AuthorizationIdentity, DisclosureIdentity};
use erebus_transport::message::{Message, MessageType};
use erebus_transport::relay::{FileRelay, MailboxId, Relay};
use erebus_transport::session::{self, Handshake};
use erebus_transport::store::{FileTranscriptStore, TranscriptStore};
use erebus_transport::transcript::Transcript;

const BACKUP_NAMESPACE: &str = "metropolis.backup";
const BACKUP_NOW: u64 = 1_700_000_000;

fn shielded_fixture() -> SelectedAgreement {
    use erebus_core::{
        shielded::note_spend_tag,
        shielded_auth::{derive_key, sign_message},
    };
    let mut evidence = fixture();
    evidence.terms.suite_id = 2;
    evidence.terms.settlement_mode = SettlementMode::Shielded;
    evidence.terms.domain.pool = evidence.terms.domain.settlement_contract.clone();
    evidence.terms.required_guarantees = GuaranteeSet::from_bits(7).unwrap();
    evidence.terms.buyer_authorization_key =
        KeyBytes::new(derive_key(&[1; 32]).unwrap().to_vec()).unwrap();
    evidence.terms.seller_authorization_key =
        KeyBytes::new(derive_key(&[2; 32]).unwrap().to_vec()).unwrap();
    let mut secret = [0; 32];
    secret[31] = 3;
    evidence.terms.payment_recipient =
        KeyBytes::new(note_spend_tag(&secret).unwrap().to_vec()).unwrap();
    evidence.blinding = CommitmentBlinding::from_bytes(secret);
    let commitment = commit_agreement(&evidence.terms, &evidence.blinding).unwrap();
    let authorize = |role, seed| {
        let message = authorization_digest(&evidence.terms.domain, role, &commitment, 2).unwrap();
        let (_, signature) = sign_message(&seed, &message).unwrap();
        Authorization {
            role,
            suite_id: 2,
            commitment,
            signature: SignatureBytes::new(signature.to_vec()).unwrap(),
        }
    };
    evidence.buyer = authorize(Role::Buyer, [1; 32]);
    evidence.seller = authorize(Role::Seller, [2; 32]);
    evidence
}

#[test]
fn shielded_grants_authenticate_either_participant_without_a_secp_identity() {
    let evidence = shielded_fixture();
    let recipient = DisclosureIdentity::generate().unwrap();
    for seed in [[1; 32], [2; 32]] {
        let grant =
            DisclosureGrant::seal_shielded(&evidence, &seed, recipient.public_key(), 1000, 100)
                .unwrap();
        let encoded = grant.encode().unwrap();
        assert_eq!(&encoded[..2], &[0, 2]);
        assert!(!encoded
            .windows(b"private".len())
            .any(|part| part == b"private"));
        let grant = DisclosureGrant::decode(&encoded).unwrap();
        let (restored, facts) = grant.open(&recipient, &grant.issuer, 100).unwrap();
        assert_eq!(facts.commitment, evidence.buyer.commitment);
        assert_eq!(restored.encode().unwrap(), evidence.encode().unwrap());
        assert!(grant.open(&recipient, [0; 64], 100).is_err());
        assert!(grant
            .open(&DisclosureIdentity::generate().unwrap(), &grant.issuer, 100)
            .is_err());
        assert!(grant.open(&recipient, &grant.issuer, 1000).is_err());
        for offset in [2, 66, 98, 114, encoded.len() - 1] {
            let mut altered = encoded.clone();
            altered[offset] ^= 1;
            if let Ok(altered) = DisclosureGrant::decode(&altered) {
                assert!(altered.open(&recipient, &grant.issuer, 100).is_err());
            }
        }
        for version in [1u16, 3] {
            let mut altered = encoded.clone();
            altered[..2].copy_from_slice(&version.to_be_bytes());
            if let Ok(altered) = DisclosureGrant::decode(&altered) {
                assert!(altered.open(&recipient, &grant.issuer, 100).is_err());
            }
        }
        let mut trailing = encoded;
        trailing.push(0);
        assert!(DisclosureGrant::decode(&trailing).is_err());
    }
    assert!(
        DisclosureGrant::seal_shielded(&evidence, &[3; 32], recipient.public_key(), 1000, 100)
            .is_err()
    );
    assert!(DisclosureGrant::seal(
        &evidence,
        &AuthorizationIdentity::from_bytes(&[1; 32]).unwrap(),
        recipient.public_key(),
        1000,
        100
    )
    .is_err());
    assert!(DisclosureGrant::seal_shielded(
        &fixture(),
        &[1; 32],
        recipient.public_key(),
        1000,
        100
    )
    .is_err());
}

#[test]
fn a_validly_signed_and_encrypted_nonparticipant_grant_is_rejected_on_open() {
    use erebus_core::{
        encoding::Writer,
        shielded::disclosure_message,
        shielded_auth::{derive_key, sign_message},
        suite::keccak256,
    };
    let evidence = shielded_fixture();
    let recipient = DisclosureIdentity::generate().unwrap();
    let seed = [3; 32];
    let issuer = derive_key(&seed).unwrap();
    let domain = b"EREBUS_DEAL_DISCLOSURE_V2_SUITE2";
    let expiry = 1000u64;
    let header = keccak256(&[
        domain,
        &issuer,
        &recipient.public_key(),
        &evidence.terms.deal_id,
        &expiry.to_be_bytes(),
    ]);
    // An adversarial issuer bypasses seal_shielded's participant check and builds a valid
    // Noise frame and signature. Opening must independently reject the issuer.
    let mut noise = snow::Builder::new("Noise_N_25519_ChaChaPoly_BLAKE2s".parse().unwrap())
        .prologue(&header)
        .remote_public_key(&recipient.public_key())
        .build_initiator()
        .unwrap();
    let mut buffer = vec![0; 65_535];
    let length = noise.write_message(&[], &mut buffer).unwrap();
    let handshake = buffer[..length].to_vec();
    let mut channel = noise.into_transport_mode().unwrap();
    let length = channel
        .write_message(&evidence.encode().unwrap(), &mut buffer)
        .unwrap();
    let frame = buffer[..length].to_vec();
    let mut signed = Writer::new();
    signed.fixed(&header);
    signed.bytes(&handshake);
    signed.u32(1);
    signed.bytes_bounded(&frame, 65_535);
    let message = disclosure_message(&keccak256(&[domain, &signed.finish()])).unwrap();
    let (_, signature) = sign_message(&seed, &message).unwrap();
    erebus_core::shielded_auth::verify_message(&issuer, &message, &signature).unwrap();
    let mut encoded = Writer::new();
    encoded.u16(2);
    encoded.fixed(&issuer);
    encoded.fixed(&recipient.public_key());
    encoded.fixed(&evidence.terms.deal_id);
    encoded.u64(expiry);
    encoded.bytes(&handshake);
    encoded.u32(1);
    encoded.bytes_bounded(&frame, 65_535);
    encoded.fixed(&signature);
    let grant = DisclosureGrant::decode(&encoded.finish()).unwrap();
    assert!(matches!(
        grant.open(&recipient, issuer, 100),
        Err(GrantError::Invalid)
    ));
}

struct RetentionFixture {
    directory: tempfile::TempDir,
    agreement: SelectedAgreement,
    mailboxes: [MailboxId; 2],
}

impl RetentionFixture {
    fn participant_store(&self, participant: &str) -> FileTranscriptStore {
        FileTranscriptStore::open(self.directory.path().join(participant)).unwrap()
    }

    fn selected(&self, store: &FileTranscriptStore) -> Result<SelectedAgreement, DisclosureError> {
        let agreement = &self.agreement;
        SelectedAgreement::from_store(
            agreement.terms.clone(),
            agreement.blinding.clone(),
            agreement.buyer.clone(),
            agreement.seller.clone(),
            agreement.transcript_hash_version,
            store,
            BACKUP_NAMESPACE,
        )
    }

    fn expire_relay(&self) -> u64 {
        let relay = FileRelay::open(self.directory.path().join("relay")).unwrap();
        let expired_at = BACKUP_NOW + relay.retention_seconds();
        for mailbox in &self.mailboxes {
            assert!(relay.get(mailbox, 0, expired_at).unwrap().is_empty());
        }
        expired_at
    }
}

/// Fixed descriptor digests isolate backup behavior; authenticated discovery is tested in M2.
fn retention_fixture() -> RetentionFixture {
    let directory = tempfile::tempdir().unwrap();
    let buyer_store = FileTranscriptStore::open(directory.path().join("buyer")).unwrap();
    let seller_store = FileTranscriptStore::open(directory.path().join("seller")).unwrap();
    let relay = FileRelay::open(directory.path().join("relay")).unwrap();
    let buyer_identity = TransportIdentity::generate().unwrap();
    let seller_identity = TransportIdentity::generate().unwrap();
    let prologue = session::prologue(&[1; 32], &[2; 32]);
    let mut buyer = Handshake::initiator(
        &buyer_identity,
        Role::Buyer,
        Role::Seller,
        &prologue,
        seller_identity.public_key(),
    )
    .unwrap();
    let mut seller = Handshake::responder(
        &seller_identity,
        Role::Seller,
        Role::Buyer,
        &prologue,
        buyer_identity.public_key(),
    )
    .unwrap();
    seller.read(&buyer.write().unwrap()).unwrap();
    buyer.read(&seller.write().unwrap()).unwrap();
    seller.read(&buyer.write().unwrap()).unwrap();
    let mut buyer = buyer.finish().unwrap();
    let mut seller = seller.finish().unwrap();
    let mailboxes = [
        MailboxId::for_session(&buyer.session_id(), Role::Buyer),
        MailboxId::for_session(&buyer.session_id(), Role::Seller),
    ];
    let mut agreement = fixture();
    for message in &mut agreement.messages {
        message.session_id = buyer.session_id();
        let (sender, receiver, sender_store, receiver_store, mailbox) = match message.author {
            Role::Buyer => (
                &mut buyer,
                &mut seller,
                &buyer_store,
                &seller_store,
                &mailboxes[0],
            ),
            Role::Seller => (
                &mut seller,
                &mut buyer,
                &seller_store,
                &buyer_store,
                &mailboxes[1],
            ),
        };
        sender_store
            .append(
                BACKUP_NAMESPACE,
                message.deal_id,
                TRANSCRIPT_HASH_VERSION,
                message,
            )
            .unwrap();
        let ciphertext = sender.send_message(message).unwrap();
        let put = relay.put(mailbox, &ciphertext, BACKUP_NOW).unwrap();
        let blobs = relay.get(mailbox, put.cursor - 1, BACKUP_NOW).unwrap();
        assert_eq!(blobs.len(), 1);
        let received = receiver.receive_message(&blobs[0].blob).unwrap();
        receiver_store
            .append(
                BACKUP_NAMESPACE,
                received.deal_id,
                TRANSCRIPT_HASH_VERSION,
                &received,
            )
            .unwrap();
    }
    assert_eq!(
        buyer_store
            .load(
                BACKUP_NAMESPACE,
                agreement.terms.deal_id,
                TRANSCRIPT_HASH_VERSION
            )
            .unwrap()
            .root()
            .unwrap(),
        seller_store
            .load(
                BACKUP_NAMESPACE,
                agreement.terms.deal_id,
                TRANSCRIPT_HASH_VERSION
            )
            .unwrap()
            .root()
            .unwrap()
    );
    verify_selected_agreement(&agreement).unwrap();
    RetentionFixture {
        directory,
        agreement,
        mailboxes,
    }
}

fn fixture() -> SelectedAgreement {
    fixture_with_extra(0)
}

fn fixture_with_extra(extra: usize) -> SelectedAgreement {
    let buyer_key = AuthorizationIdentity::from_bytes(&[1; 32]).unwrap();
    let seller_key = AuthorizationIdentity::from_bytes(&[2; 32]).unwrap();
    let deal_id = [7; 16];
    let mut transcript = Transcript::new(deal_id, TRANSCRIPT_HASH_VERSION).unwrap();
    let mut messages = Vec::new();
    for (role, sequence, body) in [
        (Role::Buyer, 1, b"offer 4".as_slice()),
        (Role::Seller, 1, b"counter 5".as_slice()),
        (Role::Buyer, 2, b"accept 5".as_slice()),
    ] {
        let message = Message::new(
            [9; 32],
            deal_id,
            1,
            role,
            sequence,
            transcript.head(role),
            if sequence == 1 {
                MessageType::Offer
            } else {
                MessageType::Counter
            },
            body.to_vec(),
        )
        .unwrap();
        transcript.append(&message).unwrap();
        messages.push(message);
    }
    for sequence in 3..extra as u64 + 3 {
        let message = Message::new(
            [9; 32],
            deal_id,
            1,
            Role::Buyer,
            sequence,
            transcript.head(Role::Buyer),
            MessageType::Counter,
            vec![0x42; 8192],
        )
        .unwrap();
        transcript.append(&message).unwrap();
        messages.push(message);
    }
    let namespace = ChainNamespace::new("eip155", "10143").unwrap();
    let terms = AgreementTerms {
        protocol_version: CURRENT_PROTOCOL_VERSION,
        suite_id: 1,
        domain: DeploymentDomain {
            namespace: namespace.clone(),
            settlement_contract: Some(AddressBytes::new(vec![0x11; 20]).unwrap()),
            pool: None,
            verifier_version: 1,
        },
        deal_id,
        revision: 1,
        transcript_root: transcript.root().unwrap(),
        buyer_authorization_key: KeyBytes::new(buyer_key.address().to_vec()).unwrap(),
        seller_authorization_key: KeyBytes::new(seller_key.address().to_vec()).unwrap(),
        payment_recipient: KeyBytes::new(seller_key.address().to_vec()).unwrap(),
        asset: AssetId::parse("eip155:10143/erc20:0x00000000000000000000000000000000000000aa")
            .unwrap(),
        amount: BaseUnits::new(5),
        expiry: 1_800_000_000,
        fee_policy: FeePolicy::none(),
        settlement_mode: SettlementMode::PublicBound,
        required_guarantees: GuaranteeSet::from_guarantee(Guarantee::AgreementBoundSettlement),
        settlement_nonce: [0x66; 32],
        service: ServiceRecord {
            resource: "gpu.h100.hour".into(),
            quantity: BaseUnits::new(1),
            unit: "gpu-hour".into(),
            access_recipient: KeyBytes::new(buyer_key.address().to_vec()).unwrap(),
            delivery_deadline: 1_800_000_000,
            fulfillment_method: "http-access".into(),
            fulfillment_digest: [0; 32],
        },
    };
    let blinding = CommitmentBlinding::from_bytes([0x55; 32]);
    let commitment = commit_agreement(&terms, &blinding).unwrap();
    let authorize = |role, key: &AuthorizationIdentity| Authorization {
        role,
        suite_id: terms.suite_id,
        commitment,
        signature: SignatureBytes::new(
            key.sign_digest(
                &authorization_digest(&terms.domain, role, &commitment, terms.suite_id).unwrap(),
            )
            .to_vec(),
        )
        .unwrap(),
    };
    SelectedAgreement {
        buyer: authorize(Role::Buyer, &buyer_key),
        seller: authorize(Role::Seller, &seller_key),
        terms,
        blinding,
        transcript_hash_version: TRANSCRIPT_HASH_VERSION,
        messages,
    }
}

#[test]
fn third_party_reconstructs_one_accepted_agreement_offline() {
    let evidence = fixture();
    let verified = verify_selected_agreement(&evidence).unwrap();
    assert_eq!(verified.deal_id, evidence.terms.deal_id);
    assert_eq!(verified.transcript_root, evidence.terms.transcript_root);
    assert_eq!(verified.commitment, evidence.buyer.commitment);
    assert!(!format!("{evidence:?}").contains("gpu.h100.hour"));
    assert!(!format!("{evidence:?}").contains("offer 4"));
}

#[test]
fn altered_or_missing_messages_and_neighboring_deals_fail() {
    let mut missing = fixture();
    missing.messages.pop();
    assert!(matches!(
        verify_selected_agreement(&missing),
        Err(DisclosureError::TranscriptRoot)
    ));

    let mut altered = fixture();
    altered.messages[0].body = b"offer 6".to_vec();
    assert!(verify_selected_agreement(&altered).is_err());

    let mut neighbor = fixture();
    neighbor.messages[0].deal_id = [8; 16];
    assert!(matches!(
        verify_selected_agreement(&neighbor),
        Err(DisclosureError::Transcript(_))
    ));
}

#[test]
fn changed_terms_or_authorizations_fail() {
    let mut terms = fixture();
    terms.terms.amount = BaseUnits::new(6);
    assert!(verify_selected_agreement(&terms).is_err());

    let mut roles = fixture();
    roles.buyer.role = Role::Seller;
    assert!(matches!(
        verify_selected_agreement(&roles),
        Err(DisclosureError::Roles)
    ));

    let mut signature = fixture();
    signature.seller.signature = SignatureBytes::new(vec![0; 65]).unwrap();
    assert!(verify_selected_agreement(&signature).is_err());
}

#[test]
fn recipient_opens_a_multiframe_grant_from_a_backup() {
    let evidence = fixture_with_extra(10);
    let issuer = AuthorizationIdentity::from_bytes(&[2; 32]).unwrap();
    let recipient = DisclosureIdentity::generate().unwrap();
    let grant =
        DisclosureGrant::seal(&evidence, &issuer, recipient.public_key(), 1000, 100).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("deal.grant");
    grant.write_backup(&path).unwrap();
    let restored = DisclosureGrant::load_backup(&path).unwrap();
    let (opened, verified) = restored.open(&recipient, issuer.address(), 999).unwrap();
    assert_eq!(opened.messages.len(), 13);
    assert_eq!(verified.transcript_root, evidence.terms.transcript_root);
    assert!(grant.write_backup(&path).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn grant_rejects_wrong_recipient_issuer_expiry_and_mutation() {
    let issuer = AuthorizationIdentity::from_bytes(&[2; 32]).unwrap();
    let recipient = DisclosureIdentity::generate().unwrap();
    let other = DisclosureIdentity::generate().unwrap();
    let grant =
        DisclosureGrant::seal(&fixture(), &issuer, recipient.public_key(), 1000, 100).unwrap();
    assert!(matches!(
        grant.open(&other, issuer.address(), 100),
        Err(GrantError::WrongRecipient)
    ));
    assert!(matches!(
        grant.open(&recipient, [0; 20], 100),
        Err(GrantError::Invalid)
    ));
    assert!(matches!(
        grant.open(&recipient, issuer.address(), 1000),
        Err(GrantError::Expired)
    ));
    let mut encoded = grant.encode().unwrap();
    let last_ciphertext = encoded.len() - 66;
    encoded[last_ciphertext] ^= 1;
    let altered = DisclosureGrant::decode(&encoded).unwrap();
    assert!(matches!(
        altered.open(&recipient, issuer.address(), 100),
        Err(GrantError::Invalid)
    ));
    let mut trailing = grant.encode().unwrap();
    trailing.push(0);
    assert!(DisclosureGrant::decode(&trailing).is_err());
}

#[test]
fn saved_grant_opens_after_participant_transcripts_and_relay_data_are_gone() {
    let setup = retention_fixture();
    let issuer = AuthorizationIdentity::from_bytes(&[2; 32]).unwrap();
    let key_path = setup.directory.path().join("auditor.key");
    let backup = setup.directory.path().join("saved.grant");
    let expected = {
        let recipient = DisclosureIdentity::generate_and_store(&key_path).unwrap();
        let evidence = setup.selected(&setup.participant_store("seller")).unwrap();
        let expected = verify_selected_agreement(&evidence).unwrap();
        let relay = FileRelay::open(setup.directory.path().join("relay")).unwrap();
        let grant = DisclosureGrant::seal(
            &evidence,
            &issuer,
            recipient.public_key(),
            BACKUP_NOW + 2 * relay.retention_seconds(),
            BACKUP_NOW,
        )
        .unwrap();
        grant.write_backup(&backup).unwrap();
        expected
    };
    let expired_at = setup.expire_relay();
    std::fs::remove_dir_all(setup.directory.path().join("buyer")).unwrap();
    std::fs::remove_dir_all(setup.directory.path().join("seller")).unwrap();
    assert!(matches!(
        setup.selected(&setup.participant_store("buyer")),
        Err(DisclosureError::TranscriptUnavailable)
    ));
    let restored_key = DisclosureIdentity::load(&key_path).unwrap();
    let restored = DisclosureGrant::load_backup(&backup).unwrap();
    let (opened, verified) = restored
        .open(&restored_key, issuer.address(), expired_at)
        .unwrap();
    assert_eq!(verified, expected);
    assert_eq!(opened.messages.len(), 3);
    assert_eq!(opened.terms.deal_id, setup.agreement.terms.deal_id);
}

#[test]
fn missing_grant_is_reissued_from_reopened_participant_storage_after_relay_expiry() {
    let setup = retention_fixture();
    let issuer = AuthorizationIdentity::from_bytes(&[2; 32]).unwrap();
    let recipient = DisclosureIdentity::generate().unwrap();
    let relay = FileRelay::open(setup.directory.path().join("relay")).unwrap();
    let expiry = BACKUP_NOW + 2 * relay.retention_seconds();
    let selected = setup.selected(&setup.participant_store("seller")).unwrap();
    let expected = verify_selected_agreement(&selected).unwrap();
    let backup = setup.directory.path().join("lost.grant");
    DisclosureGrant::seal(
        &selected,
        &issuer,
        recipient.public_key(),
        expiry,
        BACKUP_NOW,
    )
    .unwrap()
    .write_backup(&backup)
    .unwrap();
    drop(selected);
    std::fs::remove_file(&backup).unwrap();
    assert!(matches!(
        DisclosureGrant::load_backup(&backup),
        Err(GrantError::Backup)
    ));
    let expired_at = setup.expire_relay();

    // Reissue needs the retained opening and authorizations as well as the transcript.
    // This is an SDK test, not an installed multi-process agreement-state backup workflow.
    let reopened = setup.participant_store("seller");
    let recovered = setup.selected(&reopened).unwrap();
    assert_eq!(verify_selected_agreement(&recovered).unwrap(), expected);
    let reissued = DisclosureGrant::seal(
        &recovered,
        &issuer,
        recipient.public_key(),
        expiry,
        expired_at,
    )
    .unwrap();
    reissued.write_backup(&backup).unwrap();
    let restored = DisclosureGrant::load_backup(&backup).unwrap();
    let (_, verified) = restored
        .open(&recipient, issuer.address(), expired_at)
        .unwrap();
    assert_eq!(verified, expected);
}

#[test]
fn stored_export_rejects_wrong_namespace_hash_version_and_signed_terms() {
    let setup = retention_fixture();
    let store = setup.participant_store("buyer");
    let agreement = &setup.agreement;
    assert!(matches!(
        SelectedAgreement::from_store(
            agreement.terms.clone(),
            agreement.blinding.clone(),
            agreement.buyer.clone(),
            agreement.seller.clone(),
            TRANSCRIPT_HASH_VERSION,
            &store,
            "another.namespace",
        ),
        Err(DisclosureError::TranscriptUnavailable)
    ));
    assert!(SelectedAgreement::from_store(
        agreement.terms.clone(),
        agreement.blinding.clone(),
        agreement.buyer.clone(),
        agreement.seller.clone(),
        TRANSCRIPT_HASH_VERSION + 1,
        &store,
        BACKUP_NAMESPACE,
    )
    .is_err());
    let mut changed = agreement.terms.clone();
    changed.amount = BaseUnits::new(changed.amount.get() + 1);
    assert!(SelectedAgreement::from_store(
        changed,
        agreement.blinding.clone(),
        agreement.buyer.clone(),
        agreement.seller.clone(),
        TRANSCRIPT_HASH_VERSION,
        &store,
        BACKUP_NAMESPACE,
    )
    .is_err());
}

#[test]
fn missing_grant_and_missing_participant_transcript_fail_without_creating_a_replacement() {
    let setup = retention_fixture();
    let backup = setup.directory.path().join("never-exported.grant");
    let expired_at = setup.expire_relay();
    assert!(expired_at > BACKUP_NOW);
    std::fs::remove_dir_all(setup.directory.path().join("buyer")).unwrap();
    std::fs::remove_dir_all(setup.directory.path().join("seller")).unwrap();
    assert!(matches!(
        DisclosureGrant::load_backup(&backup),
        Err(GrantError::Backup)
    ));
    for participant in ["buyer", "seller"] {
        assert!(matches!(
            setup.selected(&setup.participant_store(participant)),
            Err(DisclosureError::TranscriptUnavailable)
        ));
    }
    assert!(
        !backup.exists(),
        "missing evidence must not create a replacement grant"
    );
}
