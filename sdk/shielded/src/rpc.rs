//! Read-only EVM JSON-RPC source for public shielded-pool events.
//!
//! It transmits no note openings. A response is checked against block headers and then
//! against the local Poseidon tree before the wallet may use it.

use futures_util::stream::{self, StreamExt, TryStreamExt};
use reqwest::{Client, Url};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};

use crate::indexer::{IndexError, PoolBlock, PoolEvent, PoolIndex};

/// RPC transport or chain evidence could not be accepted.
#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    /// An RPC URL or deployment configuration is invalid.
    #[error("invalid pool RPC configuration")]
    Configuration,
    /// The endpoint did not return a well-formed successful response.
    #[error("pool RPC request failed during {0}")]
    Request(&'static str),
    /// A log or block header disagrees with its deployment or canonical block.
    #[error("inconsistent pool RPC evidence: {0}")]
    Evidence(&'static str),
    /// Public events failed local tree or order verification.
    #[error(transparent)]
    Index(#[from] IndexError),
}

#[derive(Deserialize)]
struct JsonRpcResponse<T> {
    result: Option<T>,
    error: Option<serde_json::Value>,
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct BlockHeader {
    number: String,
    hash: String,
    parent_hash: String,
    timestamp: String,
}

#[derive(Clone, Copy)]
pub(crate) struct PoolSnapshot {
    pub finalized: PoolFinalizedBlock,
    pub head_number: u64,
    pub head_hash: [u8; 32],
    pub head_timestamp: u64,
}

/// Explicit RPC-finalized pool-chain anchor. This is not proof that a payment occurred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolFinalizedBlock {
    /// Block number returned for the `finalized` tag.
    pub number: u64,
    /// Hash rechecked against the canonical numeric block lookup.
    pub hash: [u8; 32],
    /// Timestamp used to decide whether an agreement has expired at finality.
    pub timestamp: u64,
}

/// Pool asset and verifier version read at one pinned block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolIdentity {
    /// ERC-20 asset owned by this pool.
    pub asset: [u8; 20],
    /// Deployment-specific verifier version.
    pub verifier_version: u32,
}

/// Public account words agreed at one rechecked provider-finalized pool anchor.
/// This is diagnostic state, not spend authorization or payment evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolAccountState {
    /// Common finalized block used for every balance and allowance read.
    pub anchor: PoolFinalizedBlock,
    /// Native gas balance, as an unsigned 256-bit big-endian word.
    pub native_balance: [u8; 32],
    /// Configured pool asset balance, not shielded note value.
    pub asset_balance: [u8; 32],
    /// Configured owner's allowance for this pool only.
    pub allowance: [u8; 32],
}

/// Independently reviewed runtime hashes for the pool's four immutable dependencies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolDependencyPins {
    /// Poseidon implementation runtime hash.
    pub poseidon: [u8; 32],
    /// Deposit verifier runtime hash.
    pub deposit: [u8; 32],
    /// Transfer verifier runtime hash.
    pub transfer: [u8; 32],
    /// Withdrawal verifier runtime hash.
    pub withdraw: [u8; 32],
}

/// Independently reviewed runtime hashes for the pool contract and its four dependencies.
///
/// A deployment's verifier is only as trustworthy as the code that implements it. Pins come
/// from reviewed deployment evidence, never from the RPC being checked; every hash is read
/// back from chain code at one canonical block and compared. A zero pin is a configuration
/// error, never "skip the check".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolRuntimePins {
    /// Pool contract runtime hash.
    pub pool: [u8; 32],
    /// Poseidon implementation runtime hash.
    pub poseidon: [u8; 32],
    /// Deposit verifier runtime hash.
    pub deposit: [u8; 32],
    /// Transfer verifier runtime hash.
    pub transfer: [u8; 32],
    /// Withdrawal verifier runtime hash.
    pub withdraw: [u8; 32],
}

