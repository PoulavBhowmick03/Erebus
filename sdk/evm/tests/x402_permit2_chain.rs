//! x402 `exact` over the canonical Permit2 and `x402ExactPermit2Proxy` runtime code.
//!
//! The runtime bytes are pinned from Monad testnet (`fixtures/x402-canonical-runtime.json`) and
//! installed at their canonical addresses on Anvil, so this exercises the deployed contracts,
//! not a reimplementation. Requires Foundry: `cd contracts/evm && forge build`, then
//! `cargo test --test x402_permit2_chain -- --ignored`.

use std::io::Write;
use std::net::TcpListener;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::consensus::Transaction as _;
use alloy::network::{EthereumWallet, TransactionBuilder};
use alloy::primitives::{Address, Bytes, U64};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use erebus_core::auth::{authorization_digest, Authorization, Role};
use erebus_core::commitment::{
    commit_agreement, deal_nullifier, CommitmentBlinding, DealNullifier,
};
use erebus_core::domain::DeploymentDomain;
use erebus_core::ids::{AddressBytes, AssetId, BaseUnits, ChainNamespace, KeyBytes};
use erebus_core::service::ServiceRecord;
use erebus_core::terms::{
    AgreementTerms, FeePolicy, Guarantee, GuaranteeSet, SettlementMode, CURRENT_PROTOCOL_VERSION,
};
use erebus_evm::abi;
use erebus_evm::deployment::EvmDeployment;
use erebus_evm::x402::{
    encode_nonce_bitmap_call, nonce_consumed, settled_topic, verify_x402_exact, DealPermit,
    X402ExactEvidence, X402VerificationError, EXACT_PERMIT2_PROXY, PERMIT2,
};
use erebus_transport::disclosure::SelectedAgreement;
use erebus_transport::identity::AuthorizationIdentity;
use serde_json::{json, Value};
use sha3::{Digest, Keccak256};

/// Anvil account 0 pays for the deal; account 1 is the seller acting as its own facilitator.
const BUYER_KEY: [u8; 32] = [
    0xac, 0x09, 0x74, 0xbe, 0xc3, 0x9a, 0x17, 0xe3, 0x6b, 0xa4, 0xa6, 0xb4, 0xd2, 0x38, 0xff, 0x94,
    0x4b, 0xac, 0xb4, 0x78, 0xcb, 0xed, 0x5e, 0xfc, 0xae, 0x78, 0x4d, 0x7b, 0xf4, 0xf2, 0xff, 0x80,
];
const SELLER_GAS_KEY: [u8; 32] = [
    0x59, 0xc6, 0x99, 0x5e, 0x99, 0x8f, 0x97, 0xa5, 0xa0, 0x04, 0x49, 0x66, 0xf0, 0x94, 0x53, 0x89,
    0xdc, 0x9e, 0x86, 0xda, 0xe8, 0x8c, 0x7a, 0x84, 0x12, 0xf4, 0x60, 0x3b, 0x6b, 0x78, 0x69, 0x0d,
];
const PAY_TO: [u8; 20] = [0x3c; 20];

