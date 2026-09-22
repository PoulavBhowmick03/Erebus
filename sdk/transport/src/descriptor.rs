//! Signed service publication and discovery (decision DM2-6).
//!
//! A seller publishes a [`ServiceDescriptor`]: its suite-1 authorization address, the X25519
//! transport key that address stands behind, endpoints, the chain and assets it serves, and the
//! settlement capabilities it advertises. The descriptor is signed by the authorization address,
//! which is what binds the transport key to the identity that will later authorize settlement.
//!
//! The type has no fields for a negotiation transcript, a reservation price, or any deal-specific
//! data, so those cannot leak into a public record: their absence is structural, not a filtering
//! rule (roadmap M2 item 9).
//!
//! Discovery is a configured directory of descriptors. A client verifies each signature and
//! expiry, then filters by chain, asset, mode, and required guarantees. No chain transaction is
//! involved (architecture section 2).

use erebus_core::encoding::{EncodingError, Reader, Writer};
use erebus_core::ids::{AssetId, ChainNamespace, IdError};
use erebus_core::suite::{self, SuiteError};
use erebus_core::terms::{Guarantee, GuaranteeSet, SettlementMode};
use serde::{Deserialize, Serialize};

use crate::identity::{
    AuthorizationIdentity, AUTHORIZATION_KEY_BYTES, AUTHORIZATION_SIGNATURE_BYTES,
    TRANSPORT_KEY_BYTES,
};
use crate::limits::{MAX_DESCRIPTOR_ASSETS, MAX_DESCRIPTOR_ENDPOINTS, MAX_DESCRIPTOR_SUITES};

/// Domain separation for a service descriptor digest.
pub const DESCRIPTOR_DOMAIN: &[u8] = b"EREBUS_SERVICE_DESCRIPTOR_V1";
/// Descriptor protocol version.
pub const DESCRIPTOR_PROTOCOL_VERSION: u16 = 1;
/// The suite that signs descriptors at M2.
pub const DESCRIPTOR_SUITE_ID: u16 = 1;
/// Settlement-mode bit for public-bound settlement.
pub const MODE_PUBLIC_BOUND: u8 = 0b01;
/// Settlement-mode bit for shielded settlement.
pub const MODE_SHIELDED: u8 = 0b10;