impl PoolRuntimePins {
    /// The four dependency pins, for [`PoolRpc::authenticate_dependencies_at`].
    #[must_use]
    pub const fn dependencies(&self) -> PoolDependencyPins {
        PoolDependencyPins {
            poseidon: self.poseidon,
            deposit: self.deposit,
            transfer: self.transfer,
            withdraw: self.withdraw,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcLog {
    address: String,
    block_number: String,
    block_hash: String,
    transaction_hash: String,
    log_index: String,
    topics: Vec<String>,
    data: String,
    removed: Option<bool>,
}

/// Default number of block-header reads [`PoolRpc::sync`] keeps in flight.
pub const DEFAULT_MAX_CONCURRENT_READS: u64 = 8;

/// Hard ceiling for [`PoolRpc::with_max_concurrent_reads`].
pub const MAX_CONCURRENT_READS: u64 = 32;

/// Default request-rate ceiling for one configured source, not a provider-wide quota.
pub const DEFAULT_MAX_REQUESTS_PER_SECOND: u32 = 40;

/// Widest `eth_getLogs` block range a scan issues; the public Monad testnet RPC cap.
pub const MAX_LOG_BLOCK_RANGE: u64 = 100;

/// Requests the pacer admits back to back before spacing them at the configured rate.
const PACER_BURST: u32 = 8;

/// Generic cell-rate pacer: at most `PACER_BURST` requests at once, then one per `interval`.
///
/// In any one-second span it admits at most `PACER_BURST + rate` requests, which keeps the
/// local burst bounded. Separate sources have separate pacers; operator quotas still apply.
struct RequestPacer {
    interval: std::time::Duration,
    tolerance: std::time::Duration,
    next: tokio::sync::Mutex<tokio::time::Instant>,
}

impl RequestPacer {
    fn new(per_second: u32) -> Self {
        let interval = std::time::Duration::from_secs(1) / per_second;
        Self {
            interval,
            tolerance: interval * (PACER_BURST - 1),
            next: tokio::sync::Mutex::new(tokio::time::Instant::now()),
        }
    }

    async fn admit(&self) {
        let mut next = self.next.lock().await;
        let now = tokio::time::Instant::now();
        let theoretical = (*next).max(now);
        if let Some(earliest) = theoretical.checked_sub(self.tolerance) {
            if earliest > now {
                tokio::time::sleep_until(earliest).await;
            }
        }
        *next = theoretical + self.interval;
    }
}

/// A configured read-only source for one EVM pool deployment.
pub struct PoolRpc {
    url: Url,
    client: Client,
    chain_id: u64,
    pool: [u8; 20],
    max_concurrent_reads: usize,
    pacer: RequestPacer,
}

impl PoolRpc {
    /// Reads the pool and its four dependency runtime hashes at one canonical block.
    ///
    /// This reports the code the provider serves, for deployment review and local test setup.
    /// It is not authentication: pins must be compared against independently reviewed
    /// deployment evidence, never copied from the provider being checked.
    pub async fn runtime_pins_at(&self, block_hash: [u8; 32]) -> Result<PoolRuntimePins, RpcError> {
        self.check_chain().await?;
        let block = canonical_block(block_hash);
        let pool = self.code_hash_at(self.pool, &block).await?;
        let mut dependencies = [[0u8; 32]; 4];
        for (position, signature) in [
            "poseidonTwo()",
            "depositVerifier()",
            "transferVerifier()",
            "withdrawVerifier()",
        ]
        .into_iter()
        .enumerate()
        {
            let address: String = self.call("eth_call", serde_json::json!([
                {"to":format!("0x{}", hex::encode(self.pool)),"data":format!("0x{}",hex::encode(&Keccak256::digest(signature.as_bytes())[..4]))}, block
            ])).await?;
            let word = fixed_hex::<32>(&address)?;
            if word[..12] != [0; 12] || word[12..] == [0; 20] {
                return Err(RpcError::Evidence("invalid immutable pool dependency"));
            }
            let address: [u8; 20] = word[12..]
                .try_into()
                .map_err(|_| RpcError::Evidence("invalid immutable pool dependency"))?;
            dependencies[position] = self.code_hash_at(address, &block).await?;
        }
        self.check_chain().await?;
        Ok(PoolRuntimePins {
            pool,
            poseidon: dependencies[0],
            deposit: dependencies[1],
            transfer: dependencies[2],
            withdraw: dependencies[3],
        })
    }

    /// Checks immutable hash/verifier dependencies at one canonical block hash.
    /// Pins must come from reviewed deployment evidence; this does not authenticate providers.
    pub async fn authenticate_dependencies_at(
        &self,
        block_hash: [u8; 32],
        pins: PoolDependencyPins,
    ) -> Result<(), RpcError> {
        if [pins.poseidon, pins.deposit, pins.transfer, pins.withdraw].contains(&[0; 32]) {
            return Err(RpcError::Configuration);
        }
        if self.runtime_pins_at(block_hash).await?.dependencies() != pins {
            return Err(RpcError::Evidence(
                "pool dependency runtime differs from trusted pin",
            ));
        }
        Ok(())
    }

    /// Checks the pool's own runtime and its four dependencies at one canonical block hash.
    ///
    /// The certificate path calls this before it records a pool runtime pin, so the pin it
    /// stores is the code that was actually read, not the configuration it was read against.
    pub async fn authenticate_runtime_at(
        &self,
        block_hash: [u8; 32],
        pins: PoolRuntimePins,
    ) -> Result<(), RpcError> {
        if [
            pins.pool,
            pins.poseidon,
            pins.deposit,
            pins.transfer,
            pins.withdraw,
        ]
        .contains(&[0; 32])
        {
            return Err(RpcError::Configuration);
        }
        if self.runtime_pins_at(block_hash).await? != pins {
            return Err(RpcError::Evidence("pool runtime differs from trusted pin"));
        }
        Ok(())
    }

    /// Reads one address's code hash at a canonical block.
    async fn code_hash_at(
        &self,
        address: [u8; 20],
        block: &serde_json::Value,
    ) -> Result<[u8; 32], RpcError> {
        let code: String = self
            .call(
                "eth_getCode",
                serde_json::json!([format!("0x{}", hex::encode(address)), block]),
            )
            .await?;
        let code = hex::decode(
            code.strip_prefix("0x")
                .ok_or(RpcError::Evidence("invalid contract code"))?,
        )
        .map_err(|_| RpcError::Evidence("invalid contract code"))?;
        if code.is_empty() {
            return Err(RpcError::Evidence("contract code is empty"));
        }
        Ok(Keccak256::digest(&code).into())
    }
    /// Checks endpoint identity without exposing a URL or its credentials.
    /// Distinct URLs do not establish operational provider independence.
    pub fn shares_endpoint(&self, other: &Self) -> bool {
        self.matches_endpoint(other.endpoint())
    }

    /// Stable domain-separated hash of the configured endpoint, chain, and pool.
    ///
    /// It names this observer configuration in durable records without storing the URL or any
    /// credential in it. It does not identify or authenticate the provider behind the URL.
    pub fn endpoint_fingerprint(&self) -> [u8; 32] {
        erebus_core::suite::keccak256(&[
            b"EREBUS_POOL_RPC_ENDPOINT_V1",
            &self.chain_id.to_be_bytes(),
            &self.pool,
            self.url.as_str().as_bytes(),
        ])
    }
    /// Endpoint identity for configuration checks. It may contain credentials; do not log it.
    pub(crate) fn endpoint(&self) -> &str {
        self.url.as_str()
    }

    pub(crate) fn matches_endpoint(&self, endpoint: &str) -> bool {
        let Ok(mut endpoint) = Url::parse(endpoint) else {
            return false;
        };
        endpoint.set_fragment(None);
        endpoint == self.url
    }
    /// EVM chain ID this source expects.
    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    /// Pool address this source filters for.
    pub fn pool(&self) -> [u8; 20] {
        self.pool
    }

    /// Connects to one RPC and rejects zero-chain or zero-pool configurations.
    pub fn new(url: &str, chain_id: u64, pool: [u8; 20]) -> Result<Self, RpcError> {
        Self::with_timeout(url, chain_id, pool, std::time::Duration::from_secs(15))
    }

    /// Connects with an operator-selected request deadline. A timeout is never absence evidence.
    pub fn with_timeout(
        url: &str,
        chain_id: u64,
        pool: [u8; 20],
        timeout: std::time::Duration,
    ) -> Result<Self, RpcError> {
        if chain_id == 0 || pool == [0; 20] || timeout.is_zero() {
            return Err(RpcError::Configuration);
        }
        let mut url = Url::parse(url).map_err(|_| RpcError::Configuration)?;
        if !matches!(url.scheme(), "https" | "http") {
            return Err(RpcError::Configuration);
        }
        url.set_fragment(None);
        let client = Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|_| RpcError::Configuration)?;
        Ok(Self {
            url,
            client,
            chain_id,
            pool,
            max_concurrent_reads: DEFAULT_MAX_CONCURRENT_READS as usize,
            pacer: RequestPacer::new(DEFAULT_MAX_REQUESTS_PER_SECOND),
        })
    }

    /// Sets how many block-header reads one history window keeps in flight (1..=32).
    ///
    /// Concurrency changes only how fast blocks are fetched. Every block is still checked
    /// and applied in height order, and a batch commits only after all of its reads succeed.
    pub fn with_max_concurrent_reads(mut self, reads: u64) -> Result<Self, RpcError> {
        if reads == 0 || reads > MAX_CONCURRENT_READS {
            return Err(RpcError::Configuration);
        }
        self.max_concurrent_reads = usize::try_from(reads).map_err(|_| RpcError::Configuration)?;
        Ok(self)
    }

    /// Sets this endpoint's request-rate ceiling across every read through this source.
    pub fn with_max_requests_per_second(mut self, rate: u32) -> Result<Self, RpcError> {
        if rate == 0 {
            return Err(RpcError::Configuration);
        }
        self.pacer = RequestPacer::new(rate);
        Ok(self)
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: &'static str,
        params: serde_json::Value,
    ) -> Result<T, RpcError> {
        // Retry transport failures only for idempotent reads; callers still validate every result.
        let read_only = matches!(
            method,
            "eth_chainId"
                | "eth_blockNumber"
                | "eth_getBlockByNumber"
                | "eth_getLogs"
                | "eth_call"
                | "eth_getCode"
                | "eth_getBalance"
        );
        for attempt in 0..4 {
            self.pacer.admit().await;
            let response = self
                .client
                .post(self.url.clone())
                .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
                .send()
                .await;
            let response = match response {
                Ok(response) => response,
                Err(error)
                    if read_only && attempt < 3 && (error.is_timeout() || error.is_connect()) =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(100 << attempt)).await;
                    continue;
                }
                Err(_) => return Err(RpcError::Request(method)),
            };
            let transient = matches!(response.status().as_u16(), 429 | 502 | 503 | 504);
            if read_only && transient && attempt < 3 {
                tokio::time::sleep(std::time::Duration::from_millis(100 << attempt)).await;
                continue;
            }
            if !response.status().is_success() {
                return Err(RpcError::Request(method));
            }
            let decoded: JsonRpcResponse<T> = response
                .json()
                .await
                .map_err(|_| RpcError::Request(method))?;
            if decoded.error.is_some() {
                return Err(RpcError::Request(method));
            }
            return decoded.result.ok_or(RpcError::Request(method));
        }
        Err(RpcError::Request(method))
    }

