//! In-process transport test for the full action execution sequence.
//!
//! The servers are deterministic local JSON-RPC fixtures. This does not claim Sepolia
//! compatibility; it pins that one runtime path carries the action set through preflight,
//! proof, proof-facts-aware estimation/signing, submission, and receipt.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use erebus_sdk::action_set::ActionSetBuilder;
use erebus_sdk::actions::{ClientAction, OpenChannelInput};
use erebus_sdk::calldata;
use erebus_sdk::execution::{ExecutionConfig, Executor};
use erebus_sdk::journal::{OperationJournal, OperationStage};
use erebus_sdk::operation::{OperationId, RequestBinding, WriteOperation};
use erebus_sdk::prover::ProvingService;
use erebus_sdk::rpc::StarknetRpc;
use serde_json::{json, Value};
use starknet_types_core::felt::Felt;

/// The v3 hash of the exact `apply_actions` invoke this fixture builds.
///
/// Pinned rather than echoed from the mock: the executor now refuses a node that answers a
/// submission with a hash other than the one it signed, so a mock returning an arbitrary
/// value would exercise the failure path instead of the success path.
const SUBMITTED_HASH: &str = "0x5ce3fae88e0faa5a41a1e063b29f0c594acb6482105beef4c86ecf1a197d3e6";

fn server(responses: Vec<Value>) -> (String, Receiver<Value>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    let (sender, receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        for response in responses {
            let (mut stream, _) = listener.accept().expect("accept");
            let request = read_json_request(&mut stream);
            sender.send(request).expect("capture");
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
    (format!("http://{address}"), receiver, handle)
}

fn read_json_request(stream: &mut TcpStream) -> Value {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];
    let (header_end, content_length) = loop {
        let read = stream.read(&mut buffer).expect("request read");
        assert!(read > 0, "connection closed before headers");
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let header_end = end + 4;
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            let length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(str::trim)
                        .map(str::to_owned)
                })
                .expect("content-length")
                .parse::<usize>()
                .expect("content length");
            break (header_end, length);
        }
    };
    while bytes.len() < header_end + content_length {
        let read = stream.read(&mut buffer).expect("body read");
        assert!(read > 0, "connection closed before body");
        bytes.extend_from_slice(&buffer[..read]);
    }
    serde_json::from_slice(&bytes[header_end..header_end + content_length]).expect("JSON body")
}

