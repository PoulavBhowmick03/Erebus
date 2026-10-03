//! x402 `exact` over the canonical Permit2 and `x402ExactPermit2Proxy` runtime code.
//!
//! The runtime bytes are pinned from Monad testnet (`fixtures/x402-canonical-runtime.json`) and
//! installed at their canonical addresses on Anvil, so this exercises the deployed contracts,
//! not a reimplementation. Requires Foundry: `cd contracts/evm && forge build`, then
//! `cargo test --test x402_permit2_chain -- --ignored`.

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::network::{EthereumWallet, TransactionBuilder};
use alloy::primitives::{Address, Bytes};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use erebus_core::commitment::DealNullifier;
use erebus_evm::abi;
use erebus_evm::x402::{
    encode_nonce_bitmap_call, nonce_consumed, settled_topic, DealPermit, EXACT_PERMIT2_PROXY,
    PERMIT2,
};
use erebus_transport::identity::AuthorizationIdentity;
use serde_json::Value;
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
