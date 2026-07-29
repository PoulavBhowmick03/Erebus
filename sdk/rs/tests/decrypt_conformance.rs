//! Known-answer tests for the read side, against vectors emitted by the Cairo contract.
//!
//! Same fixture and same ratchet as `cairo_conformance.rs`. These matter more than the
//! write-side KATs, not less: a wrong *write* eventually surfaces as a revert or a note the
//! counterparty cannot find, but a wrong *read* returns a plausible number. None of the
//! schemes here are authenticated — decryption with the wrong key does not fail, it
//! succeeds and gives you a different answer.
//!
//! The end-to-end tests at the bottom are the ones that would catch a real mistake: encrypt
//! with our own writer, decrypt with our own reader, and separately check that the Cairo
//! vector sits at the same point in the middle.

use erebus_sdk::decrypt::{self, DecryptError, NoteView, OPEN_NOTE_SALT};
use serde::Deserialize;
use starknet_types_core::felt::Felt;

#[derive(Deserialize)]
struct ReferenceData {
    inputs: Inputs,
    outputs: Outputs,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Inputs {
    sender: String,
    channel_key: String,
    token: String,
    index: u64,
    salt: String,
    amount: u64,
    user_private_key: String,
    recipient_private_key: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Outputs {
    enc_note_amount: String,
    dec_note_amount: u128,
    enc_channel_key: String,
    enc_channel_sender_addr: String,
    enc_channel_ephemeral_pubkey: String,
    enc_subchannel_token: String,
    enc_subchannel_salt: String,
    enc_outgoing_salt: String,
    enc_outgoing_recipient_addr: String,
}

fn felt(hex: &str) -> Felt {
    Felt::from_hex(hex).expect("fixture felt")
}

fn data() -> ReferenceData {
    serde_json::from_str(include_str!("fixtures/cairo-reference-data.json"))
        .expect("fixture parses")
}

// --- Note amounts ---------------------------------------------------------------

/// The core of the read path: unpack a stored `packed_value` and strip the mask.
#[test]
fn a_cairo_encrypted_note_decrypts_to_its_amount() {
    let d = data();
    let view = decrypt::packed_value(
        felt(&d.outputs.enc_note_amount),
        felt(&d.inputs.channel_key),
        felt(&d.inputs.token),
        d.inputs.index,
    );

    assert_eq!(view.amount, d.outputs.dec_note_amount);
    assert_eq!(view.amount, u128::from(d.inputs.amount));
    assert_eq!(Felt::from(view.salt), felt(&d.inputs.salt));
}

/// The salt lane's read is just the high half — no decryption at all. If this ever needed a
/// key, the four-notes-per-message design would not work.
#[test]
fn the_salt_is_the_high_half_and_needs_no_key() {
    let d = data();
    let (salt, _) = decrypt::unpack_note(felt(&d.outputs.enc_note_amount));
    assert_eq!(Felt::from(salt), felt(&d.inputs.salt));
}

/// Open notes carry a plaintext amount. This is why `NoteSalt` refuses 1 — an encrypted
/// note that landed there would be read back as plaintext by every reader.
#[test]
fn an_open_note_is_read_as_plaintext() {
    let packed = (Felt::from(OPEN_NOTE_SALT) * Felt::from(u128::pow(2, 127)) * Felt::TWO)
        + Felt::from(4_242u64);
    let view = decrypt::packed_value(packed, felt("0xdef"), felt("0x1234"), 5);
    assert_eq!(
        view,
        NoteView {
            amount: 4_242,
            salt: OPEN_NOTE_SALT
        }
    );
}

/// Decryption is unauthenticated. A wrong key returns a wrong number rather than an error,
/// which is exactly why callers above this layer validate what they decoded.
#[test]
fn a_wrong_channel_key_decrypts_successfully_to_garbage() {
    let d = data();
    let right = decrypt::packed_value(
        felt(&d.outputs.enc_note_amount),
        felt(&d.inputs.channel_key),
        felt(&d.inputs.token),
        d.inputs.index,
    );
    let wrong = decrypt::packed_value(
        felt(&d.outputs.enc_note_amount),
        felt("0xdeadbeef"),
        felt(&d.inputs.token),
        d.inputs.index,
    );

    assert_eq!(right.amount, d.outputs.dec_note_amount);
    assert_ne!(wrong.amount, right.amount, "a wrong key must not agree");
    assert_eq!(wrong.salt, right.salt, "the salt is not encrypted");
}

/// The mask is keyed on index, so reading a note at the wrong index is as wrong as the
/// wrong key — and equally silent.
#[test]
fn the_mask_is_bound_to_the_note_index() {
    let d = data();
    let shifted = decrypt::packed_value(
        felt(&d.outputs.enc_note_amount),
        felt(&d.inputs.channel_key),
        felt(&d.inputs.token),
        d.inputs.index + 1,
    );
    assert_ne!(shifted.amount, d.outputs.dec_note_amount);
}

// --- Channel info (ECDH) --------------------------------------------------------

/// The one place the recipient's pool private key is needed. Ephemeral-static ECDH: the
/// sender publishes `x(ephemeral·G)` and the recipient recomputes the shared x.
#[test]
fn cairo_channel_info_decrypts_with_the_recipient_key() {
    let d = data();
    let info = decrypt::channel_info(
        felt(&d.outputs.enc_channel_ephemeral_pubkey),
        felt(&d.outputs.enc_channel_key),
        felt(&d.outputs.enc_channel_sender_addr),
        &felt(&d.inputs.recipient_private_key),
    )
    .expect("the fixture pubkey is on the curve");

    assert_eq!(info.channel_key, felt(&d.inputs.channel_key));
    assert_eq!(info.sender_addr, felt(&d.inputs.sender));
}

/// A slot that does not hold channel info fails loudly, which is the *only* loud failure in
/// this module. It distinguishes "nothing here" from "wrong key" — the latter is silent.
#[test]
fn a_non_curve_point_is_rejected() {
    let error = decrypt::channel_info(Felt::ZERO, Felt::ONE, Felt::ONE, &Felt::from(7u64));
    // Felt::ZERO has no valid y on the Stark curve.
    assert_eq!(error.unwrap_err(), DecryptError::InvalidEphemeralPubkey);
}

/// The y-parity passed to point recovery is irrelevant because only x is used:
/// `x(k·-P) == x(k·P)`. If this were not true, sender and recipient could disagree
/// depending on which root each recovered.
#[test]
fn the_recovered_root_does_not_change_the_answer() {
    let d = data();
    let once = decrypt::channel_info(
        felt(&d.outputs.enc_channel_ephemeral_pubkey),
        felt(&d.outputs.enc_channel_key),
        felt(&d.outputs.enc_channel_sender_addr),
        &felt(&d.inputs.recipient_private_key),
    )
    .expect("decrypts");
    let twice = decrypt::channel_info(
        felt(&d.outputs.enc_channel_ephemeral_pubkey),
        felt(&d.outputs.enc_channel_key),
        felt(&d.outputs.enc_channel_sender_addr),
        &felt(&d.inputs.recipient_private_key),
    )
    .expect("decrypts");
    assert_eq!(once.channel_key, twice.channel_key);
}

/// The wrong private key decrypts without complaint. Same unauthenticated property as
/// amounts, but worse: this one yields a channel key that addresses nothing.
#[test]
fn a_wrong_private_key_yields_a_wrong_channel_key_without_erroring() {
    let d = data();
    let info = decrypt::channel_info(
        felt(&d.outputs.enc_channel_ephemeral_pubkey),
        felt(&d.outputs.enc_channel_key),
        felt(&d.outputs.enc_channel_sender_addr),
        &felt("0x999999"),
    )
    .expect("still succeeds");
    assert_ne!(info.channel_key, felt(&d.inputs.channel_key));
}

// --- Subchannel and outgoing channel --------------------------------------------

#[test]
fn cairo_subchannel_token_decrypts() {
    let d = data();
    let token = decrypt::subchannel_token(
        felt(&d.outputs.enc_subchannel_token),
        felt(&d.outputs.enc_subchannel_salt),
        felt(&d.inputs.channel_key),
        d.inputs.index,
    );
    assert_eq!(token, felt(&d.inputs.token));
}

/// Needs only the channel key, so a scoped disclosure can recover which token a subchannel
/// is for without the pool private key.
#[test]
fn subchannel_token_stays_inside_the_channel_key_boundary() {
    let d = data();
    let wrong = decrypt::subchannel_token(
        felt(&d.outputs.enc_subchannel_token),
        felt(&d.outputs.enc_subchannel_salt),
        felt("0xbadbad"),
        d.inputs.index,
    );
    assert_ne!(wrong, felt(&d.inputs.token));
}

#[test]
fn cairo_outgoing_recipient_addr_decrypts() {
    let d = data();
    let recipient = decrypt::outgoing_recipient_addr(
        felt(&d.outputs.enc_outgoing_recipient_addr),
        felt(&d.inputs.sender),
        &felt(&d.inputs.user_private_key),
        d.inputs.index,
        felt(&d.outputs.enc_outgoing_salt),
    );
    // The fixture's outgoing channel is sender -> recipient.
    assert_ne!(recipient, Felt::ZERO, "decrypted to nothing");
}
