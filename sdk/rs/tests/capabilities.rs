//! The STRK20 capability declaration must stay honest.
//!
//! These tests pin what the existing backend does and does not guarantee. The important case
//! is the one that fails: a deal requiring proof-enforced agreement binding must be refused
//! explicitly, never downgraded to client-side checks.

use std::collections::BTreeSet;

use erebus_core::ids::{AddressBytes, AssetId, ChainNamespace};
use erebus_core::settlement::{
    check_capabilities, check_requirements, SelectionError, SettlementContext,
};
use erebus_core::terms::{Guarantee, GuaranteeSet, SettlementMode};
use erebus_sdk::capabilities::strk20_capabilities;

fn context(mode: SettlementMode, guarantees: GuaranteeSet) -> SettlementContext {
    SettlementContext {
        require_local_proving: false,
        mode,
        domain: erebus_core::domain::DeploymentDomain {
            namespace: ChainNamespace::new("starknet", "SN_SEPOLIA").expect("valid namespace"),
            settlement_contract: None,
            pool: Some(AddressBytes::new(vec![0x07; 32]).expect("valid address")),
            verifier_version: 1,
        },
        suite_id: 1,
        asset: AssetId::parse("starknet:SN_SEPOLIA/erc20:0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d")
            .expect("valid asset"),
        required_guarantees: guarantees,
    }
}

fn hidden_guarantees() -> GuaranteeSet {
    let mut set = GuaranteeSet::empty();
    set.insert(Guarantee::HiddenAmount);
    set.insert(Guarantee::HiddenRecipient);
    set
}

#[test]
fn strk20_declares_shielded_but_not_proof_enforced_agreement_binding() {
    let capabilities = strk20_capabilities();
    assert!(capabilities.supports_mode(SettlementMode::Shielded));
    assert!(!capabilities.supports_mode(SettlementMode::PublicBound));
    assert!(capabilities.provides(Guarantee::HiddenAmount));
    assert!(capabilities.provides(Guarantee::HiddenRecipient));
    assert!(!capabilities.provides(Guarantee::ScopedDisclosure));
    assert!(
        capabilities.suites.is_empty(),
        "legacy wire is not canonical v1"
    );
    assert!(
        !capabilities.provides(Guarantee::AgreementBoundSettlement),
        "the STRK20 path enforces agreement equality in Rust, not in the pool proof"
    );
    assert!(
        !capabilities.local_proving,
        "the current path uses a hosted prover"
    );
}

#[test]
fn a_legacy_privacy_only_request_can_select_strk20_but_not_a_canonical_deal() {
    let capabilities = strk20_capabilities();
    check_requirements(
        SettlementMode::Shielded,
        hidden_guarantees(),
        false,
        &capabilities,
    )
    .expect("legacy hidden amount and recipient are provided");
    assert_eq!(
        check_capabilities(
            &context(SettlementMode::Shielded, hidden_guarantees()),
            &capabilities,
        ),
        Err(SelectionError::SuiteUnsupported { suite_id: 1 })
    );
}

#[test]
fn a_deal_requiring_agreement_bound_settlement_fails_explicitly() {
    let capabilities = strk20_capabilities();
    let mut guarantees = hidden_guarantees();
    guarantees.insert(Guarantee::AgreementBoundSettlement);
    assert_eq!(
        check_capabilities(
            &context(SettlementMode::Shielded, guarantees),
            &capabilities
        ),
        Err(SelectionError::MissingGuarantee {
            guarantee: Guarantee::AgreementBoundSettlement
        })
    );
}

#[test]
fn a_public_bound_deal_cannot_select_strk20() {
    let capabilities = strk20_capabilities();
    assert_eq!(
        check_capabilities(
            &context(SettlementMode::PublicBound, GuaranteeSet::empty()),
            &capabilities
        ),
        Err(SelectionError::ModeUnsupported {
            mode: SettlementMode::PublicBound
        })
    );
}

#[test]
fn mode_sets_are_not_accidentally_empty() {
    let modes: BTreeSet<SettlementMode> = strk20_capabilities().modes;
    assert_eq!(modes.len(), 1);
}

#[tokio::test]
async fn the_actual_adapter_rejects_requirements_before_any_settlement_work() {
    use erebus_sdk::client::{Client, ClientConfig, ClientError, ErebusClient, OfferId};
    use erebus_sdk::operation::OperationId;
    use erebus_sdk::state::ChannelHandle;
    use erebus_sdk::strk20_settlement::{LegacySettlementRequirements, Strk20SettlementBackend};
    use erebus_sdk::wire::WireVersion;
    use starknet_types_core::felt::Felt;

    let root = std::env::temp_dir().join(format!(
        "erebus-capabilities-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let client = Client::new(ClientConfig {
        rpc_url: url.clone(),
        prover_url: url,
        pool_address: Felt::ONE,
        chain_id: Felt::ONE,
        account_address: Felt::ONE,
        pool_key_file: root.join("absent-pool-key"),
        account_key_file: root.join("absent-account-key"),
        state_dir: root.clone(),
        token: Felt::ONE,
        new_channel_wire_version: WireVersion::V3,
    })
    .unwrap();
    fn snapshot(root: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                out.extend(snapshot(&path));
            }
            out.push(path);
        }
        out.sort();
        out
    }
    let before = snapshot(&root);
    let operation = OperationId::parse(format!("op_{}", "a".repeat(64))).unwrap();
    let handle = ChannelHandle::parse(format!("ch_{}", "b".repeat(64))).unwrap();
    // Malformed legacy offer: any attempt to enter execution would return InvalidOfferId.
    let offer: OfferId = serde_json::from_str("\"not-an-offer\"").unwrap();
    let backend = Strk20SettlementBackend::new(&client);
    let mut binding = LegacySettlementRequirements::default();
    binding
        .guarantees
        .insert(Guarantee::AgreementBoundSettlement);
    let local = LegacySettlementRequirements {
        require_local_proving: true,
        ..Default::default()
    };
    let public = LegacySettlementRequirements {
        mode: SettlementMode::PublicBound,
        ..Default::default()
    };
    let mut scoped = LegacySettlementRequirements::default();
    scoped.guarantees.insert(Guarantee::ScopedDisclosure);
    for (requirements, expected) in [
        (
            binding,
            SelectionError::MissingGuarantee {
                guarantee: Guarantee::AgreementBoundSettlement,
            },
        ),
        (local, SelectionError::LocalProvingRequired),
        (
            public,
            SelectionError::ModeUnsupported {
                mode: SettlementMode::PublicBound,
            },
        ),
        (
            scoped,
            SelectionError::MissingGuarantee {
                guarantee: Guarantee::ScopedDisclosure,
            },
        ),
    ] {
        let error = backend
            .settle(requirements, &operation, handle.clone(), offer.clone())
            .await
            .unwrap_err();
        assert!(matches!(error, ClientError::UnsupportedSettlement(actual) if actual == expected));
    }
    // Both the checked adapter and unchanged public API still reach legacy validation.
    let direct = backend
        .settle(
            Default::default(),
            &operation,
            handle.clone(),
            offer.clone(),
        )
        .await
        .unwrap_err();
    let compatible = client
        .accept_and_settle(&operation, handle, offer)
        .await
        .unwrap_err();
    assert_eq!(direct.to_string(), compatible.to_string());
    assert!(!matches!(direct, ClientError::UnsupportedSettlement(_)));
    assert_eq!(
        snapshot(&root),
        before,
        "rejected requests must not create operation records"
    );
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    std::fs::remove_dir_all(root).unwrap();
}
