//! Channels, identities, and the negotiation operations built on them.
//!
//! This is where the primitives compose into the things an agent actually does: derive a
//! channel to a counterparty, write a negotiation message into it, and work out where the
//! counterparty's messages will be.
//!
//! ## Where the key lives
//!
//! [`PoolIdentity`] owns the pool private key and never lends it out. There is no accessor,
//! its `Debug` redacts, and every operation that needs the key takes `&PoolIdentity` and
//! uses it internally. That is CLAUDE.md constraint 6 made structural: key material does
//! not leave the SDK boundary, because there is no way to ask for it.
//!
//! This matters more under the architecture we settled on. The library runs inside the
//! agent operator's own process, so the boundary it defends is the one between Erebus's
//! code and the agent's policy engine — the layer that decides *what* to offer must never
//! be able to touch *what signs for it*.
//!
//! ## Channels are directional
//!
//! `channel_key = h(TAG, sender_addr, sender_privkey, recipient_addr, recipient_pubkey)`
//! hashes the **sender's** private key, so only the sender can derive it. The recipient
//! learns it out of band, encrypted in `EncChannelInfo`, and reconstructs the channel with
//! [`Channel::from_key`]. A→B and B→A are two different channels with two different keys.
//!
//! ## Reads are keyed, never scanned
//!
//! [`Channel::note_ids_for_message`] computes exactly where a message's notes live. The
//! reader seeks those storage slots directly. Scanning the chain for notes is both slower
//! and wrong — it defeats the discovery design and does not work at any real pool size.

use starknet_crypto::get_public_key;
use starknet_types_core::felt::Felt;

use crate::action_set::{ActionSet, ActionSetBuilder, ActionSetError};
use crate::actions::{
    ClientAction, CreateEncNoteInput, FeltEntropy, NoteSalt, OpenChannelInput,
    OpenSubchannelInput, RandomSalt, SetViewingKeyInput, UseNoteInput,
};
use crate::hashes;
use crate::subchannel::{IndexError, SubchannelCursor};
use crate::wire::{encode_message, MessageType, WireError, WireMessage, NOTES_PER_MESSAGE};

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
    /// A settlement consumed no notes.
    #[error("a settlement must spend at least one note")]
    NothingToSpend,
    /// The payment note index falls inside the acceptance record's range.
    #[error(
        "payment note index {payment} collides with the acceptance record at {acceptance_first}..{}",
        acceptance_first + 4
    )]
    IndexCollision {
        /// The payment note's index.
        payment: u32,
        /// First index of the acceptance record.
        acceptance_first: u32,
    },
}

/// An agent's identity inside the pool.
///
/// Holds the pool private key. There is deliberately no way to read it back out.
#[derive(Clone)]
pub struct PoolIdentity {
    address: Felt,
    private_key: Felt,
}

impl PoolIdentity {
    /// Creates an identity from an address and its pool private key.
    pub fn new(address: Felt, private_key: Felt) -> Self {
        Self { address, private_key }
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
    /// Registration publishes the public key so others can send here — and, in the same
    /// step, writes this identity's **private key encrypted to the pool's auditor**
    /// on-chain (`privacy.cairo:329-334`). `random` is the ephemeral secret for that
    /// encryption.
    ///
    /// That is worth being deliberate about rather than discovering later: from the moment
    /// an agent registers, the auditor can decrypt everything it will ever do in the pool.
    /// It is StarkWare's threshold-auditor design and a condition of using the pool at all,
    /// not something Erebus adds or can opt out of.
    pub fn register(&self, random: FeltEntropy) -> ClientAction {
        ClientAction::SetViewingKey(SetViewingKeyInput { random: random.get() })
    }
}

/// Redacts the key. A `Debug` that printed it would leak into any log line, panic message
/// or error report that happened to include an identity.
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
/// The channel key is a *locator*, not a secret in the same class as the private key: the
/// counterparty holds it too, by design. Anyone else holding it could find the notes, so it
/// is not public either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Channel {
    channel_key: Felt,
    counterparty: Counterparty,
}

impl Channel {
    /// Derives the channel from us to `counterparty`. Only the sender can do this.
    pub fn derive(identity: &PoolIdentity, counterparty: Counterparty) -> Self {
        let channel_key = hashes::compute_channel_key(
            identity.address,
            identity.private_key,
            counterparty.address,
            counterparty.public_key,
        );
        Self { channel_key, counterparty }
    }

