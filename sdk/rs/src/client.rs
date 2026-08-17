//! High-level Rust client implementing the MVP interface.
//!
//! Public methods use opaque handles and offer ids. Pool and channel secrets live in
//! [`crate::state::StateStore`], while the two private signing values are read from local
//! files for each operation and dropped when the call returns.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use starknet_types_core::felt::Felt;

use crate::actions::{FeltEntropy, RandomSalt};
use crate::channel::{
    ChangeChannelSetup, ChangeOutput, Channel, ChannelError, Counterparty, OwnedNote, PoolIdentity,
    SetupParams,
};
use crate::decrypt;
use crate::disclosure::{self, ViewingGrant};
use crate::doctor::{Check, Report};
use crate::erc20::{self, Erc20Error};
use crate::execution::{ExecutionConfig, ExecutionError, Executor};
use crate::hashes;
use crate::negotiation::{
    Author, NegotiationError, OfferBook, OfferId as InternalOfferId, OfferStatus as InternalStatus,
};
use crate::prover::{BlockId, ProverError, ProvingService};
use crate::read::{reconstruct, ChannelReader, ReadError};
use crate::rpc::{RpcError, StarknetRpc};
use crate::state::{ChannelHandle, StateError, StateStore, StoredChannel};
use crate::subchannel::SubchannelCursor;
use crate::wire::{MessageType, WireMessage, WireVersion};

const MAX_DISCOVERY_ITEMS: u64 = 4096;
const MAX_SELECTION_NOTES: usize = 256;
const MAX_SELECTION_STATES: usize = 100_000;

/// Construction inputs for the high-level client.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Operator-trusted Starknet RPC URL.
    ///
    /// The `compile_actions` preflight sends the pool private key as calldata, so a public
    /// third-party RPC is not a safe configuration for writes.
    pub rpc_url: String,
    /// Operator-controlled proving-service URL.
    pub prover_url: String,
    /// Privacy-pool contract.
    pub pool_address: Felt,
    /// Chain id felt.
    pub chain_id: Felt,
    /// Agent account and pool identity address.
    pub account_address: Felt,
    /// Path to the pool identity/viewing private key.
    pub pool_key_file: PathBuf,
    /// Path to the account transaction-signing private key.
    pub account_key_file: PathBuf,
    /// Rust-owned state directory.
    pub state_dir: PathBuf,
    /// The one token supported by the MVP client instance.
    pub token: Felt,
}

/// Concrete Rust implementation.
#[derive(Debug, Clone)]
pub struct Client {
    config: ClientConfig,
    executor: Executor,
    state: StateStore,
}

impl Client {
    /// Builds the client without touching either key file.
    pub fn new(config: ClientConfig) -> Result<Self, ClientError> {
        let rpc = StarknetRpc::new(config.rpc_url.clone())?;
        let prover = ProvingService::new(config.prover_url.clone())?;
        let execution =
            ExecutionConfig::new(config.pool_address, config.chain_id, config.account_address);
        let state = StateStore::new(&config.state_dir)?;
        Ok(Self {
            config,
            executor: Executor::new(rpc, prover, execution),
            state,
        })
    }

    /// Funds the identity with one exact-value private note.
    ///
    /// This administrative helper is outside the seven negotiation methods. The first call
    /// opens the self-channel. Later calls append a note because the channel marker is
    /// `WriteOnce` and reopening it reverts. See
    /// [`Channel::deposit_into_open_channel`] and friction.md F32.
    pub async fn shield(&self, amount: u128) -> Result<SettlementReceipt, ClientError> {
        if amount == 0 {
            return Err(ClientError::InvalidRequest(
                "shield amount must be non-zero".to_owned(),
            ));
        }
        let (identity, pool_key, account_key) = self.identity_keys()?;
        let registered = self.registered_public_key(identity.address()).await?;
        self.verify_own_registration(&identity, registered)?;
        let counterparty = Counterparty {
            address: identity.address(),
            public_key: identity.public_key(),
        };
        let channel = Channel::derive(
            self.config.chain_id,
            self.config.pool_address,
            &identity,
            counterparty,
        );

        // Choose the funding path from chain state. Another machine can shield the identity,
        // and a wrong path fails after proof generation and fee payment.
        let self_channel_key = hashes::compute_channel_key(
            identity.address(),
            pool_key,
            identity.address(),
            identity.public_key(),
        );
        let already_open = self.self_channel_open(&identity, self_channel_key).await?;

        let actions = if already_open {
            // Use the proof anchor for the index. A newer block can contain a slot that
            // `head - proving_block_lag` cannot see.
            let block = self.executor.wait_until_provable(0).await?;
            let note_index = self
                .next_free_note_index(self_channel_key, self.config.token, &block)
                .await?;
            channel.deposit_into_open_channel(
                self.config.token,
                note_index,
                amount,
                random_salt(),
            )?
        } else {
            let channel_index = self.outgoing_channel_count(&identity, pool_key).await?;
            channel.shield(
                &identity,
                SetupParams {
                    register: (registered == Felt::ZERO).then(entropy),
                    channel_index,
                    channel_random: entropy(),
                    channel_salt: entropy(),
                    subchannel_index: 0,
                    token: self.config.token,
                    subchannel_salt: entropy(),
                },
                amount,
                random_salt(),
            )?
        };
        let receipt = self
            .executor
            .execute(identity.address(), pool_key, account_key, &actions)
            .await?;
        Ok(SettlementReceipt {
            offer_id: None,
            tx_hash: hex(receipt.transaction_hash),
            nullifiers: Vec::new(),
            proved_at: receipt.proving_block,
            // A shield creates a note from public funds. It selects nothing and returns
            // nothing, so these are absent rather than zero.
            selected_input: None,
            change: None,
        })
    }

    /// Inspects configuration, identity, and chain state before anything is spent.
    ///
    /// Read-only and never returns `Err` for a failed inspection: a `doctor` that stops at the
    /// first problem makes an operator repair and rerun once per fault. Transport failures are
    /// recorded as checks, so one run reports everything wrong at once.
    ///
    /// Ordered so that a failure explains the skips beneath it. Files first because they need
    /// no network, then the endpoints, then the pool, then this identity's position in it.
    pub async fn doctor(&self) -> Report {
        let mut checks = Vec::new();

        checks.push(check_key_file(&self.config.pool_key_file, "pool_key_file"));
        checks.push(check_key_file(
            &self.config.account_key_file,
            "account_key_file",
        ));
        checks.push(check_state_dir(&self.config.state_dir));

        let rpc_live = match self.executor.rpc().block_number().await {
            Ok(block) => {
                checks.push(Check::pass(
                    "rpc",
                    format!("reachable, head is block {block}"),
                ));
                true
            }
            Err(error) => {
                checks.push(Check::fail(
                    "rpc",
                    format!("unreachable: {error}"),
                    "check STARKNET_RPC_URL. Writes need an operator-controlled node, because \
                     compile_actions sends the pool key as calldata",
                ));
                false
            }
        };

        checks.push(match self.executor.prover().spec_version().await {
            Ok(version) => Check::pass("prover", format!("reachable, spec {version}")),
            Err(error) => Check::fail(
                "prover",
                format!("unreachable: {error}"),
                "check PROVING_SERVICE_URL. Without a prover there is no proof, and \
                 apply_actions reverts on EMPTY_PROOF_FACTS",
            ),
        });

        if !rpc_live {
            for name in [
                "chain_id",
                "pool",
                "registration",
                "allowance",
                "gas_balance",
            ] {
                checks.push(Check::skipped(name, "needs a reachable RPC"));
            }
            return Report { checks };
        }

        checks.push(self.check_chain_id().await);
        checks.push(self.check_pool().await);
        checks.push(self.check_registration().await);

        let (allowance, gas) = self.check_funding().await;
        checks.push(allowance);
        checks.push(gas);

        Report { checks }
    }

    async fn check_chain_id(&self) -> Check {
        match self.executor.rpc().chain_id().await {
            Err(error) => Check::skipped("chain_id", format!("could not read: {error}")),
            Ok(live) if live == self.config.chain_id => {
                Check::pass("chain_id", format!("{live:#x}, matches configuration"))
            }
            Ok(live) => Check::fail(
                "chain_id",
                format!(
                    "configured {:#x} but the RPC serves {live:#x}",
                    self.config.chain_id
                ),
                "point STARKNET_RPC_URL at the configured chain, or fix STARKNET_CHAIN_ID. \
                 The chain id is part of every channel-key preimage, so a mismatch derives \
                 slots nobody wrote to and every read returns not-found",
            ),
        }
    }

    async fn check_pool(&self) -> Check {
        let version = self.view("get_version", &[], &BlockId::Latest).await;
        let Ok(version) = version.and_then(|values| one_felt("get_version", &values)) else {
            return Check::fail(
                "pool",
                format!("no pool answered at {:#x}", self.config.pool_address),
                "check POOL_ADDRESS against the deployment for this chain",
            );
        };
        let validity = self
            .view("get_proof_validity_blocks", &[], &BlockId::Latest)
            .await
            .and_then(|values| one_felt("get_proof_validity_blocks", &values))
            .map(|felt| felt.to_string())
            .unwrap_or_else(|_| "unknown".to_owned());
        Check::pass(
            "pool",
            format!(
                "live at {:#x}, version felt {version:#x}, proof validity {validity} blocks",
                self.config.pool_address
            ),
        )
    }

