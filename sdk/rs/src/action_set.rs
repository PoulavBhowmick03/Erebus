//! Ordered, validated action sets.
//!
//! The pool checks three properties in `privacy.cairo:251-310`. This type checks them before
//! proof generation and avoids a failed prover round-trip of about 29 seconds.
//!
//! 1. Phase order cannot decrease. `assert_and_advance_phase` returns
//!    `ACTIONS_OUT_OF_ORDER` when an action is below the current phase.
//! 2. A set can contain one invoke action. An invoke moves the phase to `INVOKE + 1`, which
//!    no later action can satisfy.
//! 3. One action must compile to `ServerAction::WriteOnce`. Otherwise the pool returns
//!    `NO_REPLAY_PROTECTION`.
//!
//! ## Replay protection
//!
//! Deposit, Withdraw, InvokeExternal and ComputeAndInvoke do not produce `WriteOnce`. A
//! shield by itself reverts. A deposit normally includes the note that receives its value.
//! Without that note, the contract reports a replay-protection error.
//!
//! ## Balance checks
//!
//! This type does not check token balances because it cannot see the amounts of consumed
//! notes. `token_balances.squash().assert_valid()` checks each token at runtime.

use crate::actions::{phase, ClientAction};

/// Errors assembling an action set.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ActionSetError {
    /// An action's phase is below the running maximum.
    #[error(
        "actions out of order: {action} is phase {action_phase}, but the set has reached phase {current_phase}"
    )]
    OutOfOrder {
        /// Variant name of the offending action.
        action: &'static str,
        /// The phase it belongs to.
        action_phase: u8,
        /// The phase the set has already reached.
        current_phase: u8,
    },
    /// A second invoke-phase action was added.
    #[error("only one invoke-phase action is allowed per transaction")]
    SecondInvoke,
    /// No action in the set compiles to a `WriteOnce`.
    #[error(
        "no action provides replay protection: the set needs at least one that writes storage \
         (a note, channel, subchannel, nullifier or viewing key)"
    )]
    NoReplayProtection,
    /// The set was empty.
    #[error("an action set must contain at least one action")]
    Empty,
}

/// Whether an action compiles to at least one `ServerAction::WriteOnce`.
///
/// The pool action handlers define this list:
/// `set_viewing_key:337`, `open_channel:416`, `open_subchannel:467`, `use_note:622`,
/// `create_enc_note:666`, `create_open_note:707` all emit one. `deposit:496`,
/// `withdraw:524`, `invoke_external:538` and `compute_and_invoke:576` do not.
fn provides_replay_protection(action: &ClientAction) -> bool {
    match action {
        ClientAction::SetViewingKey(_)
        | ClientAction::OpenChannel(_)
        | ClientAction::OpenSubchannel(_)
        | ClientAction::CreateEncNote(_)
        | ClientAction::CreateOpenNote(_)
        | ClientAction::UseNote(_) => true,
        ClientAction::Deposit(_)
        | ClientAction::Withdraw(_)
        | ClientAction::InvokeExternal(_)
        | ClientAction::ComputeAndInvoke(_) => false,
    }
}

fn variant_name(action: &ClientAction) -> &'static str {
    match action {
        ClientAction::SetViewingKey(_) => "SetViewingKey",
        ClientAction::OpenChannel(_) => "OpenChannel",
        ClientAction::OpenSubchannel(_) => "OpenSubchannel",
        ClientAction::CreateEncNote(_) => "CreateEncNote",
        ClientAction::CreateOpenNote(_) => "CreateOpenNote",
        ClientAction::Deposit(_) => "Deposit",
        ClientAction::UseNote(_) => "UseNote",
        ClientAction::Withdraw(_) => "Withdraw",
        ClientAction::InvokeExternal(_) => "InvokeExternal",
        ClientAction::ComputeAndInvoke(_) => "ComputeAndInvoke",
    }
}

/// Client actions that satisfy the pool's ordering and replay rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionSet(Vec<ClientAction>);

impl ActionSet {
    /// The actions, in submission order.
    pub fn actions(&self) -> &[ClientAction] {
        &self.0
    }

    /// Consumes the set, returning the actions.
    pub fn into_actions(self) -> Vec<ClientAction> {
        self.0
    }

    /// Cairo Serde encoding of the whole span.
    pub fn serialize(&self) -> Vec<starknet_types_core::felt::Felt> {
        crate::actions::serialize_actions(&self.0)
    }
}

/// Accumulates actions and reports the action that breaks phase order.
#[derive(Debug, Default)]
pub struct ActionSetBuilder {
    actions: Vec<ClientAction>,
    current_phase: u8,
    invoke_used: bool,
    has_replay_protection: bool,
}

impl ActionSetBuilder {
    /// A new, empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends an action, mirroring `assert_and_advance_phase`.
    pub fn push(&mut self, action: ClientAction) -> Result<&mut Self, ActionSetError> {
        let action_phase = action.phase();

        if self.invoke_used {
            return Err(ActionSetError::SecondInvoke);
        }
        if action_phase < self.current_phase {
            return Err(ActionSetError::OutOfOrder {
                action: variant_name(&action),
                action_phase,
                current_phase: self.current_phase,
            });
        }

        if provides_replay_protection(&action) {
            self.has_replay_protection = true;
        }
        if action_phase == phase::INVOKE {
            self.invoke_used = true;
        }
        self.current_phase = action_phase;
        self.actions.push(action);
        Ok(self)
    }

    /// Appends an action, consuming and returning the builder for chaining.
    pub fn with(mut self, action: ClientAction) -> Result<Self, ActionSetError> {
        self.push(action)?;
        Ok(self)
    }

    /// Validates and finalises.
    pub fn build(self) -> Result<ActionSet, ActionSetError> {
        if self.actions.is_empty() {
            return Err(ActionSetError::Empty);
        }
        if !self.has_replay_protection {
            return Err(ActionSetError::NoReplayProtection);
        }
        Ok(ActionSet(self.actions))
    }
}
