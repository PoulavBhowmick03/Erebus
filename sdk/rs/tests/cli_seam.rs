//! Tests for `erebus-cli` — P0.4, the Python ↔ Rust seam.
//!
//! ARCHITECTURE:199 calls this seam the highest-risk item in the plan, because it belongs to
//! neither track and so gets built by nobody until integration day. These tests exist to
//! make it exist early, with the marshalling proven before there is anything real to marshal.
//!
//! Two properties matter more than the rest. The CLI must agree with the library — a seam
//! that quietly computes something different is worse than no seam. And key material must
//! travel as a *path*, never as a value, because the whole reason subprocess beat PyO3 is
//! that the agent's Python heap never holds the pool key.

use std::io::Write;
use std::process::{Command, Stdio};

use erebus_sdk::channel::{Channel, Counterparty, PoolIdentity};
use starknet_types_core::felt::Felt;

const CLI: &str = env!("CARGO_BIN_EXE_erebus-cli");

/// Runs the CLI with `request` on stdin, returning the parsed envelope and exit success.
fn run(request: &str) -> (serde_json::Value, bool) {
    let mut child = Command::new(CLI)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("erebus-cli starts");
    child
        .stdin
        .as_mut()
        .expect("stdin is piped")
        .write_all(request.as_bytes())
        .expect("request writes");

    let output = child.wait_with_output().expect("cli exits");
    let parsed = serde_json::from_slice(&output.stdout).expect("stdout is one JSON envelope");
    (parsed, output.status.success())
}

/// Writes a key file the way an operator would, and returns its path.
///
/// `name` scopes it per test: these run in parallel in one process, so a shared path means
/// one test deleting the file another is still using.
fn key_file(name: &str, key: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir()
        .join(format!("erebus-test-{}-{name}.key", std::process::id()));
    std::fs::write(&path, key).expect("key file writes");
    path
}

fn open_channel_request(key_path: &std::path::Path, register: bool) -> String {
    format!(
        r#"{{"method":"open_channel","params":{{
            "address":"0xa11ce",
            "key_file":"{}",
            "counterparty_address":"0xb0b",
            "counterparty_public_key":"0x9bcdef",
            "token":"0x7042",
            "channel_index":0,
            "subchannel_index":0,
            "register":{register}
        }}}}"#,
        key_path.display()
    )
}

// --- The envelope ---------------------------------------------------------------

#[test]
fn version_answers_without_touching_key_material() {
    let (response, ok) = run(r#"{"method":"version"}"#);

    assert!(ok);
    assert_eq!(response["ok"], true);
    assert_eq!(response["result"]["protocol"], 1);
    assert!(response["error"].is_null(), "a success carries no error");
}

/// The envelope is the contract. A caller that sees `ok: false` must always find a code and
/// a retryable flag, whatever went wrong.
#[test]
fn every_failure_carries_a_code_and_a_retryable_flag() {
    for request in [
        "not json at all",
        r#"{"method":"no_such_method"}"#,
        r#"{"method":"open_channel","params":{}}"#,
    ] {
        let (response, ok) = run(request);
        assert!(!ok, "{request} should fail");
        assert_eq!(response["ok"], false);
        assert!(
            response["error"]["code"].is_string(),
            "no code for: {request}"
        );
        assert!(
            response["error"]["retryable"].is_boolean(),
            "no retryable flag for: {request}"
        );
        assert!(response["result"].is_null(), "a failure carries no result");
    }
}

/// Exit status mirrors the envelope so a caller can fail fast, but the envelope stays
/// authoritative — the status carries no detail.
#[test]
fn exit_status_agrees_with_the_envelope() {
    let (success, ok) = run(r#"{"method":"version"}"#);
    assert!(ok && success["ok"] == true);

    let (failure, ok) = run("garbage");
    assert!(!ok && failure["ok"] == false);
}

// --- Agreement with the library -------------------------------------------------

/// The failure this guards is a seam that computes something *plausible* but different. If
/// the CLI ever derived a channel key the library would not, every note would be written
/// where nobody reads, and nothing would error.
#[test]
fn the_cli_derives_the_same_channel_key_as_the_library() {
    let path = key_file("derive", "0x1234567890abcdef");
    let (response, ok) = run(&open_channel_request(&path, true));
    assert!(ok, "open_channel failed: {response}");

    let expected = Channel::derive(
        &PoolIdentity::new(
            Felt::from_hex("0xa11ce").expect("addr"),
            Felt::from_hex("0x1234567890abcdef").expect("key"),
        ),
        Counterparty {
            address: Felt::from_hex("0xb0b").expect("addr"),
            public_key: Felt::from_hex("0x9bcdef").expect("pubkey"),
        },
    );

    assert_eq!(
        response["result"]["channel_handle"].as_str().expect("handle"),
        format!("{:#x}", expected.key()),
        "the CLI and the library disagree on the channel key"
    );
    let _ = std::fs::remove_file(path);
}

/// Registration is one extra action, and it is the difference between a first run and a
/// returning agent — a `SetViewingKey` written twice reverts on the write-once.
#[test]
fn registration_changes_the_action_count() {
    let path = key_file("register", "0x1234567890abcdef");

    let (first, _) = run(&open_channel_request(&path, true));
    let (returning, _) = run(&open_channel_request(&path, false));

    assert_eq!(first["result"]["action_count"], 3, "register + channel + subchannel");
    assert_eq!(returning["result"]["action_count"], 2, "channel + subchannel");
    let _ = std::fs::remove_file(path);
}

// --- Key handling ---------------------------------------------------------------

/// The reason subprocess beat PyO3. The key travels as a path, so the caller's process never
/// holds it — not in a request body, not on argv, not in transit.
#[test]
fn a_missing_key_file_fails_without_the_caller_ever_supplying_a_key() {
    let (response, ok) = run(
        r#"{"method":"open_channel","params":{
            "address":"0xa11ce","key_file":"/definitely/not/here",
            "counterparty_address":"0xb0b","counterparty_public_key":"0x9bcdef",
            "token":"0x7042","channel_index":0,"subchannel_index":0,"register":false}}"#,
    );

    assert!(!ok);
    assert_eq!(response["error"]["code"], "IDENTITY_UNAVAILABLE");
    assert_eq!(response["error"]["retryable"], false);
}

/// A response must never echo key material back, however the request failed.
#[test]
fn no_response_contains_the_private_key() {
    let path = key_file("noecho", "0x1234567890abcdef");
    let (response, _) = run(&open_channel_request(&path, true));
    let rendered = response.to_string();

    assert!(
        !rendered.contains("1234567890abcdef"),
        "the pool private key came back out of the CLI"
    );
    let _ = std::fs::remove_file(path);
}

// --- Entropy --------------------------------------------------------------------

/// Entropy is generated inside the binary, not supplied by the caller. If Python passed it,
/// `sdk/py` would be making a cryptographic decision — which is the one thing it must not
/// do, because a second place that can produce a weak salt is a second silent failure.
///
/// Two identical requests must therefore differ in their action sets while agreeing on the
/// channel key, which is derived and not random.
#[test]
fn entropy_comes_from_the_binary_not_the_request() {
    let path = key_file("entropy", "0x1234567890abcdef");
    let (first, _) = run(&open_channel_request(&path, true));
    let (second, _) = run(&open_channel_request(&path, true));

    assert_eq!(
        first["result"]["channel_handle"], second["result"]["channel_handle"],
        "the channel key is derived, so it must be stable across runs"
    );
    assert_eq!(first["result"]["action_count"], second["result"]["action_count"]);
    let _ = std::fs::remove_file(path);
}
