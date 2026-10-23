//! Per-method gas limits: the estimate precheck refuses before any claim, and a claim saved at
//! the old shared limit resumes at that limit after an upgrade.

use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

use axum::{extract::State, routing::post, Json, Router};
use erebus_core::{
    commitment::{DealCommitment, DealNullifier},
    domain::DeploymentDomain,
    ids::{AddressBytes, ChainNamespace},
    settlement::PreparedSettlement,
    shielded::SHIELDED_GUARANTEES,
    terms::{GuaranteeSet, SettlementMode},
};
use erebus_evm::{
    chain::{Eip1559Fees, EvmChain, NonceError, SignerJournal},
    deployment::EvmDeployment,
};
use erebus_shielded_prover::{
    gas_policy::{GasPolicy, APPROVE_GAS_LIMIT, DEPOSIT_GAS_LIMIT, SETTLE_GAS_LIMIT},
    operations::{planned_gas_limit, OperationError},
};
use serde_json::{json, Value};

const POOL: [u8; 20] = [0x11; 20];
const TOKEN: [u8; 20] = [0x22; 20];
const SENDER: [u8; 20] = [0x33; 20];

#[derive(Clone)]
struct Node {
    estimate: Arc<AtomicU64>,
    estimates: Arc<AtomicUsize>,
    values: Arc<std::sync::Mutex<Vec<String>>>,
}

async fn answer(State(node): State<Node>, Json(request): Json<Value>) -> Json<Value> {
    let result = match request["method"].as_str() {
        Some("eth_chainId") => json!("0x7a69"),
        Some("eth_estimateGas") => {
            node.estimates.fetch_add(1, Ordering::SeqCst);
            node.values.lock().unwrap().push(
                request["params"][0]["value"]
                    .as_str()
                    .unwrap_or("absent")
                    .to_owned(),
            );
            json!(format!("0x{:x}", node.estimate.load(Ordering::SeqCst)))
        }
        Some("eth_getTransactionCount") => json!("0x7"),
        _ => return Json(json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32601}})),
    };
    Json(json!({"jsonrpc":"2.0","id":request["id"],"result":result}))
}

