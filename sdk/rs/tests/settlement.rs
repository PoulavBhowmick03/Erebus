//! Tests for P2.1 atomic accept-and-settle.
//!
//! The property under test is that acceptance and payment cannot be separated. They go
//! into one action set, which becomes one proof, so the chain either applies both or
//! neither. Anything that let them split would reintroduce exactly the failure Erebus
//! claims to remove: a counterparty holding an acceptance and no money.

use erebus_sdk::actions::{ClientAction, FeltEntropy, RandomSalt};
use erebus_sdk::channel::{
    Acceptance, ChangeChannelSetup, ChangeOutput, Channel, ChannelError, Counterparty, OwnedNote,
    Payment, PoolIdentity,
};
use erebus_sdk::wire::{MessageType, WireMessage, WireVersion};
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

fn salt() -> RandomSalt {
    RandomSalt::from_entropy([
        0x9a, 0x3f, 0x11, 0x7c, 0x42, 0xd8, 0x05, 0xbe, 0x6e, 0x21, 0xa0, 0x77, 0x13, 0x94, 0xcc,
        0x58,
    ])
}

fn change_salt() -> RandomSalt {
    RandomSalt::from_entropy([
        0x31, 0x7a, 0xc4, 0x0d, 0x91, 0xee, 0x62, 0x58, 0xa3, 0x16, 0xb9, 0x44, 0x73, 0x20, 0xd5,
        0x8f,
    ])
}

fn accept_message() -> WireMessage {
    WireMessage {
        deal_id: 0,
        message_type: MessageType::Accept,
        reply_to: Some(1),
        created_at: 1_753_699_320,
        amount: 950_000,
        deadline: 1_753_702_800,
        memo_hash: 0xdead_beef,
    }
}

fn inputs() -> Vec<OwnedNote> {
    vec![OwnedNote {
        channel_key: Felt::from_hex("0xc0ffee").expect("incoming channel"),
        token: token(),
        index: 0,
    }]
}

fn settle(
    channel: &Channel,
    payment_index: u32,
    message_index: u32,
) -> Result<erebus_sdk::action_set::ActionSet, ChannelError> {
    channel.accept_and_settle(
        token(),
        &inputs(),
        Payment {
            amount: 950_000,
            index: payment_index,
            salt: salt(),
        },
        Acceptance {
            message_index,
            message: accept_message(),
        },
    )
}

// --- Atomicity ------------------------------------------------------------------

/// One action set means one proof, which means both legs land or neither does.
#[test]
fn acceptance_and_payment_land_in_one_action_set() {
    let channel = Channel::derive_with_version(chain(), pool(), &alice(), bob(), WireVersion::V2);
    let set = settle(&channel, 4, 2).expect("valid settlement");

    // 1 spend + 1 payment + 5 encrypted acceptance notes.
    assert_eq!(set.actions().len(), 7);

    let spends = set
        .actions()
        .iter()
        .filter(|a| matches!(a, ClientAction::UseNote(_)))
        .count();
    let notes = set
        .actions()
        .iter()
        .filter(|a| matches!(a, ClientAction::CreateEncNote(_)))
        .count();
    assert_eq!(spends, 1, "the input note must be consumed in this set");
    assert_eq!(notes, 6, "payment plus the five-note acceptance record");
}

/// Spends must precede creations, or the contract rejects with ACTIONS_OUT_OF_ORDER after
/// a proof has already been paid for.
#[test]
fn spends_come_before_creations() {
    let channel = Channel::derive_with_version(chain(), pool(), &alice(), bob(), WireVersion::V2);
    let set = settle(&channel, 4, 2).expect("valid settlement");

    let first_create = set
        .actions()
        .iter()
        .position(|a| matches!(a, ClientAction::CreateEncNote(_)))
        .expect("a note is created");
    let last_spend = set
        .actions()
        .iter()
        .rposition(|a| matches!(a, ClientAction::UseNote(_)))
        .expect("a note is spent");

    assert!(last_spend < first_create, "a spend followed a creation");
}

