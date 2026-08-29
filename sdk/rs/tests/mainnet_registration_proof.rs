//! Proof-only mainnet registration canary.
//!
//! This ignored test builds and proves one `SetViewingKey` action for an unregistered STRK20
//! identity. It never constructs or submits `apply_actions`, so a successful run cannot
//! change mainnet state. Key values are read from protected files and never printed.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use erebus_sdk::action_set::ActionSetBuilder;
use erebus_sdk::actions::{ClientAction, SetViewingKeyInput};
use erebus_sdk::calldata;
use erebus_sdk::execution::build_proof_invocation;
use erebus_sdk::prover::{BlockId, ProvingService};
use erebus_sdk::rpc::StarknetRpc;
use erebus_sdk::signing;
use erebus_sdk::tx::{DataAvailabilityMode, InvokeV3, ResourceBound, ResourceBounds};
use rand::{rngs::OsRng, RngCore};
use starknet_crypto::Signature;
use starknet_types_core::felt::Felt;

const SN_MAIN: &str = "0x534e5f4d41494e";
const MAINNET_POOL: &str = "0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a";

fn required_env(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(value) if !value.is_empty() => Some(value),
        _ => {
            eprintln!("{name} unset, skipping");
            None
        }
    }
}

fn felt(name: &str, value: &str) -> Felt {
    Felt::from_hex(value).unwrap_or_else(|error| panic!("{name} is not a hex felt: {error}"))
}

fn read_key(name: &str, path: &Path) -> Felt {
    let value = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("could not read {name} at {}: {error}", path.display()));
    Felt::from_hex(value.trim())
        .unwrap_or_else(|error| panic!("{name} at {} is not a hex felt: {error}", path.display()))
}

fn fresh_entropy() -> Felt {
    loop {
        let mut bytes = [0u8; 31];
        OsRng.fill_bytes(&mut bytes);
        let value = Felt::from_bytes_be_slice(&bytes);
        if value != Felt::ZERO {
            return value;
        }
    }
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

fn write_private(path: &Path, value: &[u8]) {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .unwrap_or_else(|error| panic!("could not create {}: {error}", path.display()));
    file.write_all(value)
        .unwrap_or_else(|error| panic!("could not write {}: {error}", path.display()));
    file.sync_all()
        .unwrap_or_else(|error| panic!("could not sync {}: {error}", path.display()));
}

fn read_felt_list(path: &Path, separator: char) -> Vec<Felt> {
    let contents = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()));
    contents
        .split(separator)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| felt("prepared felt", value))
        .collect()
}

fn bound_fee(bound: ResourceBound) -> u128 {
    u128::from(bound.max_amount)
        .checked_mul(bound.max_price_per_unit)
        .expect("resource fee fits u128")
}

#[tokio::test]
#[ignore = "proves against the live mainnet pool but never submits apply_actions"]
async fn prove_fresh_registration_without_submission() {
    let Some(prover_url) = required_env("PROVING_SERVICE_URL") else {
        return;
    };
    let Some(rpc_url) = required_env("EREBUS_MAINNET_RPC_URL") else {
        return;
    };
    let Some(account_address) = required_env("EREBUS_ACCOUNT_ADDRESS") else {
        return;
    };
    let Some(account_key_file) = required_env("EREBUS_ACCOUNT_KEY_FILE") else {
        return;
    };
    let Some(pool_key_file) = required_env("EREBUS_POOL_KEY_FILE") else {
        return;
    };

    let chain_id = felt("SN_MAIN", SN_MAIN);
    let pool = felt("mainnet pool", MAINNET_POOL);
    let account = felt("account address", &account_address);
    let account_key = read_key("account key", &PathBuf::from(account_key_file));
    let pool_key = read_key("pool key", &PathBuf::from(pool_key_file));

    let rpc = StarknetRpc::new(rpc_url).expect("RPC client builds");
    assert_eq!(rpc.chain_id().await.expect("chain id"), chain_id);
    let registered = rpc
        .call_contract(pool, "get_public_key", &[account], &BlockId::Latest)
        .await
        .expect("registration read");
    assert_eq!(
        registered,
        vec![Felt::ZERO],
        "account is already registered"
    );

    let actions = ActionSetBuilder::new()
        .with(ClientAction::SetViewingKey(SetViewingKeyInput {
            random: fresh_entropy(),
        }))
        .expect("registration action is valid")
        .build()
        .expect("registration action set is replay-protected");
    let head = rpc.block_number().await.expect("block number");
    let proving_number = head.saturating_sub(10).max(1);
    let proving_block = BlockId::Number(proving_number);
    let compile_calldata = calldata::compile_actions(account, pool_key, &actions);
    let simulated = rpc
        .call_contract(pool, "compile_actions", &compile_calldata, &proving_block)
        .await
        .expect("action simulation");
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
        .expect("registration proves");

    assert!(!proof.proof.is_empty(), "prover returned an empty proof");
    assert!(
        !proof.proof_facts.is_empty(),
        "prover returned no proof facts"
    );
    let server_actions = server_actions(&proof, pool);
    assert_eq!(simulated, server_actions, "simulation and proof disagree");

    if let Some(directory) = std::env::var_os("EREBUS_PREPARED_DIR") {
        let directory = PathBuf::from(directory);
        assert!(directory.is_dir(), "prepared directory does not exist");
        let apply_calldata =
            calldata::apply_actions(&server_actions, proof.additional_data.as_ref())
                .expect("apply_actions calldata");
        let proof_facts = proof.proof_facts.join(",");
        let calldata = apply_calldata
            .iter()
            .map(|value| format!("{value:#x}\n"))
            .collect::<String>();
        let metadata = serde_json::to_vec_pretty(&serde_json::json!({
            "account": format!("{account:#x}"),
            "pool": format!("{pool:#x}"),
            "proving_block": proving_number,
            "server_actions": server_actions.len(),
            "proof_facts": proof.proof_facts.len(),
        }))
        .expect("metadata serializes");

        write_private(
            &directory.join("registration.proof"),
            proof.proof.as_bytes(),
        );
        write_private(
            &directory.join("registration.proof-facts"),
            proof_facts.as_bytes(),
        );
        write_private(
            &directory.join("registration.calldata"),
            calldata.as_bytes(),
        );
        write_private(&directory.join("registration.json"), &metadata);
    }
    println!(
        "registration prepared without submission: proving_block={} proof_facts={} messages={}",
        proving_number,
        proof.proof_facts.len(),
        proof.l2_to_l1_messages.len()
    );
}

