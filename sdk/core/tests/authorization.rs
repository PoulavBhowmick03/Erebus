//! Authorization binding: replay, role substitution, expiry, and mutation.
//!
//! These are the cases the M1 done criteria name: service-record mutations invalidate
//! authorization, and cross-domain replay fails. They exercise the public API exactly as a
//! coordinator would, with deterministic keys and no chain.

use erebus_core::auth::{
    authorization_digest, verify_authorization, verify_authorization_signature, AuthError,
    Authorization, Role,
};
use erebus_core::commitment::{commit_agreement, CommitmentBlinding};
use erebus_core::domain::DeploymentDomain;
use erebus_core::ids::{
    AddressBytes, AssetId, BaseUnits, ChainNamespace, KeyBytes, SignatureBytes,
};
use erebus_core::service::ServiceRecord;
use erebus_core::suite::{suite, SuiteError, EVM_SECP256K1_KECCAK_SUITE_ID};
use erebus_core::terms::{
    AgreementTerms, FeePolicy, Guarantee, GuaranteeSet, SettlementMode, TermsError,
    CURRENT_PROTOCOL_VERSION,
};
use k256::ecdsa::SigningKey;
use sha3::{Digest, Keccak256};

fn signing_key(seed: u8) -> SigningKey {
    SigningKey::from_slice(&[seed; 32]).expect("deterministic key")
}

fn address_of(key: &SigningKey) -> Vec<u8> {
    let point = key.verifying_key().to_encoded_point(false);
    let hash = Keccak256::digest(&point.as_bytes()[1..]);
    hash[12..].to_vec()
}

fn terms_with(buyer: &SigningKey, seller: &SigningKey) -> AgreementTerms {
    AgreementTerms {
        protocol_version: CURRENT_PROTOCOL_VERSION,
        suite_id: EVM_SECP256K1_KECCAK_SUITE_ID,
        domain: DeploymentDomain {
            namespace: ChainNamespace::new("eip155", "10143").expect("valid namespace"),
            settlement_contract: Some(AddressBytes::new(vec![0x11; 20]).expect("valid address")),
            pool: None,
            verifier_version: 1,
        },
        deal_id: [0x07; 16],
        revision: 1,
        transcript_root: [0x33; 32],
        buyer_authorization_key: KeyBytes::new(address_of(buyer)).expect("valid key"),
        seller_authorization_key: KeyBytes::new(address_of(seller)).expect("valid key"),
        payment_recipient: KeyBytes::new(address_of(seller)).expect("valid key"),
        asset: AssetId::parse("eip155:10143/erc20:0x00000000000000000000000000000000000000aa")
            .expect("valid asset"),
        amount: BaseUnits::new(1_000_000),
        expiry: 1_800_000_000,
        fee_policy: FeePolicy::none(),
        settlement_mode: SettlementMode::PublicBound,
        required_guarantees: GuaranteeSet::from_guarantee(Guarantee::AgreementBoundSettlement),
        settlement_nonce: [0x66; 32],
        service: ServiceRecord {
            resource: "gpu.h100.hour".to_owned(),
            quantity: BaseUnits::new(500),
            unit: "gpu-hour".to_owned(),
            access_recipient: KeyBytes::new(address_of(buyer)).expect("valid key"),
            delivery_deadline: 1_800_000_000,
            fulfillment_method: "http-access".to_owned(),
            fulfillment_digest: [0u8; 32],
        },
    }
}

fn authorize(
    terms: &AgreementTerms,
    commitment: &erebus_core::commitment::DealCommitment,
    role: Role,
    key: &SigningKey,
) -> Authorization {
    let digest =
        authorization_digest(&terms.domain, role, commitment, terms.suite_id).expect("digest");
    let (signature, recovery) = key
        .sign_prehash_recoverable(&digest)
        .expect("deterministic signing");
    let mut bytes = signature.to_bytes().to_vec();
    bytes.push(u8::from(recovery));
    Authorization {
        role,
        suite_id: terms.suite_id,
        commitment: *commitment,
        signature: SignatureBytes::new(bytes).expect("valid signature"),
    }
}

