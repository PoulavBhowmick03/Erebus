//! Read/prove-only mainnet screening probe.
//!
//! This ignored test builds a real one-STRK shield for an already registered identity and
//! asks the configured prover to prove it. It never estimates or submits `apply_actions`.
//! Its purpose is to verify whether the prover returns the screening signature that the
//! canonical mainnet pool requires for a deposit.

use std::path::{Path, PathBuf};

use erebus_sdk::actions::{FeltEntropy, RandomSalt};
use erebus_sdk::calldata;
use erebus_sdk::channel::{Channel, Counterparty, PoolIdentity, SetupParams};
use erebus_sdk::execution::build_proof_invocation;
use erebus_sdk::hashes;
use erebus_sdk::prover::{BlockId, ProvingService};
use erebus_sdk::rpc::StarknetRpc;
use rand::{rngs::OsRng, RngCore};
use starknet_types_core::felt::Felt;

const SN_MAIN: &str = "0x534e5f4d41494e";
const MAINNET_POOL: &str = "0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a";
const STRK: &str = "0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";
const MAX_CHANNELS: u32 = 4096;

fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is required"))
}

fn felt(name: &str, value: &str) -> Felt {
    Felt::from_hex(value).unwrap_or_else(|error| panic!("{name} is not a hex felt: {error}"))
}

fn read_key(name: &str, path: &Path) -> Felt {
    let value = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("could not read {name} at {}: {error}", path.display()));
    felt(name, value.trim())
}

fn entropy() -> FeltEntropy {
    loop {
        let mut bytes = [0u8; 31];
        OsRng.fill_bytes(&mut bytes);
        if let Ok(value) = FeltEntropy::new(Felt::from_bytes_be_slice(&bytes)) {
            return value;
        }
    }
}

fn random_salt() -> RandomSalt {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    RandomSalt::from_entropy(bytes)
}

fn server_actions(proof: &erebus_sdk::prover::ProveTransactionResult, pool: Felt) -> Vec<Felt> {
    let mut messages = proof
        .l2_to_l1_messages
        .iter()
        .filter(|message| Felt::from_hex(&message.from_address).ok() == Some(pool));
    let message = messages.next().expect("proof emitted no pool message");
    assert!(
        messages.next().is_none(),
        "proof emitted multiple pool messages"
    );
    let payload = message
        .payload
        .iter()
        .map(|value| felt("L2 to L1 payload", value))
        .collect::<Vec<_>>();
    assert!(!payload.is_empty(), "pool message payload was empty");
    payload[1..].to_vec()
}

async fn outgoing_channel_count(
    rpc: &StarknetRpc,
    pool: Felt,
    account: Felt,
    pool_key: Felt,
    block: &BlockId,
) -> u32 {
    for index in 0..MAX_CHANNELS {
        let id = hashes::compute_outgoing_channel_id(account, pool_key, u64::from(index));
        let stored = rpc
            .call_contract(pool, "get_outgoing_channel_info", &[id], block)
            .await
            .expect("outgoing channel read");
        assert_eq!(stored.len(), 2, "unexpected outgoing channel response");
        if stored[0] == Felt::ZERO {
            return index;
        }
    }
    panic!("outgoing channel discovery exceeded {MAX_CHANNELS}");
}

#[tokio::test]
#[ignore = "proves a mainnet deposit but never estimates or submits it"]
async fn canonical_pool_deposit_returns_screening_signature() {
    let prover_url = required_env("PROVING_SERVICE_URL");
    let rpc_url = required_env("EREBUS_MAINNET_RPC_URL");
    let account = felt("account address", &required_env("EREBUS_ACCOUNT_ADDRESS"));
    let account_key = read_key(
        "account key",
        &PathBuf::from(required_env("EREBUS_ACCOUNT_KEY_FILE")),
    );
    let pool_key = read_key(
        "pool key",
        &PathBuf::from(required_env("EREBUS_POOL_KEY_FILE")),
    );
    let chain_id = felt("SN_MAIN", SN_MAIN);
    let pool = felt("mainnet pool", MAINNET_POOL);
    let token = felt("STRK", STRK);
    let rpc = StarknetRpc::new(rpc_url).expect("RPC client builds");

    assert_eq!(rpc.chain_id().await.expect("chain id"), chain_id);
    let identity = PoolIdentity::new(account, pool_key);
    assert_eq!(
        rpc.call_contract(pool, "get_public_key", &[account], &BlockId::Latest)
            .await
            .expect("registration read"),
        vec![identity.public_key()],
        "probe identity is not registered with this pool key"
    );

    let head = rpc.block_number().await.expect("block number");
    let proving_number = head.saturating_sub(10).max(1);
    let proving_block = BlockId::Number(proving_number);
    let channel_index =
        outgoing_channel_count(&rpc, pool, identity.address(), pool_key, &proving_block).await;
    let channel = Channel::derive(
        chain_id,
        pool,
        &identity,
        Counterparty {
            address: identity.address(),
            public_key: identity.public_key(),
        },
    );
    let actions = channel
        .shield(
            &identity,
            SetupParams {
                register: None,
                channel_index,
                channel_random: entropy(),
                channel_salt: entropy(),
                subchannel_index: 0,
                token,
                subchannel_salt: entropy(),
            },
            1_000_000_000_000_000_000,
            random_salt(),
        )
        .expect("shield action set builds");
    let compile_calldata = calldata::compile_actions(account, pool_key, &actions);
    let simulated = rpc
        .call_contract(pool, "compile_actions", &compile_calldata, &proving_block)
        .await
        .expect("shield action simulation");
    let nonce = rpc.nonce(pool, &proving_block).await.expect("pool nonce");
    let invocation = build_proof_invocation(
        pool,
        chain_id,
        account,
        pool_key,
        account_key,
        nonce,
        &actions,
    )
    .expect("proof invocation builds");
    let proof = ProvingService::new(prover_url)
        .expect("prover client builds")
        .with_max_retries(0)
        .prove_transaction(&proving_block, &invocation)
        .await
        .expect("shield proves");
    assert_eq!(
        simulated,
        server_actions(&proof, pool),
        "simulation and proof disagree"
    );

    let signature = proof
        .additional_data
        .as_ref()
        .and_then(|data| data.signature.as_ref());
    assert!(
        signature.is_some(),
        "configured prover returned no screening signature; canonical-pool shielding cannot be submitted"
    );
}
