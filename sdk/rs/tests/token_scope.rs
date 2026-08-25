//! Funding checks follow the asset an operation moves, not the client's configured token.
//!
//! `ClientConfig::token` names the token a *new* channel opens on. It is not the token every
//! operation touches: a channel opened by an earlier run with a different configuration, or
//! one recovered by `rebuild_state`, carries its own.
//!
//! Before this, `accept_and_settle` read the allowance and public balance of
//! `ClientConfig::token` and then spent notes of the channel's token. Where the two differ,
//! the settlement passed or failed its funding check against the wrong asset entirely.
//!
//! The mock node records the contract address each call targets, which is the only way to
//! see which token was actually asked about.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

use erebus_sdk::client::{Client, ClientConfig};
use erebus_sdk::wire::WireVersion;
use serde_json::{json, Value};
use starknet_types_core::felt::Felt;

const ACCOUNT: Felt = Felt::from_hex_unchecked("0x11");
const POOL: Felt = Felt::from_hex_unchecked("0x22");
const CHAIN: Felt = Felt::from_hex_unchecked("0x33");
const CONFIGURED_TOKEN: Felt = Felt::from_hex_unchecked("0xaaa");
const OTHER_TOKEN: Felt = Felt::from_hex_unchecked("0xbbb");

/// Records the `contract_address` of every `starknet_call`, so a test can see which token
/// a read was actually aimed at.
fn recording_server(
    responses: Vec<Value>,
) -> (String, Arc<Mutex<Vec<String>>>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&seen);
    let handle = thread::spawn(move || {
        for response in responses {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            if let Some(body) = read_body(&mut stream) {
                if let Ok(value) = serde_json::from_str::<Value>(&body) {
                    let target = value["params"]["request"]["contract_address"]
                        .as_str()
                        .or_else(|| value["params"][0]["contract_address"].as_str())
                        .unwrap_or("?")
                        .to_owned();
                    recorder.lock().expect("recorder").push(target);
                }
            }
            let body = response.to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
        }
    });
    (format!("http://{address}"), seen, handle)
}

fn read_body(stream: &mut TcpStream) -> Option<String> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let read = stream.read(&mut buffer).ok()?;
        if read == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..read]);
        let end = match bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            Some(end) => end,
            None => continue,
        };
        let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
        let length: usize = headers
            .lines()
            .find_map(|line| {
                line.strip_prefix("content-length:")
                    .map(str::trim)
                    .map(str::to_owned)
            })?
            .parse()
            .ok()?;
        let start = end + 4;
        while bytes.len() < start + length {
            let read = stream.read(&mut buffer).ok()?;
            if read == 0 {
                return None;
            }
            bytes.extend_from_slice(&buffer[..read]);
        }
        return Some(String::from_utf8_lossy(&bytes[start..start + length]).into_owned());
    }
}

fn client(rpc_url: String) -> (Client, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "erebus-token-scope-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    std::fs::create_dir_all(&root).expect("root");
    std::fs::write(root.join("pool.key"), "0xabc\n").expect("pool key");
    std::fs::write(root.join("account.key"), "0xdef\n").expect("account key");
    let client = Client::new(ClientConfig {
        rpc_url,
        prover_url: "http://127.0.0.1:9".to_owned(),
        pool_address: POOL,
        chain_id: CHAIN,
        account_address: ACCOUNT,
        pool_key_file: root.join("pool.key"),
        account_key_file: root.join("account.key"),
        state_dir: root.clone(),
        token: CONFIGURED_TOKEN,
        new_channel_wire_version: WireVersion::V3,
    })
    .expect("client");
    (client, root)
}

fn hex(felt: Felt) -> String {
    format!("{felt:#x}")
}

