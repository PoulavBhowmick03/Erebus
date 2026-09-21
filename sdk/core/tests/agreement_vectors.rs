//! Known-answer vectors for agreement protocol v1.
//!
//! The fixture contains three valid agreements and one shielded rejection case. Valid cases
//! pin canonical bytes, commitment, deal identity, authorization digests, and signatures.
//! Changing a pinned value is a protocol change: update the specification first, then run
//!
//! ```text
//! cargo test --test agreement_vectors -- --ignored --nocapture regenerate
//! ```
//!
//! The suite's primitives are additionally pinned against external vectors in
//! `src/suite.rs` (keccak256 published vectors and the EIP-155 signature example), so the
//! fixture is not the only ground truth.

use erebus_core::auth::{
    authorization_digest, verify_authorization_signature, Authorization, Role,
};
use erebus_core::commitment::{commit_agreement, deal_nullifier, CommitmentBlinding};
use erebus_core::domain::DeploymentDomain;
use erebus_core::ids::{
    AddressBytes, AssetId, BaseUnits, ChainNamespace, KeyBytes, SignatureBytes,
};
use erebus_core::service::ServiceRecord;
use erebus_core::suite::EVM_SECP256K1_KECCAK_SUITE_ID;
use erebus_core::terms::{
    AgreementTerms, FeePolicy, Guarantee, GuaranteeSet, SettlementMode, CURRENT_PROTOCOL_VERSION,
};
use k256::ecdsa::SigningKey;
use serde::Deserialize;
use sha3::{Digest, Keccak256};

const FIXTURE: &str = include_str!("fixtures/agreement-v1-vectors.json");

fn signing_key(seed: u8) -> SigningKey {
    SigningKey::from_slice(&[seed; 32]).expect("deterministic key")
}

fn address_of(key: &SigningKey) -> Vec<u8> {
    let point = key.verifying_key().to_encoded_point(false);
    let hash = Keccak256::digest(&point.as_bytes()[1..]);
    hash[12..].to_vec()
}

fn sign(key: &SigningKey, digest: &[u8; 32]) -> String {
    let (signature, recovery) = key
        .sign_prehash_recoverable(digest)
        .expect("deterministic signing");
    let mut bytes = signature.to_bytes().to_vec();
    bytes.push(u8::from(recovery));
    hex::encode(bytes)
}