/// A descriptor could not be built, decoded, or verified.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DescriptorError {
    /// The protocol version is not supported.
    #[error("descriptor protocol version {0} is not supported")]
    UnsupportedProtocolVersion(u16),
    /// The signature suite is not implemented.
    #[error(transparent)]
    Suite(#[from] SuiteError),
    /// A canonical field could not be read.
    #[error(transparent)]
    Encoding(#[from] EncodingError),
    /// An identifier was invalid.
    #[error(transparent)]
    Id(#[from] IdError),
    /// A descriptor must advertise at least one endpoint.
    #[error("descriptor must advertise 1 to {max} endpoints")]
    EndpointCount {
        /// The bound.
        max: usize,
    },
    /// An endpoint was empty or too long.
    #[error("endpoint must be 1 to 256 bytes")]
    InvalidEndpoint,
    /// Too many assets were listed.
    #[error("descriptor must advertise 1 to {max} assets")]
    AssetCount {
        /// The bound.
        max: usize,
    },
    /// An advertised asset belongs to a different chain namespace.
    #[error("descriptor asset chain does not match its chain namespace")]
    AssetChainMismatch,
    /// Too many suites were listed.
    #[error("descriptor must advertise 1 to {max} suites")]
    SuiteCount {
        /// The bound.
        max: usize,
    },
    /// A suite listed by the descriptor is not implemented.
    #[error("descriptor advertises unimplemented suite {0}")]
    UnsupportedSuite(u16),
    /// A guarantee set named no known guarantee.
    #[error(transparent)]
    Terms(#[from] erebus_core::terms::TermsError),
    /// Mode bits named no known settlement mode.
    #[error("descriptor advertises unknown settlement-mode bits 0x{0:02x}")]
    UnknownModeBits(u8),
    /// A descriptor must advertise at least one settlement mode.
    #[error("descriptor must advertise at least one settlement mode")]
    MissingSettlementMode,
    /// Public settlement cannot provide payment-field privacy.
    #[error("public-bound settlement cannot advertise hidden amount or recipient")]
    ContradictoryPrivacy,
    /// Expiry must be later than issuance.
    #[error("descriptor expiry must be later than its issuance time")]
    InvalidValidityWindow,
    /// The descriptor is not active yet.
    #[error("descriptor is not valid until {issued_at}; current time is {now}")]
    NotYetValid {
        /// Descriptor issuance time.
        issued_at: u64,
        /// Verification time.
        now: u64,
    },
    /// The descriptor has expired.
    #[error("descriptor expired at {expires}; current time is {now}")]
    Expired {
        /// The descriptor expiry.
        expires: u64,
        /// The verification time.
        now: u64,
    },
    /// The signature did not verify under the stated seller address.
    #[error("descriptor signature did not verify under the advertised seller address")]
    BadSignature,
    /// The signing identity's address is not the descriptor's seller address.
    #[error("signing identity does not match the descriptor's seller address")]
    SignerMismatch,
    /// The descriptor has no signature to verify.
    #[error("descriptor has no signature")]
    Unsigned,
    /// The JSON document could not be read or written.
    #[error("descriptor JSON error: {0}")]
    Json(String),
}

/// A seller's signed, portable service advertisement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceDescriptor {
    /// Descriptor protocol version.
    pub protocol_version: u16,
    /// The seller's suite-1 authorization address.
    pub seller_address: [u8; AUTHORIZATION_KEY_BYTES],
    /// The seller's X25519 transport public key.
    pub transport_key: [u8; TRANSPORT_KEY_BYTES],
    /// Reachable endpoints, most preferred first.
    pub endpoints: Vec<String>,
    /// Chain the seller serves, as a CAIP-2 namespace.
    pub chain_namespace: ChainNamespace,
    /// Assets the seller accepts, as CAIP-19 identifiers.
    pub assets: Vec<AssetId>,
    /// Agreement suites the seller supports.
    pub suites: Vec<u16>,
    /// Settlement modes the seller advertises, as [`MODE_PUBLIC_BOUND`] / [`MODE_SHIELDED`] bits.
    pub settlement_modes: u8,
    /// Guarantees the seller advertises.
    pub guarantees: GuaranteeSet,
    /// Issuance time, Unix seconds.
    pub issued_at: u64,
    /// Expiry time, Unix seconds.
    pub expires: u64,
    /// Suite-1 signature over the unsigned encoding, `r || s || v`.
    pub signature: [u8; AUTHORIZATION_SIGNATURE_BYTES],
}

impl ServiceDescriptor {
    /// Builds an unsigned descriptor and checks its structural bounds.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        seller_address: [u8; AUTHORIZATION_KEY_BYTES],
        transport_key: [u8; TRANSPORT_KEY_BYTES],
        endpoints: Vec<String>,
        chain_namespace: ChainNamespace,
        assets: Vec<AssetId>,
        suites: Vec<u16>,
        settlement_modes: u8,
        guarantees: GuaranteeSet,
        issued_at: u64,
        expires: u64,
    ) -> Result<Self, DescriptorError> {
        let descriptor = Self {
            protocol_version: DESCRIPTOR_PROTOCOL_VERSION,
            seller_address,
            transport_key,
            endpoints,
            chain_namespace,
            assets,
            suites,
            settlement_modes,
            guarantees,
            issued_at,
            expires,
            signature: [0u8; AUTHORIZATION_SIGNATURE_BYTES],
        };
        descriptor.validate_structure()?;
        Ok(descriptor)
    }

    /// Signs the descriptor with the seller's authorization identity.
    ///
    /// Fails if the identity's address is not the descriptor's seller address, so a descriptor
    /// can never claim an identity it cannot sign for.
    pub fn sign(&mut self, identity: &AuthorizationIdentity) -> Result<(), DescriptorError> {
        if identity.address() != self.seller_address {
            return Err(DescriptorError::SignerMismatch);
        }
        self.signature = identity.sign_digest(&self.digest()?);
        Ok(())
    }

    /// Checks structure, expiry, suite support, and the signature.
    pub fn verify(&self, now: u64) -> Result<(), DescriptorError> {
        self.validate_structure()?;
        if now < self.issued_at {
            return Err(DescriptorError::NotYetValid {
                issued_at: self.issued_at,
                now,
            });
        }
        if now >= self.expires {
            return Err(DescriptorError::Expired {
                expires: self.expires,
                now,
            });
        }
        let suite = suite::suite(DESCRIPTOR_SUITE_ID)?;
        suite
            .verify_authorization(&self.seller_address, &self.digest()?, &self.signature)
            .map_err(|_| DescriptorError::BadSignature)?;
        Ok(())
    }

    /// Verifies this descriptor at `now`, then checks a buyer's requirements.
    pub fn supports(&self, filter: &DiscoveryFilter, now: u64) -> Result<bool, DescriptorError> {
        self.verify(now)?;
        Ok(self.matches_filter(filter))
    }

    fn matches_filter(&self, filter: &DiscoveryFilter) -> bool {
        if self.chain_namespace != filter.chain_namespace {
            return false;
        }
        if !self.assets.contains(&filter.asset) {
            return false;
        }
        if self.settlement_modes & mode_bit(filter.mode) == 0 {
            return false;
        }
        filter
            .required_guarantees
            .iter()
            .all(|guarantee| self.guarantees.contains(guarantee))
    }

    /// The digest the seller signs.
    pub fn digest(&self) -> Result<[u8; 32], DescriptorError> {
        let suite = suite::suite(DESCRIPTOR_SUITE_ID)?;
        Ok(suite.hash(&[DESCRIPTOR_DOMAIN, &self.encode_unsigned()]))
    }

    /// Canonical encoding of every field except the signature.
    #[must_use]
    pub fn encode_unsigned(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.u16(self.protocol_version);
        writer.fixed(&self.seller_address);
        writer.fixed(&self.transport_key);
        writer.u8(self.endpoints.len() as u8);
        for endpoint in &self.endpoints {
            writer.text(endpoint);
        }
        writer.text(&self.chain_namespace.to_string());
        writer.u8(self.assets.len() as u8);
        for asset in &self.assets {
            writer.text(&asset.to_string());
        }
        writer.u8(self.suites.len() as u8);
        for suite_id in &self.suites {
            writer.u16(*suite_id);
        }
        writer.u8(self.settlement_modes);
        writer.u32(self.guarantees.bits());
        writer.u64(self.issued_at);
        writer.u64(self.expires);
        writer.finish()
    }

    /// Canonical encoding of the complete signed descriptor.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.fixed(&self.encode_unsigned());
        writer.bytes(&self.signature);
        writer.finish()
    }

    /// Decodes a complete signed descriptor.
    pub fn decode(bytes: &[u8]) -> Result<Self, DescriptorError> {
        let mut reader = Reader::new(bytes);
        let descriptor = Self::decode_unsigned(&mut reader)?;
        let signature: [u8; AUTHORIZATION_SIGNATURE_BYTES] = reader
            .bytes(
                "signature",
                AUTHORIZATION_SIGNATURE_BYTES,
                AUTHORIZATION_SIGNATURE_BYTES,
            )?
            .try_into()
            .map_err(|_| {
                DescriptorError::Encoding(EncodingError::LengthOutOfRange(
                    "signature",
                    AUTHORIZATION_SIGNATURE_BYTES,
                ))
            })?;
        reader.finish()?;
        Ok(Self {
            signature,
            ..descriptor
        })
    }

    fn decode_unsigned(reader: &mut Reader<'_>) -> Result<Self, DescriptorError> {
        let protocol_version = reader.u16("protocol_version")?;
        if protocol_version != DESCRIPTOR_PROTOCOL_VERSION {
            return Err(DescriptorError::UnsupportedProtocolVersion(
                protocol_version,
            ));
        }
        let seller_address = reader.fixed::<AUTHORIZATION_KEY_BYTES>("seller_address")?;
        let transport_key = reader.fixed::<TRANSPORT_KEY_BYTES>("transport_key")?;
        let endpoint_count = usize::from(reader.u8("endpoint_count")?);
        if endpoint_count == 0 || endpoint_count > MAX_DESCRIPTOR_ENDPOINTS {
            return Err(DescriptorError::EndpointCount {
                max: MAX_DESCRIPTOR_ENDPOINTS,
            });
        }
        let mut endpoints = Vec::with_capacity(endpoint_count);
        for _ in 0..endpoint_count {
            endpoints.push(reader.text("endpoint", 1, 256)?.to_owned());
        }
        let chain_namespace = ChainNamespace::parse(reader.text("chain_namespace", 5, 41)?)?;
        let asset_count = usize::from(reader.u8("asset_count")?);
        if asset_count == 0 || asset_count > MAX_DESCRIPTOR_ASSETS {
            return Err(DescriptorError::AssetCount {
                max: MAX_DESCRIPTOR_ASSETS,
            });
        }
        let mut assets = Vec::with_capacity(asset_count);
        for _ in 0..asset_count {
            assets.push(AssetId::parse(reader.text("asset", 1, 256)?)?);
        }
        let suite_count = usize::from(reader.u8("suite_count")?);
        if suite_count == 0 || suite_count > MAX_DESCRIPTOR_SUITES {
            return Err(DescriptorError::SuiteCount {
                max: MAX_DESCRIPTOR_SUITES,
            });
        }
        let mut suites = Vec::with_capacity(suite_count);
        for _ in 0..suite_count {
            suites.push(reader.u16("suite")?);
        }
        let settlement_modes = reader.u8("settlement_modes")?;
        if settlement_modes & !(MODE_PUBLIC_BOUND | MODE_SHIELDED) != 0 {
            return Err(DescriptorError::UnknownModeBits(settlement_modes));
        }
        let guarantees = GuaranteeSet::from_bits(reader.u32("guarantees")?)?;
        let issued_at = reader.u64("issued_at")?;
        let expires = reader.u64("expires")?;
        let descriptor = Self {
            protocol_version,
            seller_address,
            transport_key,
            endpoints,
            chain_namespace,
            assets,
            suites,
            settlement_modes,
            guarantees,
            issued_at,
            expires,
            signature: [0u8; AUTHORIZATION_SIGNATURE_BYTES],
        };
        descriptor.validate_structure()?;
        Ok(descriptor)
    }

    fn validate_structure(&self) -> Result<(), DescriptorError> {
        if self.endpoints.is_empty() || self.endpoints.len() > MAX_DESCRIPTOR_ENDPOINTS {
            return Err(DescriptorError::EndpointCount {
                max: MAX_DESCRIPTOR_ENDPOINTS,
            });
        }
        if self
            .endpoints
            .iter()
            .any(|endpoint| endpoint.is_empty() || endpoint.len() > 256)
        {
            return Err(DescriptorError::InvalidEndpoint);
        }
        if self.assets.is_empty() || self.assets.len() > MAX_DESCRIPTOR_ASSETS {
            return Err(DescriptorError::AssetCount {
                max: MAX_DESCRIPTOR_ASSETS,
            });
        }
        if self
            .assets
            .iter()
            .any(|asset| asset.namespace() != &self.chain_namespace)
        {
            return Err(DescriptorError::AssetChainMismatch);
        }
        if self.suites.is_empty() || self.suites.len() > MAX_DESCRIPTOR_SUITES {
            return Err(DescriptorError::SuiteCount {
                max: MAX_DESCRIPTOR_SUITES,
            });
        }
        for suite_id in &self.suites {
            if !suite::is_supported(*suite_id) {
                return Err(DescriptorError::UnsupportedSuite(*suite_id));
            }
        }
        if self.settlement_modes & !(MODE_PUBLIC_BOUND | MODE_SHIELDED) != 0 {
            return Err(DescriptorError::UnknownModeBits(self.settlement_modes));
        }
        if self.settlement_modes == 0 {
            return Err(DescriptorError::MissingSettlementMode);
        }
        for suite_id in &self.suites {
            if self.settlement_modes & MODE_PUBLIC_BOUND != 0 {
                suite::check_mode(*suite_id, SettlementMode::PublicBound)?;
            }
            if self.settlement_modes & MODE_SHIELDED != 0 {
                suite::check_mode(*suite_id, SettlementMode::Shielded)?;
            }
        }
        if self.settlement_modes & MODE_PUBLIC_BOUND != 0
            && (self.guarantees.contains(Guarantee::HiddenAmount)
                || self.guarantees.contains(Guarantee::HiddenRecipient))
        {
            return Err(DescriptorError::ContradictoryPrivacy);
        }
        if self.expires <= self.issued_at {
            return Err(DescriptorError::InvalidValidityWindow);
        }
        GuaranteeSet::from_bits(self.guarantees.bits())?;
        Ok(())
    }

    /// Serializes the descriptor as a portable JSON document.
    pub fn to_json(&self) -> Result<String, DescriptorError> {
        let document = DescriptorDocument::from_descriptor(self);
        serde_json::to_string_pretty(&document)
            .map_err(|error| DescriptorError::Json(error.to_string()))
    }

    /// Parses a descriptor from its JSON document.
    pub fn from_json(json: &str) -> Result<Self, DescriptorError> {
        let document: DescriptorDocument =
            serde_json::from_str(json).map_err(|error| DescriptorError::Json(error.to_string()))?;
        document.into_descriptor()
    }
}

