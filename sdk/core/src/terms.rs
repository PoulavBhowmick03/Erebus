//! Canonical agreement terms.
//!
//! [`AgreementTerms`] is the complete description of one deal revision: who authorizes it,
//! what is bought, how it settles, when it expires, and the service promise attached to it.
//! Its canonical encoding is specified in `docs/metropolis-agreement.md` and pinned by
//! `tests/fixtures/agreement-v1-vectors.json`.
//!
//! Two rules are enforced here rather than left to callers:
//!
//! - A settlement mode and the guarantee set must agree. Public-bound settlement cannot
//!   require hidden amounts or recipients, and shielded settlement must require both, so a
//!   signed agreement cannot silently downgrade a privacy requirement (decisions D01).
//! - A deal consumes one identity across every revision. The fields that derive it
//!   (`deal_id`, `settlement_nonce`, buyer key, domain) are validated as a unit in
//!   [`crate::commitment`]; revisions share all of them.

use crate::domain::{DeploymentDomain, DomainError};
use crate::encoding::{EncodingError, Reader, Writer};
use crate::ids::{AssetId, BaseUnits, IdError, KeyBytes, MAX_KEY_BYTES};
use crate::service::{ServiceError, ServiceRecord};

/// The agreement protocol version this crate encodes.
pub const CURRENT_PROTOCOL_VERSION: u16 = 1;

/// Longest accepted asset identifier text, in bytes.
pub const MAX_ASSET_ID_BYTES: usize = 256;

/// The settlement mechanism an agreement selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SettlementMode {
    /// A publicly visible payment bound to the accepted agreement.
    PublicBound,
    /// A shielded payment whose verifier enforces the agreement binding.
    Shielded,
}

impl SettlementMode {
    /// Stable encoding tag.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::PublicBound => 1,
            Self::Shielded => 2,
        }
    }

    /// Parses a stable encoding tag.
    pub fn from_tag(tag: u8) -> Result<Self, EncodingError> {
        match tag {
            1 => Ok(Self::PublicBound),
            2 => Ok(Self::Shielded),
            _ => Err(EncodingError::UnknownTag("settlement_mode", tag)),
        }
    }

    /// Human-readable name used in diagnostics.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::PublicBound => "public-bound",
            Self::Shielded => "shielded",
        }
    }
}

impl core::fmt::Display for SettlementMode {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.name())
    }
}

/// A property a settlement backend must provide for a deal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Guarantee {
    /// The paid amount is not visible on-chain.
    HiddenAmount,
    /// The payment recipient is not visible on-chain.
    HiddenRecipient,
    /// The settlement verifier enforces the agreement binding, not just client policy.
    AgreementBoundSettlement,
    /// Deal-scoped disclosure to a chosen recipient is possible.
    ScopedDisclosure,
}

impl Guarantee {
    /// Every guarantee, in a fixed order.
    pub const ALL: [Self; 4] = [
        Self::HiddenAmount,
        Self::HiddenRecipient,
        Self::AgreementBoundSettlement,
        Self::ScopedDisclosure,
    ];

    /// Single-bit encoding.
    #[must_use]
    pub const fn bit(self) -> u32 {
        match self {
            Self::HiddenAmount => 0b0001,
            Self::HiddenRecipient => 0b0010,
            Self::AgreementBoundSettlement => 0b0100,
            Self::ScopedDisclosure => 0b1000,
        }
    }

    /// Human-readable name used in diagnostics.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::HiddenAmount => "hidden-amount",
            Self::HiddenRecipient => "hidden-recipient",
            Self::AgreementBoundSettlement => "agreement-bound-settlement",
            Self::ScopedDisclosure => "scoped-disclosure",
        }
    }
}

impl core::fmt::Display for Guarantee {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.name())
    }
}

/// A set of required settlement guarantees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct GuaranteeSet(u32);

impl GuaranteeSet {
    /// The empty set.
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// A set holding exactly one guarantee.
    #[must_use]
    pub const fn from_guarantee(guarantee: Guarantee) -> Self {
        Self(guarantee.bit())
    }

    /// Adds a guarantee.
    pub fn insert(&mut self, guarantee: Guarantee) {
        self.0 |= guarantee.bit();
    }