#[tokio::test]
async fn one_path_preflights_proves_submits_and_waits() {
    let pool = Felt::from(0x123u64);
    let account = Felt::from(0x456u64);
    let server_actions = ["0x1", "0x2a"];

    let rpc_responses = vec![
        json!({"jsonrpc":"2.0","id":1,"result":20}),
        json!({"jsonrpc":"2.0","id":1,"result":server_actions}),
        json!({"jsonrpc":"2.0","id":1,"result":"0x0"}),
        json!({"jsonrpc":"2.0","id":1,"result":20}),
        json!({"jsonrpc":"2.0","id":1,"result":"0x1"}),
        json!({"jsonrpc":"2.0","id":1,"result":[{
            "l1_gas_consumed":"0x64",
            "l1_gas_price":"0xa",
            "l2_gas_consumed":"0xc8",
            "l2_gas_price":"0x14",
            "l1_data_gas_consumed":"0x2",
            "l1_data_gas_price":"0x3",
            "overall_fee":"0x1",
            "unit":"FRI"
        }]}),
        json!({"jsonrpc":"2.0","id":1,"result":{"transaction_hash":SUBMITTED_HASH}}),
        json!({"jsonrpc":"2.0","id":1,"result":{
            "transaction_hash":SUBMITTED_HASH,
            "block_number":21,
            "finality_status":"ACCEPTED_ON_L2",
            "execution_status":"SUCCEEDED"
        }}),
    ];
    let (rpc_url, rpc_requests, rpc_thread) = server(rpc_responses);
    let prover_responses = vec![json!({
        "jsonrpc":"2.0",
        "id":1,
        "result":{
            "proof":"opaque-proof",
            "proof_facts":["0xaa"],
            "l2_to_l1_messages":[{
                "from_address":format!("{pool:#x}"),
                "to_address":"0x0",
                "payload":["0x777", server_actions[0], server_actions[1]]
            }],
            "additional_data":null
        }
    })];
    let (prover_url, prover_requests, prover_thread) = server(prover_responses);

    let rpc = StarknetRpc::new(rpc_url).expect("rpc");
    let prover = ProvingService::new(prover_url)
        .expect("prover")
        .with_max_retries(0);
    let executor = Executor::new(
        rpc,
        prover,
        ExecutionConfig::new(pool, Felt::from(0x534eu64), account),
    );

    let mut builder = ActionSetBuilder::new();
    builder
        .push(ClientAction::OpenChannel(OpenChannelInput {
            recipient_addr: Felt::from(7u8),
            index: 0,
            random: Felt::from(11u8),
            salt: Felt::from(13u8),
        }))
        .expect("action");
    let actions = builder.build().expect("set");

    let state_dir = std::env::temp_dir().join(format!(
        "erebus-pipeline-{}-{}",
        std::process::id(),
        rand_suffix()
    ));
    let journal = OperationJournal::new(&state_dir).expect("journal");
    let operation_id = OperationId::parse(format!("op_{}", "5c".repeat(32))).expect("operation id");
    let binding = RequestBinding::builder(
        WriteOperation::OpenChannel,
        Felt::from(0x534eu64),
        pool,
        Felt::from(3u8),
    )
    .felt(Felt::from(7u8))
    .finish();
    let mut operation = journal
        .claim(
            &operation_id,
            WriteOperation::OpenChannel,
            binding,
            None,
            1_000,
        )
        .expect("claim");

    let receipt = executor
        .execute(
            &mut operation,
            account,
            Felt::from(0xabc_u64),
            Felt::from(0xdef_u64),
            &actions,
        )
        .await
        .expect("pipeline");

    assert_eq!(
        receipt.transaction_hash,
        Felt::from_hex_unchecked(SUBMITTED_HASH)
    );
    assert_eq!(receipt.proving_block, 10);

    // The journal walked every durable boundary, and the hash it wrote down before
    // submitting is the hash the chain reported back.
    let record = operation.record();
    assert_eq!(record.stage(), OperationStage::Accepted);
    assert_eq!(record.attempt().proving_block, Some(10));
    assert_eq!(
        record.attempt().transaction_hash,
        Some(Felt::from_hex_unchecked(SUBMITTED_HASH))
    );
    assert!(record.attempt().transaction_stored);
    assert!(record.attempt().accepted_at.is_some());

    // The stored transaction is the exact wire request, so a resume can resubmit it
    // without recomputing anything that would change the hash.
    let stored = operation
        .stored_transaction(0)
        .expect("stored transaction reads")
        .expect("a transaction was stored");
    let stored: Value = serde_json::from_str(&stored).expect("stored transaction is wire JSON");
    assert_eq!(stored["type"], "INVOKE");
    assert_eq!(stored["proof"], "opaque-proof");
    assert_eq!(stored["proof_facts"], json!(["0xaa"]));

    rpc_thread.join().expect("rpc server");
    prover_thread.join().expect("prover server");
    let rpc_requests: Vec<Value> = rpc_requests.try_iter().collect();
    assert_eq!(
        rpc_requests
            .iter()
            .map(|request| request["method"].as_str().expect("method"))
            .collect::<Vec<_>>(),
        [
            "starknet_blockNumber",
            "starknet_call",
            "starknet_getNonce",
            "starknet_blockNumber",
            "starknet_getNonce",
            "starknet_estimateFee",
            "starknet_addInvokeTransaction",
            "starknet_getTransactionReceipt",
        ]
    );

    let estimate = &rpc_requests[5]["params"]["request"][0];
    assert_eq!(estimate["version"], "0x100000000000000000000000000000003");
    assert_eq!(estimate["proof"], "opaque-proof");
    assert_eq!(estimate["proof_facts"], json!(["0xaa"]));

    let submitted = &rpc_requests[6]["params"]["invoke_transaction"];
    assert_eq!(submitted["version"], "0x3");
    assert_eq!(submitted["proof"], "opaque-proof");
    assert_eq!(submitted["proof_facts"], json!(["0xaa"]));
    assert_eq!(
        submitted["calldata"][2],
        format!("{:#x}", calldata::selector("apply_actions"))
    );
    assert_eq!(
        submitted["resource_bounds"]["l2_gas"]["max_amount"], "0x12c",
        "200 estimated plus 50 percent"
    );

    let prover_requests: Vec<Value> = prover_requests.try_iter().collect();
    assert_eq!(prover_requests.len(), 1);
    let proof_tx = &prover_requests[0]["params"]["transaction"];
    assert_eq!(proof_tx["sender_address"], format!("{pool:#x}"));
    assert!(
        proof_tx["calldata"].as_array().expect("calldata").len() > 7,
        "runtime proof calldata includes the non-empty ActionSet"
    );
    assert!(proof_tx.get("proof").is_none());
    assert!(proof_tx.get("proof_facts").is_none());
}

fn rand_suffix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after the epoch")
        .subsec_nanos()
        .into()
}
