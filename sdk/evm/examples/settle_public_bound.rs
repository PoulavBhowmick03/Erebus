//! Drives one coordinated public-bound settlement against a live EVM deployment.
//!
//! Test tooling for the M8 funded workflow: blinding, nonce, and deal values are test-only.
//! Independent mode requires an explicit seller address and external authorization.
//! A deterministic local seller requires EREBUS_EVM_ALLOW_TEST_SELLER=1.
//!
//! Environment:
//! - `EREBUS_EVM_RPC_URL`, `EREBUS_EVM_SETTLEMENT`, `EREBUS_EVM_TOKEN`
//! - `EREBUS_EVM_BUYER_KEY_FILE` (a 32-byte hex key, owner-only)
//! - `EREBUS_EVM_STATE_ROOT` (coordinator and signer-journal directory)
//! - `EREBUS_EVM_AMOUNT` (base units, default 1000000)
//! - `EREBUS_EVM_SELLER_ADDRESS` (expected independent seller, lowercase 0x address)
//!
//! Flow: durable intent, both authorizations, preparation, nonce claim, local signing,
//! journaled broadcast, finalized observation, and reconciliation.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use erebus_coordinator::Coordinator;
use erebus_core::auth::{
    authorization_digest, verify_authorization_signature, Authorization, Role,
};
use erebus_core::commitment::{commit_agreement, CommitmentBlinding};
use erebus_core::deal_state::DealState;
use erebus_core::domain::DeploymentDomain;
use erebus_core::ids::{
    AddressBytes, AssetId, BaseUnits, ChainNamespace, KeyBytes, SignatureBytes,
};
use erebus_core::policy::SpendingPolicy;
use erebus_core::service::ServiceRecord;
use erebus_core::settlement::{BackendCapabilities, SettlementContext};
use erebus_core::terms::{
    AgreementTerms, FeePolicy, Guarantee, GuaranteeSet, SettlementMode, CURRENT_PROTOCOL_VERSION,
};
use erebus_evm::backend::EvmSettlementBackend;
use erebus_evm::chain::{
    Eip1559Fees, EvmChain, HistoricalObservation, ObservationJournal, ObservationLimits,
    SignerJournal, SigningPlan, TransactionKey,
};
use erebus_evm::deployment::{parse_lowercase_address, EvmDeployment};
use erebus_transport::disclosure::SelectedAgreement;
use erebus_transport::hashing::TRANSCRIPT_HASH_VERSION;
use k256::ecdsa::SigningKey;
use serde_json::json;
use zeroize::Zeroizing;

fn private_read(
    path: &std::path::Path,
    limit: u64,
) -> Result<Zeroizing<Vec<u8>>, Box<dyn std::error::Error>> {
    use std::io::Read;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err("private input must be a bounded regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("private input must be owner-only".into());
        }
    }
    let mut bytes = Zeroizing::new(Vec::new());
    std::fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err("private input exceeds limit".into());
    }
    Ok(bytes)
}

