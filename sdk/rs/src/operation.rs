//! Stable caller-supplied identifiers for durable write operations.
//!
//! The Rust SDK validates operation IDs but does not generate them. Callers must create and
//! durably store an ID with the request intent before crossing the SDK boundary.

use core::str::FromStr;

use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use starknet_types_core::felt::Felt;

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

/// Domain separation for request bindings.
///
/// This tag is Erebus-local and has no Cairo counterpart: a binding is local bookkeeping
/// that never reaches the chain, so it deliberately does not use the Poseidon tags in
/// [`crate::hashes`], which mirror `hashes.cairo` and must stay byte-compatible with it.
const BINDING_DOMAIN: &[u8] = b"EREBUS_OPERATION_BINDING_V1";

/// The chain-writing client methods.
///
/// Every variant here submits a transaction. Read-only methods and `grant_viewing_key`,
/// which produces a capsule from local key material without writing, are absent by design.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteOperation {
    /// Moves public tokens into the pool.
    Shield,
    /// ERC-20 approval that must land before a charged `apply_actions`.
    ApprovePool,
    /// Establishes a bilateral channel.
    OpenChannel,
    /// Writes an offer.
    ProposeOffer,
    /// Writes a counter-offer.
    CounterOffer,
    /// Accepts and settles in one action set.
    AcceptAndSettle,
}

impl WriteOperation {
    /// Stable discriminant mixed into the binding.
    ///
    /// Changing one of these strings changes every binding for that method, which turns
    /// in-flight operations into conflicts rather than silent replays.
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Shield => "shield",
            Self::ApprovePool => "approve_pool",
            Self::OpenChannel => "open_channel",
            Self::ProposeOffer => "propose_offer",
            Self::CounterOffer => "counter_offer",
            Self::AcceptAndSettle => "accept_and_settle",
        }
    }
}

/// A fingerprint of the canonical parameters of one write request.
///
/// Two requests bind equal exactly when they ask for the same effect against the same pool.
/// Reusing an [`OperationId`] across unequal bindings is a caller error and must be rejected
/// before any proving or submission work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestBinding([u8; 32]);

impl RequestBinding {
    /// Starts a binding over the pool-scoped prefix every write shares.
    ///
    /// `chain_id`, `pool_address`, and `token` are included for the same reason
    /// [`crate::disclosure`] binds them into a grant's AAD: the same operation ID against a
    /// repointed configuration names a different effect, and must conflict rather than
    /// resolve as a replay.
    pub fn builder(
        operation: WriteOperation,
        chain_id: Felt,
        pool_address: Felt,
        token: Felt,
    ) -> BindingBuilder {
        let mut hasher = Sha256::new();
        hasher.update(BINDING_DOMAIN);
        hasher.update(chain_id.to_bytes_be());
        hasher.update(pool_address.to_bytes_be());
        hasher.update(token.to_bytes_be());
        let mut builder = BindingBuilder { hasher };
        builder.push_text(operation.tag());
        builder
    }

    /// Lowercase hexadecimal transport form.
    pub fn to_hex(&self) -> String {
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            use core::fmt::Write as _;
            write!(&mut out, "{byte:02x}").expect("writing to String cannot fail");
        }
        out
    }

    /// Raw digest.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl core::fmt::Display for RequestBinding {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl Serialize for RequestBinding {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for RequestBinding {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        let valid = text.len() == 64
            && text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if !valid {
            return Err(D::Error::custom(
                "request binding must be 64 lowercase hexadecimal characters",
            ));
        }
        let mut bytes = [0u8; 32];
        for (index, slot) in bytes.iter_mut().enumerate() {
            *slot = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
                .map_err(|_| D::Error::custom("request binding is not hexadecimal"))?;
        }
        Ok(Self(bytes))
    }
}

/// Accumulates the canonical encoding of one request's parameters.
///
/// Every field is self-delimiting: fixed-width numbers are written big-endian at their full
/// width, and text is length-prefixed. Without that, `("ab", "c")` and `("a", "bc")` would
/// hash alike.
pub struct BindingBuilder {
    hasher: Sha256,
}

impl BindingBuilder {
    fn push_text(&mut self, value: &str) {
        self.hasher.update((value.len() as u64).to_be_bytes());
        self.hasher.update(value.as_bytes());
    }

    /// Adds a field element at its full 32-byte width.
    #[must_use]
    pub fn felt(mut self, value: Felt) -> Self {
        self.hasher.update(value.to_bytes_be());
        self
    }

    /// Adds a 128-bit value at its full 16-byte width.
    #[must_use]
    pub fn u128_be(mut self, value: u128) -> Self {
        self.hasher.update(value.to_be_bytes());
        self
    }

    /// Adds a 64-bit value at its full 8-byte width.
    #[must_use]
    pub fn u64_be(mut self, value: u64) -> Self {
        self.hasher.update(value.to_be_bytes());
        self
    }

    /// Adds a length-prefixed string.
    #[must_use]
    pub fn text(mut self, value: &str) -> Self {
        self.push_text(value);
        self
    }

    /// Finalises the fingerprint.
    #[must_use]
    pub fn finish(self) -> RequestBinding {
        RequestBinding(self.hasher.finalize().into())
    }
}
