//! High-level Rust client implementing the MVP interface.
//!
//! The public methods deal in opaque handles and offer ids. Pool/channel secrets live in
//! [`crate::state::StateStore`], while the two private signing values are read from local
//! files for each operation and dropped when the call returns.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use starknet_types_core::felt::Felt;

use crate::actions::{FeltEntropy, RandomSalt};
use crate::channel::{Channel, ChannelError, Counterparty, OwnedNote, PoolIdentity, SetupParams};
use crate::decrypt;
use crate::disclosure::{self, ViewingGrant};
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
const MAX_EXACT_SELECTION_NOTES: usize = 256;
const MAX_EXACT_SELECTION_STATES: usize = 100_000;

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
    /// This administrative MVP helper sits outside the seven negotiation methods. Exact
    /// notes are useful because settlement deliberately refuses to destroy surplus value;
    /// general change-note construction is not yet part of the MVP.
    pub async fn shield(&self, amount: u128) -> Result<SettlementReceipt, ClientError> {
        if amount == 0 {
            return Err(ClientError::InvalidRequest(
                "shield amount must be non-zero".to_owned(),
            ));
        }
        let (identity, pool_key, account_key) = self.identity_keys()?;
        let registered = self.registered_public_key(identity.address()).await?;
        self.verify_own_registration(&identity, registered)?;
        let channel_index = self.outgoing_channel_count(&identity, pool_key).await?;
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
        let actions = channel.shield(
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
        )?;
        let receipt = self
            .executor
            .execute(identity.address(), pool_key, account_key, &actions)
            .await?;
        Ok(SettlementReceipt {
            offer_id: None,
            tx_hash: hex(receipt.transaction_hash),
            nullifiers: Vec::new(),
            proved_at: receipt.proving_block,
        })
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

    async fn outgoing_channel_count(
        &self,
        identity: &PoolIdentity,
        pool_key: Felt,
    ) -> Result<u32, ClientError> {
        for index in 0..MAX_DISCOVERY_ITEMS {
            let id = hashes::compute_outgoing_channel_id(identity.address(), pool_key, index);
            let result = self
                .view("get_outgoing_channel_info", &[id], &BlockId::Latest)
                .await?;
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

/// The frozen negotiation surface, with one correction: granting returns the bearer
/// viewing grant that must be delivered to the grantee.
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

        // One channel per pair, forever: the pool's channel key takes no index
        // (`hashes.cairo:119-124`) and its marker is WriteOnce, so re-opening reverts —
        // but only after the preflight, the proof and the fee have all been paid for, and
        // it surfaces as a bare `Contract error`. Returning the existing handle makes this
        // idempotent, which is also what a retrying agent needs. See friction.md F29.
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
        // A one-sided channel is still readable before the counterparty opens their reverse
        // direction. Only suppress the ordinary not-ready result; ambiguity is actionable.
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
        let selected =
            select_exact_notes(&available, offer.amount).ok_or(ClientError::InsufficientNotes {
                required: offer.amount,
            })?;
        let spend: Vec<OwnedNote> = selected.iter().map(|note| note.note).collect();

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
        let (_, actions) = channel.settle_next(
            state.token,
            &mut cursor,
            &spend,
            offer.amount,
            random_salt(),
            &acceptance,
        )?;
        let receipt = self
            .executor
            .execute(identity.address(), pool_key, account_key, &actions)
            .await?;
        lease.state_mut().outgoing_next_note = cursor.next_index();
        lease.state_mut().last_write_block = accepted_block(&receipt)?;
        lease.state_mut().settled = true;
        lease.commit()?;

        Ok(SettlementReceipt {
            offer_id: Some(offer_id),
            tx_hash: hex(receipt.transaction_hash),
            nullifiers: selected.iter().map(|note| hex(note.nullifier)).collect(),
            proved_at: receipt.proving_block,
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

/// Submitted settlement or administrative shielding receipt.
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

fn select_exact_notes(notes: &[ValueNote], target: u128) -> Option<Vec<ValueNote>> {
    if target == 0 {
        return Some(Vec::new());
    }
    let notes = &notes[..notes.len().min(MAX_EXACT_SELECTION_NOTES)];
    let mut sums: HashMap<u128, Vec<usize>> = HashMap::from([(0, Vec::new())]);
    for (index, note) in notes.iter().enumerate() {
        let snapshot: Vec<(u128, Vec<usize>)> = sums
            .iter()
            .map(|(sum, picks)| (*sum, picks.clone()))
            .collect();
        for (sum, mut picks) in snapshot {
            let Some(next) = sum.checked_add(note.amount) else {
                continue;
            };
            if next > target || sums.contains_key(&next) {
                continue;
            }
            picks.push(index);
            if next == target {
                return Some(picks.into_iter().map(|pick| notes[pick]).collect());
            }
            sums.insert(next, picks);
            if sums.len() >= MAX_EXACT_SELECTION_STATES {
                return None;
            }
        }
    }
    None
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
/// **The field is only meaningful for open notes.** For encrypted notes the pool never
/// writes it — `privacy.cairo:664`, *"Only `packed_value` needs to be written to storage,
/// `token` is initialized to zero"*, restated on the struct itself in `objects.cairo:98`
/// as *"the token address of the note (zero for encrypted notes)"*. A channel note's token
/// is implied by the subchannel it was found in and cannot be recovered from the note.
///
/// So the two kinds get opposite checks, and neither is dead weight: an open note must
/// carry the token we asked for, and an encrypted note must carry zero. A non-zero token
/// on an encrypted note would mean the id we derived landed on an open note — a real
/// anomaly, and one that would otherwise decrypt to garbage rather than fail.
///
/// Getting this wrong was a live bug, not a hypothetical: asserting equality for every note
/// rejected every valid message note with a protocol-mismatch error. See friction.md F28.
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
            "encrypted note carries token {stored:#x}, expected zero — \
             the derived note id landed on an open note"
        )));
    }
    Ok(())
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
    /// Available private notes cannot sum exactly to the payment.
    #[error("no exact set of unspent notes sums to {required}")]
    InsufficientNotes {
        /// Required amount.
        required: u128,
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
    fn exact_note_selection_never_burns_change() {
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
        let selected = select_exact_notes(&notes, 8).expect("5 + 3");
        assert_eq!(selected.iter().map(|note| note.amount).sum::<u128>(), 8);
        assert!(select_exact_notes(&notes, 6).is_none());
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

    /// The bug that reached Sepolia: every salt-lane data note is an *encrypted* note, and
    /// the pool leaves `token` zero on those (`privacy.cairo:664`). Asserting equality
    /// rejected the entire transcript with a protocol-mismatch error.
    #[test]
    fn an_encrypted_note_carries_no_token_and_that_is_not_an_error() {
        let token = Felt::from_hex("0x4718f5a").expect("token");
        let data_note = packed_note(0x5eed, 0);

        assert!(check_note_token(data_note, Felt::ZERO, token).is_ok());
    }

    /// The opposite check still has to bite, or a misderived id that lands on an open note
    /// would decrypt to garbage instead of failing.
    #[test]
    fn an_encrypted_note_with_a_token_is_rejected() {
        let token = Felt::from_hex("0x4718f5a").expect("token");
        let data_note = packed_note(0x5eed, 0);

        assert!(check_note_token(data_note, token, token).is_err());
    }

    /// Open notes do store their token, so equality is the right check there — this is the
    /// half of the original assertion that was correct and must not be lost.
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