    async fn check_registration(&self) -> Check {
        let Ok((identity, _)) = self.pool_identity() else {
            return Check::skipped("registration", "the pool key file could not be read");
        };
        match self.registered_public_key(identity.address()).await {
            Err(error) => Check::skipped("registration", format!("could not read: {error}")),
            Ok(registered) if registered == Felt::ZERO => Check::fail(
                "registration",
                "this address has no pool public key".to_owned(),
                "run `shield` once. Registration only happens folded into an action set, and \
                 an unregistered address cannot be a channel counterparty",
            ),
            Ok(registered) if registered != identity.public_key() => Check::fail(
                "registration",
                format!(
                    "the pool holds {registered:#x} for this address, but the key file derives \
                     {:#x}",
                    identity.public_key()
                ),
                "point POOL_KEY_FILE at the key this address registered with. Registration is \
                 write-once and cannot be replaced",
            ),
            Ok(_) => Check::pass("registration", "registered, and the key file agrees"),
        }
    }

    /// Allowance and public gas balance together, because both are read from the same token
    /// and both fail the same write for reasons an operator will confuse otherwise.
    async fn check_funding(&self) -> (Check, Check) {
        let report = match self.pool_allowance().await {
            Ok(report) => report,
            Err(error) => {
                return (
                    Check::skipped("allowance", format!("could not read: {error}")),
                    Check::skipped("gas_balance", "the token could not be read"),
                );
            }
        };

        let allowance = if report.fee_per_write == 0 {
            Check::pass(
                "allowance",
                format!(
                    "{} granted; this pool charges no fee, so only deposits consume it",
                    report.allowance
                ),
            )
        } else if !report.covers(1, 0) {
            Check::fail(
                "allowance",
                format!(
                    "{} granted, below the {} fee this pool charges per write",
                    report.allowance, report.fee_per_write
                ),
                "run the `approve` method, sized for the writes you plan plus any deposit. \
                 Without it apply_actions reverts inside collect_fee with a bare Contract error",
            )
        } else if !report.covers(5, 0) {
            Check::warn(
                "allowance",
                format!(
                    "{} granted, about {} writes at the current {} fee",
                    report.allowance,
                    report.allowance / report.fee_per_write,
                    report.fee_per_write
                ),
                "top up with `approve` before a long run. approve replaces the standing \
                 allowance rather than adding to it",
            )
        } else {
            Check::pass(
                "allowance",
                format!(
                    "{} granted, about {} writes at the current {} fee",
                    report.allowance,
                    report.allowance / report.fee_per_write,
                    report.fee_per_write
                ),
            )
        };

        let gas = match self
            .executor
            .rpc()
            .call_contract(
                self.config.token,
                "balanceOf",
                &erc20::balance_of_calldata(self.config.account_address),
                &BlockId::Latest,
            )
            .await
            .map_err(ClientError::from)
            .and_then(|values| Ok(erc20::parse_u256("balanceOf", &values)?))
        {
            Err(error) => Check::skipped("gas_balance", format!("could not read: {error}")),
            Ok(0) => Check::fail(
                "gas_balance",
                "this account holds no public token balance".to_owned(),
                "fund the account. The allowance is permission to pull, not funds to pull \
                 from, and gas is paid from the same balance",
            ),
            Ok(balance) if balance < report.fee_per_write.saturating_mul(3) => Check::warn(
                "gas_balance",
                format!("{balance} public balance, a few writes at the current fee"),
                "fund the account before a long run",
            ),
            Ok(balance) => Check::pass("gas_balance", format!("{balance} public balance")),
        };

        (allowance, gas)
    }

    /// Grants the pool a standing STRK allowance against this account.
    ///
    /// Required before any charged `apply_actions`, because the pool pulls both deposits and
    /// its own fee with `transfer_from` against the caller. See [`crate::erc20`] for why, and
    /// friction.md F20 for what the missing allowance looks like from the outside.
    ///
    /// The spender is always the configured pool. There is no parameter for it: an agent that
    /// could name its own spender could be talked into approving one, and no Erebus flow needs
    /// to approve anything else.
    ///
    /// This overwrites any existing allowance rather than adding to it, which is what ERC-20
    /// `approve` does. Read [`Self::pool_allowance`] first if the current value matters.
    pub async fn approve_pool(&self, amount: u128) -> Result<ApprovalReceipt, ClientError> {
        let account_key = read_key(&self.config.account_key_file, "account key")?;
        let calldata = erc20::approve_calldata(self.config.pool_address, amount);
        let receipt = self
            .executor
            .submit_call(account_key, self.config.token, "approve", &calldata)
            .await?;
        Ok(ApprovalReceipt {
            tx_hash: hex(receipt.transaction_hash),
            approved: amount,
        })
    }

    /// The pool's current STRK allowance against this account, and the fee it charges.
    ///
    /// Both are live reads. The fee is pool storage that `set_fee_amount` can change, and it
    /// already differs by network, so nothing should hard-code it.
    pub async fn pool_allowance(&self) -> Result<AllowanceReport, ClientError> {
        let allowance = self
            .executor
            .rpc()
            .call_contract(
                self.config.token,
                "allowance",
                &erc20::allowance_calldata(self.config.account_address, self.config.pool_address),
                &BlockId::Latest,
            )
            .await?;
        // `get_fee_amount` returns a bare `u128`, not a `u256`.
        let fee = self.view("get_fee_amount", &[], &BlockId::Latest).await?;
        let fee = one_felt("get_fee_amount", &fee)?;

        Ok(AllowanceReport {
            allowance: erc20::parse_u256("allowance", &allowance)?,
            fee_per_write: u128::try_from(fee).map_err(|_| {
                ClientError::Protocol(format!("get_fee_amount returned {fee:#x}, wider than u128"))
            })?,
        })
    }

    /// The unspent note denominations this identity holds, largest first.
    ///
    /// Settlement can select inputs above the price and return payer-owned change. Any
    /// positive amount up to the spendable total is payable. Denominations show the selected
    /// input value and possible change.
    ///
    /// A new note is not immediately spendable. Settlement simulates at
    /// `head - proving_block_lag`, so the result separates spendable and newer notes.
    pub async fn note_balance(&self) -> Result<NoteBalance, ClientError> {
        let (_, pool_key) = self.pool_identity()?;
        let provable = self.executor.wait_until_provable(0).await?;

        let mut spendable = self.note_amounts(pool_key, &provable).await?;
        let mut visible = self.note_amounts(pool_key, &BlockId::Latest).await?;

        // Multiset difference: whatever the chain shows that the proving block does not.
        for amount in &spendable {
            if let Some(at) = visible.iter().position(|candidate| candidate == amount) {
                visible.remove(at);
            }
        }
        spendable.sort_unstable_by(|a, b| b.cmp(a));
        visible.sort_unstable_by(|a, b| b.cmp(a));
        Ok(NoteBalance {
            spendable,
            pending: visible,
        })
    }

    async fn note_amounts(
        &self,
        pool_key: Felt,
        block: &BlockId,
    ) -> Result<Vec<u128>, ClientError> {
        let notes = self
            .discover_owned_notes(pool_key, self.config.token, block)
            .await?;
        Ok(notes.iter().map(|note| note.amount).collect())
    }

    fn identity_keys(&self) -> Result<(PoolIdentity, Felt, Felt), ClientError> {
        let (identity, pool_key) = self.pool_identity()?;
        let account_key = read_key(&self.config.account_key_file, "account key")?;
        Ok((identity, pool_key, account_key))
    }

    fn pool_identity(&self) -> Result<(PoolIdentity, Felt), ClientError> {
        let pool_key = read_key(&self.config.pool_key_file, "pool key")?;
        Ok((
            PoolIdentity::new(self.config.account_address, pool_key),
            pool_key,
        ))
    }

    async fn registered_public_key(&self, address: Felt) -> Result<Felt, ClientError> {
        let result = self
            .view("get_public_key", &[address], &BlockId::Latest)
            .await?;
        one_felt("get_public_key", &result)
    }

    fn verify_own_registration(
        &self,
        identity: &PoolIdentity,
        registered: Felt,
    ) -> Result<(), ClientError> {
        if registered != Felt::ZERO && registered != identity.public_key() {
            return Err(ClientError::IdentityMismatch {
                address: identity.address(),
                expected: identity.public_key(),
                registered,
            });
        }
        Ok(())
    }

    /// Whether this identity's self-channel marker has already been claimed.
    ///
    /// The marker is `WriteOnce`, and the channel key has no index. This read determines
    /// whether the identity was shielded before proof generation.
    async fn self_channel_open(
        &self,
        identity: &PoolIdentity,
        self_channel_key: Felt,
    ) -> Result<bool, ClientError> {
        self.self_channel_open_at(identity, self_channel_key, &BlockId::Latest)
            .await
    }

    async fn self_channel_open_at(
        &self,
        identity: &PoolIdentity,
        self_channel_key: Felt,
        block: &BlockId,
    ) -> Result<bool, ClientError> {
        let marker = hashes::compute_channel_marker(
            self_channel_key,
            identity.address(),
            identity.address(),
            identity.public_key(),
        );
        let result = self.view("channel_exists", &[marker], block).await?;
        Ok(one_felt("channel_exists", &result)? != Felt::ZERO)
    }