fn private_write(path: &std::path::Path, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

fn env(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    std::env::var(name).map_err(|_| format!("{name} is not set").into())
}

fn unix_now() -> Result<u64, Box<dyn std::error::Error>> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

fn address_of(key: &SigningKey) -> [u8; 20] {
    use sha3::Digest;
    let point = key.verifying_key().to_encoded_point(false);
    let digest: [u8; 32] = sha3::Keccak256::digest(&point.as_bytes()[1..]).into();
    let mut address = [0u8; 20];
    address.copy_from_slice(&digest[12..]);
    address
}

fn authorization(
    role: Role,
    terms: &AgreementTerms,
    commitment: erebus_core::commitment::DealCommitment,
    key: &SigningKey,
) -> Result<Authorization, Box<dyn std::error::Error>> {
    let digest = authorization_digest(&terms.domain, role, &commitment, terms.suite_id)?;
    let (signature, recovery_id) = key.sign_prehash_recoverable(&digest)?;
    let mut bytes = [0u8; 65];
    bytes[..64].copy_from_slice(&signature.to_bytes());
    bytes[64] = recovery_id.to_byte();
    Ok(Authorization {
        role,
        suite_id: terms.suite_id,
        commitment,
        signature: SignatureBytes::new(bytes.to_vec())?,
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rpc_url = env("EREBUS_EVM_RPC_URL")?;
    let settlement = parse_lowercase_address(&env("EREBUS_EVM_SETTLEMENT")?)
        .ok_or("EREBUS_EVM_SETTLEMENT must be a lowercase 0x address")?;
    let token = parse_lowercase_address(&env("EREBUS_EVM_TOKEN")?)
        .ok_or("EREBUS_EVM_TOKEN must be a lowercase 0x address")?;
    let state_root = std::path::PathBuf::from(env("EREBUS_EVM_STATE_ROOT")?);
    let amount: u128 = std::env::var("EREBUS_EVM_AMOUNT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1_000_000);
    let buyer_bytes: Zeroizing<[u8; 32]> = {
        let bytes = private_read(
            std::path::Path::new(&env("EREBUS_EVM_BUYER_KEY_FILE")?),
            128,
        )?;
        let text = std::str::from_utf8(&bytes)?.trim();
        let bytes = Zeroizing::new(hex::decode(text.strip_prefix("0x").unwrap_or(text))?);
        Zeroizing::new(
            bytes
                .as_slice()
                .try_into()
                .map_err(|_| "buyer key must be 32 bytes")?,
        )
    };
    let buyer_key = SigningKey::from_slice(buyer_bytes.as_ref())?;
    let seller_key =
        if std::env::var("EREBUS_EVM_ALLOW_TEST_SELLER").is_ok_and(|value| value == "1") {
            Some(SigningKey::from_slice(&[0x5e; 32])?)
        } else {
            None
        };
    // A fresh seed gives a fresh deal identity; the same seed is a different signed revision
    // of an already-consumed deal.
    let buyer_address = address_of(&buyer_key);
    let seller_address = match &seller_key {
        Some(key) => address_of(key),
        None => parse_lowercase_address(&env("EREBUS_EVM_SELLER_ADDRESS")?)
            .ok_or("expected seller must be a lowercase 0x address")?,
    };
    let now = unix_now()?;

    let namespace = ChainNamespace::parse("eip155:10143")?;
    let deployment = EvmDeployment::new(namespace.clone(), settlement, 1, rpc_url.clone())?;
    let asset = AssetId::new(namespace, "erc20", &format!("0x{}", hex::encode(token)))?;

    // The buyer proposes terms and a blinding. A separate seller process signs the same
    // proposal; this process can also load that proposal instead of rebuilding it.
    let (terms, blinding) = if let Ok(path) = std::env::var("EREBUS_EVM_PROPOSAL_IN") {
        let proposal: serde_json::Value =
            serde_json::from_slice(&private_read(std::path::Path::new(&path), 64 * 1024)?)?;
        let terms = AgreementTerms::decode(&hex::decode(
            proposal["terms"].as_str().ok_or("proposal has no terms")?,
        )?)?;
        let bytes: [u8; 32] = hex::decode(
            proposal["blinding"]
                .as_str()
                .ok_or("proposal has no blinding")?,
        )?
        .try_into()
        .map_err(|_| "proposal blinding must be 32 bytes")?;
        (terms, CommitmentBlinding::from_bytes(bytes))
    } else {
        let seed: u8 = std::env::var("EREBUS_EVM_DEAL_SEED")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0x42);
        let terms = AgreementTerms {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            suite_id: 1,
            domain: DeploymentDomain {
                namespace: ChainNamespace::parse("eip155:10143")?,
                settlement_contract: Some(AddressBytes::new(settlement.to_vec())?),
                pool: None,
                verifier_version: 1,
            },
            deal_id: [seed; 16],
            revision: 1,
            transcript_root: [0; 32],
            buyer_authorization_key: KeyBytes::new(buyer_address.to_vec())?,
            seller_authorization_key: KeyBytes::new(seller_address.to_vec())?,
            payment_recipient: KeyBytes::new(seller_address.to_vec())?,
            asset: asset.clone(),
            amount: BaseUnits::new(amount),
            expiry: now + 3_600,
            fee_policy: FeePolicy::none(),
            settlement_mode: SettlementMode::PublicBound,
            required_guarantees: GuaranteeSet::from_guarantee(Guarantee::AgreementBoundSettlement),
            settlement_nonce: [seed; 32],
            service: ServiceRecord {
                resource: "gpu.h100.hour".to_owned(),
                quantity: BaseUnits::new(1),
                unit: "gpu-hour".to_owned(),
                access_recipient: KeyBytes::new(buyer_address.to_vec())?,
                delivery_deadline: now + 7_200,
                fulfillment_method: "http-access".to_owned(),
                fulfillment_digest: [0; 32],
            },
        };
        (terms, CommitmentBlinding::from_bytes([0x0a; 32]))
    };
    deployment.matches_domain(&terms.domain)?;
    if terms.suite_id != 1
        || terms.settlement_mode != SettlementMode::PublicBound
        || terms.buyer_authorization_key.as_bytes() != buyer_address
        || terms.seller_authorization_key.as_bytes() != seller_address
        || terms.asset != asset
    {
        return Err("proposal differs from the selected buyer, seller, backend, or asset".into());
    }
    let commitment = commit_agreement(&terms, &blinding)?;
    if let Ok(path) = std::env::var("EREBUS_EVM_PROPOSAL_OUT") {
        private_write(
            std::path::Path::new(&path),
            &serde_json::to_vec(&json!({
                "terms":hex::encode(terms.encode()?),"blinding":hex::encode(blinding.as_bytes()),
            }))?,
        )?;
        println!(
            "{}",
            json!({"status":"ok","proposal":path,"deal_commitment":commitment.to_hex()})
        );
        return Ok(());
    }
    let buyer = authorization(Role::Buyer, &terms, commitment, &buyer_key)?;
    let seller = if let Ok(path) = std::env::var("EREBUS_EVM_SELLER_AUTHORIZATION_FILE") {
        let bytes = private_read(std::path::Path::new(&path), 4096)?;
        let seller = Authorization::decode(&hex::decode(std::str::from_utf8(&bytes)?.trim())?)?;
        if seller.role != Role::Seller || seller.commitment != commitment {
            return Err("seller authorization does not match the proposal".into());
        }
        verify_authorization_signature(&terms, &commitment, &blinding, &seller)?;
        seller
    } else {
        let key = seller_key
            .as_ref()
            .ok_or("independent settlement requires the seller authorization file")?;
        authorization(Role::Seller, &terms, commitment, key)?
    };
    if let Ok(path) = std::env::var("EREBUS_EVM_EVIDENCE_OUT") {
        let evidence = SelectedAgreement {
            terms: terms.clone(),
            blinding: blinding.clone(),
            buyer: buyer.clone(),
            seller: seller.clone(),
            transcript_hash_version: TRANSCRIPT_HASH_VERSION,
            messages: Vec::new(),
        };
        private_write(std::path::Path::new(&path), &evidence.encode()?)?;
    }

    let context = SettlementContext {
        require_local_proving: true,
        mode: SettlementMode::PublicBound,
        domain: terms.domain.clone(),
        suite_id: 1,
        asset: asset.clone(),
        required_guarantees: terms.required_guarantees,
    };
    let capabilities: BackendCapabilities =
        EvmSettlementBackend::connect(deployment.clone(), &buyer_bytes)?.capabilities();
    let policy = SpendingPolicy {
        per_deal_max: BaseUnits::new(amount),
        allowed_assets: [asset].into_iter().collect(),
        ..SpendingPolicy::default()
    };
    let coordinator = Coordinator::open(
        state_root.join("coordinator"),
        terms.buyer_authorization_key.clone(),
        context.clone(),
        &context,
        &capabilities,
        policy,
    )?;
    let operation_ref = [0x11; 32];
    coordinator.record_intent(operation_ref, &terms, &blinding, now)?;
    coordinator.authorize_buyer(operation_ref, now, |_, _| Ok::<_, ()>(buyer.clone()))?;
    coordinator.accept_seller(operation_ref, &seller)?;
    coordinator.prepare(operation_ref, now, |terms, blinding, buyer, seller| {
        EvmSettlementBackend::connect(deployment.clone(), &buyer_bytes)
            .and_then(|backend| backend.prepare(terms, blinding, buyer, seller, operation_ref))
    })?;

    let chain = EvmChain::connect(deployment.clone(), Duration::from_secs(15)).await?;
    let journal = SignerJournal::open(state_root.join("signer"), 10_143, buyer_address)?;
    let fees = Eip1559Fees::new(200_000_000_000, 1_000_000_000)?;
    let plan = chain
        .reserve_nonce(
            &journal,
            &coordinator.prepared_settlement(operation_ref)?,
            fees,
            1_000_000,
        )
        .await?;
    let transaction_key = TransactionKey::from_bytes(&buyer_bytes)?;
    coordinator.sign_transaction(
        operation_ref,
        now,
        &plan.encode(),
        |prepared, plan_bytes| {
            let plan = SigningPlan::decode(plan_bytes)?;
            plan.sign(&deployment, prepared, &transaction_key)
                .map(|transaction| transaction.raw().to_vec())
        },
        |prepared, plan_bytes, raw| {
            let plan = SigningPlan::decode(plan_bytes)?;
            plan.validate(&deployment, prepared, raw)
        },
    )?;
    let broadcast = chain
        .broadcast_journaled(&coordinator, operation_ref, now)
        .await?;

    let prepared = coordinator.prepared_settlement(operation_ref)?;
    // A live chain is far taller than a public RPC's `eth_getLogs` range cap, so the scan
    // starts at the deployment block and checkpoints its progress. The caller must supply a
    // block at or before the deployment's first possible settlement.
    let from_block: u64 = env("EREBUS_EVM_FROM_BLOCK")?.parse()?;
    let journal = ObservationJournal::open(state_root.join("history"))?;
    let budget = ObservationLimits {
        log_block_range: 100,
        max_log_queries: 1_024,
        max_ancestry: 8_192,
    };
    let deadline = Instant::now() + Duration::from_secs(300);
    let assessment = loop {
        match chain
            .finalized_deal_evidence_resumable_from(
                &journal,
                &prepared.deal_nullifier,
                from_block,
                budget,
            )
            .await?
        {
            HistoricalObservation::Complete { evidence, .. } => {
                let assessment = coordinator.reconcile(operation_ref, &evidence, now + 1)?;
                // Monad's finalized anchor trails the head; a winner is not payment-final
                // until its block is at or below it. Re-observe until it is.
                if matches!(assessment.state, DealState::PaidFinalized { .. }) {
                    break assessment;
                }
                if Instant::now() >= deadline {
                    return Err(
                        "payment has not finalized; retain state and reconcile before retrying"
                            .into(),
                    );
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            HistoricalObservation::Pending { .. } if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            HistoricalObservation::Pending { .. } => {
                return Err("observation did not complete".into())
            }
        }
    };

    println!(
        "{}",
        json!({
            "transaction": format!("0x{}", hex::encode(broadcast.hash)),
            "deal_commitment": commitment.to_hex(),
            "deal_nullifier": prepared.deal_nullifier.to_hex(),
            "deal_state": format!("{:?}", assessment.state),
            "payment_finalized": matches!(
                assessment.state,
                erebus_core::deal_state::DealState::PaidFinalized { .. }
            ),
        })
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proposal_files_are_private_durable_and_never_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("proposal.json");
        private_write(&path, b"private opening").unwrap();
        assert_eq!(
            private_read(&path, 64).unwrap().as_slice(),
            b"private opening"
        );
        assert!(private_write(&path, b"other opening").is_err());
        assert!(private_read(&path, 1).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::{symlink, PermissionsExt};
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            let link = root.path().join("link");
            symlink(&path, &link).unwrap();
            assert!(private_read(&link, 64).is_err());
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(private_read(&path, 64).is_err());
        }
    }
}
