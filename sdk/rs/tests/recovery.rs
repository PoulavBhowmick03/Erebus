//! End-to-end durable recovery at the Rust client boundary.
//!
//! The chain is a local JSON-RPC fixture. This test is about the crash contract: an expired
//! attempt retains its canonical request, opens a second attempt under the same operation id,
//! re-enters the ordinary method, and finishes with a replayable result.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::thread;

use erebus_sdk::client::{Client, ClientConfig};
use erebus_sdk::journal::{OperationJournal, OperationStage};
use erebus_sdk::operation::{OperationId, RequestBinding, WriteOperation};
use erebus_sdk::resume::ResumeOutcome;
use erebus_sdk::state::{StateStore, StoredChannel};
use erebus_sdk::wire::WireVersion;
use serde_json::{json, Value};
use starknet_types_core::felt::Felt;

fn server(responses: Vec<Value>) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    let handle = thread::spawn(move || {
        for response in responses {
            let (mut stream, _) = listener.accept().expect("accept");
            drain_request(&mut stream);
            let body = response.to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("response");
        }
    });
    (format!("http://{address}"), handle)
}

fn drain_request(stream: &mut TcpStream) {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let read = stream.read(&mut buffer).expect("request read");
        assert!(read > 0, "connection closed before headers");
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..end]);
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(str::trim)
                        .map(str::to_owned)
                })
                .expect("content-length")
                .parse()
                .expect("content length");
            let body_start = end + 4;
            while bytes.len() < body_start + length {
                let read = stream.read(&mut buffer).expect("body read");
                assert!(read > 0, "connection closed mid-body");
                bytes.extend_from_slice(&buffer[..read]);
            }
            return;
        }
    }
}

fn temporary_root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "erebus-rebuild-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ))
}

#[tokio::test]
async fn an_expired_proof_rebuilds_from_the_durable_request_under_the_same_id() {
    let root = temporary_root();
    std::fs::create_dir_all(&root).expect("root");
    let pool_key_file = root.join("pool.key");
    let account_key_file = root.join("account.key");
    std::fs::write(&pool_key_file, "0xabc\n").expect("pool key");
    std::fs::write(&account_key_file, "0xdef\n").expect("account key");

    let account = Felt::from(0x11u8);
    let pool = Felt::from(0x22u8);
    let chain = Felt::from(0x33u8);
    let token = Felt::from(0x44u8);
    let counterparty = Felt::from(0x55u8);
    let counterparty_public_key = Felt::from(0x66u8);

    let state = StateStore::new(&root).expect("state");
    let existing_handle = state
        .create(|handle| {
            StoredChannel::new_with_wire_version(
                handle,
                chain,
                pool,
                account,
                counterparty,
                counterparty_public_key,
                token,
                Felt::from(0x77u8),
                0,
                0,
                Felt::from(0x88u8),
                10,
                WireVersion::V3,
            )
        })
        .expect("existing channel");

    let operation_id = OperationId::parse(format!("op_{}", "9b".repeat(32))).expect("operation id");
    let binding = RequestBinding::builder(WriteOperation::OpenChannel, chain, pool, token)
        .felt(account)
        .felt(counterparty)
        .u64_be(3)
        .finish();
    let request = json!({
        "method": "open_channel",
        "counterparty": counterparty,
        "wire_version": "v3",
    });
    let journal = OperationJournal::new(&root).expect("journal");
    {
        let mut lease = journal
            .claim_with_request(
                &operation_id,
                WriteOperation::OpenChannel,
                binding,
                None,
                request,
                1_000,
            )
            .expect("claim");
        lease
            .advance(OperationStage::Prepared, 1_001)
            .expect("prepared");
        lease
            .advance(OperationStage::Proven, 1_002)
            .expect("proven");
        lease
            .persist_signed(Felt::from_hex_unchecked("0xbeef"), "{}", 1_003)
            .expect("signed");
        lease
            .amend(1_004, |attempt| {
                attempt.valid_until_block = Some(500);
                attempt.account_nonce = Some(Felt::from(5u8));
            })
            .expect("expiry and nonce");
        lease
            .advance(OperationStage::Submitted, 1_005)
            .expect("submitted");
    }

    let responses = vec![
        json!({"jsonrpc":"2.0","id":1,"result":"0x5"}),
        json!({"jsonrpc":"2.0","id":1,
               "error":{"code":29,"message":"Transaction hash not found"}}),
        json!({"jsonrpc":"2.0","id":1,"result":501}),
        json!({"jsonrpc":"2.0","id":1,"result":"0x5"}),
        json!({"jsonrpc":"2.0","id":1,"result":["0x0"]}),
        json!({"jsonrpc":"2.0","id":1,
               "result":[format!("{counterparty_public_key:#x}")]}),
    ];
    let (rpc_url, server) = server(responses);
    let client = Client::new(ClientConfig {
        rpc_url,
        prover_url: "http://127.0.0.1:9".to_owned(),
        pool_address: pool,
        chain_id: chain,
        account_address: account,
        pool_key_file,
        account_key_file,
        state_dir: root.clone(),
        token,
        new_channel_wire_version: WireVersion::V3,
    })
    .expect("client");

    let outcome = client
        .resume_operation(&operation_id)
        .await
        .expect("resume rebuilds");
    server.join().expect("server");
    assert_eq!(
        outcome,
        ResumeOutcome::Rebuilt {
            operation_result: json!(existing_handle),
        }
    );

    let lease = journal.lock(&operation_id).expect("lock").expect("record");
    assert_eq!(lease.record().attempts.len(), 2);
    assert_eq!(lease.record().stage(), OperationStage::Committed);
    assert_eq!(lease.record().result, Some(json!(existing_handle)));
}