#[test]
fn mutual_authorizations_verify_before_expiry() {
    let buyer = signing_key(0x01);
    let seller = signing_key(0x02);
    let terms = terms_with(&buyer, &seller);
    let commitment =
        commit_agreement(&terms, &CommitmentBlinding::from_bytes([0x0a; 32])).expect("commits");

    for (role, key) in [(Role::Buyer, &buyer), (Role::Seller, &seller)] {
        let authorization = authorize(&terms, &commitment, role, key);
        verify_authorization(
            &terms,
            &commitment,
            &CommitmentBlinding::from_bytes([0x0a; 32]),
            &authorization,
            terms.expiry - 1,
        )
        .unwrap_or_else(|error| panic!("{role:?}: {error}"));
    }
}

#[test]
fn expiry_is_enforced_at_and_after_the_deadline() {
    let buyer = signing_key(0x01);
    let seller = signing_key(0x02);
    let terms = terms_with(&buyer, &seller);
    let commitment =
        commit_agreement(&terms, &CommitmentBlinding::from_bytes([0x0a; 32])).expect("commits");
    let authorization = authorize(&terms, &commitment, Role::Buyer, &buyer);

    assert_eq!(
        verify_authorization(
            &terms,
            &commitment,
            &CommitmentBlinding::from_bytes([0x0a; 32]),
            &authorization,
            terms.expiry
        ),
        Err(AuthError::Expired {
            expiry: terms.expiry,
            now: terms.expiry,
        })
    );
    assert_eq!(
        verify_authorization(
            &terms,
            &commitment,
            &CommitmentBlinding::from_bytes([0x0a; 32]),
            &authorization,
            terms.expiry + 1
        ),
        Err(AuthError::Expired {
            expiry: terms.expiry,
            now: terms.expiry + 1,
        })
    );
    // Expiry does not erase the evidence; disclosure verification ignores it.
    verify_authorization_signature(
        &terms,
        &commitment,
        &CommitmentBlinding::from_bytes([0x0a; 32]),
        &authorization,
    )
    .expect("an expired authorization is still evidence it was issued");
}

#[test]
fn cross_domain_replay_fails() {
    let buyer = signing_key(0x01);
    let seller = signing_key(0x02);
    let terms = terms_with(&buyer, &seller);
    let mut other_domain = terms.clone();
    other_domain.domain.verifier_version = 2;

    let commitment =
        commit_agreement(&terms, &CommitmentBlinding::from_bytes([0x0a; 32])).expect("commits");
    let other_commitment =
        commit_agreement(&other_domain, &CommitmentBlinding::from_bytes([0x0a; 32]))
            .expect("commits");
    let authorization = authorize(&terms, &commitment, Role::Buyer, &buyer);

    assert_eq!(
        verify_authorization_signature(
            &other_domain,
            &other_commitment,
            &CommitmentBlinding::from_bytes([0x0a; 32]),
            &authorization
        ),
        Err(AuthError::CommitmentMismatch)
    );

    // The digest itself differs by domain, so the same signature cannot verify under it.
    let digest = authorization_digest(&terms.domain, Role::Buyer, &commitment, terms.suite_id)
        .expect("digest");
    let other_digest = authorization_digest(
        &other_domain.domain,
        Role::Buyer,
        &other_commitment,
        terms.suite_id,
    )
    .expect("digest");
    assert_ne!(digest, other_digest);
    assert_eq!(
        suite(terms.suite_id)
            .expect("suite 1")
            .verify_authorization(
                terms.buyer_authorization_key.as_bytes(),
                &other_digest,
                authorization.signature.as_bytes(),
            ),
        Err(SuiteError::SignerMismatch)
    );
}

#[test]
fn role_substitution_fails() {
    let buyer = signing_key(0x01);
    let seller = signing_key(0x02);
    let terms = terms_with(&buyer, &seller);
    let commitment =
        commit_agreement(&terms, &CommitmentBlinding::from_bytes([0x0a; 32])).expect("commits");
    let buyer_authorization = authorize(&terms, &commitment, Role::Buyer, &buyer);
    let as_seller = Authorization {
        role: Role::Seller,
        ..buyer_authorization
    };

    assert_eq!(
        verify_authorization_signature(
            &terms,
            &commitment,
            &CommitmentBlinding::from_bytes([0x0a; 32]),
            &as_seller
        ),
        Err(AuthError::Suite(SuiteError::SignerMismatch))
    );
}