    /// Confirms the endpoint's current EVM chain ID before reading any logs.
    pub async fn check_chain(&self) -> Result<(), RpcError> {
        let value: String = self.call("eth_chainId", serde_json::json!([])).await?;
        if quantity(&value)? != self.chain_id {
            return Err(RpcError::Evidence("wrong chain ID"));
        }
        Ok(())
    }

    /// Returns the endpoint's current head number, which is not a finality assertion.
    pub async fn head(&self) -> Result<u64, RpcError> {
        self.check_chain().await?;
        let value: String = self.call("eth_blockNumber", serde_json::json!([])).await?;
        quantity(&value)
    }

    /// Reads the RPC's explicit finalized anchor with no confirmation-depth fallback.
    /// The caller must recheck this anchor after any dependent settlement reads.
    pub async fn finalized_head(&self) -> Result<PoolFinalizedBlock, RpcError> {
        self.check_chain().await?;
        let finalized: BlockHeader = self
            .call(
                "eth_getBlockByNumber",
                serde_json::json!(["finalized", false]),
            )
            .await?;
        let number = quantity(&finalized.number)?;
        if number > self.head().await? {
            return Err(RpcError::Evidence("finalized block is ahead of head"));
        }
        let canonical = self.header(number).await?;
        if canonical.hash != finalized.hash
            || canonical.parent_hash != finalized.parent_hash
            || canonical.timestamp != finalized.timestamp
        {
            return Err(RpcError::Evidence("finalized block is not canonical"));
        }
        self.check_chain().await?;
        Ok(PoolFinalizedBlock {
            number,
            hash: fixed_hex::<32>(&finalized.hash)?,
            timestamp: quantity(&finalized.timestamp)?,
        })
    }

    /// Rechecks one block hash by its number after dependent reads.
    pub async fn canonical_block_hash(&self, number: u64) -> Result<[u8; 32], RpcError> {
        self.check_chain().await?;
        fixed_hex::<32>(&self.header(number).await?.hash)
    }

    /// Selects canonical anchors below both explicit finalized tips and heads.
    /// Callers recheck them after dependent reads; this is not consensus proof.
    pub(crate) async fn shared_snapshot(&self, peer: &Self) -> Result<PoolSnapshot, RpcError> {
        if self.chain_id != peer.chain_id
            || self.pool != peer.pool
            || self.endpoint() == peer.endpoint()
        {
            return Err(RpcError::Configuration);
        }
        let (first, second) = tokio::join!(self.finalized_head(), peer.finalized_head());
        let first = first?;
        let second = second?;
        let finalized_number = first.number.min(second.number);
        let (first_head, second_head) = tokio::join!(self.head(), peer.head());
        let head_number = first_head?.min(second_head?);
        if head_number < finalized_number {
            return Err(RpcError::Evidence(
                "shared finalized anchor is ahead of head",
            ));
        }
        let common = |number| async move {
            let (first, second) = tokio::join!(self.header(number), peer.header(number));
            let first = first?;
            if first != second? {
                return Err(RpcError::Evidence("RPC providers disagree on shared block"));
            }
            Ok(PoolFinalizedBlock {
                number,
                hash: fixed_hex::<32>(&first.hash)?,
                timestamp: quantity(&first.timestamp)?,
            })
        };
        let finalized = common(finalized_number).await?;
        if [first, second]
            .iter()
            .any(|tip| tip.number == finalized_number && *tip != finalized)
        {
            return Err(RpcError::Evidence(
                "finalized anchor changed during snapshot selection",
            ));
        }
        let head = common(head_number).await?;
        self.check_chain().await?;
        peer.check_chain().await?;
        Ok(PoolSnapshot {
            finalized,
            head_number,
            head_hash: head.hash,
            head_timestamp: head.timestamp,
        })
    }

    pub(crate) async fn shared_finalized_prefix(
        &self,
        peer: &Self,
        snapshot: PoolSnapshot,
        number: u64,
        expected_hash: [u8; 32],
    ) -> Result<PoolSnapshot, RpcError> {
        if number > snapshot.finalized.number {
            return Err(RpcError::Evidence(
                "prefix is not finalized by both providers",
            ));
        }
        let (first, second) = tokio::join!(self.header(number), peer.header(number));
        let first = first?;
        if first != second? || fixed_hex::<32>(&first.hash)? != expected_hash {
            return Err(RpcError::Evidence(
                "RPC providers disagree on finalized prefix",
            ));
        }
        Ok(PoolSnapshot {
            finalized: PoolFinalizedBlock {
                number,
                hash: expected_hash,
                timestamp: quantity(&first.timestamp)?,
            },
            head_number: number,
            head_hash: expected_hash,
            head_timestamp: quantity(&first.timestamp)?,
        })
    }

