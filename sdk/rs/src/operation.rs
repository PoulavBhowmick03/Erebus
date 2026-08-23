//! Stable caller-supplied identifiers for durable write operations.
//!
//! The Rust SDK validates operation IDs but does not generate them. Callers must create and
//! durably store an ID with the request intent before crossing the SDK boundary.

use core::str::FromStr;

use serde::{de::Error as _, Deserialize, Deserializer, Serialize};

const PREFIX: &str = "op_";
const HEX_LENGTH: usize = 64;
const ENCODED_LENGTH: usize = PREFIX.len() + HEX_LENGTH;

/// A caller-supplied identifier for one durable write operation.
///
/// Its transport form is `op_` followed by 64 lowercase hexadecimal characters.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct OperationId(String);

impl OperationId {
    /// Parses and validates an operation ID.
    pub fn parse(value: impl Into<String>) -> Result<Self, OperationIdError> {
        let value = value.into();
        let valid = value.len() == ENCODED_LENGTH
            && value.starts_with(PREFIX)
            && value[PREFIX.len()..]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));

        if !valid {
            return Err(OperationIdError);
        }

        Ok(Self(value))
    }

    /// Returns the stable transport representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for OperationId {
    type Err = OperationIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl core::fmt::Display for OperationId {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for OperationId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(D::Error::custom)
    }
}

/// An operation ID did not match the stable transport format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("operation ID must be `op_` followed by 64 lowercase hexadecimal characters")]
pub struct OperationIdError;