fn mode_bit(mode: SettlementMode) -> u8 {
    match mode {
        SettlementMode::PublicBound => MODE_PUBLIC_BOUND,
        SettlementMode::Shielded => MODE_SHIELDED,
    }
}

/// The requirements a buyer filters descriptors by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryFilter {
    /// Required chain namespace.
    pub chain_namespace: ChainNamespace,
    /// Required asset.
    pub asset: AssetId,
    /// Required settlement mode.
    pub mode: SettlementMode,
    /// Required guarantees.
    pub required_guarantees: GuaranteeSet,
}

/// A portable JSON representation of a descriptor, with byte fields hex-encoded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DescriptorDocument {
    /// Protocol version.
    pub protocol_version: u16,
    /// Hex-encoded seller address.
    pub seller_address: String,
    /// Hex-encoded transport public key.
    pub transport_key: String,
    /// Endpoints.
    pub endpoints: Vec<String>,
    /// CAIP-2 chain namespace.
    pub chain_namespace: String,
    /// CAIP-19 asset identifiers.
    pub assets: Vec<String>,
    /// Supported agreement suites.
    pub suites: Vec<u16>,
    /// Settlement-mode bits.
    pub settlement_modes: u8,
    /// Guarantee bits.
    pub guarantees: u32,
    /// Issuance time.
    pub issued_at: u64,
    /// Expiry time.
    pub expires: u64,
    /// Hex-encoded suite-1 signature.
    pub signature: String,
}

