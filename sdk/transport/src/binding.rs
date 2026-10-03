//! Discovery identity attestation for a separate shielded agreement key.
//!
//! The descriptor signer authorizes one BabyJubJub key for one exact deployment and descriptor.
//! Exchange this attestation inside Noise, not in the public discovery directory. It establishes
//! identity continuity, not payment consent or proof of possession of the BabyJubJub secret.
//! The final role-specific agreement signature remains mandatory.

use crate::{
    descriptor::{ServiceDescriptor, MODE_SHIELDED},
    identity::AuthorizationIdentity,
};
use core::fmt;
use erebus_core::{
    domain::DeploymentDomain,
    encoding::{Reader, Writer},
    ids::KeyBytes,
    shielded::validate_domain,
    suite,
};

const DOMAIN: &[u8] = b"EREBUS_AGREEMENT_KEY_BINDING_V1";
/// Maximum encoded key binding, including its descriptor signature.
pub const MAX_BINDING_BYTES: usize = 512;

/// A key binding is malformed, expired, substituted, or signed by the wrong descriptor identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid discovery-to-agreement key binding")]
pub struct BindingError;

/// Private attestation of a suite-2 agreement key by the suite-1 discovery identity.
#[derive(Clone, PartialEq, Eq)]
pub struct AgreementKeyBinding {
    descriptor_digest: [u8; 32],
    domain: DeploymentDomain,
    key: KeyBytes,
    issued_at: u64,
    expires_at: u64,
    signature: [u8; 65],
}

impl fmt::Debug for AgreementKeyBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AgreementKeyBinding(<redacted>)")
    }
}

impl AgreementKeyBinding {
    /// Attests an existing shielded key with the current descriptor's local signing identity.
    /// The validity window cannot extend beyond the descriptor's window.
    pub fn sign(
        descriptor: &ServiceDescriptor,
        domain: DeploymentDomain,
        key: KeyBytes,
        signer: &AuthorizationIdentity,
        issued_at: u64,
        expires_at: u64,
    ) -> Result<Self, BindingError> {
        if signer.address() != descriptor.seller_address {
            return Err(BindingError);
        }
        let mut binding = Self {
            descriptor_digest: descriptor.digest().map_err(|_| BindingError)?,
            domain,
            key,
            issued_at,
            expires_at,
            signature: [0; 65],
        };
        binding.check_context(descriptor, &binding.domain, &binding.key, issued_at)?;
        binding.signature = signer.sign_digest(&binding.digest()?);
        binding.verify(descriptor, &binding.domain, &binding.key, issued_at)?;
        Ok(binding)
    }

    fn unsigned(&self) -> Result<Vec<u8>, BindingError> {
        let mut writer = Writer::new();
        writer.u16(1);
        writer.fixed(&self.descriptor_digest);
        writer.bytes(&self.domain.encode().map_err(|_| BindingError)?);
        writer.u16(2);
        writer.bytes(self.key.as_bytes());
        writer.u64(self.issued_at);
        writer.u64(self.expires_at);
        Ok(writer.finish())
    }

    fn digest(&self) -> Result<[u8; 32], BindingError> {
        Ok(suite::keccak256(&[DOMAIN, &self.unsigned()?]))
    }

    fn check_context(
        &self,
        descriptor: &ServiceDescriptor,
        domain: &DeploymentDomain,
        expected_key: &KeyBytes,
        now: u64,
    ) -> Result<(), BindingError> {
        descriptor.verify(now).map_err(|_| BindingError)?;
        validate_domain(domain).map_err(|_| BindingError)?;
        if self.domain != *domain
            || self.key != *expected_key
            || self.key.as_bytes().len() != 64
            || self.descriptor_digest != descriptor.digest().map_err(|_| BindingError)?
            || descriptor.chain_namespace != domain.namespace
            || !descriptor.suites.contains(&2)
            || descriptor.settlement_modes & MODE_SHIELDED == 0
            || self.issued_at < descriptor.issued_at
            || self.expires_at > descriptor.expires
            || self.expires_at <= self.issued_at
            || now < self.issued_at
            || now >= self.expires_at
        {
            return Err(BindingError);
        }
        Ok(())
    }

    /// Verifies the descriptor signature and exact key/domain mapping selected by this deal.
    pub fn verify(
        &self,
        descriptor: &ServiceDescriptor,
        domain: &DeploymentDomain,
        expected_key: &KeyBytes,
        now: u64,
    ) -> Result<(), BindingError> {
        self.check_context(descriptor, domain, expected_key, now)?;
        suite::suite(1)
            .map_err(|_| BindingError)?
            .verify_authorization(&descriptor.seller_address, &self.digest()?, &self.signature)
            .map_err(|_| BindingError)
    }

    /// Authenticates the advertised key before a caller constructs its initial draft.
    /// This establishes descriptor authority, not possession or curve-level payment consent.
    pub fn attested_key(
        &self,
        descriptor: &ServiceDescriptor,
        domain: &DeploymentDomain,
        now: u64,
    ) -> Result<KeyBytes, BindingError> {
        self.verify(descriptor, domain, &self.key, now)?;
        Ok(self.key.clone())
    }

    /// Canonical private bytes for encrypted control delivery, not discovery publication.
    pub fn encode(&self) -> Result<Vec<u8>, BindingError> {
        let mut bytes = self.unsigned()?;
        bytes.extend_from_slice(&self.signature);
        if bytes.len() > MAX_BINDING_BYTES {
            return Err(BindingError);
        }
        Ok(bytes)
    }

    /// Strict bounded decoding. Call `verify` before trusting the identity mapping.
    pub fn decode(bytes: &[u8]) -> Result<Self, BindingError> {
        if bytes.len() > MAX_BINDING_BYTES {
            return Err(BindingError);
        }
        let parse = (|| {
            let mut reader = Reader::new(bytes);
            if reader.u16("version")? != 1 {
                return Err(BindingError);
            }
            let descriptor_digest = reader.fixed("descriptor_digest")?;
            let domain = DeploymentDomain::decode(reader.bytes("domain", 1, 128)?)
                .map_err(|_| BindingError)?;
            if reader.u16("suite_id")? != 2 {
                return Err(BindingError);
            }
            let key = KeyBytes::new(reader.bytes("agreement_key", 64, 64)?.to_vec())
                .map_err(|_| BindingError)?;
            let issued_at = reader.u64("issued_at")?;
            let expires_at = reader.u64("expires_at")?;
            let signature = reader.fixed("signature")?;
            reader.finish()?;
            Ok(Self {
                descriptor_digest,
                domain,
                key,
                issued_at,
                expires_at,
                signature,
            })
        })();
        parse
    }
}

impl From<erebus_core::encoding::EncodingError> for BindingError {
    fn from(_: erebus_core::encoding::EncodingError) -> Self {
        Self
    }
}

#[cfg(test)]
mod tests;