#[test]
fn multiple_inputs_are_all_consumed() {
    let channel = Channel::derive_with_version(chain(), pool(), &alice(), bob(), WireVersion::V2);
    let many: Vec<OwnedNote> = (0..3)
        .map(|index| OwnedNote {
            channel_key: Felt::from_hex("0xc0ffee").expect("channel"),
            token: token(),
            index,
        })
        .collect();

    let set = channel
        .accept_and_settle(
            token(),
            &many,
            Payment {
                amount: 950_000,
                index: 4,
                salt: salt(),
            },
            Acceptance {
                message_index: 2,
                message: accept_message(),
            },
        )
        .expect("valid");

    assert_eq!(
        set.actions()
            .iter()
            .filter(|a| matches!(a, ClientAction::UseNote(_)))
            .count(),
        3
    );
}

// --- The salt rule --------------------------------------------------------------

/// The payment note must not carry a structured salt. Value notes and data notes take
/// different salt types precisely so this cannot be got wrong by accident.
#[test]
fn the_payment_note_carries_the_random_salt_and_the_record_does_not() {
    let channel = Channel::derive_with_version(chain(), pool(), &alice(), bob(), WireVersion::V2);
    let set = settle(&channel, 4, 2).expect("valid settlement");

    let notes: Vec<_> = set
        .actions()
        .iter()
        .filter_map(|a| match a {
            ClientAction::CreateEncNote(n) => Some(n),
            _ => None,
        })
        .collect();

    let payment = notes
        .iter()
        .find(|n| n.amount > 0)
        .expect("a payment note exists");
    assert_eq!(
        payment.salt,
        salt().salt(),
        "payment must use the supplied random salt"
    );
    assert_eq!(payment.index, 4);

    let records: Vec<_> = notes.iter().filter(|n| n.amount == 0).collect();
    assert_eq!(records.len(), 5, "the acceptance record is five notes");
    for record in records {
        assert_ne!(
            record.salt,
            salt().salt(),
            "a record note must not reuse the payment's salt"
        );
    }
}

#[test]
fn exactly_one_note_carries_value() {
    let channel = Channel::derive_with_version(chain(), pool(), &alice(), bob(), WireVersion::V2);
    let set = settle(&channel, 4, 2).expect("valid settlement");
    let valued = set
        .actions()
        .iter()
        .filter(|a| matches!(a, ClientAction::CreateEncNote(n) if n.amount > 0))
        .count();
    assert_eq!(valued, 1);
}

/// The selector test in `client.rs` proves the selected
/// input is one note worth 5. This composition test proves the same atomic settlement pays
/// 3 to Bob and creates payer-owned change worth 2 with no gap in either channel.
#[test]
fn one_five_value_note_pays_three_and_retains_two_as_change() {
    let outgoing = Channel::derive_with_version(chain(), pool(), &alice(), bob(), WireVersion::V2);
    let payer = Counterparty {
        address: alice().address(),
        public_key: alice().public_key(),
    };
    let self_channel = Channel::derive(chain(), pool(), &alice(), payer);
    let mut acceptance = accept_message();
    acceptance.amount = 3;

    let set = outgoing
        .accept_and_settle_with_change(
            token(),
            &[OwnedNote {
                channel_key: self_channel.key(),
                token: token(),
                index: 0,
            }],
            Payment {
                amount: 3,
                index: 5,
                salt: salt(),
            },
            Acceptance {
                message_index: 0,
                message: acceptance,
            },
            Some(ChangeOutput::existing(self_channel, 2, 1, change_salt())),
        )
        .expect("5 is conserved as payment 3 plus change 2");

    let value_notes: Vec<_> = set
        .actions()
        .iter()
        .filter_map(|action| match action {
            ClientAction::CreateEncNote(note) if note.amount > 0 => Some(note),
            _ => None,
        })
        .collect();
    assert_eq!(value_notes.len(), 2);
    assert_eq!(value_notes.iter().map(|note| note.amount).sum::<u128>(), 5);

    let payment = value_notes
        .iter()
        .find(|note| note.recipient_addr == bob().address)
        .expect("Bob receives payment");
    assert_eq!(
        (payment.amount, payment.index, payment.salt),
        (3, 5, salt().salt())
    );

    let change = value_notes
        .iter()
        .find(|note| note.recipient_addr == alice().address())
        .expect("Alice retains change");
    assert_eq!(
        (change.amount, change.index, change.salt),
        (2, 1, change_salt().salt())
    );

    let outgoing_indices: Vec<u32> = set
        .actions()
        .iter()
        .filter_map(|action| match action {
            ClientAction::CreateEncNote(note) if note.recipient_addr == bob().address => {
                Some(note.index)
            }
            _ => None,
        })
        .collect();
    assert_eq!(outgoing_indices, vec![0, 1, 2, 3, 4, 5]);
    assert_eq!(change.index, 1, "self-channel index 0 is the spent 5 note");
}

