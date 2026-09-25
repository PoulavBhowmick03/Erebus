//! Exact field mapping for the M4/M5 shielded agreement statement.
//!
//! This module does not enable suite 2 for settlement. It lets the Rust side independently
//! reproduce the circuit's commitment and authorization messages before that boundary opens.

use ark_bn254::Fr;
use ark_ff::{BigInteger, PrimeField};
use light_poseidon::{Poseidon, PoseidonHasher};
use sha2::{Digest, Sha256};

use crate::auth::Role;
use crate::commitment::{CommitmentBlinding, DealCommitment, DealNullifier};
use crate::suite::SHIELDED_POSEIDON_EDDSA_SUITE_ID;
use crate::terms::{AgreementTerms, SettlementMode, CURRENT_PROTOCOL_VERSION};

/// The first shielded circuit's exact required guarantee bits.
pub const SHIELDED_GUARANTEES: u32 = 0x7;

/// A value cannot be represented by, or is outside, the first shielded circuit.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShieldedMapError {
    /// A field outside the fixed M5 shape was supplied.
    #[error("unsupported shielded agreement shape: {0}")]
    Shape(&'static str),
    /// A value is not a canonical BN254 scalar.
    #[error("{0} is outside the supported BN254 field shape")]
    Field(&'static str),
    /// A Poseidon permutation failed unexpectedly.
    #[error("Poseidon field hashing failed")]
    Hash,
}

/// Validated fields shared by the Rust agreement and the shielded circuit.
#[derive(Debug, Clone)]
pub struct ShieldedDeal {
    domain: Fr,
    deal_id: Fr,
    revision: Fr,
    transcript_lo: Fr,
    transcript_hi: Fr,
    service_lo: Fr,
    service_hi: Fr,
    nonce_lo: Fr,
    nonce_hi: Fr,
    asset: Fr,
    amount: Fr,
    buyer_ax: Fr,
    buyer_ay: Fr,
    seller_ax: Fr,
    seller_ay: Fr,
    recipient_spend_tag: Fr,
    expiry: Fr,
}

fn field_bytes(value: Fr) -> [u8; 32] {
    let bytes = value.into_bigint().to_bytes_be();
    let mut result = [0u8; 32];
    result[32 - bytes.len()..].copy_from_slice(&bytes);
    result
}

fn canonical_field(bytes: &[u8], label: &'static str) -> Result<Fr, ShieldedMapError> {
    if bytes.len() != 32 {
        return Err(ShieldedMapError::Shape(label));
    }
    let value = Fr::from_be_bytes_mod_order(bytes);
    if field_bytes(value).as_slice() != bytes {
        return Err(ShieldedMapError::Field(label));
    }
    Ok(value)
}

fn nonzero_field(bytes: &[u8], label: &'static str) -> Result<Fr, ShieldedMapError> {
    let value = canonical_field(bytes, label)?;
    if value == Fr::from(0u64) {
        return Err(ShieldedMapError::Field(label));
    }
    Ok(value)
}

fn limbs(bytes: &[u8; 32]) -> (Fr, Fr) {
    (
        Fr::from_be_bytes_mod_order(&bytes[..16]),
        Fr::from_be_bytes_mod_order(&bytes[16..]),
    )
}

fn hash(inputs: &[Fr]) -> Result<Fr, ShieldedMapError> {
    let mut poseidon =
        Poseidon::<Fr>::new_circom(inputs.len()).map_err(|_| ShieldedMapError::Hash)?;
    poseidon.hash(inputs).map_err(|_| ShieldedMapError::Hash)
}

impl ShieldedDeal {
    /// Maps only the fixed, zero-fee, one-asset suite-2 agreement shape into field inputs.
    pub fn from_terms(terms: &AgreementTerms) -> Result<Self, ShieldedMapError> {
        if terms.protocol_version != CURRENT_PROTOCOL_VERSION
            || terms.suite_id != SHIELDED_POSEIDON_EDDSA_SUITE_ID
            || terms.settlement_mode != SettlementMode::Shielded
            || terms.required_guarantees.bits() != SHIELDED_GUARANTEES
            || terms.fee_policy.fee.get() != 0
            || terms.fee_policy.recipient.is_some()
        {
            return Err(ShieldedMapError::Shape("suite, mode, guarantees, or fee"));
        }
        if terms.revision == 0 || terms.amount.get() == 0 || terms.expiry == 0 {
            return Err(ShieldedMapError::Shape("revision, amount, or expiry"));
        }
        if terms.service.validate().is_err() {
            return Err(ShieldedMapError::Shape("service record"));
        }
        if terms.domain.namespace.family() != "eip155"
            || terms.asset.namespace() != &terms.domain.namespace
            || terms.asset.asset_namespace() != "erc20"
        {
            return Err(ShieldedMapError::Shape("EVM chain or ERC-20 asset"));
        }
        let chain_reference = terms.domain.namespace.reference();
        let chain_id = chain_reference
            .parse::<u64>()
            .map_err(|_| ShieldedMapError::Shape("chain id"))?;
        if chain_id == 0 || chain_id.to_string() != chain_reference {
            return Err(ShieldedMapError::Shape("canonical chain id"));
        }
        let contract = terms
            .domain
            .settlement_contract
            .as_ref()
            .ok_or(ShieldedMapError::Shape("settlement contract"))?;
        let pool = terms
            .domain
            .pool
            .as_ref()
            .ok_or(ShieldedMapError::Shape("pool"))?;
        if contract.as_bytes().len() != 20
            || contract != pool
            || contract.as_bytes().iter().all(|byte| *byte == 0)
        {
            return Err(ShieldedMapError::Shape(
                "pool and contract must be one EVM address",
            ));
        }
        let asset_ref = terms.asset.asset_reference();
        if asset_ref.len() != 42
            || !asset_ref.starts_with("0x")
            || !asset_ref[2..]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ShieldedMapError::Shape("canonical token address"));
        }
        let asset_bytes =
            hex::decode(&asset_ref[2..]).map_err(|_| ShieldedMapError::Shape("token address"))?;
        if asset_bytes.iter().all(|byte| *byte == 0) {
            return Err(ShieldedMapError::Shape("zero token address"));
        }
        let buyer = terms.buyer_authorization_key.as_bytes();
        let seller = terms.seller_authorization_key.as_bytes();
        if buyer.len() != 64 || seller.len() != 64 {
            return Err(ShieldedMapError::Shape("BabyJubJub authorization keys"));
        }
        let buyer_ax = canonical_field(&buyer[..32], "buyer Ax")?;
        let buyer_ay = canonical_field(&buyer[32..], "buyer Ay")?;
        let seller_ax = canonical_field(&seller[..32], "seller Ax")?;
        let seller_ay = canonical_field(&seller[32..], "seller Ay")?;
        let recipient_spend_tag =
            nonzero_field(terms.payment_recipient.as_bytes(), "recipient spend tag")?;
        let domain = hash(&[
            Fr::from(chain_id),
            Fr::from_be_bytes_mod_order(contract.as_bytes()),
            Fr::from(terms.domain.verifier_version),
        ])?;
        let (transcript_lo, transcript_hi) = limbs(&terms.transcript_root);
        let service_digest: [u8; 32] = Sha256::digest(
            terms
                .service
                .encode()
                .map_err(|_| ShieldedMapError::Shape("service"))?,
        )
        .into();
        let (service_lo, service_hi) = limbs(&service_digest);
        let (nonce_lo, nonce_hi) = limbs(&terms.settlement_nonce);
        Ok(Self {
            domain,
            deal_id: Fr::from_be_bytes_mod_order(&terms.deal_id),
            revision: Fr::from(terms.revision),
            transcript_lo,
            transcript_hi,
            service_lo,
            service_hi,
            nonce_lo,
            nonce_hi,
            asset: Fr::from_be_bytes_mod_order(&asset_bytes),
            amount: Fr::from_be_bytes_mod_order(&terms.amount.get().to_be_bytes()),
            buyer_ax,
            buyer_ay,
            seller_ax,
            seller_ay,
            recipient_spend_tag,
            expiry: Fr::from(terms.expiry),
        })
    }

    /// Computes the suite-2 deal commitment that both parties authorize.
    pub fn commitment(
        &self,
        blinding: &CommitmentBlinding,
    ) -> Result<DealCommitment, ShieldedMapError> {
        let blind = nonzero_field(blinding.as_bytes(), "blinding")?;
        let payment_salt = hash(&[Fr::from(2009u64), self.domain, self.nonce_lo, self.nonce_hi])?;
        let a = hash(&[
            Fr::from(2001u64),
            self.domain,
            self.deal_id,
            self.revision,
            self.transcript_lo,
            self.transcript_hi,
        ])?;
        let b = hash(&[
            self.service_lo,
            self.service_hi,
            self.nonce_lo,
            self.nonce_hi,
            self.asset,
            self.amount,
        ])?;
        let c = hash(&[
            self.buyer_ax,
            self.buyer_ay,
            self.seller_ax,
            self.seller_ay,
            self.recipient_spend_tag,
            payment_salt,
        ])?;
        Ok(DealCommitment::from_bytes(field_bytes(hash(&[
            Fr::from(2002u64),
            a,
            b,
            c,
            blind,
            self.expiry,
        ])?)))
    }

    /// Computes the suite-2 consumption identity shared across revisions.
    pub fn deal_nullifier(&self) -> Result<DealNullifier, ShieldedMapError> {
        Ok(DealNullifier::from_bytes(field_bytes(hash(&[
            Fr::from(2003u64),
            self.domain,
            self.buyer_ax,
            self.buyer_ay,
            self.nonce_lo,
            self.nonce_hi,
        ])?)))
    }

    /// Computes the role-separated field message expected by the circuit's EdDSA verifier.
    pub fn authorization_message(
        &self,
        role: Role,
        commitment: &DealCommitment,
    ) -> Result<[u8; 32], ShieldedMapError> {
        let field = canonical_field(commitment.as_bytes(), "deal commitment")?;
        let tag = match role {
            Role::Buyer => 2004u64,
            Role::Seller => 2005u64,
        };
        Ok(field_bytes(hash(&[Fr::from(tag), self.domain, field])?))
    }
}
