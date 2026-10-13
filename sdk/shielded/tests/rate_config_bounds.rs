//! Binary-config read-rate bounds for the actual wallet and payment binaries.
//!
//! Both binaries must reject primary/peer rates 0 and >40 before any RPC request, signing or
//! broadcast, and accept 1 and 40 (and omitted fields) with the documented defaults. Request
//! counters prove the rejection happens before any network use.

use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use axum::{extract::State, routing::post, Json, Router};
use serde_json::{json, Value};

#[derive(Default)]
struct Counters {
    reads: AtomicUsize,
    sends: AtomicUsize,
}

async fn mock(State(counters): State<Arc<Counters>>, Json(request): Json<Value>) -> Json<Value> {
    if request["method"] == "eth_sendRawTransaction" {
        counters.sends.fetch_add(1, Ordering::SeqCst);
    } else {
        counters.reads.fetch_add(1, Ordering::SeqCst);
    }
    let result = match request["method"].as_str() {
        Some("eth_chainId") => json!("0x279f"),
        Some("eth_blockNumber") => json!("0x2"),
        Some("eth_getBlockByNumber") => json!({
            "number": "0x2",
            "hash": format!("0x{}", "11".repeat(32)),
            "parentHash": format!("0x{}", "22".repeat(32)),
            "timestamp": "0x64"
        }),
        Some("eth_getLogs") => json!([]),
        Some("eth_getBalance") => json!(format!("0x{:064x}", 1)),
        Some("eth_getCode") => json!("0x"),
        Some("eth_call") => json!(format!("0x{:064x}", 0)),
        _ => json!(null),
    };
    Json(json!({"jsonrpc": "2.0", "id": request["id"], "result": result}))
}

struct Mock {
    url: String,
    counters: Arc<Counters>,
    _server: tokio::task::JoinHandle<()>,
}

