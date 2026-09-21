//! Protocol identifiers that are valid by construction.
//!
//! Every type here enforces its own length and character rules when it is created, so a value
//! that exists cannot encode outside its bounds. Decoders feed the same validators, which is
//! why a canonical encoding never has to defend against an over-long field twice.
//!
//! Chain namespaces follow CAIP-2 (`family:reference`) and asset identifiers follow CAIP-19
//! (`namespace:reference/asset_namespace:asset_reference`). The core treats both as opaque
//! data: it checks syntax but never branches on a family name.

use core::fmt;

/// Longest key or key reference accepted in an agreement.
pub const MAX_KEY_BYTES: usize = 64;
/// Longest chain address accepted in a deployment domain.
pub const MAX_ADDRESS_BYTES: usize = 64;
/// Longest authorization signature accepted before suite validation.
pub const MAX_SIGNATURE_BYTES: usize = 128;

/// An identifier or bounded byte value was rejected at construction.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdError {
    /// Key bytes were empty or too long.
    #[error("key material must be 1 to 64 bytes, found {0}")]
    InvalidKeyLength(usize),
    /// Address bytes were empty or too long.
    #[error("address must be 1 to 64 bytes, found {0}")]
    InvalidAddressLength(usize),
    /// Signature bytes were empty or too long.
    #[error("signature must be 1 to 128 bytes, found {0}")]
    InvalidSignatureLength(usize),
    /// A chain namespace did not parse as `family:reference`.
    #[error("chain namespace `{0}` is invalid: {1}")]
    InvalidChainNamespace(String, &'static str),
    /// An asset identifier did not parse as CAIP-19.
    #[error("asset identifier `{0}` is invalid: {1}")]
    InvalidAssetId(String, &'static str),
}

/// A non-negative amount in an asset's base units.
///
/// An agreement requires a non-zero amount; that rule lives in
/// [`crate::terms::AgreementTerms::validate`] rather than here, because a fee of zero is
/// valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct BaseUnits(u128);

impl BaseUnits {
    /// Wraps a raw base-unit count.
    #[must_use]
    pub const fn new(value: u128) -> Self {
        Self(value)
    }

    /// Returns the raw count.
    #[must_use]
    pub const fn get(self) -> u128 {
        self.0
    }

    /// Reports whether the amount is zero.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }
}

/// Opaque key material bound into an agreement.
///
/// The agreement suite interprets these bytes; the core only enforces the shared length bound.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyBytes(Vec<u8>);

impl KeyBytes {
    /// Wraps key bytes after checking the shared bound.
    pub fn new(bytes: Vec<u8>) -> Result<Self, IdError> {
        if bytes.is_empty() || bytes.len() > MAX_KEY_BYTES {
            return Err(IdError::InvalidKeyLength(bytes.len()));
        }
        Ok(Self(bytes))
    }

    /// Wraps bytes a decoder has already bounded.
    pub(crate) fn from_validated(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// Returns the raw bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// A chain address bound into a deployment domain.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AddressBytes(Vec<u8>);

impl AddressBytes {
    /// Wraps address bytes after checking the shared bound.
    pub fn new(bytes: Vec<u8>) -> Result<Self, IdError> {
        if bytes.is_empty() || bytes.len() > MAX_ADDRESS_BYTES {
            return Err(IdError::InvalidAddressLength(bytes.len()));
        }
        Ok(Self(bytes))
    }

    /// Wraps bytes a decoder has already bounded.
    pub(crate) fn from_validated(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// Returns the raw bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// An authorization signature bound to a role and commitment.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SignatureBytes(Vec<u8>);

impl SignatureBytes {
    /// Wraps signature bytes after checking the shared bound.
    pub fn new(bytes: Vec<u8>) -> Result<Self, IdError> {
        if bytes.is_empty() || bytes.len() > MAX_SIGNATURE_BYTES {
            return Err(IdError::InvalidSignatureLength(bytes.len()));
        }
        Ok(Self(bytes))
    }

    /// Wraps bytes a decoder has already bounded.
    pub(crate) fn from_validated(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// Returns the raw bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// A CAIP-2 chain namespace, for example `eip155:10143`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChainNamespace {
    family: String,
    reference: String,
}

impl ChainNamespace {
    /// Builds a namespace from its two parts and validates both.
    pub fn new(family: &str, reference: &str) -> Result<Self, IdError> {
        validate_namespace_parts(family, reference).map_err(|reason| {
            IdError::InvalidChainNamespace(format!("{family}:{reference}"), reason)
        })?;
        Ok(Self {
            family: family.to_owned(),
            reference: reference.to_owned(),
        })
    }

    /// Parses `family:reference`.
    pub fn parse(value: &str) -> Result<Self, IdError> {
        let Some((family, reference)) = value.split_once(':') else {
            return Err(IdError::InvalidChainNamespace(
                value.to_owned(),
                "expected `family:reference`",
            ));
        };
        Self::new(family, reference)
    }

    /// Returns the namespace family, for example `eip155`.
    #[must_use]
    pub fn family(&self) -> &str {
        &self.family
    }

    /// Returns the namespace reference, for example a chain id.
    #[must_use]
    pub fn reference(&self) -> &str {
        &self.reference
    }
}

impl fmt::Display for ChainNamespace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.family, self.reference)
    }
}

fn validate_namespace_parts(family: &str, reference: &str) -> Result<(), &'static str> {
    if !(3..=8).contains(&family.len()) {
        return Err("family must be 3 to 8 characters");
    }
    if !family
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err("family must be lowercase ASCII letters, digits, or `-`");
    }
    if reference.is_empty() || reference.len() > 32 {
        return Err("reference must be 1 to 32 characters");
    }
    if !reference
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("reference must be ASCII letters, digits, `-`, or `_`");
    }
    Ok(())
}

