//! Optional hosted or self-hosted public event indexer for one EVM pool.
//!
//! It never accepts note openings, wallet keys, or proof witnesses. Clients must
//! still verify served blocks against their own RPC and deployment anchor.

use std::{env, error::Error, net::IpAddr, path::PathBuf, sync::Arc, time::Duration};

use axum::{
    extract::{Query, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::Response,
    routing::get,
    Json, Router,
};
use erebus_shielded_prover::{
    index_store::{IndexDomain, IndexStore},
    recovery::{sync_public_index, RecoveryReport},
    rpc::PoolRpc,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::RwLock;

#[derive(Default)]
struct SyncStatus {
    report: Option<RecoveryReport>,
    last_error: Option<String>,
}

#[derive(Clone)]
struct AppState {
    store: Arc<IndexStore>,
    status: Arc<RwLock<SyncStatus>>,
    token: Option<Arc<str>>,
}

#[derive(Deserialize)]
struct BlockQuery {
    after: Option<u64>,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    64
}

fn required(name: &'static str) -> Result<String, Box<dyn Error>> {
    env::var(name).map_err(|_| format!("missing {name}").into())
}

fn fixed_hex<const N: usize>(name: &'static str) -> Result<[u8; N], Box<dyn Error>> {
    let value = required(name)?;
    let value = value
        .strip_prefix("0x")
        .ok_or("hex value needs 0x prefix")?;
    Ok(hex::decode(value)?
        .try_into()
        .map_err(|_| "wrong hex width")?)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let rpc_url = required("EREBUS_INDEXER_RPC")?;
    let domain = IndexDomain {
        chain_id: required("EREBUS_INDEXER_CHAIN_ID")?.parse()?,
        pool: fixed_hex::<20>("EREBUS_INDEXER_POOL")?,
        first_block: required("EREBUS_INDEXER_DEPLOYMENT_BLOCK")?.parse()?,
        first_hash: fixed_hex::<32>("EREBUS_INDEXER_DEPLOYMENT_HASH")?,
    };
    let store = Arc::new(IndexStore::new(
        PathBuf::from(required("EREBUS_INDEXER_ROOT")?).join("pool-index.json"),
        domain,
    )?);
    let rpc = PoolRpc::new(&rpc_url, domain.chain_id, domain.pool)?;
    let confirmations: u64 = required("EREBUS_INDEXER_CONFIRMATIONS")?.parse()?;
    let poll_seconds: u64 = env::var("EREBUS_INDEXER_POLL_SECONDS")
        .unwrap_or_else(|_| "5".to_owned())
        .parse()?;
    if poll_seconds == 0 {
        return Err("poll interval must be nonzero".into());
    }
    let bind: IpAddr = env::var("EREBUS_INDEXER_BIND")
        .unwrap_or_else(|_| "127.0.0.1".to_owned())
        .parse()?;
    let port: u16 = env::var("EREBUS_INDEXER_PORT")
        .unwrap_or_else(|_| "8081".to_owned())
        .parse()?;
    let token = env::var("EREBUS_INDEXER_TOKEN").ok().map(Arc::<str>::from);
    if token.as_ref().is_some_and(|token| token.is_empty()) {
        return Err("indexer token must not be empty".into());
    }
    if !bind.is_loopback() && token.is_none() {
        return Err("non-loopback indexer requires EREBUS_INDEXER_TOKEN".into());
    }
    let state = AppState {
        store: Arc::clone(&store),
        status: Arc::new(RwLock::new(SyncStatus::default())),
        token,
    };
    let polling = {
        let status = Arc::clone(&state.status);
        tokio::spawn(async move {
            loop {
                let result = sync_public_index(&rpc, &store, confirmations).await;
                let mut health = status.write().await;
                match result {
                    Ok((_, report)) => {
                        health.report = Some(report);
                        health.last_error = None;
                    }
                    Err(error) => {
                        health.last_error = Some(error.to_string());
                    }
                }
                drop(health);
                tokio::time::sleep(Duration::from_secs(poll_seconds)).await;
            }
        })
    };
    let app = Router::new()
        .route("/healthz", get(health))
        .route("/v1/blocks", get(blocks))
        .route_layer(middleware::from_fn_with_state(state.clone(), authorize))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind((bind, port)).await?;
    eprintln!("erebus-pool-indexer: listening on {bind}:{port}; public blocks only");
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await;
    polling.abort();
    result?;
    Ok(())
}

async fn authorize(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if let Some(token) = &state.token {
        let expected = format!("Bearer {token}");
        let supplied = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|header| header.to_str().ok());
        if supplied != Some(expected.as_str()) {
            return Err(StatusCode::UNAUTHORIZED);
        }
    }
    Ok(next.run(request).await)
}

async fn health(State(state): State<AppState>) -> Json<Value> {
    let status = state.status.read().await;
    Json(json!({
        "status": if status.last_error.is_some() { "degraded" } else if status.report.is_some() { "ok" } else { "syncing" },
        "through": status.report.map(|report| report.through),
        "root": status.report.map(|report| format!("0x{}", hex::encode(report.root))),
        "last_error": status.last_error,
    }))
}

async fn blocks(
    State(state): State<AppState>,
    Query(query): Query<BlockQuery>,
) -> Result<Json<Value>, StatusCode> {
    if query.limit == 0 || query.limit > 128 {
        return Err(StatusCode::BAD_REQUEST);
    }
    let store = Arc::clone(&state.store);
    let index = tokio::task::spawn_blocking(move || store.load())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let selected: Vec<_> = index
        .blocks()
        .iter()
        .filter(|block| query.after.is_none_or(|after| block.number > after))
        .take(query.limit)
        .cloned()
        .collect();
    if selected
        .iter()
        .map(|block| block.events.len())
        .sum::<usize>()
        > 4_096
    {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let response = json!({
        "deployment_block": index.first_block(),
        "through": index.tip().map(|block| block.number),
        "root": format!("0x{}", hex::encode(index.root())),
        "blocks": selected,
    });
    if serde_json::to_vec(&response)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .len()
        > 1024 * 1024
    {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    Ok(Json(response))
}

#[cfg(test)]
mod tests {
    use super::*;
    use erebus_shielded_prover::indexer::{PoolBlock, PoolIndex};

    #[tokio::test]
    async fn omitted_cursor_includes_deployment_block_zero() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let domain = IndexDomain {
            chain_id: 1,
            pool: [3; 20],
            first_block: 0,
            first_hash: [4; 32],
        };
        let store =
            Arc::new(IndexStore::new(dir.path().join("index.json"), domain).expect("store"));
        let mut index = PoolIndex::new(0).expect("index");
        index
            .apply_block(PoolBlock {
                number: 0,
                hash: domain.first_hash,
                parent_hash: [5; 32],
                events: Vec::new(),
            })
            .expect("block zero");
        store.save_if_unchanged(&index, None).expect("save");
        let state = AppState {
            store,
            status: Arc::new(RwLock::new(SyncStatus::default())),
            token: None,
        };
        let initial = blocks(
            State(state.clone()),
            Query(BlockQuery {
                after: None,
                limit: 64,
            }),
        )
        .await
        .expect("initial page");
        assert_eq!(initial.0["blocks"].as_array().expect("blocks").len(), 1);
        let next = blocks(
            State(state),
            Query(BlockQuery {
                after: Some(0),
                limit: 64,
            }),
        )
        .await
        .expect("next page");
        assert!(next.0["blocks"].as_array().expect("blocks").is_empty());
    }
}
