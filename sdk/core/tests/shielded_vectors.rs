//! The independent M4 Circom/JS runner pins these suite-2 agreement hashes.

use erebus_core::auth::{Authorization, Role};
use erebus_core::commitment::CommitmentBlinding;
use erebus_core::domain::DeploymentDomain;
use erebus_core::ids::{
    AddressBytes, AssetId, BaseUnits, ChainNamespace, KeyBytes, SignatureBytes,
};
use erebus_core::service::ServiceRecord;
use erebus_core::shielded::ShieldedDeal;
use erebus_core::shielded_auth::{derive_key, sign_message, verify_agreement, verify_message};
use erebus_core::terms::{AgreementTerms, FeePolicy, Guarantee, GuaranteeSet, SettlementMode};
use serde_json::Value;

const FIXTURE: &str = include_str!("fixtures/agreement-suite2-vector.json");

#[test]
fn access_signatures_preserve_digest_limbs_and_cannot_authorize_payment_or_disclosure() {
    use erebus_core::shielded::{access_message, disclosure_message};
    let mut high = [0; 32];
    high[0] = 1;
    let mut low = [0; 32];
    low[31] = 1;
    assert_ne!(
        access_message(&high).unwrap(),
        access_message(&low).unwrap()
    );
    let (terms, blinding, _) = fixture_terms();
    let deal = ShieldedDeal::from_terms(&terms).unwrap();
    let commitment = deal.commitment(&blinding).unwrap();
    let digest = commitment.as_bytes();
    let access = access_message(digest).unwrap();
    let (key, signature) = sign_message(&[1; 32], &access).unwrap();
    verify_message(&key, &access, &signature).unwrap();
    let disclosure = disclosure_message(digest).unwrap();
    assert_ne!(access, disclosure);
    assert!(verify_message(&key, &disclosure, &signature).is_err());
    for role in [Role::Buyer, Role::Seller] {
        let payment = deal.authorization_message(role, &commitment).unwrap();
        assert_ne!(access, payment);
        assert!(verify_message(&key, &payment, &signature).is_err());
    }
}

#[test]
fn disclosure_messages_match_circomlib_and_cannot_authorize_payment() {
    use erebus_core::shielded::disclosure_message;
    // circomlibjs 0.1.7: Poseidon([3001, high128(digest), low128(digest)]).
    for (digest, expected) in [
        (
            [0; 32],
            "2920fd34a3f1366d8fdf72b216134c0fba4ea675cdbc96ef21158929d772fe98",
        ),
        (
            [0xff; 32],
            "00ec53d46422bcccb62bf09b2cd3e4cdcd719dfa125c3f6e5102c4ca12b8b3ab",
        ),
    ] {
        assert_eq!(hex::encode(disclosure_message(&digest).unwrap()), expected);
    }
    let mut high = [0; 32];
    high[0] = 1;
    let mut low = [0; 32];
    low[31] = 1;
    assert_ne!(
        disclosure_message(&high).unwrap(),
        disclosure_message(&low).unwrap()
    );
    let (terms, blinding, _) = fixture_terms();
    let deal = ShieldedDeal::from_terms(&terms).unwrap();
    let commitment = deal.commitment(&blinding).unwrap();
    let message = disclosure_message(commitment.as_bytes()).unwrap();
    let seed = [1; 32];
    let (key, signature) = sign_message(&seed, &message).unwrap();
    verify_message(&key, &message, &signature).unwrap();
    for role in [Role::Buyer, Role::Seller] {
        let payment = deal.authorization_message(role, &commitment).unwrap();
        assert_ne!(message, payment);
        assert!(verify_message(&key, &payment, &signature).is_err());
    }
}

fn bytes(value: &str) -> Vec<u8> {
    hex::decode(value).expect("pinned hex")
}

fn fixed<const N: usize>(value: &str) -> [u8; N] {
    bytes(value).try_into().expect("pinned width")
}

fn field<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().expect("pinned string")
}