    /// The first empty note slot in `channel_key`'s subchannel for `token`.
    ///
    /// Notes must be contiguous. Like discovery, this stops at the first empty slot. A gap
    /// hides all later notes.
    async fn next_free_note_index(
        &self,
        channel_key: Felt,
        token: Felt,
        block: &BlockId,
    ) -> Result<u32, ClientError> {
        for note_index in 0..MAX_DISCOVERY_ITEMS {
            let note_id = hashes::compute_note_id(channel_key, token, note_index);
            let stored = self.view("get_note", &[note_id], block).await?;
            require_len("get_note", &stored, 2)?;
            if stored[0] == Felt::ZERO {
                return u32::try_from(note_index)
                    .map_err(|_| ClientError::Protocol("note index exceeds u32".to_owned()));
            }
        }
        Err(ClientError::DiscoveryLimit("notes"))
    }

    async fn outgoing_channel_count(
        &self,
        identity: &PoolIdentity,
        pool_key: Felt,
    ) -> Result<u32, ClientError> {
        self.outgoing_channel_count_at(identity, pool_key, &BlockId::Latest)
            .await
    }

    async fn outgoing_channel_count_at(
        &self,
        identity: &PoolIdentity,
        pool_key: Felt,
        block: &BlockId,
    ) -> Result<u32, ClientError> {
        for index in 0..MAX_DISCOVERY_ITEMS {
            let id = hashes::compute_outgoing_channel_id(identity.address(), pool_key, index);
            let result = self.view("get_outgoing_channel_info", &[id], block).await?;
            require_len("get_outgoing_channel_info", &result, 2)?;
            if result[0] == Felt::ZERO {
                return u32::try_from(index).map_err(|_| {
                    ClientError::Protocol("outgoing channel index exceeds u32".to_owned())
                });
            }
        }
        Err(ClientError::DiscoveryLimit("outgoing channels"))
    }

    async fn sync_book(
        &self,
        state: &StoredChannel,
    ) -> Result<(OfferBook, HashMap<Felt, Felt>, u32), ClientError> {
        let outgoing = ChannelReader::with_version(
            self.config.chain_id,
            self.config.pool_address,
            state.outgoing_key,
            state.token,
            state.wire_version,
        );
        let mut source = HashMap::new();
        let outgoing_next = self.fetch_notes(outgoing, state.token, &mut source).await?;

        let incoming_reader = state
            .incoming_key
            .map(|key| {
                ChannelReader::with_version(
                    self.config.chain_id,
                    self.config.pool_address,
                    key,
                    state.token,
                    state.wire_version,
                )
            })
            .unwrap_or_else(|| {
                ChannelReader::with_version(
                    self.config.chain_id,
                    self.config.pool_address,
                    Felt::ZERO,
                    state.token,
                    state.wire_version,
                )
            });
        if state.incoming_key.is_some() {
            self.fetch_notes(incoming_reader, state.token, &mut source)
                .await?;
        }
        let note_source = |id: Felt| source.get(&id).copied();
        let book = reconstruct(&outgoing, &incoming_reader, &note_source)?;
        Ok((book, source, outgoing_next))
    }

    async fn fetch_notes(
        &self,
        reader: ChannelReader,
        token: Felt,
        source: &mut HashMap<Felt, Felt>,
    ) -> Result<u32, ClientError> {
        for index in 0..MAX_DISCOVERY_ITEMS {
            let id = reader.note_id(index);
            let result = self.view("get_note", &[id], &BlockId::Latest).await?;
            require_len("get_note", &result, 2)?;
            if result[0] == Felt::ZERO {
                return u32::try_from(index)
                    .map_err(|_| ClientError::Protocol("note index exceeds u32".to_owned()));
            }
            check_note_token(result[0], result[1], token)?;
            source.insert(id, result[0]);
        }
        Err(ClientError::DiscoveryLimit("notes"))
    }

    async fn attach_reverse_channel(
        &self,
        state: &mut StoredChannel,
        pool_key: Felt,
    ) -> Result<(), ClientError> {
        if state.incoming_key.is_some() {
            return Ok(());
        }
        let claimed: HashSet<Felt> = self.state.claimed_incoming_keys()?.into_iter().collect();
        let count = self
            .view(
                "get_num_of_channels",
                &[self.config.account_address],
                &BlockId::Latest,
            )
            .await?;
        let count = felt_u64(
            "get_num_of_channels",
            one_felt("get_num_of_channels", &count)?,
        )?;
        let mut candidates = Vec::new();

        for index in 0..count {
            let result = self
                .view(
                    "get_channel_info",
                    &[self.config.account_address, Felt::from(index)],
                    &BlockId::Latest,
                )
                .await?;
            require_len("get_channel_info", &result, 3)?;
            let info = decrypt::channel_info(result[0], result[1], result[2], &pool_key)?;
            if info.sender_addr != state.counterparty_address
                || claimed.contains(&info.channel_key)
                || !self
                    .channel_has_token(info.channel_key, state.token)
                    .await?
            {
                continue;
            }
            candidates.push(info.channel_key);
        }

        match candidates.as_slice() {
            [key] => {
                state.incoming_key = Some(*key);
                Ok(())
            }
            [] => Err(ClientError::ChannelNotReady),
            _ => Err(ClientError::AmbiguousReverseChannel(candidates.len())),
        }
    }

    async fn channel_has_token(&self, channel_key: Felt, token: Felt) -> Result<bool, ClientError> {
        for index in 0..MAX_DISCOVERY_ITEMS {
            let id = hashes::compute_subchannel_id(channel_key, index);
            let result = self
                .view("get_subchannel_info", &[id], &BlockId::Latest)
                .await?;
            require_len("get_subchannel_info", &result, 2)?;
            if result[0] == Felt::ZERO {
                return Ok(false);
            }
            if decrypt::subchannel_token(result[1], result[0], channel_key, index) == token {
                return Ok(true);
            }
        }
        Err(ClientError::DiscoveryLimit("subchannels"))
    }

    async fn discover_owned_notes(
        &self,
        pool_key: Felt,
        token: Felt,
        block: &BlockId,
    ) -> Result<Vec<ValueNote>, ClientError> {
        let count = self
            .view("get_num_of_channels", &[self.config.account_address], block)
            .await?;
        let count = felt_u64(
            "get_num_of_channels",
            one_felt("get_num_of_channels", &count)?,
        )?;
        let mut notes = Vec::new();

        for channel_index in 0..count {
            let encoded = self
                .view(
                    "get_channel_info",
                    &[self.config.account_address, Felt::from(channel_index)],
                    block,
                )
                .await?;
            require_len("get_channel_info", &encoded, 3)?;
            let channel = decrypt::channel_info(encoded[0], encoded[1], encoded[2], &pool_key)?;

            for subchannel_index in 0..MAX_DISCOVERY_ITEMS {
                let id = hashes::compute_subchannel_id(channel.channel_key, subchannel_index);
                let encoded = self.view("get_subchannel_info", &[id], block).await?;
                require_len("get_subchannel_info", &encoded, 2)?;
                if encoded[0] == Felt::ZERO {
                    break;
                }
                let found_token = decrypt::subchannel_token(
                    encoded[1],
                    encoded[0],
                    channel.channel_key,
                    subchannel_index,
                );
                if found_token != token {
                    continue;
                }

                for note_index in 0..MAX_DISCOVERY_ITEMS {
                    let note_id = hashes::compute_note_id(channel.channel_key, token, note_index);
                    let stored = self.view("get_note", &[note_id], block).await?;
                    require_len("get_note", &stored, 2)?;
                    if stored[0] == Felt::ZERO {
                        break;
                    }
                    check_note_token(stored[0], stored[1], token)?;
                    let note =
                        decrypt::packed_value(stored[0], channel.channel_key, token, note_index);
                    if note.amount == 0 {
                        continue;
                    }
                    let nullifier =
                        hashes::compute_nullifier(channel.channel_key, token, note_index, pool_key);
                    let spent = self.view("nullifier_exists", &[nullifier], block).await?;
                    if one_felt("nullifier_exists", &spent)? != Felt::ZERO {
                        continue;
                    }
                    notes.push(ValueNote {
                        note: OwnedNote {
                            channel_key: channel.channel_key,
                            token,
                            index: u32::try_from(note_index).map_err(|_| {
                                ClientError::Protocol("note index exceeds u32".to_owned())
                            })?,
                        },
                        amount: note.amount,
                        nullifier,
                    });
                }
            }
        }
        Ok(notes)
    }

    async fn view(
        &self,
        entrypoint: &str,
        calldata: &[Felt],
        block: &BlockId,
    ) -> Result<Vec<Felt>, ClientError> {
        Ok(self
            .executor
            .rpc()
            .call_contract(self.config.pool_address, entrypoint, calldata, block)
            .await?)
    }
}