    /// Simulates exactly one zero-value private transfer at a canonical hash.
    /// This never broadcasts or changes state. Success is not a reservation or payment.
    pub(crate) async fn simulate_transfer_at(
        &self,
        data: &[u8],
        sponsor: [u8; 20],
        gas_limit: u64,
        block_hash: [u8; 32],
    ) -> Result<(), RpcError> {
        let selector =
            Keccak256::digest(b"transferPrivate(uint256[2],uint256[2][2],uint256[2],uint256[11])");
        if data.len() != 4 + 19 * 32
            || data[..4] != selector[..4]
            || sponsor == [0; 20]
            || gas_limit == 0
            || block_hash == [0; 32]
        {
            return Err(RpcError::Configuration);
        }
        self.check_chain().await?;
        let value: String = self
            .call(
                "eth_call",
                serde_json::json!([
                    {
                        "from": format!("0x{}", hex::encode(sponsor)),
                        "to": format!("0x{}", hex::encode(self.pool)),
                        "data": format!("0x{}", hex::encode(data)),
                        "gas": format!("0x{gas_limit:x}"),
                        "value": "0x0",
                    },
                    canonical_block(block_hash)
                ]),
            )
            .await?;
        if value != "0x" {
            return Err(RpcError::Evidence(
                "private transfer simulation returned data",
            ));
        }
        self.check_chain().await
    }

    /// Reads the pool's one-time deal state at an explicitly pinned canonical block.
    /// A local timeout or missing RPC support is an error, never an unpaid result.
    pub async fn consumed_deal_at(
        &self,
        nullifier: [u8; 32],
        block_hash: [u8; 32],
    ) -> Result<bool, RpcError> {
        self.check_chain().await?;
        let mut data = Vec::with_capacity(36);
        data.extend_from_slice(&Keccak256::digest(b"consumedDeals(uint256)")[..4]);
        data.extend_from_slice(&nullifier);
        let value: String = self
            .call(
                "eth_call",
                serde_json::json!([
                    {
                        "to": format!("0x{}", hex::encode(self.pool)),
                        "data": format!("0x{}", hex::encode(data)),
                    },
                    {
                        "blockHash": format!("0x{}", hex::encode(block_hash)),
                        "requireCanonical": true,
                    }
                ]),
            )
            .await?;
        let word = fixed_hex::<32>(&value)?;
        if word[..31].iter().any(|byte| *byte != 0) || word[31] > 1 {
            return Err(RpcError::Evidence(
                "pool deal consumption is not an ABI bool",
            ));
        }
        Ok(word[31] == 1)
    }

    /// Reads immutable pool identity at a pinned canonical block.
    pub async fn pool_identity_at(&self, block_hash: [u8; 32]) -> Result<PoolIdentity, RpcError> {
        self.check_chain().await?;
        let asset = self.pool_word_at_hash("asset", block_hash).await?;
        if asset[..12].iter().any(|byte| *byte != 0) {
            return Err(RpcError::Evidence("pool asset is not an ABI address"));
        }
        let version = self
            .pool_word_at_hash("verifierVersion", block_hash)
            .await?;
        if version[..28].iter().any(|byte| *byte != 0) {
            return Err(RpcError::Evidence("pool verifier version overflow"));
        }
        Ok(PoolIdentity {
            asset: asset[12..].try_into().expect("fixed ABI address"),
            verifier_version: u32::from_be_bytes(version[28..].try_into().expect("fixed ABI word")),
        })
    }

    /// Reads only configured account/asset/pool diagnostics; never signs or scans history.
    pub async fn account_state_agreed(
        &self,
        peer: &Self,
        domain: crate::index_store::IndexDomain,
        expected: PoolIdentity,
        pins: PoolRuntimePins,
        owner: [u8; 20],
    ) -> Result<PoolAccountState, RpcError> {
        if domain.chain_id != self.chain_id
            || domain.pool != self.pool
            || domain.first_hash == [0; 32]
            || expected.asset == [0; 20]
            || expected.verifier_version == 0
            || owner == [0; 20]
        {
            return Err(RpcError::Configuration);
        }
        let snapshot = self.shared_snapshot(peer).await?;
        let anchor = snapshot.finalized;
        if anchor.number < domain.first_block {
            return Err(RpcError::Evidence("account anchor predates deployment"));
        }
        let mut agreed = None;
        for rpc in [self, peer] {
            for hash in [domain.first_hash, anchor.hash] {
                rpc.authenticate_runtime_at(hash, pins).await?;
                if rpc.pool_identity_at(hash).await? != expected {
                    return Err(RpcError::Evidence(
                        "account diagnostic pool identity differs",
                    ));
                }
            }
            let native: String = rpc
                .call(
                    "eth_getBalance",
                    serde_json::json!([
                        format!("0x{}", hex::encode(owner)),
                        canonical_block(anchor.hash)
                    ]),
                )
                .await?;
            let mut balance = Keccak256::digest(b"balanceOf(address)")[..4].to_vec();
            let mut owner_word = [0u8; 32];
            owner_word[12..].copy_from_slice(&owner);
            balance.extend_from_slice(&owner_word);
            let mut allowance = Keccak256::digest(b"allowance(address,address)")[..4].to_vec();
            allowance.extend_from_slice(&owner_word);
            let mut pool_word = [0u8; 32];
            pool_word[12..].copy_from_slice(&self.pool);
            allowance.extend_from_slice(&pool_word);
            let word = |data: Vec<u8>| async move {
                let result: String = rpc.call("eth_call", serde_json::json!([
                    {"to":format!("0x{}",hex::encode(expected.asset)), "data":format!("0x{}",hex::encode(data))},
                    canonical_block(anchor.hash)
                ])).await?;
                fixed_hex::<32>(&result)
            };
            let state = PoolAccountState {
                anchor,
                native_balance: quantity_word(&native)?,
                asset_balance: word(balance).await?,
                allowance: word(allowance).await?,
            };
            if agreed.is_some_and(|old| old != state) {
                return Err(RpcError::Evidence(
                    "RPC providers disagree on account diagnostics",
                ));
            }
            agreed = Some(state);
        }
        for rpc in [self, peer] {
            for (number, hash) in [
                (domain.first_block, domain.first_hash),
                (anchor.number, anchor.hash),
            ] {
                if rpc.canonical_block_hash(number).await? != hash {
                    return Err(RpcError::Evidence("account diagnostic anchor changed"));
                }
            }
            if rpc.finalized_head().await?.number < anchor.number {
                return Err(RpcError::Evidence("account diagnostic finality regressed"));
            }
            rpc.check_chain().await?;
        }
        agreed.ok_or(RpcError::Evidence("account diagnostics unavailable"))
    }

