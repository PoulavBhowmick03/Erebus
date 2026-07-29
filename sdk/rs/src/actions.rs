//! `ClientAction` and its Cairo Serde encoding.
//!
//! Mirrors `packages/privacy/src/actions.cairo` in
//! `starkware-libs/starknet-privacy`. The variant order below **is** the wire format —
//! Cairo serialises an enum as `[variant_index, ...payload]`, so reordering these
//! variants silently changes what every action means on-chain.
//!
//! ## Encoding rules
//!
//! Derived from the TS oracle, not from memory (`tests/fixtures/ts-clientaction-serde.json`):
//!
//! | Cairo type | Felts |
//! |---|---|
//! | `felt252`, `ContractAddress` | 1 |
//! | `usize` (u32) | 1 |
//! | `u128` | 1 — *not* the two-limb `u256` encoding |
//! | `Span<felt252>` | `[len, ...items]` |
//!
//! The `u128` row is the one worth double-checking against upstream if a note ever fails
//! to decrypt: `u256` in Cairo Serde is two felts, and an `amount` encoded that way would
//! shift every subsequent field by one without erroring anywhere.

use starknet_types_core::felt::Felt;

/// Errors from constructing an action or one of its constrained fields.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum ActionError {
    /// A note salt was outside `(OPEN_NOTE_SALT, 2^120)`.
    #[error("note salt must be > {min} and < 2^120, got {0}", min = NoteSalt::OPEN_NOTE_SALT)]
    SaltOutOfRange(u128),
    /// A channel-level `random` or `salt` was zero, which the contract rejects.
    #[error("channel entropy must be non-zero")]
    ZeroEntropy,
}

/// Execution phases. An action set must be ordered by non-decreasing phase, and at most
/// one invoke-phase action is permitted per transaction
/// (`ClientActionTrait::assert_and_advance_phase`).
pub mod phase {
    /// `SetViewingKey`.
    pub const ACCOUNT: u8 = 0;
    /// `OpenChannel`.
    pub const CHANNEL: u8 = 1;
    /// `OpenSubchannel`.
    pub const SUBCHANNEL: u8 = 2;
    /// `Deposit`.
    pub const DEPOSIT: u8 = 3;
    /// `UseNote`.
    pub const USE_NOTES: u8 = 4;
    /// `CreateEncNote`, `CreateOpenNote`.
    pub const CREATE_NOTES: u8 = 5;
    /// `Withdraw`.
    pub const WITHDRAW: u8 = 6;
    /// `InvokeExternal`, `ComputeAndInvoke`.
    pub const INVOKE: u8 = 7;
}

/// Salt of an encrypted note, constrained to `(1, 2^120)`.
///
/// A newtype rather than a bare `u128` because the bound is not checkable after the fact:
/// an out-of-range salt is rejected by the contract, but a salt that is merely *wrong*
/// derives a storage slot nobody wrote to and the note is silently "not found".
/// `salt == 0` means the note does not exist; `salt == 1` is reserved for open notes.
///
/// Erebus's negotiation payload rides in these salts with bit 119 pinned to 1, so a
/// well-formed payload salt is always in `[2^119, 2^120)` — a strict subset of what this
/// type permits. See ARCHITECTURE §7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct NoteSalt(u128);

impl NoteSalt {
    /// Reserved salt marking an open (unencrypted) note.
    pub const OPEN_NOTE_SALT: u128 = 1;
    /// Exclusive upper bound: salts are 120-bit.
    pub const TWO_POW_120: u128 = 1 << 120;

    /// Constructs a salt, rejecting anything outside `(OPEN_NOTE_SALT, 2^120)`.
    pub fn new(value: u128) -> Result<Self, ActionError> {
        if value <= Self::OPEN_NOTE_SALT || value >= Self::TWO_POW_120 {
            return Err(ActionError::SaltOutOfRange(value));
        }
        Ok(Self(value))
    }

