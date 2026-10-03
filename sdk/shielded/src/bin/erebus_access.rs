//! Path-only local buyer authentication and durable content retrieval.

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use erebus_shielded_prover::access::client::{AccessClient, RetrievalError, RetrievalReceipt};
use erebus_transport::disclosure::{SelectedAgreement, MAX_DISCLOSURE_BYTES};
use serde::Deserialize;
use zeroize::Zeroizing;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    method: String,
    evidence_file: PathBuf,
    buyer_key_file: PathBuf,
    service_url: String,
    service_id: String,
    cache_root: PathBuf,
    #[serde(default)]
    allow_loopback_http: bool,
}

#[tokio::main]
async fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "--version" {
        println!(
            "{}",
            serde_json::json!({"status":"ok","protocol":1,"methods":["retrieve"]})
        );
        exit(0);
    }
    if args.len() == 1 && args[0] == "--help" {
        println!("erebus-access: authenticate with the buyer agreement key and retrieve one signed snapshot.\nOne JSON request: method=retrieve, evidence_file, buyer_key_file, service_url, service_id, cache_root.\nThe endpoint must be HTTPS ending in /v1/access; loopback HTTP requires explicit development opt-in.\nKeys remain in owner-only local files. Payloads are saved locally, not printed. No payment is signed or submitted.\nExit 2 means pending access. A content receipt is not independent payment or delivery evidence.");
        exit(0);
    }
    if !args.is_empty() {
        finish(Err(RetrievalError::Configuration));
    }
    let mut bytes = Zeroizing::new(Vec::new());
    if std::io::stdin()
        .take(16385)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > 16384
    {
        finish(Err(RetrievalError::Configuration));
    }
    let request = match serde_json::from_slice(&bytes) {
        Ok(request) => request,
        Err(_) => finish(Err(RetrievalError::Configuration)),
    };
    finish(retrieve(request).await);
}

async fn retrieve(request: Request) -> Result<RetrievalReceipt, RetrievalError> {
    if request.method != "retrieve"
        || request.service_id.len() != 64
        || !request
            .service_id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(RetrievalError::Configuration);
    }
    let service_id = hex::decode(request.service_id)
        .map_err(|_| RetrievalError::Configuration)?
        .try_into()
        .map_err(|_| RetrievalError::Configuration)?;
    let evidence_file = request.evidence_file;
    let key_file = request.buyer_key_file;
    let prepared = tokio::task::spawn_blocking(move || {
        let evidence =
            SelectedAgreement::decode(&private_read(&evidence_file, MAX_DISCLOSURE_BYTES)?)
                .map_err(|_| RetrievalError::Agreement)?;
        let client = AccessClient::open(
            &request.service_url,
            service_id,
            request.cache_root,
            request.allow_loopback_http,
        )?;
        if let Some(receipt) = client.cached(&evidence)? {
            return Ok((client, evidence, None, Some(receipt)));
        }
        let bytes = private_read(&key_file, 128)?;
        let seed: Zeroizing<[u8; 32]> = if bytes.len() == 32 {
            Zeroizing::new(
                bytes
                    .as_slice()
                    .try_into()
                    .map_err(|_| RetrievalError::Authentication)?,
            )
        } else {
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| RetrievalError::Authentication)?
                .trim();
            let decoded = Zeroizing::new(
                hex::decode(text.strip_prefix("0x").unwrap_or(text))
                    .map_err(|_| RetrievalError::Authentication)?,
            );
            Zeroizing::new(
                decoded
                    .as_slice()
                    .try_into()
                    .map_err(|_| RetrievalError::Authentication)?,
            )
        };
        Ok::<_, RetrievalError>((client, evidence, Some(seed), None))
    })
    .await
    .map_err(|_| RetrievalError::Storage)??;
    let (client, evidence, seed, cached) = prepared;
    if let Some(cached) = cached {
        return Ok(cached);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| RetrievalError::Authentication)?
        .as_secs();
    let seed = seed.ok_or(RetrievalError::Authentication)?;
    client.retrieve(&evidence, &seed, now).await
}

fn private_read(path: &Path, limit: usize) -> Result<Zeroizing<Vec<u8>>, RetrievalError> {
    let info = std::fs::symlink_metadata(path).map_err(|_| RetrievalError::Storage)?;
    if !info.is_file() || info.len() > limit as u64 {
        return Err(RetrievalError::Storage);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if info.permissions().mode() & 0o077 != 0 {
            return Err(RetrievalError::Storage);
        }
    }
    #[cfg(not(unix))]
    return Err(RetrievalError::Configuration);
    let mut bytes = Zeroizing::new(Vec::new());
    std::fs::File::open(path)
        .map_err(|_| RetrievalError::Storage)?
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| RetrievalError::Storage)?;
    if bytes.len() > limit {
        return Err(RetrievalError::Storage);
    }
    Ok(bytes)
}

fn finish(result: Result<RetrievalReceipt, RetrievalError>) -> ! {
    let (code, output) = match result {
        Ok(receipt) => (
            0,
            serde_json::json!({"status":"retrieved","result":receipt}),
        ),
        Err(RetrievalError::Pending) => (
            2,
            serde_json::json!({"status":"pending","retry_without_payment":true,"payment_verified":false,"resource_verified":false,"delivery_verified":false}),
        ),
        Err(RetrievalError::SellerReportedUndelivered) => (
            2,
            serde_json::json!({"status":"paid_but_undelivered","seller_reported_payment_finalized":true,"retry_without_payment":true,"payment_verified":false,"resource_verified":false,"delivery_verified":false}),
        ),
        Err(error) => (
            1,
            serde_json::json!({"status":"error","error":error.to_string(),"retry_without_payment":true}),
        ),
    };
    println!("{output}");
    exit(code);
}

fn exit(code: i32) -> ! {
    let flushed = std::io::stdout().flush().is_ok();
    std::process::exit(if flushed { code } else { 1 });
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn private_inputs_require_bounded_regular_owner_only_files() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("input");
        std::fs::write(&file, [7; 32]).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(private_read(&file, 32).unwrap().len(), 32);
        assert!(matches!(
            private_read(&file, 31),
            Err(RetrievalError::Storage)
        ));
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            private_read(&file, 128),
            Err(RetrievalError::Storage)
        ));
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(matches!(
            private_read(&link, 128),
            Err(RetrievalError::Storage)
        ));
        assert!(matches!(
            private_read(root.path(), 128),
            Err(RetrievalError::Storage)
        ));
    }
}
