//! Process-level checks; no RPC or funded account is used.
use std::io::Write;
use std::process::{Command, Output, Stdio};

fn run(input: &[u8], serve: bool) -> Output {
    run_with_config(input, serve, &[])
}

fn run_with_config(input: &[u8], serve: bool, extra: &[(&str, &str)]) -> Output {
    run_with_verification(input, serve, extra, Some("http://127.0.0.1:10"))
}

fn run_with_verification(
    input: &[u8],
    serve: bool,
    extra: &[(&str, &str)],
    verification: Option<&str>,
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_erebus-tx-relayer"));
    if serve {
        command.arg("--serve");
    }
    command
        .env_clear()
        .env("EREBUS_RELAYER_NAMESPACE", "eip155:31337")
        .env(
            "EREBUS_RELAYER_SETTLEMENT",
            format!("0x{}", "11".repeat(20)),
        )
        .env("EREBUS_RELAYER_VERIFIER_VERSION", "1")
        .env("EREBUS_RELAYER_KEY", "01".repeat(32))
        .env(
            "EREBUS_RELAYER_FEE_RECIPIENT",
            format!("0x{}", "22".repeat(20)),
        )
        .env("EREBUS_RELAYER_FEES", format!("0x{}=1", "33".repeat(20)))
        .env("EREBUS_RELAYER_RPC_URLS", "http://127.0.0.1:9")
        .env("EREBUS_RELAYER_MAX_REQUESTS", "1")
        .env("EREBUS_RELAYER_WINDOW_SECONDS", "3600")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(url) = verification {
        command.env("EREBUS_RELAYER_VERIFICATION_RPC_URL", url);
    }
    for (name, value) in extra {
        command.env(name, value);
    }
    let mut child = command.spawn().unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn serve_retains_limits_and_counters_after_rejected_requests() {
    let output = run(b"not-json\n{\"method\":\"funding\",\"evidence\":\"00\"}\n{\"method\":\"funding\",\"evidence\":\"00\"}\n{\"method\":\"health\"}\n", true);
    assert!(output.status.success());
    let lines: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[0]["error"], "request is not valid JSON");
    assert_eq!(lines[1]["error"], "invalid relay agreement");
    assert_eq!(lines[2]["error"], "relayer access limit exceeded");
    assert_eq!(lines[3]["metrics"]["rejected"], 1);
    assert_eq!(lines[3]["metrics"]["rate_limited"], 1);
    assert_eq!(lines[3]["metrics"]["submitted"], 0);
    assert!(output.stderr.is_empty());
}

#[test]
fn one_shot_mode_keeps_its_exit_status_contract() {
    assert!(run(b"{\"method\":\"health\"}", false).status.success());
    assert!(!run(b"not-json", false).status.success());
}

#[test]
fn relay_refuses_to_send_without_durable_recovery_state() {
    let output = run(b"{\"method\":\"relay\",\"evidence\":\"00\"}", false);
    assert!(!output.status.success());
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        response["error"],
        "relay requires a durable operation journal"
    );
}

#[test]
fn oversized_frame_is_rejected_before_json_parsing() {
    let input = vec![b' '; 2 * erebus_evm::relay::MAX_RELAY_EVIDENCE_BYTES + 4097];
    let output = run(&input, true);
    assert!(!output.status.success());
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["error"], "request exceeds size limit");
}

#[test]
fn configured_durable_requests_reject_bad_evidence_before_creating_state() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("not-created");
    let output = run_with_config(
        b"{\"method\":\"relay\",\"evidence\":\"00\"}\n{\"method\":\"recover\",\"evidence\":\"00\"}\n",
        true,
        &[
            ("EREBUS_RELAYER_STATE_ROOT", state.to_str().unwrap()),
            ("EREBUS_RELAYER_MAX_FEE_PER_GAS", "2000000000"),
            ("EREBUS_RELAYER_PRIORITY_FEE_PER_GAS", "1000000000"),
            ("EREBUS_RELAYER_GAS_LIMIT", "1000000"),
            ("EREBUS_RELAYER_MAX_REQUESTS", "2"),
        ],
    );
    assert!(output.status.success());
    let responses: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(responses.len(), 2);
    for response in responses {
        assert_eq!(response["error"], "invalid relay agreement");
    }
    assert!(!state.exists());
    assert!(output.stderr.is_empty());
}

#[test]
fn persistent_submission_requires_explicit_gas_caps() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("not-created");
    let output = run_with_config(
        b"{\"method\":\"health\"}",
        false,
        &[("EREBUS_RELAYER_STATE_ROOT", state.to_str().unwrap())],
    );
    assert!(!output.status.success());
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        response["error"],
        "EREBUS_RELAYER_MAX_FEE_PER_GAS is not set"
    );
    assert!(!state.exists());
}

#[test]
fn durable_relayer_requires_a_distinct_second_rpc_without_creating_state() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("not-created");
    let config = [
        ("EREBUS_RELAYER_STATE_ROOT", state.to_str().unwrap()),
        ("EREBUS_RELAYER_MAX_FEE_PER_GAS", "2000000000"),
        ("EREBUS_RELAYER_PRIORITY_FEE_PER_GAS", "1000000000"),
        ("EREBUS_RELAYER_GAS_LIMIT", "1000000"),
    ];
    for endpoint in [
        None,
        Some("http://127.0.0.1:9/"),
        Some("http://127.0.0.1:9/#different"),
        Some("invalid"),
    ] {
        let output = run_with_verification(b"{\"method\":\"health\"}", false, &config, endpoint);
        assert!(!output.status.success());
        let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            response["error"],
            if endpoint.is_none() {
                "EREBUS_RELAYER_VERIFICATION_RPC_URL is not set"
            } else {
                "invalid durable relayer configuration"
            }
        );
        assert!(!state.exists());
        assert!(output.stderr.is_empty());
    }
    assert!(
        run_with_verification(b"{\"method\":\"health\"}", false, &[], None)
            .status
            .success()
    );
}
