//! Local disclosure command with independent shielded and public-bound payment verification.

use std::path::PathBuf;

use erebus_core::{ids::ChainNamespace, terms::SettlementMode};
use erebus_evm::{
    deployment::{parse_lowercase_address, EvmDeployment},
    disclosure::cli::{self, PaymentRequest},
};
use erebus_shielded_prover::{
    disclosure::verify_shielded_payment,
    index_store::{IndexDomain, IndexStore},
    rpc::PoolRpc,
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Deployment {
    namespace: String,
    settlement_contract: String,
    verifier_version: u32,
    rpc_url: String,
    peer_rpc_url: String,
    first_block: u64,
    first_hash: String,
    cache_root: PathBuf,
}

#[tokio::main]
async fn main() {
    cli::run(|request| async move {
        if request.evidence.terms.settlement_mode == SettlementMode::PublicBound {
            return cli::verify_public_payment(request).await;
        }
        verify(request).await
    })
    .await;
}

async fn verify(
    request: PaymentRequest,
) -> Result<erebus_transport::disclosure::VerifiedAgreement, &'static str> {
    let deployment: Deployment =
        serde_json::from_value(request.deployment).map_err(|_| "invalid shielded deployment")?;
    let namespace =
        ChainNamespace::parse(&deployment.namespace).map_err(|_| "invalid deployment")?;
    let pool =
        parse_lowercase_address(&deployment.settlement_contract).ok_or("invalid deployment")?;
    let configured = EvmDeployment::new(
        namespace,
        pool,
        deployment.verifier_version,
        &deployment.rpc_url,
    )
    .map_err(|_| "invalid deployment")?;
    configured
        .matches_domain(&request.evidence.terms.domain)
        .map_err(|_| "deployment does not match agreement")?;
    let digits = deployment
        .first_hash
        .strip_prefix("0x")
        .ok_or("invalid deployment anchor")?;
    if digits.len() != 64
        || !digits
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("invalid deployment anchor");
    }
    let first_hash = hex::decode(digits)
        .map_err(|_| "invalid deployment anchor")?
        .try_into()
        .map_err(|_| "invalid deployment anchor")?;
    let domain = IndexDomain {
        chain_id: configured.chain_id,
        pool,
        first_block: deployment.first_block,
        first_hash,
    };
    let rpc = PoolRpc::new(&deployment.rpc_url, configured.chain_id, pool)
        .map_err(|_| "invalid pool RPC configuration")?;
    let peer_rpc = PoolRpc::new(&deployment.peer_rpc_url, configured.chain_id, pool)
        .map_err(|_| "invalid peer pool RPC configuration")?;
    let index = IndexStore::new(deployment.cache_root.join("primary/index.json"), domain)
        .map_err(|_| "public cache unavailable")?;
    let peer_index = IndexStore::new(deployment.cache_root.join("peer/index.json"), domain)
        .map_err(|_| "peer public cache unavailable")?;
    let result = verify_shielded_payment(request.evidence, &rpc, &index, &peer_rpc, &peer_index)
        .await
        .map_err(|_| "payment not independently verified; retain caches and retry")?;
    Ok(result.agreement)
}
