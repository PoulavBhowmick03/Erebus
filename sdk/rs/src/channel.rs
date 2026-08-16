//! Channels, identities, and the negotiation operations built on them.
//!
//! Combines protocol primitives into channel setup, message writes, and settlement.
//!
//! ## Where the key lives
//!
//! [`PoolIdentity`] owns the pool private key and has no accessor. Its `Debug` output
//! redacts the key. Operations use the key through `&PoolIdentity`. This enforces CLAUDE.md
//! constraint 6 at the API boundary. The agent policy cannot access signing material.
//!
//! ## Channels are directional
//!
//! `channel_key = h(TAG, sender_addr, sender_privkey, recipient_addr, recipient_pubkey)`
//! includes the sender's private key, so only the sender can derive it. The recipient
//! learns it out of band, encrypted in `EncChannelInfo`, and reconstructs the channel with
//! [`Channel::from_key`]. Each direction has a different channel and key.
//!
//! ## Reads are keyed, never scanned
//!
//! [`Channel::note_ids_for_message`] computes the storage slots for a message. Readers use
//! those slots directly. They do not scan pool storage.

use starknet_crypto::get_public_key;
use starknet_types_core::felt::Felt;

use crate::action_set::{ActionSet, ActionSetBuilder, ActionSetError};
use crate::actions::{
    ClientAction, CreateEncNoteInput, DepositInput, FeltEntropy, NoteSalt, OpenChannelInput,
    OpenSubchannelInput, RandomSalt, SetViewingKeyInput, UseNoteInput,
};
use crate::disclosure::{ViewingGrant, ViewingGrantFields};
use crate::hashes;
use crate::subchannel::{IndexError, SubchannelCursor};
use crate::wire::{
    encode_message, MessageType, WireContext, WireError, WireMessage, WireVersion,
    NOTES_PER_MESSAGE,
};

/// Errors from channel operations.
#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    /// The message could not be encoded into salts.
    #[error(transparent)]
    Wire(#[from] WireError),
    /// The resulting action set was invalid.
    #[error(transparent)]
    ActionSet(#[from] ActionSetError),
    /// The write would violate one of the pool's note-index rules.
    #[error(transparent)]
    Index(#[from] IndexError),
    /// A settlement was handed a message that is not an acceptance.
    #[error("settlement record must be an Accept, got {0:?}")]
    NotAnAcceptance(MessageType),
    /// A settlement paid nothing.
    #[error("a settlement must move a non-zero amount")]
    ZeroPayment,
    /// A shield operation deposited nothing.
    #[error("a shield deposit must move a non-zero amount")]
    ZeroDeposit,
    /// A settlement consumed no notes.
    #[error("a settlement must spend at least one note")]
    NothingToSpend,
    /// The acceptance record and the payment note disagree on the amount.
    #[error(
        "acceptance records {agreed} but the payment note carries {paid}; \
         atomicity guarantees both land, not that they agree"
    )]
    AmountMismatch {
        /// What the acceptance message says.
        agreed: u128,
        /// What the payment note actually carries.
        paid: u128,
    },
    /// The payment note index falls inside the acceptance record's range.
    #[error(
        "payment note index {payment} collides with the acceptance record at \
         {acceptance_first}..{acceptance_end}"
    )]
    IndexCollision {
        /// The payment note's index.
        payment: u32,
        /// First index of the acceptance record.
        acceptance_first: u32,
        /// Exclusive end of the acceptance record.
        acceptance_end: u32,
    },
    /// Two outputs target the same note slot in one channel.
    #[error("two settlement outputs target note index {index} in channel {channel_key:#x}")]
    OutputIndexCollision {
        /// Channel containing the duplicate slot.
        channel_key: Felt,
        /// Duplicate note index.
        index: u32,
    },
    /// A present change output carried no value.
    #[error("a change output must move a non-zero amount")]
    ZeroChange,
}

/// An agent's identity inside the pool.
///
/// Holds the pool private key without exposing an accessor.
#[derive(Clone)]
pub struct PoolIdentity {
    address: Felt,
    private_key: Felt,
}

impl PoolIdentity {
    /// Creates an identity from an address and its pool private key.
    pub fn new(address: Felt, private_key: Felt) -> Self {
        Self {
            address,
            private_key,
        }
    }

    /// The agent's Starknet address.
    pub fn address(&self) -> Felt {
        self.address
    }

    /// The public half, which counterparties need in order to send to this identity.
    pub fn public_key(&self) -> Felt {
        get_public_key(&self.private_key)
    }

