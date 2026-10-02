//! Read-only network diagnostics. Passing these checks is not deployment or payment evidence.
//!
//! Every state read uses an explicit finalized block hash. There is no fallback to `latest`,
//! no wallet, and no broadcast method. RPC errors are deliberately stripped of provider text.

use std::time::Duration;

use alloy::{
    primitives::{Address, Bytes, B256, U256, U64},
    providers::{DynProvider, Provider, ProviderBuilder},
    rpc::json_rpc::{RpcRecv, RpcSend},
    transports::RpcError,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::deployment::normalized_rpc_url;

/// A bounded read-only probe configuration. Its debug output never includes the endpoint.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkCheck {
    /// Chain ID expected from an independently selected network configuration.
    pub chain_id: u64,
    /// HTTP(S) RPC endpoint. May contain credentials; never included in a report.
    pub rpc_url: String,
    /// Per-request timeout, from 100 through 30,000 milliseconds.
    pub timeout_ms: u64,
}

impl std::fmt::Debug for NetworkCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetworkCheck")
            .field("chain_id", &self.chain_id)
            .field("timeout_ms", &self.timeout_ms)
            .finish_non_exhaustive()
    }
}

/// A public block anchor returned by the probe, not a consensus proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkAnchor {
    /// Block number.
    pub number: u64,
    /// Block hash.
    pub hash: B256,
    /// Unix timestamp in seconds.
    pub timestamp: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Block {
    number: U64,
    hash: B256,
    parent_hash: B256,
    timestamp: U64,
}

impl Block {
    fn anchor(&self) -> NetworkAnchor {
        NetworkAnchor {
            number: self.number.to(),
            hash: self.hash,
            timestamp: self.timestamp.to(),
        }
    }

    fn pin(&self) -> Value {
        json!({"blockHash": self.hash, "requireCanonical": true})
    }
}

/// One diagnostic result. Errors contain only static, locally defined descriptions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    /// Stable machine-readable check name.
    pub name: String,
    /// Whether this specific check passed.
    pub passed: bool,
    /// Sanitized failure reason, absent on success.
    pub error: Option<String>,
}

/// Diagnostic output. No participant state, private witness, key, or RPC URL is included.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkReport {
    /// Report format version.
    pub version: u16,
    /// Expected chain ID.
    pub chain_id: u64,
    /// Explicit finalized block used for all state reads, if available.
    pub finalized: Option<NetworkAnchor>,
    /// Head observed after the finalized anchor, if available.
    pub head: Option<NetworkAnchor>,
    /// True only if every required network check passed.
    pub passed: bool,
    /// Individual diagnostic results.
    pub checks: Vec<CheckResult>,
}

impl NetworkReport {
    fn record(&mut self, name: &str, result: Result<(), &'static str>) {
        self.checks.push(CheckResult {
            name: name.into(),
            passed: result.is_ok(),
            error: result.err().map(str::to_owned),
        });
        self.passed = self.checks.iter().all(|check| check.passed);
    }
}

struct Reader {
    provider: DynProvider,
    timeout: Duration,
    chain_id: u64,
}

impl Reader {
    async fn rpc<P: RpcSend, R: RpcRecv>(
        &self,
        method: &'static str,
        params: P,
    ) -> Result<R, &'static str> {
        tokio::time::timeout(self.timeout, self.provider.client().request(method, params))
            .await
            .map_err(|_| "RPC request timed out")?
            .map_err(|_| "RPC request failed")
    }

    async fn check_chain(&self) -> Result<(), &'static str> {
        let found: U64 = self.rpc("eth_chainId", ()).await?;
        if found.to::<u64>() != self.chain_id {
            return Err("RPC serves a different chain");
        }
        Ok(())
    }

    async fn block(&self, tag: &str) -> Result<Block, &'static str> {
        let block: Option<Block> = self.rpc("eth_getBlockByNumber", (tag, false)).await?;
        let block = block.ok_or("requested block unavailable")?;
        if block.hash == B256::ZERO {
            return Err("RPC returned a zero block hash");
        }
        Ok(block)
    }

    async fn recheck(&self, block: &Block) -> Result<(), &'static str> {
        let again = self
            .block(&format!("0x{:x}", block.number.to::<u64>()))
            .await?;
        if again != *block {
            return Err("finalized anchor changed during the check");
        }
        Ok(())
    }

    async fn call(
        &self,
        address: Address,
        input: Vec<u8>,
        block: &Block,
    ) -> Result<Bytes, &'static str> {
        self.rpc(
            "eth_call",
            (
                json!({"to": address, "data": Bytes::from(input), "gas": "0x4c4b40"}),
                block.pin(),
            ),
        )
        .await
    }

    async fn unknown_block_rejected(&self) -> Result<(), &'static str> {
        let request = self.provider.client().request::<_, Bytes>(
            "eth_call",
            (
                json!({"to": precompile(4), "data": "0x455245425553"}),
                json!({"blockHash": B256::repeat_byte(0xff), "requireCanonical": true}),
            ),
        );
        match tokio::time::timeout(self.timeout, request).await {
            Ok(Err(RpcError::ErrorResp(_))) => Ok(()),
            Ok(Ok(_)) => Err("RPC accepted an unknown block hash"),
            _ => Err("unknown-block check received no JSON-RPC rejection"),
        }
    }
}