/// An allowance is read against the token it was asked about.
///
/// If the token argument were ignored, both reads would target the configured token and the
/// two recorded addresses would be identical.
#[tokio::test]
async fn an_allowance_is_read_against_the_token_it_was_asked_about() {
    let (rpc_url, seen, server) = recording_server(vec![
        json!({"jsonrpc":"2.0","id":1,"result":["0x64","0x0"]}),
        json!({"jsonrpc":"2.0","id":1,"result":["0x2"]}),
        json!({"jsonrpc":"2.0","id":1,"result":["0xc8","0x0"]}),
        json!({"jsonrpc":"2.0","id":1,"result":["0x2"]}),
    ]);
    let (client, root) = client(rpc_url);

    let configured = client
        .pool_allowance_for(CONFIGURED_TOKEN)
        .await
        .expect("configured token");
    let other = client
        .pool_allowance_for(OTHER_TOKEN)
        .await
        .expect("other token");
    drop(server);

    assert_eq!(configured.allowance, 0x64);
    assert_eq!(other.allowance, 0xc8);

    let targets = seen.lock().expect("recorder").clone();
    assert!(
        targets.contains(&hex(CONFIGURED_TOKEN)),
        "the configured token was never read: {targets:?}"
    );
    assert!(
        targets.contains(&hex(OTHER_TOKEN)),
        "the token argument was ignored, so a channel on another token would be checked \
         against the wrong asset: {targets:?}"
    );

    std::fs::remove_dir_all(&root).ok();
}

/// The no-argument form still means "this client's configured token".
///
/// Kept so the CLI and existing callers keep their meaning; the token-scoped form is the
/// one channel operations use.
#[tokio::test]
async fn the_default_allowance_still_uses_the_configured_token() {
    let (rpc_url, seen, server) = recording_server(vec![
        json!({"jsonrpc":"2.0","id":1,"result":["0x64","0x0"]}),
        json!({"jsonrpc":"2.0","id":1,"result":["0x2"]}),
    ]);
    let (client, root) = client(rpc_url);

    client.pool_allowance().await.expect("allowance");
    drop(server);

    let targets = seen.lock().expect("recorder").clone();
    assert!(targets.contains(&hex(CONFIGURED_TOKEN)), "{targets:?}");
    assert!(!targets.contains(&hex(OTHER_TOKEN)), "{targets:?}");

    std::fs::remove_dir_all(&root).ok();
}

// --- account signer ----------------------------------------------------------------------

/// A signer whose address disagrees with the client is refused before any proof is paid for.
///
/// The pool validates a signature against the account contract at `ClientConfig::account_address`
/// (`utils.cairo:383`), so a mismatched signer produces a signature the chain rejects — after
/// roughly thirty seconds of proving and a fee. Catching it at injection makes it free.
#[tokio::test]
async fn a_signer_for_the_wrong_account_is_refused_at_injection() {
    use erebus_sdk::signer::LocalKeySigner;
    use std::sync::Arc;

    let (rpc_url, _seen, server) = recording_server(vec![]);
    let (client, root) = client(rpc_url);
    drop(server);

    let wrong = Arc::new(LocalKeySigner::new(
        Felt::from_hex_unchecked("0x999"),
        root.join("account.key"),
    ));
    let error = client
        .with_signer(wrong)
        .expect_err("a signer for another account must be refused");

    let message = error.to_string();
    assert!(message.contains("0x999"), "{message}");
    assert!(message.contains("0x11"), "{message}");

    std::fs::remove_dir_all(&root).ok();
}

/// A signer for the configured account is accepted, so the injection point is usable.
///
/// Without this the refusal test above would pass just as well if `with_signer` rejected
/// everything.
#[tokio::test]
async fn a_signer_for_the_configured_account_is_accepted() {
    use erebus_sdk::signer::LocalKeySigner;
    use std::sync::Arc;

    let (rpc_url, _seen, server) = recording_server(vec![]);
    let (client, root) = client(rpc_url);
    drop(server);

    let right = Arc::new(LocalKeySigner::new(ACCOUNT, root.join("account.key")));

    assert!(client.with_signer(right).is_ok());

    std::fs::remove_dir_all(&root).ok();
}