fn fixture_terms() -> (AgreementTerms, CommitmentBlinding, Value) {
    let vector: Value = serde_json::from_str(FIXTURE).expect("pinned JSON");
    let input = &vector["terms"];
    let domain = &input["domain"];
    let service = &input["service"];
    let mut guarantees = GuaranteeSet::empty();
    for guarantee in [
        Guarantee::HiddenAmount,
        Guarantee::HiddenRecipient,
        Guarantee::AgreementBoundSettlement,
    ] {
        guarantees.insert(guarantee);
    }
    let terms = AgreementTerms {
        protocol_version: input["protocolVersion"].as_u64().expect("version") as u16,
        suite_id: input["suiteId"].as_u64().expect("suite") as u16,
        domain: DeploymentDomain {
            namespace: ChainNamespace::parse(field(domain, "namespace")).expect("namespace"),
            settlement_contract: Some(
                AddressBytes::new(bytes(field(domain, "settlementContractHex"))).expect("contract"),
            ),
            pool: Some(AddressBytes::new(bytes(field(domain, "poolHex"))).expect("pool")),
            verifier_version: domain["verifierVersion"].as_u64().expect("version") as u32,
        },
        deal_id: fixed(field(input, "dealIdHex")),
        revision: input["revision"].as_u64().expect("revision") as u32,
        transcript_root: fixed(field(input, "transcriptRootHex")),
        buyer_authorization_key: KeyBytes::new(bytes(field(input, "buyerAuthorizationKeyHex")))
            .expect("buyer"),
        seller_authorization_key: KeyBytes::new(bytes(field(input, "sellerAuthorizationKeyHex")))
            .expect("seller"),
        payment_recipient: KeyBytes::new(bytes(field(input, "paymentRecipientHex")))
            .expect("recipient"),
        asset: AssetId::parse(field(input, "asset")).expect("asset"),
        amount: BaseUnits::new(field(input, "amount").parse().expect("amount")),
        expiry: input["expiry"].as_u64().expect("expiry"),
        fee_policy: FeePolicy::none(),
        settlement_mode: SettlementMode::Shielded,
        required_guarantees: guarantees,
        settlement_nonce: fixed(field(input, "settlementNonceHex")),
        service: ServiceRecord {
            resource: field(service, "resource").to_owned(),
            quantity: BaseUnits::new(field(service, "quantity").parse().expect("quantity")),
            unit: field(service, "unit").to_owned(),
            access_recipient: KeyBytes::new(bytes(field(service, "accessRecipientHex")))
                .expect("access key"),
            delivery_deadline: service["deliveryDeadline"].as_u64().expect("deadline"),
            fulfillment_method: field(service, "fulfillmentMethod").to_owned(),
            fulfillment_digest: fixed(field(service, "fulfillmentDigestHex")),
        },
    };
    let blinding = CommitmentBlinding::from_bytes(fixed(field(&vector, "blindingHex")));
    (terms, blinding, vector)
}

#[test]
fn rust_matches_circomlib_commitment_and_messages() {
    let (terms, blinding, vector) = fixture_terms();
    let deal = ShieldedDeal::from_terms(&terms).expect("fixed suite-2 shape");
    let commitment = deal.commitment(&blinding).expect("Poseidon commitment");
    let expected = &vector["expected"];
    assert_eq!(commitment.to_hex(), field(expected, "commitmentHex"));
    assert_eq!(
        deal.deal_nullifier().expect("nullifier").to_hex(),
        field(expected, "dealNullifierHex")
    );
    assert_eq!(
        hex::encode(
            deal.authorization_message(Role::Buyer, &commitment)
                .expect("buyer")
        ),
        field(expected, "buyerMessageHex")
    );
    assert_eq!(
        hex::encode(
            deal.authorization_message(Role::Seller, &commitment)
                .expect("seller")
        ),
        field(expected, "sellerMessageHex")
    );
    assert_eq!(
        hex::encode(deal.payment_note_commitment().expect("payment note")),
        field(expected, "paymentCommitmentHex")
    );
}

