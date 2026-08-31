//! Local transport contract for Starkscan's asynchronous STRK20 prover relay.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

use erebus_sdk::prover::{BlockId, ProvingService};
use erebus_sdk::tx::{DataAvailabilityMode, InvokeV3, ResourceBounds};
use serde_json::{json, Value};
use starknet_crypto::Signature;
use starknet_types_core::felt::Felt;

struct Request {
    method: String,
    path: String,
    headers: String,
    body: Value,
}

fn read_request(stream: &mut TcpStream) -> Request {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];
    let header_end = loop {
        let read = stream.read(&mut buffer).expect("request read");
        assert!(read > 0, "connection closed before headers");
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let headers = String::from_utf8(bytes[..header_end].to_vec()).expect("UTF-8 headers");
    let mut request_line = headers.lines().next().expect("request line").split(' ');
    let method = request_line.next().expect("method").to_owned();
    let path = request_line.next().expect("path").to_owned();
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(str::trim)
                .map(str::to_owned)
        })
        .map(|length| length.parse::<usize>().expect("content length"))
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let read = stream.read(&mut buffer).expect("body read");
        assert!(read > 0, "connection closed before body");
        bytes.extend_from_slice(&buffer[..read]);
    }
    let body = if content_length == 0 {
        Value::Null
    } else {
        serde_json::from_slice(&bytes[header_end..header_end + content_length]).expect("JSON body")
    };
    Request {
        method,
        path,
        headers,
        body,
    }
}

fn respond(stream: &mut TcpStream, status: &str, body: Value) {
    let body = body.to_string();
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
    .expect("response");
}

fn signed_invoke() -> erebus_sdk::tx::SignedInvokeV3 {
    InvokeV3 {
        sender_address: Felt::from(0x123u64),
        calldata: vec![Felt::ONE, Felt::TWO],
        chain_id: Felt::from(0x534eu64),
        nonce: Felt::ZERO,
        account_deployment_data: vec![],
        nonce_da_mode: DataAvailabilityMode::L1,
        fee_da_mode: DataAvailabilityMode::L1,
        resource_bounds: ResourceBounds::for_proof_invocation(),
        tip: 0,
        paymaster_data: vec![],
        proof_facts: vec![],
    }
    .with_signature(Signature {
        r: Felt::from(3u8),
        s: Felt::from(4u8),
    })
}

#[tokio::test]
async fn async_result_is_authenticated_and_persisted_before_return() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("capabilities accept");
        let request = read_request(&mut stream);
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/v1/meta/capabilities");
        assert!(request
            .headers
            .to_ascii_lowercase()
            .contains("x-starkscan-api-key: test-secret"));
        respond(
            &mut stream,
            "200 OK",
            json!({"caller":{"scopes":["read","prove"]}}),
        );

        let (mut stream, _) = listener.accept().expect("submit accept");
        let request = read_request(&mut stream);
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/v1/SN_MAIN/prove");
        let idempotency = request
            .headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("idempotency-key:")
                    .map(str::trim)
                    .map(str::to_owned)
            })
            .expect("idempotency header");
        assert!(idempotency.starts_with("erebus-"));
        assert_eq!(idempotency.len(), 72);
        assert_eq!(request.body["block_id"], json!({"block_number": 123}));
        assert!(request.body["transaction"].is_object());
        respond(
            &mut stream,
            "202 Accepted",
            json!({"jobId":"prv_test_job_1234","status":"queued","terminal":false}),
        );

        let (mut stream, _) = listener.accept().expect("poll accept");
        let request = read_request(&mut stream);
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/v1/SN_MAIN/prove/prv_test_job_1234");
        respond(
            &mut stream,
            "200 OK",
            json!({
                "jobId":"prv_test_job_1234",
                "status":"succeeded",
                "terminal":true,
                "result":{
                    "proof":"opaque-proof",
                    "proof_facts":["0xaa"],
                    "l2_to_l1_messages":[],
                    "additional_data":{"signature":{
                        "issued_at":1_800_000_000,
                        "sig_r":"0x1",
                        "sig_s":"0x2"
                    }}
                }
            }),
        );
    });

    let state_dir = std::env::temp_dir().join(format!(
        "erebus-starkscan-prover-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    std::env::set_var("STARKSCAN_API_KEY", "test-secret");
    let service =
        ProvingService::new_persistent(format!("http://{address}/v1/SN_MAIN/prove"), &state_dir)
            .expect("client");
    std::env::remove_var("STARKSCAN_API_KEY");

    assert_eq!(
        service.spec_version().await.expect("capabilities"),
        "starkscan-async/prove"
    );
    let key = format!("op_{}", "a".repeat(64));
    let first = service
        .prove_transaction_idempotent(&BlockId::Number(123), &signed_invoke(), &key)
        .await
        .expect("first proof");
    assert_eq!(first.proof, "opaque-proof");
    assert!(first
        .additional_data
        .as_ref()
        .and_then(|data| data.signature.as_ref())
        .is_some());

    // The server has no fourth response. A second call can only succeed by loading the
    // proof that was durably written before the first call returned.
    let second = service
        .prove_transaction_idempotent(&BlockId::Number(123), &signed_invoke(), &key)
        .await
        .expect("cached proof");
    assert_eq!(second, first);
    server.join().expect("server");

    let jobs = std::fs::read_dir(state_dir.join("prover-jobs"))
        .expect("job directory")
        .collect::<Result<Vec<_>, _>>()
        .expect("job entries");
    assert_eq!(jobs.len(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            jobs[0].metadata().expect("metadata").permissions().mode() & 0o777,
            0o600
        );
    }
    std::fs::remove_dir_all(state_dir).expect("cleanup");
}
