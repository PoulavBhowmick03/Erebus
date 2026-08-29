//! One-time Account A signer rotation after an operational key exposure.
//!
//! This ignored canary requires an explicit environment guard. It signs the ownership
//! acceptance with the new key, signs the account transaction with the current key, persists
//! the exact transaction before submission, and verifies the onchain public key afterward.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use erebus_sdk::calldata;
use erebus_sdk::prover::BlockId;
use erebus_sdk::rpc::StarknetRpc;
use erebus_sdk::signing;
use erebus_sdk::tx::{DataAvailabilityMode, InvokeV3, ResourceBounds};
use starknet_crypto::{poseidon_hash_many, Signature};
use starknet_types_core::felt::Felt;

const SN_MAIN: &str = "0x534e5f4d41494e";

fn required_env(name: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| panic!("{name} is required"))
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

#[tokio::test]
#[ignore = "rotates the explicitly approved Account A signer on mainnet"]
async fn rotate_agent_a_signer_once() {
    assert_eq!(
        required_env("EREBUS_MAINNET_ROTATE"),
        "ROTATE_AGENT_A",
        "set EREBUS_MAINNET_ROTATE=ROTATE_AGENT_A to authorize this write"
    );
    let rpc = StarknetRpc::new(required_env("EREBUS_MAINNET_RPC_URL")).expect("RPC client builds");
    let account = felt("account address", &required_env("EREBUS_ACCOUNT_ADDRESS"));
    let current_key = read_key(
        "current account key",
        &PathBuf::from(required_env("EREBUS_CURRENT_ACCOUNT_KEY_FILE")),
    );
    let new_key = read_key(
        "new account key",
        &PathBuf::from(required_env("EREBUS_NEW_ACCOUNT_KEY_FILE")),
    );
    let directory = PathBuf::from(required_env("EREBUS_ROTATION_DIR"));
    assert!(directory.is_dir(), "rotation directory does not exist");

    let current_public_key = signing::public_key(&current_key);
    let new_public_key = signing::public_key(&new_key);
    assert_ne!(
        current_public_key, new_public_key,
        "new key equals current key"
    );
    assert_eq!(
        rpc.call_contract(account, "get_public_key", &[], &BlockId::Latest)
            .await
            .expect("current public key read"),
        vec![current_public_key],
        "protected current key does not control the deployed account"
    );

    // OpenZeppelin AccountComponent requires the incoming owner to accept ownership.
    // It verifies Poseidon('StarkNet Message', 'accept_ownership', account, current_owner)
    // under the new public key before changing the stored owner.
    let acceptance_hash = poseidon_hash_many(&[
        Felt::from_bytes_be_slice(b"StarkNet Message"),
        Felt::from_bytes_be_slice(b"accept_ownership"),
        account,
        current_public_key,
    ]);
    let acceptance =
        signing::sign(&new_key, &acceptance_hash).expect("new owner acceptance signature succeeds");
    let call_calldata = [new_public_key, Felt::TWO, acceptance.r, acceptance.s];
    let account_calldata = calldata::single_call(account, "set_public_key", &call_calldata);
    let nonce = rpc
        .nonce(account, &BlockId::Latest)
        .await
        .expect("account nonce");
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
        proof_facts: Vec::new(),
    };
    let bounds = rpc
        .estimate_bounds(
            &base.with_signature(Signature {
                r: Felt::ZERO,
                s: Felt::ZERO,
            }),
            &BlockId::Latest,
        )
        .await
        .expect("rotation fee estimate");
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
        proof_facts: Vec::new(),
    };
    let expected_hash = invoke.transaction_hash();
    let account_signature =
        signing::sign(&current_key, &expected_hash).expect("current account signing succeeds");
    let transaction = invoke.with_signature(account_signature);
    let persisted = serde_json::to_vec_pretty(&serde_json::json!({
        "expected_transaction_hash": format!("{expected_hash:#x}"),
        "new_public_key": format!("{new_public_key:#x}"),
        "transaction": transaction.to_wire(),
    }))
    .expect("rotation payload serializes");
    write_private(&directory.join("rotation.submission.json"), &persisted);

    let submitted_hash = rpc
        .add_invoke_transaction(&transaction)
        .await
        .expect("mainnet account rotation submission");
    assert_eq!(
        submitted_hash, expected_hash,
        "RPC returned a different hash"
    );

    let started = Instant::now();
    let receipt = loop {
        match rpc.transaction_receipt(submitted_hash).await {
            Ok(receipt) if receipt.is_accepted() => break receipt,
            Ok(receipt) if receipt.is_reverted() => {
                panic!("account rotation reverted: {:?}", receipt.revert_reason)
            }
            Ok(_) => {}
            Err(error) if error.is_transaction_not_found() => {}
            Err(error) => panic!("receipt read failed: {error}"),
        }
        assert!(
            started.elapsed() < Duration::from_secs(300),
            "receipt timeout; inspect rotation.submission.json before retrying"
        );
        tokio::time::sleep(Duration::from_secs(3)).await;
    };
    write_private(
        &directory.join("rotation.receipt.json"),
        &serde_json::to_vec_pretty(&receipt).expect("receipt serializes"),
    );
    assert_eq!(
        rpc.call_contract(account, "get_public_key", &[], &BlockId::Latest)
            .await
            .expect("rotated public key read"),
        vec![new_public_key],
        "deployed account did not retain the new public key"
    );
    println!("mainnet Account A signer rotated: {submitted_hash:#x}");
}