/// Checks network prerequisites without submitting or signing a transaction.
///
/// Invalid configuration is rejected before connecting. A failed prerequisite is returned in
/// the report. An incorrect chain or unavailable finalized anchor stops all later state reads.
/// An honest result remains contingent on provider honesty; this is not a consensus verifier.
pub async fn check_network(config: &NetworkCheck) -> Result<NetworkReport, &'static str> {
    if config.chain_id == 0 || !(100..=30_000).contains(&config.timeout_ms) {
        return Err("invalid chain ID or timeout");
    }
    let url = normalized_rpc_url(&config.rpc_url).map_err(|_| "invalid RPC endpoint")?;
    let reader = Reader {
        provider: ProviderBuilder::new()
            .connect_http(url.parse().map_err(|_| "invalid RPC endpoint")?)
            .erased(),
        timeout: Duration::from_millis(config.timeout_ms),
        chain_id: config.chain_id,
    };
    let mut report = NetworkReport {
        version: 1,
        chain_id: config.chain_id,
        finalized: None,
        head: None,
        passed: false,
        checks: Vec::new(),
    };
    let chain = reader.check_chain().await;
    report.record("chain_id", chain);
    if chain.is_err() {
        return Ok(report);
    }
    let anchor = match reader.block("finalized").await {
        Ok(block) => {
            report.finalized = Some(block.anchor());
            report.record("explicit_finalized", Ok(()));
            block
        }
        Err(error) => {
            report.record("explicit_finalized", Err(error));
            return Ok(report);
        }
    };
    let head = reader.block("latest").await;
    let head_ok = match head {
        Ok(head) => {
            report.head = Some(head.anchor());
            if anchor.number > head.number || anchor.timestamp > head.timestamp {
                Err("finalized anchor is ahead of head")
            } else if anchor.number == head.number && anchor != head {
                Err("head and finalized disagree at one height")
            } else {
                Ok(())
            }
        }
        Err(error) => Err(error),
    };
    report.record("head_after_finalized", head_ok);
    let canonical = reader.recheck(&anchor).await;
    report.record("anchor_before_reads", canonical);
    if head_ok.is_err() || canonical.is_err() {
        return Ok(report);
    }

    // These are the EIP-1898 methods used by nonce management and settlement observers.
    let nonce: Result<U64, _> = reader
        .rpc("eth_getTransactionCount", (Address::ZERO, anchor.pin()))
        .await;
    report.record("hash_pinned_nonce", nonce.map(|_| ()));
    let balance: Result<U256, _> = reader
        .rpc("eth_getBalance", (Address::ZERO, anchor.pin()))
        .await;
    report.record("hash_pinned_balance", balance.map(|_| ()));
    let code: Result<Bytes, _> = reader
        .rpc("eth_getCode", (Address::ZERO, anchor.pin()))
        .await;
    report.record("hash_pinned_code", code.map(|_| ()));
    let logs: Result<Vec<Value>, _> = reader
        .rpc(
            "eth_getLogs",
            (json!({"blockHash": anchor.hash, "address": Address::ZERO}),),
        )
        .await;
    report.record("hash_pinned_logs", logs.map(|_| ()));
    report.record(
        "unknown_block_rejected",
        reader.unknown_block_rejected().await,
    );

    for vector in precompile_vectors() {
        let result = reader
            .call(precompile(vector.address), vector.input, &anchor)
            .await;
        report.record(
            vector.name,
            result.and_then(|output| {
                if output.as_ref() == vector.expected {
                    Ok(())
                } else {
                    Err("precompile returned an unexpected result")
                }
            }),
        );
    }
    report.record("anchor_after_reads", reader.recheck(&anchor).await);
    report.record("chain_id_after_reads", reader.check_chain().await);
    Ok(report)
}