    /// Reads the pool's frontier at one finalized block that both providers agree on.
    ///
    /// Both providers must return the same finalized block, authenticate the pinned runtimes there,
    /// and return identical `nextLeafIndex`, `currentRoot`, `filledSubtrees` and `knownRoots(currentRoot)`
    /// for exactly that block hash. The frontier must recompute the root locally, and the block must
    /// still be canonical and finalized afterwards. Agreement between providers is not a cryptographic
    /// proof of chain state. This never signs, sends or scans history.
    pub async fn acquire_checkpoint_agreed(
        &self,
        peer: &Self,
        deployment: crate::index_store::IndexDomain,
        pins: PoolRuntimePins,
    ) -> Result<crate::indexer::TreeCheckpoint, RpcError> {
        if deployment.chain_id != self.chain_id
            || deployment.pool != self.pool
            || peer.chain_id != self.chain_id
            || peer.pool != self.pool
            || deployment.first_hash == [0; 32]
            || self.shares_endpoint(peer)
        {
            return Err(RpcError::Configuration);
        }
        let anchor = self.shared_snapshot(peer).await?.finalized;
        if anchor.number < deployment.first_block {
            return Err(RpcError::Evidence("checkpoint anchor predates deployment"));
        }
        let mut agreed = None;
        for rpc in [self, peer] {
            if rpc.canonical_block_hash(deployment.first_block).await? != deployment.first_hash {
                return Err(RpcError::Evidence("deployment anchor is not canonical"));
            }
            rpc.authenticate_runtime_at(deployment.first_hash, pins)
                .await?;
            rpc.authenticate_runtime_at(anchor.hash, pins).await?;
            let state = rpc.frontier_at(anchor).await?;
            if agreed.is_some_and(|old| old != state) {
                return Err(RpcError::Evidence(
                    "RPC providers disagree on the pool checkpoint state",
                ));
            }
            agreed = Some(state);
        }
        let checkpoint = agreed.ok_or(RpcError::Evidence("pool checkpoint unavailable"))?;
        checkpoint.verify().map_err(|_| {
            RpcError::Evidence("pool checkpoint frontier does not recompute its root")
        })?;
        for rpc in [self, peer] {
            if rpc.canonical_block_hash(anchor.number).await? != anchor.hash {
                return Err(RpcError::Evidence("checkpoint anchor changed"));
            }
            if rpc.finalized_head().await?.number < anchor.number {
                return Err(RpcError::Evidence("checkpoint finality regressed"));
            }
            rpc.check_chain().await?;
        }
        Ok(checkpoint)
    }

    /// One provider's frontier reads, every one bound to the anchor's exact canonical hash.
    async fn frontier_at(
        &self,
        anchor: PoolFinalizedBlock,
    ) -> Result<crate::indexer::TreeCheckpoint, RpcError> {
        self.check_chain().await?;
        let count = self.pool_word_at_hash("nextLeafIndex", anchor.hash).await?;
        if count[..28].iter().any(|byte| *byte != 0) {
            return Err(RpcError::Evidence("pool leaf count overflow"));
        }
        let root = self.pool_word_at_hash("currentRoot", anchor.hash).await?;
        let mut filled_subtrees = [[0u8; 32]; erebus_core::shielded::NOTE_TREE_DEPTH];
        for (level, slot) in filled_subtrees.iter_mut().enumerate() {
            let mut argument = [0u8; 32];
            argument[24..].copy_from_slice(&(level as u64).to_be_bytes());
            *slot = self
                .pool_call_at_hash("filledSubtrees(uint256)", argument, anchor.hash)
                .await?;
        }
        let known = self
            .pool_call_at_hash("knownRoots(uint256)", root, anchor.hash)
            .await?;
        if known[..31].iter().any(|byte| *byte != 0) || known[31] != 1 {
            return Err(RpcError::Evidence("pool does not know its current root"));
        }
        Ok(crate::indexer::TreeCheckpoint {
            block_number: anchor.number,
            block_hash: anchor.hash,
            leaf_count: u32::from_be_bytes(count[28..].try_into().expect("fixed ABI word")),
            root,
            filled_subtrees,
        })
    }

    async fn pool_call_at_hash(
        &self,
        signature: &'static str,
        argument: [u8; 32],
        block_hash: [u8; 32],
    ) -> Result<[u8; 32], RpcError> {
        let mut data = Keccak256::digest(signature.as_bytes())[..4].to_vec();
        data.extend_from_slice(&argument);
        let value: String = self
            .call(
                "eth_call",
                serde_json::json!([
                    {
                        "to": format!("0x{}", hex::encode(self.pool)),
                        "data": format!("0x{}", hex::encode(data)),
                    },
                    canonical_block(block_hash)
                ]),
            )
            .await?;
        fixed_hex::<32>(&value)
    }

    async fn pool_word_at_hash(
        &self,
        function: &'static str,
        block_hash: [u8; 32],
    ) -> Result<[u8; 32], RpcError> {
        let selector = &Keccak256::digest(format!("{function}()").as_bytes())[..4];
        let value: String = self
            .call(
                "eth_call",
                serde_json::json!([
                    {
                        "to": format!("0x{}", hex::encode(self.pool)),
                        "data": format!("0x{}", hex::encode(selector)),
                    },
                    {
                        "blockHash": format!("0x{}", hex::encode(block_hash)),
                        "requireCanonical": true,
                    }
                ]),
            )
            .await?;
        fixed_hex::<32>(&value)
    }

    async fn header(&self, number: u64) -> Result<BlockHeader, RpcError> {
        let header: BlockHeader = self
            .call(
                "eth_getBlockByNumber",
                serde_json::json!([format!("0x{number:x}"), false]),
            )
            .await?;
        if quantity(&header.number)? != number {
            return Err(RpcError::Evidence("wrong block number"));
        }
        Ok(header)
    }

    async fn pool_word(&self, function: &'static str, number: u64) -> Result<[u8; 32], RpcError> {
        let selector = &Keccak256::digest(format!("{function}()").as_bytes())[..4];
        let value: String = self
            .call(
                "eth_call",
                serde_json::json!([
                    {
                        "to": format!("0x{}", hex::encode(self.pool)),
                        "data": format!("0x{}", hex::encode(selector))
                    },
                    format!("0x{number:x}")
                ]),
            )
            .await?;
        fixed_hex::<32>(&value)
    }

