//! End-to-end M3 evidence: one authorized agreement settles exactly once on a local EVM chain.
//!
//! These tests spawn `anvil`, deploy the contracts from `contracts/evm`, and drive the real
//! adapter against real JSON-RPC. They require Foundry, so they are `#[ignore]`d by default and
//! run in the dedicated CI job that installs it:
//!
//! ```text
//! cd contracts/evm && forge build
//! cd ../../sdk/evm && cargo test -- --ignored
//! ```

#[path = "support/funded_faults.rs"]
mod funded_faults;

use std::convert::Infallible;
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy::network::{EthereumWallet, TransactionBuilder};
use alloy::primitives::{Address, Bytes, U64};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use k256::ecdsa::SigningKey;

use erebus_coordinator::{Coordinator, Stage};
use erebus_core::auth::{authorization_digest, Authorization, Role};
use erebus_core::commitment::{commit_agreement, CommitmentBlinding, DealNullifier};
use erebus_core::deal_state::{DealEvidence, DealState};
use erebus_core::domain::DeploymentDomain;
use erebus_core::ids::{
    AddressBytes, AssetId, BaseUnits, ChainNamespace, KeyBytes, SignatureBytes,
};
use erebus_core::policy::{ReservationState, SpendingPolicy};
use erebus_core::service::ServiceRecord;
use erebus_core::settlement::{BackendCapabilities, PreparedSettlement, SettlementContext};
use erebus_core::terms::{
    AgreementTerms, FeePolicy, Guarantee, GuaranteeSet, SettlementMode, CURRENT_PROTOCOL_VERSION,
};
use erebus_evm::backend::EvmSettlementBackend;
use erebus_evm::chain::{
    BroadcastOutcome, Eip1559Fees, EvmChain, ObservationLimits, SignerJournal, SigningPlan,
    TransactionKey,
};
use erebus_evm::deployment::EvmDeployment;
use erebus_evm::error::EvmError;
use erebus_evm::evidence::SettlementEvidence;
use erebus_evm::{abi, backend::SettlementEstimate};

/// Anvil account 0: the deal payer. Its key is public test-chain material, not a secret.
const BUYER_KEY: [u8; 32] =
    hex_literal("ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80");
/// Anvil account 1: the transaction signer (relayer). Pays gas, never the deal.
const RELAYER_KEY: [u8; 32] =
    hex_literal("59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d");
/// Anvil account 2: the payment recipient.
const RECIPIENT: [u8; 20] = hex_literal("3c44cdddb6a900fa2b585dd299e03d12fa4293bc");
/// Anvil account 3: the fee recipient.
const FEE_RECIPIENT: [u8; 20] = hex_literal("90f79bf6eb2c4f870365e785982e1f101e93b906");

const AMOUNT: u128 = 1_000_000;
const FEE: u128 = 25_000;

const fn hex_literal<const N: usize>(value: &str) -> [u8; N] {
    const fn nibble(byte: u8) -> u8 {
        match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            _ => panic!("hex_literal takes lowercase hex"),
        }
    }
    let bytes = value.as_bytes();
    assert!(bytes.len() == N * 2, "hex literal has the wrong length");
    let mut out = [0u8; N];
    let mut i = 0;
    while i < N {
        out[i] = (nibble(bytes[i * 2]) << 4) | nibble(bytes[i * 2 + 1]);
        i += 1;
    }
    out
}

struct Anvil {
    child: Child,
    rpc_url: String,
}

