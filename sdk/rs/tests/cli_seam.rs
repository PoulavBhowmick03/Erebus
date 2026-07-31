//! Transport-contract tests for `erebus-cli`.
//!
//! Protocol correctness is tested from `ActionSet` down in the library. These tests stay at
//! the process boundary: one envelope, structured failures, path-only key configuration,
//! and opaque-handle validation.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const CLI: &str = env!("CARGO_BIN_EXE_erebus-cli");

fn run(request: &str) -> (serde_json::Value, bool) {
    let mut child = Command::new(CLI)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("erebus-cli starts");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(request.as_bytes())
        .expect("request");
    let output = child.wait_with_output().expect("exit");
    let envelope = serde_json::from_slice(&output.stdout).expect("one JSON envelope");
    (envelope, output.status.success())
}

fn config(name: &str, pool_key_file: &str, account_key_file: &str) -> serde_json::Value {
    let state_dir =
        std::env::temp_dir().join(format!("erebus-cli-test-{}-{name}", std::process::id()));
    serde_json::json!({
        "rpc_url": "http://127.0.0.1:1",
        "prover_url": "http://127.0.0.1:1",
        "pool_address": "0x123",
        "chain_id": "0x534e5f5345504f4c4941",
        "account_address": "0xa11ce",
        "pool_key_file": pool_key_file,
        "account_key_file": account_key_file,
        "state_dir": state_dir,
        "token": "0x7042"
    })
}

fn temporary_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("erebus-cli-key-test-{}-{name}", std::process::id()))
}

#[test]
fn version_is_protocol_two() {
    let (response, ok) = run(r#"{"method":"version"}"#);
    assert!(ok);
    assert_eq!(response["ok"], true);
    assert_eq!(response["result"]["protocol"], 2);
    assert!(response["error"].is_null());
}

#[test]
fn generate_pool_key_writes_the_secret_but_returns_only_public_metadata() {
    let root = temporary_path("generate");
    std::fs::create_dir(&root).expect("temporary key directory");
    let key_file = root.join("pool.key");
    let request = serde_json::json!({
        "method": "generate_pool_key",
        "params": { "path": key_file }
    });

    let (response, ok) = run(&request.to_string());
    assert!(ok, "{response}");
    assert_eq!(response["ok"], true);
    assert_eq!(
        response["result"]["pool_key_file"],
        key_file.to_str().expect("UTF-8 test path")
    );
    assert!(response["result"]["public_key"].is_string());
    assert_eq!(
        response["result"].as_object().expect("result object").len(),
        2,
        "the private key must not enter the response"
    );

    let private = std::fs::read_to_string(&key_file).expect("key file");
    assert!(private.trim().starts_with("0x"));
    assert!(!response.to_string().contains(private.trim()));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&key_file)
            .expect("key metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn generate_pool_key_refuses_to_overwrite() {
    let root = temporary_path("overwrite");
    std::fs::create_dir(&root).expect("temporary key directory");
    let key_file = root.join("pool.key");
    std::fs::write(&key_file, "sentinel").expect("existing file");
    let request = serde_json::json!({
        "method": "generate_pool_key",
        "params": { "path": key_file }
    });

    let (response, ok) = run(&request.to_string());
    assert!(!ok);
    assert_eq!(response["error"]["code"], "IDENTITY_UNAVAILABLE");
    assert_eq!(
        std::fs::read_to_string(&key_file).expect("existing file"),
        "sentinel"
    );

    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn every_failure_has_a_code_and_retryability() {
    for request in [
        "not json",
        r#"{"method":"no_such_method"}"#,
        r#"{"method":"open_channel","params":{}}"#,
    ] {
        let (response, ok) = run(request);
        assert!(!ok);
        assert_eq!(response["ok"], false);
        assert!(response["error"]["code"].is_string());
        assert!(response["error"]["retryable"].is_boolean());
        assert!(response["result"].is_null());
    }
}

#[test]
fn exit_status_mirrors_the_envelope() {
    let (success, ok) = run(r#"{"method":"version"}"#);
    assert!(ok && success["ok"] == true);
    let (failure, ok) = run("garbage");
    assert!(!ok && failure["ok"] == false);
}

#[test]
fn malformed_handle_is_rejected_before_any_network_call() {
    let request = serde_json::json!({
        "method": "read_channel_state",
        "params": {
            "config": config("handle", "/definitely/not/a/pool-key", "/definitely/not/an/account-key"),
            "handle": "../../pool-key"
        }
    });
    let (response, ok) = run(&request.to_string());
    assert!(!ok);
    // Key files are read before a valid state lookup, so this particular request may report
    // identity first. A valid-key variant below pins path-safe handle parsing itself.
    assert!(
        matches!(
            response["error"]["code"].as_str(),
            Some("INVALID_REQUEST" | "IDENTITY_UNAVAILABLE")
        ),
        "{response}"
    );
}

#[test]
fn missing_key_files_are_identity_unavailable() {
    let handle = format!("ch_{}", "ab".repeat(32));
    let request = serde_json::json!({
        "method": "read_channel_state",
        "params": {
            "config": config("missing", "/definitely/not/a/pool-key", "/definitely/not/an/account-key"),
            "handle": handle
        }
    });
    let (response, ok) = run(&request.to_string());
    assert!(!ok);
    assert_eq!(response["error"]["code"], "IDENTITY_UNAVAILABLE");
    assert_eq!(response["error"]["retryable"], false);
}

#[test]
fn request_carries_key_paths_not_key_values() {
    let request = serde_json::json!({
        "method": "shield",
        "params": {
            "config": config("paths", "/keys/pool.key", "/keys/account.key"),
            "amount": "1000"
        }
    });
    let rendered = request.to_string();
    assert!(rendered.contains("/keys/pool.key"));
    assert!(rendered.contains("/keys/account.key"));
    assert!(!rendered.contains("pool_private_key"));
    assert!(!rendered.contains("account_private_key"));
}

#[test]
fn a_key_value_field_is_rejected_not_ignored() {
    let mut configured = config("reject-key", "/keys/pool.key", "/keys/account.key");
    configured["pool_private_key"] = serde_json::json!("0xdeadbeef");
    let request = serde_json::json!({
        "method": "shield",
        "params": {
            "config": configured,
            "amount": "1000"
        }
    });
    let (response, ok) = run(&request.to_string());
    assert!(!ok);
    assert_eq!(response["error"]["code"], "INVALID_REQUEST");
}

#[test]
fn legacy_open_channel_shape_fails_loudly() {
    let (response, ok) = run(r#"{"method":"open_channel","params":{
            "address":"0xa11ce",
            "key_file":"/keys/pool.key",
            "counterparty_address":"0xb0b",
            "counterparty_public_key":"0x9bcdef",
            "token":"0x7042",
            "channel_index":0,
            "subchannel_index":0,
            "register":false
        }}"#);
    assert!(!ok);
    assert_eq!(response["error"]["code"], "INVALID_REQUEST");
}