async fn node(estimate: u64) -> (Node, String, tokio::task::JoinHandle<()>) {
    let state = Node {
        estimate: Arc::new(AtomicU64::new(estimate)),
        estimates: Arc::new(AtomicUsize::new(0)),
        values: Arc::new(std::sync::Mutex::new(Vec::new())),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/", post(answer))
        .with_state(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (state, url, task)
}

fn domain() -> DeploymentDomain {
    let pool = AddressBytes::new(POOL.to_vec()).unwrap();
    DeploymentDomain {
        namespace: ChainNamespace::parse("eip155:31337").unwrap(),
        settlement_contract: Some(pool.clone()),
        pool: Some(pool),
        verifier_version: 2,
    }
}

fn stub(operation: u8) -> PreparedSettlement {
    PreparedSettlement {
        operation_ref: [operation; 32],
        deal_commitment: DealCommitment::from_bytes([operation; 32]),
        deal_nullifier: DealNullifier::from_bytes([operation; 32]),
        domain: domain(),
        mode: SettlementMode::Shielded,
        required_guarantees: GuaranteeSet::from_bits(SHIELDED_GUARANTEES).unwrap(),
        backend_evidence: vec![1, 2, 3],
    }
}

async fn chain(url: &str) -> EvmChain {
    let deployment =
        EvmDeployment::new(ChainNamespace::parse("eip155:31337").unwrap(), POOL, 2, url).unwrap();
    EvmChain::connect(deployment, Duration::from_secs(5))
        .await
        .unwrap()
}

fn fees() -> Eip1559Fees {
    Eip1559Fees::new(300_000_000_000, 2_000_000_000).unwrap()
}

#[tokio::test]
async fn an_estimate_above_the_limit_with_margin_refuses_before_any_claim() {
    // 2,000,001 x 1.05 > 2,100,000: refused with nothing written.
    let (state, url, _task) = node(2_000_001).await;
    let chain = chain(&url).await;
    let root = tempfile::tempdir().unwrap();
    let journal = SignerJournal::open(root.path(), 31337, SENDER).unwrap();
    let policy = GasPolicy {
        limit: DEPOSIT_GAS_LIMIT,
        legacy_cap: 3_000_000,
    };
    let refused = planned_gas_limit(&chain, &journal, &stub(5), POOL, &[9; 36], 0, policy).await;
    assert!(matches!(
        refused,
        Err(OperationError::GasLimit {
            estimate: 2_000_001,
            limit: DEPOSIT_GAS_LIMIT
        })
    ));
    assert_eq!(journal.claim_nonce().unwrap(), None);
    // Exactly at the margin the method's own limit is used, never the estimate.
    state.estimate.store(2_000_000, Ordering::SeqCst);
    let limit = planned_gas_limit(&chain, &journal, &stub(5), POOL, &[9; 36], 0, policy)
        .await
        .unwrap();
    assert_eq!(limit, DEPOSIT_GAS_LIMIT);
    assert_eq!(state.estimates.load(Ordering::SeqCst), 2);
    assert_eq!(journal.claim_nonce().unwrap(), None);
}

#[tokio::test]
async fn a_wrap_is_estimated_with_its_value() {
    let (state, url, _task) = node(51_221).await;
    let chain = chain(&url).await;
    let root = tempfile::tempdir().unwrap();
    let journal = SignerJournal::open(root.path(), 31337, SENDER).unwrap();
    let policy = GasPolicy {
        limit: APPROVE_GAS_LIMIT,
        legacy_cap: 3_000_000,
    };
    planned_gas_limit(
        &chain,
        &journal,
        &stub(6),
        TOKEN,
        &[0xd0, 0xe3, 0x0d, 0xb0],
        3_000_000_000_000_000,
        policy,
    )
    .await
    .unwrap();
    assert_eq!(state.values.lock().unwrap().as_slice(), ["0xaa87bee538000"]);
}

#[tokio::test]
async fn a_claim_saved_at_the_old_shared_limit_resumes_without_an_estimate() {
    // An estimate that would now refuse must not block a claim that already exists.
    let (state, url, _task) = node(10_000_000).await;
    let chain = chain(&url).await;
    let root = tempfile::tempdir().unwrap();
    let journal = SignerJournal::open(root.path(), 31337, SENDER).unwrap();
    let deployment = chain.deployment().clone();
    let legacy = journal
        .reserve_call_with_value(
            &deployment,
            &stub(7),
            POOL,
            &[9; 36],
            0,
            4,
            fees(),
            3_000_000,
        )
        .unwrap();
    let policy = GasPolicy {
        limit: SETTLE_GAS_LIMIT,
        legacy_cap: 3_000_000,
    };
    let limit = planned_gas_limit(&chain, &journal, &stub(7), POOL, &[9; 36], 0, policy)
        .await
        .unwrap();
    assert_eq!(limit, 3_000_000);
    assert_eq!(state.estimates.load(Ordering::SeqCst), 0);
    // Reserving at the saved limit resumes the same plan; the new limit would conflict.
    let resumed = chain
        .reserve_nonce_for_call(&journal, &stub(7), POOL, &[9; 36], fees(), limit)
        .await
        .unwrap();
    assert_eq!(resumed.encode(), legacy.encode());
    assert!(matches!(
        chain
            .reserve_nonce_for_call(&journal, &stub(7), POOL, &[9; 36], fees(), SETTLE_GAS_LIMIT)
            .await,
        Err(NonceError::Conflict)
    ));
    // Another operation sees the lane busy before any estimate.
    assert!(matches!(
        planned_gas_limit(&chain, &journal, &stub(8), POOL, &[9; 36], 0, policy).await,
        Err(OperationError::Nonce(NonceError::Busy))
    ));
    assert_eq!(state.estimates.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_saved_limit_above_the_cap_is_refused() {
    let (_state, url, _task) = node(1).await;
    let chain = chain(&url).await;
    let root = tempfile::tempdir().unwrap();
    let journal = SignerJournal::open(root.path(), 31337, SENDER).unwrap();
    let deployment = chain.deployment().clone();
    journal
        .reserve_call_with_value(
            &deployment,
            &stub(9),
            POOL,
            &[9; 36],
            0,
            4,
            fees(),
            5_000_000,
        )
        .unwrap();
    let policy = GasPolicy {
        limit: SETTLE_GAS_LIMIT,
        legacy_cap: 3_000_000,
    };
    assert!(matches!(
        planned_gas_limit(&chain, &journal, &stub(9), POOL, &[9; 36], 0, policy).await,
        Err(OperationError::PlanAboveCap {
            saved: 5_000_000,
            cap: 3_000_000
        })
    ));
}