impl Drop for Anvil {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Anvil {
    async fn start() -> Self {
        let port = free_port();
        let mut child = Command::new("anvil")
            .args([
                "--port",
                &port.to_string(),
                "--chain-id",
                "31337",
                // Shorten the `finalized` lag so observation tests need only a few mined blocks.
                "--slots-in-an-epoch",
                "1",
                "--silent",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("anvil must be installed: https://getfoundry.sh");
        let rpc_url = format!("http://127.0.0.1:{port}");
        let probe = read_only_provider(&rpc_url);
        for _ in 0..100 {
            if probe.get_chain_id().await.is_ok() {
                return Self { child, rpc_url };
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let _ = child.kill();
        let _ = child.wait();
        panic!("anvil did not become ready");
    }
}

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback bind");
    listener.local_addr().expect("local addr").port()
}

fn provider(rpc_url: &str, key: &[u8; 32]) -> DynProvider {
    let signer = PrivateKeySigner::from_slice(key).expect("valid anvil key");
    let wallet = EthereumWallet::from(signer);
    ProviderBuilder::new()
        .wallet(wallet)
        .connect_http(rpc_url.parse().expect("url"))
        .erased()
}

fn read_only_provider(rpc_url: &str) -> DynProvider {
    ProviderBuilder::new()
        .connect_http(rpc_url.parse().expect("url"))
        .erased()
}

struct Artifact {
    bytecode: Vec<u8>,
}

fn load_artifact(name: &str) -> Artifact {
    let path = format!(
        "{}/../../contracts/evm/out/{name}.sol/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let document: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {path}: {error}; run `forge build` first")),
    )
    .expect("artifact json");
    let object = document["bytecode"]["object"]
        .as_str()
        .expect("artifact has bytecode");
    Artifact {
        bytecode: hex::decode(object.trim_start_matches("0x")).expect("hex bytecode"),
    }
}

async fn deploy(provider: &DynProvider, name: &str, constructor: Vec<u8>) -> Address {
    let artifact = load_artifact(name);
    let mut code = artifact.bytecode;
    code.extend_from_slice(&constructor);
    let request = TransactionRequest::default().with_deploy_code(Bytes::from(code));
    let pending = provider
        .send_transaction(request)
        .await
        .expect("deployment transaction");
    let receipt = pending.get_receipt().await.expect("deployment receipt");
    receipt.contract_address.expect("deployed address")
}

async fn send(provider: &DynProvider, to: Address, calldata: Vec<u8>) -> [u8; 32] {
    let request = TransactionRequest::default()
        .with_to(to)
        .with_input(Bytes::from(calldata));
    let pending = provider
        .send_transaction(request)
        .await
        .expect("transaction");
    let hash = pending.tx_hash().0;
    pending.get_receipt().await.expect("receipt");
    hash
}

async fn token_balance(provider: &DynProvider, token: Address, account: &[u8; 20]) -> u128 {
    let request = TransactionRequest::default()
        .with_to(token)
        .with_input(Bytes::from(abi::encode_balance_of_call(account)));
    let result = provider.call(request).await.expect("balance call");
    let mut word = [0u8; 32];
    word.copy_from_slice(&result[result.len() - 32..]);
    u128::from_be_bytes(word[16..].try_into().expect("low 16 bytes"))
}

fn authorization(
    role: Role,
    terms: &AgreementTerms,
    commitment: &erebus_core::commitment::DealCommitment,
    key: &SigningKey,
) -> Authorization {
    let digest = authorization_digest(&terms.domain, role, commitment, terms.suite_id)
        .expect("authorization digest");
    let (signature, recovery_id) = key
        .sign_prehash_recoverable(&digest)
        .expect("signing a fixed digest cannot fail");
    let mut bytes = [0u8; 65];
    bytes[..64].copy_from_slice(&signature.to_bytes());
    bytes[64] = recovery_id.to_byte();
    Authorization {
        role,
        suite_id: terms.suite_id,
        commitment: *commitment,
        signature: SignatureBytes::new(bytes.to_vec()).expect("65-byte signature"),
    }
}

struct Fixture {
    _anvil: Anvil,
    verification_proxy: FaultProxy,
    rpc_url: String,
    settlement: Address,
    token: Address,
    terms: AgreementTerms,
    blinding: CommitmentBlinding,
    buyer_key: SigningKey,
    seller_key: SigningKey,
}

fn address_bytes(address: Address) -> [u8; 20] {
    address.0 .0
}

fn terms_for(
    settlement: Address,
    token: Address,
    buyer: [u8; 20],
    seller: [u8; 20],
    expiry: u64,
) -> AgreementTerms {
    let namespace = ChainNamespace::new("eip155", "31337").expect("namespace");
    let asset = AssetId::new(
        namespace.clone(),
        "erc20",
        &format!("0x{}", hex::encode(token)),
    )
    .expect("asset");
    AgreementTerms {
        protocol_version: CURRENT_PROTOCOL_VERSION,
        suite_id: 1,
        domain: DeploymentDomain {
            namespace,
            settlement_contract: Some(AddressBytes::new(settlement.to_vec()).expect("address")),
            pool: None,
            verifier_version: 1,
        },
        deal_id: [0x42; 16],
        revision: 1,
        transcript_root: [0x11; 32],
        buyer_authorization_key: KeyBytes::new(buyer.to_vec()).expect("key"),
        seller_authorization_key: KeyBytes::new(seller.to_vec()).expect("key"),
        payment_recipient: KeyBytes::new(RECIPIENT.to_vec()).expect("key"),
        asset,
        amount: BaseUnits::new(AMOUNT),
        expiry,
        fee_policy: FeePolicy {
            fee: BaseUnits::new(FEE),
            recipient: Some(KeyBytes::new(FEE_RECIPIENT.to_vec()).expect("key")),
        },
        settlement_mode: SettlementMode::PublicBound,
        required_guarantees: GuaranteeSet::from_guarantee(Guarantee::AgreementBoundSettlement),
        settlement_nonce: [0x77; 32],
        service: ServiceRecord {
            resource: "gpu.h100.hour".to_owned(),
            quantity: BaseUnits::new(500),
            unit: "gpu-hour".to_owned(),
            access_recipient: KeyBytes::new(buyer.to_vec()).expect("key"),
            delivery_deadline: expiry + 3_600,
            fulfillment_method: "http-access".to_owned(),
            fulfillment_digest: [0u8; 32],
        },
    }
}

async fn fixture() -> Fixture {
    let anvil = Anvil::start().await;
    let rpc_url = anvil.rpc_url.clone();
    let deployer = provider(&rpc_url, &RELAYER_KEY);

    let settlement = deploy(
        &deployer,
        "ErebusSettlement",
        abi::encode_settlement_constructor(31_337, 1),
    )
    .await;
    let token = deploy(
        &deployer,
        "MockERC20",
        abi::encode_token_constructor("Test Token", "TST"),
    )
    .await;

    let buyer = Address::from(BUYER_KEY_ADDRESS);
    send(
        &deployer,
        token,
        abi::encode_mint_call(&address_bytes(buyer), AMOUNT + FEE),
    )
    .await;
    let buyer_provider = provider(&rpc_url, &BUYER_KEY);
    send(
        &buyer_provider,
        token,
        abi::encode_approve_call(&address_bytes(settlement), AMOUNT + FEE),
    )
    .await;

    let buyer_key = SigningKey::from_slice(&BUYER_KEY).expect("buyer key");
    let seller_key = SigningKey::from_slice(&[0x5e; 32]).expect("seller key");
    let seller_address = address_recover(&seller_key);

    let expiry = now() + 3_600;
    let terms = terms_for(
        settlement,
        token,
        address_bytes(buyer),
        seller_address,
        expiry,
    );
    let verification_proxy =
        FaultProxy::start(rpc_url.strip_prefix("http://").unwrap().to_owned()).await;
    Fixture {
        _anvil: anvil,
        verification_proxy,
        rpc_url,
        settlement,
        token,
        terms,
        blinding: CommitmentBlinding::from_bytes([0x0a; 32]),
        buyer_key,
        seller_key,
    }
}

const BUYER_KEY_ADDRESS: [u8; 20] = hex_literal("f39fd6e51aad88f6f4ce6ab8827279cfffb92266");

fn address_recover(key: &SigningKey) -> [u8; 20] {
    use k256::ecdsa::VerifyingKey;
    let verifying: &VerifyingKey = key.verifying_key();
    let point = verifying.to_encoded_point(false);
    let digest = keccak256(&point.as_bytes()[1..]);
    let mut address = [0u8; 20];
    address.copy_from_slice(&digest[12..]);
    address
}

fn keccak256(bytes: &[u8]) -> [u8; 32] {
    use sha3::Digest;
    sha3::Keccak256::digest(bytes).into()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

fn deployment(fixture: &Fixture) -> EvmDeployment {
    EvmDeployment::new(
        ChainNamespace::new("eip155", "31337").expect("namespace"),
        address_bytes(fixture.settlement),
        1,
        fixture.rpc_url.clone(),
    )
    .expect("deployment")
}

fn prepared(fixture: &Fixture) -> PreparedSettlement {
    let commitment = commit_agreement(&fixture.terms, &fixture.blinding).expect("commitment");
    let buyer = authorization(Role::Buyer, &fixture.terms, &commitment, &fixture.buyer_key);
    let seller = authorization(
        Role::Seller,
        &fixture.terms,
        &commitment,
        &fixture.seller_key,
    );
    let backend =
        EvmSettlementBackend::connect(deployment(fixture), &RELAYER_KEY).expect("backend");
    backend
        .prepare(
            &fixture.terms,
            &fixture.blinding,
            &buyer,
            &seller,
            [0x07; 32],
        )
        .expect("prepare")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn one_authorized_agreement_settles_exactly_once() {
    let fixture = fixture().await;
    let backend =
        EvmSettlementBackend::connect(deployment(&fixture), &RELAYER_KEY).expect("backend");
    let prepared = prepared(&fixture);

    let commitment = commit_agreement(&fixture.terms, &fixture.blinding).expect("commitment");
    let buyer = authorization(Role::Buyer, &fixture.terms, &commitment, &fixture.buyer_key);
    let seller = authorization(
        Role::Seller,
        &fixture.terms,
        &commitment,
        &fixture.seller_key,
    );
    let estimate: SettlementEstimate = backend
        .estimate(
            &fixture.terms,
            &fixture.blinding,
            &buyer,
            &seller,
            [0x07; 32],
        )
        .await
        .expect("estimate");
    assert!(estimate.allowance_is_sufficient());
    assert!(estimate.balance_is_sufficient());
    assert!(estimate.gas > 21_000);

    let DealEvidence::Observed(reads) = settle_coordinated(&fixture, &RELAYER_KEY).await else {
        panic!("verified payment evidence");
    };
    assert_eq!(reads.deal_nullifier, prepared.deal_nullifier);
    let winner = reads.winner.expect("winner");
    assert_eq!(winner.commitment, commitment);
    assert_eq!(winner.amount, fixture.terms.amount);
    assert!(winner.is_final);

    let reader = read_only_provider(&fixture.rpc_url);
    assert_eq!(
        token_balance(&reader, fixture.token, &RECIPIENT).await,
        AMOUNT
    );
    assert_eq!(
        token_balance(&reader, fixture.token, &FEE_RECIPIENT).await,
        FEE
    );
    assert_eq!(
        token_balance(&reader, fixture.token, &BUYER_KEY_ADDRESS).await,
        0
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn a_replay_is_rejected_on_chain() {
    let fixture = fixture().await;
    let prepared = prepared(&fixture);
    settle_coordinated(&fixture, &RELAYER_KEY).await;

    // The contract consumed the deal nullifier, so the same authorized revision cannot settle a
    // second time even though both authorizations are still valid and unexpired.
    assert_contract_reverts(
        &fixture,
        &SettlementEvidence::decode(&prepared.backend_evidence).unwrap(),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn auditor_opens_one_grant_and_verifies_final_payment_from_chain() {
    use erebus_evm::disclosure::{verify_public_bound_disclosure, DisclosureVerificationError};
    use erebus_transport::disclosure::{DisclosureGrant, SelectedAgreement};
    use erebus_transport::hashing::TRANSCRIPT_HASH_VERSION;
    use erebus_transport::identity::{AuthorizationIdentity, DisclosureIdentity};
    use erebus_transport::message::{Message, MessageType};
    use erebus_transport::transcript::Transcript;

    let mut fixture = fixture().await;
    let message = Message::new(
        [0x19; 32],
        fixture.terms.deal_id,
        fixture.terms.revision,
        Role::Buyer,
        1,
        [0; 32],
        MessageType::Offer,
        b"one gpu-hour".to_vec(),
    )
    .unwrap();
    let mut transcript = Transcript::new(fixture.terms.deal_id, TRANSCRIPT_HASH_VERSION).unwrap();
    transcript.append(&message).unwrap();
    fixture.terms.transcript_root = transcript.root().unwrap();
    let prepared = prepared(&fixture);
    let commitment = prepared.deal_commitment;
    let selected = SelectedAgreement {
        terms: fixture.terms.clone(),
        blinding: fixture.blinding.clone(),
        buyer: authorization(Role::Buyer, &fixture.terms, &commitment, &fixture.buyer_key),
        seller: authorization(
            Role::Seller,
            &fixture.terms,
            &commitment,
            &fixture.seller_key,
        ),
        transcript_hash_version: TRANSCRIPT_HASH_VERSION,
        messages: vec![message],
    };
    let issuer = AuthorizationIdentity::from_bytes(&[0x5e; 32]).unwrap();
    let auditor = DisclosureIdentity::generate().unwrap();
    let grant = DisclosureGrant::seal(
        &selected,
        &issuer,
        auditor.public_key(),
        now() + 3600,
        now(),
    )
    .unwrap();
    let chain = EvmChain::connect(deployment(&fixture), Duration::from_secs(5))
        .await
        .unwrap();
    let before = verify_public_bound_disclosure(
        &grant,
        &auditor,
        issuer.address(),
        now(),
        &chain,
        ObservationLimits::default(),
    )
    .await;
    assert!(matches!(before, Err(DisclosureVerificationError::Payment)));

    settle_coordinated(&fixture, &RELAYER_KEY).await;
    mine_blocks(&read_only_provider(&fixture.rpc_url), 3).await;
    let verified = verify_public_bound_disclosure(
        &grant,
        &auditor,
        issuer.address(),
        now(),
        &chain,
        ObservationLimits::default(),
    )
    .await
    .unwrap();
    assert_eq!(verified.agreement.commitment, prepared.deal_commitment);
    assert_eq!(verified.evidence.terms.amount, fixture.terms.amount);

    let outsider = AuthorizationIdentity::from_bytes(&[0x33; 32]).unwrap();
    let outsider_grant = DisclosureGrant::seal(
        &selected,
        &outsider,
        auditor.public_key(),
        now() + 3600,
        now(),
    )
    .unwrap();
    assert!(matches!(
        verify_public_bound_disclosure(
            &outsider_grant,
            &auditor,
            outsider.address(),
            now(),
            &chain,
            ObservationLimits::default(),
        )
        .await,
        Err(DisclosureVerificationError::Payment)
    ));

    let mut changed = selected;
    changed.terms.amount = BaseUnits::new(AMOUNT + 1);
    let changed_commitment = commit_agreement(&changed.terms, &changed.blinding).unwrap();
    changed.buyer = authorization(
        Role::Buyer,
        &changed.terms,
        &changed_commitment,
        &fixture.buyer_key,
    );
    changed.seller = authorization(
        Role::Seller,
        &changed.terms,
        &changed_commitment,
        &fixture.seller_key,
    );
    let changed_grant =
        DisclosureGrant::seal(&changed, &issuer, auditor.public_key(), now() + 3600, now())
            .unwrap();
    let wrong_payment = verify_public_bound_disclosure(
        &changed_grant,
        &auditor,
        issuer.address(),
        now(),
        &chain,
        ObservationLimits::default(),
    )
    .await;
    assert!(matches!(
        wrong_payment,
        Err(DisclosureVerificationError::Payment)
    ));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn an_independent_auditor_process_verifies_without_participant_keys_or_relay() {
    use erebus_transport::disclosure::{DisclosureGrant, SelectedAgreement};
    use erebus_transport::hashing::TRANSCRIPT_HASH_VERSION;
    use erebus_transport::identity::{AuthorizationIdentity, DisclosureIdentity};
    use erebus_transport::message::{Message, MessageType};
    use erebus_transport::transcript::Transcript;
    use std::io::Write;

    let mut fixture = fixture().await;
    let message = Message::new(
        [0x19; 32],
        fixture.terms.deal_id,
        1,
        Role::Buyer,
        1,
        [0; 32],
        MessageType::Offer,
        b"one gpu-hour".to_vec(),
    )
    .unwrap();
    let mut transcript = Transcript::new(fixture.terms.deal_id, TRANSCRIPT_HASH_VERSION).unwrap();
    transcript.append(&message).unwrap();
    fixture.terms.transcript_root = transcript.root().unwrap();
    let prepared = prepared(&fixture);
    let selected = SelectedAgreement {
        terms: fixture.terms.clone(),
        blinding: fixture.blinding.clone(),
        buyer: authorization(
            Role::Buyer,
            &fixture.terms,
            &prepared.deal_commitment,
            &fixture.buyer_key,
        ),
        seller: authorization(
            Role::Seller,
            &fixture.terms,
            &prepared.deal_commitment,
            &fixture.seller_key,
        ),
        transcript_hash_version: TRANSCRIPT_HASH_VERSION,
        messages: vec![message],
    };
    let issuer = AuthorizationIdentity::from_bytes(&[0x5e; 32]).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let auditor_key = directory.path().join("auditor.key");
    let auditor = DisclosureIdentity::generate_and_store(&auditor_key).unwrap();
    let grant = DisclosureGrant::seal(
        &selected,
        &issuer,
        auditor.public_key(),
        now() + 3600,
        now(),
    )
    .unwrap();
    let grant_file = directory.path().join("deal.grant");
    grant.write_backup(&grant_file).unwrap();
    settle_coordinated(&fixture, &RELAYER_KEY).await;
    mine_blocks(&read_only_provider(&fixture.rpc_url), 3).await;

    // The child sees only the auditor's key, encrypted grant, and public chain endpoint.
    let output = Command::new(std::env::current_exe().unwrap())
        .current_dir(directory.path())
        .env_clear()
        .arg("--exact")
        .arg("disclosure_subprocess_worker")
        .arg("--nocapture")
        .env("EREBUS_DISCLOSURE_WORKER", "1")
        .env("EREBUS_DISCLOSURE_GRANT", &grant_file)
        .env("EREBUS_DISCLOSURE_KEY", &auditor_key)
        .env("EREBUS_DISCLOSURE_RPC", &fixture.rpc_url)
        .env(
            "EREBUS_DISCLOSURE_CONTRACT",
            hex::encode(fixture.settlement),
        )
        .env("EREBUS_DISCLOSURE_ISSUER", hex::encode(issuer.address()))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("disclosure_subprocess_worker ... ok"));

    let binary = std::env::var_os("EREBUS_TEST_DISCLOSURE_BIN")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_erebus-disclosure").into());
    let mut child = Command::new(binary)
        .current_dir(directory.path())
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let request = serde_json::json!({
        "method": "verify_payment",
        "grant_file": "deal.grant",
        "key_file": "auditor.key",
        "expected_issuer": format!("0x{}", hex::encode(issuer.address())),
        "deployment": {
            "namespace": "eip155:31337",
            "settlement_contract": format!("0x{}", hex::encode(fixture.settlement)),
            "verifier_version": 1,
            "rpc_url": fixture.rpc_url,
        },
    });
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&request).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.stderr.is_empty());
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(output.status.success(), "{response}");
    assert_eq!(response["agreement_verified"], true);
    assert_eq!(response["payment_verified"], true);
    assert_eq!(response["delivery_verified"], false);
    assert_eq!(response["deal_id"], hex::encode(fixture.terms.deal_id));
}

#[tokio::test(flavor = "multi_thread")]
async fn disclosure_subprocess_worker() {
    use erebus_evm::disclosure::verify_public_bound_disclosure;
    use erebus_transport::disclosure::DisclosureGrant;
    use erebus_transport::identity::DisclosureIdentity;

    if std::env::var_os("EREBUS_DISCLOSURE_WORKER").is_none() {
        return;
    }
    let key = DisclosureIdentity::load(std::env::var("EREBUS_DISCLOSURE_KEY").unwrap()).unwrap();
    let grant =
        DisclosureGrant::load_backup(std::env::var("EREBUS_DISCLOSURE_GRANT").unwrap()).unwrap();
    let contract: [u8; 20] = hex::decode(std::env::var("EREBUS_DISCLOSURE_CONTRACT").unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let issuer: [u8; 20] = hex::decode(std::env::var("EREBUS_DISCLOSURE_ISSUER").unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let deployment = EvmDeployment::new(
        ChainNamespace::new("eip155", "31337").unwrap(),
        contract,
        1,
        std::env::var("EREBUS_DISCLOSURE_RPC").unwrap(),
    )
    .unwrap();
    let chain = EvmChain::connect(deployment, Duration::from_secs(5))
        .await
        .unwrap();
    let verified = verify_public_bound_disclosure(
        &grant,
        &key,
        issuer,
        now(),
        &chain,
        ObservationLimits::default(),
    )
    .await
    .unwrap();
    assert_eq!(verified.evidence.terms.deal_id, verified.agreement.deal_id);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn the_contract_rejects_mutated_terms_and_expiry() {
    let mut fixture = fixture().await;
    let original = SettlementEvidence::decode(&prepared(&fixture).backend_evidence).unwrap();
    let mut mutated_terms = fixture.terms.clone();
    mutated_terms.amount = BaseUnits::new(AMOUNT + 1);
    let mutated = SettlementEvidence {
        terms: mutated_terms.encode().unwrap(),
        ..original
    };
    assert_contract_reverts(&fixture, &mutated).await;

    // Re-authorize genuinely expired terms so the contract's expiry rule, not signature
    // mismatch or local preparation, is the reason execution fails.
    fixture.terms.expiry = now() - 1;
    let expired = SettlementEvidence::decode(&prepared(&fixture).backend_evidence).unwrap();
    assert_contract_reverts(&fixture, &expired).await;
    let reader = read_only_provider(&fixture.rpc_url);
    assert_eq!(token_balance(&reader, fixture.token, &RECIPIENT).await, 0);
    assert_eq!(
        token_balance(&reader, fixture.token, &FEE_RECIPIENT).await,
        0
    );
    assert_eq!(
        token_balance(&reader, fixture.token, &BUYER_KEY_ADDRESS).await,
        AMOUNT + FEE
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn an_unrelated_successful_receipt_is_not_settlement_evidence() {
    let fixture = fixture().await;
    let chain = EvmChain::connect(deployment(&fixture), Duration::from_secs(5))
        .await
        .unwrap();
    let mut peer_deployment = deployment(&fixture);
    peer_deployment.rpc_url = fixture.verification_proxy.url.clone();
    let peer = EvmChain::connect(peer_deployment, Duration::from_secs(5))
        .await
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let (coordinator, _, prepared, _) = coordinated_setup(&fixture, root.path()).await;
    let relayer = provider(&fixture.rpc_url, &FOREIGN_KEY);
    let unrelated = send(&relayer, Address::from(RECIPIENT), Vec::new()).await;
    assert!(matches!(
        chain.observe(unrelated).await.unwrap(),
        erebus_evm::chain::TxStatus::Included(inclusion) if inclusion.success
    ));
    mine_blocks(&read_only_provider(&fixture.rpc_url), 3).await;
    let evidence = chain
        .finalized_deal_evidence_agreed(
            &peer,
            &prepared.deal_nullifier,
            ObservationLimits::default(),
        )
        .await
        .unwrap();
    assert!(
        matches!(&evidence, DealEvidence::Observed(reads) if !reads.consumed_at_head && reads.winner.is_none())
    );
    coordinator
        .reconcile(prepared.operation_ref, &evidence, now())
        .unwrap();
    assert_eq!(
        coordinator.ledger().unwrap().reservations()[0].state,
        ReservationState::Reserved
    );
    assert_ne!(
        coordinator.diagnostics().unwrap()[0].stage,
        Stage::Finalized
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn the_rpc_chain_must_match_the_configured_chain() {
    let fixture = fixture().await;
    let mut wrong_chain = deployment(&fixture);
    wrong_chain.chain_id = 1;
    wrong_chain.namespace = ChainNamespace::new("eip155", "1").unwrap();
    assert!(matches!(
        EvmChain::connect(wrong_chain, Duration::from_secs(5)).await,
        Err(EvmError::ChainIdMismatch {
            expected: 1,
            found: 31_337
        })
    ));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn prepared_routing_fields_must_match_the_evidence() {
    use erebus_evm::chain::TransactionParams;
    let fixture = fixture().await;
    let prepared = PreparedSettlement {
        deal_nullifier: DealNullifier::from_bytes([0xFF; 32]),
        ..prepared(&fixture)
    };
    let key = TransactionKey::from_bytes(&RELAYER_KEY).unwrap();
    let reader = read_only_provider(&fixture.rpc_url);
    let nonce_before = reader
        .get_transaction_count(Address::from(key.address()))
        .await
        .unwrap();
    let plan = SigningPlan::new(
        key.address(),
        TransactionParams {
            nonce: 0,
            fees: Eip1559Fees::new(2_000_000_000, 1_000_000_000).unwrap(),
            gas_limit: 1_000_000,
        },
    )
    .unwrap();
    assert!(matches!(
        plan.sign(&deployment(&fixture), &prepared, &key),
        Err(EvmError::PreparedMismatch)
    ));
    assert_eq!(
        reader
            .get_transaction_count(Address::from(key.address()))
            .await
            .unwrap(),
        nonce_before
    );
}

#[test]
fn prepare_rejects_terms_that_do_not_match_their_authorizations() {
    let buyer_key = SigningKey::from_slice(&BUYER_KEY).expect("buyer key");
    let seller_key = SigningKey::from_slice(&[0x5e; 32]).expect("seller key");
    let namespace = ChainNamespace::new("eip155", "31337").expect("namespace");
    let token: [u8; 20] = hex_literal("00000000000000000000000000000000000000aa");
    let asset = AssetId::new(
        namespace.clone(),
        "erc20",
        &format!("0x{}", hex::encode(token)),
    )
    .expect("asset");
    let terms = AgreementTerms {
        protocol_version: CURRENT_PROTOCOL_VERSION,
        suite_id: 1,
        domain: DeploymentDomain {
            namespace,
            settlement_contract: Some(AddressBytes::new(vec![0x11; 20]).expect("address")),
            pool: None,
            verifier_version: 1,
        },
        deal_id: [0x42; 16],
        revision: 1,
        transcript_root: [0x11; 32],
        buyer_authorization_key: KeyBytes::new(address_recover(&buyer_key).to_vec()).expect("key"),
        seller_authorization_key: KeyBytes::new(address_recover(&seller_key).to_vec())
            .expect("key"),
        payment_recipient: KeyBytes::new(RECIPIENT.to_vec()).expect("key"),
        asset,
        amount: BaseUnits::new(AMOUNT),
        expiry: now() + 3_600,
        fee_policy: FeePolicy::none(),
        settlement_mode: SettlementMode::PublicBound,
        required_guarantees: GuaranteeSet::from_guarantee(Guarantee::AgreementBoundSettlement),
        settlement_nonce: [0x77; 32],
        service: ServiceRecord {
            resource: "gpu.h100.hour".to_owned(),
            quantity: BaseUnits::new(500),
            unit: "gpu-hour".to_owned(),
            access_recipient: KeyBytes::new(RECIPIENT.to_vec()).expect("key"),
            delivery_deadline: now() + 7_200,
            fulfillment_method: "http-access".to_owned(),
            fulfillment_digest: [0u8; 32],
        },
    };
    let blinding = CommitmentBlinding::from_bytes([0x0a; 32]);
    let commitment = commit_agreement(&terms, &blinding).expect("commitment");
    let buyer = authorization(Role::Buyer, &terms, &commitment, &buyer_key);
    let seller = authorization(Role::Seller, &terms, &commitment, &seller_key);

    let deployment = EvmDeployment::new(
        ChainNamespace::new("eip155", "31337").expect("namespace"),
        [0x11; 20],
        1,
        "http://127.0.0.1:1",
    )
    .expect("deployment");
    let backend = EvmSettlementBackend::connect(deployment, &RELAYER_KEY).expect("backend");

    // A well-formed agreement prepares; a mutated one fails locally with no RPC call, which the
    // unreachable RPC URL proves: prepare never touches the network.
    backend
        .prepare(&terms, &blinding, &buyer, &seller, [0x01; 32])
        .expect("valid terms prepare");
    let mut mutated = terms.clone();
    mutated.amount = BaseUnits::new(AMOUNT + 1);
    assert!(backend
        .prepare(&mutated, &blinding, &buyer, &seller, [0x01; 32])
        .is_err());

    let mut wrong_role = buyer.clone();
    wrong_role.role = Role::Seller;
    assert!(backend
        .prepare(&terms, &blinding, &wrong_role, &seller, [0x01; 32])
        .is_err());

    let mut unsupported = terms.clone();
    unsupported
        .required_guarantees
        .insert(Guarantee::ScopedDisclosure);
    let unsupported_commitment =
        commit_agreement(&unsupported, &blinding).expect("unsupported commitment");
    let unsupported_buyer = authorization(
        Role::Buyer,
        &unsupported,
        &unsupported_commitment,
        &buyer_key,
    );
    let unsupported_seller = authorization(
        Role::Seller,
        &unsupported,
        &unsupported_commitment,
        &seller_key,
    );
    assert!(matches!(
        backend.prepare(
            &unsupported,
            &blinding,
            &unsupported_buyer,
            &unsupported_seller,
            [0x01; 32],
        ),
        Err(EvmError::Selection(_))
    ));
}

// ---------------------------------------------------------------------
// M6: coordinated lifecycle, reconciliation, and signer release
// ---------------------------------------------------------------------

/// Anvil account 2 as a foreign submitter key; public test-chain material.
const FOREIGN_KEY: [u8; 32] =
    hex_literal("5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a");

fn context_for(fixture: &Fixture) -> SettlementContext {
    SettlementContext {
        require_local_proving: true,
        mode: fixture.terms.settlement_mode,
        domain: fixture.terms.domain.clone(),
        suite_id: fixture.terms.suite_id,
        asset: fixture.terms.asset.clone(),
        required_guarantees: fixture.terms.required_guarantees,
    }
}

fn policy_for(fixture: &Fixture) -> SpendingPolicy {
    let mut allowed_assets = std::collections::BTreeSet::new();
    allowed_assets.insert(fixture.terms.asset.clone());
    SpendingPolicy {
        per_deal_max: BaseUnits::new(AMOUNT + FEE),
        allowed_assets,
        ..SpendingPolicy::default()
    }
}

fn capabilities_for(fixture: &Fixture) -> BackendCapabilities {
    EvmSettlementBackend::connect(deployment(fixture), &RELAYER_KEY)
        .expect("backend")
        .capabilities()
}

async fn mine_blocks(provider: &DynProvider, blocks: u64) {
    let _: serde_json::Value = provider
        .client()
        .request("anvil_mine", (U64::from(blocks),))
        .await
        .expect("anvil_mine");
}

/// Drives the durable buyer lifecycle up to a signed, persisted transaction.
///
/// Returns the coordinator, the signer journal, the prepared settlement, and the signing plan.
async fn coordinated_setup(
    fixture: &Fixture,
    state_root: &std::path::Path,
) -> (Coordinator, SignerJournal, PreparedSettlement, SigningPlan) {
    coordinated_setup_with_key(fixture, state_root, &RELAYER_KEY).await
}

async fn coordinated_setup_with_key(
    fixture: &Fixture,
    state_root: &std::path::Path,
    signing_key: &[u8; 32],
) -> (Coordinator, SignerJournal, PreparedSettlement, SigningPlan) {
    let deployment = deployment(fixture);
    let chain = EvmChain::connect(deployment.clone(), Duration::from_secs(5))
        .await
        .expect("chain");
    let prepared = prepared(fixture);
    let now = now();

    let context = context_for(fixture);
    let coordinator = Coordinator::open(
        state_root.join("coordinator"),
        KeyBytes::new(BUYER_KEY_ADDRESS.to_vec()).expect("buyer key"),
        context.clone(),
        &context,
        &capabilities_for(fixture),
        policy_for(fixture),
    )
    .expect("coordinator");

    let signer = address_recover(&SigningKey::from_slice(signing_key).expect("gas payer key"));
    let journal = SignerJournal::open(state_root.join("signer"), 31_337, signer).expect("journal");
    let fees = Eip1559Fees::new(2_000_000_000, 1_000_000_000).expect("fees");
    let plan = chain
        .reserve_nonce(&journal, &prepared, fees, 1_000_000)
        .await
        .expect("nonce");

    let commitment = commit_agreement(&fixture.terms, &fixture.blinding).expect("commitment");
    let buyer = authorization(Role::Buyer, &fixture.terms, &commitment, &fixture.buyer_key);
    let seller = authorization(
        Role::Seller,
        &fixture.terms,
        &commitment,
        &fixture.seller_key,
    );
    let operation_ref = prepared.operation_ref;

    coordinator
        .record_intent(operation_ref, &fixture.terms, &fixture.blinding, now)
        .expect("intent");
    coordinator
        .authorize_buyer(operation_ref, now, |_, _| {
            Ok::<_, Infallible>(buyer.clone())
        })
        .expect("buyer authorization");
    coordinator
        .accept_seller(operation_ref, &seller)
        .expect("seller authorization");
    coordinator
        .prepare(operation_ref, now, |terms, blinding, buyer, seller| {
            EvmSettlementBackend::connect(deployment.clone(), &RELAYER_KEY)
                .expect("backend")
                .prepare(terms, blinding, buyer, seller, operation_ref)
        })
        .expect("prepare");

    let key = TransactionKey::from_bytes(signing_key).expect("transaction key");
    coordinator
        .sign_transaction(
            operation_ref,
            now,
            &plan.encode(),
            |prepared, plan_bytes| {
                let plan = SigningPlan::decode(plan_bytes)?;
                plan.sign(&deployment, prepared, &key)
                    .map(|transaction| transaction.raw().to_vec())
            },
            |prepared, plan_bytes, raw| {
                let plan = SigningPlan::decode(plan_bytes)?;
                plan.validate(&deployment, prepared, raw)
            },
        )
        .expect("sign");

    (coordinator, journal, prepared, plan)
}

/// Successful payments use durable signing/broadcast and paired finalized evidence.
/// The two test URLs share Anvil; this checks mechanics, not provider independence.
async fn settle_coordinated(fixture: &Fixture, signing_key: &[u8; 32]) -> DealEvidence {
    use erebus_evm::chain::{HistoricalObservation, ObservationJournal};

    let root = tempfile::tempdir().unwrap();
    let (coordinator, signer_journal, prepared, _) =
        coordinated_setup_with_key(fixture, root.path(), signing_key).await;
    let chain = EvmChain::connect(deployment(fixture), Duration::from_secs(5))
        .await
        .unwrap();
    let mut peer_deployment = deployment(fixture);
    peer_deployment.rpc_url = fixture.verification_proxy.url.clone();
    let peer = EvmChain::connect(peer_deployment, Duration::from_secs(5))
        .await
        .unwrap();
    let broadcast = chain
        .broadcast_journaled(&coordinator, prepared.operation_ref, now())
        .await
        .unwrap();
    assert_eq!(broadcast.outcome, BroadcastOutcome::Acknowledged);
    mine_blocks(&read_only_provider(&fixture.rpc_url), 3).await;
    let primary_history = ObservationJournal::open(root.path().join("primary-history")).unwrap();
    let peer_history = ObservationJournal::open(root.path().join("peer-history")).unwrap();
    let evidence = loop {
        match chain
            .finalized_deal_evidence_resumable_agreed(
                &primary_history,
                &peer,
                &peer_history,
                &prepared.deal_nullifier,
                ObservationLimits::default(),
            )
            .await
            .unwrap()
        {
            HistoricalObservation::Pending { .. } => continue,
            HistoricalObservation::Complete { evidence, .. } => break evidence,
        }
    };
    let assessment = coordinator
        .reconcile(prepared.operation_ref, &evidence, now())
        .unwrap();
    assert!(matches!(assessment.state, DealState::PaidFinalized { .. }));
    assert_eq!(
        coordinator.diagnostics().unwrap()[0].stage,
        Stage::Finalized
    );
    assert_eq!(
        coordinator.ledger().unwrap().reservations()[0].state,
        ReservationState::Committed
    );
    let signer = TransactionKey::from_bytes(signing_key).unwrap().address();
    signer_journal
        .release(
            &chain
                .verified_finalized_nonce_agreed(&peer, signer)
                .await
                .unwrap(),
        )
        .unwrap();
    evidence
}

/// Adversarial contract tests deliberately bypass SDK validation with a gas-limited call.
async fn assert_contract_reverts(fixture: &Fixture, evidence: &SettlementEvidence) {
    let reader = read_only_provider(&fixture.rpc_url);
    let before = reader
        .get_transaction_count(Address::from(BUYER_KEY_ADDRESS))
        .await
        .unwrap();
    let request = TransactionRequest::default()
        .with_to(fixture.settlement)
        .with_input(Bytes::from(abi::encode_settle_call(
            &evidence.terms,
            &evidence.blinding,
            &evidence.buyer_signature,
            &evidence.seller_signature,
            &address_bytes(fixture.token),
        )))
        .with_gas_limit(1_000_000);
    let receipt = provider(&fixture.rpc_url, &BUYER_KEY)
        .send_transaction(request)
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    assert!(
        !receipt.status(),
        "the contract must revert after actual inclusion"
    );
    assert!(receipt.logs().is_empty());
    assert_eq!(
        reader
            .get_transaction_count(Address::from(BUYER_KEY_ADDRESS))
            .await
            .unwrap(),
        before + 1
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn coordinated_settlement_finalizes_commits_and_releases_the_signer() {
    let fixture = fixture().await;
    let deployment = deployment(&fixture);
    let chain = EvmChain::connect(deployment.clone(), Duration::from_secs(5))
        .await
        .expect("chain");
    let state_root = tempfile::tempdir().expect("state dir");
    let (coordinator, journal, prepared, _plan) =
        coordinated_setup(&fixture, state_root.path()).await;
    let operation_ref = prepared.operation_ref;
    let now = now();

    let broadcast = chain
        .broadcast_journaled(&coordinator, operation_ref, now)
        .await
        .expect("broadcast");
    assert_eq!(broadcast.outcome, BroadcastOutcome::Acknowledged);

    // Advance past anvil's `finalized` lag, then reconcile from chain evidence only.
    mine_blocks(&read_only_provider(&fixture.rpc_url), 3).await;
    let evidence = chain
        .finalized_deal_evidence(&prepared.deal_nullifier, ObservationLimits::default())
        .await
        .expect("deal evidence");
    let assessment = coordinator
        .reconcile(operation_ref, &evidence, now + 1)
        .expect("reconcile");
    assert!(matches!(
        assessment.state,
        erebus_core::deal_state::DealState::PaidFinalized { .. }
    ));

    assert_eq!(
        coordinator.diagnostics().expect("diagnostics")[0].stage,
        Stage::Finalized
    );
    assert_eq!(
        coordinator.ledger().expect("ledger").reservations()[0].state,
        ReservationState::Committed
    );
    assert_eq!(
        token_balance(
            &read_only_provider(&fixture.rpc_url),
            fixture.token,
            &RECIPIENT
        )
        .await,
        AMOUNT
    );

    // The gas payer's claimed nonce is consumed at a finalized block, so the slot is free.
    let signer = address_recover(&SigningKey::from_slice(&RELAYER_KEY).expect("relayer key"));
    let finalized = chain
        .verified_finalized_nonce(signer)
        .await
        .expect("finalized nonce");
    journal.release(&finalized).expect("release");
    assert!(journal
        .resume(&deployment, &prepared)
        .expect("resume")
        .is_none());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn a_foreign_winner_resolves_an_unknown_attempt_and_holds_the_unconsumed_nonce() {
    let fixture = fixture().await;
    let deployment = deployment(&fixture);
    let chain = EvmChain::connect(deployment.clone(), Duration::from_secs(5))
        .await
        .expect("chain");
    let state_root = tempfile::tempdir().expect("state dir");
    let (coordinator, journal, prepared, _plan) =
        coordinated_setup(&fixture, state_root.path()).await;
    let operation_ref = prepared.operation_ref;
    let now = now();

    // A foreign submitter (the seller or another relayer) settles the same authorized revision
    // first. Our own transaction can never land, and we do not learn that from a broadcast
    // response: the attempt is recorded Unknown, exactly as a dropped response would be.
    settle_coordinated(&fixture, &FOREIGN_KEY).await;
    let attempt = coordinator
        .begin_broadcast_attempt(operation_ref, now)
        .expect("attempt");
    coordinator
        .finish_broadcast_attempt(
            operation_ref,
            &attempt.token,
            erebus_coordinator::BroadcastOutcome::Unknown,
        )
        .expect("unknown outcome");

    mine_blocks(&read_only_provider(&fixture.rpc_url), 3).await;
    let evidence = chain
        .finalized_deal_evidence(&prepared.deal_nullifier, ObservationLimits::default())
        .await
        .expect("deal evidence");
    coordinator
        .reconcile(operation_ref, &evidence, now + 1)
        .expect("reconcile");

    // The deal is resolved and paid exactly once, from the foreign transaction.
    assert_eq!(
        coordinator.diagnostics().expect("diagnostics")[0].stage,
        Stage::Finalized
    );
    assert_eq!(
        coordinator.ledger().expect("ledger").reservations()[0].state,
        ReservationState::Committed
    );
    let reader = read_only_provider(&fixture.rpc_url);
    assert_eq!(
        token_balance(&reader, fixture.token, &RECIPIENT).await,
        AMOUNT
    );
    assert_eq!(
        token_balance(&reader, fixture.token, &FEE_RECIPIENT).await,
        FEE
    );

    // Our gas payer's nonce was never consumed, so the signer slot stays held. A resolved deal
    // is not permission to reuse a nonce, and the claim must not be cleared on a timeout.
    let signer = address_recover(&SigningKey::from_slice(&RELAYER_KEY).expect("relayer key"));
    let finalized = chain
        .verified_finalized_nonce(signer)
        .await
        .expect("finalized nonce");
    assert!(matches!(
        journal.release(&finalized),
        Err(erebus_evm::chain::NonceError::NotConsumed)
    ));
    assert!(journal
        .resume(&deployment, &prepared)
        .expect("resume")
        .is_some());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn relayed_settlement_pays_the_published_fee_recipient() {
    let fixture = fixture().await;
    let deployment = deployment(&fixture);
    let prepared = prepared(&fixture);
    let evidence = SettlementEvidence::decode(&prepared.backend_evidence).expect("evidence");
    let now = now();

    let policy = erebus_evm::relay::RelayPolicy::new(
        deployment.clone(),
        FEE_RECIPIENT,
        [(address_bytes(fixture.token), FEE)].into_iter().collect(),
        3_600,
    )
    .expect("policy");

    let admitted = policy.admit(&evidence.encode(), now).expect("admit");
    assert_eq!(admitted.deal_commitment, prepared.deal_commitment);
    assert_eq!(admitted.deal_nullifier, prepared.deal_nullifier);

    assert_eq!(admitted.backend_evidence, prepared.backend_evidence);
    settle_coordinated(&fixture, &RELAYER_KEY).await;

    let reader = read_only_provider(&fixture.rpc_url);
    assert_eq!(
        token_balance(&reader, fixture.token, &FEE_RECIPIENT).await,
        FEE
    );
    assert_eq!(
        token_balance(&reader, fixture.token, &RECIPIENT).await,
        AMOUNT
    );
    assert_eq!(
        token_balance(&reader, fixture.token, &BUYER_KEY_ADDRESS).await,
        0
    );

    // The fee and its recipient are committed terms. A relayer that publishes a different fee
    // or a different recipient cannot admit the same signed deal, so it cannot redirect value.
    let wrong_recipient = erebus_evm::relay::RelayPolicy::new(
        deployment.clone(),
        [0x99; 20],
        [(address_bytes(fixture.token), FEE)].into_iter().collect(),
        3_600,
    )
    .expect("policy");
    assert_eq!(
        wrong_recipient.admit(&evidence.encode(), now),
        Err(erebus_evm::relay::RelayError::Fee)
    );
    let wrong_fee = erebus_evm::relay::RelayPolicy::new(
        deployment,
        FEE_RECIPIENT,
        [(address_bytes(fixture.token), FEE + 1)]
            .into_iter()
            .collect(),
        3_600,
    )
    .expect("policy");
    assert_eq!(
        wrong_fee.admit(&evidence.encode(), now),
        Err(erebus_evm::relay::RelayError::Fee)
    );
}

/// Where the simulated process death happens, after the named durable boundary completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CrashPoint {
    /// No crash; the lifecycle runs to completion in one process.
    None,
    /// The signed transaction is durable and the process dies before broadcast.
    AfterSigned,
    /// A broadcast attempt is recorded Unknown and the process dies before sending.
    AfterAttempt,
}

/// Runs the buyer lifecycle, drops the coordinator at the crash point, and reopens from disk.
async fn crash_phase(
    fixture: &Fixture,
    root: &std::path::Path,
    crash: CrashPoint,
) -> (Coordinator, SignerJournal, PreparedSettlement) {
    let (coordinator, _journal, prepared, _plan) = coordinated_setup(fixture, root).await;
    let now = now();
    if crash == CrashPoint::AfterAttempt {
        coordinator
            .begin_broadcast_attempt(prepared.operation_ref, now)
            .expect("attempt");
    }
    // The process dies here. Everything durable is on disk; nothing after the boundary ran.
    drop(coordinator);

    let context = context_for(fixture);
    let reopened = Coordinator::open(
        root.join("coordinator"),
        KeyBytes::new(BUYER_KEY_ADDRESS.to_vec()).expect("buyer key"),
        context.clone(),
        &context,
        &capabilities_for(fixture),
        policy_for(fixture),
    )
    .expect("reopen coordinator");
    let signer = address_recover(&SigningKey::from_slice(&RELAYER_KEY).expect("relayer key"));
    let journal = SignerJournal::open(root.join("signer"), 31_337, signer).expect("reopen journal");
    (reopened, journal, prepared)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn crashes_at_durable_boundaries_yield_one_payment_and_recoverable_state() {
    for crash in [
        CrashPoint::None,
        CrashPoint::AfterSigned,
        CrashPoint::AfterAttempt,
    ] {
        let fixture = fixture().await;
        let deployment = deployment(&fixture);
        let chain = EvmChain::connect(deployment.clone(), Duration::from_secs(5))
            .await
            .expect("chain");
        let root = tempfile::tempdir().expect("state dir");
        let (coordinator, journal, prepared) = crash_phase(&fixture, root.path(), crash).await;
        let operation_ref = prepared.operation_ref;
        let now = now();

        // Recovery is a fresh broadcast of the same durable bytes, not a new payment.
        let _ = chain
            .broadcast_journaled(&coordinator, operation_ref, now)
            .await
            .expect("recovery broadcast");
        mine_blocks(&read_only_provider(&fixture.rpc_url), 3).await;
        let evidence = chain
            .finalized_deal_evidence(&prepared.deal_nullifier, ObservationLimits::default())
            .await
            .expect("deal evidence");
        coordinator
            .reconcile(operation_ref, &evidence, now + 1)
            .expect("reconcile");

        let diagnostic = &coordinator.diagnostics().expect("diagnostics")[0];
        assert_eq!(diagnostic.stage, Stage::Finalized, "crash {crash:?}");
        assert_eq!(
            coordinator.ledger().expect("ledger").reservations()[0].state,
            ReservationState::Committed,
            "crash {crash:?}"
        );
        let reader = read_only_provider(&fixture.rpc_url);
        assert_eq!(
            token_balance(&reader, fixture.token, &RECIPIENT).await,
            AMOUNT,
            "crash {crash:?}: exactly one payment"
        );
        assert_eq!(
            token_balance(&reader, fixture.token, &FEE_RECIPIENT).await,
            FEE,
            "crash {crash:?}: exactly one fee"
        );

        let signer = address_recover(&SigningKey::from_slice(&RELAYER_KEY).expect("relayer key"));
        let finalized = chain
            .verified_finalized_nonce(signer)
            .await
            .expect("finalized nonce");
        journal.release(&finalized).expect("release");
        assert!(journal
            .resume(&deployment, &prepared)
            .expect("resume")
            .is_none());
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn the_relayer_fails_over_providers_and_reports_funding_shortfall() {
    use erebus_evm::relay::{AccessLimits, RelayPolicy, RelayService};

    let fixture = fixture().await;
    let deployment = deployment(&fixture);
    let prepared = prepared(&fixture);
    let evidence = SettlementEvidence::decode(&prepared.backend_evidence).expect("evidence");
    let now = now();

    let policy = RelayPolicy::new(
        deployment.clone(),
        FEE_RECIPIENT,
        [(address_bytes(fixture.token), FEE)].into_iter().collect(),
        3_600,
    )
    .expect("policy");

    // The first provider is a dead port; the second is the live chain. Admission and funding
    // are local reads against the first provider, so funding uses the live one.
    let live = EvmSettlementBackend::connect(deployment.clone(), &RELAYER_KEY).expect("live");
    let service = RelayService::new(policy, vec![live], AccessLimits::new(4, 60)).expect("service");
    let admitted = service
        .admit("buyer", &evidence.encode(), now)
        .expect("admit");

    let funded = service.funding(&admitted).await.expect("funding");
    assert!(funded.is_funded(), "anvil funds the relayer by default");
    assert_eq!(funded.shortfall(), 0);

    // Drain the gas payer and observe the shortfall without sending a transaction.
    let provider = read_only_provider(&fixture.rpc_url);
    let signer = address_recover(&SigningKey::from_slice(&RELAYER_KEY).expect("relayer key"));
    let _: serde_json::Value = provider
        .client()
        .request("anvil_setBalance", (Address::from(signer), "0x0"))
        .await
        .expect("drain");
    let drained = service.funding(&admitted).await.expect("funding");
    assert!(!drained.is_funded());
    assert!(drained.shortfall() > 0);

    // Restore funding and submit through a service whose first provider is dead.
    let _: serde_json::Value = provider
        .client()
        .request(
            "anvil_setBalance",
            (Address::from(signer), "0x21e19e0c9bab2400000"),
        )
        .await
        .expect("refill");
    let dead = EvmDeployment::new(
        ChainNamespace::new("eip155", "31337").expect("namespace"),
        address_bytes(fixture.settlement),
        1,
        "http://127.0.0.1:9",
    )
    .expect("dead deployment");
    let failing_over = RelayService::new(
        service.policy().clone(),
        vec![
            EvmSettlementBackend::connect(dead, &RELAYER_KEY).expect("dead"),
            EvmSettlementBackend::connect(deployment.clone(), &RELAYER_KEY).expect("live"),
        ],
        AccessLimits::new(4, 60),
    )
    .expect("service");
    let state_root = tempfile::tempdir().expect("state root");
    let (coordinator, _, prepared, _) = coordinated_setup(&fixture, state_root.path()).await;
    let failover_funding = failing_over
        .funding(&prepared)
        .await
        .expect("funding diagnostics use the live backup");
    assert!(failover_funding.is_funded());
    let _: serde_json::Value = provider
        .client()
        .request("anvil_setBalance", (Address::from(signer), "0x0"))
        .await
        .expect("drain after provider failover");
    let failover_shortfall = failing_over.funding(&prepared).await.expect("shortfall");
    assert!(!failover_shortfall.is_funded());
    assert!(failover_shortfall.shortfall() > 0);
    let _: serde_json::Value = provider
        .client()
        .request(
            "anvil_setBalance",
            (Address::from(signer), "0x21e19e0c9bab2400000"),
        )
        .await
        .expect("restore funding before submission");
    let transaction = failing_over
        .submit_journaled(
            "buyer",
            &coordinator,
            prepared.operation_ref,
            now,
            Duration::from_secs(5),
        )
        .await
        .expect("failover submit");
    assert_eq!(transaction.outcome, BroadcastOutcome::Acknowledged);
    assert_ne!(transaction.hash, [0; 32]);

    let metrics = failing_over.metrics();
    assert_eq!(metrics.admitted, 1);
    assert_eq!(metrics.submitted, 1);
    assert_eq!(metrics.submit_failed, 0);

    let reader = read_only_provider(&fixture.rpc_url);
    assert_eq!(
        token_balance(&reader, fixture.token, &RECIPIENT).await,
        AMOUNT
    );
    assert_eq!(
        token_balance(&reader, fixture.token, &FEE_RECIPIENT).await,
        FEE
    );
}

// ---------------------------------------------------------------------
// M6: JSON-RPC fault proxy (DM6-9)
// ---------------------------------------------------------------------

/// What the proxy does with one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    /// Forward everything and return the upstream response.
    Pass,
    /// Delay a selected observation method beyond its client deadline.
    DelayMethod(&'static str),
    /// Forward `eth_sendRawTransaction`, then close without returning its response.
    DropBroadcastResponse,
    /// Forward the transaction, but keep its response beyond the client deadline.
    DelayBroadcastResponse,
    /// Keep the finalized response beyond the client deadline.
    DelayFinalizedResponse,
    /// Return a different hash for `eth_sendRawTransaction` without forwarding it.
    FalsifyBroadcastHash,
    /// Return an impossible `finalized` block, ahead of the real head.
    FalsifyFinalized,
}

/// A TCP JSON-RPC proxy in front of anvil that can drop or falsify responses.
struct FaultProxy {
    url: String,
    fault: Arc<Mutex<Fault>>,
    broadcasts: Arc<AtomicUsize>,
    _task: tokio::task::JoinHandle<()>,
}

impl FaultProxy {
    async fn start(upstream: String) -> Self {
        use tokio::net::TcpListener as TokioListener;

        let listener = TokioListener::bind("127.0.0.1:0")
            .await
            .expect("proxy bind");
        let port = listener.local_addr().expect("proxy addr").port();
        let fault = Arc::new(Mutex::new(Fault::Pass));
        let fault_for_task = Arc::clone(&fault);
        let broadcasts = Arc::new(AtomicUsize::new(0));
        let broadcasts_for_task = Arc::clone(&broadcasts);
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let upstream = upstream.clone();
                let fault = Arc::clone(&fault_for_task);
                let broadcasts = Arc::clone(&broadcasts_for_task);
                tokio::spawn(async move {
                    handle_proxy_connection(stream, upstream, fault, broadcasts).await;
                });
            }
        });
        Self {
            url: format!("http://127.0.0.1:{port}"),
            fault,
            broadcasts,
            _task: task,
        }
    }

    fn set(&self, fault: Fault) {
        *self.fault.lock().expect("proxy lock") = fault;
    }
}

impl Drop for FaultProxy {
    fn drop(&mut self) {
        self._task.abort();
    }
}

async fn handle_proxy_connection(
    mut stream: tokio::net::TcpStream,
    upstream: String,
    fault: Arc<Mutex<Fault>>,
    broadcasts: Arc<AtomicUsize>,
) {
    use tokio::io::AsyncWriteExt;

    let Some((request, json)) = read_http_request(&mut stream).await else {
        return;
    };
    let method = json["method"].as_str().unwrap_or_default();
    let is_broadcast = method == "eth_sendRawTransaction";
    if is_broadcast {
        broadcasts.fetch_add(1, Ordering::SeqCst);
    }
    let is_finalized =
        method == "eth_getBlockByNumber" && json["params"][0].as_str() == Some("finalized");
    let fault = *fault.lock().expect("proxy lock");
    match fault {
        Fault::DelayMethod(selected) if method == selected => {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        Fault::DelayBroadcastResponse if is_broadcast => {
            let response = forward_http(&upstream, &request).await;
            tokio::time::sleep(Duration::from_secs(5)).await;
            if let Some(response) = response {
                let _ = stream.write_all(&response).await;
            }
            return;
        }
        Fault::DelayFinalizedResponse if is_finalized => {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        Fault::DropBroadcastResponse if is_broadcast => {
            // The transaction reaches the chain; the caller never learns the response.
            let _ = forward_http(&upstream, &request).await;
            return;
        }
        Fault::FalsifyBroadcastHash if is_broadcast => {
            let result = format!("0x{}", "ff".repeat(32));
            let _ = stream
                .write_all(&json_rpc_response(&json, serde_json::json!(result)))
                .await;
            return;
        }
        Fault::FalsifyFinalized if is_finalized => {
            let block = serde_json::json!({
                "number": "0xffff",
                "hash": format!("0x{}", "11".repeat(32)),
                "parentHash": format!("0x{}", "22".repeat(32)),
                "timestamp": "0x1",
                "transactions": [],
            });
            let _ = stream.write_all(&json_rpc_response(&json, block)).await;
            return;
        }
        _ => {}
    }
    if let Some(response) = forward_http(&upstream, &request).await {
        let _ = stream.write_all(&response).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn each_observation_rpc_timeout_holds_until_honest_evidence_recovers() {
    let fixture = fixture().await;
    let root = tempfile::tempdir().expect("state root");
    let (coordinator, journal, prepared, _) = coordinated_setup(&fixture, root.path()).await;
    let honest = EvmChain::connect(deployment(&fixture), Duration::from_secs(5))
        .await
        .expect("honest chain");
    honest
        .broadcast_journaled(&coordinator, prepared.operation_ref, now())
        .await
        .expect("broadcast");
    mine_blocks(&read_only_provider(&fixture.rpc_url), 3).await;
    let proxy =
        FaultProxy::start(fixture.rpc_url.strip_prefix("http://").unwrap().to_owned()).await;
    let chain = EvmChain::connect(
        proxied_deployment(&fixture, &proxy),
        Duration::from_millis(200),
    )
    .await
    .expect("proxy chain");
    for method in ["eth_getBlockByNumber", "eth_call", "eth_getLogs"] {
        proxy.set(Fault::DelayMethod(method));
        assert!(
            chain
                .finalized_deal_evidence(&prepared.deal_nullifier, ObservationLimits::default())
                .await
                .is_err(),
            "timeout at {method}"
        );
        coordinator
            .reconcile(
                prepared.operation_ref,
                &erebus_core::deal_state::DealEvidence::Unknown,
                now(),
            )
            .unwrap();
        assert_eq!(
            coordinator.ledger().unwrap().reservations()[0].state,
            ReservationState::Reserved
        );
        assert!(journal
            .resume(&deployment(&fixture), &prepared)
            .unwrap()
            .is_some());
    }
    proxy.set(Fault::Pass);
    let evidence = chain
        .finalized_deal_evidence(&prepared.deal_nullifier, ObservationLimits::default())
        .await
        .expect("honest evidence after timeouts");
    coordinator
        .reconcile(prepared.operation_ref, &evidence, now())
        .unwrap();
    assert_eq!(
        coordinator.ledger().unwrap().reservations()[0].state,
        ReservationState::Committed
    );
    assert_eq!(
        token_balance(
            &read_only_provider(&fixture.rpc_url),
            fixture.token,
            &RECIPIENT
        )
        .await,
        AMOUNT
    );
    assert_eq!(
        proxy.broadcasts.load(Ordering::SeqCst),
        0,
        "observation never resubmits"
    );
}

fn json_rpc_response(request: &serde_json::Value, result: serde_json::Value) -> Vec<u8> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": request["id"].clone(),
        "result": result,
    })
    .to_string();
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
    .into_bytes()
}

async fn read_http_request(
    stream: &mut tokio::net::TcpStream,
) -> Option<(Vec<u8>, serde_json::Value)> {
    use tokio::io::AsyncReadExt;

    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(headers_end) = find_bytes(&buffer, b"\r\n\r\n") {
            let length = content_length(&buffer[..headers_end]);
            let total = headers_end + 4 + length;
            while buffer.len() < total {
                let read = stream.read(&mut chunk).await.ok()?;
                if read == 0 {
                    return None;
                }
                buffer.extend_from_slice(&chunk[..read]);
            }
            let json = serde_json::from_slice(&buffer[headers_end + 4..total]).ok()?;
            return Some((buffer[..total].to_vec(), json));
        }
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
}

async fn forward_http(upstream: &str, request: &[u8]) -> Option<Vec<u8>> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut outbound = tokio::net::TcpStream::connect(upstream).await.ok()?;
    outbound.write_all(request).await.ok()?;
    let mut response = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = outbound.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        response.extend_from_slice(&chunk[..read]);
        if let Some(headers_end) = find_bytes(&response, b"\r\n\r\n") {
            let length = content_length(&response[..headers_end]);
            if response.len() >= headers_end + 4 + length {
                break;
            }
        }
    }
    connection_close_response(&response)
}

fn connection_close_response(response: &[u8]) -> Option<Vec<u8>> {
    // The proxy serves one request per socket; do not forward Anvil's keep-alive promise.
    let end = find_bytes(response, b"\r\n\r\n")?;
    let headers = std::str::from_utf8(&response[..end]).ok()?;
    let mut result = Vec::with_capacity(response.len());
    for line in headers.split("\r\n") {
        if line
            .split_once(':')
            .is_some_and(|(name, _)| name.eq_ignore_ascii_case("connection"))
        {
            continue;
        }
        result.extend_from_slice(line.as_bytes());
        result.extend_from_slice(b"\r\n");
    }
    result.extend_from_slice(b"Connection: close\r\n\r\n");
    result.extend_from_slice(&response[end + 4..]);
    Some(result)
}

#[test]
fn rpc_proxy_closes_each_response_without_changing_its_status_or_body() {
    for header in ["Connection", "connection", "CONNECTION"] {
        let input =
            format!("HTTP/1.1 200 OK\r\n{header}: keep-alive\r\nContent-Length: 2\r\n\r\n{{}}");
        let output = connection_close_response(input.as_bytes()).unwrap();
        assert_eq!(
            output,
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"
        );
    }
}

fn content_length(headers: &[u8]) -> usize {
    String::from_utf8_lossy(headers)
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// A deployment whose RPC endpoint is the proxy.
fn proxied_deployment(fixture: &Fixture, proxy: &FaultProxy) -> EvmDeployment {
    EvmDeployment::new(
        ChainNamespace::new("eip155", "31337").expect("namespace"),
        address_bytes(fixture.settlement),
        1,
        proxy.url.clone(),
    )
    .expect("deployment")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn a_dropped_broadcast_response_stays_unknown_and_reconciles_from_chain() {
    uncertain_broadcast_reconciles_without_resubmission(Fault::DropBroadcastResponse).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn a_timed_out_broadcast_stays_unknown_and_reconciles_from_chain() {
    uncertain_broadcast_reconciles_without_resubmission(Fault::DelayBroadcastResponse).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn journaled_relayer_failover_never_resigns_an_uncertain_payment() {
    use erebus_evm::relay::{AccessLimits, RelayPolicy, RelayService};
    use std::collections::BTreeMap;

    for fault in [
        Fault::FalsifyBroadcastHash,
        Fault::DropBroadcastResponse,
        Fault::DelayBroadcastResponse,
    ] {
        let fixture = fixture().await;
        let root = tempfile::tempdir().expect("state dir");
        let (coordinator, _, prepared, plan) = coordinated_setup(&fixture, root.path()).await;
        let proxy =
            FaultProxy::start(fixture.rpc_url.strip_prefix("http://").unwrap().to_owned()).await;
        proxy.set(fault);
        let service = RelayService::new(
            RelayPolicy::new(
                deployment(&fixture),
                FEE_RECIPIENT,
                BTreeMap::from([(address_bytes(fixture.token), FEE)]),
                3600,
            )
            .unwrap(),
            vec![
                EvmSettlementBackend::connect(proxied_deployment(&fixture, &proxy), &RELAYER_KEY)
                    .unwrap(),
                EvmSettlementBackend::connect(deployment(&fixture), &RELAYER_KEY).unwrap(),
            ],
            AccessLimits::default(),
        )
        .unwrap();
        let stored = coordinator
            .signed_transaction(prepared.operation_ref)
            .unwrap()
            .unwrap();
        let expected = erebus_evm::chain::SignedTransaction::from_raw(stored.raw())
            .unwrap()
            .hash();
        let sent = service
            .submit_journaled(
                "buyer",
                &coordinator,
                prepared.operation_ref,
                now(),
                Duration::from_secs(2),
            )
            .await
            .unwrap();
        assert_eq!(sent.hash, expected);
        assert_eq!(proxy.broadcasts.load(Ordering::SeqCst), 1);
        assert_eq!(
            coordinator.ledger().unwrap().reservations()[0].state,
            ReservationState::Reserved
        );
        let provider = read_only_provider(&fixture.rpc_url);
        assert_eq!(
            provider
                .get_transaction_count(Address::from(plan.sender()))
                .await
                .unwrap(),
            plan.params().nonce + 1
        );
        assert_eq!(
            token_balance(&provider, fixture.token, &RECIPIENT).await,
            AMOUNT
        );
        mine_blocks(&provider, 3).await;
        let chain = EvmChain::connect(deployment(&fixture), Duration::from_secs(5))
            .await
            .unwrap();
        let evidence = chain
            .finalized_deal_evidence(&prepared.deal_nullifier, ObservationLimits::default())
            .await
            .unwrap();
        coordinator
            .reconcile(prepared.operation_ref, &evidence, now())
            .unwrap();
        assert_eq!(
            coordinator.ledger().unwrap().reservations()[0].state,
            ReservationState::Committed
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn rejected_relay_admission_does_not_record_a_broadcast_attempt() {
    use erebus_evm::relay::{AccessLimits, RelayPolicy, RelayService};
    use std::collections::BTreeMap;

    let fixture = fixture().await;
    let root = tempfile::tempdir().expect("state dir");
    let (coordinator, _, prepared, _) = coordinated_setup(&fixture, root.path()).await;
    let backend = EvmSettlementBackend::connect(deployment(&fixture), &RELAYER_KEY).unwrap();
    let service = RelayService::new(
        RelayPolicy::new(
            deployment(&fixture),
            FEE_RECIPIENT,
            BTreeMap::from([(address_bytes(fixture.token), FEE)]),
            3600,
        )
        .unwrap(),
        vec![backend],
        AccessLimits::new(1, 60),
    )
    .unwrap();
    service
        .admit("buyer", &prepared.backend_evidence, now())
        .unwrap();
    for _ in 0..3 {
        assert!(service
            .submit_journaled(
                "buyer",
                &coordinator,
                prepared.operation_ref,
                now(),
                Duration::from_secs(2),
            )
            .await
            .is_err());
    }
    let diagnostic = coordinator.diagnostics().unwrap();
    assert_eq!(diagnostic[0].broadcast_attempts, 0);
    assert_eq!(diagnostic[0].unknown_attempts, 0);
    assert_eq!(
        coordinator.ledger().unwrap().reservations()[0].state,
        ReservationState::Reserved
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn historical_observation_resumes_logs_and_old_ancestry_across_restarts() {
    use erebus_evm::chain::{HistoricalObservation, ObservationJournal};
    let fixture = fixture().await;
    let root = tempfile::tempdir().unwrap();
    let (coordinator, _, prepared, _) = coordinated_setup(&fixture, root.path()).await;
    let chain = EvmChain::connect(deployment(&fixture), Duration::from_secs(2))
        .await
        .unwrap();
    chain
        .broadcast_journaled(&coordinator, prepared.operation_ref, now())
        .await
        .unwrap();
    let provider = read_only_provider(&fixture.rpc_url);
    mine_blocks(&provider, 20).await;
    let budget = ObservationLimits {
        log_block_range: 2,
        max_log_queries: 1,
        max_ancestry: 2,
    };
    assert!(matches!(
        chain
            .finalized_deal_evidence(&prepared.deal_nullifier, budget)
            .await,
        Err(EvmError::ObservationLimit)
    ));
    let mut pending = 0;
    let mut completed = None;
    for _ in 0..100 {
        let store = ObservationJournal::open(root.path().join("history")).unwrap();
        match chain
            .finalized_deal_evidence_resumable(&store, &prepared.deal_nullifier, budget)
            .await
            .unwrap()
        {
            HistoricalObservation::Pending { .. } => {
                pending += 1;
                assert_eq!(
                    coordinator.ledger().unwrap().reservations()[0].state,
                    ReservationState::Reserved
                );
            }
            HistoricalObservation::Complete { evidence, .. } => {
                completed = Some(evidence);
                break;
            }
        }
    }
    assert!(pending > 2, "both history phases require continuation");
    let evidence = completed.expect("bounded work eventually completes");
    assert_eq!(
        evidence,
        chain
            .finalized_deal_evidence(&prepared.deal_nullifier, ObservationLimits::default())
            .await
            .unwrap()
    );
    let store = ObservationJournal::open(root.path().join("history")).unwrap();
    assert!(matches!(
        chain
            .finalized_deal_evidence_resumable(&store, &prepared.deal_nullifier, budget)
            .await
            .unwrap(),
        HistoricalObservation::Complete { .. }
    ));
    coordinator
        .reconcile(prepared.operation_ref, &evidence, now())
        .unwrap();
    assert_eq!(
        coordinator.ledger().unwrap().reservations()[0].state,
        ReservationState::Committed
    );
    mine_blocks(&provider, 4).await;
    let mut calls = 0;
    let refreshed = loop {
        calls += 1;
        assert!(
            calls < 10,
            "reuse the finalized prefix, not genesis or the old winner walk"
        );
        let store = ObservationJournal::open(root.path().join("history")).unwrap();
        if let HistoricalObservation::Complete { evidence, .. } = chain
            .finalized_deal_evidence_resumable(&store, &prepared.deal_nullifier, budget)
            .await
            .unwrap()
        {
            break evidence;
        }
    };
    assert_eq!(
        refreshed,
        chain
            .finalized_deal_evidence(&prepared.deal_nullifier, ObservationLimits::default())
            .await
            .unwrap()
    );
    let path = std::fs::read_dir(root.path().join("history"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "json"))
        .unwrap();
    std::fs::write(path, b"invalid checkpoint").unwrap();
    assert!(chain
        .finalized_deal_evidence_resumable(&store, &prepared.deal_nullifier, budget)
        .await
        .is_err());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn historical_observation_restarts_nonfinal_work_after_a_reorg() {
    use erebus_evm::chain::{HistoricalObservation, ObservationJournal};
    let fixture = fixture().await;
    let root = tempfile::tempdir().unwrap();
    let (coordinator, _, prepared, plan) = coordinated_setup(&fixture, root.path()).await;
    let chain = EvmChain::connect(deployment(&fixture), Duration::from_secs(2))
        .await
        .unwrap();
    let provider = read_only_provider(&fixture.rpc_url);
    let snapshot: U64 = provider.client().request("evm_snapshot", ()).await.unwrap();
    let stored = coordinator
        .signed_transaction(prepared.operation_ref)
        .unwrap()
        .unwrap();
    let sent = chain
        .broadcast(&prepared, &plan, stored.raw())
        .await
        .unwrap();
    let mut included = false;
    for _ in 0..100 {
        if provider
            .get_transaction_receipt(alloy::primitives::B256::from(sent.hash))
            .await
            .unwrap()
            .is_some()
        {
            included = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        included,
        "pin a head containing the payment before reorganizing it"
    );
    let budget = ObservationLimits {
        log_block_range: 1,
        max_log_queries: 1,
        max_ancestry: 1,
    };
    let store = ObservationJournal::open(root.path().join("history")).unwrap();
    for next in [1, 2] {
        assert!(
            matches!(chain.finalized_deal_evidence_resumable(&store, &prepared.deal_nullifier, budget).await.unwrap(),
            HistoricalObservation::Pending { next_log_block: Some(n), .. } if n == next)
        );
    }
    let reverted: bool = provider
        .client()
        .request("evm_revert", (snapshot,))
        .await
        .unwrap();
    assert!(reverted);
    let _: serde_json::Value = provider
        .client()
        .request("evm_setNextBlockTimestamp", (now() + 10,))
        .await
        .unwrap();
    mine_blocks(&provider, 1).await;
    let restarted = chain
        .finalized_deal_evidence_resumable(&store, &prepared.deal_nullifier, budget)
        .await
        .unwrap();
    assert!(
        matches!(
            restarted,
            HistoricalObservation::Pending {
                next_log_block: Some(1),
                ..
            }
        ),
        "{restarted:?}"
    );
    let mut completed = None;
    for _ in 0..30 {
        if let HistoricalObservation::Complete { evidence, .. } = chain
            .finalized_deal_evidence_resumable(&store, &prepared.deal_nullifier, budget)
            .await
            .unwrap()
        {
            completed = Some(evidence);
            break;
        }
    }
    let evidence = completed.unwrap();
    assert!(
        matches!(evidence, erebus_core::deal_state::DealEvidence::Observed(ref reads) if !reads.consumed_at_head && reads.winner.is_none())
    );
    coordinator
        .reconcile(prepared.operation_ref, &evidence, now())
        .unwrap();
    assert_eq!(
        coordinator.ledger().unwrap().reservations()[0].state,
        ReservationState::Reserved
    );
    assert_eq!(token_balance(&provider, fixture.token, &RECIPIENT).await, 0);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn historical_pair_keeps_completed_anchors_while_its_peer_is_pending() {
    use erebus_evm::chain::{HistoricalObservation, ObservationJournal};
    let fixture = fixture().await;
    let prepared = prepared(&fixture);
    settle_coordinated(&fixture, &RELAYER_KEY).await;
    let provider = read_only_provider(&fixture.rpc_url);
    mine_blocks(&provider, 3).await;
    let chain = EvmChain::connect(deployment(&fixture), Duration::from_secs(5))
        .await
        .unwrap();
    let mut peer_deployment = deployment(&fixture);
    peer_deployment.rpc_url = fixture.verification_proxy.url.clone();
    let peer = EvmChain::connect(peer_deployment, Duration::from_secs(5))
        .await
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let first_path = root.path().join("first");
    let peer_path = root.path().join("peer");
    let first_journal = ObservationJournal::open(&first_path).unwrap();
    let peer_journal = ObservationJournal::open(&peer_path).unwrap();
    let pinned = chain
        .finalized_deal_evidence_resumable(
            &first_journal,
            &prepared.deal_nullifier,
            ObservationLimits::default(),
        )
        .await
        .unwrap();
    let budget = ObservationLimits {
        log_block_range: 1,
        max_log_queries: 1,
        max_ancestry: 1,
    };
    assert!(matches!(
        peer.finalized_deal_evidence_resumable(&peer_journal, &prepared.deal_nullifier, budget)
            .await
            .unwrap(),
        HistoricalObservation::Pending { .. }
    ));
    mine_blocks(&provider, 4).await;
    for _ in 0..40 {
        let first_journal = ObservationJournal::open(&first_path).unwrap();
        let peer_journal = ObservationJournal::open(&peer_path).unwrap();
        let result = chain
            .finalized_deal_evidence_resumable_agreed(
                &first_journal,
                &peer,
                &peer_journal,
                &prepared.deal_nullifier,
                budget,
            )
            .await
            .unwrap();
        if matches!(result, HistoricalObservation::Complete { .. }) {
            assert_eq!(result, pinned);
            return;
        }
    }
    panic!("completed observer kept refreshing while its peer was pending");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn packaged_relayer_never_downgrades_verification_when_a_backup_is_healthy() {
    let fixture = fixture().await;
    let root = tempfile::tempdir().unwrap();
    let prepared = prepared(&fixture);
    let primary =
        FaultProxy::start(fixture.rpc_url.strip_prefix("http://").unwrap().to_owned()).await;
    let primary_urls = format!("http://127.0.0.1:9,{},{}", primary.url, fixture.rpc_url);
    fixture.verification_proxy.set(Fault::FalsifyFinalized);
    let held = relayer_process(
        &fixture,
        root.path(),
        &primary_urls,
        "relay",
        &prepared.backend_evidence,
    );
    assert_eq!(
        held["error"],
        "relayer chain observation unavailable; retain the operation"
    );
    let service_prepared = erebus_evm::relay::RelayPolicy::new(
        deployment(&fixture),
        FEE_RECIPIENT,
        [(address_bytes(fixture.token), FEE)].into_iter().collect(),
        3600,
    )
    .unwrap()
    .admit(&prepared.backend_evidence, now())
    .unwrap();
    let context = context_for(&fixture);
    let coordinator = Coordinator::open(
        root.path()
            .join("operations")
            .join(hex::encode(service_prepared.operation_ref)),
        fixture.terms.buyer_authorization_key.clone(),
        context.clone(),
        &context,
        &capabilities_for(&fixture),
        SpendingPolicy {
            per_deal_max: BaseUnits::new(u128::MAX),
            allowed_assets: [fixture.terms.asset.clone()].into_iter().collect(),
            ..SpendingPolicy::default()
        },
    )
    .unwrap();
    assert_eq!(coordinator.diagnostics().unwrap()[0].stage, Stage::Prepared);
    assert_eq!(coordinator.diagnostics().unwrap()[0].broadcast_attempts, 0);
    assert_eq!(
        coordinator.ledger().unwrap().reservations()[0].state,
        ReservationState::Reserved
    );
    let signer = TransactionKey::from_bytes(&RELAYER_KEY).unwrap().address();
    let journal = SignerJournal::open(root.path().join("signer"), 31_337, signer).unwrap();
    assert!(journal
        .resume(&deployment(&fixture), &service_prepared)
        .unwrap()
        .is_none());
    assert_eq!(primary.broadcasts.load(Ordering::SeqCst), 0);
    fixture.verification_proxy.set(Fault::Pass);
    let sent = relayer_process(
        &fixture,
        root.path(),
        &primary_urls,
        "relay",
        &prepared.backend_evidence,
    );
    assert_eq!(sent["stage"], "Submitted", "{sent}");
    mine_blocks(&read_only_provider(&fixture.rpc_url), 3).await;
    fixture
        .verification_proxy
        .set(Fault::DelayFinalizedResponse);
    let held = relayer_process(
        &fixture,
        root.path(),
        &primary_urls,
        "recover",
        &prepared.backend_evidence,
    );
    assert_eq!(
        held["error"],
        "relayer chain observation unavailable; retain the operation"
    );
    assert_eq!(
        coordinator.ledger().unwrap().reservations()[0].state,
        ReservationState::Reserved
    );
    assert!(journal
        .resume(&deployment(&fixture), &service_prepared)
        .unwrap()
        .is_some());
    fixture.verification_proxy.set(Fault::Pass);
    let final_result = relayer_process(
        &fixture,
        root.path(),
        &primary_urls,
        "recover",
        &prepared.backend_evidence,
    );
    assert_eq!(final_result["stage"], "Finalized", "{final_result}");
    assert_eq!(
        coordinator.ledger().unwrap().reservations()[0].state,
        ReservationState::Committed
    );
    assert!(journal
        .resume(&deployment(&fixture), &service_prepared)
        .unwrap()
        .is_none());
    assert_eq!(primary.broadcasts.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture.verification_proxy.broadcasts.load(Ordering::SeqCst),
        0
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn packaged_relayer_continues_history_before_allocating_or_sending() {
    let fixture = fixture().await;
    let root = tempfile::tempdir().unwrap();
    let prepared = prepared(&fixture);
    let proxy =
        FaultProxy::start(fixture.rpc_url.strip_prefix("http://").unwrap().to_owned()).await;
    let budget = [
        ("EREBUS_RELAYER_LOG_BLOCK_RANGE", "1"),
        ("EREBUS_RELAYER_LOG_QUERIES", "1"),
        ("EREBUS_RELAYER_ANCESTRY_LINKS", "1"),
    ];
    let first = relayer_process_config(
        &fixture,
        root.path(),
        &proxy.url,
        "relay",
        &prepared.backend_evidence,
        FEE,
        &budget,
    );
    assert_eq!(
        first["error"],
        "relayer history scan pending; repeat recovery with the original request"
    );
    let mut ready = false;
    for _ in 0..30 {
        let response = relayer_process_config(
            &fixture,
            root.path(),
            &proxy.url,
            "recover",
            &prepared.backend_evidence,
            FEE,
            &budget,
        );
        assert_eq!(proxy.broadcasts.load(Ordering::SeqCst), 0);
        if response["status"] == "ok" {
            assert_eq!(response["stage"], "Prepared", "{response}");
            assert_eq!(response["gas_account_busy"], false);
            ready = true;
            break;
        }
        assert_eq!(response["error"], first["error"]);
    }
    assert!(
        ready,
        "persisted scan continues across independent CLI invocations"
    );
    let sent = relayer_process_config(
        &fixture,
        root.path(),
        &proxy.url,
        "relay",
        &prepared.backend_evidence,
        FEE,
        &budget,
    );
    assert_eq!(sent["stage"], "Submitted", "{sent}");
    assert_eq!(proxy.broadcasts.load(Ordering::SeqCst), 1);
    let provider = read_only_provider(&fixture.rpc_url);
    mine_blocks(&provider, 3).await;
    let mut finalized = false;
    for _ in 0..30 {
        let response = relayer_process_config(
            &fixture,
            root.path(),
            &proxy.url,
            "recover",
            &prepared.backend_evidence,
            FEE,
            &budget,
        );
        if response["status"] == "ok" {
            assert_eq!(response["stage"], "Finalized", "{response}");
            assert_eq!(response["transaction_hash"], sent["transaction_hash"]);
            assert_eq!(response["gas_account_busy"], false);
            finalized = true;
            break;
        }
        assert_eq!(response["error"], first["error"]);
    }
    assert!(finalized);
    assert_eq!(proxy.broadcasts.load(Ordering::SeqCst), 1);
    assert_eq!(
        token_balance(&provider, fixture.token, &RECIPIENT).await,
        AMOUNT
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn packaged_relayer_recovers_a_dropped_response_after_process_restart() {
    let fixture = fixture().await;
    let provider = read_only_provider(&fixture.rpc_url);
    let signer = address_recover(&SigningKey::from_slice(&RELAYER_KEY).unwrap());
    let initial_nonce = provider
        .get_transaction_count(Address::from(signer))
        .await
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let prepared = prepared(&fixture);
    let proxy =
        FaultProxy::start(fixture.rpc_url.strip_prefix("http://").unwrap().to_owned()).await;
    proxy.set(Fault::DropBroadcastResponse);
    let first = relayer_process(
        &fixture,
        root.path(),
        &proxy.url,
        "relay",
        &prepared.backend_evidence,
    );
    assert_eq!(first["status"], "ok", "{first}");
    assert_eq!(first["stage"], "BroadcastUnknown", "{first}");
    assert_eq!(first["gas_account_busy"], true);
    assert_eq!(proxy.broadcasts.load(Ordering::SeqCst), 1);
    proxy.set(Fault::Pass);
    let again = relayer_process(
        &fixture,
        root.path(),
        &proxy.url,
        "relay",
        &prepared.backend_evidence,
    );
    assert_eq!(again["status"], "ok", "{again}");
    assert_eq!(again["transaction_hash"], first["transaction_hash"]);
    assert_eq!(
        proxy.broadcasts.load(Ordering::SeqCst),
        1,
        "observe before retrying"
    );
    mine_blocks(&provider, 3).await;
    let recovered = relayer_process(
        &fixture,
        root.path(),
        &fixture.rpc_url,
        "recover",
        &prepared.backend_evidence,
    );
    assert_eq!(recovered["status"], "ok", "{recovered}");
    assert_eq!(recovered["stage"], "Finalized");
    assert_eq!(recovered["gas_account_busy"], false);
    assert_eq!(recovered["transaction_hash"], first["transaction_hash"]);
    assert_eq!(
        token_balance(&provider, fixture.token, &RECIPIENT).await,
        AMOUNT
    );
    assert_eq!(
        token_balance(&provider, fixture.token, &FEE_RECIPIENT).await,
        FEE
    );
    assert_eq!(
        provider
            .get_transaction_count(Address::from(signer))
            .await
            .unwrap(),
        initial_nonce + 1
    );
    let journal = SignerJournal::open(root.path().join("signer"), 31337, signer).unwrap();
    // The service derives its own operation reference from the agreement, not the client's.
    let service_prepared = erebus_evm::relay::RelayPolicy::new(
        deployment(&fixture),
        FEE_RECIPIENT,
        [(address_bytes(fixture.token), FEE)].into_iter().collect(),
        3600,
    )
    .unwrap()
    .admit(&prepared.backend_evidence, now())
    .unwrap();
    assert!(journal
        .resume(&deployment(&fixture), &service_prepared)
        .unwrap()
        .is_none());
    let _: serde_json::Value = provider
        .client()
        .request("evm_setNextBlockTimestamp", (fixture.terms.expiry + 1,))
        .await
        .unwrap();
    mine_blocks(&provider, 3).await;
    let expired_recovery = relayer_process_with_fee(
        &fixture,
        root.path(),
        &fixture.rpc_url,
        "recover",
        &prepared.backend_evidence,
        FEE + 1,
    );
    assert_eq!(expired_recovery["stage"], "Finalized", "{expired_recovery}");
    assert_eq!(proxy.broadcasts.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn packaged_relayer_expiry_releases_accounting_but_holds_unused_nonce() {
    let fixture = fixture().await;
    let provider = read_only_provider(&fixture.rpc_url);
    let signer = address_recover(&SigningKey::from_slice(&RELAYER_KEY).unwrap());
    let initial_nonce = provider
        .get_transaction_count(Address::from(signer))
        .await
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let prepared = prepared(&fixture);
    let proxy =
        FaultProxy::start(fixture.rpc_url.strip_prefix("http://").unwrap().to_owned()).await;
    proxy.set(Fault::FalsifyBroadcastHash);
    let first = relayer_process(
        &fixture,
        root.path(),
        &proxy.url,
        "relay",
        &prepared.backend_evidence,
    );
    assert_eq!(first["stage"], "BroadcastUnknown", "{first}");
    let _: serde_json::Value = provider
        .client()
        .request("evm_setNextBlockTimestamp", (fixture.terms.expiry + 1,))
        .await
        .unwrap();
    mine_blocks(&provider, 3).await;
    let recovered = relayer_process(
        &fixture,
        root.path(),
        &fixture.rpc_url,
        "recover",
        &prepared.backend_evidence,
    );
    assert_eq!(recovered["stage"], "ClosedUnpaid", "{recovered}");
    assert_eq!(recovered["gas_account_busy"], true);
    assert_eq!(recovered["transaction_hash"], first["transaction_hash"]);
    assert_eq!(
        provider
            .get_transaction_count(Address::from(signer))
            .await
            .unwrap(),
        initial_nonce
    );
    assert_eq!(token_balance(&provider, fixture.token, &RECIPIENT).await, 0);
    let journal = SignerJournal::open(root.path().join("signer"), 31337, signer).unwrap();
    let service_prepared = erebus_evm::relay::RelayPolicy::new(
        deployment(&fixture),
        FEE_RECIPIENT,
        [(address_bytes(fixture.token), FEE)].into_iter().collect(),
        3600,
    )
    .unwrap()
    .admit(&prepared.backend_evidence, fixture.terms.expiry - 1)
    .unwrap();
    assert!(journal
        .resume(&deployment(&fixture), &service_prepared)
        .unwrap()
        .is_some());
    let unsubmitted = relayer_process(
        &fixture,
        root.path(),
        &fixture.rpc_url,
        "relay",
        &prepared.backend_evidence,
    );
    assert_eq!(unsubmitted["stage"], "ClosedUnpaid", "{unsubmitted}");
    assert_eq!(proxy.broadcasts.load(Ordering::SeqCst), 1);
}

fn relayer_process(
    fixture: &Fixture,
    root: &std::path::Path,
    rpc_url: &str,
    method: &str,
    evidence: &[u8],
) -> serde_json::Value {
    relayer_process_with_fee(fixture, root, rpc_url, method, evidence, FEE)
}

fn relayer_process_with_fee(
    fixture: &Fixture,
    root: &std::path::Path,
    rpc_url: &str,
    method: &str,
    evidence: &[u8],
    fee: u128,
) -> serde_json::Value {
    relayer_process_config(fixture, root, rpc_url, method, evidence, fee, &[])
}

fn relayer_process_config(
    fixture: &Fixture,
    root: &std::path::Path,
    rpc_url: &str,
    method: &str,
    evidence: &[u8],
    fee: u128,
    extra: &[(&str, &str)],
) -> serde_json::Value {
    use std::io::Write;
    let mut child = Command::new(env!("CARGO_BIN_EXE_erebus-tx-relayer"))
        .env_clear()
        .env("EREBUS_RELAYER_NAMESPACE", "eip155:31337")
        .env(
            "EREBUS_RELAYER_SETTLEMENT",
            format!("0x{}", hex::encode(address_bytes(fixture.settlement))),
        )
        .env("EREBUS_RELAYER_VERIFIER_VERSION", "1")
        .env("EREBUS_RELAYER_KEY", hex::encode(RELAYER_KEY))
        .env(
            "EREBUS_RELAYER_FEE_RECIPIENT",
            format!("0x{}", hex::encode(FEE_RECIPIENT)),
        )
        .env(
            "EREBUS_RELAYER_FEES",
            format!("0x{}={fee}", hex::encode(address_bytes(fixture.token))),
        )
        .env("EREBUS_RELAYER_RPC_URLS", rpc_url)
        .env(
            "EREBUS_RELAYER_VERIFICATION_RPC_URL",
            &fixture.verification_proxy.url,
        )
        .env("EREBUS_RELAYER_STATE_ROOT", root)
        .env("EREBUS_RELAYER_MAX_FEE_PER_GAS", "2000000000")
        .env("EREBUS_RELAYER_PRIORITY_FEE_PER_GAS", "1000000000")
        .env("EREBUS_RELAYER_GAS_LIMIT", "1000000")
        .env("EREBUS_RELAYER_RPC_TIMEOUT_SECONDS", "2")
        .env("EREBUS_RELAYER_MAX_REQUESTS", "1")
        .envs(extra.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            serde_json::json!({
                "method": method, "evidence": hex::encode(evidence),
            })
            .to_string()
            .as_bytes(),
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.stderr.is_empty(),
        "CLI must not export diagnostics on stderr"
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(!String::from_utf8_lossy(&output.stdout).contains(&hex::encode(RELAYER_KEY)));
    value
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn funding_timeout_uses_backup_without_broadcasting() {
    use erebus_evm::relay::{AccessLimits, RelayPolicy, RelayService};

    let fixture = fixture().await;
    let prepared = prepared(&fixture);
    let proxy =
        FaultProxy::start(fixture.rpc_url.strip_prefix("http://").unwrap().to_owned()).await;
    proxy.set(Fault::DelayMethod("eth_estimateGas"));
    let service = RelayService::new(
        RelayPolicy::new(
            deployment(&fixture),
            FEE_RECIPIENT,
            [(address_bytes(fixture.token), FEE)].into_iter().collect(),
            3600,
        )
        .unwrap(),
        vec![
            EvmSettlementBackend::connect(proxied_deployment(&fixture, &proxy), &RELAYER_KEY)
                .unwrap(),
            EvmSettlementBackend::connect(deployment(&fixture), &RELAYER_KEY).unwrap(),
        ],
        AccessLimits::default(),
    )
    .unwrap();
    assert!(service
        .funding_with_timeout(&prepared, Duration::ZERO)
        .await
        .is_err());
    let funding = service
        .funding_with_timeout(&prepared, Duration::from_millis(200))
        .await
        .expect("timed-out primary falls back to the live provider");
    assert!(funding.is_funded());
    assert_eq!(proxy.broadcasts.load(Ordering::SeqCst), 0);
    assert_eq!(service.metrics().submitted, 0);
    assert_eq!(service.metrics().submit_failed, 0);
    let provider = read_only_provider(&fixture.rpc_url);
    assert_eq!(token_balance(&provider, fixture.token, &RECIPIENT).await, 0);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn invalid_relayer_signer_or_deadline_preserves_durable_state_without_sending() {
    use erebus_evm::relay::{
        AccessLimits, DurableRelayError, RelayError, RelayPolicy, RelayService,
    };
    use std::collections::BTreeMap;

    let fixture = fixture().await;
    let root = tempfile::tempdir().expect("state dir");
    let (coordinator, journal, prepared, plan) = coordinated_setup(&fixture, root.path()).await;
    let original = coordinator
        .signed_transaction(prepared.operation_ref)
        .unwrap()
        .unwrap();
    let proxy =
        FaultProxy::start(fixture.rpc_url.strip_prefix("http://").unwrap().to_owned()).await;
    let provider = read_only_provider(&fixture.rpc_url);
    let initial_nonce = provider
        .get_transaction_count(Address::from(plan.sender()))
        .await
        .unwrap();

    for (key, timeout) in [
        (BUYER_KEY, Duration::from_secs(2)),
        (RELAYER_KEY, Duration::ZERO),
    ] {
        let service = RelayService::new(
            RelayPolicy::new(
                deployment(&fixture),
                FEE_RECIPIENT,
                BTreeMap::from([(address_bytes(fixture.token), FEE)]),
                3600,
            )
            .unwrap(),
            vec![
                EvmSettlementBackend::connect(proxied_deployment(&fixture, &proxy), &key).unwrap(),
            ],
            AccessLimits::default(),
        )
        .unwrap();
        let result = service
            .submit_journaled(
                "buyer",
                &coordinator,
                prepared.operation_ref,
                now(),
                timeout,
            )
            .await;
        if timeout.is_zero() {
            assert!(matches!(
                result,
                Err(DurableRelayError::Admission(RelayError::Configuration))
            ));
        } else {
            assert!(matches!(
                result,
                Err(DurableRelayError::Transaction(
                    EvmError::SignedIntentMismatch
                ))
            ));
        }
        assert_eq!(proxy.broadcasts.load(Ordering::SeqCst), 0);
        let diagnostic = coordinator.diagnostics().unwrap();
        assert_eq!(diagnostic[0].broadcast_attempts, 0);
        assert_eq!(diagnostic[0].unknown_attempts, 0);
        assert_eq!(diagnostic[0].stage, Stage::Signed);
        assert_eq!(
            coordinator.ledger().unwrap().reservations()[0].state,
            ReservationState::Reserved
        );
        let retained = coordinator
            .signed_transaction(prepared.operation_ref)
            .unwrap()
            .unwrap();
        assert_eq!(retained.raw(), original.raw());
        assert_eq!(retained.plan(), original.plan());
        assert!(journal
            .resume(&deployment(&fixture), &prepared)
            .unwrap()
            .is_some());
        assert_eq!(service.metrics().submitted, 0);
        assert_eq!(service.metrics().submit_failed, 0);
    }
    assert_eq!(
        provider
            .get_transaction_count(Address::from(plan.sender()))
            .await
            .unwrap(),
        initial_nonce
    );
    assert_eq!(token_balance(&provider, fixture.token, &RECIPIENT).await, 0);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn a_reorged_payment_keeps_its_reservation_and_reuses_the_signed_transaction() {
    use erebus_evm::chain::TxStatus;

    let fixture = fixture().await;
    let root = tempfile::tempdir().expect("state dir");
    let (coordinator, journal, prepared, plan) = coordinated_setup(&fixture, root.path()).await;
    let provider = read_only_provider(&fixture.rpc_url);
    let chain = EvmChain::connect(deployment(&fixture), Duration::from_secs(5))
        .await
        .expect("chain");
    let snapshot: U64 = provider
        .client()
        .request("evm_snapshot", ())
        .await
        .expect("snapshot");
    let operation = prepared.operation_ref;
    let first = chain
        .broadcast_journaled(&coordinator, operation, now())
        .await
        .expect("first send");
    assert_eq!(first.outcome, BroadcastOutcome::Acknowledged);
    assert!(matches!(
        chain.observe(first.hash).await.expect("included"),
        TxStatus::Included(_)
    ));
    assert_eq!(
        token_balance(&provider, fixture.token, &RECIPIENT).await,
        AMOUNT
    );

    let reverted: bool = provider
        .client()
        .request("evm_revert", (snapshot,))
        .await
        .expect("revert");
    assert!(reverted);
    assert_eq!(token_balance(&provider, fixture.token, &RECIPIENT).await, 0);
    assert!(matches!(
        chain.observe(first.hash).await.expect("orphaned"),
        TxStatus::NotFound
    ));
    let evidence = chain
        .finalized_deal_evidence(&prepared.deal_nullifier, ObservationLimits::default())
        .await
        .expect("post-reorg evidence");
    coordinator
        .reconcile(operation, &evidence, now())
        .expect("hold after reorg");
    assert_eq!(
        coordinator.ledger().expect("ledger").reservations()[0].state,
        ReservationState::Reserved
    );
    assert_eq!(
        journal
            .resume(&deployment(&fixture), &prepared)
            .expect("nonce claim"),
        Some(plan)
    );

    let second = chain
        .broadcast_journaled(&coordinator, operation, now())
        .await
        .expect("rebroadcast stored transaction");
    assert_eq!(second.hash, first.hash);
    assert_eq!(second.outcome, BroadcastOutcome::Acknowledged);
    mine_blocks(&provider, 3).await;
    let evidence = chain
        .finalized_deal_evidence(&prepared.deal_nullifier, ObservationLimits::default())
        .await
        .expect("final evidence");
    coordinator
        .reconcile(operation, &evidence, now())
        .expect("finalize");
    assert_eq!(
        coordinator.ledger().expect("ledger").reservations()[0].state,
        ReservationState::Committed
    );
    assert_eq!(
        token_balance(&provider, fixture.token, &RECIPIENT).await,
        AMOUNT
    );
}

async fn uncertain_broadcast_reconciles_without_resubmission(fault: Fault) {
    let fixture = fixture().await;
    let state_root = tempfile::tempdir().expect("state dir");
    let (coordinator, _journal, prepared, _plan) =
        coordinated_setup(&fixture, state_root.path()).await;
    let operation_ref = prepared.operation_ref;
    let now = now();

    let proxy = FaultProxy::start(
        fixture
            .rpc_url
            .strip_prefix("http://")
            .expect("http url")
            .to_owned(),
    )
    .await;
    proxy.set(fault);
    let chain = EvmChain::connect(proxied_deployment(&fixture, &proxy), Duration::from_secs(2))
        .await
        .expect("proxied chain");

    let broadcast = chain
        .broadcast_journaled(&coordinator, operation_ref, now)
        .await
        .expect("uncertain broadcast is recorded");
    assert_eq!(broadcast.outcome, BroadcastOutcome::Unknown);
    assert_eq!(proxy.broadcasts.load(Ordering::SeqCst), 1);
    // The response was dropped, so the attempt is recorded Unknown and nothing is released.
    let diagnostic = coordinator.diagnostics().expect("diagnostics")[0];
    assert_eq!(diagnostic.unknown_attempts, 1);
    assert_eq!(
        coordinator.ledger().expect("ledger").reservations()[0].state,
        ReservationState::Reserved
    );

    // Restart from durable state before resolving the uncertain send.
    drop(coordinator);
    let context = context_for(&fixture);
    let coordinator = Coordinator::open(
        state_root.path().join("coordinator"),
        KeyBytes::new(BUYER_KEY_ADDRESS.to_vec()).expect("buyer key"),
        context.clone(),
        &context,
        &capabilities_for(&fixture),
        policy_for(&fixture),
    )
    .expect("reopen after uncertain broadcast");
    assert_eq!(
        coordinator.diagnostics().expect("diagnostics")[0].unknown_attempts,
        1
    );
    assert_eq!(
        coordinator.ledger().expect("ledger").reservations()[0].state,
        ReservationState::Reserved
    );

    // The transaction did land. Honest chain observation resolves the deal without a new send.
    mine_blocks(&read_only_provider(&fixture.rpc_url), 3).await;
    let honest = EvmChain::connect(deployment(&fixture), Duration::from_secs(5))
        .await
        .expect("chain");
    let evidence = honest
        .finalized_deal_evidence(&prepared.deal_nullifier, ObservationLimits::default())
        .await
        .expect("deal evidence");
    coordinator
        .reconcile(operation_ref, &evidence, now + 1)
        .expect("reconcile");
    assert_eq!(
        coordinator.diagnostics().expect("diagnostics")[0].stage,
        Stage::Finalized
    );
    assert_eq!(
        coordinator.ledger().expect("ledger").reservations()[0].state,
        ReservationState::Committed
    );
    assert_eq!(proxy.broadcasts.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn a_falsified_or_dropped_rpc_response_is_never_payment_evidence() {
    let fixture = fixture().await;
    let state_root = tempfile::tempdir().expect("state dir");
    let (coordinator, _journal, prepared, _plan) =
        coordinated_setup(&fixture, state_root.path()).await;
    let operation_ref = prepared.operation_ref;
    let now = now();

    let proxy = FaultProxy::start(
        fixture
            .rpc_url
            .strip_prefix("http://")
            .expect("http url")
            .to_owned(),
    )
    .await;
    let chain = EvmChain::connect(proxied_deployment(&fixture, &proxy), Duration::from_secs(2))
        .await
        .expect("proxied chain");

    // A falsified hash is not an acknowledgment: the attempt stays Unknown.
    proxy.set(Fault::FalsifyBroadcastHash);
    let broadcast = chain
        .broadcast_journaled(&coordinator, operation_ref, now)
        .await;
    if let Ok(broadcast) = broadcast {
        assert_eq!(broadcast.outcome, BroadcastOutcome::Unknown);
    }
    assert_eq!(
        coordinator.ledger().expect("ledger").reservations()[0].state,
        ReservationState::Reserved
    );

    // A falsified finalized anchor makes observation fail, and a failed observation holds the
    // reservation: an untrusted provider can never release or commit on its own word.
    for fault in [Fault::FalsifyFinalized, Fault::DelayFinalizedResponse] {
        proxy.set(fault);
        assert!(chain
            .finalized_deal_evidence(&prepared.deal_nullifier, ObservationLimits::default())
            .await
            .is_err());
    }
    assert_eq!(
        coordinator.ledger().expect("ledger").reservations()[0].state,
        ReservationState::Reserved
    );
    assert_ne!(
        coordinator.diagnostics().expect("diagnostics")[0].stage,
        Stage::Finalized
    );
}