    /// Reconstructs an *incoming* channel from a key learned out of band.
    ///
    /// The recipient cannot derive this — it hashes the sender's private key — so it
    /// arrives encrypted in `EncChannelInfo` and is passed here.
    pub fn from_key(channel_key: Felt, counterparty: Counterparty) -> Self {
        Self { channel_key, counterparty }
    }

    /// The channel key.
    pub fn key(&self) -> Felt {
        self.channel_key
    }

    /// The other party.
    pub fn counterparty(&self) -> Counterparty {
        self.counterparty
    }

    /// The action that opens this channel to the counterparty.
    ///
    /// `index` is the channel's position among this sender's outgoing channels, and must
    /// be contiguous — the contract derives storage from it, so a gap makes later channels
    /// unreachable rather than merely untidy.
    ///
    /// `random` encrypts the channel info so the recipient can learn the channel key they
    /// cannot derive themselves; `salt` guarantees one-time key usage for that encryption.
    pub fn open_channel(
        &self,
        index: u32,
        random: FeltEntropy,
        salt: FeltEntropy,
    ) -> ClientAction {
        ClientAction::OpenChannel(OpenChannelInput {
            recipient_addr: self.counterparty.address,
            index,
            random: random.get(),
            salt: salt.get(),
        })
    }

    /// The action that opens a subchannel for `token` within this channel.
    ///
    /// A subchannel is *per token*, not per topic — one channel carries one subchannel for
    /// each token the parties transact in, and notes live in the subchannel. This is why
    /// the wire format can drop `token` from a message: the subchannel already says it.
    pub fn open_subchannel(
        &self,
        index: u32,
        token: Felt,
        salt: FeltEntropy,
    ) -> ClientAction {
        ClientAction::OpenSubchannel(OpenSubchannelInput {
            recipient_addr: self.counterparty.address,
            recipient_public_key: self.counterparty.public_key,
            channel_key: self.channel_key,
            index,
            token,
            salt: salt.get(),
        })
    }

    /// The whole setup for a first conversation, in one action set: register, open the
    /// channel, open the subchannel.
    ///
    /// One transaction, so one proof (~29 s) rather than three. Phase order is
    /// ACCOUNT → CHANNEL → SUBCHANNEL, which is the order the contract requires and which
    /// [`ActionSetBuilder`] checks.
    ///
    /// Skip `register` on an identity already known to the pool — the viewing key is
    /// immutable once set and a second registration reverts on the `WriteOnce`.
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

    /// Storage note ids for every note of message `message_index`.
    ///
    /// This is how a counterparty reads: compute, then fetch those slots. Never scan.
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
    /// Four zero-amount notes at consecutive indices, one action set, one proof.
    ///
    /// The notes carry **zero value on purpose**. A structured salt on a value-bearing note
    /// would reuse the one-time-pad nonce that masks the amount, letting an observer
    /// subtract two ciphertexts and recover the difference. Zero-amount notes have no
    /// amount variance to leak, which is what makes the salt lane safe at all.
    pub fn write_message(
        &self,
        token: Felt,
        message_index: u32,
        message: &WireMessage,
    ) -> Result<ActionSet, ChannelError> {
        let salts = encode_message(message)?;
        let first = message_index * NOTES_PER_MESSAGE as u32;

        let mut builder = ActionSetBuilder::new();
        for (slot, salt) in salts.iter().enumerate() {
            builder.push(self.data_note(token, first + slot as u32, *salt))?;
        }
        Ok(builder.build()?)
    }

    /// Writes the next negotiation message, allocating its indices from `cursor`.
    ///
    /// Prefer this over [`Channel::write_message`]. The pool checks contiguity
    /// (`INDEX_NOT_SEQUENTIAL`) and single-use (`NON_ZERO_VALUE`) *after* the proof, so a
    /// caller-chosen index that is wrong costs a proof to discover. Routing every write
    /// through one allocator makes both rules hold by construction.
    ///
    /// The cursor advances only if the whole message encodes and validates, so a rejected
    /// message does not burn indices.
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

    /// A value-bearing note. Requires a [`RandomSalt`] — a structured salt here would leak
    /// the amount under mask reuse, which is why the two salt types are distinct.
    fn value_note(
        &self,
        token: Felt,
        index: u32,
        amount: u128,
        salt: RandomSalt,
    ) -> ClientAction {
        ClientAction::CreateEncNote(CreateEncNoteInput {
            recipient_addr: self.counterparty.address,
            recipient_public_key: self.counterparty.public_key,
            token,
            amount,
            index,
            salt: salt.salt(),
        })
    }

