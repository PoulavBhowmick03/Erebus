//! Deployment domains: the on-chain identity an agreement binds its authorizations to.
//!
//! A domain is part of the authorized agreement and of the authorization digest, so a
//! signature made for one deployment cannot be replayed against another. Changing a
//! deployment changes the domain, which changes the commitment and every authorization over
//! it. The decisions record (`docs/metropolis-decisions.md`, D02) states explicitly that this
//! does not promise global cross-chain deal uniqueness; it promises domain separation.

use crate::encoding::{EncodingError, Reader, Writer};
use crate::ids::{AddressBytes, ChainNamespace, IdError, MAX_ADDRESS_BYTES};

/// The on-chain deployment an agreement authorizes settlement against.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeploymentDomain {
    /// CAIP-2 chain namespace, for example `eip155:10143`.
    pub namespace: ChainNamespace,
    /// Settlement contract address, when the settlement mode uses one.
    pub settlement_contract: Option<AddressBytes>,
    /// Shielded pool address, when the settlement mode uses one.
    pub pool: Option<AddressBytes>,
    /// Backend-declared verifier version the agreement was built against.
    pub verifier_version: u32,
}

/// A deployment domain was internally inconsistent.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    /// The domain named neither a settlement contract nor a pool.
    #[error("deployment domain must name a settlement contract or a pool address")]
    NoSettlementTarget,
    /// An identifier was invalid.
    #[error(transparent)]
    Id(#[from] IdError),
    /// A canonical encoding could not be read.
    #[error(transparent)]
    Encoding(#[from] EncodingError),
}

impl DeploymentDomain {
    /// Checks that the domain names at least one settlement target.
    ///
    /// Which target a mode requires is a terms-level rule; this only rejects a domain that
    /// names nothing at all.
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.settlement_contract.is_none() && self.pool.is_none() {
            return Err(DomainError::NoSettlementTarget);
        }
        Ok(())
    }

    pub(crate) fn write(&self, writer: &mut Writer) {
        writer.text(&self.namespace.to_string());
        writer.presence(self.settlement_contract.is_some());
        if let Some(contract) = &self.settlement_contract {
            writer.bytes(contract.as_bytes());
        }
        writer.presence(self.pool.is_some());
        if let Some(pool) = &self.pool {
            writer.bytes(pool.as_bytes());
        }
        writer.u32(self.verifier_version);
    }

    pub(crate) fn read(reader: &mut Reader<'_>) -> Result<Self, DomainError> {
        let namespace = ChainNamespace::parse(reader.text("chain_namespace", 5, 41)?)?;
        let settlement_contract = if reader.optional("settlement_contract")? {
            Some(AddressBytes::from_validated(
                reader
                    .bytes("settlement_contract", 1, MAX_ADDRESS_BYTES)?
                    .to_vec(),
            ))
        } else {
            None
        };
        let pool = if reader.optional("pool")? {
            Some(AddressBytes::from_validated(
                reader.bytes("pool", 1, MAX_ADDRESS_BYTES)?.to_vec(),
            ))
        } else {
            None
        };
        let verifier_version = reader.u32("verifier_version")?;
        Ok(Self {
            namespace,
            settlement_contract,
            pool,
            verifier_version,
        })
    }

    /// Encodes the domain after validating it.
    pub fn encode(&self) -> Result<Vec<u8>, DomainError> {
        self.validate()?;
        let mut writer = Writer::new();
        self.write(&mut writer);
        Ok(writer.finish())
    }

    /// Decodes a complete domain from its canonical bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, DomainError> {
        let mut reader = Reader::new(bytes);
        let domain = Self::read(&mut reader)?;
        reader.finish()?;
        domain.validate()?;
        Ok(domain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn namespace() -> ChainNamespace {
        ChainNamespace::new("eip155", "10143").expect("valid namespace")
    }

    fn address(byte: u8) -> AddressBytes {
        AddressBytes::new(vec![byte; 20]).expect("valid address")
    }

    #[test]
    fn round_trips_through_the_canonical_encoding() {
        let domain = DeploymentDomain {
            namespace: namespace(),
            settlement_contract: Some(address(0x11)),
            pool: None,
            verifier_version: 7,
        };
        let bytes = domain.encode().expect("encodes");
        assert_eq!(DeploymentDomain::decode(&bytes), Ok(domain));
    }

    #[test]
    fn presence_bits_distinguish_absent_addresses() {
        let contract_only = DeploymentDomain {
            namespace: namespace(),
            settlement_contract: Some(address(0x11)),
            pool: None,
            verifier_version: 1,
        };
        let pool_only = DeploymentDomain {
            namespace: namespace(),
            settlement_contract: None,
            pool: Some(address(0x11)),
            verifier_version: 1,
        };
        assert_ne!(
            contract_only.encode().expect("encodes"),
            pool_only.encode().expect("encodes")
        );
    }

    #[test]
    fn empty_domain_is_rejected() {
        let domain = DeploymentDomain {
            namespace: namespace(),
            settlement_contract: None,
            pool: None,
            verifier_version: 1,
        };
        assert_eq!(domain.validate(), Err(DomainError::NoSettlementTarget));
        let bytes = {
            let mut writer = Writer::new();
            domain.write(&mut writer);
            writer.finish()
        };
        assert_eq!(
            DeploymentDomain::decode(&bytes),
            Err(DomainError::NoSettlementTarget)
        );
    }
}