#[tokio::test]
#[ignore = "estimates a prepared proof-carrying registration but never submits it"]
async fn estimate_prepared_registration_without_submission() {
    let Some(rpc_url) = required_env("EREBUS_MAINNET_RPC_URL") else {
        return;
    };
    let Some(account_address) = required_env("EREBUS_ACCOUNT_ADDRESS") else {
        return;
    };
    let Some(directory) = std::env::var_os("EREBUS_PREPARED_DIR") else {
        eprintln!("EREBUS_PREPARED_DIR unset, skipping");
        return;
    };
    let directory = PathBuf::from(directory);
    let account = felt("account address", &account_address);
    let pool = felt("mainnet pool", MAINNET_POOL);
    let proof = std::fs::read_to_string(directory.join("registration.proof"))
        .expect("prepared proof is readable");
    let proof_facts = read_felt_list(&directory.join("registration.proof-facts"), ',');
    let apply_calldata = read_felt_list(&directory.join("registration.calldata"), '\n');

    let rpc = StarknetRpc::new(rpc_url).expect("RPC client builds");
    let nonce = rpc
        .nonce(account, &BlockId::Latest)
        .await
        .expect("account nonce");
    let estimate = InvokeV3 {
        sender_address: account,
        calldata: calldata::single_call(pool, "apply_actions", &apply_calldata),
        chain_id: felt("SN_MAIN", SN_MAIN),
        nonce,
        account_deployment_data: Vec::new(),
        nonce_da_mode: DataAvailabilityMode::L1,
        fee_da_mode: DataAvailabilityMode::L1,
        resource_bounds: ResourceBounds::default(),
        tip: 0,
        paymaster_data: Vec::new(),
        proof_facts,
    }
    .with_signature(Signature {
        r: Felt::ZERO,
        s: Felt::ZERO,
    })
    .with_proof(proof.trim().to_owned());
    let bounds = rpc
        .estimate_bounds(&estimate, &BlockId::Latest)
        .await
        .expect("proof-aware fee estimate");
    let max_fee = bound_fee(bounds.l1_gas)
        .checked_add(bound_fee(bounds.l2_gas))
        .and_then(|fee| fee.checked_add(bound_fee(bounds.l1_data_gas)))
        .expect("total fee fits u128");

    println!(
        "registration estimate succeeded without submission: max_fee_fri={max_fee} bounds={bounds:?}"
    );
}