/// Frozen negotiation surface. Granting returns a bearer viewing grant for delivery to the
/// grantee.
#[allow(async_fn_in_trait)]
pub trait ErebusClient {
    /// Establishes and submits a private channel.
    async fn open_channel(&self, counterparty: Felt) -> Result<ChannelHandle, ClientError>;
    /// Writes an offer.
    async fn propose_offer(
        &self,
        handle: ChannelHandle,
        terms: OfferTerms,
    ) -> Result<OfferId, ClientError>;
    /// Writes a counter-offer.
    async fn counter_offer(
        &self,
        handle: ChannelHandle,
        reply_to: OfferId,
        terms: OfferTerms,
    ) -> Result<OfferId, ClientError>;
    /// Reads the complete visible negotiation state.
    async fn read_channel_state(&self, handle: ChannelHandle) -> Result<ChannelState, ClientError>;
    /// Accepts the counterparty's offer and settles in one action set.
    async fn accept_and_settle(
        &self,
        handle: ChannelHandle,
        offer_id: OfferId,
    ) -> Result<SettlementReceipt, ClientError>;
    /// Exports the scoped bearer viewing grant.
    async fn grant_viewing_key(
        &self,
        handle: ChannelHandle,
        grantee: Felt,
    ) -> Result<ViewingKeyGrant, ClientError>;
    /// Reconstructs a disclosed record from chain data.
    async fn reveal(&self, viewing_key: ViewingKeyGrant) -> Result<DisclosedRecord, ClientError>;
}

impl ErebusClient for Client {
    async fn open_channel(&self, counterparty_address: Felt) -> Result<ChannelHandle, ClientError> {
        let (identity, pool_key, account_key) = self.identity_keys()?;
        let registered = self.registered_public_key(identity.address()).await?;
        self.verify_own_registration(&identity, registered)?;
        let counterparty_public_key = self.registered_public_key(counterparty_address).await?;
        if counterparty_public_key == Felt::ZERO {
            return Err(ClientError::CounterpartyUnregistered(counterparty_address));
        }

        // A pair has one channel because the key has no index (`hashes.cairo:119-124`) and
        // its marker is `WriteOnce`. Reopening returns `Contract error` after preflight,
        // proving, and fee payment. Reuse the handle for idempotent retries. See F29.
        if let Some(existing) = self.state.find_channel(
            self.config.chain_id,
            self.config.pool_address,
            identity.address(),
            counterparty_address,
            self.config.token,
        )? {
            return Ok(existing);
        }

        let channel_index = self.outgoing_channel_count(&identity, pool_key).await?;
        let counterparty = Counterparty {
            address: counterparty_address,
            public_key: counterparty_public_key,
        };
        let channel = Channel::derive(
            self.config.chain_id,
            self.config.pool_address,
            &identity,
            counterparty,
        );
        let actions = channel.setup(
            &identity,
            SetupParams {
                register: (registered == Felt::ZERO).then(entropy),
                channel_index,
                channel_random: entropy(),
                channel_salt: entropy(),
                subchannel_index: 0,
                token: self.config.token,
                subchannel_salt: entropy(),
            },
        )?;
        let receipt = self
            .executor
            .execute(identity.address(), pool_key, account_key, &actions)
            .await?;
        let tx_hash = receipt.transaction_hash;
        let opened_block = accepted_block(&receipt)?;
        let handle = self.state.create(|handle| {
            StoredChannel::new(
                handle,
                self.config.chain_id,
                self.config.pool_address,
                identity.address(),
                counterparty_address,
                counterparty_public_key,
                self.config.token,
                channel.key(),
                channel_index,
                0,
                tx_hash,
                opened_block,
            )
        })?;
        Ok(handle)
    }

    async fn propose_offer(
        &self,
        handle: ChannelHandle,
        terms: OfferTerms,
    ) -> Result<OfferId, ClientError> {
        validate_terms(&terms)?;
        let (identity, pool_key, account_key) = self.identity_keys()?;
        let mut lease = self.state.lock(&handle)?;
        validate_owner(lease.state(), identity.address())?;
        validate_scope(lease.state(), &self.config)?;
        validate_token(lease.state(), terms.token)?;
        if lease.state().settled {
            return Err(ClientError::AlreadySettled);
        }

        self.executor
            .wait_until_provable(lease.state().last_write_block)
            .await?;
        let (_, _, chain_next) = self.sync_book(lease.state()).await?;
        let mut cursor = SubchannelCursor::resume_at(chain_next);
        let state = lease.state();
        let channel = Channel::from_key_with_version(
            self.config.chain_id,
            self.config.pool_address,
            state.outgoing_key,
            Counterparty {
                address: state.counterparty_address,
                public_key: state.counterparty_public_key,
            },
            state.wire_version,
        );
        let message = WireMessage {
            message_type: MessageType::Offer,
            reply_to: None,
            created_at: now()?,
            amount: terms.amount,
            deadline: terms.deadline,
            memo_hash: terms.memo_hash,
        };
        let (index, actions) = channel.write_next_message(state.token, &mut cursor, &message)?;
        let receipt = self
            .executor
            .execute(identity.address(), pool_key, account_key, &actions)
            .await?;
        lease.state_mut().outgoing_next_note = cursor.next_index();
        lease.state_mut().last_write_block = accepted_block(&receipt)?;
        lease.commit()?;
        Ok(external_offer_id(&handle, Author::Us, index))
    }

    async fn counter_offer(
        &self,
        handle: ChannelHandle,
        reply_to: OfferId,
        terms: OfferTerms,
    ) -> Result<OfferId, ClientError> {
        validate_terms(&terms)?;
        let target = parse_offer_id(&handle, &reply_to)?;
        if target.author != Author::Counterparty {
            return Err(ClientError::NotCounterpartyOffer);
        }
        let (identity, pool_key, account_key) = self.identity_keys()?;
        let mut lease = self.state.lock(&handle)?;
        validate_owner(lease.state(), identity.address())?;
        validate_scope(lease.state(), &self.config)?;
        validate_token(lease.state(), terms.token)?;
        self.attach_reverse_channel(lease.state_mut(), pool_key)
            .await?;
        self.executor
            .wait_until_provable(lease.state().last_write_block)
            .await?;
        let (book, _, chain_next) = self.sync_book(lease.state()).await?;
        let target_message = book
            .entries()
            .find(|(id, _)| *id == target)
            .map(|(_, message)| message)
            .ok_or(NegotiationError::UnknownOffer {
                index: target.index,
            })?;
        if !matches!(
            target_message.message_type,
            MessageType::Offer | MessageType::Counter
        ) {
            return Err(ClientError::NotCounterpartyOffer);
        }

        let state = lease.state();
        let channel = Channel::from_key_with_version(
            self.config.chain_id,
            self.config.pool_address,
            state.outgoing_key,
            Counterparty {
                address: state.counterparty_address,
                public_key: state.counterparty_public_key,
            },
            state.wire_version,
        );
        let mut cursor = SubchannelCursor::resume_at(chain_next);
        let message = WireMessage {
            message_type: MessageType::Counter,
            reply_to: Some(target.index),
            created_at: now()?,
            amount: terms.amount,
            deadline: terms.deadline,
            memo_hash: terms.memo_hash,
        };
        let (index, actions) = channel.write_next_message(state.token, &mut cursor, &message)?;
        let receipt = self
            .executor
            .execute(identity.address(), pool_key, account_key, &actions)
            .await?;
        lease.state_mut().incoming_key = state.incoming_key;
        lease.state_mut().outgoing_next_note = cursor.next_index();
        lease.state_mut().last_write_block = accepted_block(&receipt)?;
        lease.commit()?;
        Ok(external_offer_id(&handle, Author::Us, index))
    }

    async fn read_channel_state(&self, handle: ChannelHandle) -> Result<ChannelState, ClientError> {
        let (identity, pool_key) = self.pool_identity()?;
        let mut lease = self.state.lock(&handle)?;
        validate_owner(lease.state(), identity.address())?;
        validate_scope(lease.state(), &self.config)?;
        // A one-sided channel is readable before the reverse direction opens. Suppress only
        // the not-ready result. Multiple candidates remain an error.
        if let Err(error) = self
            .attach_reverse_channel(lease.state_mut(), pool_key)
            .await
        {
            if !matches!(error, ClientError::ChannelNotReady) {
                return Err(error);
            }
        }
        let (book, _, chain_next) = self.sync_book(lease.state()).await?;
        let result = channel_state(&handle, lease.state(), &book, now()?);
        lease.state_mut().outgoing_next_note = chain_next;
        lease.state_mut().settled = result.settled;
        lease.commit()?;
        Ok(result)
    }

