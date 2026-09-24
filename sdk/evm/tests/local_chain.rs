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

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::network::{EthereumWallet, TransactionBuilder};
use alloy::primitives::{Address, Bytes};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use k256::ecdsa::SigningKey;

use erebus_core::auth::{authorization_digest, Authorization, Role};
use erebus_core::commitment::{commit_agreement, CommitmentBlinding, DealNullifier};
use erebus_core::domain::DeploymentDomain;
use erebus_core::ids::{
    AddressBytes, AssetId, BaseUnits, ChainNamespace, KeyBytes, SignatureBytes,
};
use erebus_core::service::ServiceRecord;
use erebus_core::settlement::{Finality, PaymentStatus, PreparedSettlement};
use erebus_core::terms::{
    AgreementTerms, FeePolicy, Guarantee, GuaranteeSet, SettlementMode, CURRENT_PROTOCOL_VERSION,
};
use erebus_evm::backend::EvmSettlementBackend;
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
    Fixture {
        _anvil: anvil,
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
    // A local chain has no reorg risk, so inclusion is declared final here. A testnet
    // deployment keeps the conservative default of one confirmation.
    let backend = EvmSettlementBackend::connect(deployment(&fixture), &RELAYER_KEY)
        .expect("backend")
        .with_confirmations(0);
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

    let receipt = backend.settle(&prepared).await.expect("settle");
    assert_eq!(receipt.payment, PaymentStatus::Settled);
    assert_eq!(receipt.finality, Finality::Finalized);
    assert!(receipt.payment_finalized());
    assert!(!receipt.delivery_issued());
    assert!(!receipt.is_complete(), "payment alone is not completion");
    assert_eq!(receipt.deal_commitment, commitment);
    assert_eq!(receipt.deal_nullifier, prepared.deal_nullifier);
    assert!(receipt
        .verified_guarantees
        .contains(Guarantee::AgreementBoundSettlement));

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
    let backend =
        EvmSettlementBackend::connect(deployment(&fixture), &RELAYER_KEY).expect("backend");
    let prepared = prepared(&fixture);
    backend.settle(&prepared).await.expect("first settle");

    // The contract consumed the deal nullifier, so the same authorized revision cannot settle a
    // second time even though both authorizations are still valid and unexpired.
    if let Ok(receipt) = backend.settle(&prepared).await {
        assert_eq!(receipt.payment, PaymentStatus::Reverted);
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn the_contract_rejects_mutated_terms_and_expiry() {
    let fixture = fixture().await;
    let backend =
        EvmSettlementBackend::connect(deployment(&fixture), &RELAYER_KEY).expect("backend");
    let prepared = prepared(&fixture);

    // A relayer that bypasses the adapter's local checks and mutates the amount still fails on
    // chain, because the commitment no longer opens and the signatures do not verify.
    let mut mutated_terms = fixture.terms.clone();
    mutated_terms.amount = BaseUnits::new(AMOUNT + 1);
    let original = SettlementEvidence::decode(&prepared.backend_evidence).expect("evidence");
    let mutated = SettlementEvidence {
        terms: mutated_terms.encode().expect("terms"),
        ..original.clone()
    };
    let mutated_prepared = PreparedSettlement {
        backend_evidence: mutated.encode(),
        ..prepared.clone()
    };
    let mutated_rejected = match backend.settle(&mutated_prepared).await {
        Err(_) => true,
        Ok(receipt) => receipt.payment == PaymentStatus::Reverted,
    };
    assert!(mutated_rejected, "mutated amount must not settle");

    // An expired agreement is rejected by the contract's timestamp check.
    let expired = PreparedSettlement {
        backend_evidence: SettlementEvidence {
            terms: {
                let mut terms = fixture.terms.clone();
                terms.expiry = now() - 1;
                terms.encode().expect("terms")
            },
            ..original
        }
        .encode(),
        ..prepared
    };
    let result = backend.settle(&expired).await;
    assert!(
        result.is_err() || result.expect("receipt").payment == PaymentStatus::Reverted,
        "an expired agreement must not settle"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn an_unrelated_successful_receipt_is_not_settlement_evidence() {
    let fixture = fixture().await;
    let backend = EvmSettlementBackend::connect(deployment(&fixture), &RELAYER_KEY)
        .expect("backend")
        .with_confirmations(0);
    let prepared = prepared(&fixture);
    let relayer = provider(&fixture.rpc_url, &RELAYER_KEY);
    let unrelated = send(&relayer, Address::from(RECIPIENT), Vec::new()).await;

    assert_eq!(
        backend.verify(&prepared, &unrelated).await,
        Err(EvmError::ReceiptMismatch)
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn the_rpc_chain_must_match_the_configured_chain() {
    let fixture = fixture().await;
    let prepared = prepared(&fixture);
    let mut wrong_chain = deployment(&fixture);
    wrong_chain.chain_id = 1;
    let backend = EvmSettlementBackend::connect(wrong_chain, &RELAYER_KEY).expect("backend");

    assert_eq!(
        backend.submit(&prepared).await,
        Err(EvmError::ChainIdMismatch {
            expected: 1,
            found: 31_337,
        })
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn prepared_routing_fields_must_match_the_evidence() {
    let fixture = fixture().await;
    let backend =
        EvmSettlementBackend::connect(deployment(&fixture), &RELAYER_KEY).expect("backend");
    let prepared = PreparedSettlement {
        deal_nullifier: DealNullifier::from_bytes([0xFF; 32]),
        ..prepared(&fixture)
    };

    assert_eq!(
        backend.submit(&prepared).await,
        Err(EvmError::PreparedMismatch)
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
