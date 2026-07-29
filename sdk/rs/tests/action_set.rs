//! Tests for the ordering and replay rules the pool enforces on an action set.
//!
//! Every case here corresponds to a revert in `privacy.cairo`. The point of the builder is
//! that these fail before a proof is generated rather than after — each one otherwise costs
//! a prover round-trip and roughly 29 seconds to discover.

use erebus_sdk::action_set::{ActionSetBuilder, ActionSetError};
use erebus_sdk::actions::{
    ClientAction, CreateEncNoteInput, DepositInput, InvokeExternalInput, NoteSalt,
    OpenSubchannelInput, SetViewingKeyInput, UseNoteInput, WithdrawInput,
};
use erebus_sdk::tx::{
    DataAvailabilityMode, InvokeV3, PoolInvocation, PoolInvocationError, ResourceBound,
    ResourceBounds,
};
use starknet_types_core::felt::Felt;

fn one() -> Felt {
    Felt::ONE
}

fn set_viewing_key() -> ClientAction {
    ClientAction::SetViewingKey(SetViewingKeyInput { random: one() })
}

fn open_subchannel() -> ClientAction {
    ClientAction::OpenSubchannel(OpenSubchannelInput {
        recipient_addr: one(),
        recipient_public_key: one(),
        channel_key: one(),
        index: 0,
        token: one(),
        salt: one(),
    })
}

fn deposit() -> ClientAction {
    ClientAction::Deposit(DepositInput { token: one(), amount: 100 })
}

fn use_note(index: u32) -> ClientAction {
    ClientAction::UseNote(UseNoteInput { channel_key: one(), token: one(), index })
}

fn data_note(index: u32, salt: u128) -> ClientAction {
    ClientAction::CreateEncNote(CreateEncNoteInput {
        recipient_addr: one(),
        recipient_public_key: one(),
        token: one(),
        amount: 0,
        index,
        salt: NoteSalt::new(salt).expect("salt in range"),
    })
}

fn withdraw() -> ClientAction {
    ClientAction::Withdraw(WithdrawInput {
        to_addr: one(),
        token: one(),
        amount: 1,
        random: one(),
    })
}

fn invoke() -> ClientAction {
    ClientAction::InvokeExternal(InvokeExternalInput {
        contract_address: one(),
        calldata: vec![],
    })
}

// --- Ordering -------------------------------------------------------------------

#[test]
fn a_correctly_ordered_set_builds() {
    let set = ActionSetBuilder::new()
        .with(set_viewing_key())
        .and_then(|b| b.with(open_subchannel()))
        .and_then(|b| b.with(deposit()))
        .and_then(|b| b.with(use_note(0)))
        .and_then(|b| b.with(data_note(0, 1 << 119)))
        .and_then(|b| b.with(withdraw()))
        .and_then(|b| b.with(invoke()))
        .expect("ordering is valid")
        .build()
        .expect("set is valid");
    assert_eq!(set.actions().len(), 7);
}

/// The trap the phase table exists for: `UseNote` is variant 6 and `CreateEncNote` is
/// variant 3, so enum order suggests notes are created before they are spent. The
/// contract runs spends first.
#[test]
fn creating_a_note_before_spending_one_is_rejected() {
    let mut builder = ActionSetBuilder::new();
    builder.push(data_note(0, 1 << 119)).expect("first action always fits");
    let error = builder.push(use_note(0)).expect_err("UseNote is an earlier phase");

    assert!(matches!(
        error,
        ActionSetError::OutOfOrder { action: "UseNote", .. }
    ));
}

#[test]
fn a_second_invoke_is_rejected() {
    let mut builder = ActionSetBuilder::new();
    builder.push(data_note(0, 1 << 119)).expect("fits");
    builder.push(invoke()).expect("first invoke fits");
    assert_eq!(builder.push(invoke()).unwrap_err(), ActionSetError::SecondInvoke);
}

#[test]
fn nothing_may_follow_an_invoke() {
    // The contract advances the phase cursor past INVOKE, so even a same-phase action is
    // unreachable afterwards.
    let mut builder = ActionSetBuilder::new();
    builder.push(data_note(0, 1 << 119)).expect("fits");
    builder.push(invoke()).expect("fits");
    assert_eq!(builder.push(withdraw()).unwrap_err(), ActionSetError::SecondInvoke);
}

