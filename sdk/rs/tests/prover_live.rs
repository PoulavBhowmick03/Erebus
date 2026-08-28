//! Live probe against a proving service.
//!
//! **`#[ignore]` on purpose.** These hit a real prover. StarkWare's shared dev endpoint was
//! shared with us privately and asked to be used sparingly. They are not part of
//! `cargo test`, do not belong in CI, and should be run intentionally:
//!
//! ```sh
//! PROVING_SERVICE_URL=... cargo test --test prover_live -- --ignored --nocapture
//! ```
//!
//! The URL lives in the repo's gitignored `.env` and must never be committed.
//!
//! Network defaults to Sepolia. Point the probe at another network by overriding both
//! values together; a chain id that disagrees with the pool address produces a signature
//! rejection rather than the state error the probe is looking for:
//!
//! ```sh
//! PROVING_SERVICE_URL=http://127.0.0.1:3000 \
//! EREBUS_PROBE_CHAIN_ID=0x534e5f4d41494e \
//! EREBUS_PROBE_POOL=0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a \
//!   cargo test --test prover_live -- --ignored --nocapture
//! ```
//!
//! What these are for is the error *shape*. A proof needs a registered identity and funded
//! notes, neither of which exists yet. These tests check whether the service reaches state
//! validation instead of rejecting the invocation encoding. Nothing here is broadcast:
//! `starknet_proveTransaction` is a read-only call to the prover.

use erebus_sdk::prover::{BlockId, ProvingService};
use erebus_sdk::signing::sign;
use erebus_sdk::tx::{DataAvailabilityMode, InvokeV3, ResourceBounds};
use starknet_types_core::felt::Felt;

/// Privacy pool v2.0 on Sepolia, the probe default.
const SEPOLIA_POOL: &str = "0x254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91";
/// Short-string felt for "SN_SEPOLIA".
const SEPOLIA_CHAIN_ID: &str = "0x534e5f5345504f4c4941";

fn endpoint() -> Option<String> {
    std::env::var("PROVING_SERVICE_URL")
        .ok()
        .filter(|s| !s.is_empty())
}

fn env_felt(key: &str, default: &str) -> Felt {
    let raw = std::env::var(key)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default.to_owned());
    Felt::from_hex(&raw).unwrap_or_else(|e| panic!("{key}={raw} is not a hex felt: {e}"))
}

/// The pool the probe addresses. Override with `EREBUS_PROBE_POOL`.
fn pool_address() -> Felt {
    env_felt("EREBUS_PROBE_POOL", SEPOLIA_POOL)
}

/// The chain the probe signs for. Override with `EREBUS_PROBE_CHAIN_ID`.
fn chain_id() -> Felt {
    env_felt("EREBUS_PROBE_CHAIN_ID", SEPOLIA_CHAIN_ID)
}

#[tokio::test]
#[ignore = "hits StarkWare's shared dev prover; run deliberately"]
async fn spec_version_is_reachable() {
    let Some(url) = endpoint() else {
        eprintln!("PROVING_SERVICE_URL unset, skipping");
        return;
    };
    let service = ProvingService::new(url).expect("client builds");
    let version = service.spec_version().await.expect("spec version");
    println!("proving service spec version: {version}");
    assert!(
        version.starts_with("0."),
        "unexpected spec version: {version}"
    );
}

/// One `starknet_proveTransaction` call, to learn how it answers a well-formed invocation
/// for an identity that does not exist. Deliberately a single request with retries off.
#[tokio::test]
#[ignore = "hits StarkWare's shared dev prover; run deliberately"]
async fn prove_transaction_error_shape() {
    let Some(url) = endpoint() else {
        eprintln!("PROVING_SERVICE_URL unset, skipping");
        return;
    };

    let pool = pool_address();
    let chain_id = chain_id();
    println!("probing pool {pool:#x} on chain {chain_id:#x}");
    let signing_key =
        Felt::from_hex("0x1111111111111111111111111111111111111111111111111111111111")
            .expect("throwaway key");

    // compile_actions(user_addr, user_private_key, []) wrapped in __execute__.
    let calldata = vec![
        Felt::ONE,
        pool,
        Felt::from_hex("0x360f8727b971d0bc6b93fc840d637c077f8ae59eb6ca8ce27fdb5422b688192")
            .expect("compile_actions selector"),
        Felt::THREE,
        Felt::from_hex("0xdeadbeef").expect("throwaway addr"),
        Felt::from_hex("0xcafebabe").expect("throwaway pool key"),
        Felt::ZERO,
    ];

    let invoke = InvokeV3 {
        sender_address: pool,
        calldata,
        chain_id,
        nonce: Felt::ZERO,
        account_deployment_data: vec![],
        nonce_da_mode: DataAvailabilityMode::L1,
        fee_da_mode: DataAvailabilityMode::L1,
        resource_bounds: ResourceBounds::for_proof_invocation(),
        tip: 0,
        paymaster_data: vec![],
        proof_facts: vec![],
    };

    let signature = sign(&signing_key, &invoke.transaction_hash()).expect("signing");
    let signed = invoke.with_signature(signature);

    // Retries off: one request, whatever the answer.
    let service = ProvingService::new(url)
        .expect("client builds")
        .with_max_retries(0);
    match service.prove_transaction(&BlockId::Latest, &signed).await {
        Ok(result) => {
            println!(
                "proved unexpectedly: {} proof facts",
                result.proof_facts.len()
            );
        }
        Err(error) => {
            println!("prove_transaction error: {error}");
            println!("screening rejection: {}", error.is_screening_rejection());
        }
    }
}