    async fn accept_and_settle(
        &self,
        handle: ChannelHandle,
        offer_id: OfferId,
    ) -> Result<SettlementReceipt, ClientError> {
        let target = parse_offer_id(&handle, &offer_id)?;
        let (identity, pool_key, account_key) = self.identity_keys()?;
        let mut lease = self.state.lock(&handle)?;
        validate_owner(lease.state(), identity.address())?;
        validate_scope(lease.state(), &self.config)?;
        if lease.state().settled {
            return Err(ClientError::AlreadySettled);
        }
        self.attach_reverse_channel(lease.state_mut(), pool_key)
            .await?;
        let spend_block = self
            .executor
            .wait_until_provable(lease.state().last_write_block)
            .await?;
        let (book, _, chain_next) = self.sync_book(lease.state()).await?;
        let decision_time = now()?;
        book.check_acceptable(target, decision_time)?;
        let offer = book
            .entries()
            .find(|(id, _)| *id == target)
            .map(|(_, message)| message)
            .ok_or(NegotiationError::UnknownOffer {
                index: target.index,
            })?;

        let available = self
            .discover_owned_notes(pool_key, lease.state().token, &spend_block)
            .await?;
        let selected = select_notes(&available, offer.amount).ok_or_else(|| {
            let mut held: Vec<u128> = available.iter().map(|note| note.amount).collect();
            held.sort_unstable_by(|a, b| b.cmp(a));
            ClientError::InsufficientNotes {
                required: offer.amount,
                total: held
                    .iter()
                    .fold(0u128, |total, amount| total.saturating_add(*amount)),
                held,
            }
        })?;
        let spend: Vec<OwnedNote> = selected.notes.iter().map(|note| note.note).collect();
        let change = if selected.change == 0 {
            None
        } else {
            let self_counterparty = Counterparty {
                address: identity.address(),
                public_key: identity.public_key(),
            };
            let self_channel = Channel::derive(
                self.config.chain_id,
                self.config.pool_address,
                &identity,
                self_counterparty,
            );
            let self_channel_key = self_channel.key();
            if self
                .self_channel_open_at(&identity, self_channel_key, &spend_block)
                .await?
            {
                let change_index = self
                    .next_free_note_index(self_channel_key, lease.state().token, &spend_block)
                    .await?;
                Some(ChangeOutput::existing(
                    self_channel,
                    selected.change,
                    change_index,
                    random_salt(),
                ))
            } else {
                let channel_index = self
                    .outgoing_channel_count_at(&identity, pool_key, &spend_block)
                    .await?;
                Some(ChangeOutput::opening(
                    self_channel,
                    selected.change,
                    random_salt(),
                    ChangeChannelSetup {
                        channel_index,
                        channel_random: entropy(),
                        channel_salt: entropy(),
                        subchannel_index: 0,
                        subchannel_salt: entropy(),
                    },
                ))
            }
        };

        let state = lease.state();
        let channel = Channel::from_key_with_version(
            self.config.chain_id,
            self.config.pool_address,
            state.outgoing_key,
            Counterparty {
                address: state.counterparty_address,
                public_key: state.counterparty_public_key,
            },
            state.wire_version,
        );
        let mut cursor = SubchannelCursor::resume_at(chain_next);
        let acceptance = WireMessage {
            message_type: MessageType::Accept,
            reply_to: Some(target.index),
            created_at: decision_time,
            amount: offer.amount,
            deadline: offer.deadline,
            memo_hash: offer.memo_hash,
        };
        let (_, actions) = channel.settle_next_with_change(
            state.token,
            &mut cursor,
            &spend,
            offer.amount,
            random_salt(),
            &acceptance,
            change,
        )?;
        let receipt = self
            .executor
            .execute(identity.address(), pool_key, account_key, &actions)
            .await?;
        lease.state_mut().outgoing_next_note = cursor.next_index();
        lease.state_mut().last_write_block = accepted_block(&receipt)?;
        lease.state_mut().settled = true;
        lease.commit()?;

        // Report from the same selection the action set was built from, not from a fresh
        // read. A later balance query anchors at a different block and would answer a
        // different question.
        let selected_input = selected
            .notes
            .iter()
            .fold(0u128, |total, note| total.saturating_add(note.amount));

        Ok(SettlementReceipt {
            offer_id: Some(offer_id),
            tx_hash: hex(receipt.transaction_hash),
            nullifiers: selected
                .notes
                .iter()
                .map(|note| hex(note.nullifier))
                .collect(),
            proved_at: receipt.proving_block,
            selected_input: Some(selected_input.to_string()),
            change: Some(selected.change.to_string()),
        })
    }

    async fn grant_viewing_key(
        &self,
        handle: ChannelHandle,
        grantee: Felt,
    ) -> Result<ViewingKeyGrant, ClientError> {
        if grantee == Felt::ZERO {
            return Err(ClientError::InvalidRequest(
                "grantee public key must be non-zero".to_owned(),
            ));
        }
        let (identity, pool_key) = self.pool_identity()?;
        let mut lease = self.state.lock(&handle)?;
        validate_owner(lease.state(), identity.address())?;
        validate_scope(lease.state(), &self.config)?;
        self.attach_reverse_channel(lease.state_mut(), pool_key)
            .await?;
        let state = lease.state();
        let channel = Channel::from_key_with_version(
            self.config.chain_id,
            self.config.pool_address,
            state.outgoing_key,
            Counterparty {
                address: state.counterparty_address,
                public_key: state.counterparty_public_key,
            },
            state.wire_version,
        );
        let viewing_key = channel.grant_viewing_key(
            &identity,
            state.incoming_key.ok_or(ClientError::ChannelNotReady)?,
            state.token,
        );
        lease.commit()?;
        Ok(ViewingKeyGrant {
            channel_id: handle,
            grantee,
            viewing_key,
        })
    }

    async fn reveal(&self, grant: ViewingKeyGrant) -> Result<DisclosedRecord, ClientError> {
        if let Some((chain_id, pool_address)) = grant.viewing_key.authenticated_scope() {
            if chain_id != self.config.chain_id || pool_address != self.config.pool_address {
                return Err(ClientError::InvalidRequest(
                    "viewing grant chain/pool does not match client config".to_owned(),
                ));
            }
        }
        let (outgoing, incoming) = grant.viewing_key.readers();
        let mut source = HashMap::new();
        self.fetch_notes(outgoing, grant.viewing_key.token, &mut source)
            .await?;
        self.fetch_notes(incoming, grant.viewing_key.token, &mut source)
            .await?;
        let note_source = |id: Felt| source.get(&id).copied();
        let record = disclosure::reveal(&grant.viewing_key, &note_source, now()?)?;
        Ok(disclosed_record(&grant.channel_id, record))
    }
}

/// Offer terms represented by the canonical negotiation wire.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct OfferTerms {
    /// Token base units.
    pub amount: u128,
    /// Must match this client's configured token.
    pub token: Felt,
    /// Unix seconds.
    pub deadline: u64,
    /// 128-bit commitment to off-chain detail.
    pub memo_hash: u128,
}

/// Opaque offer identifier, scoped to a channel handle and direction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct OfferId(String);

impl OfferId {
    /// Transport representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Public status vocabulary.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OfferStatus {
    /// Live proposal.
    Proposed,
    /// Has a later reply.
    Countered,
    /// Acceptance and payment landed atomically.
    Settled,
    /// Deadline passed.
    Expired,
}

/// One reconstructed offer-state entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Offer {
    /// Opaque id.
    pub offer_id: OfferId,
    /// Owning channel handle.
    pub channel_id: ChannelHandle,
    /// Author address.
    pub proposer: Felt,
    /// Cross-direction parent.
    pub reply_to: Option<OfferId>,
    /// Terms carried by the message.
    pub terms: OfferTerms,
    /// Derived status.
    pub status: OfferStatus,
    /// Unix creation time.
    pub created_at: u64,
}

/// Current negotiation state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelState {
    /// Opaque channel handle.
    pub channel_id: ChannelHandle,
    /// Local and counterparty addresses.
    pub participants: [Felt; 2],
    /// Every visible message.
    pub offers: Vec<Offer>,
    /// Whether an acceptance/payment has landed.
    pub settled: bool,
}

/// Result of granting the pool an allowance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalReceipt {
    /// The `approve` transaction.
    pub tx_hash: String,
    /// The allowance now standing, in token base units.
    pub approved: u128,
}

/// What the pool may currently pull, and what each write costs.
///
/// `allowance` covers deposits and fees together, because both are `transfer_from` against the
/// same account. A settlement that also shields therefore needs headroom for both.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AllowanceReport {
    /// Standing allowance granted to the pool.
    pub allowance: u128,
    /// Live `get_fee_amount`. Zero on a pool that charges nothing.
    pub fee_per_write: u128,
}

impl AllowanceReport {
    /// Whether the standing allowance covers `writes` charged calls plus `deposits` of value.
    pub fn covers(&self, writes: u32, deposits: u128) -> bool {
        self.fee_per_write
            .checked_mul(u128::from(writes))
            .and_then(|fees| fees.checked_add(deposits))
            .is_some_and(|needed| self.allowance >= needed)
    }
}

/// What an identity can pay now, and what it will be able to pay shortly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NoteBalance {
    /// Note denominations spendable at the proving block, largest first.
    pub spendable: Vec<u128>,
    /// Notes on chain but newer than the proving block, so not yet spendable.
    pub pending: Vec<u128>,
}

/// Submitted settlement or administrative shielding receipt.
///
/// Amounts are decimal strings, not numbers. A `u128` of token base units routinely exceeds
/// the JSON safe-integer range: one STRK is 1e18 and 2^53 is about 9.007e15, so any JavaScript
/// consumer between here and the agent would round the value without erroring. `NoteBalance`
/// already crosses the CLI boundary as strings for the same reason.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SettlementReceipt {
    /// Settled offer; absent for administrative shielding.
    pub offer_id: Option<OfferId>,
    /// Starknet transaction hash.
    pub tx_hash: String,
    /// Spent note nullifiers.
    pub nullifiers: Vec<String>,
    /// Historical block the proof was anchored to.
    pub proved_at: u64,
    /// Total value of the notes this settlement spent. Absent for shielding, which spends
    /// nothing. Always at least the paid amount, because notes are indivisible.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_input: Option<String>,
    /// Value returned to the payer as a change note, `"0"` when the selected notes summed
    /// exactly to the price. Absent for shielding.
    ///
    /// Absent and `"0"` mean different things: absent is "no selection happened", zero is
    /// "a selection happened and left nothing over". Collapsing them would tell an agent that
    /// a shield produced exact change.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<String>,
}