#[tokio::test]
#[ignore = "submits one explicitly approved mainnet registration"]
async fn submit_prepared_registration_once() {
    let rpc_url = required_env("EREBUS_MAINNET_RPC_URL").expect("RPC URL is required");
    let account_address =
        required_env("EREBUS_ACCOUNT_ADDRESS").expect("account address is required");
    assert_eq!(
        required_env("EREBUS_MAINNET_SUBMIT").as_deref(),
        Some(account_address.as_str()),
        "set EREBUS_MAINNET_SUBMIT to the exact configured account address to authorize this write"
    );
    let account_key_file =
        required_env("EREBUS_ACCOUNT_KEY_FILE").expect("account key file is required");
    let pool_key_file = required_env("EREBUS_POOL_KEY_FILE").expect("pool key file is required");
    let directory = PathBuf::from(
        std::env::var_os("EREBUS_PREPARED_DIR").expect("prepared directory is required"),
    );

    let account = felt("account address", &account_address);
    let account_key = read_key("account key", &PathBuf::from(account_key_file));
    let pool_key = read_key("pool key", &PathBuf::from(pool_key_file));
    let pool = felt("mainnet pool", MAINNET_POOL);
    let proof = std::fs::read_to_string(directory.join("registration.proof"))
        .expect("prepared proof is readable");
    let proof_facts = read_felt_list(&directory.join("registration.proof-facts"), ',');
    let apply_calldata = read_felt_list(&directory.join("registration.calldata"), '\n');

    let rpc = StarknetRpc::new(rpc_url).expect("RPC client builds");
    assert_eq!(
        rpc.call_contract(pool, "get_public_key", &[account], &BlockId::Latest)
            .await
            .expect("registration read"),
        vec![Felt::ZERO],
        "account is already registered; refusing a second submission"
    );
    let nonce = rpc
        .nonce(account, &BlockId::Latest)
        .await
        .expect("account nonce");
    let account_calldata = calldata::single_call(pool, "apply_actions", &apply_calldata);
    let base = InvokeV3 {
        sender_address: account,
        calldata: account_calldata.clone(),
        chain_id: felt("SN_MAIN", SN_MAIN),
        nonce,
        account_deployment_data: Vec::new(),
        nonce_da_mode: DataAvailabilityMode::L1,
        fee_da_mode: DataAvailabilityMode::L1,
        resource_bounds: ResourceBounds::default(),
        tip: 0,
        paymaster_data: Vec::new(),
        proof_facts: proof_facts.clone(),
    };
    let bounds = rpc
        .estimate_bounds(
            &base
                .with_signature(Signature {
                    r: Felt::ZERO,
                    s: Felt::ZERO,
                })
                .with_proof(proof.trim().to_owned()),
            &BlockId::Latest,
        )
        .await
        .expect("proof-aware fee estimate");
    let invoke = InvokeV3 {
        sender_address: account,
        calldata: account_calldata,
        chain_id: felt("SN_MAIN", SN_MAIN),
        nonce,
        account_deployment_data: Vec::new(),
        nonce_da_mode: DataAvailabilityMode::L1,
        fee_da_mode: DataAvailabilityMode::L1,
        resource_bounds: bounds,
        tip: 0,
        paymaster_data: Vec::new(),
        proof_facts,
    };
    let expected_hash = invoke.transaction_hash();
    let signature = signing::sign(&account_key, &expected_hash).expect("account signing succeeds");
    let transaction = invoke
        .with_signature(signature)
        .with_proof(proof.trim().to_owned());
    let persisted = serde_json::to_vec_pretty(&serde_json::json!({
        "expected_transaction_hash": format!("{expected_hash:#x}"),
        "transaction": transaction.to_wire(),
    }))
    .expect("submission payload serializes");
    write_private(&directory.join("registration.submission.json"), &persisted);

    let submitted_hash = rpc
        .add_invoke_transaction(&transaction)
        .await
        .expect("mainnet registration submission");
    assert_eq!(
        submitted_hash, expected_hash,
        "RPC returned a different hash"
    );

    let started = Instant::now();
    let receipt = loop {
        match rpc.transaction_receipt(submitted_hash).await {
            Ok(receipt) if receipt.is_accepted() => break receipt,
            Ok(receipt) if receipt.is_reverted() => {
                panic!("registration reverted: {:?}", receipt.revert_reason)
            }
            Ok(_) => {}
            Err(error) if error.is_transaction_not_found() => {}
            Err(error) => panic!("receipt read failed: {error}"),
        }
        assert!(
            started.elapsed() < Duration::from_secs(300),
            "receipt timeout; inspect registration.submission.json before retrying"
        );
        tokio::time::sleep(Duration::from_secs(3)).await;
    };
    write_private(
        &directory.join("registration.receipt.json"),
        &serde_json::to_vec_pretty(&receipt).expect("receipt serializes"),
    );
    assert_eq!(
        rpc.call_contract(pool, "get_public_key", &[account], &BlockId::Latest)
            .await
            .expect("registered key read"),
        vec![signing::public_key(&pool_key)],
        "registered pool public key does not match the protected key file"
    );
    println!("mainnet registration accepted: {submitted_hash:#x}");
}