    async fn verify_pool_state(&self, index: &PoolIndex) -> Result<(), RpcError> {
        let tip = index
            .tip()
            .ok_or(RpcError::Evidence("index has no block"))?;
        let root = self.pool_word("currentRoot", tip.number).await?;
        let next = self.pool_word("nextLeafIndex", tip.number).await?;
        if next[..28].iter().any(|byte| *byte != 0) {
            return Err(RpcError::Evidence("pool leaf count overflow"));
        }
        let next = u32::from_be_bytes(next[28..].try_into().expect("fixed width"));
        if root != index.root() || next != index.leaf_count() {
            return Err(RpcError::Evidence("pool state differs from indexed events"));
        }
        if fixed_hex::<32>(&self.header(tip.number).await?.hash)? != tip.hash {
            return Err(RpcError::Evidence("block changed during pool state check"));
        }
        Ok(())
    }

    /// Fetches one block's pool logs and checks their hashes against its canonical header.
    pub async fn read_block(&self, number: u64) -> Result<PoolBlock, RpcError> {
        self.check_chain().await?;
        let mut blocks = self.read_range(number, number).await?;
        match (blocks.pop(), blocks.is_empty()) {
            (Some(block), true) => Ok(block),
            _ => Err(RpcError::Evidence("wrong block count")),
        }
    }

    /// Reads canonical headers `from..=to` with bounded concurrency, returned in height order.
    ///
    /// Any failed or missing response fails the whole range; nothing is treated as absent.
    async fn headers(&self, from: u64, to: u64) -> Result<Vec<BlockHeader>, RpcError> {
        stream::iter(from..=to)
            .map(|number| self.header(number))
            .buffered(self.max_concurrent_reads)
            .try_collect()
            .await
    }

    /// Fetches the pool logs of `from..=to` and checks each against its canonical header.
    ///
    /// Every block's header is read before the ranged `eth_getLogs` and again after it, and
    /// each hash must be unchanged. Every log must name a block in the range, that block's
    /// pre-read hash, and the pool, and must arrive in (block, log index) order. Parent-hash
    /// continuity is checked by [`PoolIndex::apply_block`] as the caller applies the blocks in
    /// order. The range never exceeds [`MAX_LOG_BLOCK_RANGE`].
    async fn read_range(&self, from: u64, to: u64) -> Result<Vec<PoolBlock>, RpcError> {
        if from > to || to - from >= MAX_LOG_BLOCK_RANGE {
            return Err(RpcError::Configuration);
        }
        let mut blocks = Vec::new();
        for (number, header) in (from..=to).zip(self.headers(from, to).await?) {
            blocks.push(PoolBlock {
                number,
                hash: fixed_hex::<32>(&header.hash)?,
                parent_hash: fixed_hex::<32>(&header.parent_hash)?,
                events: Vec::new(),
            });
        }
        let topics = [
            event_topic("NoteInserted(uint256,uint256,uint256)"),
            event_topic("DealTransferred(uint256,uint256,uint256)"),
            event_topic("NoteWithdrawn(uint256,address,uint256)"),
        ];
        let logs: Vec<RpcLog> = self.call("eth_getLogs", serde_json::json!([{
            "address": format!("0x{}", hex::encode(self.pool)),
            "fromBlock": format!("0x{from:x}"),
            "toBlock": format!("0x{to:x}"),
            "topics": [topics.iter().map(|topic| format!("0x{}", hex::encode(topic))).collect::<Vec<_>>()]
        }])).await?;
        let mut last = None;
        for log in logs {
            let number = quantity(&log.block_number)?;
            let block = number
                .checked_sub(from)
                .and_then(|offset| usize::try_from(offset).ok())
                .and_then(|offset| blocks.get_mut(offset))
                .ok_or(RpcError::Evidence("log block or pool mismatch"))?;
            if fixed_hex::<20>(&log.address)? != self.pool
                || block.number != number
                || fixed_hex::<32>(&log.block_hash)? != block.hash
                || log.removed.unwrap_or(false)
            {
                return Err(RpcError::Evidence("log block or pool mismatch"));
            }
            let position = (number, quantity(&log.log_index)?);
            if last.is_some_and(|last| position <= last) {
                return Err(RpcError::Evidence("logs out of block order"));
            }
            last = Some(position);
            block.events.push(decode_log(&log, &topics)?);
        }
        let after = self.headers(from, to).await?;
        if after.len() != blocks.len() {
            return Err(RpcError::Evidence("wrong block count"));
        }
        for (block, header) in blocks.iter().zip(&after) {
            if fixed_hex::<32>(&header.hash)? != block.hash {
                return Err(RpcError::Evidence("block changed while logs were read"));
            }
        }
        Ok(blocks)
    }

    /// Rewinds a changed tip and appends at most `limit` consecutive canonical blocks.
    ///
    /// Blocks are fetched in contiguous windows of at most [`MAX_LOG_BLOCK_RANGE`] heights,
    /// with header reads running concurrently up to the configured bound, and are applied
    /// strictly in height order. Any failed read aborts the batch without committing.
    /// A batch is committed only after the pool's stored root and leaf count agree with
    /// the event index. An observation is not finality; callers still need their chosen
    /// confirmation rule before treating notes as irreversible.
    pub async fn sync(
        &self,
        index: &mut PoolIndex,
        through: u64,
        limit: u64,
    ) -> Result<u64, RpcError> {
        if limit == 0 || limit > 1_000 {
            return Err(RpcError::Configuration);
        }
        self.check_chain().await?;
        let mut staged = index.clone();
        if let Some(tip) = staged.tip() {
            let canonical = self.header(tip.number).await?;
            if fixed_hex::<32>(&canonical.hash)? != tip.hash {
                // Canonical branches share one prefix. Find its boundary without one
                // RPC per orphaned block; u64 heights need at most 64 search steps.
                let mut low = 0;
                let mut high = staged.blocks().len();
                while low < high {
                    let middle = low + (high - low) / 2;
                    let cached = &staged.blocks()[middle];
                    let canonical = self.header(cached.number).await?;
                    if fixed_hex::<32>(&canonical.hash)? == cached.hash {
                        low = middle + 1;
                    } else {
                        high = middle;
                    }
                }
                let height = staged
                    .blocks()
                    .get(low)
                    .ok_or(RpcError::Evidence("chain changed during prefix search"))?
                    .number;
                staged.rewind_from(height)?;
            }
        }
        let start = staged
            .tip()
            .map_or(staged.first_block(), |tip| tip.number + 1);
        if through < start {
            if staged.tip().is_some() {
                self.verify_pool_state(&staged).await?;
            }
            *index = staged;
            return Ok(0);
        }
        let end = through.min(start.saturating_add(limit - 1));
        let mut from = start;
        loop {
            let to = end.min(from.saturating_add(MAX_LOG_BLOCK_RANGE - 1));
            for block in self.read_range(from, to).await? {
                staged.apply_block(block)?;
            }
            if to >= end {
                break;
            }
            from = to + 1;
        }
        self.verify_pool_state(&staged).await?;
        self.check_chain().await?;
        *index = staged;
        Ok(end - start + 1)
    }
}

