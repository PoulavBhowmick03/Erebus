//! Measured x402 service-model comparison (roadmap M8: per-request vs prepaid vs batched).
//!
//! EXPERIMENT, not product. The supported rail is per-request `exact` through
//! `erebus_evm::x402::DealPermit`. The two alternatives exist only in this file:
//! - prepaid allocation: one `exact` payment for N units, then buyer-signed off-chain draws
//!   against a durable seller ledger;
//! - batched usage: one canonical x402 `upto` permit for N units, a durable usage ledger, and one
//!   settlement of the measured usage.
//!
//! Every settlement runs on the canonical Permit2, exact proxy, and upto proxy runtime pinned
//! from Monad testnet. Latency is Anvil with one-second blocks: it measures this protocol's
//! round trips, not Monad inclusion or finality. Gas is EVM gas used; the gas limit is reported
//! too because Monad charges the limit. Run with
//! `cargo test --test x402_service_models -- --ignored --nocapture` after `forge build`.

use std::fs::OpenOptions;
use std::io::Write;
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use alloy::network::{EthereumWallet, TransactionBuilder};
use alloy::primitives::{Address, Bytes, B256};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::rpc::types::{TransactionReceipt, TransactionRequest};
use alloy::signers::local::PrivateKeySigner;
use erebus_core::commitment::DealNullifier;
use erebus_evm::abi;
use erebus_evm::x402::{
    domain_separator, transfer_topic, DealPermit, EXACT_PERMIT2_PROXY, PERMIT2,
};
use erebus_transport::identity::AuthorizationIdentity;
use serde_json::{json, Value};
use sha3::{Digest, Keccak256};

const BUYER_KEY: [u8; 32] = [0x15; 32];
/// Anvil account 1: the seller's facilitator gas account.
const SELLER_KEY: [u8; 32] = [
    0x59, 0xc6, 0x99, 0x5e, 0x99, 0x8f, 0x97, 0xa5, 0xa0, 0x04, 0x49, 0x66, 0xf0, 0x94, 0x53, 0x89,
    0xdc, 0x9e, 0x86, 0xda, 0xe8, 0x8c, 0x7a, 0x84, 0x12, 0xf4, 0x60, 0x3b, 0x6b, 0x78, 0x69, 0x0d,
];
const PAY_TO: [u8; 20] = [0x3c; 20];
const UPTO_PROXY: [u8; 20] = [
    0x40, 0x20, 0xa4, 0xf3, 0xb7, 0xb9, 0x0c, 0xca, 0x42, 0x3b, 0x9f, 0xab, 0xcc, 0x0c, 0xe5, 0x7c,
    0x6c, 0x24, 0x00, 0x02,
];
const REQUESTS: u128 = 20;
const PRICE: u128 = 10;
const GAS_LIMIT: u64 = 250_000;
const CRASH_AFTER: u128 = 7;

