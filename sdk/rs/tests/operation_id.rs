//! Focused tests for the caller-supplied durable operation identifier.

use erebus_sdk::operation::{OperationId, OperationIdError};

const VALID: &str = "op_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn valid_id_parses() {
    let id: OperationId = VALID.parse().expect("valid operation ID");

    assert_eq!(id.as_str(), VALID);
}

#[test]
fn incorrect_prefix_or_length_is_rejected() {
    let wrong_prefix = "id_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let too_short = "op_0123456789abcdef";
    let too_long = "op_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0";

    for invalid in [wrong_prefix, too_short, too_long] {
        assert_eq!(invalid.parse::<OperationId>(), Err(OperationIdError));
    }
}

#[test]
fn uppercase_and_non_hexadecimal_characters_are_rejected() {
    let uppercase = "op_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdeF";
    let non_hex = "op_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdeg";

    for invalid in [uppercase, non_hex] {
        assert_eq!(invalid.parse::<OperationId>(), Err(OperationIdError));
    }
}

#[test]
fn serde_round_trip_preserves_the_id_and_validates_input() {
    let id = OperationId::parse(VALID).expect("valid operation ID");
    let json = serde_json::to_string(&id).expect("operation ID serializes");

    assert_eq!(json, format!("\"{VALID}\""));
    assert_eq!(
        serde_json::from_str::<OperationId>(&json).expect("operation ID deserializes"),
        id
    );
    assert!(serde_json::from_str::<OperationId>("\"op_NOT_HEX\"").is_err());
}

#[test]
fn display_is_the_stable_transport_form() {
    let id = OperationId::parse(VALID).expect("valid operation ID");

    assert_eq!(id.to_string(), VALID);
}