    /// Builds the action set that accepts an offer **and pays for it, atomically**.
    ///
    /// This is the operation the whole design exists for. Acceptance and payment go into
    /// one action set, so they share one proof: either both land or neither does. There is
    /// no reachable state where the counterparty has an acceptance on record and no money.
    ///
    /// The set is ordered by the contract's phases — spends (phase 4) strictly before note
    /// creation (phase 5) — which [`ActionSetBuilder`] enforces, so an accidental
    /// create-then-spend is rejected here rather than reverting after a proof.
    ///
    /// The payment note takes a [`RandomSalt`]; only the acceptance record carries
    /// structure. Mixing those up is the confidentiality bug the type split prevents.
    pub fn accept_and_settle(
        &self,
        token: Felt,
        spend: &[OwnedNote],
        payment: Payment,
        acceptance: Acceptance,
    ) -> Result<ActionSet, ChannelError> {
        if acceptance.message.message_type != MessageType::Accept {
            return Err(ChannelError::NotAnAcceptance(acceptance.message.message_type));
        }
        if payment.amount == 0 {
            return Err(ChannelError::ZeroPayment);
        }
        if spend.is_empty() {
            return Err(ChannelError::NothingToSpend);
        }

        // The payment note and the acceptance notes share one subchannel index space.
        let acceptance_first = acceptance.message_index * NOTES_PER_MESSAGE as u32;
        let acceptance_range = acceptance_first..acceptance_first + NOTES_PER_MESSAGE as u32;
        if acceptance_range.contains(&payment.index) {
            return Err(ChannelError::IndexCollision {
                payment: payment.index,
                acceptance_first,
            });
        }

        let salts = encode_message(&acceptance.message)?;
        let mut builder = ActionSetBuilder::new();

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
        // Order matters here and it is not obvious. `compile_actions` runs the set through
        // `compile_and_panic`, and `_client_apply_actions` applies each `WriteOnce` as it
        // walks (`privacy.cairo:761`) — so the contiguity check on note `n` sees notes this
        // same set created earlier. Emitting the payment before a lower-indexed acceptance
        // record would fail `INDEX_NOT_SEQUENTIAL` against a slot the set was about to fill.
        //
        // Sorting is the SDK doing its job rather than papering over a caller error: the
        // caller picks *which* indices, there is only one legal order to write them in.
        let mut creates: Vec<(u32, ClientAction)> = Vec::with_capacity(1 + NOTES_PER_MESSAGE);
        creates.push((
            payment.index,
            self.value_note(token, payment.index, payment.amount, payment.salt),
        ));
        for (slot, salt) in salts.iter().enumerate() {
            let index = acceptance_first + slot as u32;
            creates.push((index, self.data_note(token, index, *salt)));
        }
        creates.sort_by_key(|(index, _)| *index);
        for (_, action) in creates {
            builder.push(action)?;
        }

        Ok(builder.build()?)
    }

    /// Accepts and settles using the next free indices, allocated from `cursor`.
    ///
    /// Layout is the acceptance record on the message grid, then the payment note directly
    /// after it. That ordering is forced: the record has to stay on the `4k..4k+3` grid or
    /// the counterparty's reader misframes every later message, so the odd-sized payment
    /// note can only go after it.
    ///
    /// The consequence is that **settlement leaves the cursor off the message grid**, and a
    /// second negotiation in the same subchannel cannot start. That is currently correct —
    /// one channel, one deal — but it is a real constraint on multi-deal subchannels and is
    /// recorded as an open question in `docs/poulav.md` P1.3.
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
        let message_index = cursor.next_message_index()?;
        let payment_index = message_index * NOTES_PER_MESSAGE as u32 + NOTES_PER_MESSAGE as u32;

        let set = self.accept_and_settle(
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
        )?;

        cursor.reserve_message()?;
        cursor.reserve_note()?;
        Ok((message_index, set))
    }
}

/// A note this agent owns and may spend.
///
/// `channel_key` is the channel the note *arrived* in, which is not this channel — notes
/// are owned in the direction they were sent. Channels being directional means a spend and
/// the payment it funds reference two different keys.
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
