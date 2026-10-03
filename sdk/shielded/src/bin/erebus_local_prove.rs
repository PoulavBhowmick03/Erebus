//! A bounded, local-only proving command with automatic public artifact installation.

use std::{
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use erebus_shielded_prover::artifacts::{
    ArtifactRelease, CircuitKind, InstallOptions, MAX_MANIFEST_BYTES,
};
use serde::Deserialize;
use zeroize::Zeroizing;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    manifest_file: PathBuf,
    manifest_sha256: String,
    circuit: CircuitKind,
    cache_root: PathBuf,
    witness_file: PathBuf,
    expected_public: Vec<String>,
    #[serde(default)]
    allow_test_artifacts: bool,
    #[serde(default)]
    allow_loopback_http: bool,
}

#[tokio::main]
async fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "--version" {
        finish(Ok(
            serde_json::json!({"status":"ok","protocol":1,"methods":["prove"]}),
        ));
    }
    if args.len() == 1 && args[0] == "--help" {
        println!("erebus-local-prove: fetch hash-pinned public artifacts and generate a proof locally.\nOne JSON request on stdin: manifest_file, manifest_sha256, circuit, cache_root, witness_file, expected_public.\nThe witness must be an owner-only regular file. No private witness is uploaded.\nPrototype keys and loopback HTTP require explicit development opt-in. This command does not settle or sign.");
        return;
    }
    if !args.is_empty() {
        finish(Err("unexpected arguments"));
    }
    let mut bytes = Zeroizing::new(Vec::new());
    if std::io::stdin()
        .take(16_385)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > 16_384
    {
        finish(Err("invalid request size"));
    }
    let request = match serde_json::from_slice(&bytes) {
        Ok(request) => request,
        Err(_) => finish(Err("invalid request")),
    };
    finish(prove(request).await);
}

fn read_file(path: &Path, limit: usize, private: bool) -> Result<Zeroizing<Vec<u8>>, &'static str> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| "local input unavailable")?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return Err("local input unavailable");
    }
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("private witness must be owner-only");
        }
    }
    #[cfg(not(unix))]
    if private {
        return Err("private witness permissions are unsupported on this platform");
    }
    let file = std::fs::File::open(path).map_err(|_| "local input unavailable")?;
    let mut bytes = Zeroizing::new(Vec::new());
    file.take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "local input unavailable")?;
    if bytes.len() > limit {
        return Err("local input unavailable");
    }
    Ok(bytes)
}

async fn prove(request: Request) -> Result<serde_json::Value, &'static str> {
    let expected = hex::decode(&request.manifest_sha256)
        .map_err(|_| "invalid manifest digest")?
        .try_into()
        .map_err(|_| "invalid manifest digest")?;
    let bytes = read_file(&request.manifest_file, MAX_MANIFEST_BYTES, false)?;
    let release = ArtifactRelease::authenticate(&bytes, expected)
        .map_err(|_| "manifest not authenticated")?;
    let witness = read_file(&request.witness_file, 512 * 1024, true)?;
    let input = serde_json::from_slice(&witness).map_err(|_| "invalid local witness")?;
    let proof = release
        .prove(
            request.circuit,
            &InstallOptions {
                cache_root: request.cache_root,
                timeout: Duration::from_secs(120),
                allow_test_artifacts: request.allow_test_artifacts,
                allow_loopback_http: request.allow_loopback_http,
            },
            input,
            request.expected_public,
        )
        .await
        .map_err(|_| {
            "local proving failed; check trusted artifacts, expected transition, and local witness"
        })?;
    Ok(
        serde_json::json!({"status":"ok", "calldata":proof.proof.solidity_calldata(),
        "downloaded_bytes":proof.downloaded_bytes,"cache_hits":proof.cache_hits,
        "installation_ms":proof.installation_ms,"proving_ms":proof.proving_ms}),
    )
}

fn finish(result: Result<serde_json::Value, &'static str>) -> ! {
    use std::io::Write;
    let failed = result.is_err();
    let output = result.unwrap_or_else(|error| serde_json::json!({"status":"error","error":error}));
    println!("{output}");
    let flushed = std::io::stdout().flush().is_ok();
    std::process::exit(i32::from(failed || !flushed));
}