struct Anvil(Child);
impl Drop for Anvil {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn keccak(bytes: &[u8]) -> [u8; 32] {
    Keccak256::digest(bytes).into()
}

fn word(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[32 - bytes.len()..].copy_from_slice(bytes);
    out
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn micros(duration: Duration) -> u128 {
    duration.as_micros()
}

/// One canonical x402 `upto` authorization: a cap the facilitator may settle at most once.
struct UptoPermit {
    token: [u8; 20],
    cap: u128,
    nonce: [u8; 32],
    deadline: u64,
    to: [u8; 20],
    facilitator: [u8; 20],
    valid_after: u64,
}

impl UptoPermit {
    fn digest(&self, chain_id: u64) -> [u8; 32] {
        let permitted = keccak(
            &[
                keccak(b"TokenPermissions(address token,uint256 amount)"),
                word(&self.token),
                word(&self.cap.to_be_bytes()),
            ]
            .concat(),
        );
        let witness = keccak(
            &[
                keccak(b"Witness(address to,address facilitator,uint256 validAfter)"),
                word(&self.to),
                word(&self.facilitator),
                word(&self.valid_after.to_be_bytes()),
            ]
            .concat(),
        );
        let structure = keccak(
            &[
                keccak(b"PermitWitnessTransferFrom(TokenPermissions permitted,address spender,uint256 nonce,uint256 deadline,Witness witness)TokenPermissions(address token,uint256 amount)Witness(address to,address facilitator,uint256 validAfter)"),
                permitted,
                word(&UPTO_PROXY),
                self.nonce,
                word(&self.deadline.to_be_bytes()),
                witness,
            ]
            .concat(),
        );
        keccak(&[&b"\x19\x01"[..], &domain_separator(chain_id), &structure].concat())
    }

    fn settle_call(&self, amount: u128, owner: &[u8; 20], signature: &[u8; 65]) -> Vec<u8> {
        let selector = keccak(b"settle(((address,uint256),uint256,uint256),uint256,address,(address,address,uint256),bytes)");
        let mut out = selector[..4].to_vec();
        for head in [
            word(&self.token),
            word(&self.cap.to_be_bytes()),
            self.nonce,
            word(&self.deadline.to_be_bytes()),
            word(&amount.to_be_bytes()),
            word(owner),
            word(&self.to),
            word(&self.facilitator),
            word(&self.valid_after.to_be_bytes()),
            word(&(10u64 * 32).to_be_bytes()),
            word(&65u64.to_be_bytes()),
        ] {
            out.extend_from_slice(&head);
        }
        out.extend_from_slice(signature);
        out.extend_from_slice(&[0u8; 31]);
        out
    }
}

fn sign(identity: &AuthorizationIdentity, digest: &[u8; 32]) -> [u8; 65] {
    let mut signature = identity.sign_digest(digest);
    signature[64] += 27;
    signature
}

/// Durable, fsynced, append-only seller ledger: what both experimental models rely on.
struct Ledger(std::path::PathBuf);
impl Ledger {
    fn append(&self, line: &str) -> Duration {
        let started = Instant::now();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.0)
            .unwrap();
        writeln!(file, "{line}").unwrap();
        file.sync_all().unwrap();
        started.elapsed()
    }
    fn entries(&self) -> usize {
        std::fs::read_to_string(&self.0)
            .map(|text| text.lines().count())
            .unwrap_or(0)
    }
}

async fn chain() -> (Anvil, DynProvider, DynProvider, DynProvider) {
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
            "--block-time",
            "1",
            "--slots-in-an-epoch",
            "1",
            "--silent",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("anvil must be installed");
    let url = format!("http://127.0.0.1:{port}");
    let anvil = Anvil(child);
    let reader: DynProvider = ProviderBuilder::new()
        .connect_http(url.parse().unwrap())
        .erased();
    for _ in 0..100 {
        if reader.get_chain_id().await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let wallet = |key: &[u8; 32]| -> DynProvider {
        ProviderBuilder::new()
            .wallet(EthereumWallet::from(
                PrivateKeySigner::from_slice(key).unwrap(),
            ))
            .connect_http(url.parse().unwrap())
            .erased()
    };
    for fixture in [
        include_str!("fixtures/x402-canonical-runtime.json"),
        include_str!("fixtures/x402-upto-runtime.json"),
    ] {
        let fixture: Value = serde_json::from_str(fixture).unwrap();
        for contract in fixture["contracts"].as_object().unwrap().values() {
            let code = hex::decode(
                contract["runtime"]
                    .as_str()
                    .unwrap()
                    .trim_start_matches("0x"),
            )
            .unwrap();
            assert_eq!(
                format!("0x{}", hex::encode(keccak(&code))),
                contract["runtime_keccak256"].as_str().unwrap()
            );
            reader
                .raw_request::<_, ()>(
                    "anvil_setCode".into(),
                    (contract["address"].clone(), contract["runtime"].clone()),
                )
                .await
                .unwrap();
        }
    }
    let buyer = AuthorizationIdentity::from_bytes(&BUYER_KEY).unwrap();
    reader
        .raw_request::<_, ()>(
            "anvil_setBalance".into(),
            (
                format!("0x{}", hex::encode(buyer.address())),
                "0xde0b6b3a7640000",
            ),
        )
        .await
        .unwrap();
    (anvil, reader, wallet(&BUYER_KEY), wallet(&SELLER_KEY))
}

async fn send(
    provider: &DynProvider,
    to: Address,
    data: Vec<u8>,
    gas: Option<u64>,
) -> (TransactionReceipt, Duration) {
    let mut request = TransactionRequest::default()
        .with_to(to)
        .with_input(Bytes::from(data));
    if let Some(gas) = gas {
        request = request.with_gas_limit(gas);
    }
    let started = Instant::now();
    let receipt = provider
        .send_transaction(request)
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap();
    (receipt, started.elapsed())
}

async fn reverts(provider: &DynProvider, from: [u8; 20], to: Address, data: Vec<u8>) -> bool {
    let request = TransactionRequest::default()
        .with_from(Address::from(from))
        .with_to(to)
        .with_input(Bytes::from(data));
    provider.call(request).await.is_err()
}

async fn transfers(reader: &DynProvider, token: Address, from: &[u8; 20], start: u64) -> usize {
    let filter = alloy::rpc::types::Filter::new()
        .address(token)
        .event_signature(B256::from(transfer_topic()))
        .topic1(B256::from(word(from)))
        .topic2(B256::from(word(&PAY_TO)))
        .from_block(start);
    reader.get_logs(&filter).await.unwrap().len()
}

fn summary(latencies: &[Duration]) -> Value {
    let mut sorted: Vec<u128> = latencies.iter().map(|d| d.as_millis()).collect();
    sorted.sort_unstable();
    let total: u128 = sorted.iter().sum();
    json!({"count": sorted.len(), "mean_ms": total / sorted.len().max(1) as u128,
        "p50_ms": sorted[sorted.len() / 2], "max_ms": sorted[sorted.len() - 1]})
}

#[tokio::test]
#[ignore = "experiment: requires anvil and forge-built contracts; measures x402 service models"]
async fn measure_per_request_prepaid_and_batched_x402_models() {
    let (_anvil, reader, buyer_wallet, seller) = chain().await;
    let buyer = AuthorizationIdentity::from_bytes(&BUYER_KEY).unwrap();
    let seller_address = PrivateKeySigner::from_slice(&SELLER_KEY)
        .unwrap()
        .address()
        .0
         .0;
    let artifact: Value = serde_json::from_str(
        &std::fs::read_to_string(format!(
            "{}/../../contracts/evm/out/MockERC20.sol/MockERC20.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("run forge build"),
    )
    .unwrap();
    let mut code = hex::decode(
        artifact["bytecode"]["object"]
            .as_str()
            .unwrap()
            .trim_start_matches("0x"),
    )
    .unwrap();
    code.extend_from_slice(&abi::encode_token_constructor("Test", "TEST"));
    let token = buyer_wallet
        .send_transaction(TransactionRequest::default().with_deploy_code(Bytes::from(code)))
        .await
        .unwrap()
        .get_receipt()
        .await
        .unwrap()
        .contract_address
        .unwrap();
    send(
        &buyer_wallet,
        token,
        abi::encode_mint_call(&buyer.address(), 10_000),
        None,
    )
    .await;
    send(
        &buyer_wallet,
        token,
        abi::encode_approve_call(&PERMIT2, 10_000),
        None,
    )
    .await;
    let chain_id = 31337;
    let directory = tempfile::tempdir().unwrap();
    let deal = |label: &str, index: u128| {
        DealNullifier::from_bytes(keccak(format!("{label}-{index}").as_bytes()))
    };

    // A. Supported: per-request exact. Each request is one buyer permit and one settlement.
    let start_a = reader.get_block_number().await.unwrap() + 1;
    let (mut latency_a, mut gas_a, mut signing_a) = (Vec::new(), 0u128, Duration::ZERO);
    for index in 0..REQUESTS {
        let permit = DealPermit {
            token: token.0 .0,
            amount: PRICE,
            deal: deal("per-request", index),
            deadline: now() + 3600,
            to: PAY_TO,
            valid_after: now() - 60,
        };
        let started = Instant::now();
        let signature = permit.sign(chain_id, &buyer);
        signing_a += started.elapsed();
        let (receipt, latency) = send(
            &seller,
            Address::from(EXACT_PERMIT2_PROXY),
            permit.encode_settle_call(&buyer.address(), &signature),
            Some(GAS_LIMIT),
        )
        .await;
        assert!(receipt.status());
        latency_a.push(latency);
        gas_a += u128::from(receipt.gas_used);
    }
    let transfers_a = transfers(&reader, token, &buyer.address(), start_a).await;

    // B. Experimental: prepaid allocation. One exact payment for N units, then signed draws.
    let start_b = reader.get_block_number().await.unwrap() + 1;
    let prepay = DealPermit {
        token: token.0 .0,
        amount: PRICE * REQUESTS,
        deal: deal("prepaid", 0),
        deadline: now() + 3600,
        to: PAY_TO,
        valid_after: now() - 60,
    };
    let signature = prepay.sign(chain_id, &buyer);
    let (receipt, prepay_latency) = send(
        &seller,
        Address::from(EXACT_PERMIT2_PROXY),
        prepay.encode_settle_call(&buyer.address(), &signature),
        Some(GAS_LIMIT),
    )
    .await;
    assert!(receipt.status());
    let gas_b = u128::from(receipt.gas_used);
    let ledger_b = Ledger(directory.path().join("prepaid.ledger"));
    let (mut draw_b, mut sign_b, mut verify_b) = (Vec::new(), Duration::ZERO, Duration::ZERO);
    for index in 0..REQUESTS {
        let started = Instant::now();
        let message = keccak(
            &[
                &b"EREBUS_EXPERIMENTAL_PREPAID_DRAW"[..],
                prepay.deal.as_bytes(),
                &index.to_be_bytes(),
            ]
            .concat(),
        );
        let draw = sign(&buyer, &message);
        sign_b += started.elapsed();
        let checked = Instant::now();
        let key = k256::ecdsa::VerifyingKey::recover_from_prehash(
            &message,
            &k256::ecdsa::Signature::from_slice(&draw[..64]).unwrap(),
            k256::ecdsa::RecoveryId::from_byte(draw[64] - 27).unwrap(),
        )
        .unwrap();
        assert_eq!(
            &keccak(&key.to_encoded_point(false).as_bytes()[1..])[12..],
            &buyer.address()
        );
        verify_b += checked.elapsed();
        let durable = ledger_b.append(&format!("draw {index}"));
        draw_b.push(started.elapsed().max(durable));
    }
    let transfers_b = transfers(&reader, token, &buyer.address(), start_b).await;

    // C. Experimental: batched usage. One upto cap, a durable usage ledger, one settlement.
    let start_c = reader.get_block_number().await.unwrap() + 1;
    let used = REQUESTS - 3;
    let upto = UptoPermit {
        token: token.0 .0,
        cap: PRICE * REQUESTS,
        nonce: keccak(b"batched-0"),
        deadline: now() + 3600,
        to: PAY_TO,
        facilitator: seller_address,
        valid_after: now() - 60,
    };
    let started = Instant::now();
    let upto_signature = sign(&buyer, &upto.digest(chain_id));
    let upto_signing = started.elapsed();
    let ledger_c = Ledger(directory.path().join("batched.ledger"));
    let usage_c: Vec<Duration> = (0..used)
        .map(|index| ledger_c.append(&format!("use {index}")))
        .collect();
    let over_cap = upto.settle_call(upto.cap + 1, &buyer.address(), &upto_signature);
    assert!(
        reverts(&reader, seller_address, Address::from(UPTO_PROXY), over_cap).await,
        "upto cannot exceed the cap"
    );
    let other_facilitator = upto.settle_call(PRICE * used, &buyer.address(), &upto_signature);
    assert!(
        reverts(
            &reader,
            buyer.address(),
            Address::from(UPTO_PROXY),
            other_facilitator
        )
        .await,
        "only the named facilitator settles"
    );
    let (receipt, batch_latency) = send(
        &seller,
        Address::from(UPTO_PROXY),
        upto.settle_call(PRICE * used, &buyer.address(), &upto_signature),
        Some(GAS_LIMIT),
    )
    .await;
    assert!(receipt.status());
    let gas_c = u128::from(receipt.gas_used);
    let replay = upto.settle_call(PRICE, &buyer.address(), &upto_signature);
    assert!(
        reverts(&reader, seller_address, Address::from(UPTO_PROXY), replay).await,
        "an upto permit settles once"
    );
    let transfers_c = transfers(&reader, token, &buyer.address(), start_c).await;

    // Recovery: the seller crashes after CRASH_AFTER of N requests.
    // A: every served request is already a final on-chain payment; nothing is outstanding.
    // B: the buyer has paid N units; the reopened ledger decides how many were consumed.
    let crashed_b = Ledger(directory.path().join("prepaid-crash.ledger"));
    (0..CRASH_AFTER).for_each(|index| {
        crashed_b.append(&format!("draw {index}"));
    });
    let reopened_b = Ledger(directory.path().join("prepaid-crash.ledger")).entries() as u128;
    // C: unsettled usage is a seller receivable. Before the deadline the retained permit recovers
    // it; after the deadline the seller cannot settle and absorbs the loss.
    let crashed_c = Ledger(directory.path().join("batched-crash.ledger"));
    (0..CRASH_AFTER).for_each(|index| {
        crashed_c.append(&format!("use {index}"));
    });
    let reopened_c = Ledger(directory.path().join("batched-crash.ledger")).entries() as u128;
    let in_time = UptoPermit {
        nonce: keccak(b"batched-recover"),
        ..upto
    };
    let in_time_signature = sign(&buyer, &in_time.digest(chain_id));
    let (recovered, _) = send(
        &seller,
        Address::from(UPTO_PROXY),
        in_time.settle_call(PRICE * reopened_c, &buyer.address(), &in_time_signature),
        Some(GAS_LIMIT),
    )
    .await;
    let late = UptoPermit {
        nonce: keccak(b"batched-late"),
        deadline: now() + 30,
        ..upto
    };
    let late_signature = sign(&buyer, &late.digest(chain_id));
    reader
        .raw_request::<_, serde_json::Value>("evm_increaseTime".into(), (120u64,))
        .await
        .unwrap();
    reader
        .raw_request::<_, serde_json::Value>("evm_mine".into(), ())
        .await
        .unwrap();
    let expired = reverts(
        &reader,
        seller_address,
        Address::from(UPTO_PROXY),
        late.settle_call(PRICE * reopened_c, &buyer.address(), &late_signature),
    )
    .await;

    assert_eq!(
        (transfers_a, transfers_b, transfers_c),
        (REQUESTS as usize, 1, 1)
    );
    assert!(recovered.status() && expired);
    let results = json!({
        "environment": "Anvil, chain 31337, --block-time 1, canonical Permit2/exact/upto runtime pinned from Monad testnet; not Monad latency",
        "requests": REQUESTS, "unit_price": PRICE, "gas_limit_per_settlement": GAS_LIMIT,
        "per_request_exact": {"status": "supported product rail",
            "onchain_settlements": REQUESTS, "gas_used_total": gas_a, "gas_used_per_request": gas_a / REQUESTS,
            "gas_limit_charged_total": u128::from(GAS_LIMIT) * REQUESTS,
            "request_latency_submit_to_receipt": summary(&latency_a),
            "buyer_signing_us_per_request": micros(signing_a) / REQUESTS,
            "public_on_chain": "one transfer per request: request count, timing, and per-request amount",
            "accounting": "the chain is the ledger; each request is independently final",
            "recovery_after_crash": {"served": CRASH_AFTER, "paid": CRASH_AFTER, "outstanding_units": 0}},
        "prepaid_allocation": {"status": "experimental only, not a product rail",
            "onchain_settlements": 1, "gas_used_total": gas_b, "gas_used_per_request": gas_b / REQUESTS,
            "gas_limit_charged_total": GAS_LIMIT,
            "prepay_latency_ms": prepay_latency.as_millis(),
            "draw_latency_including_fsync": summary(&draw_b),
            "buyer_signing_us_per_draw": micros(sign_b) / REQUESTS, "seller_verify_us_per_draw": micros(verify_b) / REQUESTS,
            "public_on_chain": "one transfer of the prepaid total; per-request use is off chain",
            "accounting": "seller's durable ledger is authoritative for consumption; the buyer cannot verify draws on chain",
            "recovery_after_crash": {"served": CRASH_AFTER, "ledger_entries_after_reopen": reopened_b,
                "buyer_prepaid_exposure_units": REQUESTS - reopened_b, "refund": "requires seller cooperation"}},
        "batched_upto": {"status": "experimental only, not a product rail",
            "onchain_settlements": 1, "units_used": used, "gas_used_total": gas_c, "gas_used_per_request": gas_c / used,
            "gas_limit_charged_total": GAS_LIMIT,
            "cap_signing_us": micros(upto_signing),
            "usage_record_latency_including_fsync": summary(&usage_c),
            "settlement_latency_ms": batch_latency.as_millis(),
            "public_on_chain": "one transfer of total usage; cap and request count stay off chain",
            "accounting": "seller's usage ledger decides the charge up to the signed cap; the buyer cannot verify per-request use",
            "recovery_after_crash": {"served": CRASH_AFTER, "ledger_entries_after_reopen": reopened_c,
                "settled_before_deadline": recovered.status(), "settlement_after_deadline_reverts": expired,
                "seller_receivable_until_settlement_units": reopened_c, "buyer_max_exposure_units": REQUESTS}},
    });
    println!("X402_SERVICE_MODELS {results}");
}