#[test]
fn revision_replay_fails() {
    let buyer = signing_key(0x01);
    let seller = signing_key(0x02);
    let terms = terms_with(&buyer, &seller);
    let mut counteroffer = terms.clone();
    counteroffer.revision = 2;
    counteroffer.amount = BaseUnits::new(900_000);

    let first_commitment =
        commit_agreement(&terms, &CommitmentBlinding::from_bytes([0x0a; 32])).expect("commits");
    let second_commitment =
        commit_agreement(&counteroffer, &CommitmentBlinding::from_bytes([0x0b; 32]))
            .expect("commits");
    let authorization = authorize(&terms, &first_commitment, Role::Buyer, &buyer);

    assert_eq!(
        verify_authorization_signature(
            &counteroffer,
            &second_commitment,
            &CommitmentBlinding::from_bytes([0x0b; 32]),
            &authorization
        ),
        Err(AuthError::CommitmentMismatch)
    );
}

#[test]
fn service_record_mutation_invalidates_authorization() {
    let buyer = signing_key(0x01);
    let seller = signing_key(0x02);
    let terms = terms_with(&buyer, &seller);
    let commitment =
        commit_agreement(&terms, &CommitmentBlinding::from_bytes([0x0a; 32])).expect("commits");
    let authorization = authorize(&terms, &commitment, Role::Seller, &seller);

    let mut mutated = terms.clone();
    mutated.service.resource = "gpu.a100.hour".to_owned();
    let mutated_commitment =
        commit_agreement(&mutated, &CommitmentBlinding::from_bytes([0x0a; 32])).expect("commits");
    assert_ne!(commitment, mutated_commitment);
    assert_eq!(
        verify_authorization_signature(
            &mutated,
            &mutated_commitment,
            &CommitmentBlinding::from_bytes([0x0a; 32]),
            &authorization
        ),
        Err(AuthError::CommitmentMismatch)
    );

    let mut mutated = terms.clone();
    mutated.service.access_recipient = KeyBytes::new(vec![0xee; 20]).expect("valid key");
    let mutated_commitment =
        commit_agreement(&mutated, &CommitmentBlinding::from_bytes([0x0a; 32])).expect("commits");
    assert_eq!(
        verify_authorization_signature(
            &mutated,
            &mutated_commitment,
            &CommitmentBlinding::from_bytes([0x0a; 32]),
            &authorization
        ),
        Err(AuthError::CommitmentMismatch)
    );
}

#[test]
fn a_flipped_signature_bit_fails() {
    let buyer = signing_key(0x01);
    let seller = signing_key(0x02);
    let terms = terms_with(&buyer, &seller);
    let commitment =
        commit_agreement(&terms, &CommitmentBlinding::from_bytes([0x0a; 32])).expect("commits");
    let authorization = authorize(&terms, &commitment, Role::Buyer, &buyer);

    let mut tampered = authorization.signature.as_bytes().to_vec();
    tampered[0] ^= 0x01;
    let tampered = Authorization {
        signature: SignatureBytes::new(tampered).expect("bounds"),
        ..authorization
    };
    assert!(matches!(
        verify_authorization_signature(
            &terms,
            &commitment,
            &CommitmentBlinding::from_bytes([0x0a; 32]),
            &tampered
        ),
        Err(AuthError::Suite(
            SuiteError::InvalidSignature | SuiteError::SignerMismatch
        ))
    ));
}

/// The EIP-155 example signature, rewritten with `s' = n - s`.
///
/// A high-`s` encoding is malleable; the suite rejects it before recovery.
#[test]
fn high_s_signatures_are_rejected() {
    let digest: [u8; 32] =
        hex::decode("daf5a779ae972f972197303d7b574746c7ef83eadac0f2791ad23db92e4c8e53")
            .expect("hex")
            .try_into()
            .expect("32 bytes");
    let mut signature = hex::decode(concat!(
        "28ef61340bd939bc2195fe537567866003e1a15d3c71ff63e1590620aa636276",
        "98341627668089e51348fccfb4c7ff31c55912f2d2e47ef09652acf665fad3be",
    ))
    .expect("hex");
    signature.push(0);
    let address = hex::decode("9d8a62f656a8d1615c1294fd71e9cfb3e4855a4f").expect("hex");
    assert_eq!(
        suite(EVM_SECP256K1_KECCAK_SUITE_ID)
            .expect("suite 1")
            .verify_authorization(&address, &digest, &signature),
        Err(SuiteError::NonCanonicalSignature)
    );
}

