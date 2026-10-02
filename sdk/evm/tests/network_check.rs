//! Read-only network diagnostics against a local anvil chain.
//!
//! These checks must pass on a chain with the same precompiles and block-tag behavior as the
//! target; the live Monad testnet run is recorded in `docs/metropolis-m8-progress.md`.
//! Ignored by default because they spawn anvil.

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use erebus_evm::readiness::{check_network, NetworkCheck};

struct Anvil {
    child: Child,
}

impl Drop for Anvil {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback bind");
    listener.local_addr().expect("local addr").port()
}

async fn start() -> (Anvil, String) {
    let port = free_port();
    let mut child = Command::new("anvil")
        .args([
            "--port",
            &port.to_string(),
            "--chain-id",
            "31337",
            "--slots-in-an-epoch",
            "1",
            "--silent",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("anvil must be installed: https://getfoundry.sh");
    let rpc_url = format!("http://127.0.0.1:{port}");
    let address = format!("127.0.0.1:{port}");
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(&address).await.is_ok() {
            return (Anvil { child }, rpc_url);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("anvil did not become ready");
}

fn config(chain_id: u64, rpc_url: &str) -> NetworkCheck {
    NetworkCheck {
        chain_id,
        rpc_url: rpc_url.to_owned(),
        timeout_ms: 5_000,
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires anvil from a Foundry install"]
async fn a_local_chain_passes_and_a_wrong_chain_fails_closed() {
    let (_anvil, rpc_url) = start().await;

    let report = check_network(&config(31_337, &rpc_url))
        .await
        .expect("diagnostic report");
    assert!(report.passed, "{report:?}");
    assert!(report.finalized.is_some(), "explicit finalized anchor");
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.name == "bn254_pairing_accept" && check.passed),
        "the BN254 pairing precompile must accept a valid vector"
    );
    assert!(
        !format!("{report:?}").contains(&rpc_url),
        "a report must not echo the endpoint"
    );

    let wrong = check_network(&config(1, &rpc_url))
        .await
        .expect("diagnostic report");
    assert!(!wrong.passed);
    assert!(wrong
        .checks
        .iter()
        .any(|check| check.name == "chain_id" && !check.passed));
    assert!(
        wrong.finalized.is_none(),
        "a wrong chain stops before state reads"
    );
}
