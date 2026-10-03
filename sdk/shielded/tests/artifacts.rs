//! Public artifact downloads never send private witnesses and publish only verified bytes.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use erebus_shielded_prover::artifacts::{
    ArtifactRelease, CircuitKind, InstallError, InstallOptions, MAX_ARTIFACT_BYTES,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

struct Files {
    bodies: HashMap<String, Vec<u8>>,
    calls: AtomicUsize,
}

struct Server {
    url: String,
    state: Arc<Files>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(Path(name): Path<String>, State(files): State<Arc<Files>>) -> Response {
    files.calls.fetch_add(1, Ordering::SeqCst);
    let (status, body) = match files.bodies.get(&name) {
        Some(body) => (StatusCode::OK, body.clone()),
        None => (StatusCode::NOT_FOUND, b"provider-private-error".to_vec()),
    };
    // Exercise the streaming byte bound without a trustworthy Content-Length header.
    let chunks: Vec<Result<axum::body::Bytes, std::convert::Infallible>> = body
        .chunks(65536)
        .map(|chunk| Ok(axum::body::Bytes::copy_from_slice(chunk)))
        .collect();
    (
        status,
        axum::body::Body::from_stream(tokio_stream::iter(chunks)),
    )
        .into_response()
}

async fn server(bodies: HashMap<String, Vec<u8>>) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let state = Arc::new(Files {
        bodies,
        calls: AtomicUsize::new(0),
    });
    let router = Router::new()
        .route("/{name}", get(serve))
        .with_state(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Server { url, state, task }
}

fn bodies() -> HashMap<String, Vec<u8>> {
    ["wasm", "r1cs", "zkey"]
        .into_iter()
        .map(|name| (name.into(), format!("public-{name}").into_bytes()))
        .collect()
}

fn manifest(url: &str) -> Value {
    let files = bodies();
    let file = |name: &str| json!({"url":format!("{url}/{name}"), "bytes":files[name].len(), "sha256":hex::encode(Sha256::digest(&files[name]))});
    json!({
        "version":1, "chain_id":10143, "settlement_contract":format!("0x{}", "01".repeat(20)),
        "verifier_version":1, "test_only":true,
        "circuits":(["deposit", "transfer", "withdraw"].map(|kind| json!({"kind":kind,"wasm":file("wasm"),"r1cs":file("r1cs"),"zkey":file("zkey")})))
    })
}

fn authenticate(value: &Value) -> Result<ArtifactRelease, InstallError> {
    let bytes = serde_json::to_vec(value).unwrap();
    ArtifactRelease::authenticate(&bytes, Sha256::digest(&bytes).into())
}

fn options(root: &std::path::Path) -> InstallOptions {
    InstallOptions {
        cache_root: root.join("cache"),
        timeout: Duration::from_secs(5),
        allow_test_artifacts: true,
        allow_loopback_http: true,
    }
}

#[test]
fn manifest_authentication_and_deployment_binding_fail_closed() {
    let value = manifest("https://artifacts.invalid");
    let bytes = serde_json::to_vec(&value).unwrap();
    assert!(matches!(
        ArtifactRelease::authenticate(&bytes, [0; 32]),
        Err(InstallError::Manifest)
    ));
    let release = authenticate(&value).unwrap();
    assert!(release.matches_deployment(10143, [1; 20], 1));
    assert!(!release.matches_deployment(1, [1; 20], 1));
    for (pointer, replacement) in [
        ("/version", json!(2)),
        ("/chain_id", json!(0)),
        ("/circuits/1/kind", json!("deposit")),
        ("/circuits/0/wasm/bytes", json!(0)),
        ("/circuits/0/wasm/bytes", json!(MAX_ARTIFACT_BYTES + 1)),
        ("/circuits/0/wasm/sha256", json!("AB".repeat(32))),
        (
            "/settlement_contract",
            json!(format!("0x{}", "00".repeat(20))),
        ),
    ] {
        let mut bad = value.clone();
        *bad.pointer_mut(pointer).unwrap() = replacement;
        assert!(
            matches!(authenticate(&bad), Err(InstallError::Manifest)),
            "{pointer}"
        );
    }
}

#[tokio::test]
async fn downloaded_files_are_rechecked_cached_and_repaired() {
    let server = server(bodies()).await;
    let release = authenticate(&manifest(&server.url)).unwrap();
    let root = tempfile::tempdir().unwrap();
    let config = options(root.path());
    let first = release
        .install(CircuitKind::Transfer, &config)
        .await
        .unwrap();
    assert_eq!(first.cache_hits, 0);
    assert_eq!(
        first.downloaded_bytes,
        bodies().values().map(|body| body.len() as u64).sum::<u64>()
    );
    first.artifacts.verify().unwrap();
    let second = release
        .install(CircuitKind::Transfer, &config)
        .await
        .unwrap();
    assert_eq!((second.cache_hits, second.downloaded_bytes), (3, 0));
    assert_eq!(server.state.calls.load(Ordering::SeqCst), 3);
    std::fs::write(&second.artifacts.wasm, b"corrupt").unwrap();
    let repaired = release
        .install(CircuitKind::Transfer, &config)
        .await
        .unwrap();
    assert_eq!(repaired.cache_hits, 2);
    assert_eq!(server.state.calls.load(Ordering::SeqCst), 4);
    repaired.artifacts.verify().unwrap();
}

#[tokio::test]
async fn development_exceptions_are_explicit_and_never_contact_disallowed_urls() {
    let root = tempfile::tempdir().unwrap();
    let mut config = options(root.path());
    let release = authenticate(&manifest("http://127.0.0.1:9")).unwrap();
    config.allow_test_artifacts = false;
    assert!(matches!(
        release.install(CircuitKind::Transfer, &config).await,
        Err(InstallError::TestOnly)
    ));
    config.allow_test_artifacts = true;
    config.allow_loopback_http = false;
    assert!(matches!(
        release.install(CircuitKind::Transfer, &config).await,
        Err(InstallError::Configuration)
    ));
    config.allow_loopback_http = true;
    for url in [
        "http://example.invalid",
        "https://user:secret@example.invalid",
        "https://example.invalid/#secret",
    ] {
        let release = authenticate(&manifest(url)).unwrap();
        assert!(matches!(
            release.install(CircuitKind::Transfer, &config).await,
            Err(InstallError::Configuration)
        ));
    }
    assert!(!config.cache_root.exists());
}

#[tokio::test]
async fn wrong_bytes_and_truncation_never_publish_an_artifact() {
    for body in [b"public-WASM".to_vec(), b"short".to_vec(), vec![0; 64]] {
        let mut files = bodies();
        files.insert("wasm".into(), body);
        let server = server(files).await;
        let release = authenticate(&manifest(&server.url)).unwrap();
        let root = tempfile::tempdir().unwrap();
        let config = options(root.path());
        assert!(matches!(
            release.install(CircuitKind::Transfer, &config).await,
            Err(InstallError::Integrity)
        ));
        assert!(std::fs::read_dir(&config.cache_root)
            .unwrap()
            .all(|entry| entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|ext| ext == "lock")));
    }
}

