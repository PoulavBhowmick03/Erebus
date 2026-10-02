//! Read-only EVM JSON-RPC source for public shielded-pool events.
//!
//! It transmits no note openings. A response is checked against block headers and then
//! against the local Poseidon tree before the wallet may use it.

use reqwest::{Client, Url};
use serde::de::DeserializeOwned;
use serde::Deserialize;
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BlockHeader {
    number: String,
    hash: String,
    parent_hash: String,
    timestamp: String,
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

/// A configured read-only source for one EVM pool deployment.
pub struct PoolRpc {
    url: Url,
    client: Client,
    chain_id: u64,
    pool: [u8; 20],
}

impl PoolRpc {
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
        })
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: &'static str,
        params: serde_json::Value,
    ) -> Result<T, RpcError> {
        let response = self
            .client
            .post(self.url.clone())
            .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
            .send()
            .await
            .map_err(|_| RpcError::Request(method))?;
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
        decoded.result.ok_or(RpcError::Request(method))
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
        if root != index.root() || next as usize != index.leaves().len() {
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
        let before = self.header(number).await?;
        let hash = fixed_hex::<32>(&before.hash)?;
        let parent_hash = fixed_hex::<32>(&before.parent_hash)?;
        let topics = [
            event_topic("NoteInserted(uint256,uint256,uint256)"),
            event_topic("DealTransferred(uint256,uint256,uint256)"),
            event_topic("NoteWithdrawn(uint256,address,uint256)"),
        ];
        let logs: Vec<RpcLog> = self.call("eth_getLogs", serde_json::json!([{
            "address": format!("0x{}", hex::encode(self.pool)),
            "fromBlock": format!("0x{number:x}"),
            "toBlock": format!("0x{number:x}"),
            "topics": [topics.iter().map(|topic| format!("0x{}", hex::encode(topic))).collect::<Vec<_>>()]
        }])).await?;
        let mut events = Vec::with_capacity(logs.len());
        for log in logs {
            if fixed_hex::<20>(&log.address)? != self.pool
                || quantity(&log.block_number)? != number
                || fixed_hex::<32>(&log.block_hash)? != hash
                || log.removed.unwrap_or(false)
            {
                return Err(RpcError::Evidence("log block or pool mismatch"));
            }
            events.push(decode_log(&log, &topics)?);
        }
        let after = self.header(number).await?;
        if fixed_hex::<32>(&after.hash)? != hash {
            return Err(RpcError::Evidence("block changed while logs were read"));
        }
        Ok(PoolBlock {
            number,
            hash,
            parent_hash,
            events,
        })
    }

    /// Rewinds a changed tip and appends at most `limit` consecutive canonical blocks.
    ///
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
        for number in start..=end {
            let block = self.read_block(number).await?;
            staged.apply_block(block)?;
        }
        self.verify_pool_state(&staged).await?;
        *index = staged;
        Ok(end - start + 1)
    }
}

fn event_topic(signature: &str) -> [u8; 32] {
    Keccak256::digest(signature.as_bytes()).into()
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