/// Intentional secret export for out-of-band delivery.
#[derive(Clone, Serialize, Deserialize)]
pub struct ViewingKeyGrant {
    /// Opaque record id carried to the auditor; no local state lookup is needed.
    pub channel_id: ChannelHandle,
    /// Intended recipient public key. This is metadata in MVP v1, not encryption.
    pub grantee: Felt,
    /// Bearer secret. Anyone holding it can read this one relationship and token.
    pub viewing_key: ViewingGrant,
}

impl core::fmt::Debug for ViewingKeyGrant {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ViewingKeyGrant")
            .field("channel_id", &self.channel_id)
            .field("grantee", &self.grantee)
            .field("viewing_key", &"<redacted>")
            .finish()
    }
}

/// Disclosed record shaped for the high-level API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DisclosedRecord {
    /// Local opaque handle.
    pub channel_id: ChannelHandle,
    /// Both parties.
    pub participants: [Felt; 2],
    /// Token covered by the grant.
    pub token: Felt,
    /// Reconstructed messages.
    pub offers: Vec<Offer>,
    /// Reconstructed settlement consistency.
    pub settlement: Option<DisclosedSettlement>,
}

/// Settlement evidence visible to a grant holder.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DisclosedSettlement {
    /// Acceptance message.
    pub acceptance: OfferId,
    /// Offer it accepted.
    pub accepted_offer: Option<OfferId>,
    /// Terms amount.
    pub agreed_amount: u128,
    /// Decrypted payment.
    pub paid_amount: Option<u128>,
}

#[derive(Debug, Clone, Copy)]
struct ValueNote {
    note: OwnedNote,
    amount: u128,
    nullifier: Felt,
}

#[derive(Debug)]
struct NoteSelection {
    notes: Vec<ValueNote>,
    change: u128,
}

/// Chooses a sufficient subset with the least surplus found by its bounded search.
fn select_notes(notes: &[ValueNote], target: u128) -> Option<NoteSelection> {
    if target == 0 {
        return Some(NoteSelection {
            notes: Vec::new(),
            change: 0,
        });
    }
    let mut notes = notes.to_vec();
    notes.sort_by_key(|note| std::cmp::Reverse(note.amount));
    if notes
        .iter()
        .fold(0u128, |total, note| total.saturating_add(note.amount))
        < target
    {
        return None;
    }

    let search_len = notes.len().min(MAX_SELECTION_NOTES);
    let mut sums: BTreeMap<u128, Vec<usize>> = BTreeMap::from([(0, Vec::new())]);
    let mut best: Option<(u128, Vec<usize>)> = None;
    for (index, note) in notes[..search_len].iter().enumerate() {
        let snapshot: Vec<(u128, Vec<usize>)> = sums
            .iter()
            .map(|(sum, picks)| (*sum, picks.clone()))
            .collect();
        for (sum, mut picks) in snapshot {
            let Some(next) = sum.checked_add(note.amount) else {
                continue;
            };
            picks.push(index);
            if next >= target {
                let replace = best.as_ref().is_none_or(|(best_sum, best_picks)| {
                    next < *best_sum || (next == *best_sum && picks.len() < best_picks.len())
                });
                if replace {
                    best = Some((next, picks));
                }
                continue;
            }
            if !sums.contains_key(&next) && sums.len() < MAX_SELECTION_STATES {
                sums.insert(next, picks);
            }
        }
    }

    let (selected_total, picks) = match best {
        Some(selection) => selection,
        None => {
            let mut total = 0u128;
            let mut picks = Vec::new();
            for (index, note) in notes.iter().enumerate() {
                total = total.saturating_add(note.amount);
                picks.push(index);
                if total >= target {
                    break;
                }
            }
            (total, picks)
        }
    };
    Some(NoteSelection {
        notes: picks.into_iter().map(|pick| notes[pick]).collect(),
        change: selected_total - target,
    })
}

fn accepted_block(receipt: &crate::execution::ExecutionReceipt) -> Result<u64, ClientError> {
    receipt.receipt.block_number.ok_or_else(|| {
        ClientError::Protocol("accepted transaction receipt omitted its block number".to_owned())
    })
}

fn channel_state(
    handle: &ChannelHandle,
    state: &StoredChannel,
    book: &OfferBook,
    now: u64,
) -> ChannelState {
    let offers = book
        .entries()
        .map(|(id, message)| offer(handle, state, book, id, message, now))
        .collect();
    ChannelState {
        channel_id: handle.clone(),
        participants: [state.owner, state.counterparty_address],
        offers,
        settled: state.settled
            || book
                .entries()
                .any(|(_, message)| message.message_type == MessageType::Accept),
    }
}

fn offer(
    handle: &ChannelHandle,
    state: &StoredChannel,
    book: &OfferBook,
    id: InternalOfferId,
    message: WireMessage,
    now: u64,
) -> Offer {
    Offer {
        offer_id: external_offer_id(handle, id.author, id.index),
        channel_id: handle.clone(),
        proposer: match id.author {
            Author::Us => state.owner,
            Author::Counterparty => state.counterparty_address,
        },
        reply_to: message
            .reply_to
            .map(|index| external_offer_id(handle, id.author.opposite(), index)),
        terms: OfferTerms {
            amount: message.amount,
            token: state.token,
            deadline: message.deadline,
            memo_hash: message.memo_hash,
        },
        status: match book.status(id, now).expect("entry came from this book") {
            InternalStatus::Proposed => OfferStatus::Proposed,
            InternalStatus::Countered => OfferStatus::Countered,
            InternalStatus::Settled => OfferStatus::Settled,
            InternalStatus::Expired => OfferStatus::Expired,
        },
        created_at: message.created_at,
    }
}

fn disclosed_record(
    handle: &ChannelHandle,
    record: disclosure::DisclosedRecord,
) -> DisclosedRecord {
    let offers = record
        .messages
        .iter()
        .map(|entry| Offer {
            offer_id: external_offer_id(handle, entry.id.author, entry.id.index),
            channel_id: handle.clone(),
            proposer: entry.author_addr,
            reply_to: entry
                .message
                .reply_to
                .map(|index| external_offer_id(handle, entry.id.author.opposite(), index)),
            terms: OfferTerms {
                amount: entry.message.amount,
                token: record.token,
                deadline: entry.message.deadline,
                memo_hash: entry.message.memo_hash,
            },
            status: match entry.status {
                InternalStatus::Proposed => OfferStatus::Proposed,
                InternalStatus::Countered => OfferStatus::Countered,
                InternalStatus::Settled => OfferStatus::Settled,
                InternalStatus::Expired => OfferStatus::Expired,
            },
            created_at: entry.message.created_at,
        })
        .collect();
    let settlement = record.settlement.map(|settlement| DisclosedSettlement {
        acceptance: external_offer_id(
            handle,
            settlement.acceptance.author,
            settlement.acceptance.index,
        ),
        accepted_offer: settlement
            .accepted_offer
            .map(|id| external_offer_id(handle, id.author, id.index)),
        agreed_amount: settlement.agreed_amount,
        paid_amount: settlement.paid_amount,
    });
    DisclosedRecord {
        channel_id: handle.clone(),
        participants: record.participants,
        token: record.token,
        offers,
        settlement,
    }
}

fn external_offer_id(handle: &ChannelHandle, author: Author, index: u32) -> OfferId {
    let direction = match author {
        Author::Us => "us",
        Author::Counterparty => "them",
    };
    OfferId(format!("{}:{direction}:{index}", handle.as_str()))
}

fn parse_offer_id(
    handle: &ChannelHandle,
    offer_id: &OfferId,
) -> Result<InternalOfferId, ClientError> {
    let prefix = format!("{}:", handle.as_str());
    let suffix = offer_id
        .as_str()
        .strip_prefix(&prefix)
        .ok_or_else(|| ClientError::InvalidOfferId(offer_id.0.clone()))?;
    let (direction, index) = suffix
        .split_once(':')
        .ok_or_else(|| ClientError::InvalidOfferId(offer_id.0.clone()))?;
    let author = match direction {
        "us" => Author::Us,
        "them" => Author::Counterparty,
        _ => return Err(ClientError::InvalidOfferId(offer_id.0.clone())),
    };
    let index = index
        .parse()
        .map_err(|_| ClientError::InvalidOfferId(offer_id.0.clone()))?;
    Ok(InternalOfferId::new(author, index))
}

fn validate_terms(terms: &OfferTerms) -> Result<(), ClientError> {
    if terms.amount == 0 {
        return Err(ClientError::InvalidRequest(
            "offer amount must be non-zero".to_owned(),
        ));
    }
    if terms.deadline < now()? {
        return Err(ClientError::InvalidRequest(
            "offer deadline is already past".to_owned(),
        ));
    }
    Ok(())
}

fn validate_owner(state: &StoredChannel, owner: Felt) -> Result<(), ClientError> {
    if state.owner != owner {
        return Err(ClientError::InvalidRequest(
            "channel belongs to a different identity".to_owned(),
        ));
    }
    Ok(())
}

fn validate_token(state: &StoredChannel, token: Felt) -> Result<(), ClientError> {
    if state.token != token {
        return Err(ClientError::TokenMismatch {
            expected: state.token,
            received: token,
        });
    }
    Ok(())
}