#[tokio::test]
async fn concurrent_installers_serialize_and_reuse_verified_files() {
    let server = server(bodies()).await;
    let release = Arc::new(authenticate(&manifest(&server.url)).unwrap());
    let root = tempfile::tempdir().unwrap();
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let release = release.clone();
        let config = options(root.path());
        tasks.push(tokio::spawn(async move {
            release
                .install(CircuitKind::Transfer, &config)
                .await
                .unwrap()
        }));
    }
    let mut downloaded = 0;
    for task in tasks {
        downloaded += task.await.unwrap().downloaded_bytes;
    }
    assert_eq!(
        downloaded,
        bodies().values().map(|body| body.len() as u64).sum::<u64>()
    );
    assert_eq!(server.state.calls.load(Ordering::SeqCst), 3);
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_cache_and_lock_are_rejected() {
    use std::os::unix::fs::symlink;
    let server = server(bodies()).await;
    let release = authenticate(&manifest(&server.url)).unwrap();
    for extension in ["artifact", "lock"] {
        let root = tempfile::tempdir().unwrap();
        let config = options(root.path());
        std::fs::create_dir(&config.cache_root).unwrap();
        let outside = root.path().join("outside");
        std::fs::write(&outside, b"untouched").unwrap();
        let digest = hex::encode(Sha256::digest(&bodies()["wasm"]));
        symlink(
            &outside,
            config.cache_root.join(format!("{digest}.{extension}")),
        )
        .unwrap();
        assert!(matches!(
            release.install(CircuitKind::Transfer, &config).await,
            Err(InstallError::Cache)
        ));
        assert_eq!(std::fs::read(outside).unwrap(), b"untouched");
    }
    assert_eq!(server.state.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn installed_binary_returns_public_paths_and_reuses_cache() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let server = server(bodies()).await;
    let root = tempfile::tempdir().unwrap();
    let bytes = serde_json::to_vec(&manifest(&server.url)).unwrap();
    let path = root.path().join("manifest.json");
    std::fs::write(&path, &bytes).unwrap();
    let request = json!({"manifest_file":path,"manifest_sha256":hex::encode(Sha256::digest(&bytes)),"circuit":"transfer","cache_root":root.path().join("cache"),"allow_test_artifacts":true,"allow_loopback_http":true});
    for expected_hits in [0, 3] {
        let request = request.clone();
        let output = tokio::task::spawn_blocking(move || {
            let mut child = Command::new(env!("CARGO_BIN_EXE_erebus-artifacts"))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(request.to_string().as_bytes())
                .unwrap();
            child.wait_with_output().unwrap()
        })
        .await
        .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["cache_hits"], expected_hits);
        assert_eq!(response["status"], "ok");
        assert!(output.stderr.is_empty());
    }
    assert_eq!(server.state.calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn mismatched_proving_domain_fails_before_installation() {
    let root = tempfile::tempdir().unwrap();
    let config = options(root.path());
    let release = authenticate(&manifest("http://127.0.0.1:9")).unwrap();
    for expected in [vec![], vec!["1".into(); 11]] {
        let result = release
            .prove(
                CircuitKind::Transfer,
                &config,
                json!({"secret":"not uploaded"}),
                expected,
            )
            .await;
        assert!(matches!(
            result,
            Err(
                erebus_shielded_prover::artifacts::InstalledProvingError::Proving(
                    erebus_shielded_prover::ProverError::PublicInputMismatch
                )
            )
        ));
    }
    assert!(!config.cache_root.exists());
}

#[tokio::test]
#[ignore = "requires generated M5 prototype artifacts; known test setup, not a release"]
async fn fresh_process_downloads_real_artifacts_and_proves_locally() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let build =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../circuits/m5/build");
    let mut files = HashMap::new();
    for kind in ["deposit", "transfer", "withdraw"] {
        for suffix in ["wasm", "r1cs", "zkey"] {
            let path = if suffix == "wasm" {
                build.join(format!("{kind}_js/{kind}.wasm"))
            } else {
                build.join(format!("{kind}.{suffix}"))
            };
            files.insert(format!("{kind}.{suffix}"), std::fs::read(path).unwrap());
        }
    }
    let server = server(files).await;
    let mut value = manifest(&server.url);
    let input: Value =
        serde_json::from_slice(&std::fs::read(build.join("deposit-input.json")).unwrap()).unwrap();
    value["chain_id"] = json!(input["chainId"].as_str().unwrap().parse::<u64>().unwrap());
    value["settlement_contract"] = json!(format!(
        "0x{:040x}",
        input["contractAddress"]
            .as_str()
            .unwrap()
            .parse::<num_bigint::BigUint>()
            .unwrap()
    ));
    value["verifier_version"] = json!(input["verifierVersion"]
        .as_str()
        .unwrap()
        .parse::<u32>()
        .unwrap());
    for circuit in value["circuits"].as_array_mut().unwrap() {
        let kind = circuit["kind"].as_str().unwrap().to_owned();
        for suffix in ["wasm", "r1cs", "zkey"] {
            let name = format!("{kind}.{suffix}");
            let body = &server.state.bodies[&name];
            circuit[suffix] = json!({"url":format!("{}/{name}",server.url),"sha256":hex::encode(Sha256::digest(body)),"bytes":body.len()});
        }
    }
    let root = tempfile::tempdir().unwrap();
    let manifest_path = root.path().join("manifest.json");
    let bytes = serde_json::to_vec(&value).unwrap();
    std::fs::write(&manifest_path, &bytes).unwrap();
    for (kind, names) in [
        (
            "deposit",
            vec![
                "chainId",
                "contractAddress",
                "verifierVersion",
                "asset",
                "amount",
                "noteCommitment",
            ],
        ),
        (
            "transfer",
            vec![
                "chainId",
                "contractAddress",
                "verifierVersion",
                "asset",
                "dealCommitment",
                "dealNullifier",
                "root",
                "inputNullifier",
                "paymentCommitment",
                "changeCommitment",
                "expiry",
            ],
        ),
        (
            "withdraw",
            vec![
                "chainId",
                "contractAddress",
                "verifierVersion",
                "asset",
                "root",
                "noteNullifier",
                "recipient",
                "amount",
            ],
        ),
    ] {
        let witness_path = root.path().join(format!("{kind}.json"));
        let witness = std::fs::read(build.join(format!("{kind}-input.json"))).unwrap();
        std::fs::write(&witness_path, &witness).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&witness_path, std::fs::Permissions::from_mode(0o600))
                .unwrap();
        }
        let input: Value = serde_json::from_slice(&witness).unwrap();
        let expected: Vec<_> = names
            .iter()
            .map(|name| input[*name].as_str().unwrap())
            .collect();
        let request = json!({"manifest_file":manifest_path,"manifest_sha256":hex::encode(Sha256::digest(&bytes)),"circuit":kind,"cache_root":root.path().join("cache"),"witness_file":witness_path,"expected_public":expected,"allow_test_artifacts":true,"allow_loopback_http":true});
        let output = tokio::task::spawn_blocking(move || {
            let mut child = Command::new(env!("CARGO_BIN_EXE_erebus-local-prove"))
                .env_clear()
                .current_dir("/")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(request.to_string().as_bytes())
                .unwrap();
            child.wait_with_output().unwrap()
        })
        .await
        .unwrap();
        assert!(
            output.status.success(),
            "{kind}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["status"], "ok");
        assert!(response["downloaded_bytes"].as_u64().unwrap() > 0);
        assert_eq!(response["calldata"][3], json!(expected));
        assert_eq!(response["cache_hits"], 0);
        assert!(output.stderr.is_empty());
        println!(
            "{kind}: download={} bytes, install={} ms, prove={} ms",
            response["downloaded_bytes"], response["installation_ms"], response["proving_ms"]
        );
    }
    assert_eq!(server.state.calls.load(Ordering::SeqCst), 9);
}