/// A CAIP-19 asset identifier, for example `eip155:10143/erc20:0xabc`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetId {
    namespace: ChainNamespace,
    asset_namespace: String,
    asset_reference: String,
}

impl AssetId {
    /// Builds an asset identifier from its three parts and validates the asset parts.
    pub fn new(
        namespace: ChainNamespace,
        asset_namespace: &str,
        asset_reference: &str,
    ) -> Result<Self, IdError> {
        validate_asset_parts(asset_namespace, asset_reference).map_err(|reason| {
            IdError::InvalidAssetId(
                format!("{namespace}/{asset_namespace}:{asset_reference}"),
                reason,
            )
        })?;
        Ok(Self {
            namespace,
            asset_namespace: asset_namespace.to_owned(),
            asset_reference: asset_reference.to_owned(),
        })
    }

    /// Parses `namespace:reference/asset_namespace:asset_reference`.
    pub fn parse(value: &str) -> Result<Self, IdError> {
        let Some((chain, asset)) = value.split_once('/') else {
            return Err(IdError::InvalidAssetId(
                value.to_owned(),
                "expected `namespace:reference/asset_namespace:asset_reference`",
            ));
        };
        let Some((asset_namespace, asset_reference)) = asset.split_once(':') else {
            return Err(IdError::InvalidAssetId(
                value.to_owned(),
                "expected `namespace:reference/asset_namespace:asset_reference`",
            ));
        };
        let namespace = ChainNamespace::parse(chain)?;
        Self::new(namespace, asset_namespace, asset_reference)
    }

    /// Returns the chain namespace the asset lives on.
    #[must_use]
    pub fn namespace(&self) -> &ChainNamespace {
        &self.namespace
    }

    /// Returns the CAIP-19 asset namespace, for example `erc20`.
    #[must_use]
    pub fn asset_namespace(&self) -> &str {
        &self.asset_namespace
    }

    /// Returns the asset reference, for example a token address.
    #[must_use]
    pub fn asset_reference(&self) -> &str {
        &self.asset_reference
    }
}

impl fmt::Display for AssetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}/{}:{}",
            self.namespace.family(),
            self.namespace.reference(),
            self.asset_namespace,
            self.asset_reference
        )
    }
}

