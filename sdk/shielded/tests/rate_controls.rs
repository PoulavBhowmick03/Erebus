//! Per-endpoint history read-rate controls: bounds, defaults and bounded burst pacing.
//!
//! The binaries cap the configured rate at `DEFAULT_MAX_REQUESTS_PER_SECOND`; the library
//! rejects zero and paces every read through its own per-endpoint pacer.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{extract::State, routing::post, Json, Router};
use erebus_shielded_prover::rpc::{PoolRpc, RpcError, DEFAULT_MAX_REQUESTS_PER_SECOND};
use serde_json::{json, Value};

#[test]
fn zero_rate_is_refused_and_positive_rates_are_accepted() {
    assert_eq!(DEFAULT_MAX_REQUESTS_PER_SECOND, 40);
    let rpc = || PoolRpc::new("http://127.0.0.1:1", 31_337, [3; 20]).unwrap();
    assert!(matches!(
        rpc().with_max_requests_per_second(0),
        Err(RpcError::Configuration)
    ));
    for rate in [1, 8, 40, 41] {
        assert!(
            rpc().with_max_requests_per_second(rate).is_ok(),
            "the library accepts any positive rate; the 1..=40 cap is the binaries' config policy"
        );
    }
}

#[derive(Default)]
struct Arrivals {
    at: Mutex<Vec<Instant>>,
}

async fn chain_id(State(state): State<Arc<Arrivals>>, Json(request): Json<Value>) -> Json<Value> {
    state.at.lock().unwrap().push(Instant::now());
    let result = match request["method"].as_str() {
        Some("eth_chainId") => json!("0x7a69"),
        Some("eth_blockNumber") => json!("0x2"),
        Some("eth_getBlockByNumber") => json!({
            "number": "0x2",
            "hash": format!("0x{}", "11".repeat(32)),
            "parentHash": format!("0x{}", "22".repeat(32)),
            "timestamp": "0x64"
        }),
        other => panic!("unexpected method {other:?}"),
    };
    Json(json!({"jsonrpc": "2.0", "id": request["id"], "result": result}))
}

#[tokio::test]
async fn pacing_bounds_the_burst_and_spaces_later_requests() {
    let state = Arc::new(Arrivals::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn({
        let state = state.clone();
        async move {
            axum::serve(
                listener,
                Router::new().route("/", post(chain_id)).with_state(state),
            )
            .await
            .unwrap();
        }
    });
    let rpc = PoolRpc::new(&url, 31_337, [3; 20])
        .unwrap()
        .with_max_requests_per_second(5)
        .unwrap();
    let started = Instant::now();
    for _ in 0..6 {
        rpc.finalized_head().await.unwrap();
    }
    let elapsed = started.elapsed();
    server.abort();

    let at = state.at.lock().unwrap();
    assert!(
        at.len() >= 14,
        "every paced request must reach the provider: {}",
        at.len()
    );
    let burst = at[7].duration_since(at[0]);
    assert!(
        burst < Duration::from_millis(400),
        "the first eight requests are a bounded local burst: {burst:?}"
    );
    let tail = at.last().unwrap().duration_since(at[7]);
    assert!(
        tail >= Duration::from_millis(600),
        "after the burst, six requests at 5/s must be spaced: {tail:?}"
    );
    assert!(
        elapsed <= Duration::from_secs(20),
        "pacing must not deadlock or stall: {elapsed:?}"
    );
}