fn validate_scope(state: &StoredChannel, config: &ClientConfig) -> Result<(), ClientError> {
    if state.wire_version == WireVersion::V2
        && (state.chain_id != config.chain_id || state.pool_address != config.pool_address)
    {
        return Err(ClientError::InvalidRequest(
            "channel state chain/pool does not match client config".to_owned(),
        ));
    }
    Ok(())
}

/// Validates the `token` field the pool returns alongside a stored note.
///
/// The field is meaningful only for open notes. For encrypted notes, `privacy.cairo:664`
/// says *"Only `packed_value` needs to be written to storage, `token` is initialized to
/// zero"*. `objects.cairo:98` also says *"the token address of the note (zero for encrypted
/// notes)"*. A channel note's token
/// is implied by the subchannel it was found in and cannot be recovered from the note.
///
/// An open note must contain the requested token. An encrypted note must contain zero. A
/// non-zero value on an encrypted note means that its derived id found an open note.
///
/// A previous equality check rejected every valid message note. See friction.md F28.
fn check_note_token(packed: Felt, stored: Felt, expected: Felt) -> Result<(), ClientError> {
    let (salt, _) = decrypt::unpack_note(packed);
    if salt == decrypt::OPEN_NOTE_SALT {
        if stored != expected {
            return Err(ClientError::Protocol(format!(
                "open note carries token {stored:#x}, expected {expected:#x}"
            )));
        }
    } else if stored != Felt::ZERO {
        return Err(ClientError::Protocol(format!(
            "encrypted note carries token {stored:#x}, expected zero. \
             The derived note id landed on an open note"
        )));
    }
    Ok(())
}

/// Inspects one private-key file: present, parseable, and not readable by other users.
///
/// Permissions are checked because a key file is only a secret while the filesystem says so,
/// and a mode that widened during a copy or a container build is invisible until it matters.
fn check_key_file(path: &PathBuf, name: &'static str) -> Check {
    let display = path.display();
    let Ok(text) = std::fs::read_to_string(path) else {
        return Check::fail(
            name,
            format!("{display} cannot be read"),
            format!("point {name} at an existing key file, or create one with generate_pool_key"),
        );
    };
    if Felt::from_hex(text.trim()).is_err() {
        return Check::fail(
            name,
            format!("{display} does not contain a hex felt"),
            format!("{name} must hold a single 0x-prefixed felt and nothing else"),
        );
    }
    match file_mode(path) {
        Some(mode) if mode & 0o077 != 0 => Check::warn(
            name,
            format!(
                "{display} is mode {:o}, readable beyond its owner",
                mode & 0o777
            ),
            format!("chmod 600 {display}"),
        ),
        _ => Check::pass(name, format!("{display} present and owner-only")),
    }
}

fn check_state_dir(path: &PathBuf) -> Check {
    let display = path.display();
    if !path.is_dir() {
        return Check::warn(
            "state_dir",
            format!("{display} does not exist yet"),
            "no action needed; the first write creates it with mode 0700".to_owned(),
        );
    }
    match file_mode(path) {
        Some(mode) if mode & 0o077 != 0 => Check::warn(
            "state_dir",
            format!(
                "{display} is mode {:o}, open beyond its owner",
                mode & 0o777
            ),
            format!(
                "chmod 700 {display}. Channel state is not key material, but it names \
                     counterparties and amounts"
            ),
        ),
        _ => Check::pass("state_dir", format!("{display} present and owner-only")),
    }
}

#[cfg(unix)]
fn file_mode(path: &PathBuf) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .ok()
        .map(|data| data.permissions().mode())
}

/// Windows has no mode bits, so the permission arm of these checks is skipped rather than
/// guessed at. Presence and parseability still apply.
#[cfg(not(unix))]
fn file_mode(_path: &PathBuf) -> Option<u32> {
    None
}

fn read_key(path: &PathBuf, kind: &'static str) -> Result<Felt, ClientError> {
    let text = std::fs::read_to_string(path).map_err(|source| ClientError::KeyFile {
        kind,
        path: path.clone(),
        source,
    })?;
    Felt::from_hex(text.trim()).map_err(|_| ClientError::InvalidKey {
        kind,
        path: path.clone(),
    })
}

fn entropy() -> FeltEntropy {
    loop {
        let mut bytes = [0u8; 31];
        OsRng.fill_bytes(&mut bytes);
        if let Ok(value) = FeltEntropy::new(Felt::from_bytes_be_slice(&bytes)) {
            return value;
        }
    }
}

fn random_salt() -> RandomSalt {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    RandomSalt::from_entropy(bytes)
}

fn now() -> Result<u64, ClientError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| ClientError::ClockBeforeEpoch)
}

fn one_felt(entrypoint: &'static str, values: &[Felt]) -> Result<Felt, ClientError> {
    require_len(entrypoint, values, 1)?;
    Ok(values[0])
}

fn require_len(
    entrypoint: &'static str,
    values: &[Felt],
    expected: usize,
) -> Result<(), ClientError> {
    if values.len() != expected {
        return Err(ClientError::Protocol(format!(
            "{entrypoint} returned {} felts, expected {expected}",
            values.len()
        )));
    }
    Ok(())
}

fn felt_u64(field: &'static str, value: Felt) -> Result<u64, ClientError> {
    let bytes = value.to_bytes_be();
    if bytes[..24].iter().any(|byte| *byte != 0) {
        return Err(ClientError::Protocol(format!("{field} exceeds u64")));
    }
    Ok(u64::from_be_bytes(
        bytes[24..].try_into().expect("eight-byte suffix"),
    ))
}

fn hex(value: Felt) -> String {
    format!("{value:#x}")
}