struct Anvil(Child, String);
impl Drop for Anvil {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn anvil() -> Anvil {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let child = Command::new("anvil")
        .args([
            "--port",
            &port.to_string(),
            "--chain-id",
            "31337",
            "--slots-in-an-epoch",
            "1",
            "--silent",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("anvil must be installed");
    let url = format!("http://127.0.0.1:{port}");
    let anvil = Anvil(child, url);
    for _ in 0..100 {
        if reader(&anvil.1).get_chain_id().await.is_ok() {
            return anvil;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("anvil did not become ready");
}

fn reader(url: &str) -> DynProvider {
    ProviderBuilder::new()
        .connect_http(url.parse().unwrap())
        .erased()
}

fn wallet(url: &str, key: &[u8; 32]) -> DynProvider {
    ProviderBuilder::new()
        .wallet(EthereumWallet::from(
            PrivateKeySigner::from_slice(key).unwrap(),
        ))
        .connect_http(url.parse().unwrap())
        .erased()
}

async fn send(
    provider: &DynProvider,
    to: Address,
    calldata: Vec<u8>,
) -> alloy::rpc::types::TransactionReceipt {
    let request = TransactionRequest::default()
        .with_to(to)
        .with_input(Bytes::from(calldata));
    provider
        .send_transaction(request)
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap()
}

async fn call(provider: &DynProvider, to: Address, calldata: Vec<u8>) -> Result<Bytes, String> {
    let request = TransactionRequest::default()
        .with_to(to)
        .with_input(Bytes::from(calldata));
    provider
        .call(request)
        .await
        .map_err(|error| error.to_string())
}

async fn install_canonical_code(url: &str) {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/x402-canonical-runtime.json")).unwrap();
    for name in ["permit2", "x402_exact_permit2_proxy"] {
        let contract = &fixture["contracts"][name];
        let code = hex::decode(
            contract["runtime"]
                .as_str()
                .unwrap()
                .trim_start_matches("0x"),
        )
        .unwrap();
        let hash: [u8; 32] = Keccak256::digest(&code).into();
        assert_eq!(
            format!("0x{}", hex::encode(hash)),
            contract["runtime_keccak256"].as_str().unwrap()
        );
        let address = contract["address"].as_str().unwrap();
        reader(url)
            .raw_request::<_, ()>(
                "anvil_setCode".into(),
                (address, format!("0x{}", hex::encode(&code))),
            )
            .await
            .unwrap();
    }
}

fn token_artifact() -> Vec<u8> {
    let path = format!(
        "{}/../../contracts/evm/out/MockERC20.sol/MockERC20.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let document: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("run forge build")).unwrap();
    let mut code = hex::decode(
        document["bytecode"]["object"]
            .as_str()
            .unwrap()
            .trim_start_matches("0x"),
    )
    .unwrap();
    code.extend_from_slice(&abi::encode_token_constructor("Test", "TEST"));
    code
}

async fn balance(provider: &DynProvider, token: Address, account: &[u8; 20]) -> u128 {
    let word = call(provider, token, abi::encode_balance_of_call(account))
        .await
        .unwrap();
    u128::from_be_bytes(word[16..32].try_into().unwrap())
}

async fn consumed(provider: &DynProvider, owner: &[u8; 20], deal: &DealNullifier) -> bool {
    let word = call(
        provider,
        Address::from(PERMIT2),
        encode_nonce_bitmap_call(owner, deal),
    )
    .await
    .unwrap();
    nonce_consumed(&word[..].try_into().unwrap(), deal)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[tokio::test]
#[ignore = "requires anvil and forge-built contracts"]
async fn deal_permit_settles_once_through_the_canonical_proxy() {
    let chain = anvil().await;
    install_canonical_code(&chain.1).await;
    let buyer = AuthorizationIdentity::from_bytes(&BUYER_KEY).unwrap();
    let buyer_wallet = wallet(&chain.1, &BUYER_KEY);
    let seller = wallet(&chain.1, &SELLER_GAS_KEY);
    let deployed = buyer_wallet
        .send_transaction(
            TransactionRequest::default().with_deploy_code(Bytes::from(token_artifact())),
        )
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    let token = deployed.contract_address.unwrap();
    send(
        &buyer_wallet,
        token,
        abi::encode_mint_call(&buyer.address(), 1_000),
    )
    .await;
    // The one-time Permit2 approval, which is buyer onboarding for this rail.
    send(
        &buyer_wallet,
        token,
        abi::encode_approve_call(&PERMIT2, 1_000),
    )
    .await;

    let deal = DealNullifier::from_bytes(Keccak256::digest(b"erebus x402 test deal").into());
    let permit = DealPermit {
        token: token.0 .0,
        amount: 70,
        deal,
        deadline: now() + 3600,
        to: PAY_TO,
        valid_after: now() - 600,
    };
    let signature = permit.sign(31337, &buyer);
    assert!(!consumed(&seller, &buyer.address(), &deal).await);

    // A seller cannot redirect the payment: the witness fixes the recipient.
    let mut redirected = permit;
    redirected.to = [0x99; 20];
    let proxy = Address::from(EXACT_PERMIT2_PROXY);
    assert!(call(
        &seller,
        proxy,
        redirected.encode_settle_call(&buyer.address(), &signature)
    )
    .await
    .is_err());
    // A signature for another chain does not verify here.
    let foreign = permit.sign(10143, &buyer);
    assert!(call(
        &seller,
        proxy,
        permit.encode_settle_call(&buyer.address(), &foreign)
    )
    .await
    .is_err());

    let receipt = send(
        &seller,
        proxy,
        permit.encode_settle_call(&buyer.address(), &signature),
    )
    .await;
    assert!(receipt.status());
    assert_eq!(balance(&seller, token, &PAY_TO).await, 70);
    assert_eq!(balance(&seller, token, &buyer.address()).await, 930);
    assert!(consumed(&seller, &buyer.address(), &deal).await);
    let logs = receipt.inner.logs();
    assert!(logs.iter().any(|log| log.address() == proxy
        && log.topics().first().map(|t| t.0) == Some(settled_topic())
        && log.data().data.is_empty()));
    let transfer = logs
        .iter()
        .find(|log| log.address() == token)
        .expect("token Transfer in the settling transaction");
    let topics = transfer.topics();
    assert_eq!(
        topics[0].0,
        <[u8; 32]>::from(Keccak256::digest(b"Transfer(address,address,uint256)"))
    );
    assert_eq!(&topics[1].0[12..], &buyer.address());
    assert_eq!(&topics[2].0[12..], &PAY_TO);
    assert_eq!(
        u128::from_be_bytes(transfer.data().data[16..32].try_into().unwrap()),
        70
    );

    // The same deal cannot be paid twice: Permit2 consumed the nonce.
    assert!(call(
        &seller,
        proxy,
        permit.encode_settle_call(&buyer.address(), &signature)
    )
    .await
    .is_err());
    assert_eq!(balance(&seller, token, &PAY_TO).await, 70);
}

#[tokio::test]
#[ignore = "requires anvil and forge-built contracts"]
async fn finalized_chain_evidence_verifies_and_each_mutation_is_rejected() {
    let chain = anvil().await;
    install_canonical_code(&chain.1).await;
    let buyer = AuthorizationIdentity::from_bytes(&BUYER_KEY).unwrap();
    let buyer_wallet = wallet(&chain.1, &BUYER_KEY);
    let seller = wallet(&chain.1, &SELLER_GAS_KEY);
    let reader = reader(&chain.1);

    let deployed = buyer_wallet
        .send_transaction(
            TransactionRequest::default().with_deploy_code(Bytes::from(token_artifact())),
        )
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    let token = deployed.contract_address.unwrap();
    send(
        &buyer_wallet,
        token,
        abi::encode_mint_call(&buyer.address(), 1_000),
    )
    .await;
    send(
        &buyer_wallet,
        token,
        abi::encode_approve_call(&PERMIT2, 1_000),
    )
    .await;

    // The agreement fixes the token, amount, recipient, buyer key, and deployment.
    let namespace = ChainNamespace::new("eip155", "31337").unwrap();
    let terms = AgreementTerms {
        protocol_version: CURRENT_PROTOCOL_VERSION,
        suite_id: 1,
        domain: DeploymentDomain {
            namespace: namespace.clone(),
            settlement_contract: Some(AddressBytes::new(vec![0x55; 20]).unwrap()),
            pool: None,
            verifier_version: 1,
        },
        deal_id: [0x42; 16],
        revision: 1,
        transcript_root: [0; 32],
        buyer_authorization_key: KeyBytes::new(buyer.address().to_vec()).unwrap(),
        seller_authorization_key: KeyBytes::new(PAY_TO.to_vec()).unwrap(),
        payment_recipient: KeyBytes::new(PAY_TO.to_vec()).unwrap(),
        asset: AssetId::new(
            namespace.clone(),
            "erc20",
            &format!("0x{}", hex::encode(token.0 .0)),
        )
        .unwrap(),
        amount: BaseUnits::new(70),
        expiry: now() + 7_200,
        fee_policy: FeePolicy::none(),
        settlement_mode: SettlementMode::PublicBound,
        required_guarantees: GuaranteeSet::from_guarantee(Guarantee::AgreementBoundSettlement),
        settlement_nonce: [0x77; 32],
        service: ServiceRecord {
            resource: "x".into(),
            quantity: BaseUnits::new(1),
            unit: "u".into(),
            access_recipient: KeyBytes::new(buyer.address().to_vec()).unwrap(),
            delivery_deadline: now() + 7_200,
            fulfillment_method: "http-access".into(),
            fulfillment_digest: [0; 32],
        },
    };
    let deal = deal_nullifier(&terms).unwrap();
    let permit = DealPermit {
        token: token.0 .0,
        amount: 70,
        deal,
        deadline: now() + 3_600,
        to: PAY_TO,
        valid_after: now() - 600,
    };
    let signature = permit.sign(31337, &buyer);

    let receipt = send(
        &seller,
        Address::from(EXACT_PERMIT2_PROXY),
        permit.encode_settle_call(&buyer.address(), &signature),
    )
    .await;
    assert!(receipt.status());
    // Advance so the inclusion is behind the local chain's finalized anchor.
    let _: Value = reader
        .client()
        .request("anvil_mine", (U64::from(8),))
        .await
        .unwrap();

    let transaction = reader
        .get_transaction_by_hash(receipt.transaction_hash)
        .await
        .unwrap()
        .expect("settling transaction");
    let input = transaction.input().to_vec();
    let transfer = receipt
        .inner
        .logs()
        .iter()
        .find(|log| log.address() == token)
        .expect("token Transfer");
    let bitmap = call(
        &seller,
        Address::from(PERMIT2),
        encode_nonce_bitmap_call(&buyer.address(), &deal),
    )
    .await
    .unwrap();
    let nonce_bit: [u8; 32] = bitmap[..32].try_into().unwrap();

    let deployment = EvmDeployment::new(namespace, [0x55; 20], 1, "http://127.0.0.1:1").unwrap();
    // Evidence is read from the finalized transaction and receipt, not assumed.
    let topics = transfer.topics();
    let mut transfer_from = [0u8; 20];
    transfer_from.copy_from_slice(&topics[1].0[12..]);
    let mut transfer_to = [0u8; 20];
    transfer_to.copy_from_slice(&topics[2].0[12..]);
    let transfer_amount = u128::from_be_bytes(transfer.data().data[16..32].try_into().unwrap());
    let evidence = X402ExactEvidence {
        finalized: true,
        transaction_to: EXACT_PERMIT2_PROXY,
        calldata: input,
        transfer_from,
        transfer_to,
        transfer_token: token.0 .0,
        transfer_amount,
        nonce_bit,
    };
    verify_x402_exact(&deployment, &terms, &permit, &signature, &evidence)
        .expect("finalized evidence must verify");

    let rejected = |mutate: &dyn Fn(&mut X402ExactEvidence)| {
        let mut mutated = evidence.clone();
        mutate(&mut mutated);
        verify_x402_exact(&deployment, &terms, &permit, &signature, &mutated)
            .expect_err("mutation must be rejected")
    };
    assert_eq!(
        rejected(&|e| e.finalized = false),
        X402VerificationError::NotFinalized
    );
    assert_eq!(
        rejected(&|e| e.transaction_to = [0x99; 20]),
        X402VerificationError::WrongTarget
    );
    assert_eq!(
        rejected(&|e| e.transfer_amount = 71),
        X402VerificationError::TransferMismatch
    );
    assert_eq!(
        rejected(&|e| e.transfer_to = [0x99; 20]),
        X402VerificationError::TransferMismatch
    );
    assert_eq!(
        rejected(&|e| e.nonce_bit = [0u8; 32]),
        X402VerificationError::NonceNotConsumed
    );

    // One rail, one payment: the consumed Permit2 nonce makes any retry revert, and the
    // recipient balance cannot change.
    assert!(call(
        &seller,
        Address::from(EXACT_PERMIT2_PROXY),
        permit.encode_settle_call(&buyer.address(), &signature)
    )
    .await
    .is_err());
    assert_eq!(balance(&seller, token, &PAY_TO).await, 70);
}

fn write_private(path: &Path, bytes: &[u8]) {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
}

fn disclosure(request: &Value) -> (i32, Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_erebus-disclosure"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(request.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let value: Value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
    (output.status.code().unwrap_or(-1), value)
}

#[allow(clippy::too_many_arguments)]
fn signed_agreement(
    namespace: &ChainNamespace,
    token: [u8; 20],
    buyer: &AuthorizationIdentity,
    seller: &AuthorizationIdentity,
    nonce_byte: u8,
    recipient: [u8; 20],
) -> (AgreementTerms, SelectedAgreement) {
    let terms = AgreementTerms {
        protocol_version: CURRENT_PROTOCOL_VERSION,
        suite_id: 1,
        domain: DeploymentDomain {
            namespace: namespace.clone(),
            settlement_contract: Some(AddressBytes::new(EXACT_PERMIT2_PROXY.to_vec()).unwrap()),
            pool: None,
            verifier_version: 1,
        },
        deal_id: [nonce_byte; 16],
        revision: 1,
        transcript_root: [0; 32],
        buyer_authorization_key: KeyBytes::new(buyer.address().to_vec()).unwrap(),
        seller_authorization_key: KeyBytes::new(seller.address().to_vec()).unwrap(),
        payment_recipient: KeyBytes::new(recipient.to_vec()).unwrap(),
        asset: AssetId::new(
            namespace.clone(),
            "erc20",
            &format!("0x{}", hex::encode(token)),
        )
        .unwrap(),
        amount: BaseUnits::new(70),
        expiry: now() + 7_200,
        fee_policy: FeePolicy::none(),
        settlement_mode: SettlementMode::PublicBound,
        required_guarantees: GuaranteeSet::from_guarantee(Guarantee::AgreementBoundSettlement),
        settlement_nonce: [nonce_byte; 32],
        service: ServiceRecord {
            resource: "dataset.snapshot.v1".into(),
            quantity: BaseUnits::new(1),
            unit: "snapshot".into(),
            access_recipient: KeyBytes::new(buyer.address().to_vec()).unwrap(),
            delivery_deadline: now() + 7_200,
            fulfillment_method: "http-access-v1".into(),
            fulfillment_digest: [0; 32],
        },
    };
    let blinding = CommitmentBlinding::from_bytes([nonce_byte.wrapping_add(1); 32]);
    let commitment = commit_agreement(&terms, &blinding).unwrap();
    let sign = |role: Role, key: &AuthorizationIdentity| Authorization {
        role,
        suite_id: 1,
        commitment,
        signature: erebus_core::ids::SignatureBytes::new(
            key.sign_digest(&authorization_digest(&terms.domain, role, &commitment, 1).unwrap())
                .to_vec(),
        )
        .unwrap(),
    };
    let evidence = SelectedAgreement {
        buyer: sign(Role::Buyer, buyer),
        seller: sign(Role::Seller, seller),
        terms,
        blinding,
        transcript_hash_version: 1,
        messages: Vec::new(),
    };
    (evidence.terms.clone(), evidence)
}

#[tokio::test]
#[ignore = "requires anvil and forge-built contracts"]
async fn auditor_verifies_finalized_x402_payment_from_the_grant_alone() {
    let chain = anvil().await;
    install_canonical_code(&chain.1).await;
    let runtime: Value =
        serde_json::from_str(include_str!("fixtures/x402-canonical-runtime.json")).unwrap();
    let runtime_hash = |name: &str| -> [u8; 32] {
        hex::decode(
            runtime["contracts"][name]["runtime_keccak256"]
                .as_str()
                .unwrap()
                .trim_start_matches("0x"),
        )
        .unwrap()
        .try_into()
        .unwrap()
    };
    let buyer = AuthorizationIdentity::from_bytes(&BUYER_KEY).unwrap();
    let buyer_wallet = wallet(&chain.1, &BUYER_KEY);
    let seller = AuthorizationIdentity::from_bytes(&[2; 32]).unwrap();
    let gas = wallet(&chain.1, &SELLER_GAS_KEY);
    let deployed = buyer_wallet
        .send_transaction(
            TransactionRequest::default().with_deploy_code(Bytes::from(token_artifact())),
        )
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    let token = deployed.contract_address.unwrap();
    let mint = send(
        &buyer_wallet,
        token,
        abi::encode_mint_call(&buyer.address(), 1_000),
    )
    .await;
    send(
        &buyer_wallet,
        token,
        abi::encode_approve_call(&PERMIT2, 1_000),
    )
    .await;
    let namespace = ChainNamespace::new("eip155", "31337").unwrap();
    let root = tempfile::tempdir().unwrap();
    // Two distinct endpoint spellings of the same local node: configuration hygiene, not
    // independent providers. A real auditor must use independently operated RPCs.
    let peer_url = chain.1.replace("127.0.0.1", "localhost");
    let deployment = |transaction_hash: &str| {
        json!({"rail":"x402_exact","namespace":"eip155:31337","rpc_url":chain.1,
            "peer_rpc_url":peer_url,"permit2_runtime_hash":format!("0x{}",hex::encode(runtime_hash("permit2"))),
            "proxy_runtime_hash":format!("0x{}",hex::encode(runtime_hash("x402_exact_permit2_proxy"))),
            "transaction_hash":transaction_hash})
    };

    // Agreement A: the honest payment to the agreed recipient.
    let (terms_a, evidence_a) = signed_agreement(
        &namespace,
        token.0 .0,
        &buyer,
        &seller,
        0x1a,
        seller.address(),
    );
    let permit_a = DealPermit {
        token: token.0 .0,
        amount: 70,
        deal: deal_nullifier(&terms_a).unwrap(),
        deadline: now() + 3_600,
        to: seller.address(),
        valid_after: now() - 600,
    };
    let signature_a = permit_a.sign(31337, &buyer);
    let receipt_a = send(
        &gas,
        Address::from(EXACT_PERMIT2_PROXY),
        permit_a.encode_settle_call(&buyer.address(), &signature_a),
    )
    .await;
    assert!(receipt_a.status());
    let hash_a = format!("0x{}", hex::encode(receipt_a.transaction_hash));
    let _: Value = reader(&chain.1)
        .client()
        .request("anvil_mine", (U64::from(8),))
        .await
        .unwrap();

    let evidence_file_a = root.path().join("a.evidence");
    write_private(&evidence_file_a, &evidence_a.encode().unwrap());
    let buyer_key = root.path().join("buyer.key");
    write_private(&buyer_key, &BUYER_KEY);
    let keygen = disclosure(&json!({"method":"keygen","key_file":root.path().join("auditor.key")}));
    assert_eq!(keygen.0, 0, "{keygen:?}");
    let recipient = keygen.1["recipient_public_key"]
        .as_str()
        .unwrap()
        .to_string();
    let grant_a = root.path().join("a.grant");
    let exported = disclosure(&json!({"method":"export","evidence_file":evidence_file_a,
        "issuer_key_file":buyer_key,"recipient_public_key":recipient,"grant_file":grant_a,
        "expires_at":now()+3_600}));
    assert_eq!(exported.0, 0, "{exported:?}");

    // Grant + auditor key + public configuration only: no participant state, no spending key.
    let verified = disclosure(&json!({"method":"verify_payment","grant_file":grant_a,
        "key_file":root.path().join("auditor.key"),
        "expected_issuer":format!("0x{}",hex::encode(buyer.address())),
        "deployment":deployment(&hash_a)}));
    assert_eq!(verified.0, 0, "{verified:?}");
    assert_eq!(verified.1["payment_verified"], true, "{verified:?}");
    assert_eq!(verified.1["agreement_verified"], true);
    assert_eq!(verified.1["delivery_verified"], false);

    // Tampering with the encrypted grant fails closed.
    let mut tampered = std::fs::read(&grant_a).unwrap();
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    let tampered_file = root.path().join("tampered.grant");
    write_private(&tampered_file, &tampered);
    let tampered_result = disclosure(
        &json!({"method":"verify_payment","grant_file":tampered_file,
        "key_file":root.path().join("auditor.key"),
        "expected_issuer":format!("0x{}",hex::encode(buyer.address())),
        "deployment":deployment(&hash_a)}),
    );
    assert_ne!(tampered_result.0, 0, "{tampered_result:?}");

    // A buyer-signed permit that pays a different recipient must be rejected.
    let (terms_b, evidence_b) = signed_agreement(
        &namespace,
        token.0 .0,
        &buyer,
        &seller,
        0x2b,
        seller.address(),
    );
    let permit_b = DealPermit {
        token: token.0 .0,
        amount: 70,
        deal: deal_nullifier(&terms_b).unwrap(),
        deadline: now() + 3_600,
        to: [0x99; 20],
        valid_after: now() - 600,
    };
    let signature_b = permit_b.sign(31337, &buyer);
    let receipt_b = send(
        &gas,
        Address::from(EXACT_PERMIT2_PROXY),
        permit_b.encode_settle_call(&buyer.address(), &signature_b),
    )
    .await;
    assert!(receipt_b.status());
    let hash_b = format!("0x{}", hex::encode(receipt_b.transaction_hash));
    let evidence_file_b = root.path().join("b.evidence");
    write_private(&evidence_file_b, &evidence_b.encode().unwrap());
    let grant_b = root.path().join("b.grant");
    let exported_b = disclosure(&json!({"method":"export","evidence_file":evidence_file_b,
        "issuer_key_file":buyer_key,"recipient_public_key":recipient,"grant_file":grant_b,
        "expires_at":now()+3_600}));
    assert_eq!(exported_b.0, 0, "{exported_b:?}");
    let wrong_recipient = disclosure(&json!({"method":"verify_payment","grant_file":grant_b,
        "key_file":root.path().join("auditor.key"),
        "expected_issuer":format!("0x{}",hex::encode(buyer.address())),
        "deployment":deployment(&hash_b)}));
    assert_ne!(wrong_recipient.0, 0, "{wrong_recipient:?}");

    // Neighboring deals and non-settlement evidence cannot verify this agreement.
    let neighbor = disclosure(&json!({"method":"verify_payment","grant_file":grant_a,
        "key_file":root.path().join("auditor.key"),
        "expected_issuer":format!("0x{}",hex::encode(buyer.address())),
        "deployment":deployment(&hash_b)}));
    assert_ne!(neighbor.0, 0, "{neighbor:?}");
    let unrelated = disclosure(&json!({"method":"verify_payment","grant_file":grant_a,
        "key_file":root.path().join("auditor.key"),
        "expected_issuer":format!("0x{}",hex::encode(buyer.address())),
        "deployment":deployment(&format!("0x{}", hex::encode(mint.transaction_hash)))}));
    assert_ne!(unrelated.0, 0, "{unrelated:?}");

    // A broadcast but unmined settlement is pending, never a payment claim; after inclusion
    // and finality the same request verifies.
    let (terms_c, evidence_c) = signed_agreement(
        &namespace,
        token.0 .0,
        &buyer,
        &seller,
        0x3c,
        seller.address(),
    );
    let permit_c = DealPermit {
        token: token.0 .0,
        amount: 70,
        deal: deal_nullifier(&terms_c).unwrap(),
        deadline: now() + 3_600,
        to: seller.address(),
        valid_after: now() - 600,
    };
    let signature_c = permit_c.sign(31337, &buyer);
    let evidence_file_c = root.path().join("c.evidence");
    write_private(&evidence_file_c, &evidence_c.encode().unwrap());
    let grant_c = root.path().join("c.grant");
    let exported_c = disclosure(&json!({"method":"export","evidence_file":evidence_file_c,
        "issuer_key_file":buyer_key,"recipient_public_key":recipient,"grant_file":grant_c,
        "expires_at":now()+3_600}));
    assert_eq!(exported_c.0, 0, "{exported_c:?}");
    let _: Value = reader(&chain.1)
        .client()
        .request("anvil_setAutomine", (false,))
        .await
        .unwrap();
    let pending = gas
        .send_transaction(
            TransactionRequest::default()
                .with_to(Address::from(EXACT_PERMIT2_PROXY))
                .with_input(Bytes::from(
                    permit_c.encode_settle_call(&buyer.address(), &signature_c),
                )),
        )
        .await
        .unwrap();
    let pending_hash = format!("0x{}", hex::encode(*pending.tx_hash()));
    let unmined = disclosure(&json!({"method":"verify_payment","grant_file":grant_c,
        "key_file":root.path().join("auditor.key"),
        "expected_issuer":format!("0x{}",hex::encode(buyer.address())),
        "deployment":deployment(&pending_hash)}));
    assert_eq!(unmined.0, 2, "{unmined:?}");
    assert_eq!(unmined.1["status"], "pending");
    assert_eq!(unmined.1["payment_verified"], false);
    let _: Value = reader(&chain.1)
        .client()
        .request("anvil_mine", (U64::from(8),))
        .await
        .unwrap();
    let finalized = disclosure(&json!({"method":"verify_payment","grant_file":grant_c,
        "key_file":root.path().join("auditor.key"),
        "expected_issuer":format!("0x{}",hex::encode(buyer.address())),
        "deployment":deployment(&pending_hash)}));
    assert_eq!(finalized.0, 0, "{finalized:?}");
    assert_eq!(finalized.1["payment_verified"], true, "{finalized:?}");
}