fn event_topic(signature: &str) -> [u8; 32] {
    Keccak256::digest(signature.as_bytes()).into()
}

/// The block parameter every code read must use: one hash, required canonical.
fn canonical_block(block_hash: [u8; 32]) -> serde_json::Value {
    serde_json::json!({"blockHash":format!("0x{}", hex::encode(block_hash)),"requireCanonical":true})
}

fn quantity_word(value: &str) -> Result<[u8; 32], RpcError> {
    let raw = value
        .strip_prefix("0x")
        .ok_or(RpcError::Evidence("invalid account quantity"))?;
    if raw.is_empty()
        || raw.len() > 64
        || (raw.len() > 1 && raw.starts_with('0'))
        || !raw
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(RpcError::Evidence("invalid account quantity"));
    }
    let number = num_bigint::BigUint::parse_bytes(raw.as_bytes(), 16)
        .ok_or(RpcError::Evidence("invalid account quantity"))?;
    let bytes = number.to_bytes_be();
    let mut word = [0; 32];
    word[32 - bytes.len()..].copy_from_slice(&bytes);
    Ok(word)
}

fn fixed_hex<const N: usize>(value: &str) -> Result<[u8; N], RpcError> {
    let raw = value
        .strip_prefix("0x")
        .ok_or(RpcError::Evidence("missing hex prefix"))?;
    if raw.len() != 2 * N {
        return Err(RpcError::Evidence("wrong hex width"));
    }
    let bytes = hex::decode(raw).map_err(|_| RpcError::Evidence("invalid hex"))?;
    bytes
        .try_into()
        .map_err(|_| RpcError::Evidence("wrong hex width"))
}

fn quantity(value: &str) -> Result<u64, RpcError> {
    let raw = value
        .strip_prefix("0x")
        .ok_or(RpcError::Evidence("missing quantity prefix"))?;
    if raw.is_empty() || (raw.len() > 1 && raw.starts_with('0')) {
        return Err(RpcError::Evidence("noncanonical quantity"));
    }
    u64::from_str_radix(raw, 16).map_err(|_| RpcError::Evidence("quantity overflow"))
}

