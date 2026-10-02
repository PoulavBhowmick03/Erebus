//! Fresh `erebus-settle` processes: read-only onboarding without keys, chain, or RPC access
//! for the failure paths. The funded path reuses the Anvil-tested `estimate` and
//! `funding_diagnostics` methods.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_erebus-settle"))
}

fn run(root: &std::path::Path, bytes: &[u8]) -> Output {
    let mut child = Command::new(binary())
        .current_dir(root)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(bytes).unwrap();
    child.wait_with_output().unwrap()
}

fn request(root: &std::path::Path, value: Value) -> (Output, Value) {
    let output = run(root, &serde_json::to_vec(&value).unwrap());
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response = serde_json::from_slice(&output.stdout).unwrap();
    (output, response)
}

fn funding_request() -> Value {
    json!({
        "method": "funding",
        "deployment": {
            "namespace": "eip155:10143",
            "settlement_contract": "0x1111111111111111111111111111111111111111",
            "verifier_version": 1,
            "rpc_url": "http://127.0.0.1:1",
        },
        "signer_address": "0x2222222222222222222222222222222222222222",
        "evidence": "00",
    })
}

#[test]
fn help_needs_no_configuration_or_stdin() {
    let output = Command::new(binary())
        .env_clear()
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("funding"));
    assert!(text.contains("read-only"));
}

#[test]
fn invalid_requests_are_rejected_without_echoing_the_input() {
    let dir = tempfile::tempdir().unwrap();

    let (output, response) = request(dir.path(), json!({"method": "nope"}));
    assert!(!output.status.success());
    assert_eq!(response["error"], "invalid request");

    let output = run(dir.path(), b"not json at all");
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("not json at all"));

    let output = run(dir.path(), &vec![b'x'; 128 * 1024]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("size limit"));
}

#[test]
fn malformed_evidence_and_deployments_fail_before_any_connection() {
    let dir = tempfile::tempdir().unwrap();

    let (output, response) = request(dir.path(), funding_request());
    assert!(!output.status.success());
    assert_eq!(response["error"], "invalid evidence");

    let mut wrong = funding_request();
    wrong["deployment"]["namespace"] = json!("not-a-namespace");
    let (output, response) = request(dir.path(), wrong);
    assert!(!output.status.success());
    assert_eq!(response["error"], "invalid deployment namespace");

    let mut wrong = funding_request();
    wrong["signer_address"] = json!("0xAA");
    let (output, response) = request(dir.path(), wrong);
    assert!(!output.status.success());
    assert_eq!(response["error"], "invalid signer address");
}

#[test]
fn capabilities_reports_the_public_bound_backend_without_a_network() {
    let dir = tempfile::tempdir().unwrap();
    let (output, response) = request(dir.path(), json!({"method": "capabilities"}));
    assert!(output.status.success());
    assert_eq!(response["backend"], "evm-public-bound");
    assert_eq!(response["suites"], json!([1]));
    assert_eq!(response["modes"], json!(["public-bound"]));
    assert_eq!(response["local_proving"], true);
    assert!(response["guarantees"]
        .as_array()
        .expect("guarantees")
        .iter()
        .any(|guarantee| guarantee == "agreement-bound-settlement"));
}

#[test]
fn receipt_requires_durable_state_and_a_valid_operation_reference() {
    let dir = tempfile::tempdir().unwrap();
    let deployment = json!({
        "namespace": "eip155:10143",
        "settlement_contract": "0x1111111111111111111111111111111111111111",
        "verifier_version": 1,
        "rpc_url": "http://127.0.0.1:1",
    });

    let (output, response) = request(
        dir.path(),
        json!({
            "method": "receipt",
            "deployment": deployment,
            "state_root": dir.path().join("missing"),
            "operation_ref": "01".repeat(32),
        }),
    );
    assert!(!output.status.success());
    assert_eq!(response["error"], "cannot read durable agreement opening");

    let (output, response) = request(
        dir.path(),
        json!({
            "method": "receipt",
            "deployment": deployment,
            "state_root": dir.path(),
            "operation_ref": "not-hex",
        }),
    );
    assert!(!output.status.success());
    assert_eq!(response["error"], "invalid operation reference");
}