#[test]
fn shared_agreement_api_matches_the_shielded_circuit() {
    use erebus_core::auth::{
        authorization_digest, verify_authorization, verify_authorization_signature,
    };
    use erebus_core::commitment::{commit_agreement, deal_nullifier};
    let (terms, blinding, vector) = fixture_terms();
    let encoded = terms.encode().expect("suite-2 terms encode");
    assert_eq!(AgreementTerms::decode(&encoded).unwrap(), terms);
    use erebus_core::settlement::{check_capabilities, BackendCapabilities, SettlementContext};
    let capabilities = BackendCapabilities {
        suites: [2].into_iter().collect(),
        modes: [SettlementMode::Shielded].into_iter().collect(),
        guarantees: terms.required_guarantees,
        local_proving: true,
    };
    let context = SettlementContext {
        require_local_proving: true,
        mode: terms.settlement_mode,
        domain: terms.domain.clone(),
        suite_id: 2,
        asset: terms.asset.clone(),
        required_guarantees: terms.required_guarantees,
    };
    check_capabilities(&context, &capabilities).expect("shielded selection");
    let mut invalid_context = context.clone();
    invalid_context.domain.pool = None;
    assert!(check_capabilities(&invalid_context, &capabilities).is_err());
    let commitment = commit_agreement(&terms, &blinding).expect("shared commitment");
    assert_eq!(
        commitment.to_hex(),
        field(&vector["expected"], "commitmentHex")
    );
    assert_eq!(
        deal_nullifier(&terms).unwrap().to_hex(),
        field(&vector["expected"], "dealNullifierHex")
    );
    for (role, name) in [(Role::Buyer, "buyer"), (Role::Seller, "seller")] {
        let message = authorization_digest(&terms.domain, role, &commitment, 2).unwrap();
        assert_eq!(
            hex::encode(message),
            field(&vector["expected"], &format!("{name}MessageHex"))
        );
        let auth = Authorization {
            role,
            suite_id: 2,
            commitment,
            signature: SignatureBytes::new(bytes(field(
                &vector["expected"],
                &format!("{name}SignatureHex"),
            )))
            .unwrap(),
        };
        verify_authorization(&terms, &commitment, &blinding, &auth, terms.expiry - 1).unwrap();
        assert!(verify_authorization(&terms, &commitment, &blinding, &auth, terms.expiry).is_err());
        verify_authorization_signature(&terms, &commitment, &blinding, &auth).unwrap();
        let mut changed = terms.clone();
        changed.transcript_root[31] ^= 1;
        assert!(verify_authorization_signature(&changed, &commitment, &blinding, &auth).is_err());
        let mut changed = terms.clone();
        changed.domain.verifier_version += 1;
        assert!(verify_authorization_signature(&changed, &commitment, &blinding, &auth).is_err());
    }
    let mut invalid = terms;
    invalid.domain.verifier_version = 0;
    assert!(invalid.validate().is_err());
}

#[test]
fn shielded_shape_and_opening_reject_mutations() {
    let (terms, blinding, _) = fixture_terms();
    let original = ShieldedDeal::from_terms(&terms).expect("shape");
    let original_commitment = original.commitment(&blinding).expect("commitment");
    let mut changed = terms.clone();
    changed.amount = BaseUnits::new(71);
    let revised = ShieldedDeal::from_terms(&changed).expect("shape");
    assert_ne!(
        revised.commitment(&blinding).expect("commitment"),
        original_commitment
    );
    assert_ne!(
        revised.payment_note_commitment().expect("other payment"),
        original.payment_note_commitment().expect("payment")
    );
    assert_eq!(
        revised.deal_nullifier().expect("nullifier"),
        original.deal_nullifier().expect("nullifier")
    );

    let mut changed = terms.clone();
    changed.fee_policy.fee = BaseUnits::new(1);
    assert!(ShieldedDeal::from_terms(&changed).is_err());
    let mut changed = terms.clone();
    changed.payment_recipient = KeyBytes::new(vec![1; 20]).expect("key");
    assert!(ShieldedDeal::from_terms(&changed).is_err());
    let mut changed = terms;
    changed.domain.verifier_version += 1;
    assert_ne!(
        ShieldedDeal::from_terms(&changed)
            .expect("shape")
            .commitment(&blinding)
            .expect("commitment"),
        original_commitment
    );
    assert_ne!(
        ShieldedDeal::from_terms(&changed)
            .expect("shape")
            .payment_note_commitment()
            .expect("other payment"),
        original.payment_note_commitment().expect("payment")
    );
    assert!(original
        .commitment(&CommitmentBlinding::from_bytes([0; 32]))
        .is_err());
}