fn validate_asset_parts(asset_namespace: &str, asset_reference: &str) -> Result<(), &'static str> {
    if !(3..=8).contains(&asset_namespace.len()) {
        return Err("asset namespace must be 3 to 8 characters");
    }
    if !asset_namespace
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err("asset namespace must be lowercase ASCII letters, digits, or `-`");
    }
    if asset_reference.is_empty() || asset_reference.len() > 128 {
        return Err("asset reference must be 1 to 128 characters");
    }
    if !asset_reference
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'%'))
    {
        return Err("asset reference must be ASCII letters, digits, `.`, `-`, or `%`");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caip_bounds_match_the_identifier_standards() {
        assert!(ChainNamespace::new("ab", "1").is_err());
        assert!(ChainNamespace::new("abcdefghi", "1").is_err());
        assert!(ChainNamespace::new("-ab", "1").is_ok());
        assert!(ChainNamespace::new("eip155", "1.0").is_err());
        assert!(ChainNamespace::new("chainstd", &"a".repeat(32)).is_ok());
        assert!(ChainNamespace::new("chainstd", &"a".repeat(33)).is_err());
        let chain = ChainNamespace::parse("eip155:1").unwrap();
        assert!(AssetId::new(chain.clone(), "ab", "x").is_err());
        assert!(AssetId::new(chain.clone(), "abcdefghi", "x").is_err());
        assert!(AssetId::new(chain.clone(), "erc20", &"a".repeat(128)).is_ok());
        assert!(AssetId::new(chain.clone(), "erc20", &"a".repeat(129)).is_err());
        assert!(AssetId::new(chain, "erc20", "a_b").is_err());
    }

    #[test]
    fn byte_bounds_are_enforced() {
        assert_eq!(KeyBytes::new(Vec::new()), Err(IdError::InvalidKeyLength(0)));
        assert!(KeyBytes::new(vec![0u8; MAX_KEY_BYTES]).is_ok());
        assert_eq!(
            KeyBytes::new(vec![0u8; MAX_KEY_BYTES + 1]),
            Err(IdError::InvalidKeyLength(MAX_KEY_BYTES + 1))
        );
        assert!(AddressBytes::new(vec![1u8; 20]).is_ok());
        assert!(SignatureBytes::new(vec![2u8; 65]).is_ok());
    }

    #[test]
    fn namespaces_round_trip() {
        let namespace = ChainNamespace::new("eip155", "10143").expect("valid namespace");
        assert_eq!(namespace.family(), "eip155");
        assert_eq!(namespace.reference(), "10143");
        assert_eq!(namespace.to_string(), "eip155:10143");
        assert_eq!(ChainNamespace::parse("eip155:10143"), Ok(namespace));
    }

    #[test]
    fn invalid_namespaces_are_rejected() {
        for value in [
            "",
            "eip155",
            ":10143",
            "EIP155:10143",
            "eip_155:10143",
            "eip155:",
            "eip155:1 0",
            "starknet:SN_SEPOLIA:extra",
        ] {
            assert!(ChainNamespace::parse(value).is_err(), "accepted `{value}`");
        }
    }

    #[test]
    fn assets_round_trip() {
        let namespace = ChainNamespace::new("eip155", "10143").expect("valid namespace");
        let asset = AssetId::new(namespace, "erc20", "0xabc123").expect("valid asset");
        assert_eq!(asset.to_string(), "eip155:10143/erc20:0xabc123");
        assert_eq!(AssetId::parse("eip155:10143/erc20:0xabc123"), Ok(asset));
    }

    #[test]
    fn invalid_assets_are_rejected() {
        for value in [
            "eip155:10143",
            "eip155:10143/",
            "eip155:10143/erc20",
            "eip155:10143/ERC20:0xabc",
            "eip155:10143/erc20:",
            "eip155:10143/erc20:0xab c",
        ] {
            assert!(AssetId::parse(value).is_err(), "accepted `{value}`");
        }
    }

    #[test]
    fn zero_amount_is_representable_but_flagged() {
        assert!(BaseUnits::new(0).is_zero());
        assert!(!BaseUnits::new(1).is_zero());
    }
}
