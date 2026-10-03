use super::*;
use crate::{descriptor::MODE_SHIELDED, identity::TransportIdentity, negotiation::tests::proposal};

const NOW: u64 = 1_700_000_000;

fn fixture() -> (
    ServiceDescriptor,
    AgreementKeyBinding,
    AuthorizationIdentity,
) {
    let draft = proposal(2);
    let signer = AuthorizationIdentity::from_bytes(&[2; 32]).unwrap();
    let transport = TransportIdentity::from_private_key([22; 32]).unwrap();
    let mut descriptor = ServiceDescriptor::new(
        signer.address(),
        transport.public_key(),
        vec!["tcp://seller.example".into()],
        draft.terms().domain.namespace.clone(),
        vec![draft.terms().asset.clone()],
        vec![2],
        MODE_SHIELDED,
        draft.terms().required_guarantees,
        NOW - 1,
        NOW + 1000,
    )
    .unwrap();
    descriptor.sign(&signer).unwrap();
    let binding = AgreementKeyBinding::sign(
        &descriptor,
        draft.terms().domain.clone(),
        draft.terms().seller_authorization_key.clone(),
        &signer,
        NOW,
        NOW + 300,
    )
    .unwrap();
    (descriptor, binding, signer)
}

#[test]
fn signed_binding_round_trips_and_is_not_a_public_descriptor_field() {
    let (descriptor, binding, _) = fixture();
    let draft = proposal(2);
    binding
        .verify(
            &descriptor,
            &draft.terms().domain,
            &draft.terms().seller_authorization_key,
            NOW,
        )
        .unwrap();
    let encoded = binding.encode().unwrap();
    assert_eq!(
        binding
            .attested_key(&descriptor, &draft.terms().domain, NOW)
            .unwrap(),
        draft.terms().seller_authorization_key
    );
    assert_eq!(AgreementKeyBinding::decode(&encoded).unwrap(), binding);
    assert_eq!(format!("{binding:?}"), "AgreementKeyBinding(<redacted>)");
    let public = descriptor.encode_unsigned();
    assert!(!public
        .windows(64)
        .any(|bytes| bytes == draft.terms().seller_authorization_key.as_bytes()));
    assert!(encoded.len() <= MAX_BINDING_BYTES);
}

#[test]
fn descriptor_key_domain_and_signature_substitution_are_rejected() {
    let (descriptor, binding, signer) = fixture();
    let draft = proposal(2);
    let domain = &draft.terms().domain;
    let key = &draft.terms().seller_authorization_key;
    let mut changed = descriptor.clone();
    changed.transport_key[0] ^= 1;
    changed.sign(&signer).unwrap();
    assert!(binding.verify(&changed, domain, key, NOW).is_err());
    let mut changed_domain = domain.clone();
    changed_domain.verifier_version += 1;
    assert!(binding
        .verify(&descriptor, &changed_domain, key, NOW)
        .is_err());
    assert!(binding
        .verify(
            &descriptor,
            domain,
            &draft.terms().buyer_authorization_key,
            NOW
        )
        .is_err());
    let mut changed_binding = binding.clone();
    changed_binding.signature[0] ^= 1;
    assert!(changed_binding
        .verify(&descriptor, domain, key, NOW)
        .is_err());
    let wrong = AuthorizationIdentity::from_bytes(&[1; 32]).unwrap();
    assert!(AgreementKeyBinding::sign(
        &descriptor,
        domain.clone(),
        key.clone(),
        &wrong,
        NOW,
        NOW + 100
    )
    .is_err());
}

#[test]
fn expired_future_and_extended_validity_windows_fail_at_the_exact_boundary() {
    let (descriptor, binding, signer) = fixture();
    let draft = proposal(2);
    let domain = &draft.terms().domain;
    let key = &draft.terms().seller_authorization_key;
    assert!(binding.verify(&descriptor, domain, key, NOW - 1).is_err());
    binding.verify(&descriptor, domain, key, NOW + 299).unwrap();
    assert!(binding.verify(&descriptor, domain, key, NOW + 300).is_err());
    for (issued, expires) in [
        (NOW - 2, NOW + 100),
        (NOW, NOW),
        (NOW, descriptor.expires + 1),
    ] {
        assert!(AgreementKeyBinding::sign(
            &descriptor,
            domain.clone(),
            key.clone(),
            &signer,
            issued,
            expires
        )
        .is_err());
    }
}

#[test]
fn malformed_noncanonical_and_oversized_control_data_fail_closed() {
    let (_, binding, _) = fixture();
    let encoded = binding.encode().unwrap();
    for end in [0, 1, 33, encoded.len() - 1] {
        assert!(AgreementKeyBinding::decode(&encoded[..end]).is_err());
    }
    let mut trailing = encoded.clone();
    trailing.push(0);
    assert!(AgreementKeyBinding::decode(&trailing).is_err());
    let mut unknown = encoded;
    unknown[1] = 2;
    assert!(AgreementKeyBinding::decode(&unknown).is_err());
    assert!(AgreementKeyBinding::decode(&[0; MAX_BINDING_BYTES + 1]).is_err());
}