fn base_terms(buyer: &SigningKey, seller: &SigningKey) -> AgreementTerms {
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    suite_id: u16,
    vectors: Vec<Vector>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Vector {
    name: String,
    terms: TermsJson,
    blinding_hex: String,
    expected: Expected,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TermsJson {
    protocol_version: u16,
    suite_id: u16,
    domain: DomainJson,
    deal_id_hex: String,
    revision: u32,
    transcript_root_hex: String,
    buyer_authorization_key_hex: String,
    seller_authorization_key_hex: String,
    payment_recipient_hex: String,
    asset: String,
    amount: String,
    expiry: u64,
    fee: String,
    fee_recipient_hex: Option<String>,
    settlement_mode: String,
    required_guarantees: Vec<String>,
    settlement_nonce_hex: String,
    service: ServiceJson,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DomainJson {
    namespace: String,
    settlement_contract_hex: Option<String>,
    pool_hex: Option<String>,
    verifier_version: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServiceJson {
    resource: String,
    quantity: String,
    unit: String,
    access_recipient_hex: String,
    delivery_deadline: u64,
    fulfillment_method: String,
    fulfillment_digest_hex: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Expected {
    canonical_hex: String,
    commitment_hex: String,
    deal_nullifier_hex: String,
    buyer_digest_hex: String,
    seller_digest_hex: String,
    buyer_signature_hex: String,
    seller_signature_hex: String,
}

fn hex_bytes(value: &str) -> Vec<u8> {
    hex::decode(value).expect("fixture hex decodes")
}

fn hex_32(value: &str) -> [u8; 32] {
    hex_bytes(value)
        .try_into()
        .expect("fixture field is 32 bytes")
}

fn hex_16(value: &str) -> [u8; 16] {
    hex_bytes(value)
        .try_into()
        .expect("fixture field is 16 bytes")
}

fn parse_amount(value: &str) -> BaseUnits {
    BaseUnits::new(value.parse().expect("fixture amount is a u128"))
}

fn parse_mode(value: &str) -> SettlementMode {
    match value {
        "public_bound" => SettlementMode::PublicBound,
        "shielded" => SettlementMode::Shielded,
        other => panic!("unknown settlement mode `{other}`"),
    }
}

fn parse_guarantees(values: &[String]) -> GuaranteeSet {
    let mut set = GuaranteeSet::empty();
    for value in values {
        let guarantee = match value.as_str() {
            "hidden-amount" => Guarantee::HiddenAmount,
            "hidden-recipient" => Guarantee::HiddenRecipient,
            "agreement-bound-settlement" => Guarantee::AgreementBoundSettlement,
            "scoped-disclosure" => Guarantee::ScopedDisclosure,
            other => panic!("unknown guarantee `{other}`"),
        };
        set.insert(guarantee);
    }
    set
}

fn build_domain(domain: &DomainJson) -> DeploymentDomain {
    DeploymentDomain {
        namespace: ChainNamespace::parse(&domain.namespace).expect("fixture namespace"),
        settlement_contract: domain
            .settlement_contract_hex
            .as_ref()
            .map(|value| AddressBytes::new(hex_bytes(value)).expect("fixture address")),
        pool: domain
            .pool_hex
            .as_ref()
            .map(|value| AddressBytes::new(hex_bytes(value)).expect("fixture address")),
        verifier_version: domain.verifier_version,
    }
}

fn build_terms(terms: &TermsJson) -> AgreementTerms {
    AgreementTerms {
        protocol_version: terms.protocol_version,
        suite_id: terms.suite_id,
        domain: build_domain(&terms.domain),
        deal_id: hex_16(&terms.deal_id_hex),
        revision: terms.revision,
        transcript_root: hex_32(&terms.transcript_root_hex),
        buyer_authorization_key: KeyBytes::new(hex_bytes(&terms.buyer_authorization_key_hex))
            .expect("fixture key"),
        seller_authorization_key: KeyBytes::new(hex_bytes(&terms.seller_authorization_key_hex))
            .expect("fixture key"),
        payment_recipient: KeyBytes::new(hex_bytes(&terms.payment_recipient_hex))
            .expect("fixture key"),
        asset: AssetId::parse(&terms.asset).expect("fixture asset"),
        amount: parse_amount(&terms.amount),
        expiry: terms.expiry,
        fee_policy: FeePolicy {
            fee: parse_amount(&terms.fee),
            recipient: terms
                .fee_recipient_hex
                .as_ref()
                .map(|value| KeyBytes::new(hex_bytes(value)).expect("fixture key")),
        },
        settlement_mode: parse_mode(&terms.settlement_mode),
        required_guarantees: parse_guarantees(&terms.required_guarantees),
        settlement_nonce: hex_32(&terms.settlement_nonce_hex),
        service: ServiceRecord {
            resource: terms.service.resource.clone(),
            quantity: parse_amount(&terms.service.quantity),
            unit: terms.service.unit.clone(),
            access_recipient: KeyBytes::new(hex_bytes(&terms.service.access_recipient_hex))
                .expect("fixture key"),
            delivery_deadline: terms.service.delivery_deadline,
            fulfillment_method: terms.service.fulfillment_method.clone(),
            fulfillment_digest: hex_32(&terms.service.fulfillment_digest_hex),
        },
    }
}

#[test]
fn vectors_match_the_pinned_values() {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("fixture parses");
    assert_eq!(fixture.suite_id, EVM_SECP256K1_KECCAK_SUITE_ID);
    assert!(fixture.vectors.len() >= 4, "expected four vectors");

    for vector in &fixture.vectors {
        let terms = build_terms(&vector.terms);
        let blinding = CommitmentBlinding::from_bytes(hex_32(&vector.blinding_hex));

        // Keep the pre-review shielded bytes as a rejection vector, not a supported suite.
        if terms.settlement_mode == SettlementMode::Shielded {
            let error = erebus_core::terms::TermsError::SuiteMode(
                erebus_core::suite::SuiteError::UnsupportedMode {
                    suite_id: 1,
                    mode: SettlementMode::Shielded,
                },
            );
            assert_eq!(terms.encode(), Err(error.clone()));
            assert_eq!(
                AgreementTerms::decode(&hex_bytes(&vector.expected.canonical_hex)),
                Err(error)
            );
            continue;
        }

        let canonical = terms.encode().expect("terms encode");
        assert_eq!(
            hex::encode(&canonical),
            vector.expected.canonical_hex,
            "{}: canonical bytes",
            vector.name
        );
        assert_eq!(
            AgreementTerms::decode(&canonical).expect("decodes"),
            terms,
            "{}: decode round trip",
            vector.name
        );

        let commitment = commit_agreement(&terms, &blinding).expect("commits");
        assert_eq!(
            commitment.to_hex(),
            vector.expected.commitment_hex,
            "{}: commitment",
            vector.name
        );
        assert_eq!(
            deal_nullifier(&terms).expect("derives").to_hex(),
            vector.expected.deal_nullifier_hex,
            "{}: deal identity",
            vector.name
        );

        for (role, digest_hex, signature_hex) in [
            (
                Role::Buyer,
                &vector.expected.buyer_digest_hex,
                &vector.expected.buyer_signature_hex,
            ),
            (
                Role::Seller,
                &vector.expected.seller_digest_hex,
                &vector.expected.seller_signature_hex,
            ),
        ] {
            let digest = authorization_digest(&terms.domain, role, &commitment, terms.suite_id)
                .expect("digest");
            assert_eq!(
                hex::encode(digest),
                *digest_hex,
                "{}: {role:?} digest",
                vector.name
            );
            let authorization = Authorization {
                role,
                suite_id: terms.suite_id,
                commitment,
                signature: SignatureBytes::new(hex_bytes(signature_hex))
                    .expect("fixture signature"),
            };
            verify_authorization_signature(&terms, &commitment, &blinding, &authorization)
                .unwrap_or_else(|error| panic!("{}: {role:?} signature: {error}", vector.name));
        }
    }
}

#[test]
#[ignore = "regenerates the pinned fixture; run with --ignored --nocapture"]
fn regenerate_vectors() {
    let buyer = signing_key(0x01);
    let seller = signing_key(0x02);
    let fee_recipient = signing_key(0x04);

    let base = base_terms(&buyer, &seller);
    let mut counteroffer = base.clone();
    counteroffer.revision = 2;
    counteroffer.amount = BaseUnits::new(900_000);
    counteroffer.service.quantity = BaseUnits::new(450);

    let existing: serde_json::Value = serde_json::from_str(FIXTURE).expect("fixture parses");
    let rejected_shielded = existing["vectors"]
        .as_array()
        .expect("vectors")
        .iter()
        .find(|entry| entry["terms"]["settlementMode"] == "shielded")
        .expect("preserve the shielded rejection vector")
        .clone();

    let mut with_fee = base.clone();
    with_fee.fee_policy = FeePolicy {
        fee: BaseUnits::new(2_500),
        recipient: Some(KeyBytes::new(address_of(&fee_recipient)).expect("valid key")),
    };

    let vectors = vec![
        fixture_entry("public-bound, no fee", &base, [0x0a; 32], &buyer, &seller),
        fixture_entry(
            "counteroffer revision shares the deal identity",
            &counteroffer,
            [0x0b; 32],
            &buyer,
            &seller,
        ),
        rejected_shielded,
        fixture_entry("bound fee policy", &with_fee, [0x0d; 32], &buyer, &seller),
    ];

    let fixture = serde_json::json!({
        "_comment": "Pinned agreement-v1 vectors. Regenerate with `cargo test -p erebus-core --test agreement_vectors -- --ignored --nocapture regenerate`. Changing any value is a protocol change; update docs/metropolis-agreement.md first.",
        "suiteId": EVM_SECP256K1_KECCAK_SUITE_ID,
        "vectors": vectors,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&fixture).expect("fixture serializes")
    );
}

fn fixture_entry(
    name: &str,
    terms: &AgreementTerms,
    blinding: [u8; 32],
    buyer: &SigningKey,
    seller: &SigningKey,
) -> serde_json::Value {
    let commitment =
        commit_agreement(terms, &CommitmentBlinding::from_bytes(blinding)).expect("commits");
    let buyer_digest =
        authorization_digest(&terms.domain, Role::Buyer, &commitment, terms.suite_id)
            .expect("digest");
    let seller_digest =
        authorization_digest(&terms.domain, Role::Seller, &commitment, terms.suite_id)
            .expect("digest");
    serde_json::json!({
        "name": name,
        "terms": terms_json(terms),
        "blindingHex": hex::encode(blinding),
        "expected": {
            "canonicalHex": hex::encode(terms.encode().expect("terms encode")),
            "commitmentHex": commitment.to_hex(),
            "dealNullifierHex": deal_nullifier(terms).expect("derives").to_hex(),
            "buyerDigestHex": hex::encode(buyer_digest),
            "sellerDigestHex": hex::encode(seller_digest),
            "buyerSignatureHex": sign(buyer, &buyer_digest),
            "sellerSignatureHex": sign(seller, &seller_digest),
        },
    })
}

fn terms_json(terms: &AgreementTerms) -> serde_json::Value {
    serde_json::json!({
        "protocolVersion": terms.protocol_version,
        "suiteId": terms.suite_id,
        "domain": {
            "namespace": terms.domain.namespace.to_string(),
            "settlementContractHex": terms
                .domain
                .settlement_contract
                .as_ref()
                .map(|address| hex::encode(address.as_bytes())),
            "poolHex": terms
                .domain
                .pool
                .as_ref()
                .map(|address| hex::encode(address.as_bytes())),
            "verifierVersion": terms.domain.verifier_version,
        },
        "dealIdHex": hex::encode(terms.deal_id),
        "revision": terms.revision,
        "transcriptRootHex": hex::encode(terms.transcript_root),
        "buyerAuthorizationKeyHex": hex::encode(terms.buyer_authorization_key.as_bytes()),
        "sellerAuthorizationKeyHex": hex::encode(terms.seller_authorization_key.as_bytes()),
        "paymentRecipientHex": hex::encode(terms.payment_recipient.as_bytes()),
        "asset": terms.asset.to_string(),
        "amount": terms.amount.get().to_string(),
        "expiry": terms.expiry,
        "fee": terms.fee_policy.fee.get().to_string(),
        "feeRecipientHex": terms
            .fee_policy
            .recipient
            .as_ref()
            .map(|key| hex::encode(key.as_bytes())),
        "settlementMode": match terms.settlement_mode {
            SettlementMode::PublicBound => "public_bound",
            SettlementMode::Shielded => "shielded",
        },
        "requiredGuarantees": terms
            .required_guarantees
            .iter()
            .map(Guarantee::name)
            .collect::<Vec<_>>(),
        "settlementNonceHex": hex::encode(terms.settlement_nonce),
        "service": {
            "resource": terms.service.resource,
            "quantity": terms.service.quantity.get().to_string(),
            "unit": terms.service.unit,
            "accessRecipientHex": hex::encode(terms.service.access_recipient.as_bytes()),
            "deliveryDeadline": terms.service.delivery_deadline,
            "fulfillmentMethod": terms.service.fulfillment_method,
            "fulfillmentDigestHex": hex::encode(terms.service.fulfillment_digest),
        },
    })
}