    /// Reports whether a guarantee is present.
    #[must_use]
    pub const fn contains(self, guarantee: Guarantee) -> bool {
        self.0 & guarantee.bit() != 0
    }

    /// Raw bits.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Builds a set from raw bits, rejecting bits that name no guarantee.
    pub fn from_bits(bits: u32) -> Result<Self, TermsError> {
        let known = Self::all_bits();
        if bits & !known != 0 {
            return Err(TermsError::UnknownGuaranteeBits(bits & !known));
        }
        Ok(Self(bits))
    }

    const fn all_bits() -> u32 {
        let mut bits = 0;
        let mut index = 0;
        while index < Guarantee::ALL.len() {
            bits |= Guarantee::ALL[index].bit();
            index += 1;
        }
        bits
    }

    /// Iterates the present guarantees in [`Guarantee::ALL`] order.
    pub fn iter(self) -> impl Iterator<Item = Guarantee> {
        Guarantee::ALL
            .into_iter()
            .filter(move |guarantee| self.contains(*guarantee))
    }
}

/// How relayer or settlement fees are bound into the agreement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeePolicy {
    /// Fixed fee in base units of the agreement asset; zero means no fee.
    pub fee: BaseUnits,
    /// Fee recipient. Present exactly when the fee is non-zero.
    pub recipient: Option<KeyBytes>,
}

impl FeePolicy {
    /// A policy with no fee.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            fee: BaseUnits::new(0),
            recipient: None,
        }
    }

    /// Checks that a recipient is present exactly when a fee is charged.
    pub fn validate(&self) -> Result<(), TermsError> {
        match (self.fee.is_zero(), self.recipient.is_some()) {
            (true, false) | (false, true) => Ok(()),
            (true, true) => Err(TermsError::UnexpectedFeeRecipient),
            (false, false) => Err(TermsError::MissingFeeRecipient),
        }
    }
}

/// The complete authorized description of one deal revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgreementTerms {
    /// Agreement protocol version.
    pub protocol_version: u16,
    /// Agreement suite that hashes and authorizes this revision.
    pub suite_id: u16,
    /// Deployment the authorizations are bound to.
    pub domain: DeploymentDomain,
    /// Deal identifier shared by every revision of the same deal.
    pub deal_id: [u8; 16],
    /// Monotonic revision number, starting at one.
    pub revision: u32,
    /// Root of the negotiation transcript this revision concludes.
    pub transcript_root: [u8; 32],
    /// Buyer's agreement authorization key; interpreted by the suite.
    pub buyer_authorization_key: KeyBytes,
    /// Seller's agreement authorization key; interpreted by the suite.
    pub seller_authorization_key: KeyBytes,
    /// Payment recipient key; interpreted by the backend (a note key for shielded modes).
    pub payment_recipient: KeyBytes,
    /// Asset being paid, as a CAIP-19 identifier.
    pub asset: AssetId,
    /// Amount in the asset's base units; must be greater than zero.
    pub amount: BaseUnits,
    /// Unix timestamp at which the authorization stops being executable.
    /// Verification rejects at `now >= expiry`.
    pub expiry: u64,
    /// Fee policy bound into the authorized terms.
    pub fee_policy: FeePolicy,
    /// Selected settlement mechanism.
    pub settlement_mode: SettlementMode,
    /// Guarantees the selected backend must provide.
    pub required_guarantees: GuaranteeSet,
    /// Fixed per-deal nonce that seeds the consumed deal identity.
    pub settlement_nonce: [u8; 32],
    /// Service promise bound into the agreement.
    pub service: ServiceRecord,
}

