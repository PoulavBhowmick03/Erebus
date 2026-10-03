//! x402 `exact` Permit2 bytes pinned to the reference x402 SDK (`fixtures/x402-permit2-vectors.json`).

use erebus_core::commitment::DealNullifier;
use erebus_evm::x402::{domain_separator, DealPermit, EXACT_PERMIT2_PROXY};
use erebus_transport::identity::AuthorizationIdentity;
use serde_json::Value;

fn bytes<const N: usize>(value: &Value) -> [u8; N] {
    hex::decode(value.as_str().unwrap().trim_start_matches("0x"))
        .unwrap()
        .try_into()
        .unwrap()
}

#[test]
fn permit2_digest_signature_and_calldata_match_the_reference_sdk() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/x402-permit2-vectors.json")).unwrap();
    let vectors = fixture["vectors"].as_array().unwrap();
    assert_eq!(vectors.len(), 3);
    for vector in vectors {
        let name = vector["name"].as_str().unwrap();
        let chain_id: u64 = vector["chain_id"].as_u64().unwrap();
        assert_eq!(
            bytes::<20>(&vector["spender"]),
            EXACT_PERMIT2_PROXY,
            "{name}"
        );
        let permit = DealPermit {
            token: bytes(&vector["token"]),
            amount: vector["amount"].as_str().unwrap().parse().unwrap(),
            deal: DealNullifier::from_bytes(bytes(&vector["nonce"])),
            deadline: vector["deadline"].as_str().unwrap().parse().unwrap(),
            to: bytes(&vector["to"]),
            valid_after: vector["valid_after"].as_str().unwrap().parse().unwrap(),
        };
        let expected = &vector["expected"];
        assert_eq!(
            domain_separator(chain_id),
            bytes(&expected["domain_separator"]),
            "{name}"
        );
        assert_eq!(
            permit.struct_hash(),
            bytes(&expected["struct_hash"]),
            "{name}"
        );
        assert_eq!(
            permit.digest(chain_id),
            bytes(&expected["digest"]),
            "{name}"
        );
        let owner =
            AuthorizationIdentity::from_bytes(&bytes::<32>(&vector["private_key"])).unwrap();
        assert_eq!(owner.address(), bytes(&vector["owner"]), "{name}");
        let signature = permit.sign(chain_id, &owner);
        assert_eq!(signature, bytes::<65>(&expected["signature"]), "{name}");
        assert_eq!(
            permit.recover_owner(chain_id, &signature),
            Some(owner.address()),
            "{name}"
        );
        assert_eq!(
            hex::encode(permit.encode_settle_call(&owner.address(), &signature)),
            expected["settle_calldata"].as_str().unwrap(),
            "{name}"
        );
    }
}
