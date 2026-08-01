//! Tests for the setup path: register, open channel, open subchannel.
//!
//! This is the cold-start sequence before two agents can say anything to each other. It
//! folds into a single action set and therefore a single proof, which matters because at
//! ~29 s each, three separate transactions would be a minute and a half of dead air before
//! the first offer.

use erebus_sdk::actions::{ActionError, ClientAction, FeltEntropy, RandomSalt};
use erebus_sdk::channel::{Channel, Counterparty, PoolIdentity, SetupParams};
use starknet_types_core::felt::Felt;

fn alice() -> PoolIdentity {
    PoolIdentity::new(
        Felt::from_hex("0xa11ce").expect("addr"),
        Felt::from_hex("0x1234567890abcdef").expect("key"),
    )
}

fn bob() -> Counterparty {
    Counterparty {
        address: Felt::from_hex("0xb0b").expect("addr"),
        public_key: Felt::from_hex("0x9bcdef").expect("pubkey"),
    }
}

fn token() -> Felt {
    Felt::from_hex("0x7042").expect("token")
}

fn pool() -> Felt {
    Felt::from_hex("0x9001").expect("pool")
}

fn chain() -> Felt {
    Felt::from_hex("0x534e5f5345504f4c4941").expect("chain")
}

fn entropy(value: u64) -> FeltEntropy {
    FeltEntropy::new(Felt::from(value)).expect("non-zero")
}

fn params(register: bool) -> SetupParams {
    SetupParams {
        register: register.then(|| entropy(0x1111)),
        channel_index: 0,
        channel_random: entropy(0x2222),
        channel_salt: entropy(0x3333),
        subchannel_index: 0,
        token: token(),
        subchannel_salt: entropy(0x4444),
    }
}

// --- Entropy typing -------------------------------------------------------------

/// Constraint 5 as a type rather than a comment: channel entropy is a `felt252` with a
/// non-zero requirement, note salts are 120-bit `u128`s. They are not interchangeable and
/// the compiler now says so.
#[test]
fn zero_entropy_is_rejected() {
    assert_eq!(
        FeltEntropy::new(Felt::ZERO).unwrap_err(),
        ActionError::ZeroEntropy
    );
}

#[test]
fn entropy_round_trips() {
    let value = Felt::from_hex("0xdeadbeef").expect("felt");
    assert_eq!(FeltEntropy::new(value).expect("non-zero").get(), value);
}

// --- Setup composition ----------------------------------------------------------

#[test]
fn full_setup_is_three_actions_in_one_set() {
    let channel = Channel::derive(chain(), pool(), &alice(), bob());
    let set = channel.setup(&alice(), params(true)).expect("valid setup");

    assert_eq!(set.actions().len(), 3);
    assert!(matches!(set.actions()[0], ClientAction::SetViewingKey(_)));
    assert!(matches!(set.actions()[1], ClientAction::OpenChannel(_)));
    assert!(matches!(set.actions()[2], ClientAction::OpenSubchannel(_)));
}

/// A returning agent skips registration because the viewing key is immutable. A second
/// `SetViewingKey` reverts on `WriteOnce`.
#[test]
fn setup_without_registration_omits_it() {
    let channel = Channel::derive(chain(), pool(), &alice(), bob());
    let set = channel.setup(&alice(), params(false)).expect("valid setup");

    assert_eq!(set.actions().len(), 2);
    assert!(!set
        .actions()
        .iter()
        .any(|a| matches!(a, ClientAction::SetViewingKey(_))));
}

/// Phases run ACCOUNT(0) → CHANNEL(1) → SUBCHANNEL(2). The builder enforces it, so a
/// reordering would be caught here rather than reverting after a proof.
#[test]
fn setup_actions_are_in_ascending_phase_order() {
    let channel = Channel::derive(chain(), pool(), &alice(), bob());
    let set = channel.setup(&alice(), params(true)).expect("valid setup");

    let phases: Vec<u8> = set.actions().iter().map(|a| a.phase()).collect();
    assert_eq!(phases, vec![0, 1, 2]);
    assert!(phases.windows(2).all(|w| w[0] <= w[1]));
}