impl DescriptorDocument {
    /// Projects a descriptor into its JSON form.
    #[must_use]
    pub fn from_descriptor(descriptor: &ServiceDescriptor) -> Self {
        Self {
            protocol_version: descriptor.protocol_version,
            seller_address: hex::encode(descriptor.seller_address),
            transport_key: hex::encode(descriptor.transport_key),
            endpoints: descriptor.endpoints.clone(),
            chain_namespace: descriptor.chain_namespace.to_string(),
            assets: descriptor.assets.iter().map(ToString::to_string).collect(),
            suites: descriptor.suites.clone(),
            settlement_modes: descriptor.settlement_modes,
            guarantees: descriptor.guarantees.bits(),
            issued_at: descriptor.issued_at,
            expires: descriptor.expires,
            signature: hex::encode(descriptor.signature),
        }
    }

    /// Rebuilds a descriptor from its JSON form.
    pub fn into_descriptor(self) -> Result<ServiceDescriptor, DescriptorError> {
        let seller_address = hex_to_array::<AUTHORIZATION_KEY_BYTES>(&self.seller_address)?;
        let transport_key = hex_to_array::<TRANSPORT_KEY_BYTES>(&self.transport_key)?;
        let signature = hex_to_array::<AUTHORIZATION_SIGNATURE_BYTES>(&self.signature)?;
        let mut assets = Vec::with_capacity(self.assets.len());
        for asset in &self.assets {
            assets.push(AssetId::parse(asset)?);
        }
        let descriptor = ServiceDescriptor {
            protocol_version: self.protocol_version,
            seller_address,
            transport_key,
            endpoints: self.endpoints,
            chain_namespace: ChainNamespace::parse(&self.chain_namespace)?,
            assets,
            suites: self.suites,
            settlement_modes: self.settlement_modes,
            guarantees: GuaranteeSet::from_bits(self.guarantees)?,
            issued_at: self.issued_at,
            expires: self.expires,
            signature,
        };
        descriptor.validate_structure()?;
        Ok(descriptor)
    }
}

