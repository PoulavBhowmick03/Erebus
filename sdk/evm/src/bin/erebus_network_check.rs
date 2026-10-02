//! Read-only EVM network diagnostics with one bounded JSON configuration on stdin.

use std::io::{Read, Write};

use erebus_evm::readiness::{check_network, NetworkCheck};

const LIMIT: usize = 16 * 1024;

#[tokio::main]
async fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "--help" {
        println!("erebus-network-check: read-only EVM diagnostics.\nSupply JSON on stdin: chain_id, rpc_url, timeout_ms.\nOutput excludes the RPC URL. Exit 0 means network checks passed, not deployment or payment verification.");
        return;
    }
    if !args.is_empty() {
        finish(Err("unexpected arguments"));
    }
    let mut input = Vec::new();
    if std::io::stdin()
        .take((LIMIT + 1) as u64)
        .read_to_end(&mut input)
        .is_err()
    {
        finish(Err("cannot read configuration"));
    }
    if input.len() > LIMIT {
        finish(Err("configuration exceeds size limit"));
    }
    let config: NetworkCheck = match serde_json::from_slice(&input) {
        Ok(config) => config,
        Err(_) => finish(Err("invalid configuration")),
    };
    finish(check_network(&config).await);
}

fn finish(result: Result<erebus_evm::readiness::NetworkReport, &'static str>) -> ! {
    let (output, success) = match result {
        Ok(report) => {
            let success = report.passed;
            (
                serde_json::to_value(report).expect("serializable report"),
                success,
            )
        }
        Err(error) => (
            serde_json::json!({"version": 1, "passed": false, "error": error}),
            false,
        ),
    };
    let mut stdout = std::io::stdout().lock();
    let written = serde_json::to_writer(&mut stdout, &output).is_ok()
        && writeln!(&mut stdout).is_ok()
        && stdout.flush().is_ok();
    std::process::exit(if success && written { 0 } else { 1 });
}