    /// The underlying value.
    pub fn get(self) -> u128 {
        self.0
    }
}

/// Caller-supplied entropy for a channel-level field: a `random` or a `salt` on
/// `SetViewingKey`, `OpenChannel`, `OpenSubchannel` or `CreateOpenNote`.
///
/// **Deliberately a different type from [`NoteSalt`].** The contract's salts are not
/// uniform: a note salt is a `u128` bounded to 120 bits, while these are full `felt252`
/// values with only a non-zero requirement. The OpenZeppelin audit flagged that
/// inconsistency and StarkWare acknowledged it without changing it, so it is a permanent
/// feature of the surface. Two types make a mix-up a compile error instead of a note that
/// silently fails to decrypt.
///
/// The contract only checks non-zero. Unpredictability is the caller's responsibility, and
/// it matters: these are one-time key nonces, and a repeat leaks the relationship they were
/// meant to hide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeltEntropy(Felt);

impl FeltEntropy {
    /// Wraps a felt, rejecting zero.
    pub fn new(value: Felt) -> Result<Self, ActionError> {
        if value == Felt::ZERO {
            return Err(ActionError::ZeroEntropy);
        }
        Ok(Self(value))
    }

    /// The underlying felt.
    pub fn get(self) -> Felt {
        self.0
    }
}

/// A salt for a **value-bearing** note, which must be unpredictable.
///
/// A separate type from a structured salt, because the two are not interchangeable and
/// swapping them is a confidentiality bug rather than a compile error otherwise.
///
/// The salt is the one-time-pad nonce masking a note's encrypted amount. Reuse a mask
/// across two notes with different amounts and an observer can subtract the ciphertexts to
/// recover the difference — so a salt carrying *structure* (an offer, a counter, anything
/// with a predictable field layout) must never sit on a note that carries value.
/// Zero-amount notes have no amount variance and are immune, which is what makes the salt
/// lane safe.
///
/// This type does not generate entropy itself: the crate stays RNG-free so every test is
/// deterministic. The caller supplies 16 bytes from a CSPRNG.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RandomSalt(NoteSalt);

impl RandomSalt {
    /// Builds a salt from 16 bytes of entropy.
    ///
    /// The top byte is masked to keep the value inside the contract's 120-bit bound, and
    /// the result is forced above `OPEN_NOTE_SALT`, so any input produces a valid salt.
    /// **The bytes must come from a cryptographically secure source** — a predictable salt
    /// defeats the amount masking exactly as reuse does.
    pub fn from_entropy(bytes: [u8; 16]) -> Self {
        let raw = u128::from_le_bytes(bytes) & (NoteSalt::TWO_POW_120 - 1);
        // 0 and 1 are reserved; nudge into range rather than rejecting, so a caller
        // cannot end up retrying entropy.
        let value = if raw <= NoteSalt::OPEN_NOTE_SALT { raw + 2 } else { raw };
        Self(NoteSalt::new(value).expect("masked into range by construction"))
    }

    /// The underlying salt.
    pub fn salt(self) -> NoteSalt {
        self.0
    }
}

/// Register a user with a viewing key. Immutable once set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetViewingKeyInput {
    /// Encrypts the private key for the auditor.
    pub random: Felt,
}

/// Open a channel from the user to a recipient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenChannelInput {
    /// The recipient's address.
    pub recipient_addr: Felt,
    /// Channel index within the sender's outgoing channels.
    pub index: u32,
    /// Encrypts the channel info for the recipient.
    pub random: Felt,
    /// Guarantees one-time key usage for the encrypted outgoing channel info.
    pub salt: Felt,
}

/// Open a subchannel within a channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenSubchannelInput {
    /// The recipient's address.
    pub recipient_addr: Felt,
    /// The recipient's public key.
    pub recipient_public_key: Felt,
    /// The channel key of the subchannel.
    pub channel_key: Felt,
    /// Index of the subchannel within the channel.
    pub index: u32,
    /// The subchannel's token.
    pub token: Felt,
    /// Encrypts the subchannel token.
    pub salt: Felt,
}