fn precompile(index: u8) -> Address {
    let mut address = [0; 20];
    address[19] = index;
    Address::from(address)
}

struct Vector {
    name: &'static str,
    address: u8,
    input: Vec<u8>,
    expected: Vec<u8>,
}

fn words(values: &[U256]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_be_bytes::<32>())
        .collect()
}

fn decimal(value: &str) -> U256 {
    U256::from_str_radix(value, 10).expect("fixed EIP-197 BN254 constant")
}

fn precompile_vectors() -> Vec<Vector> {
    let one = U256::from(1);
    let two = U256::from(2);
    let double = words(&[
        decimal("1368015179489954701390400359078579693043519447331113978918064868415326638035"),
        decimal("9918110051302171585080402603319702774565515993150576347155970296011118125764"),
    ]);
    // EIP-197 encodes Fp2 with the imaginary coordinate first, unlike some SDK encoders.
    let generator = [
        one,
        two,
        decimal("11559732032986387107991004021392285783925812861821192530917403151452391805634"),
        decimal("10857046999023057135944570762232829481370756359578518086990519993285655852781"),
        decimal("4082367875863433681332203403145435568316851327593401208105741076214120093531"),
        decimal("8495653923123431417604973247489272438418190587263600148770280649306958101930"),
    ];
    let single = words(&generator);
    let mut negative = generator;
    negative[1] =
        decimal("21888242871839275222246405745257275088696311157297823662689037894645226208583")
            - two;
    let mut cancellation = single.clone();
    cancellation.extend(words(&negative));
    vec![
        Vector {
            name: "hash_pinned_call",
            address: 4,
            input: b"EREBUS".to_vec(),
            expected: b"EREBUS".to_vec(),
        },
        Vector {
            name: "bn254_add_double",
            address: 6,
            input: words(&[one, two, one, two]),
            expected: double.clone(),
        },
        Vector {
            name: "bn254_mul_double",
            address: 7,
            input: words(&[one, two, two]),
            expected: double,
        },
        Vector {
            name: "bn254_pairing_accept",
            address: 8,
            input: cancellation,
            expected: words(&[one]),
        },
        Vector {
            name: "bn254_pairing_reject",
            address: 8,
            input: single,
            expected: words(&[U256::ZERO]),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(chain_id: u64, rpc_url: &str, timeout_ms: u64) -> NetworkCheck {
        NetworkCheck {
            chain_id,
            rpc_url: rpc_url.to_owned(),
            timeout_ms,
        }
    }

    #[tokio::test]
    async fn invalid_configuration_is_rejected_before_connecting() {
        for invalid in [
            config(0, "https://rpc.example", 1_000),
            config(1, "https://rpc.example", 50),
            config(1, "https://rpc.example", 30_001),
            config(1, "not a url", 1_000),
        ] {
            assert!(
                check_network(&invalid).await.is_err(),
                "accepted {invalid:?}"
            );
        }
    }

    #[test]
    fn debug_never_prints_the_endpoint() {
        let check = config(10143, "https://user:secret@rpc.example/key", 1_000);
        let rendered = format!("{check:?}");
        assert!(!rendered.contains("rpc.example"));
        assert!(!rendered.contains("secret"));
        assert!(rendered.contains("10143"));
    }

    #[test]
    fn bn254_vectors_are_the_documented_generator_operations() {
        let vectors = precompile_vectors();
        assert_eq!(vectors.len(), 5);
        assert_eq!(vectors[0].address, 4);
        assert_eq!(vectors[1].address, 6);
        assert_eq!(vectors[2].address, 7);
        assert_eq!(vectors[3].address, 8);
        // The pairing vectors must be a cancellation (accept) and a single point (reject).
        assert_ne!(vectors[3].input, vectors[4].input);
        assert_eq!(vectors[3].expected, words(&[U256::from(1)]));
        assert_eq!(vectors[4].expected, words(&[U256::ZERO]));
    }
}