#[test]
fn rust_signatures_match_circomlib_for_both_roles() {
    let (terms, blinding, vector) = fixture_terms();
    let deal = ShieldedDeal::from_terms(&terms).expect("shape");
    let commitment = deal.commitment(&blinding).expect("commitment");
    for (role, label) in [(Role::Buyer, "buyer"), (Role::Seller, "seller")] {
        let seed = fixed(
            vector["testOnlySeeds"][format!("{label}Hex")]
                .as_str()
                .expect("seed"),
        );
        let key = derive_key(&seed).expect("derive key");
        let agreed_key = match role {
            Role::Buyer => terms.buyer_authorization_key.as_bytes(),
            Role::Seller => terms.seller_authorization_key.as_bytes(),
        };
        assert_eq!(key.as_slice(), agreed_key);
        let message = deal
            .authorization_message(role, &commitment)
            .expect("message");
        let (signed_key, signature) = sign_message(&seed, &message).expect("sign");
        assert_eq!(signed_key, key);
        assert_eq!(
            hex::encode(signature),
            field(&vector["expected"], &format!("{label}SignatureHex"))
        );
        verify_message(&key, &message, &signature).expect("matching signature");
        let wrong_role = if role == Role::Buyer {
            Role::Seller
        } else {
            Role::Buyer
        };
        let other_message = deal
            .authorization_message(wrong_role, &commitment)
            .expect("message");
        assert!(verify_message(&key, &other_message, &signature).is_err());
        let mut changed = signature;
        changed[95] ^= 1;
        assert!(verify_message(&key, &message, &changed).is_err());
    }
}

#[test]
fn shielded_agreement_requires_both_roles_and_the_exact_opening() {
    let (terms, blinding, vector) = fixture_terms();
    let commitment = ShieldedDeal::from_terms(&terms)
        .expect("shape")
        .commitment(&blinding)
        .expect("commitment");
    let make_auth = |role, name: &str| Authorization {
        role,
        suite_id: 2,
        commitment,
        signature: SignatureBytes::new(bytes(field(
            &vector["expected"],
            &format!("{name}SignatureHex"),
        )))
        .expect("signature"),
    };
    let buyer = make_auth(Role::Buyer, "buyer");
    let seller = make_auth(Role::Seller, "seller");
    let verified = verify_agreement(&terms, &blinding, &buyer, &seller, terms.expiry - 1)
        .expect("agreed and unexpired");
    assert_eq!(verified.commitment, commitment);
    assert_eq!(
        hex::encode(verified.payment_note),
        field(&vector["expected"], "paymentCommitmentHex")
    );
    assert!(verify_agreement(&terms, &blinding, &buyer, &seller, terms.expiry).is_err());
    assert!(verify_agreement(&terms, &blinding, &seller, &buyer, 1).is_err());
    let mut altered = terms.clone();
    altered.amount = BaseUnits::new(71);
    assert!(verify_agreement(&altered, &blinding, &buyer, &seller, 1).is_err());
    let mut bad = seller.clone();
    bad.signature = SignatureBytes::new(vec![0; 96]).expect("length");
    assert!(verify_agreement(&terms, &blinding, &buyer, &bad, 1).is_err());
}
