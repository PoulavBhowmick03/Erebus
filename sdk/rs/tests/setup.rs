//! Tests for the setup path: register, open channel, open subchannel.
//!
//! This is the cold-start sequence before two agents can say anything to each other. It
//! folds into a single action set and therefore a single proof, which matters because at
//! ~29 s each, three separate transactions would be a minute and a half of dead air before
//! the first offer.

use erebus_sdk::actions::{ActionError, ClientAction, FeltEntropy};
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
    let channel = Channel::derive(&alice(), bob());
    let set = channel.setup(&alice(), params(true)).expect("valid setup");

    assert_eq!(set.actions().len(), 3);
    assert!(matches!(set.actions()[0], ClientAction::SetViewingKey(_)));
    assert!(matches!(set.actions()[1], ClientAction::OpenChannel(_)));
    assert!(matches!(set.actions()[2], ClientAction::OpenSubchannel(_)));
}

/// A returning agent skips registration — the viewing key is immutable once set, so a
/// second `SetViewingKey` reverts on the WriteOnce.
#[test]
fn setup_without_registration_omits_it() {
    let channel = Channel::derive(&alice(), bob());
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
    let channel = Channel::derive(&alice(), bob());
    let set = channel.setup(&alice(), params(true)).expect("valid setup");

    let phases: Vec<u8> = set.actions().iter().map(|a| a.phase()).collect();
    assert_eq!(phases, vec![0, 1, 2]);
    assert!(phases.windows(2).all(|w| w[0] <= w[1]));
}

// --- Field wiring ---------------------------------------------------------------

#[test]
fn the_channel_action_addresses_the_counterparty() {
    let channel = Channel::derive(&alice(), bob());
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
    let channel = Channel::derive(&alice(), bob());
    let action = channel.open_subchannel(0, token(), entropy(0xcc));

    let ClientAction::OpenSubchannel(input) = action else {
        panic!("expected OpenSubchannel");
    };
    assert_eq!(input.channel_key, channel.key());
    assert_eq!(input.token, token());
    assert_eq!(input.recipient_addr, bob().address);
    assert_eq!(input.recipient_public_key, bob().public_key);
}

/// A subchannel is per token, so two tokens are two subchannels within one channel — which
/// is why the wire format can leave `token` out of a message.
#[test]
fn two_tokens_need_two_subchannels_on_the_same_channel() {
    let channel = Channel::derive(&alice(), bob());
    let first = channel.open_subchannel(0, token(), entropy(0xcc));
    let second = channel.open_subchannel(
        1,
        Felt::from_hex("0x9999").expect("token"),
        entropy(0xdd),
    );

    let (ClientAction::OpenSubchannel(a), ClientAction::OpenSubchannel(b)) = (first, second)
    else {
        panic!("expected OpenSubchannel");
    };
    assert_eq!(a.channel_key, b.channel_key, "same channel");
    assert_ne!(a.token, b.token, "different tokens");
    assert_ne!(a.index, b.index, "different subchannel indices");
}

// --- Setup then talk ------------------------------------------------------------

/// The sequence an agent actually runs: set up, then send. Two transactions, two proofs,
/// and the channel key is the same in both.
#[test]
fn setup_and_the_first_message_agree_on_the_channel() {
    use erebus_sdk::wire::{MessageType, WireMessage};

    let channel = Channel::derive(&alice(), bob());
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
    let message = channel.write_message(token(), 0, &offer).expect("valid message");

    let ClientAction::CreateEncNote(note) = &message.actions()[0] else {
        panic!("expected CreateEncNote");
    };
    assert_eq!(subchannel.token, note.token, "same subchannel token");
    assert_eq!(subchannel.recipient_addr, note.recipient_addr, "same recipient");
}