#[tokio::test]
async fn a_valid_recorded_transaction_is_resubmitted_exactly_and_committed() {
    let root = temporary_root();
    let account = Felt::from(0x11u8);
    let pool = Felt::from(0x22u8);
    let chain = Felt::from(0x33u8);
    let token = Felt::from(0x44u8);
    let transaction_hash = Felt::from_hex_unchecked("0xbeef");
    let operation_id = OperationId::parse(format!("op_{}", "ac".repeat(32))).expect("operation id");
    let binding = RequestBinding::builder(WriteOperation::ApprovePool, chain, pool, token)
        .felt(account)
        .u128_be(100)
        .finish();
    let journal = OperationJournal::new(&root).expect("journal");
    {
        let mut lease = journal
            .claim_with_request(
                &operation_id,
                WriteOperation::ApprovePool,
                binding,
                None,
                json!({"method":"approve_pool","amount":"100"}),
                1_000,
            )
            .expect("claim");
        lease
            .record_completion(
                json!({
                    "result": {"kind":"approval","approved":"100"},
                    "local_mutation": null,
                }),
                1_001,
            )
            .expect("completion");
        lease
            .advance(OperationStage::Prepared, 1_002)
            .expect("prepared");
        lease
            .persist_signed(transaction_hash, "{}", 1_003)
            .expect("signed");
        lease
            .amend(1_004, |attempt| {
                attempt.account_nonce = Some(Felt::from(5u8));
            })
            .expect("nonce");
        lease
            .advance(OperationStage::Submitted, 1_005)
            .expect("submitted");
    }

    let responses = vec![
        json!({"jsonrpc":"2.0","id":1,"result":"0x5"}),
        json!({"jsonrpc":"2.0","id":1,
               "error":{"code":29,"message":"Transaction hash not found"}}),
        json!({"jsonrpc":"2.0","id":1,"result":100}),
        json!({"jsonrpc":"2.0","id":1,
               "result":{"transaction_hash":format!("{transaction_hash:#x}")}}),
        json!({"jsonrpc":"2.0","id":1,"result":{
            "transaction_hash":format!("{transaction_hash:#x}"),
            "block_number":101,
            "finality_status":"ACCEPTED_ON_L2",
            "execution_status":"SUCCEEDED"
        }}),
    ];
    let (rpc_url, server) = server(responses);
    let client = Client::new(ClientConfig {
        rpc_url,
        prover_url: "http://127.0.0.1:9".to_owned(),
        pool_address: pool,
        chain_id: chain,
        account_address: account,
        pool_key_file: root.join("pool.key"),
        account_key_file: root.join("account.key"),
        state_dir: root.clone(),
        token,
        new_channel_wire_version: WireVersion::V3,
    })
    .expect("client");

    let outcome = client
        .resume_operation(&operation_id)
        .await
        .expect("resume resubmits");
    server.join().expect("server");
    assert_eq!(outcome, ResumeOutcome::Resubmitted { transaction_hash });

    let lease = journal.lock(&operation_id).expect("lock").expect("record");
    assert_eq!(lease.record().stage(), OperationStage::Committed);
    assert_eq!(
        lease.record().result,
        Some(json!({"tx_hash":"0xbeef","approved":"100"}))
    );
    assert!(lease.record().attempt().receipt.is_some());
}