    /// The action that registers this identity with the pool.
    ///
    /// Registration publishes the public key and writes the private key encrypted to the
    /// pool auditor (`privacy.cairo:329-334`). `random` is the ephemeral encryption secret.
    ///
    /// Registration is irreversible. After registration, the auditor can decrypt the
    /// identity's full pool history. This is part of StarkWare's threshold-auditor design
    /// and cannot be disabled by Erebus.
    pub fn register(&self, random: FeltEntropy) -> ClientAction {
        ClientAction::SetViewingKey(SetViewingKeyInput {
            random: random.get(),
        })
    }
}

/// Redacts the key to keep it out of logs, panic messages, and error reports.
impl core::fmt::Debug for PoolIdentity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PoolIdentity")
            .field("address", &self.address)
            .field("private_key", &"<redacted>")
            .finish()
    }
}

/// A counterparty, described entirely by public information.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counterparty {
    /// Their Starknet address.
    pub address: Felt,
    /// Their registered pool public key.
    pub public_key: Felt,
}

/// A directional channel.
///
/// Both parties hold the channel key. It locates and decrypts channel notes, so it is not
/// public.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Channel {
    chain_id: Felt,
    pool_address: Felt,
    channel_key: Felt,
    counterparty: Counterparty,
    wire_version: WireVersion,
}

impl Channel {
    /// Derives the channel from us to `counterparty`. Only the sender can do this.
    pub fn derive(
        chain_id: Felt,
        pool_address: Felt,
        identity: &PoolIdentity,
        counterparty: Counterparty,
    ) -> Self {
        let channel_key = hashes::compute_channel_key(
            identity.address,
            identity.private_key,
            counterparty.address,
            counterparty.public_key,
        );
        Self {
            chain_id,
            pool_address,
            channel_key,
            counterparty,
            wire_version: WireVersion::V2,
        }
    }

    /// Reconstructs an *incoming* channel from a key learned out of band.
    ///
    /// The recipient cannot derive this key because it includes the sender's private key.
    /// It arrives encrypted in `EncChannelInfo`.
    pub fn from_key(
        chain_id: Felt,
        pool_address: Felt,
        channel_key: Felt,
        counterparty: Counterparty,
    ) -> Self {
        Self::from_key_with_version(
            chain_id,
            pool_address,
            channel_key,
            counterparty,
            WireVersion::V2,
        )
    }

    /// Reconstructs a channel stored under an explicit wire generation.
    ///
    /// Wire v1 remains readable so existing grants do not become useless, but attempts to
    /// write through it fail with [`WireError::LegacyReadOnly`].
    pub fn from_key_with_version(
        chain_id: Felt,
        pool_address: Felt,
        channel_key: Felt,
        counterparty: Counterparty,
        wire_version: WireVersion,
    ) -> Self {
        Self {
            chain_id,
            pool_address,
            channel_key,
            counterparty,
            wire_version,
        }
    }

    /// The channel key.
    pub fn key(&self) -> Felt {
        self.channel_key
    }

    /// The other party.
    pub fn counterparty(&self) -> Counterparty {
        self.counterparty
    }

    /// Wire generation used by this channel.
    pub fn wire_version(&self) -> WireVersion {
        self.wire_version
    }

    fn wire_context(&self, token: Felt, message_index: u32) -> WireContext {
        WireContext {
            chain_id: self.chain_id,
            pool_address: self.pool_address,
            channel_key: self.channel_key,
            token,
            message_index,
        }
    }

    /// The action that opens this channel to the counterparty.
    ///
    /// `index` is the channel's position among the sender's outgoing channels. It must be
    /// contiguous because a gap makes later channels unreachable.
    ///
    /// `random` encrypts the channel information. `salt` provides one-time key use.
    pub fn open_channel(&self, index: u32, random: FeltEntropy, salt: FeltEntropy) -> ClientAction {
        ClientAction::OpenChannel(OpenChannelInput {
            recipient_addr: self.counterparty.address,
            index,
            random: random.get(),
            salt: salt.get(),
        })
    }

    /// The action that opens a subchannel for `token` within this channel.
    ///
    /// Each token has one subchannel. Notes live in that subchannel, so wire messages omit
    /// the token.
    pub fn open_subchannel(&self, index: u32, token: Felt, salt: FeltEntropy) -> ClientAction {
        ClientAction::OpenSubchannel(OpenSubchannelInput {
            recipient_addr: self.counterparty.address,
            recipient_public_key: self.counterparty.public_key,
            channel_key: self.channel_key,
            index,
            token,
            salt: salt.get(),
        })
    }