fn hex_to_array<const N: usize>(value: &str) -> Result<[u8; N], DescriptorError> {
    let bytes = hex::decode(value).map_err(|_| DescriptorError::Json("invalid hex".to_owned()))?;
    bytes
        .try_into()
        .map_err(|_| DescriptorError::Json(format!("expected {N} bytes of hex")))
}

/// A configured directory of published descriptors.
#[derive(Debug, Clone, Default)]
pub struct Directory {
    descriptors: Vec<ServiceDescriptor>,
}

/// The JSON shape of a directory document.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DirectoryDocument {
    descriptors: Vec<DescriptorDocument>,
}

impl Directory {
    /// Builds a directory from descriptors after verifying every entry.
    pub fn new(descriptors: Vec<ServiceDescriptor>, now: u64) -> Result<Self, DescriptorError> {
        for descriptor in &descriptors {
            descriptor.verify(now)?;
        }
        Ok(Self { descriptors })
    }

    /// Loads and verifies every descriptor in a directory document.
    ///
    /// Verification is all-or-nothing: a directory containing one bad signature does not load,
    /// because a partially trusted directory is a silent downgrade.
    pub fn from_json(json: &str, now: u64) -> Result<Self, DescriptorError> {
        let document: DirectoryDocument =
            serde_json::from_str(json).map_err(|error| DescriptorError::Json(error.to_string()))?;
        let mut descriptors = Vec::with_capacity(document.descriptors.len());
        for entry in document.descriptors {
            let descriptor = entry.into_descriptor()?;
            descriptor.verify(now)?;
            descriptors.push(descriptor);
        }
        Ok(Self { descriptors })
    }