// --- Field wiring ---------------------------------------------------------------

#[test]
fn the_channel_action_addresses_the_counterparty() {
    let channel = Channel::derive(chain(), pool(), &alice(), bob());
    let action = channel.open_channel(3, entropy(0xaa), entropy(0xbb));

    let ClientAction::OpenChannel(input) = action else {
        panic!("expected OpenChannel");
    };
    assert_eq!(input.recipient_addr, bob().address);
    assert_eq!(input.index, 3);
    assert_eq!(input.random, Felt::from(0xaa));
    assert_eq!(input.salt, Felt::from(0xbb));
}

/// The subchannel carries the channel key the counterparty will use to locate notes. If
/// this were ever the wrong key, every note would land somewhere neither party reads.
#[test]
fn the_subchannel_action_carries_this_channels_key() {
    let channel = Channel::derive(chain(), pool(), &alice(), bob());
    let action = channel.open_subchannel(0, token(), entropy(0xcc));

    let ClientAction::OpenSubchannel(input) = action else {
        panic!("expected OpenSubchannel");
    };
    assert_eq!(input.channel_key, channel.key());
    assert_eq!(input.token, token());
    assert_eq!(input.recipient_addr, bob().address);
    assert_eq!(input.recipient_public_key, bob().public_key);
}

/// Each token uses a separate subchannel. The wire message can omit its token because the
/// subchannel identifies it.
#[test]
fn two_tokens_need_two_subchannels_on_the_same_channel() {
    let channel = Channel::derive(chain(), pool(), &alice(), bob());
    let first = channel.open_subchannel(0, token(), entropy(0xcc));
    let second =
        channel.open_subchannel(1, Felt::from_hex("0x9999").expect("token"), entropy(0xdd));

    let (ClientAction::OpenSubchannel(a), ClientAction::OpenSubchannel(b)) = (first, second) else {
        panic!("expected OpenSubchannel");
    };
    assert_eq!(a.channel_key, b.channel_key, "same channel");
    assert_ne!(a.token, b.token, "different tokens");
    assert_ne!(a.index, b.index, "different subchannel indices");
}

// --- Setup then talk ------------------------------------------------------------

/// The sequence an agent runs: set up, then send. Two transactions, two proofs,
/// and the channel key is the same in both.
#[test]
fn setup_and_the_first_message_agree_on_the_channel() {
    use erebus_sdk::wire::{MessageType, WireMessage};

    let channel = Channel::derive(chain(), pool(), &alice(), bob());
    let setup = channel.setup(&alice(), params(true)).expect("valid setup");

    let ClientAction::OpenSubchannel(subchannel) = &setup.actions()[2] else {
        panic!("expected OpenSubchannel");
    };

    let offer = WireMessage {
        message_type: MessageType::Offer,
        reply_to: None,
        created_at: 1_753_699_200,
        amount: 1_000_000,
        deadline: 1_753_702_800,
        memo_hash: 1,
    };
    let message = channel
        .write_message(token(), 0, &offer)
        .expect("valid message");

    let ClientAction::CreateEncNote(note) = &message.actions()[0] else {
        panic!("expected CreateEncNote");
    };
    assert_eq!(subchannel.token, note.token, "same subchannel token");
    assert_eq!(
        subchannel.recipient_addr, note.recipient_addr,
        "same recipient"
    );
}

#[test]
fn shield_is_one_balanced_replay_protected_action_set() {
    let identity = alice();
    let self_counterparty = Counterparty {
        address: identity.address(),
        public_key: identity.public_key(),
    };
    let channel = Channel::derive(chain(), pool(), &identity, self_counterparty);
    let set = channel
        .shield(
            &identity,
            params(true),
            1_000,
            RandomSalt::from_entropy([0x42; 16]),
        )
        .expect("shield");

    assert_eq!(set.actions().len(), 5);
    assert!(matches!(set.actions()[0], ClientAction::SetViewingKey(_)));
    assert!(matches!(set.actions()[1], ClientAction::OpenChannel(_)));
    assert!(matches!(set.actions()[2], ClientAction::OpenSubchannel(_)));
    let ClientAction::Deposit(deposit) = &set.actions()[3] else {
        panic!("expected Deposit");
    };
    let ClientAction::CreateEncNote(note) = &set.actions()[4] else {
        panic!("expected CreateEncNote");
    };
    assert_eq!(deposit.amount, note.amount);
    assert_eq!(note.recipient_addr, identity.address());
    assert_eq!(note.index, 0);
}