async fn start_mock() -> Mock {
    let counters = Arc::new(Counters::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn({
        let counters = counters.clone();
        async move {
            axum::serve(
                listener,
                Router::new().route("/", post(mock)).with_state(counters),
            )
            .await
            .unwrap();
        }
    });
    Mock {
        url,
        counters,
        _server: server,
    }
}

fn write_private(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn run(binary: &str, request: &Value) -> (i32, String, String) {
    let mut child = Command::new(binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(request.to_string().as_bytes())
        .unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

fn hex32() -> String {
    "11".repeat(32)
}

fn payment_config(
    root: &Path,
    primary: &str,
    peer: &str,
    primary_rate: Option<u32>,
    peer_rate: Option<u32>,
) -> Value {
    let mut config = json!({
        "version": 1,
        "mode": "shielded",
        "state_root": root.join("state"),
        "namespace": "eip155:10143",
        "pool": format!("0x{}", "22".repeat(20)),
        "verifier_version": 2,
        "runtime_keccak256": hex32(),
        "poseidon_keccak256": hex32(),
        "deposit_verifier_keccak256": hex32(),
        "transfer_verifier_keccak256": hex32(),
        "withdraw_verifier_keccak256": hex32(),
        "first_block": 1,
        "first_hash": hex32(),
        "rpc_url": primary,
        "peer_rpc_url": peer,
        "buyer_key_hex": hex32(),
        "asset": format!("eip155:10143/erc20:0x{}", "33".repeat(20)),
        "signer_address": format!("0x{}", "44".repeat(20)),
        "signer_journal_root": root.join("nonces"),
        "transaction_key_file": null,
        "wallet_file": root.join("wallet.enc"),
        "wallet_key_file": root.join("wallet.key"),
        "manifest_file": root.join("manifest.json"),
        "manifest_sha256": hex32(),
        "artifact_cache": root.join("artifacts"),
        "allow_test_artifacts": true,
        "allow_loopback_http": true,
        "maximum_price": "100",
        "gas_limit": 3_000_000,
        "max_fee_per_gas": "2000000000",
        "max_priority_fee_per_gas": "1000000000",
        "timeout_seconds": 5,
        "max_scan_blocks": 1000,
    });
    if let Some(rate) = primary_rate {
        config["rpc_requests_per_second"] = json!(rate);
    }
    if let Some(rate) = peer_rate {
        config["peer_rpc_requests_per_second"] = json!(rate);
    }
    config
}

fn wallet_config(
    root: &Path,
    primary: &str,
    peer: &str,
    primary_rate: Option<u32>,
    peer_rate: Option<u32>,
) -> Value {
    let mut config = json!({
        "version": 1,
        "state_root": root.join("state"),
        "namespace": "eip155:10143",
        "pool": format!("0x{}", "22".repeat(20)),
        "verifier_version": 2,
        "asset": format!("0x{}", "33".repeat(20)),
        "rpc_url": primary,
        "peer_rpc_url": peer,
        "first_block": 1,
        "first_hash": hex32(),
        "wallet_file": root.join("wallet.enc"),
        "wallet_key_file": root.join("wallet.key"),
        "agreement_key_file": root.join("agreement.key"),
        "signer_address": format!("0x{}", "44".repeat(20)),
        "signer_journal_root": root.join("nonces"),
        "transaction_key_file": null,
        "artifact_manifest_file": root.join("manifest.json"),
        "artifact_manifest_sha256": hex32(),
        "artifact_cache_root": root.join("artifacts"),
        "runtime_pins": {
            "pool": hex32(), "poseidon": hex32(), "deposit": hex32(),
            "transfer": hex32(), "withdraw": hex32(),
        },
        "allow_test_artifacts": true,
        "gas_limit": 3_000_000,
        "max_fee_per_gas": "2000000000",
        "max_priority_fee_per_gas": "1000000000",
        "timeout_seconds": 5,
        "artifact_timeout_seconds": 5,
        "max_scan_blocks": 1000,
    });
    if let Some(rate) = primary_rate {
        config["rpc_requests_per_second"] = json!(rate);
    }
    if let Some(rate) = peer_rate {
        config["peer_rpc_requests_per_second"] = json!(rate);
    }
    config
}

fn write_config(root: &Path, name: &str, config: &Value) -> std::path::PathBuf {
    let path = root.join(name);
    write_private(&path, &serde_json::to_vec(config).unwrap());
    path
}

#[tokio::test]
async fn payment_binary_rejects_out_of_range_rates_before_any_request() {
    let root = tempfile::tempdir().unwrap();
    let primary = start_mock().await;
    let peer = start_mock().await;
    for (label, primary_rate, peer_rate) in [
        ("primary zero", Some(0), None),
        ("peer zero", None, Some(0)),
        ("primary over", Some(41), None),
        ("peer over", None, Some(41)),
        ("both over", Some(41), Some(41)),
    ] {
        let config = payment_config(
            root.path(),
            &primary.url,
            &peer.url,
            primary_rate,
            peer_rate,
        );
        let path = write_config(root.path(), "payment-rate.json", &config);
        let (code, stdout, _) = run(
            env!("CARGO_BIN_EXE_erebus-payment"),
            &json!({"method": "observe", "config_file": path, "operation_ref": "ab".repeat(32)}),
        );
        assert_ne!(code, 0, "{label}");
        assert!(
            stdout.contains("invalid shielded RPC read rate"),
            "{label}: {stdout}"
        );
        assert_eq!(primary.counters.reads.load(Ordering::SeqCst), 0, "{label}");
        assert_eq!(peer.counters.reads.load(Ordering::SeqCst), 0, "{label}");
        assert_eq!(primary.counters.sends.load(Ordering::SeqCst), 0, "{label}");
        assert_eq!(peer.counters.sends.load(Ordering::SeqCst), 0, "{label}");
    }
}

#[tokio::test]
async fn payment_binary_accepts_in_range_and_omitted_rates_past_validation() {
    let root = tempfile::tempdir().unwrap();
    for (label, primary_rate, peer_rate) in [
        ("primary one", Some(1), None),
        ("peer one", None, Some(1)),
        ("primary forty", Some(40), None),
        ("peer forty", None, Some(40)),
        ("omitted", None, None),
    ] {
        let primary = start_mock().await;
        let peer = start_mock().await;
        let config = payment_config(
            root.path(),
            &primary.url,
            &peer.url,
            primary_rate,
            peer_rate,
        );
        let path = write_config(root.path(), "payment-accept.json", &config);
        let (code, stdout, stderr) = run(
            env!("CARGO_BIN_EXE_erebus-payment"),
            &json!({"method": "observe", "config_file": path, "operation_ref": "ab".repeat(32)}),
        );
        assert!(
            !stdout.contains("invalid shielded RPC read rate"),
            "{label}: accepted rates must pass the rate gate: {stdout} {stderr}"
        );
        // Acceptance proves the config gate is passed (the failure is later state, not the
        // rate message). Reaching the read path needs funded coordinator/wallet state and is
        // recorded as an explicit scope gap in the handoff.
        assert_ne!(code, 0, "{label}: an empty state root cannot observe");
        assert_eq!(
            primary.counters.sends.load(Ordering::SeqCst),
            0,
            "{label}: observe never signs"
        );
        assert_eq!(peer.counters.sends.load(Ordering::SeqCst), 0, "{label}");
    }
}

#[tokio::test]
async fn wallet_binary_rate_gate_is_independent_and_precedes_any_request() {
    let root = tempfile::tempdir().unwrap();
    for (label, primary_rate, peer_rate, rejected) in [
        ("primary zero", Some(0), None, true),
        ("peer zero", None, Some(0), true),
        ("primary over", Some(41), None, true),
        ("peer over", None, Some(41), true),
        ("primary one", Some(1), None, false),
        ("peer forty", None, Some(40), false),
        ("omitted", None, None, false),
    ] {
        let primary = start_mock().await;
        let peer = start_mock().await;
        let config = wallet_config(
            root.path(),
            &primary.url,
            &peer.url,
            primary_rate,
            peer_rate,
        );
        let path = write_config(root.path(), "wallet-rate.json", &config);
        let (code, _, stderr) = run(
            env!("CARGO_BIN_EXE_erebus-wallet"),
            &json!({"method": "status", "config_file": path}),
        );
        assert_ne!(code, 0, "{label}");
        if rejected {
            assert!(
                stderr.contains("invalid config"),
                "{label}: out-of-range rates must fail config validation: {stderr}"
            );
            assert_eq!(primary.counters.reads.load(Ordering::SeqCst), 0, "{label}");
            assert_eq!(peer.counters.reads.load(Ordering::SeqCst), 0, "{label}");
        } else {
            // Acceptance proves the config gate is passed; reaching diagnostics reads needs a
            // funded wallet/manifest state and is recorded as an explicit scope gap.
            assert!(
                !stderr.contains("invalid config"),
                "{label}: in-range rates must pass config validation: {stderr}"
            );
        }
        assert_eq!(primary.counters.sends.load(Ordering::SeqCst), 0, "{label}");
        assert_eq!(peer.counters.sends.load(Ordering::SeqCst), 0, "{label}");
    }
}