    /// Serializes all verified descriptors as a portable JSON directory document.
    pub fn to_json(&self) -> Result<String, DescriptorError> {
        let document = DirectoryDocument {
            descriptors: self
                .descriptors
                .iter()
                .map(DescriptorDocument::from_descriptor)
                .collect(),
        };
        serde_json::to_string_pretty(&document)
            .map_err(|error| DescriptorError::Json(error.to_string()))
    }

    /// Returns every verified descriptor.
    #[must_use]
    pub fn descriptors(&self) -> &[ServiceDescriptor] {
        &self.descriptors
    }

    /// Revalidates and returns descriptors that satisfy a buyer's requirements at `now`.
    pub fn compatible(
        &self,
        filter: &DiscoveryFilter,
        now: u64,
    ) -> Result<Vec<&ServiceDescriptor>, DescriptorError> {
        let mut compatible = Vec::new();
        for descriptor in &self.descriptors {
            if descriptor.supports(filter, now)? {
                compatible.push(descriptor);
            }
        }
        Ok(compatible)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use erebus_core::terms::Guarantee;

    fn identity() -> AuthorizationIdentity {
        AuthorizationIdentity::from_bytes(&[0x33; 32]).expect("valid key")
    }

    fn transport_key() -> [u8; TRANSPORT_KEY_BYTES] {
        crate::identity::TransportIdentity::generate()
            .expect("entropy")
            .public_key()
    }

    fn descriptor(now: u64) -> ServiceDescriptor {
        let owner = identity();
        let namespace = ChainNamespace::new("eip155", "10143").expect("namespace");
        let asset = AssetId::new(namespace.clone(), "erc20", "0xabc").expect("asset");
        let guarantees = GuaranteeSet::empty();
        let mut descriptor = ServiceDescriptor::new(
            owner.address(),
            transport_key(),
            vec!["https://seller.example/eleusis".to_owned()],
            namespace,
            vec![asset],
            vec![1],
            MODE_PUBLIC_BOUND,
            guarantees,
            now,
            now + 3600,
        )
        .expect("valid descriptor");
        descriptor.sign(&owner).expect("sign");
        descriptor
    }

    #[test]
    fn a_signed_descriptor_verifies_and_round_trips() {
        let descriptor = descriptor(1_000);
        descriptor.verify(1_000).expect("verify");
        assert_eq!(
            ServiceDescriptor::decode(&descriptor.encode()).expect("decode"),
            descriptor
        );
    }

    #[test]
    fn a_tampered_field_breaks_the_signature() {
        let mut descriptor = descriptor(1_000);
        descriptor.endpoints = vec!["https://attacker.example/eleusis".to_owned()];
        assert_eq!(descriptor.verify(1_000), Err(DescriptorError::BadSignature));
    }

    #[test]
    fn an_expired_descriptor_is_rejected() {
        let descriptor = descriptor(1_000);
        assert!(matches!(
            descriptor.verify(5_000),
            Err(DescriptorError::Expired { .. })
        ));
    }

    #[test]
    fn unsupported_suite_mode_combinations_are_rejected() {
        let mut descriptor = descriptor(1_000);
        descriptor.settlement_modes = MODE_SHIELDED;
        assert!(matches!(
            descriptor.verify(1_000),
            Err(DescriptorError::Suite(SuiteError::UnsupportedMode { .. }))
        ));
    }

    #[test]
    fn invalid_validity_windows_are_rejected() {
        let mut invalid = descriptor(1_000);
        invalid.expires = invalid.issued_at;
        assert_eq!(
            invalid.verify(1_000),
            Err(DescriptorError::InvalidValidityWindow)
        );
        let descriptor = descriptor(1_000);
        assert!(matches!(
            descriptor.verify(999),
            Err(DescriptorError::NotYetValid { .. })
        ));
    }

    #[test]
    fn an_asset_from_another_chain_is_rejected() {
        let mut descriptor = descriptor(1_000);
        let other = ChainNamespace::new("eip155", "1").expect("namespace");
        descriptor.assets = vec![AssetId::new(other, "erc20", "0xabc").expect("asset")];
        assert_eq!(
            descriptor.verify(1_000),
            Err(DescriptorError::AssetChainMismatch)
        );
    }

    #[test]
    fn an_identity_cannot_sign_for_another_address() {
        let mut descriptor = descriptor(1_000);
        let impostor = AuthorizationIdentity::from_bytes(&[0x44; 32]).expect("key");
        assert_eq!(
            descriptor.sign(&impostor),
            Err(DescriptorError::SignerMismatch)
        );
    }

    #[test]
    fn discovery_filters_by_chain_asset_mode_and_guarantees() {
        let descriptor = descriptor(1_000);
        let namespace = ChainNamespace::new("eip155", "10143").expect("namespace");
        let asset = AssetId::new(namespace.clone(), "erc20", "0xabc").expect("asset");
        let mut needed = GuaranteeSet::empty();
        needed.insert(Guarantee::ScopedDisclosure);

        let matching = DiscoveryFilter {
            chain_namespace: namespace.clone(),
            asset: asset.clone(),
            mode: SettlementMode::PublicBound,
            required_guarantees: needed,
        };
        assert!(!descriptor.supports(&matching, 1_000).expect("verified"));

        let matching = DiscoveryFilter {
            required_guarantees: GuaranteeSet::empty(),
            ..matching
        };
        assert!(descriptor.supports(&matching, 1_000).expect("verified"));

        let wrong_asset = DiscoveryFilter {
            asset: AssetId::new(namespace.clone(), "erc20", "0xdef").expect("asset"),
            ..matching.clone()
        };
        assert!(!descriptor.supports(&wrong_asset, 1_000).expect("verified"));

        let mut too_much = GuaranteeSet::empty();
        too_much.insert(Guarantee::AgreementBoundSettlement);
        let unadvertised = DiscoveryFilter {
            required_guarantees: too_much,
            ..matching.clone()
        };
        assert!(!descriptor.supports(&unadvertised, 1_000).expect("verified"));
    }

    #[test]
    fn json_round_trips_with_signature_intact() {
        let descriptor = descriptor(1_000);
        let json = descriptor.to_json().expect("serialize");
        let parsed = ServiceDescriptor::from_json(&json).expect("deserialize");
        assert_eq!(parsed, descriptor);
        parsed.verify(1_000).expect("verify");
    }

    #[test]
    fn a_directory_rejects_a_single_bad_descriptor() {
        let good = descriptor(1_000);
        let mut bad = descriptor(1_000);
        bad.expires = 0;
        let document = format!(
            "{{\"descriptors\":[{},{}]}}",
            good.to_json().expect("json"),
            bad.to_json().expect("json")
        );
        assert!(Directory::from_json(&document, 1_000).is_err());
    }

    #[test]
    fn a_verified_directory_round_trips_through_json() {
        let directory = Directory::new(vec![descriptor(1_000)], 1_000).expect("directory");
        let json = directory.to_json().expect("json");
        let decoded = Directory::from_json(&json, 1_000).expect("decode");
        assert_eq!(decoded.descriptors(), directory.descriptors());
    }

    #[test]
    fn a_descriptor_advertising_an_unknown_suite_is_rejected() {
        let mut descriptor = descriptor(1_000);
        descriptor.suites = vec![9];
        descriptor.verify(1_000).expect_err("unsupported suite");
    }
}