    /// Registers the identity and opens the channel and subchannel in one action set.
    ///
    /// One action set needs one ~29 s proof. [`ActionSetBuilder`] enforces the required
    /// ACCOUNT, CHANNEL, SUBCHANNEL phase order.
    ///
    /// Skip `register` for an existing identity. The viewing key is immutable, and a second
    /// registration reverts on `WriteOnce`.
    pub fn setup(
        &self,
        identity: &PoolIdentity,
        params: SetupParams,
    ) -> Result<ActionSet, ChannelError> {
        let mut builder = ActionSetBuilder::new();
        if let Some(random) = params.register {
            builder.push(identity.register(random))?;
        }
        builder.push(self.open_channel(
            params.channel_index,
            params.channel_random,
            params.channel_salt,
        ))?;
        builder.push(self.open_subchannel(
            params.subchannel_index,
            params.token,
            params.subchannel_salt,
        ))?;
        Ok(builder.build()?)
    }

    /// Registers if needed, opens a self-channel, deposits, and creates one encrypted note
    /// in a single action set.
    ///
    /// A deposit alone leaves a positive token balance and has no replay protection. The new
    /// value note balances the action set and provides replay protection.
    pub fn shield(
        &self,
        identity: &PoolIdentity,
        params: SetupParams,
        amount: u128,
        note_salt: RandomSalt,
    ) -> Result<ActionSet, ChannelError> {
        if amount == 0 {
            return Err(ChannelError::ZeroDeposit);
        }
        let mut builder = ActionSetBuilder::new();
        if let Some(random) = params.register {
            builder.push(identity.register(random))?;
        }
        builder.push(self.open_channel(
            params.channel_index,
            params.channel_random,
            params.channel_salt,
        ))?;
        builder.push(self.open_subchannel(
            params.subchannel_index,
            params.token,
            params.subchannel_salt,
        ))?;
        builder.push(ClientAction::Deposit(DepositInput {
            token: params.token,
            amount,
        }))?;
        builder.push(self.value_note(params.token, 0, amount, note_salt))?;
        Ok(builder.build()?)
    }

    /// Deposits into a self-channel that is *already* open, appending one note at
    /// `note_index`.
    ///
    /// [`Self::shield`] can run once per identity. The channel key takes no index
    /// (`hashes.cairo`, `compute_channel_key`), so a self-channel has exactly one marker,
    /// and that marker is `WriteOnce`. The first shield claims it. Every later shield
    /// re-derives the same marker and reverts with a bare `NON_ZERO_VALUE` from deep inside
    /// `_apply_write_once`. A top-up must reuse the channel and subchannel and append a note.
    ///
    /// `note_index` must be the next free index: the contract asserts notes are sequential
    /// (`_prepare_note_creation`, `INDEX_NOT_SEQUENTIAL`) and discovery stops at the first
    /// empty slot. A gap hides every later note.
    ///
    /// `create_enc_note` emits the `WriteOnce` that provides replay protection. No channel
    /// action is required.
    pub fn deposit_into_open_channel(
        &self,
        token: Felt,
        note_index: u32,
        amount: u128,
        note_salt: RandomSalt,
    ) -> Result<ActionSet, ChannelError> {
        if amount == 0 {
            return Err(ChannelError::ZeroDeposit);
        }
        let mut builder = ActionSetBuilder::new();
        builder.push(ClientAction::Deposit(DepositInput { token, amount }))?;
        builder.push(self.value_note(token, note_index, amount, note_salt))?;
        Ok(builder.build()?)
    }

    /// Storage note ids for every note of message `message_index`.
    ///
    /// Counterparties compute these ids and fetch the slots without scanning.
    pub fn note_ids_for_message(
        &self,
        token: Felt,
        message_index: u32,
    ) -> [Felt; NOTES_PER_MESSAGE] {
        let first = u64::from(message_index) * NOTES_PER_MESSAGE as u64;
        core::array::from_fn(|slot| {
            hashes::compute_note_id(self.channel_key, token, first + slot as u64)
        })
    }