/// A second shield must not re-open the channel.
///
/// The channel key takes no index, so the self-channel has one WriteOnce marker and the
/// first shield claims it permanently. Re-opening reverts with a bare `NON_ZERO_VALUE`
/// after the proof and the fee are already spent, which is how a funded-looking identity
/// ends up unable to top up. See friction.md F32.
#[test]
fn topping_up_reuses_the_channel_instead_of_reopening_it() {
    let identity = alice();
    let self_counterparty = Counterparty {
        address: identity.address(),
        public_key: identity.public_key(),
    };
    let channel = Channel::derive(chain(), pool(), &identity, self_counterparty);
    let set = channel
        .deposit_into_open_channel(token(), 1, 2_500, RandomSalt::from_entropy([0x7; 16]))
        .expect("top up");

    assert_eq!(set.actions().len(), 2, "deposit and note only");
    let ClientAction::Deposit(deposit) = &set.actions()[0] else {
        panic!("expected Deposit");
    };
    let ClientAction::CreateEncNote(note) = &set.actions()[1] else {
        panic!("expected CreateEncNote");
    };
    assert!(
        !set.actions()
            .iter()
            .any(|action| matches!(action, ClientAction::OpenChannel(_))),
        "re-opening the self-channel is the revert this path exists to avoid",
    );
    assert!(
        !set.actions()
            .iter()
            .any(|action| matches!(action, ClientAction::OpenSubchannel(_))),
        "the subchannel is write-once too",
    );
    assert_eq!(deposit.amount, note.amount, "action set stays balanced");
    assert_eq!(note.index, 1, "appends at the requested index");
    assert_eq!(note.recipient_addr, identity.address());
}

/// The top-up note must land in the same channel the first shield opened, or it is
/// invisible to discovery and the funds are stranded.
#[test]
fn a_top_up_note_addresses_the_same_self_channel_as_the_first_shield() {
    let identity = alice();
    let self_counterparty = Counterparty {
        address: identity.address(),
        public_key: identity.public_key(),
    };
    let channel = Channel::derive(chain(), pool(), &identity, self_counterparty);

    let first = channel
        .shield(
            &identity,
            params(false),
            1_000,
            RandomSalt::from_entropy([0x42; 16]),
        )
        .expect("shield");
    let second = channel
        .deposit_into_open_channel(token(), 1, 1_000, RandomSalt::from_entropy([0x43; 16]))
        .expect("top up");

    let ClientAction::CreateEncNote(opening) = &first.actions()[3] else {
        panic!("expected CreateEncNote");
    };
    let ClientAction::CreateEncNote(topup) = &second.actions()[1] else {
        panic!("expected CreateEncNote");
    };
    // Equal sender, recipient, and token fields derive the same channel. Only the note index
    // differs.
    assert_eq!(opening.recipient_addr, topup.recipient_addr);
    assert_eq!(opening.recipient_public_key, topup.recipient_public_key);
    assert_eq!(opening.token, topup.token);
    assert_eq!(opening.index + 1, topup.index, "indices stay contiguous");
}

#[test]
fn a_zero_top_up_is_rejected() {
    let identity = alice();
    let self_counterparty = Counterparty {
        address: identity.address(),
        public_key: identity.public_key(),
    };
    let channel = Channel::derive(chain(), pool(), &identity, self_counterparty);
    assert!(
        channel
            .deposit_into_open_channel(token(), 1, 0, RandomSalt::from_entropy([0x7; 16]))
            .is_err(),
        "a zero deposit unbalances the set and has nothing to spend",
    );
}