fn decode_log(log: &RpcLog, topics: &[[u8; 32]; 3]) -> Result<PoolEvent, RpcError> {
    let topic = fixed_hex::<32>(
        log.topics
            .first()
            .ok_or(RpcError::Evidence("missing event topic"))?,
    )?;
    let tx_hash = fixed_hex::<32>(&log.transaction_hash)?;
    let log_index: u32 = quantity(&log.log_index)?
        .try_into()
        .map_err(|_| RpcError::Evidence("log index overflow"))?;
    if topic == topics[0] {
        if log.topics.len() != 3 {
            return Err(RpcError::Evidence("insertion topic count"));
        }
        let index = fixed_hex::<32>(&log.topics[1])?;
        if index[..28].iter().any(|byte| *byte != 0) {
            return Err(RpcError::Evidence("leaf index overflow"));
        }
        let index = u32::from_be_bytes(
            index[28..]
                .try_into()
                .map_err(|_| RpcError::Evidence("index"))?,
        );
        Ok(PoolEvent::Inserted {
            index,
            commitment: fixed_hex::<32>(&log.topics[2])?,
            root: fixed_hex::<32>(&log.data)?,
            tx_hash,
            log_index,
        })
    } else if topic == topics[1] {
        if log.topics.len() != 4 || log.data != "0x" {
            return Err(RpcError::Evidence("transfer event shape"));
        }
        Ok(PoolEvent::Transferred {
            deal_commitment: fixed_hex::<32>(&log.topics[1])?,
            deal_nullifier: fixed_hex::<32>(&log.topics[2])?,
            input_nullifier: fixed_hex::<32>(&log.topics[3])?,
            tx_hash,
            log_index,
        })
    } else if topic == topics[2] {
        if log.topics.len() != 3 {
            return Err(RpcError::Evidence("withdrawal topic count"));
        }
        fixed_hex::<32>(&log.topics[2])?;
        fixed_hex::<32>(&log.data)?;
        Ok(PoolEvent::Consumed {
            nullifier: fixed_hex::<32>(&log.topics[1])?,
            tx_hash,
            log_index,
        })
    } else {
        Err(RpcError::Evidence("unknown pool event"))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn account_quantities_are_canonical_and_preserve_all_256_bits() {
        assert_eq!(super::quantity_word("0x0").unwrap(), [0; 32]);
        assert_eq!(
            super::quantity_word(&format!("0x{}", "f".repeat(64))).unwrap(),
            [255; 32]
        );
        for value in ["0x", "0x00", "0X1", "0xgg", "0x-1", "0x 1", "0xA"] {
            assert!(super::quantity_word(value).is_err());
        }
        assert!(super::quantity_word(&format!("0x1{}", "0".repeat(64))).is_err());
    }

    async fn retry_rpc(
        failures: usize,
        status: u16,
        payload: serde_json::Value,
    ) -> (
        super::PoolRpc,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        use axum::{http::StatusCode, routing::post, Json, Router};
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let rpc = super::PoolRpc::new(
            &format!("http://{}", listener.local_addr().unwrap()),
            10143,
            [1; 20],
        )
        .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route(
                    "/",
                    post(move || {
                        let count = counter.fetch_add(1, Ordering::SeqCst);
                        let payload = payload.clone();
                        async move {
                            (
                                if count < failures {
                                    StatusCode::from_u16(status).unwrap()
                                } else {
                                    StatusCode::OK
                                },
                                Json(payload),
                            )
                        }
                    }),
                ),
            )
            .await
            .unwrap();
        });
        (rpc, calls, server)
    }

    #[tokio::test]
    async fn transient_read_retries_are_bounded_and_never_retry_writes_or_invalid_evidence() {
        use super::RpcError;
        use serde_json::json;
        use std::sync::atomic::Ordering;
        for status in [429, 502, 503, 504] {
            let (rpc, calls, server) = retry_rpc(2, status, json!({"result":"0x279f"})).await;
            rpc.check_chain().await.unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 3);
            server.abort();
        }
        let (rpc, calls, server) = retry_rpc(99, 429, json!({})).await;
        assert!(matches!(
            rpc.check_chain().await,
            Err(RpcError::Request("eth_chainId"))
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        server.abort();

        for (status, payload) in [
            (400, json!({})),
            (
                200,
                json!({"error":{"code":-32602,"message":"invalid params"}}),
            ),
            (200, json!({"result":null})),
            (200, json!({"result":123})),
        ] {
            let (rpc, calls, server) = retry_rpc(99, status, payload).await;
            assert!(rpc.check_chain().await.is_err());
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            server.abort();
        }
        let (rpc, calls, server) = retry_rpc(1, 429, json!({"result":"0x1"})).await;
        assert!(matches!(
            rpc.check_chain().await,
            Err(RpcError::Evidence("wrong chain ID"))
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        server.abort();

        let (rpc, calls, server) = retry_rpc(99, 429, json!({})).await;
        assert!(rpc
            .call::<String>("eth_sendRawTransaction", json!([]))
            .await
            .is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        server.abort();
    }

    async fn snapshot_rpc(
        head: u64,
        finalized: Option<u64>,
        conflicting_height: Option<u64>,
        change_after_finality_read: bool,
    ) -> (super::PoolRpc, tokio::task::JoinHandle<()>) {
        use axum::{routing::post, Json, Router};
        use serde_json::{json, Value};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let finalized_reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let server = tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/", post(move |Json(request): Json<Value>| {
                let finalized_reads = finalized_reads.clone();
                async move {
                let block = |number: u64| {
                    let mut hash = [0x11; 32];
                    hash[..8].copy_from_slice(&number.to_be_bytes());
                    if conflicting_height == Some(number) { hash[31] = 0x33; }
                    if change_after_finality_read && finalized == Some(number)
                        && request["params"][0] != "finalized"
                        && finalized_reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0
                    { hash[31] = 0x44; }
                    json!({"number":format!("0x{number:x}"),"hash":format!("0x{}",hex::encode(hash)),
                        "parentHash":format!("0x{}", "22".repeat(32)),"timestamp":format!("0x{:x}",number+100)})
                };
                let result = match request["method"].as_str().unwrap() {
                    "eth_chainId" => json!("0x7a69"),
                    "eth_blockNumber" => json!(format!("0x{head:x}")),
                    "eth_getBlockByNumber" if request["params"][0] == "finalized" => finalized.map_or(Value::Null, block),
                    "eth_getBlockByNumber" => block(u64::from_str_radix(request["params"][0].as_str().unwrap().trim_start_matches("0x"),16).unwrap()),
                    _ => panic!("unexpected snapshot RPC method"),
                };
                Json(json!({"jsonrpc":"2.0","id":request["id"],"result":result}))
                }
            }))).await.unwrap();
        });
        (super::PoolRpc::new(&url, 31337, [3; 20]).unwrap(), server)
    }

    #[tokio::test]
    async fn shared_snapshot_accepts_honest_tip_lag_but_rejects_conflicts_and_missing_finality() {
        let (first, first_server) = snapshot_rpc(12, Some(10), None, false).await;
        let (peer, peer_server) = snapshot_rpc(13, Some(11), None, false).await;
        assert_ne!(
            first.finalized_head().await.unwrap(),
            peer.finalized_head().await.unwrap()
        );
        let snapshot = first.shared_snapshot(&peer).await.unwrap();
        assert_eq!(snapshot.finalized.number, 10);
        assert_eq!(snapshot.head_number, 12);
        assert_eq!(
            snapshot.head_hash,
            first.canonical_block_hash(12).await.unwrap()
        );
        for (finalized, conflict) in [(Some(11), Some(10)), (Some(11), Some(12)), (None, None)] {
            let (bad, server) = snapshot_rpc(13, finalized, conflict, false).await;
            assert!(first.shared_snapshot(&bad).await.is_err());
            server.abort();
        }
        assert!(first.shared_snapshot(&first).await.is_err());
        first_server.abort();
        peer_server.abort();
    }

    #[tokio::test]
    async fn shared_snapshot_rejects_both_providers_changing_the_reported_finalized_block() {
        let (first, first_server) = snapshot_rpc(12, Some(10), None, true).await;
        let (peer, peer_server) = snapshot_rpc(13, Some(10), None, true).await;
        assert!(matches!(
            first.shared_snapshot(&peer).await,
            Err(super::RpcError::Evidence(
                "finalized anchor changed during snapshot selection"
            ))
        ));
        first_server.abort();
        peer_server.abort();
    }
    #[test]
    fn endpoint_identity_ignores_fragments_and_normalizes_the_root_path() {
        let first = super::PoolRpc::new("http://127.0.0.1:9", 31_337, [3; 20]).unwrap();
        let peer =
            super::PoolRpc::new("http://127.0.0.1:9/#not-a-second-provider", 31_337, [3; 20])
                .unwrap();
        assert_eq!(first.endpoint(), peer.endpoint());
        assert!(first.matches_endpoint("http://127.0.0.1:9/#ignored"));
    }

    use super::*;

    #[test]
    fn decodes_fixed_pool_events_and_rejects_wrong_shapes() {
        let topics = [
            event_topic("NoteInserted(uint256,uint256,uint256)"),
            event_topic("DealTransferred(uint256,uint256,uint256)"),
            event_topic("NoteWithdrawn(uint256,address,uint256)"),
        ];
        let mut log = RpcLog {
            address: format!("0x{}", hex::encode([2; 20])),
            block_number: "0x64".to_owned(),
            block_hash: format!("0x{}", hex::encode([3; 32])),
            transaction_hash: format!("0x{}", hex::encode([4; 32])),
            log_index: "0x0".to_owned(),
            topics: vec![
                format!("0x{}", hex::encode(topics[0])),
                format!("0x{:064x}", 0),
                format!("0x{}", hex::encode([5; 32])),
            ],
            data: format!("0x{}", hex::encode([6; 32])),
            removed: Some(false),
        };
        assert!(matches!(
            decode_log(&log, &topics),
            Ok(PoolEvent::Inserted { index: 0, .. })
        ));
        log.topics[1] = format!("0x{:064x}", u64::MAX);
        assert!(decode_log(&log, &topics).is_err());
        log.topics[0] = format!("0x{}", hex::encode(topics[1]));
        log.topics.push(format!("0x{}", hex::encode([7; 32])));
        log.data = "0x".to_owned();
        assert!(matches!(
            decode_log(&log, &topics),
            Ok(PoolEvent::Transferred {
                deal_nullifier,
                input_nullifier,
                ..
            }) if deal_nullifier == [5; 32] && input_nullifier == [7; 32]
        ));
    }

    #[test]
    fn quantities_require_canonical_hex() {
        assert_eq!(quantity("0x0").expect("zero"), 0);
        assert_eq!(quantity("0x10").expect("sixteen"), 16);
        assert!(quantity("0x00").is_err());
        assert!(quantity("16").is_err());
    }
}