#[test]
fn same_phase_actions_may_repeat() {
    // Four data notes at the same phase is exactly one negotiation message.
    let mut builder = ActionSetBuilder::new();
    for index in 0..4 {
        builder
            .push(data_note(index, (1 << 119) | u128::from(index)))
            .expect("same phase repeats");
    }
    assert_eq!(builder.build().expect("valid").actions().len(), 4);
}

// --- Replay protection ----------------------------------------------------------

/// The rule that catches people out: a shield on its own reverts, because Deposit emits
/// no `WriteOnce`.
#[test]
fn a_deposit_alone_has_no_replay_protection() {
    let error = ActionSetBuilder::new()
        .with(deposit())
        .expect("fits")
        .build()
        .expect_err("deposit alone must be rejected");
    assert_eq!(error, ActionSetError::NoReplayProtection);
}

#[test]
fn a_deposit_paired_with_a_note_is_fine() {
    ActionSetBuilder::new()
        .with(deposit())
        .and_then(|b| b.with(data_note(0, 1 << 119)))
        .expect("fits")
        .build()
        .expect("deposit plus a note is valid");
}

#[test]
fn withdraw_and_invoke_alone_also_fail() {
    for action in [withdraw(), invoke()] {
        let error = ActionSetBuilder::new()
            .with(action)
            .expect("fits")
            .build()
            .expect_err("no WriteOnce means no replay protection");
        assert_eq!(error, ActionSetError::NoReplayProtection);
    }
}

#[test]
fn an_empty_set_is_rejected() {
    assert_eq!(
        ActionSetBuilder::new().build().unwrap_err(),
        ActionSetError::Empty
    );
}

#[test]
fn the_set_serialises_as_a_span() {
    let set = ActionSetBuilder::new()
        .with(data_note(0, 1 << 119))
        .expect("fits")
        .build()
        .expect("valid");
    let encoded = set.serialize();
    assert_eq!(encoded[0], Felt::ONE, "span length prefix");
}

// --- Pool invocation preconditions ----------------------------------------------

fn invocation(tip: u64, bounds: ResourceBounds) -> InvokeV3 {
    InvokeV3 {
        sender_address: one(),
        calldata: vec![],
        chain_id: one(),
        nonce: Felt::ZERO,
        account_deployment_data: vec![],
        nonce_da_mode: DataAvailabilityMode::L1,
        fee_da_mode: DataAvailabilityMode::L1,
        resource_bounds: bounds,
        tip,
        paymaster_data: vec![],
        proof_facts: vec![],
    }
}

#[test]
fn a_zero_fee_invocation_is_accepted() {
    PoolInvocation::new(invocation(0, ResourceBounds::for_proof_invocation()))
        .expect("zero tip and zero prices");
}

#[test]
fn a_non_zero_tip_is_rejected() {
    assert_eq!(
        PoolInvocation::new(invocation(1, ResourceBounds::for_proof_invocation())).unwrap_err(),
        PoolInvocationError::NonZeroTip(1)
    );
}

#[test]
fn a_non_zero_resource_price_is_rejected() {
    let mut bounds = ResourceBounds::for_proof_invocation();
    bounds.l2_gas = ResourceBound { max_amount: 100, max_price_per_unit: 7 };
    assert_eq!(
        PoolInvocation::new(invocation(0, bounds)).unwrap_err(),
        PoolInvocationError::NonZeroResourcePrice { resource: "l2_gas", price: 7 }
    );
}

#[test]
fn every_resource_is_checked() {
    for which in 0..3 {
        let mut bounds = ResourceBounds::for_proof_invocation();
        match which {
            0 => bounds.l1_gas.max_price_per_unit = 1,
            1 => bounds.l2_gas.max_price_per_unit = 1,
            _ => bounds.l1_data_gas.max_price_per_unit = 1,
        }
        assert!(
            PoolInvocation::new(invocation(0, bounds)).is_err(),
            "resource {which} was not checked"
        );
    }
}