#[test]
fn authorizations_round_trip_through_their_encoding() {
    let buyer = signing_key(0x01);
    let seller = signing_key(0x02);
    let terms = terms_with(&buyer, &seller);
    let commitment =
        commit_agreement(&terms, &CommitmentBlinding::from_bytes([0x0a; 32])).expect("commits");
    let authorization = authorize(&terms, &commitment, Role::Seller, &seller);
    let bytes = authorization.encode().expect("encodes");
    assert_eq!(
        Authorization::decode(&bytes).expect("decodes"),
        authorization
    );
}

#[test]
fn keys_must_match_the_suite_length_at_validation() {
    let buyer = signing_key(0x01);
    let seller = signing_key(0x02);
    let mut terms = terms_with(&buyer, &seller);
    terms.buyer_authorization_key = KeyBytes::new(vec![0x01; 19]).expect("bounds allow 19");
    assert_eq!(
        terms.validate(),
        Err(TermsError::AuthorizationKeyLength {
            role: "buyer",
            expected: 20,
            actual: 19,
        })
    );
}

#[test]
fn original_commitment_cannot_authorize_changed_terms() {
    let buyer = signing_key(1);
    let seller = signing_key(2);
    let terms = terms_with(&buyer, &seller);
    let blinding = CommitmentBlinding::from_bytes([10; 32]);
    let commitment = commit_agreement(&terms, &blinding).unwrap();
    let mutations: [fn(&mut AgreementTerms); 15] = [
        |t| t.amount = BaseUnits::new(999_999_999),
        |t| t.expiry += 1000,
        |t| t.service.resource = "changed-resource".into(),
        |t| t.service.quantity = BaseUnits::new(1),
        |t| t.service.unit = "second".into(),
        |t| t.service.delivery_deadline += 1,
        |t| t.service.fulfillment_method = "changed-method".into(),
        |t| t.service.fulfillment_digest[0] ^= 1,
        |t| t.service.access_recipient = KeyBytes::new(vec![99; 20]).unwrap(),
        |t| t.payment_recipient = KeyBytes::new(vec![98; 20]).unwrap(),
        |t| {
            t.fee_policy = FeePolicy {
                fee: BaseUnits::new(1),
                recipient: Some(t.payment_recipient.clone()),
            }
        },
        |t| t.required_guarantees.insert(Guarantee::ScopedDisclosure),
        |t| t.revision += 1,
        |t| t.settlement_nonce[0] ^= 1,
        |t| t.asset = AssetId::parse("eip155:10143/erc20:0xbb").unwrap(),
    ];
    for (role, key) in [(Role::Buyer, &buyer), (Role::Seller, &seller)] {
        let authorization = authorize(&terms, &commitment, role, key);
        for (index, mutate) in mutations.iter().enumerate() {
            let mut changed = terms.clone();
            mutate(&mut changed);
            assert_eq!(
                verify_authorization_signature(&changed, &commitment, &blinding, &authorization),
                Err(AuthError::OpeningMismatch),
                "mutation {index}, {role:?}"
            );
            assert_eq!(
                verify_authorization(
                    &changed,
                    &commitment,
                    &blinding,
                    &authorization,
                    terms.expiry + 1
                ),
                Err(AuthError::OpeningMismatch),
                "mutation {index}, {role:?}"
            );
        }
    }
}

#[test]
fn wrong_blinding_fails_at_audit_and_settlement() {
    let buyer = signing_key(1);
    let terms = terms_with(&buyer, &signing_key(2));
    let commitment = commit_agreement(&terms, &CommitmentBlinding::from_bytes([10; 32])).unwrap();
    let authorization = authorize(&terms, &commitment, Role::Buyer, &buyer);
    let wrong = CommitmentBlinding::from_bytes([11; 32]);
    assert_eq!(
        verify_authorization_signature(&terms, &commitment, &wrong, &authorization),
        Err(AuthError::OpeningMismatch)
    );
    assert_eq!(
        verify_authorization(&terms, &commitment, &wrong, &authorization, 1),
        Err(AuthError::OpeningMismatch)
    );
}