/// High-level client failure.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Caller input was malformed.
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    /// An ERC-20 return value was malformed.
    #[error(transparent)]
    Erc20(#[from] Erc20Error),
    /// A key file could not be read.
    #[error("cannot read {kind} file {}: {source}", path.display())]
    KeyFile {
        /// Key role.
        kind: &'static str,
        /// File path.
        path: PathBuf,
        /// I/O error.
        source: std::io::Error,
    },
    /// Key file contents were not a felt.
    #[error("{kind} file {} does not contain a felt", path.display())]
    InvalidKey {
        /// Key role.
        kind: &'static str,
        /// File path.
        path: PathBuf,
    },
    /// Pool registration disagrees with the supplied pool key.
    #[error(
        "identity {address:#x} is registered as {registered:#x}, \
         but the supplied pool key derives {expected:#x}"
    )]
    IdentityMismatch {
        /// Identity.
        address: Felt,
        /// Locally derived public key.
        expected: Felt,
        /// On-chain public key.
        registered: Felt,
    },
    /// Counterparty cannot receive a channel.
    #[error("counterparty {0:#x} has no registered pool public key")]
    CounterpartyUnregistered(Felt),
    /// Reverse direction has not been opened yet.
    #[error("counterparty has not opened the reverse channel yet")]
    ChannelNotReady,
    /// More than one unclaimed reverse direction exists.
    #[error("{0} unclaimed reverse channels match this counterparty and token")]
    AmbiguousReverseChannel(usize),
    /// Caller supplied a token different from the configured subchannel.
    #[error("offer token {received:#x} does not match channel token {expected:#x}")]
    TokenMismatch {
        /// Channel token.
        expected: Felt,
        /// Caller token.
        received: Felt,
    },
    /// Offer id is malformed or belongs to another channel.
    #[error("invalid or foreign offer id: {0}")]
    InvalidOfferId(String),
    /// A counter or settlement targeted our own/non-offer message.
    #[error("the target is not a counterparty offer")]
    NotCounterpartyOffer,
    /// State already records terminal settlement.
    #[error("this channel is already settled")]
    AlreadySettled,
    /// Available private notes do not cover the payment.
    ///
    /// Includes holdings so an agent can calculate and fund the shortfall.
    #[error(
        "unspent notes do not cover {required}; holding {} note(s) worth {total} in total ({})",
        held.len(),
        if held.is_empty() { "none".to_owned() } else { held.iter().map(u128::to_string).collect::<Vec<_>>().join(", ") }
    )]
    InsufficientNotes {
        /// Required amount.
        required: u128,
        /// Unspent note denominations, largest first.
        held: Vec<u128>,
        /// Sum of `held`.
        total: u128,
    },
    /// A keyed discovery run exceeded its defensive cap.
    #[error("discovery limit exceeded while reading {0}")]
    DiscoveryLimit(&'static str),
    /// Local clock is unusable.
    #[error("system clock is before the Unix epoch")]
    ClockBeforeEpoch,
    /// Successful RPC response contradicted the ABI.
    #[error("privacy-pool protocol mismatch: {0}")]
    Protocol(String),
    /// State-store failure.
    #[error(transparent)]
    State(#[from] StateError),
    /// RPC failure.
    #[error(transparent)]
    Rpc(#[from] RpcError),
    /// Prover construction failure.
    #[error(transparent)]
    Prover(#[from] ProverError),
    /// Action construction failure.
    #[error(transparent)]
    Channel(#[from] ChannelError),
    /// Execution pipeline failure.
    #[error(transparent)]
    Execution(#[from] ExecutionError),
    /// Channel-info decryption failure.
    #[error(transparent)]
    Decrypt(#[from] decrypt::DecryptError),
    /// Transcript read failure.
    #[error(transparent)]
    Read(#[from] ReadError),
    /// Offer-state failure.
    #[error(transparent)]
    Negotiation(#[from] NegotiationError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle() -> ChannelHandle {
        ChannelHandle::parse(format!("ch_{}", "ab".repeat(32))).expect("handle")
    }

    #[test]
    fn offer_ids_are_scoped_to_the_handle_and_direction() {
        let first = handle();
        let second =
            ChannelHandle::parse(format!("ch_{}", "cd".repeat(32))).expect("second handle");
        let id = external_offer_id(&first, Author::Counterparty, 9);
        assert_eq!(
            parse_offer_id(&first, &id).expect("parse"),
            InternalOfferId::new(Author::Counterparty, 9)
        );
        assert!(parse_offer_id(&second, &id).is_err());
    }

    #[test]
    fn an_allowance_must_cover_every_charged_write_plus_the_deposit() {
        // Mainnet numbers: 6 STRK per apply_actions.
        let report = AllowanceReport {
            allowance: 20_000_000_000_000_000_000,
            fee_per_write: 6_000_000_000_000_000_000,
        };
        assert!(report.covers(3, 0), "18 STRK of fees fits in 20");
        assert!(!report.covers(4, 0), "24 STRK of fees does not");
        assert!(
            !report.covers(3, 3_000_000_000_000_000_000),
            "the deposit shares the same allowance as the fees"
        );

        // A pool that charges nothing still needs allowance for the deposit itself.
        let free = AllowanceReport {
            allowance: 0,
            fee_per_write: 0,
        };
        assert!(free.covers(9, 0));
        assert!(!free.covers(0, 1));

        // Overflow is not a pass.
        let wide = AllowanceReport {
            allowance: u128::MAX,
            fee_per_write: u128::MAX,
        };
        assert!(!wide.covers(2, 0));
    }

    #[test]
    fn note_selection_prefers_an_exact_subset_then_returns_change() {
        let note = |index, amount| ValueNote {
            note: OwnedNote {
                channel_key: Felt::ONE,
                token: Felt::TWO,
                index,
            },
            amount,
            nullifier: Felt::from(index),
        };
        let notes = [note(1, 7), note(2, 5), note(3, 3)];
        let exact = select_notes(&notes, 8).expect("5 + 3");
        assert_eq!(exact.notes.iter().map(|note| note.amount).sum::<u128>(), 8);
        assert_eq!(exact.change, 0);

        let with_change = select_notes(&[note(4, 5)], 3).expect("5 covers 3");
        assert_eq!(with_change.notes.len(), 1);
        assert_eq!(with_change.notes[0].amount, 5);
        assert_eq!(with_change.change, 2);
        assert!(select_notes(&notes, 16).is_none());
    }

    /// The receipt reports selected input and change as separate numbers. They are only
    /// meaningful together: value is conserved, so the notes spent must equal what was paid
    /// plus what came back. If these ever drift, an agent reconciling its own balance from a
    /// receipt is quietly wrong and nothing errors.
    #[test]
    fn selected_input_always_equals_the_paid_amount_plus_change() {
        let note = |index, amount| ValueNote {
            note: OwnedNote {
                channel_key: Felt::ONE,
                token: Felt::TWO,
                index,
            },
            amount,
            nullifier: Felt::from(index),
        };
        let holdings = [note(1, 7), note(2, 5), note(3, 3), note(4, 1)];

        for target in 1..=16u128 {
            let Some(selection) = select_notes(&holdings, target) else {
                assert!(
                    target > 16,
                    "16 is the total, so anything at or under it is payable"
                );
                continue;
            };
            let selected_input = selection
                .notes
                .iter()
                .fold(0u128, |total, note| total.saturating_add(note.amount));
            assert_eq!(
                selected_input,
                target + selection.change,
                "value is not conserved at target {target}"
            );
            assert!(
                selected_input >= target,
                "a selection must cover the price at target {target}"
            );
        }
    }

    /// Absent and zero are different facts. A shield selects nothing; a settlement that spent
    /// exact notes selected something and kept nothing.
    #[test]
    fn a_shield_receipt_omits_the_selection_fields_rather_than_zeroing_them() {
        let shield = SettlementReceipt {
            offer_id: None,
            tx_hash: "0x1".to_owned(),
            nullifiers: Vec::new(),
            proved_at: 1,
            selected_input: None,
            change: None,
        };
        let json = serde_json::to_value(&shield).expect("serializes");
        assert!(json.get("selected_input").is_none());
        assert!(json.get("change").is_none());

        let settled = SettlementReceipt {
            selected_input: Some(5_000_000_000_000_000_000u128.to_string()),
            change: Some("0".to_owned()),
            ..shield
        };
        let json = serde_json::to_value(&settled).expect("serializes");
        // Strings, not numbers: 5e18 is past the JSON safe-integer range and a JavaScript
        // consumer would round it without erroring.
        assert_eq!(json["selected_input"], "5000000000000000000");
        assert_eq!(json["change"], "0");
        assert!(json["selected_input"].is_string());
    }

    #[test]
    fn chain_acceptance_recovers_a_stale_local_settled_flag() {
        let handle = handle();
        let state = StoredChannel::new(
            handle.clone(),
            Felt::from_hex("0x534e5f5345504f4c4941").expect("chain"),
            Felt::from_hex("0x9001").expect("pool"),
            Felt::ONE,
            Felt::TWO,
            Felt::THREE,
            Felt::from(4u8),
            Felt::from(5u8),
            0,
            0,
            Felt::from(6u8),
            7,
        );
        let mut book = OfferBook::new();
        let terms = WireMessage {
            message_type: MessageType::Offer,
            reply_to: None,
            created_at: 10,
            amount: 100,
            deadline: 1000,
            memo_hash: 7,
        };
        book.record(0, Author::Counterparty, terms).expect("offer");
        book.record(
            0,
            Author::Us,
            WireMessage {
                message_type: MessageType::Accept,
                reply_to: Some(0),
                ..terms
            },
        )
        .expect("acceptance");

        assert!(channel_state(&handle, &state, &book, 20).settled);
    }

    #[test]
    fn wire_v2_state_cannot_be_reused_under_another_chain_or_pool() {
        let chain = Felt::from_hex("0x534e5f5345504f4c4941").expect("chain");
        let pool = Felt::from_hex("0x9001").expect("pool");
        let mut state = StoredChannel::new(
            handle(),
            chain,
            pool,
            Felt::ONE,
            Felt::TWO,
            Felt::THREE,
            Felt::from(4u8),
            Felt::from(5u8),
            0,
            0,
            Felt::from(6u8),
            7,
        );
        let mut config = ClientConfig {
            rpc_url: String::new(),
            prover_url: String::new(),
            pool_address: pool,
            chain_id: chain,
            account_address: Felt::ONE,
            pool_key_file: PathBuf::new(),
            account_key_file: PathBuf::new(),
            state_dir: PathBuf::new(),
            token: Felt::from(4u8),
        };

        assert!(validate_scope(&state, &config).is_ok());
        config.pool_address += Felt::ONE;
        assert!(validate_scope(&state, &config).is_err());
        config.pool_address = pool;
        config.chain_id += Felt::ONE;
        assert!(validate_scope(&state, &config).is_err());

        state.wire_version = WireVersion::V1;
        state.chain_id = Felt::ZERO;
        state.pool_address = Felt::ZERO;
        assert!(
            validate_scope(&state, &config).is_ok(),
            "historical v1 state had no authenticated chain/pool scope"
        );
    }

    /// Packs a note the way the pool stores it: salt in the high 128 bits, encrypted amount
    /// in the low 128.
    fn packed_note(salt: u128, enc_amount: u128) -> Felt {
        let mut bytes = [0u8; 32];
        bytes[..16].copy_from_slice(&salt.to_be_bytes());
        bytes[16..].copy_from_slice(&enc_amount.to_be_bytes());
        Felt::from_bytes_be(&bytes)
    }

    /// The Sepolia bug: every salt-lane data note is encrypted, and
    /// the pool leaves `token` zero on those (`privacy.cairo:664`). Asserting equality
    /// rejected the entire transcript with a protocol-mismatch error.
    #[test]
    fn encrypted_note_token_is_zero() {
        let token = Felt::from_hex("0x4718f5a").expect("token");
        let data_note = packed_note(0x5eed, 0);

        assert!(check_note_token(data_note, Felt::ZERO, token).is_ok());
    }

    /// Rejects a derived encrypted-note id that finds an open note.
    #[test]
    fn an_encrypted_note_with_a_token_is_rejected() {
        let token = Felt::from_hex("0x4718f5a").expect("token");
        let data_note = packed_note(0x5eed, 0);

        assert!(check_note_token(data_note, token, token).is_err());
    }

    /// Open notes store their token and must match the requested token.
    #[test]
    fn an_open_note_must_match_the_token_we_asked_for() {
        let token = Felt::from_hex("0x4718f5a").expect("token");
        let other = Felt::from_hex("0xdead").expect("token");
        let open = packed_note(decrypt::OPEN_NOTE_SALT, 1_000);

        assert!(check_note_token(open, token, token).is_ok());
        assert!(check_note_token(open, other, token).is_err());
        assert!(check_note_token(open, Felt::ZERO, token).is_err());
    }
}