    /// Builds the action set that writes one negotiation message into this channel.
    ///
    /// Writes five zero-amount notes at consecutive indices. A structured salt on a value
    /// note can reuse an amount mask. An observer can then subtract ciphertexts and recover
    /// the amount difference. Zero-amount notes have no amount difference to expose.
    pub fn write_message(
        &self,
        token: Felt,
        message_index: u32,
        message: &WireMessage,
    ) -> Result<ActionSet, ChannelError> {
        if self.wire_version == WireVersion::V1 {
            return Err(WireError::LegacyReadOnly.into());
        }
        let salts = encode_message(&self.wire_context(token, message_index), message)?;
        let first = message_index * NOTES_PER_MESSAGE as u32;

        let mut builder = ActionSetBuilder::new();
        for (slot, salt) in salts.iter().enumerate() {
            builder.push(self.data_note(token, first + slot as u32, *salt))?;
        }
        Ok(builder.build()?)
    }

    /// Writes the next negotiation message, allocating its indices from `cursor`.
    ///
    /// Prefer this to [`Channel::write_message`]. The pool checks `INDEX_NOT_SEQUENTIAL` and
    /// `NON_ZERO_VALUE` after proving. The cursor checks both rules before proving.
    ///
    /// The cursor advances only after the message encodes and validates.
    pub fn write_next_message(
        &self,
        token: Felt,
        cursor: &mut SubchannelCursor,
        message: &WireMessage,
    ) -> Result<(u32, ActionSet), ChannelError> {
        let message_index = cursor.next_message_index()?;
        let set = self.write_message(token, message_index, message)?;
        cursor.reserve_message()?;
        Ok((message_index, set))
    }

    /// Grants a scoped viewing key for this channel pair on `token`.
    ///
    /// `incoming_key` is the counterparty-to-local key from `EncChannelInfo`. The grant needs
    /// both directional keys to show all messages.
    ///
    /// The grant contains channel keys, not a pool private key. It can read this channel but
    /// cannot create a nullifier or spend.
    /// See [`crate::disclosure`] for how this differs from the pool's own auditor escrow.
    pub fn grant_viewing_key(
        &self,
        identity: &PoolIdentity,
        incoming_key: Felt,
        token: Felt,
    ) -> ViewingGrant {
        ViewingGrant::new(ViewingGrantFields {
            chain_id: self.chain_id,
            pool_address: self.pool_address,
            wire_version: self.wire_version,
            outgoing_key: self.channel_key,
            incoming_key,
            token,
            granter: identity.address(),
            counterparty: self.counterparty.address,
        })
    }

    /// A single zero-amount note carrying a structured salt.
    fn data_note(&self, token: Felt, index: u32, salt: NoteSalt) -> ClientAction {
        ClientAction::CreateEncNote(CreateEncNoteInput {
            recipient_addr: self.counterparty.address,
            recipient_public_key: self.counterparty.public_key,
            token,
            amount: 0,
            index,
            salt,
        })
    }

    /// Value note. Requires [`RandomSalt`] because a structured salt can expose an amount
    /// difference under mask reuse.
    fn value_note(&self, token: Felt, index: u32, amount: u128, salt: RandomSalt) -> ClientAction {
        ClientAction::CreateEncNote(CreateEncNoteInput {
            recipient_addr: self.counterparty.address,
            recipient_public_key: self.counterparty.public_key,
            token,
            amount,
            index,
            salt: salt.salt(),
        })
    }

    /// Builds one action set that accepts and pays for an offer atomically.
    ///
    /// Acceptance and payment share one proof. Both land, or neither lands.
    ///
    /// [`ActionSetBuilder`] puts spends in phase 4 before note creation in phase 5. It rejects
    /// create-then-spend before proof generation.
    ///
    /// The payment note uses [`RandomSalt`]. Only the acceptance record carries structure.
    pub fn accept_and_settle(
        &self,
        token: Felt,
        spend: &[OwnedNote],
        payment: Payment,
        acceptance: Acceptance,
    ) -> Result<ActionSet, ChannelError> {
        self.accept_and_settle_with_change(token, spend, payment, acceptance, None)
    }

