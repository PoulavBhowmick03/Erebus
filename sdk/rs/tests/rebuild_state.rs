//! Rebuilding channel records from the pool key and chain data.
//!
//! The state directory is a local convenience. Everything in a `StoredChannel` except its
//! handle is either derived from the pool key or written on chain, so losing the directory
//! should cost an operator a rebuild rather than the channels themselves.
//!
//! The chain is a scripted JSON-RPC fixture. What these assert is the contract around the
//! rebuild — additive, keyed, honest about what it could not recover — rather than the pool's
//! own behaviour, which `channel_ops` and the conformance vectors already pin.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use erebus_sdk::client::{Client, ClientConfig};
use erebus_sdk::state::{StateStore, StoredChannel};
use erebus_sdk::wire::WireVersion;
use serde_json::{json, Value};
use starknet_types_core::felt::Felt;

const ACCOUNT: Felt = Felt::from_hex_unchecked("0x11");
const POOL: Felt = Felt::from_hex_unchecked("0x22");
const CHAIN: Felt = Felt::from_hex_unchecked("0x33");
const TOKEN: Felt = Felt::from_hex_unchecked("0x44");

fn server(responses: Vec<Value>) -> (String, Arc<Mutex<Vec<String>>>, thread::JoinHandle<()>) {
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
                if let Some(method) = serde_json::from_str::<Value>(&body)
                    .ok()
                    .and_then(|value| value["method"].as_str().map(str::to_owned))
                {
                    recorder.lock().expect("recorder").push(method);
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

fn root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "erebus-rebuild-{label}-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    std::fs::create_dir_all(&root).expect("root");
    std::fs::write(root.join("pool.key"), "0xabc\n").expect("pool key");
    std::fs::write(root.join("account.key"), "0xdef\n").expect("account key");
    root
}

fn client(root: &Path, rpc_url: String) -> Client {
    Client::new(ClientConfig {
        rpc_url,
        prover_url: "http://127.0.0.1:9".to_owned(),
        pool_address: POOL,
        chain_id: CHAIN,
        account_address: ACCOUNT,
        pool_key_file: root.join("pool.key"),
        account_key_file: root.join("account.key"),
        state_dir: root.to_path_buf(),
        token: TOKEN,
        new_channel_wire_version: WireVersion::V3,
    })
    .expect("client")
}

/// An empty pool means an empty rebuild, and no invention.
///
/// The first outgoing-channel slot reads zero, so enumeration stops immediately. A rebuild
/// that produced a record here would be manufacturing a relationship from nothing.
#[tokio::test]
async fn nothing_on_chain_rebuilds_nothing() {
    let root = root("empty");
    let (rpc_url, seen, server) =
        server(vec![json!({"jsonrpc":"2.0","id":1,"result":["0x0","0x0"]})]);

    let report = client(&root, rpc_url)
        .rebuild_state()
        .await
        .expect("rebuild");
    drop(server);

    assert_eq!(report.channels_found, 0);
    assert!(report.rebuilt.is_empty());
    assert_eq!(report.kept, 0);
    assert!(
        !seen.lock().expect("recorder").is_empty(),
        "the rebuild should have asked the chain something"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// A rebuild never touches a record that is already there.
///
/// This is the property that makes it safe to run at any time. A live record knows things a
/// rebuild cannot recover — its original handle, its opened transaction — so overwriting one
/// would make an operator strictly worse off than not running the command.
#[tokio::test]
async fn an_existing_channel_is_kept_untouched_rather_than_rebuilt() {
    let root = root("kept");
    let counterparty = Felt::from(0x55u8);

    let state = StateStore::new(&root).expect("state");
    let handle = state
        .create(|handle| {
            StoredChannel::new_with_wire_version(
                handle,
                CHAIN,
                POOL,
                ACCOUNT,
                counterparty,
                Felt::from(0x66u8),
                TOKEN,
                Felt::from(0x77u8),
                0,
                0,
                Felt::from_hex_unchecked("0xdead"),
                42,
                WireVersion::V3,
            )
        })
        .expect("existing channel");
    let before = state.snapshot(&handle).expect("read").expect("present");

    // The chain says there are no outgoing channels, so the rebuild finds nothing to add.
    let (rpc_url, _seen, server) =
        server(vec![json!({"jsonrpc":"2.0","id":1,"result":["0x0","0x0"]})]);
    let report = client(&root, rpc_url)
        .rebuild_state()
        .await
        .expect("rebuild");
    drop(server);

    assert!(report.rebuilt.is_empty());

    let after = state
        .snapshot(&handle)
        .expect("read")
        .expect("still present");
    assert_eq!(
        after, before,
        "an existing record must survive a rebuild byte for byte"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// Enumeration is keyed, never scanned.
///
/// Every chain read a rebuild makes is a view call against an id derived from the pool key.
/// A rebuild that reached for events or global storage would violate CLAUDE.md constraint 3,
/// and would also not work: the ids are only computable by the key holder.
#[tokio::test]
async fn the_rebuild_only_makes_keyed_view_calls() {
    let root = root("keyed");
    let (rpc_url, seen, server) =
        server(vec![json!({"jsonrpc":"2.0","id":1,"result":["0x0","0x0"]})]);

    let _ = client(&root, rpc_url).rebuild_state().await;
    drop(server);

    let methods = seen.lock().expect("recorder").clone();
    assert!(!methods.is_empty());
    for method in &methods {
        assert_eq!(
            method, "starknet_call",
            "a rebuild must read only through keyed view calls, saw {method}"
        );
    }
    std::fs::remove_dir_all(&root).ok();
}

/// The report tells an operator what it could not recover, not just what it did.
///
/// A rebuild that reported only successes would let someone believe the directory is whole
/// when a deregistered counterparty or a second token left part of it behind.
#[test]
fn the_report_defaults_to_having_recovered_nothing() {
    let report = erebus_sdk::client::RebuildReport::default();

    assert_eq!(report.channels_found, 0);
    assert!(report.rebuilt.is_empty());
    assert_eq!(report.kept, 0);
    assert_eq!(report.other_token, 0);
    assert_eq!(report.unrecoverable, 0);

    let json = serde_json::to_value(&report).expect("serializes");
    for field in [
        "channels_found",
        "rebuilt",
        "kept",
        "other_token",
        "unrecoverable",
    ] {
        assert!(json.get(field).is_some(), "{field} missing from the report");
    }
}

/// The path that matters: a channel on chain and nothing locally is recovered in full.
///
/// The fixture is built with the SDK's own hash functions rather than hand-written hex,
/// because the point is that a rebuild reverses exactly what an open wrote. Hard-coding the
/// encrypted values would let the test drift from the derivation it is supposed to invert.
///
/// The counterparty address is *encrypted to the pool key* on chain. Recovering it is what
/// makes the rebuild keyed rather than a scan: nobody without that key can read this slot.
#[tokio::test]
async fn a_channel_on_chain_is_rebuilt_with_its_key_and_cursor() {
    use erebus_sdk::{decrypt, hashes};

    let root = root("happy");
    // Matches the pool.key written by `root`, which is what the client will load.
    let pool_key = Felt::from_hex_unchecked("0xabc");
    let counterparty = Felt::from(0x55u8);
    let counterparty_public_key = Felt::from(0x66u8);
    let channel_salt = Felt::from(0x1234u32);
    let subchannel_salt = Felt::from(0x5678u32);

    // The chain's outgoing-channel slot: a salt, and the recipient masked under the pool key.
    let enc_recipient =
        hashes::compute_enc_recipient_addr_hash(ACCOUNT, pool_key, 0, channel_salt) + counterparty;
    let channel_key =
        hashes::compute_channel_key(ACCOUNT, pool_key, counterparty, counterparty_public_key);
    // The subchannel slot, masked under the channel key rather than the pool key.
    let enc_token = hashes::compute_enc_token_hash(channel_key, 0, subchannel_salt) + TOKEN;

    // Sanity: the fixture really does invert. If this fails the test is wrong, not the code.
    assert_eq!(
        decrypt::outgoing_recipient_addr(enc_recipient, ACCOUNT, &pool_key, 0, channel_salt),
        counterparty
    );
    assert_eq!(
        decrypt::subchannel_token(enc_token, subchannel_salt, channel_key, 0),
        TOKEN
    );

    let responses = vec![
        // index 0: the channel exists
        json!({"jsonrpc":"2.0","id":1,"result":[format!("{channel_salt:#x}"), format!("{enc_recipient:#x}")]}),
        // the counterparty's registered pool public key
        json!({"jsonrpc":"2.0","id":1,"result":[format!("{counterparty_public_key:#x}")]}),
        // subchannel 0 carries our configured token
        json!({"jsonrpc":"2.0","id":1,"result":[format!("{subchannel_salt:#x}"), format!("{enc_token:#x}")]}),
        // the outgoing subchannel has no notes yet, so the cursor is zero
        json!({"jsonrpc":"2.0","id":1,"result":["0x0","0x0"]}),
        // reverse-channel discovery: this identity has no channels addressed to it
        json!({"jsonrpc":"2.0","id":1,"result":["0x0"]}),
        // index 1: enumeration stops
        json!({"jsonrpc":"2.0","id":1,"result":["0x0","0x0"]}),
    ];

    let (rpc_url, _seen, server) = server(responses);
    let report = client(&root, rpc_url)
        .rebuild_state()
        .await
        .expect("rebuild");
    drop(server);

    assert_eq!(report.channels_found, 1, "{report:?}");
    assert_eq!(report.rebuilt.len(), 1, "{report:?}");
    assert_eq!(report.kept, 0);
    assert_eq!(report.unrecoverable, 0);

    let state = StateStore::new(&root).expect("state");
    let rebuilt = state
        .snapshot(&report.rebuilt[0])
        .expect("read")
        .expect("the rebuilt record exists");

    assert_eq!(rebuilt.counterparty_address, counterparty);
    assert_eq!(rebuilt.counterparty_public_key, counterparty_public_key);
    assert_eq!(
        rebuilt.outgoing_key, channel_key,
        "the derived channel key must match what an open would have produced"
    );
    assert_eq!(rebuilt.token, TOKEN);
    assert_eq!(rebuilt.owner, ACCOUNT);
    // Honest about what did not come back: these are not recoverable without event scanning.
    assert_eq!(rebuilt.opened_transaction, Felt::ZERO);
    assert_eq!(rebuilt.last_write_block, 0);

    std::fs::remove_dir_all(&root).ok();
}
