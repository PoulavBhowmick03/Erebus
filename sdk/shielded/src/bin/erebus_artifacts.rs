//! Install independently hash-authenticated public proving artifacts, never private witnesses.

use std::{io::Read, path::PathBuf, time::Duration};

use erebus_shielded_prover::artifacts::{
    ArtifactRelease, CircuitKind, InstallOptions, MAX_MANIFEST_BYTES,
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    manifest_file: PathBuf,
    manifest_sha256: String,
    circuit: CircuitKind,
    cache_root: PathBuf,
    #[serde(default)]
    allow_test_artifacts: bool,
    #[serde(default)]
    allow_loopback_http: bool,
}

#[tokio::main]
async fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "--help" {
        println!("erebus-artifacts: install public artifacts from an independently hash-pinned manifest.\nOne JSON request on stdin: manifest_file, manifest_sha256, circuit, cache_root.\nPrototype keys and loopback HTTP require explicit development opt-in. No witnesses or wallet keys are read.");
        return;
    }
    if !args.is_empty() {
        finish(Err("unexpected arguments"));
    }
    let mut bytes = Vec::new();
    if std::io::stdin()
        .take(16_385)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > 16_384
    {
        finish(Err("invalid request size"));
    }
    let request: Request = match serde_json::from_slice(&bytes) {
        Ok(request) => request,
        Err(_) => finish(Err("invalid request")),
    };
    finish(install(request).await);
}

async fn install(request: Request) -> Result<serde_json::Value, &'static str> {
    let expected = hex::decode(&request.manifest_sha256)
        .map_err(|_| "invalid manifest digest")?
        .try_into()
        .map_err(|_| "invalid manifest digest")?;
    let metadata =
        std::fs::symlink_metadata(&request.manifest_file).map_err(|_| "manifest unavailable")?;
    if !metadata.is_file() || metadata.len() > MAX_MANIFEST_BYTES as u64 {
        return Err("manifest unavailable");
    }
    let file = std::fs::File::open(request.manifest_file).map_err(|_| "manifest unavailable")?;
    let mut bytes = Vec::new();
    file.take((MAX_MANIFEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "manifest unavailable")?;
    let release = ArtifactRelease::authenticate(&bytes, expected)
        .map_err(|_| "manifest not authenticated")?;
    let installed = release
        .install(
            request.circuit,
            &InstallOptions {
                cache_root: request.cache_root,
                timeout: Duration::from_secs(120),
                allow_test_artifacts: request.allow_test_artifacts,
                allow_loopback_http: request.allow_loopback_http,
            },
        )
        .await
        .map_err(|_| {
            "artifact installation failed; check trusted manifest, development opt-ins, and cache"
        })?;
    Ok(serde_json::json!({
        "status": "ok", "downloaded_bytes": installed.downloaded_bytes, "cache_hits": installed.cache_hits,
        "wasm": installed.artifacts.wasm, "r1cs": installed.artifacts.r1cs, "zkey": installed.artifacts.zkey,
        "wasm_sha256": installed.artifacts.wasm_sha256, "r1cs_sha256": installed.artifacts.r1cs_sha256,
        "zkey_sha256": installed.artifacts.zkey_sha256,
    }))
}

fn finish(result: Result<serde_json::Value, &'static str>) -> ! {
    use std::io::Write;
    let failed = result.is_err();
    let output =
        result.unwrap_or_else(|error| serde_json::json!({"status": "error", "error": error}));
    println!("{output}");
    let flushed = std::io::stdout().flush().is_ok();
    std::process::exit(i32::from(failed || !flushed));
}