/// Create an encrypted note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateEncNoteInput {
    /// The recipient's address.
    pub recipient_addr: Felt,
    /// The recipient's public key.
    pub recipient_public_key: Felt,
    /// The token's address.
    pub token: Felt,
    /// The amount the note represents. Zero is permitted — that is what carries a
    /// structured salt without moving value.
    pub amount: u128,
    /// Index of the note within the channel. Indices must be contiguous.
    pub index: u32,
    /// One-time salt.
    pub salt: NoteSalt,
}

/// Create an open (unencrypted, zero-value) note to be deposited to by a server action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateOpenNoteInput {
    /// The recipient's address.
    pub recipient_addr: Felt,
    /// The recipient's public key.
    pub recipient_public_key: Felt,
    /// The token's address.
    pub token: Felt,
    /// Index of the note within the channel.
    pub index: u32,
    /// Encrypts the recipient address for the auditor.
    pub random: Felt,
}

/// Deposit funds into the pool. Requires a screening attestation on `apply_actions`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepositInput {
    /// The token's address.
    pub token: Felt,
    /// The amount to deposit.
    pub amount: u128,
}

/// Consume a note, creating a nullifier for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseNoteInput {
    /// The channel key of the note's channel.
    pub channel_key: Felt,
    /// The note's token address.
    pub token: Felt,
    /// Index of the note within the channel.
    pub index: u32,
}

/// Withdraw funds from the pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithdrawInput {
    /// The address to withdraw to.
    pub to_addr: Felt,
    /// The token's address.
    pub token: Felt,
    /// The amount to withdraw.
    pub amount: u128,
    /// Encrypts the user address for the auditor.
    pub random: Felt,
}

/// Invoke an external contract from inside the private transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvokeExternalInput {
    /// The target contract.
    pub contract_address: Felt,
    /// Calldata forwarded to the target.
    pub calldata: Vec<Felt>,
}

/// Run `privacy_compute` on the target and forward its result to
/// `privacy_invoke_with_computation`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputeAndInvokeInput {
    /// The target contract.
    pub contract_address: Felt,
    /// Appended after the derived `identity_key` for the compute call.
    pub compute_additional_data: Vec<Felt>,
    /// Appended after the compute result for the invoke call.
    pub invoke_additional_data: Vec<Felt>,
}

/// An action to be executed by the client.
///
/// **Variant order is the wire format.** Do not reorder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientAction {
    /// Register a viewing key.
    SetViewingKey(SetViewingKeyInput),
    /// Open a channel.
    OpenChannel(OpenChannelInput),
    /// Open a subchannel.
    OpenSubchannel(OpenSubchannelInput),
    /// Create an encrypted note.
    CreateEncNote(CreateEncNoteInput),
    /// Create an open note.
    CreateOpenNote(CreateOpenNoteInput),
    /// Deposit into the pool.
    Deposit(DepositInput),
    /// Consume a note.
    UseNote(UseNoteInput),
    /// Withdraw from the pool.
    Withdraw(WithdrawInput),
    /// Invoke an external contract.
    InvokeExternal(InvokeExternalInput),
    /// Compute then invoke.
    ComputeAndInvoke(ComputeAndInvokeInput),
}

/// Appends a `Span<felt252>` as `[len, ...items]`.
fn push_span(out: &mut Vec<Felt>, items: &[Felt]) {
    out.push(Felt::from(items.len() as u64));
    out.extend_from_slice(items);
}