/// Agreement terms were internally inconsistent or not decodable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TermsError {
    /// Asset and deployment must name the same chain.
    #[error("agreement asset is not on the deployment chain")]
    AssetDomainMismatch,
    /// Revisions start at one.
    #[error("agreement revision must be greater than zero")]
    ZeroRevision,
    /// The suite is not available for the selected settlement mode.
    #[error(transparent)]
    SuiteMode(#[from] crate::suite::SuiteError),
    /// The protocol version is not the one this crate encodes.
    #[error("protocol version {0} is not supported")]
    UnsupportedProtocolVersion(u16),
    /// The suite id names no implemented suite.
    #[error("agreement suite {0} is not supported")]
    UnsupportedSuite(u16),
    /// The amount was zero.
    #[error("agreement amount must be greater than zero")]
    ZeroAmount,
    /// The expiry was zero.
    #[error("agreement expiry must be greater than zero")]
    ZeroExpiry,
    /// A fee was charged with no recipient bound.
    #[error("a non-zero fee requires a fee recipient")]
    MissingFeeRecipient,
    /// A recipient was bound with no fee charged.
    #[error("a fee recipient requires a non-zero fee")]
    UnexpectedFeeRecipient,
    /// A guarantee bit was set that names no guarantee.
    #[error("required guarantees contain unknown bits 0x{0:08x}")]
    UnknownGuaranteeBits(u32),
    /// A mode and guarantee combination is contradictory.
    #[error("settlement mode {mode} cannot require guarantee `{guarantee}`")]
    ContradictoryGuarantee {
        /// The selected mode.
        mode: SettlementMode,
        /// The guarantee that contradicts it.
        guarantee: Guarantee,
    },
    /// Shielded settlement did not require the core privacy guarantees.
    #[error("shielded settlement must require guarantee `{0}`")]
    ShieldedWithoutGuarantee(Guarantee),
    /// An authorization key did not match the suite's key length.
    #[error("{role} authorization key must be {expected} bytes, found {actual}")]
    AuthorizationKeyLength {
        /// Which role's key failed.
        role: &'static str,
        /// Length the suite requires.
        expected: usize,
        /// Length found in the terms.
        actual: usize,
    },
    /// Public-bound settlement named no settlement contract.
    #[error("public-bound settlement requires a settlement contract address")]
    MissingSettlementContract,
    /// Shielded settlement named no pool.
    #[error("shielded settlement requires a pool address")]
    MissingPool,
    /// The service record was invalid.
    #[error(transparent)]
    Service(#[from] ServiceError),
    /// The deployment domain was invalid.
    #[error(transparent)]
    Domain(#[from] DomainError),
    /// An identifier was invalid.
    #[error(transparent)]
    Id(#[from] IdError),
    /// A canonical encoding could not be read.
    #[error(transparent)]
    Encoding(#[from] EncodingError),
}

impl AgreementTerms {
    /// Checks every field and the mode/guarantee/suite consistency rules.
    pub fn validate(&self) -> Result<(), TermsError> {
        if self.protocol_version != CURRENT_PROTOCOL_VERSION {
            return Err(TermsError::UnsupportedProtocolVersion(
                self.protocol_version,
            ));
        }
        if !crate::suite::is_supported(self.suite_id) {
            return Err(TermsError::UnsupportedSuite(self.suite_id));
        }
        let key_length = crate::suite::suite(self.suite_id)
            .map_err(|_| TermsError::UnsupportedSuite(self.suite_id))?
            .authorization_key_length();
        for (role, key) in [
            ("buyer", &self.buyer_authorization_key),
            ("seller", &self.seller_authorization_key),
        ] {
            if key.as_bytes().len() != key_length {
                return Err(TermsError::AuthorizationKeyLength {
                    role,
                    expected: key_length,
                    actual: key.as_bytes().len(),
                });
            }
        }
        self.domain.validate()?;
        if self.asset.namespace() != &self.domain.namespace {
            return Err(TermsError::AssetDomainMismatch);
        }
        if self.revision == 0 {
            return Err(TermsError::ZeroRevision);
        }
        if self.amount.is_zero() {
            return Err(TermsError::ZeroAmount);
        }
        if self.expiry == 0 {
            return Err(TermsError::ZeroExpiry);
        }
        self.fee_policy.validate()?;
        match self.settlement_mode {
            SettlementMode::PublicBound => {
                if self.domain.settlement_contract.is_none() {
                    return Err(TermsError::MissingSettlementContract);
                }
                for guarantee in [Guarantee::HiddenAmount, Guarantee::HiddenRecipient] {
                    if self.required_guarantees.contains(guarantee) {
                        return Err(TermsError::ContradictoryGuarantee {
                            mode: self.settlement_mode,
                            guarantee,
                        });
                    }
                }
            }
            SettlementMode::Shielded => {
                if self.domain.pool.is_none() {
                    return Err(TermsError::MissingPool);
                }
                for guarantee in [Guarantee::HiddenAmount, Guarantee::HiddenRecipient] {
                    if !self.required_guarantees.contains(guarantee) {
                        return Err(TermsError::ShieldedWithoutGuarantee(guarantee));
                    }
                }
            }
        }
        self.service.validate()?;
        crate::suite::check_mode(self.suite_id, self.settlement_mode)?;
        Ok(())
    }

    pub(crate) fn write(&self, writer: &mut Writer) {
        writer.u16(self.protocol_version);
        writer.u16(self.suite_id);
        self.domain.write(writer);
        writer.fixed(&self.deal_id);
        writer.u32(self.revision);
        writer.fixed(&self.transcript_root);
        writer.bytes(self.buyer_authorization_key.as_bytes());
        writer.bytes(self.seller_authorization_key.as_bytes());
        writer.bytes(self.payment_recipient.as_bytes());
        writer.text(&self.asset.to_string());
        writer.u128(self.amount.get());
        writer.u64(self.expiry);
        writer.u128(self.fee_policy.fee.get());
        writer.presence(self.fee_policy.recipient.is_some());
        if let Some(recipient) = &self.fee_policy.recipient {
            writer.bytes(recipient.as_bytes());
        }
        writer.u8(self.settlement_mode.tag());
        writer.u32(self.required_guarantees.bits());
        writer.fixed(&self.settlement_nonce);
        self.service.write(writer);
    }

    /// Encodes the terms after validating them.
    pub fn encode(&self) -> Result<Vec<u8>, TermsError> {
        self.validate()?;
        let mut writer = Writer::new();
        self.write(&mut writer);
        Ok(writer.finish())
    }

    /// Decodes a complete terms value from its canonical bytes and validates it.
    pub fn decode(bytes: &[u8]) -> Result<Self, TermsError> {
        let mut reader = Reader::new(bytes);
        let protocol_version = reader.u16("protocol_version")?;
        let suite_id = reader.u16("suite_id")?;
        let domain = DeploymentDomain::read(&mut reader)?;
        let deal_id = reader.fixed("deal_id")?;
        let revision = reader.u32("revision")?;
        let transcript_root = reader.fixed("transcript_root")?;
        let buyer_authorization_key = KeyBytes::from_validated(
            reader
                .bytes("buyer_authorization_key", 1, MAX_KEY_BYTES)?
                .to_vec(),
        );
        let seller_authorization_key = KeyBytes::from_validated(
            reader
                .bytes("seller_authorization_key", 1, MAX_KEY_BYTES)?
                .to_vec(),
        );
        let payment_recipient = KeyBytes::from_validated(
            reader
                .bytes("payment_recipient", 1, MAX_KEY_BYTES)?
                .to_vec(),
        );
        let asset = AssetId::parse(reader.text("asset_identifier", 1, MAX_ASSET_ID_BYTES)?)?;
        let amount = BaseUnits::new(reader.u128("amount")?);
        let expiry = reader.u64("expiry")?;
        let fee = BaseUnits::new(reader.u128("fee")?);
        let recipient = if reader.optional("fee_recipient")? {
            Some(KeyBytes::from_validated(
                reader.bytes("fee_recipient", 1, MAX_KEY_BYTES)?.to_vec(),
            ))
        } else {
            None
        };
        let settlement_mode = SettlementMode::from_tag(reader.u8("settlement_mode")?)?;
        let required_guarantees = GuaranteeSet::from_bits(reader.u32("required_guarantees")?)?;
        let settlement_nonce = reader.fixed("settlement_nonce")?;
        let service = ServiceRecord::read(&mut reader)?;
        reader.finish()?;
        let terms = Self {
            protocol_version,
            suite_id,
            domain,
            deal_id,
            revision,
            transcript_root,
            buyer_authorization_key,
            seller_authorization_key,
            payment_recipient,
            asset,
            amount,
            expiry,
            fee_policy: FeePolicy { fee, recipient },
            settlement_mode,
            required_guarantees,
            settlement_nonce,
            service,
        };
        terms.validate()?;
        Ok(terms)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::ids::{AddressBytes, ChainNamespace};

    pub(crate) fn example_terms() -> AgreementTerms {
        let namespace = ChainNamespace::new("eip155", "10143").expect("valid namespace");
        AgreementTerms {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            suite_id: crate::suite::EVM_SECP256K1_KECCAK_SUITE_ID,
            domain: DeploymentDomain {
                namespace,
                settlement_contract: Some(
                    AddressBytes::new(vec![0x11; 20]).expect("valid address"),
                ),
                pool: None,
                verifier_version: 1,
            },
            deal_id: [0x07; 16],
            revision: 1,
            transcript_root: [0x33; 32],
            buyer_authorization_key: KeyBytes::new(vec![0x44; 20]).expect("valid key"),
            seller_authorization_key: KeyBytes::new(vec![0x55; 20]).expect("valid key"),
            payment_recipient: KeyBytes::new(vec![0x55; 20]).expect("valid key"),
            asset: AssetId::parse("eip155:10143/erc20:0x00000000000000000000000000000000000000aa")
                .expect("valid asset"),
            amount: BaseUnits::new(1_000_000),
            expiry: 1_800_000_000,
            fee_policy: FeePolicy::none(),
            settlement_mode: SettlementMode::PublicBound,
            required_guarantees: GuaranteeSet::from_guarantee(Guarantee::AgreementBoundSettlement),
            settlement_nonce: [0x66; 32],
            service: ServiceRecord {
                resource: "gpu.h100.hour".to_owned(),
                quantity: BaseUnits::new(500),
                unit: "gpu-hour".to_owned(),
                access_recipient: KeyBytes::new(vec![0x44; 20]).expect("valid key"),
                delivery_deadline: 1_800_000_000,
                fulfillment_method: "http-access".to_owned(),
                fulfillment_digest: [0u8; 32],
            },
        }
    }

    #[test]
    fn round_trips_through_the_canonical_encoding() {
        let terms = example_terms();
        let bytes = terms.encode().expect("encodes");
        assert_eq!(AgreementTerms::decode(&bytes), Ok(terms));
    }

    #[test]
    fn public_bound_rejects_hidden_guarantees() {
        let mut terms = example_terms();
        terms.required_guarantees = GuaranteeSet::from_guarantee(Guarantee::HiddenAmount);
        assert_eq!(
            terms.validate(),
            Err(TermsError::ContradictoryGuarantee {
                mode: SettlementMode::PublicBound,
                guarantee: Guarantee::HiddenAmount,
            })
        );
    }

    #[test]
    fn shielded_requires_both_privacy_guarantees() {
        let mut terms = example_terms();
        terms.settlement_mode = SettlementMode::Shielded;
        terms.domain.pool = Some(AddressBytes::new(vec![0x22; 20]).expect("valid address"));
        terms.required_guarantees = GuaranteeSet::from_guarantee(Guarantee::HiddenAmount);
        assert_eq!(
            terms.validate(),
            Err(TermsError::ShieldedWithoutGuarantee(
                Guarantee::HiddenRecipient
            ))
        );
        terms.required_guarantees.insert(Guarantee::HiddenRecipient);
        assert_eq!(
            terms.validate(),
            Err(TermsError::SuiteMode(
                crate::suite::SuiteError::UnsupportedMode {
                    suite_id: 1,
                    mode: SettlementMode::Shielded,
                }
            ))
        );
    }

    #[test]
    fn shielded_requires_a_pool() {
        let mut terms = example_terms();
        terms.settlement_mode = SettlementMode::Shielded;
        terms.required_guarantees = GuaranteeSet::from_guarantee(Guarantee::HiddenAmount);
        terms.required_guarantees.insert(Guarantee::HiddenRecipient);
        assert_eq!(terms.validate(), Err(TermsError::MissingPool));
    }

    #[test]
    fn zero_amount_and_expiry_are_rejected() {
        let mut terms = example_terms();
        terms.amount = BaseUnits::new(0);
        assert_eq!(terms.validate(), Err(TermsError::ZeroAmount));

        let mut terms = example_terms();
        terms.expiry = 0;
        assert_eq!(terms.validate(), Err(TermsError::ZeroExpiry));
    }

    #[test]
    fn zero_revisions_and_cross_chain_assets_fail_at_decode() {
        let mut terms = example_terms();
        terms.revision = 0;
        assert_eq!(terms.validate(), Err(TermsError::ZeroRevision));
        assert_eq!(
            AgreementTerms::decode(&encode_unchecked(&terms)),
            Err(TermsError::ZeroRevision)
        );
        terms.revision = 1;
        terms.asset = AssetId::parse("eip155:1/erc20:0xaa").unwrap();
        assert_eq!(terms.validate(), Err(TermsError::AssetDomainMismatch));
        assert_eq!(
            AgreementTerms::decode(&encode_unchecked(&terms)),
            Err(TermsError::AssetDomainMismatch)
        );
    }

    #[test]
    fn fee_recipient_must_match_the_fee() {
        let mut terms = example_terms();
        terms.fee_policy = FeePolicy {
            fee: BaseUnits::new(5),
            recipient: None,
        };
        assert_eq!(terms.validate(), Err(TermsError::MissingFeeRecipient));

        let mut terms = example_terms();
        terms.fee_policy = FeePolicy {
            fee: BaseUnits::new(0),
            recipient: Some(KeyBytes::new(vec![0x11; 20]).expect("valid key")),
        };
        assert_eq!(terms.validate(), Err(TermsError::UnexpectedFeeRecipient));
    }

    #[test]
    fn unsupported_versions_and_suites_are_rejected() {
        let mut terms = example_terms();
        terms.protocol_version = 2;
        assert_eq!(
            terms.validate(),
            Err(TermsError::UnsupportedProtocolVersion(2))
        );

        let mut terms = example_terms();
        terms.suite_id = 999;
        assert_eq!(terms.validate(), Err(TermsError::UnsupportedSuite(999)));
    }

    #[test]
    fn unknown_guarantee_bits_are_rejected() {
        assert_eq!(
            GuaranteeSet::from_bits(0b1_0000),
            Err(TermsError::UnknownGuaranteeBits(0b1_0000))
        );
    }

    fn encode_unchecked(terms: &AgreementTerms) -> Vec<u8> {
        let mut writer = Writer::new();
        terms.write(&mut writer);
        writer.finish()
    }

    fn first_difference(left: &[u8], right: &[u8]) -> usize {
        left.iter()
            .zip(right)
            .position(|(a, b)| a != b)
            .expect("encodings must differ")
    }

    fn shielded_variant(terms: &AgreementTerms) -> AgreementTerms {
        let mut shielded = terms.clone();
        shielded.settlement_mode = SettlementMode::Shielded;
        shielded.required_guarantees = GuaranteeSet::empty();
        shielded.required_guarantees.insert(Guarantee::HiddenAmount);
        shielded
            .required_guarantees
            .insert(Guarantee::HiddenRecipient);
        shielded
            .required_guarantees
            .insert(Guarantee::AgreementBoundSettlement);
        shielded
    }

    #[test]
    fn unsupported_version_and_suite_fail_at_decode() {
        let mut terms = example_terms();
        terms.protocol_version = 2;
        assert_eq!(
            AgreementTerms::decode(&encode_unchecked(&terms)),
            Err(TermsError::UnsupportedProtocolVersion(2))
        );

        let mut terms = example_terms();
        terms.suite_id = 99;
        assert_eq!(
            AgreementTerms::decode(&encode_unchecked(&terms)),
            Err(TermsError::UnsupportedSuite(99))
        );
    }

    #[test]
    fn unknown_settlement_mode_tags_fail_at_decode() {
        let public = example_terms();
        let shielded = shielded_variant(&public);
        // Both variants share every earlier field, so the first difference is the mode tag.
        let offset = first_difference(&encode_unchecked(&public), &encode_unchecked(&shielded));
        for tag in [0u8, 3u8] {
            let mut bytes = encode_unchecked(&public);
            bytes[offset] = tag;
            assert_eq!(
                AgreementTerms::decode(&bytes),
                Err(TermsError::Encoding(EncodingError::UnknownTag(
                    "settlement_mode",
                    tag
                )))
            );
        }
    }

    #[test]
    fn unknown_guarantee_bits_fail_at_decode() {
        let mut with_scoped = example_terms();
        with_scoped
            .required_guarantees
            .insert(Guarantee::ScopedDisclosure);
        let offset = first_difference(
            &encode_unchecked(&example_terms()),
            &encode_unchecked(&with_scoped),
        );
        let mut bytes = encode_unchecked(&example_terms());
        bytes[offset] = 0x10;
        assert_eq!(
            AgreementTerms::decode(&bytes),
            Err(TermsError::UnknownGuaranteeBits(0x10))
        );
    }
}
