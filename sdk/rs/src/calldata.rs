//! Calldata construction for the two calls in the privacy pipeline.
//!
//! The client passes actions to the pool view `compile_actions`. The prover receives the
//! same calldata in the pool account's `__execute__` payload. It returns serialized server
//! actions for an account call to `apply_actions`.
//!
//! Keeping the layouts together lets tests cover serialization from
//! [`crate::action_set::ActionSet`] instead of starting with fixture-built calldata.

use sha3::{Digest, Keccak256};
use starknet_types_core::felt::Felt;

use crate::action_set::ActionSet;
use crate::prover::AdditionalData;

/// Starknet's selector for an entrypoint name: Keccak-256 with the top six bits cleared.
pub fn selector(name: &str) -> Felt {
    let mut digest: [u8; 32] = Keccak256::digest(name.as_bytes()).into();
    digest[0] &= 0x03;
    Felt::from_bytes_be(&digest)
}

/// `compile_actions(user_addr, user_private_key, client_actions)`.
pub fn compile_actions(
    user_address: Felt,
    pool_private_key: Felt,
    actions: &ActionSet,
) -> Vec<Felt> {
    let serialized = actions.serialize();
    let mut calldata = Vec::with_capacity(2 + serialized.len());
    calldata.push(user_address);
    calldata.push(pool_private_key);
    calldata.extend(serialized);
    calldata
}

/// Cairo-1 `Array<Call>` encoding for exactly one call.
pub fn single_call(target: Felt, entrypoint: &str, calldata: &[Felt]) -> Vec<Felt> {
    let mut execute = Vec::with_capacity(4 + calldata.len());
    execute.push(Felt::ONE);
    execute.push(target);
    execute.push(selector(entrypoint));
    execute.push(Felt::from(calldata.len()));
    execute.extend_from_slice(calldata);
    execute
}

/// Pool-account `__execute__` calldata for a call to `compile_actions`.
pub fn proof_execute(pool_address: Felt, compile_calldata: &[Felt]) -> Vec<Felt> {
    single_call(pool_address, "compile_actions", compile_calldata)
}

/// Serialized `Option<ScreeningAttestation>` appended to `apply_actions`.
pub fn screening_suffix(
    additional_data: Option<&AdditionalData>,
) -> Result<Vec<Felt>, CalldataError> {
    let Some(signature) = additional_data.and_then(|data| data.signature.as_ref()) else {
        // Cairo Serde encodes Option::None as variant index 1.
        return Ok(vec![Felt::ONE]);
    };

    Ok(vec![
        Felt::ZERO,
        Felt::from(signature.issued_at),
        parse_felt("screening signature r", &signature.sig_r)?,
        parse_felt("screening signature s", &signature.sig_s)?,
    ])
}

/// Full `apply_actions(actions, screening)` calldata.
pub fn apply_actions(
    server_actions: &[Felt],
    additional_data: Option<&AdditionalData>,
) -> Result<Vec<Felt>, CalldataError> {
    let suffix = screening_suffix(additional_data)?;
    let mut calldata = Vec::with_capacity(server_actions.len() + suffix.len());
    calldata.extend_from_slice(server_actions);
    calldata.extend(suffix);
    Ok(calldata)
}

fn parse_felt(field: &'static str, value: &str) -> Result<Felt, CalldataError> {
    Felt::from_hex(value).map_err(|_| CalldataError::InvalidFelt {
        field,
        value: value.to_owned(),
    })
}

/// Malformed prover data while constructing calldata.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CalldataError {
    /// A prover-returned felt was not canonical.
    #[error("{field} is not a felt: {value}")]
    InvalidFelt {
        /// Which value was malformed.
        field: &'static str,
        /// The received value.
        value: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action_set::ActionSetBuilder;
    use crate::actions::{ClientAction, OpenChannelInput};

    fn felt(value: &str) -> Felt {
        Felt::from_hex(value).expect("felt")
    }

    #[test]
    fn selector_matches_starknetjs() {
        assert_eq!(
            selector("compile_actions"),
            felt("0x360f8727b971d0bc6b93fc840d637c077f8ae59eb6ca8ce27fdb5422b688192")
        );
        assert_eq!(
            selector("apply_actions"),
            felt("0x246333a752c1ac637ff1591c5c885e27d56060d241a29aad8475072da0777db")
        );
    }

    #[test]
    fn action_set_is_part_of_the_proof_calldata_composition() {
        let action = ClientAction::OpenChannel(OpenChannelInput {
            recipient_addr: Felt::from(7u8),
            index: 3,
            random: Felt::from(11u8),
            salt: Felt::from(13u8),
        });
        let mut builder = ActionSetBuilder::new();
        builder.push(action).expect("valid action");
        let set = builder.build().expect("replay protected");

        let inner = compile_actions(Felt::from(2u8), Felt::from(5u8), &set);
        assert_eq!(inner[0], Felt::from(2u8));
        assert_eq!(inner[1], Felt::from(5u8));
        assert_eq!(inner[2], Felt::ONE, "ActionSet span length");

        let outer = proof_execute(Felt::from(17u8), &inner);
        assert_eq!(outer[0], Felt::ONE);
        assert_eq!(outer[1], Felt::from(17u8));
        assert_eq!(outer[2], selector("compile_actions"));
        assert_eq!(outer[3], Felt::from(inner.len()));
        assert_eq!(&outer[4..], inner);
    }

    #[test]
    fn screening_option_matches_cairo_serde() {
        assert_eq!(screening_suffix(None).expect("none"), vec![Felt::ONE]);
        let data = AdditionalData {
            signature: Some(crate::prover::ScreeningSignature {
                issued_at: 42,
                sig_r: "0x2".to_owned(),
                sig_s: "0x3".to_owned(),
            }),
        };
        assert_eq!(
            screening_suffix(Some(&data)).expect("some"),
            vec![Felt::ZERO, Felt::from(42u8), Felt::TWO, Felt::THREE]
        );
    }
}