#[test]
fn first_change_opens_the_self_channel_at_note_zero_before_spending() {
    let outgoing = Channel::derive_with_version(chain(), pool(), &alice(), bob(), WireVersion::V2);
    let payer = Counterparty {
        address: alice().address(),
        public_key: alice().public_key(),
    };
    let self_channel = Channel::derive(chain(), pool(), &alice(), payer);
    let mut acceptance = accept_message();
    acceptance.amount = 3;
    let entropy = |value| FeltEntropy::new(Felt::from(value)).expect("non-zero entropy");

    let set = outgoing
        .accept_and_settle_with_change(
            token(),
            &inputs(),
            Payment {
                amount: 3,
                index: 5,
                salt: salt(),
            },
            Acceptance {
                message_index: 0,
                message: acceptance,
            },
            Some(ChangeOutput::opening(
                self_channel,
                2,
                change_salt(),
                ChangeChannelSetup {
                    channel_index: 1,
                    channel_random: entropy(11),
                    channel_salt: entropy(12),
                    subchannel_index: 0,
                    subchannel_salt: entropy(13),
                },
            )),
        )
        .expect("opening self-channel can share the settlement action set");

    assert!(matches!(set.actions()[0], ClientAction::OpenChannel(_)));
    assert!(matches!(set.actions()[1], ClientAction::OpenSubchannel(_)));
    assert!(matches!(set.actions()[2], ClientAction::UseNote(_)));
    let change = set
        .actions()
        .iter()
        .find_map(|action| match action {
            ClientAction::CreateEncNote(note)
                if note.recipient_addr == alice().address() && note.amount == 2 =>
            {
                Some(note)
            }
            _ => None,
        })
        .expect("payer change note");
    assert_eq!(change.index, 0, "new self-subchannel must start at zero");
    assert_eq!(change.salt, change_salt().salt());
}