    /// Builds an atomic acceptance, payment, and optional payer-owned change output.
    /// The payment uses the counterparty channel. Retained value uses the payer
    /// self-channel. Both value notes require [`RandomSalt`].
    pub fn accept_and_settle_with_change(
        &self,
        token: Felt,
        spend: &[OwnedNote],
        payment: Payment,
        acceptance: Acceptance,
        change: Option<ChangeOutput>,
    ) -> Result<ActionSet, ChannelError> {
        if acceptance.message.message_type != MessageType::Accept {
            return Err(ChannelError::NotAnAcceptance(
                acceptance.message.message_type,
            ));
        }
        if payment.amount == 0 {
            return Err(ChannelError::ZeroPayment);
        }
        if spend.is_empty() {
            return Err(ChannelError::NothingToSpend);
        }
        if matches!(change, Some(output) if output.amount == 0) {
            return Err(ChannelError::ZeroChange);
        }
        // Atomicity does not make the accepted amount equal the payment. Without this check,
        // the SDK can record 900 and pay 800 in the same proof.
        if payment.amount != acceptance.message.amount {
            return Err(ChannelError::AmountMismatch {
                agreed: acceptance.message.amount,
                paid: payment.amount,
            });
        }

        // The payment note and the acceptance notes share one subchannel index space.
        let acceptance_first = acceptance.message_index * NOTES_PER_MESSAGE as u32;
        let acceptance_range = acceptance_first..acceptance_first + NOTES_PER_MESSAGE as u32;
        if acceptance_range.contains(&payment.index) {
            return Err(ChannelError::IndexCollision {
                payment: payment.index,
                acceptance_first,
                acceptance_end: acceptance_range.end,
            });
        }

        if self.wire_version == WireVersion::V1 {
            return Err(WireError::LegacyReadOnly.into());
        }
        let salts = encode_message(
            &self.wire_context(token, acceptance.message_index),
            &acceptance.message,
        )?;
        let mut builder = ActionSetBuilder::new();

        // A payer can receive funds without ever opening a self-channel. If its first
        // retained value is settlement change, open that channel and token subchannel in
        // this same action set before the spend/create phases.
        if let Some(ChangeOutput {
            channel,
            setup: Some(setup),
            ..
        }) = change
        {
            builder.push(channel.open_channel(
                setup.channel_index,
                setup.channel_random,
                setup.channel_salt,
            ))?;
            builder.push(channel.open_subchannel(
                setup.subchannel_index,
                token,
                setup.subchannel_salt,
            ))?;
        }

        // Phase 4: consume the inputs.
        for note in spend {
            builder.push(ClientAction::UseNote(UseNoteInput {
                channel_key: note.channel_key,
                token: note.token,
                index: note.index,
            }))?;
        }

        // Phase 5: the creations, **in ascending index order**.
        //
        // `_client_apply_actions` applies each `WriteOnce` in order (`privacy.cairo:761`). A
        // payment before a lower-indexed acceptance fails `INDEX_NOT_SEQUENTIAL`. Sort the
        // selected indices into the only valid write order.
        let mut creates: Vec<(Felt, u32, ClientAction)> = Vec::with_capacity(2 + NOTES_PER_MESSAGE);
        creates.push((
            self.channel_key,
            payment.index,
            self.value_note(token, payment.index, payment.amount, payment.salt),
        ));
        for (slot, salt) in salts.iter().enumerate() {
            let index = acceptance_first + slot as u32;
            creates.push((self.channel_key, index, self.data_note(token, index, *salt)));
        }
        if let Some(output) = change {
            creates.push((
                output.channel.channel_key,
                output.index,
                output
                    .channel
                    .value_note(token, output.index, output.amount, output.salt),
            ));
        }
        // Contiguity is per subchannel. Order each channel group so an earlier slot is
        // created before a later slot in the same action set.
        creates.sort_by_key(|(channel_key, index, _)| (channel_key.to_bytes_be(), *index));
        for pair in creates.windows(2) {
            if pair[0].0 == pair[1].0 && pair[0].1 == pair[1].1 {
                return Err(ChannelError::OutputIndexCollision {
                    channel_key: pair[0].0,
                    index: pair[0].1,
                });
            }
        }
        for (_, _, action) in creates {
            builder.push(action)?;
        }

        Ok(builder.build()?)
    }

    /// Accepts and settles using the next free indices, allocated from `cursor`.
    ///
    /// Places the acceptance on the `5k..5k+4` message grid and the payment after it. Putting
    /// the payment first misaligns the reader.
    ///
    /// Settlement leaves the cursor off the message grid. A second negotiation cannot start
    /// in that subchannel. `docs/poulav.md` P1.3 tracks this multi-deal constraint.
    #[allow(clippy::too_many_arguments)]
    pub fn settle_next(
        &self,
        token: Felt,
        cursor: &mut SubchannelCursor,
        spend: &[OwnedNote],
        amount: u128,
        salt: RandomSalt,
        message: &WireMessage,
    ) -> Result<(u32, ActionSet), ChannelError> {
        self.settle_next_with_change(token, cursor, spend, amount, salt, message, None)
    }