impl ClientAction {
    /// The Cairo enum variant index. This is the first felt of the encoding.
    pub fn variant_index(&self) -> u8 {
        match self {
            Self::SetViewingKey(_) => 0,
            Self::OpenChannel(_) => 1,
            Self::OpenSubchannel(_) => 2,
            Self::CreateEncNote(_) => 3,
            Self::CreateOpenNote(_) => 4,
            Self::Deposit(_) => 5,
            Self::UseNote(_) => 6,
            Self::Withdraw(_) => 7,
            Self::InvokeExternal(_) => 8,
            Self::ComputeAndInvoke(_) => 9,
        }
    }

    /// The execution phase this action belongs to.
    ///
    /// Note this is *not* the variant index: `CreateEncNote` and `CreateOpenNote` share a
    /// phase, as do the two invoke variants, and `UseNote` runs before note creation
    /// despite having a higher variant index.
    pub fn phase(&self) -> u8 {
        match self {
            Self::SetViewingKey(_) => phase::ACCOUNT,
            Self::OpenChannel(_) => phase::CHANNEL,
            Self::OpenSubchannel(_) => phase::SUBCHANNEL,
            Self::Deposit(_) => phase::DEPOSIT,
            Self::UseNote(_) => phase::USE_NOTES,
            Self::CreateEncNote(_) | Self::CreateOpenNote(_) => phase::CREATE_NOTES,
            Self::Withdraw(_) => phase::WITHDRAW,
            Self::InvokeExternal(_) | Self::ComputeAndInvoke(_) => phase::INVOKE,
        }
    }

    /// Appends this action's Cairo Serde encoding to `out`.
    pub fn serialize_into(&self, out: &mut Vec<Felt>) {
        out.push(Felt::from(self.variant_index()));
        match self {
            Self::SetViewingKey(i) => out.push(i.random),
            Self::OpenChannel(i) => {
                out.push(i.recipient_addr);
                out.push(Felt::from(i.index));
                out.push(i.random);
                out.push(i.salt);
            }
            Self::OpenSubchannel(i) => {
                out.push(i.recipient_addr);
                out.push(i.recipient_public_key);
                out.push(i.channel_key);
                out.push(Felt::from(i.index));
                out.push(i.token);
                out.push(i.salt);
            }
            Self::CreateEncNote(i) => {
                out.push(i.recipient_addr);
                out.push(i.recipient_public_key);
                out.push(i.token);
                out.push(Felt::from(i.amount));
                out.push(Felt::from(i.index));
                out.push(Felt::from(i.salt.get()));
            }
            Self::CreateOpenNote(i) => {
                out.push(i.recipient_addr);
                out.push(i.recipient_public_key);
                out.push(i.token);
                out.push(Felt::from(i.index));
                out.push(i.random);
            }
            Self::Deposit(i) => {
                out.push(i.token);
                out.push(Felt::from(i.amount));
            }
            Self::UseNote(i) => {
                out.push(i.channel_key);
                out.push(i.token);
                out.push(Felt::from(i.index));
            }
            Self::Withdraw(i) => {
                out.push(i.to_addr);
                out.push(i.token);
                out.push(Felt::from(i.amount));
                out.push(i.random);
            }
            Self::InvokeExternal(i) => {
                out.push(i.contract_address);
                push_span(out, &i.calldata);
            }
            Self::ComputeAndInvoke(i) => {
                out.push(i.contract_address);
                push_span(out, &i.compute_additional_data);
                push_span(out, &i.invoke_additional_data);
            }
        }
    }

    /// This action's Cairo Serde encoding.
    pub fn serialize(&self) -> Vec<Felt> {
        let mut out = Vec::new();
        self.serialize_into(&mut out);
        out
    }
}

/// Encodes an action set as the `Span<ClientAction>` argument of `compile_actions`:
/// `[len, ...actions]`.
pub fn serialize_actions(actions: &[ClientAction]) -> Vec<Felt> {
    let mut out = Vec::with_capacity(actions.len() + 1);
    out.push(Felt::from(actions.len() as u64));
    for action in actions {
        action.serialize_into(&mut out);
    }
    out
}
