//! The independent M4 Circom/JS runner pins these suite-2 agreement hashes.

use erebus_core::auth::Role;
use erebus_core::commitment::CommitmentBlinding;
use erebus_core::domain::DeploymentDomain;
use erebus_core::ids::{AddressBytes, AssetId, BaseUnits, ChainNamespace, KeyBytes};
use erebus_core::service::ServiceRecord;
use erebus_core::shielded::ShieldedDeal;
use erebus_core::terms::{AgreementTerms, FeePolicy, Guarantee, GuaranteeSet, SettlementMode};
use serde_json::Value;

const FIXTURE: &str = include_str!("fixtures/agreement-suite2-vector.json");

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
    assert!(original
        .commitment(&CommitmentBlinding::from_bytes([0; 32]))
        .is_err());
}