#[test]
fn random_salts_stay_inside_the_contract_bound() {
    // Including the degenerate inputs, which must be nudged rather than rejected.
    for bytes in [
        [0u8; 16],
        [0xff; 16],
        [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    ] {
        let salt = RandomSalt::from_entropy(bytes).salt();
        assert!(salt.get() > 1, "salt {} is reserved", salt.get());
        assert!(salt.get() < erebus_sdk::actions::NoteSalt::TWO_POW_120);
    }
}

#[test]
fn wire_v3_exact_and_change_settlements_create_the_same_number_of_notes() {
    let outgoing = Channel::derive(chain(), pool(), &alice(), bob());
    let payer = Counterparty {
        address: alice().address(),
        public_key: alice().public_key(),
    };
    let self_channel = Channel::derive(chain(), pool(), &alice(), payer);
    let mut acceptance = accept_message();
    acceptance.deal_id = 0x1234_5678_9abc_def0;

    let build = |change| {
        outgoing
            .accept_and_settle_with_change(
                token(),
                &inputs(),
                Payment {
                    amount: acceptance.amount,
                    index: 5,
                    salt: salt(),
                },
                Acceptance {
                    message_index: 0,
                    message: acceptance,
                },
                Some(ChangeOutput::existing(
                    self_channel,
                    change,
                    0,
                    change_salt(),
                )),
            )
            .expect("wire-v3 settlement")
    };
    let exact = build(0);
    let surplus = build(2);
    let creation_count = |set: &erebus_sdk::action_set::ActionSet| {
        set.actions()
            .iter()
            .filter(|action| matches!(action, ClientAction::CreateEncNote(_)))
            .count()
    };
    assert_eq!(creation_count(&exact), 7);
    assert_eq!(creation_count(&surplus), 7);
}

#[test]
fn wire_v3_rejects_a_settlement_without_the_constant_change_slot() {
    let channel = Channel::derive(chain(), pool(), &alice(), bob());
    let error = channel
        .accept_and_settle(
            token(),
            &inputs(),
            Payment {
                amount: 950_000,
                index: 5,
                salt: salt(),
            },
            Acceptance {
                message_index: 0,
                message: accept_message(),
            },
        )
        .expect_err("wire v3 requires an explicit zero-change note");
    assert!(matches!(error, ChannelError::MissingV3Change));
}

// --- Rejections -----------------------------------------------------------------

#[test]
fn a_non_acceptance_message_is_rejected() {
    let channel = Channel::derive_with_version(chain(), pool(), &alice(), bob(), WireVersion::V2);
    let mut message = accept_message();
    message.message_type = MessageType::Counter;

    let error = channel
        .accept_and_settle(
            token(),
            &inputs(),
            Payment {
                amount: 1,
                index: 4,
                salt: salt(),
            },
            Acceptance {
                message_index: 2,
                message,
            },
        )
        .expect_err("a counter is not a settlement record");
    assert!(matches!(
        error,
        ChannelError::NotAnAcceptance(MessageType::Counter)
    ));
}

/// Atomicity puts the acceptance and the payment in one proof, so both land or neither
/// does. That says nothing about them *agreeing*. An acceptance recording 950,000 next to a
/// note carrying 1 is atomic but underpays the counterparty. The counterparty still holds a
/// valid on-chain acceptance.
#[test]
fn a_payment_that_disagrees_with_the_acceptance_is_rejected() {
    let channel = Channel::derive_with_version(chain(), pool(), &alice(), bob(), WireVersion::V2);
    let error = channel
        .accept_and_settle(
            token(),
            &inputs(),
            Payment {
                amount: 1,
                index: 4,
                salt: salt(),
            },
            Acceptance {
                message_index: 2,
                message: accept_message(),
            },
        )
        .expect_err("the record says 950000 and the note carries 1");
    assert!(matches!(
        error,
        ChannelError::AmountMismatch {
            agreed: 950_000,
            paid: 1
        }
    ));
}

#[test]
fn a_zero_payment_is_rejected() {
    let channel = Channel::derive_with_version(chain(), pool(), &alice(), bob(), WireVersion::V2);
    let error = channel
        .accept_and_settle(
            token(),
            &inputs(),
            Payment {
                amount: 0,
                index: 4,
                salt: salt(),
            },
            Acceptance {
                message_index: 2,
                message: accept_message(),
            },
        )
        .expect_err("settling nothing is not settling");
    assert!(matches!(error, ChannelError::ZeroPayment));
}

#[test]
fn settling_without_inputs_is_rejected() {
    let channel = Channel::derive_with_version(chain(), pool(), &alice(), bob(), WireVersion::V2);
    let error = channel
        .accept_and_settle(
            token(),
            &[],
            Payment {
                amount: 1,
                index: 4,
                salt: salt(),
            },
            Acceptance {
                message_index: 2,
                message: accept_message(),
            },
        )
        .expect_err("payment must be funded by a spend");
    assert!(matches!(error, ChannelError::NothingToSpend));
}

/// The payment note and the acceptance record share one subchannel index space, so an
/// overlap would silently overwrite part of the record.
#[test]
fn a_payment_index_inside_the_record_range_is_rejected() {
    let channel = Channel::derive_with_version(chain(), pool(), &alice(), bob(), WireVersion::V2);
    // Message 2 occupies 10..14.
    for colliding in 10..15 {
        let error = settle(&channel, colliding, 2)
            .expect_err("payment index {colliding} overlaps the record");
        assert!(
            matches!(error, ChannelError::IndexCollision { .. }),
            "index {colliding} was not caught"
        );
    }
}

#[test]
fn an_index_just_outside_the_record_range_is_allowed() {
    let channel = Channel::derive_with_version(chain(), pool(), &alice(), bob(), WireVersion::V2);
    settle(&channel, 9, 2).expect("index 9 is below the 10..14 record");
    settle(&channel, 15, 2).expect("index 15 is above the 10..14 record");
}
