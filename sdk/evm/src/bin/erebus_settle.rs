//! Buyer-side public-bound settlement onboarding: one bounded JSON request on stdin.
//!
//! `funding` is read-only. It reports, before a buyer authorizes anything, the buyer's token
//! allowance and balance against the signed amount plus fee, and the gas payer's native
//! shortfall. It never signs, submits, or prints the endpoint, terms, or signatures.
//!
//! Request:
//! ```json
//! {"method":"funding",
//!  "deployment":{"namespace":"eip155:10143","settlement_contract":"0x...",
//!                "verifier_version":1,"rpc_url":"https://..."},
//!  "signer_address":"0x...",
//!  "evidence":"<hex SettlementEvidence>"}
//! ```

use std::io::{Read, Write};

use erebus_core::auth::{Authorization, Role};
use erebus_core::commitment::{commit_agreement, CommitmentBlinding};
use erebus_core::ids::{ChainNamespace, SignatureBytes};
use erebus_core::terms::AgreementTerms;
use erebus_evm::backend::EvmSettlementBackend;
use erebus_evm::deployment::{parse_lowercase_address, EvmDeployment};
use erebus_evm::evidence::SettlementEvidence;
use serde::Deserialize;
use serde_json::{json, Value};

const MAX_REQUEST_BYTES: usize = 64 * 1024;

#[derive(Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Funding {
        deployment: Deployment,
        signer_address: String,
        evidence: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Deployment {
    namespace: String,
    settlement_contract: String,
    verifier_version: u32,
    rpc_url: String,
}

const HELP: &str = "erebus-settle: buyer-side public-bound onboarding as JSON on stdin.
Methods: funding.
funding is read-only: it reports the buyer's token allowance and balance against the signed
amount plus fee, and the gas payer's native shortfall, before any authorization is signed.
Output excludes the RPC URL, terms, signatures, and keys.
See docs/metropolis-m8-runbook.md.";

#[tokio::main]
async fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if matches!(arguments.as_slice(), [argument] if argument == "--help") {
        println!("{HELP}");
        return;
    }
    if !arguments.is_empty() {
        finish(Err("invalid arguments"));
    }
    let mut input = Vec::new();
    if std::io::stdin()
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut input)
        .is_err()
    {
        finish(Err("cannot read request"));
    }
    if input.len() > MAX_REQUEST_BYTES {
        finish(Err("request exceeds size limit"));
    }
    let request = match serde_json::from_slice(&input) {
        Ok(request) => request,
        Err(_) => finish(Err("invalid request")),
    };
    finish(handle(request).await);
}

fn finish(result: Result<Value, &'static str>) -> ! {
    let failed = result.is_err();
    let response = result.unwrap_or_else(|error| json!({"status": "error", "error": error}));
    println!("{response}");
    let _ = std::io::stdout().flush();
    std::process::exit(i32::from(failed));
}

async fn handle(request: Request) -> Result<Value, &'static str> {
    match request {
        Request::Funding {
            deployment,
            signer_address,
            evidence,
        } => {
            let namespace = ChainNamespace::parse(&deployment.namespace)
                .map_err(|_| "invalid deployment namespace")?;
            let contract = parse_lowercase_address(&deployment.settlement_contract)
                .ok_or("invalid deployment contract")?;
            let signer =
                parse_lowercase_address(&signer_address).ok_or("invalid signer address")?;
            let deployment = EvmDeployment::new(
                namespace,
                contract,
                deployment.verifier_version,
                deployment.rpc_url,
            )
            .map_err(|_| "invalid deployment")?;

            let bytes = hex::decode(&evidence).map_err(|_| "invalid evidence")?;
            let evidence = SettlementEvidence::decode(&bytes).map_err(|_| "invalid evidence")?;
            let terms =
                AgreementTerms::decode(&evidence.terms).map_err(|_| "invalid evidence terms")?;
            let blinding = CommitmentBlinding::from_bytes(evidence.blinding);
            let commitment =
                commit_agreement(&terms, &blinding).map_err(|_| "invalid evidence opening")?;
            let authorization = |role: Role, signature: [u8; 65]| Authorization {
                role,
                suite_id: terms.suite_id,
                commitment,
                signature: SignatureBytes::new(signature.to_vec())
                    .expect("a 65-byte signature is in range"),
            };
            let buyer = authorization(Role::Buyer, evidence.buyer_signature);
            let seller = authorization(Role::Seller, evidence.seller_signature);

            let backend = EvmSettlementBackend::read_only(deployment, signer)
                .map_err(|_| "invalid deployment")?;
            let estimate = backend
                .estimate(&terms, &blinding, &buyer, &seller, [1; 32])
                .await
                .map_err(|_| "funding check failed")?;
            let prepared = backend
                .prepare(&terms, &blinding, &buyer, &seller, [1; 32])
                .map_err(|_| "evidence does not match the deployment")?;
            let gas = backend
                .funding_diagnostics(&prepared)
                .await
                .map_err(|_| "funding check failed")?;

            Ok(json!({
                "status": "ok",
                "required_allowance": estimate.required_allowance.to_string(),
                "allowance": estimate.allowance.to_string(),
                "allowance_shortfall": estimate.allowance_shortfall().to_string(),
                "buyer_balance": estimate.buyer_balance.to_string(),
                "balance_shortfall": estimate.balance_shortfall().to_string(),
                "gas": estimate.gas,
                "signer_required": gas.required.to_string(),
                "signer_balance": gas.balance.to_string(),
                "signer_shortfall": gas.shortfall().to_string(),
                "funded": estimate.allowance_is_sufficient()
                    && estimate.balance_is_sufficient()
                    && gas.is_funded(),
            }))
        }
    }
}