    /// Allocates the outgoing acceptance/payment slots and includes optional change.
    ///
    /// The change index belongs to its own channel cursor and must already be the next free
    /// slot there. This method advances only the outgoing negotiation cursor.
    #[allow(clippy::too_many_arguments)]
    pub fn settle_next_with_change(
        &self,
        token: Felt,
        cursor: &mut SubchannelCursor,
        spend: &[OwnedNote],
        amount: u128,
        salt: RandomSalt,
        message: &WireMessage,
        change: Option<ChangeOutput>,
    ) -> Result<(u32, ActionSet), ChannelError> {
        let message_index = cursor.next_message_index()?;
        let payment_index = message_index * NOTES_PER_MESSAGE as u32 + NOTES_PER_MESSAGE as u32;

        let set = self.accept_and_settle_with_change(
            token,
            spend,
            Payment {
                amount,
                index: payment_index,
                salt,
            },
            Acceptance {
                message_index,
                message: *message,
            },
            change,
        )?;

        cursor.reserve_message()?;
        cursor.reserve_note()?;
        Ok((message_index, set))
    }
}

/// A note this agent owns and may spend.
///
/// `channel_key` identifies the incoming channel that owns the note. A spend and its funded
/// payment use different directional channel keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnedNote {
    /// The channel the note arrived in.
    pub channel_key: Felt,
    /// The note's token.
    pub token: Felt,
    /// The note's index within that channel.
    pub index: u32,
}

/// The value leg of a settlement.
#[derive(Debug, Clone, Copy)]
pub struct Payment {
    /// Amount in the token's smallest unit.
    pub amount: u128,
    /// Note index within the outgoing subchannel.
    pub index: u32,
    /// Unpredictable salt. See [`RandomSalt`].
    pub salt: RandomSalt,
}

/// The record leg of a settlement.
#[derive(Debug, Clone, Copy)]
pub struct Acceptance {
    /// Message index within the outgoing subchannel.
    pub message_index: u32,
    /// The acceptance message. Must be [`MessageType::Accept`].
    pub message: WireMessage,
}

/// Parameters needed when settlement change opens the payer's self-channel.
#[derive(Debug, Clone, Copy)]
pub struct ChangeChannelSetup {
    /// Position among the payer's outgoing channels.
    pub channel_index: u32,
    /// Encrypts channel information for the payer-as-recipient.
    pub channel_random: FeltEntropy,
    /// One-time entropy for the channel information.
    pub channel_salt: FeltEntropy,
    /// Token subchannel index. The single-token client uses zero.
    pub subchannel_index: u32,
    /// Encrypts the token stored in the subchannel.
    pub subchannel_salt: FeltEntropy,
}

/// Payer-owned value retained when selected inputs exceed the payment.
///
/// [`RandomSalt`] prevents a structured salt on the value-bearing change note.
#[derive(Debug, Clone, Copy)]
pub struct ChangeOutput {
    channel: Channel,
    amount: u128,
    index: u32,
    salt: RandomSalt,
    setup: Option<ChangeChannelSetup>,
}

impl ChangeOutput {
    /// Change written to an existing payer self-channel.
    pub fn existing(channel: Channel, amount: u128, index: u32, salt: RandomSalt) -> Self {
        Self {
            channel,
            amount,
            index,
            salt,
            setup: None,
        }
    }

    /// Change that opens the payer self-channel in the settlement action set.
    pub fn opening(
        channel: Channel,
        amount: u128,
        salt: RandomSalt,
        setup: ChangeChannelSetup,
    ) -> Self {
        Self {
            channel,
            amount,
            // Zero is the only contiguous index in a new subchannel.
            index: 0,
            salt,
            setup: Some(setup),
        }
    }
}

/// Everything a first conversation needs to be set up.
#[derive(Debug, Clone, Copy)]
pub struct SetupParams {
    /// `Some(random)` to register this identity, `None` if it is already registered.
    pub register: Option<FeltEntropy>,
    /// Position of this channel among the sender's outgoing channels. Must be contiguous.
    pub channel_index: u32,
    /// Encrypts the channel info for the recipient.
    pub channel_random: FeltEntropy,
    /// One-time key nonce for the channel info encryption.
    pub channel_salt: FeltEntropy,
    /// Position of the subchannel within the channel.
    pub subchannel_index: u32,
    /// The token this subchannel carries.
    pub token: Felt,
    /// One-time key nonce for the subchannel token encryption.
    pub subchannel_salt: FeltEntropy,
}
