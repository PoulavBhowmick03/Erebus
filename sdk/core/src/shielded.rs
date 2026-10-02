//! Exact field mapping for the M4/M5 shielded agreement statement.
//!
//! Suite 2 uses this fixed mapping for commitments and role messages. Transcript roots
//! retain the independent transport hash and enter the circuit as two 128-bit limbs.

use ark_bn254::Fr;
use ark_ff::{BigInteger, PrimeField};
use light_poseidon::{Poseidon, PoseidonHasher};
use sha2::{Digest, Sha256};

use crate::auth::Role;
use crate::commitment::{CommitmentBlinding, DealCommitment, DealNullifier};
use crate::domain::DeploymentDomain;
use crate::suite::SHIELDED_POSEIDON_EDDSA_SUITE_ID;
use crate::terms::{AgreementTerms, SettlementMode, CURRENT_PROTOCOL_VERSION};

/// The first shielded circuit's exact required guarantee bits.
pub const SHIELDED_GUARANTEES: u32 = 0x7;
/// The fixed Merkle depth used by the M5 pool and its circuits.
pub const NOTE_TREE_DEPTH: usize = 20;

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
    chain_id: Fr,
    contract_address: Fr,
    verifier_version: Fr,
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

pub(crate) fn field_bytes(value: Fr) -> [u8; 32] {
    let bytes = value.into_bigint().to_bytes_be();
    let mut result = [0u8; 32];
    result[32 - bytes.len()..].copy_from_slice(&bytes);
    result
}

pub(crate) fn canonical_field(bytes: &[u8], label: &'static str) -> Result<Fr, ShieldedMapError> {
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

/// Maps a complete disclosure-grant digest to a domain-separated suite-2 message.
/// Both 128-bit limbs are retained; no reduction of the original digest occurs.
/// This offchain domain is distinct from payment authorizations and note hashes.
pub fn disclosure_message(digest: &[u8; 32]) -> Result<[u8; 32], ShieldedMapError> {
    let (high, low) = limbs(digest);
    Ok(field_bytes(hash(&[Fr::from(3001u64), high, low])?))
}

fn domain_fields(domain: &DeploymentDomain) -> Result<[Fr; 4], ShieldedMapError> {
    if domain.namespace.family() != "eip155" || domain.verifier_version == 0 {
        return Err(ShieldedMapError::Shape("EVM domain or verifier version"));
    }
    let reference = domain.namespace.reference();
    let chain = reference
        .parse::<u64>()
        .map_err(|_| ShieldedMapError::Shape("chain id"))?;
    if chain == 0 || chain.to_string() != reference {
        return Err(ShieldedMapError::Shape("canonical chain id"));
    }
    let contract = domain
        .settlement_contract
        .as_ref()
        .ok_or(ShieldedMapError::Shape("settlement contract"))?;
    if domain.pool.as_ref() != Some(contract)
        || contract.as_bytes().len() != 20
        || contract.as_bytes().iter().all(|byte| *byte == 0)
    {
        return Err(ShieldedMapError::Shape(
            "pool and contract must be one EVM address",
        ));
    }
    let chain = Fr::from(chain);
    let contract = Fr::from_be_bytes_mod_order(contract.as_bytes());
    let version = Fr::from(domain.verifier_version);
    Ok([chain, contract, version, hash(&[chain, contract, version])?])
}

/// Checks that a deployment is representable by the fixed shielded circuit.
pub fn validate_domain(domain: &DeploymentDomain) -> Result<(), ShieldedMapError> {
    domain_fields(domain).map(|_| ())
}

/// Computes the suite-2 role message with the exact circuit deployment mapping.
pub fn authorization_message(
    domain: &DeploymentDomain,
    role: Role,
    commitment: &DealCommitment,
) -> Result<[u8; 32], ShieldedMapError> {
    let [_, _, _, domain] = domain_fields(domain)?;
    role_message(domain, role, commitment)
}

fn role_message(
    domain: Fr,
    role: Role,
    commitment: &DealCommitment,
) -> Result<[u8; 32], ShieldedMapError> {
    let field = canonical_field(commitment.as_bytes(), "deal commitment")?;
    let tag = match role {
        Role::Buyer => 2004u64,
        Role::Seller => 2005u64,
    };
    Ok(field_bytes(hash(&[Fr::from(tag), domain, field])?))
}

/// Computes the owner-specific tag committed into an M5 note.
pub fn note_spend_tag(spend_secret: &[u8; 32]) -> Result<[u8; 32], ShieldedMapError> {
    let secret = nonzero_field(spend_secret, "spend secret")?;
    Ok(field_bytes(hash(&[Fr::from(2006u64), secret])?))
}

/// Computes an M5 note commitment. `amount` may be zero for a change output.
pub fn note_commitment(
    asset: &[u8; 20],
    amount: u128,
    owner: &[u8; 64],
    spend_tag: &[u8; 32],
    salt: &[u8; 32],
) -> Result<[u8; 32], ShieldedMapError> {
    if asset.iter().all(|byte| *byte == 0) {
        return Err(ShieldedMapError::Shape("zero note asset"));
    }
    let ax = canonical_field(&owner[..32], "owner Ax")?;
    let ay = canonical_field(&owner[32..], "owner Ay")?;
    let tag = nonzero_field(spend_tag, "spend tag")?;
    let salt = canonical_field(salt, "note salt")?;
    Ok(field_bytes(hash(&[
        Fr::from(2007u64),
        Fr::from_be_bytes_mod_order(asset),
        Fr::from(amount),
        ax,
        ay,
        tag,
        salt,
    ])?))
}

/// Computes the one-time spend identity for a note opening.
pub fn note_nullifier(
    spend_secret: &[u8; 32],
    commitment: &[u8; 32],
) -> Result<[u8; 32], ShieldedMapError> {
    let secret = nonzero_field(spend_secret, "spend secret")?;
    let note = canonical_field(commitment, "note commitment")?;
    Ok(field_bytes(hash(&[Fr::from(2008u64), secret, note])?))
}

/// Hashes two canonical M5 tree nodes in left-to-right order.
pub fn note_tree_parent(left: &[u8; 32], right: &[u8; 32]) -> Result<[u8; 32], ShieldedMapError> {
    let left = canonical_field(left, "left tree node")?;
    let right = canonical_field(right, "right tree node")?;
    Ok(field_bytes(hash(&[left, right])?))
}

/// Recomputes the M5 tree root for a leaf at `index` and its 20 siblings.
pub fn note_root_from_path(
    commitment: &[u8; 32],
    index: u32,
    siblings: &[[u8; 32]; NOTE_TREE_DEPTH],
) -> Result<[u8; 32], ShieldedMapError> {
    if index >= 1 << NOTE_TREE_DEPTH {
        return Err(ShieldedMapError::Shape("note index outside tree"));
    }
    let mut node = *commitment;
    canonical_field(&node, "note commitment")?;
    for (level, sibling) in siblings.iter().enumerate() {
        node = if index & (1 << level) == 0 {
            note_tree_parent(&node, sibling)?
        } else {
            note_tree_parent(sibling, &node)?
        };
    }
    Ok(node)
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
        let [chain_id, contract_address, verifier_version, domain] = domain_fields(&terms.domain)?;
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
            chain_id,
            contract_address,
            verifier_version,
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
        let payment_salt = canonical_field(&self.payment_salt()?, "payment salt")?;
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
        role_message(self.domain, role, commitment)
    }

    /// Predicts the recipient note bound to this deal's seller, amount, and nonce.
    ///
    /// The recipient still needs its private spend secret to recognize and spend the note.
    pub fn payment_note_commitment(&self) -> Result<[u8; 32], ShieldedMapError> {
        let salt = canonical_field(&self.payment_salt()?, "payment salt")?;
        Ok(field_bytes(hash(&[
            Fr::from(2007u64),
            self.asset,
            self.amount,
            self.seller_ax,
            self.seller_ay,
            self.recipient_spend_tag,
            salt,
        ])?))
    }

    /// Derives the seller's deterministic note salt from the private agreement context.
    pub fn payment_salt(&self) -> Result<[u8; 32], ShieldedMapError> {
        Ok(field_bytes(hash(&[
            Fr::from(2009u64),
            self.domain,
            self.nonce_lo,
            self.nonce_hi,
        ])?))
    }

    /// Field elements expected by the fixed M5 transfer circuit for this agreement.
    ///
    /// The caller must still verify both authorizations and construct the note witness.
    pub fn transfer_circuit_fields(
        &self,
        blinding: &CommitmentBlinding,
    ) -> Result<Vec<(&'static str, [u8; 32])>, ShieldedMapError> {
        let blind = nonzero_field(blinding.as_bytes(), "blinding")?;
        let commitment = self.commitment(blinding)?;
        let nullifier = self.deal_nullifier()?;
        let payment = self.payment_note_commitment()?;
        Ok(vec![
            ("chainId", field_bytes(self.chain_id)),
            ("contractAddress", field_bytes(self.contract_address)),
            ("verifierVersion", field_bytes(self.verifier_version)),
            ("asset", field_bytes(self.asset)),
            ("dealCommitment", *commitment.as_bytes()),
            ("dealNullifier", *nullifier.as_bytes()),
            ("paymentCommitment", payment),
            ("expiry", field_bytes(self.expiry)),
            ("dealId", field_bytes(self.deal_id)),
            ("revision", field_bytes(self.revision)),
            ("transcriptRootLo", field_bytes(self.transcript_lo)),
            ("transcriptRootHi", field_bytes(self.transcript_hi)),
            ("serviceDigestLo", field_bytes(self.service_lo)),
            ("serviceDigestHi", field_bytes(self.service_hi)),
            ("settlementNonceLo", field_bytes(self.nonce_lo)),
            ("settlementNonceHi", field_bytes(self.nonce_hi)),
            ("amount", field_bytes(self.amount)),
            ("blinding", field_bytes(blind)),
            ("buyerAx", field_bytes(self.buyer_ax)),
            ("buyerAy", field_bytes(self.buyer_ay)),
            ("sellerAx", field_bytes(self.seller_ax)),
            ("sellerAy", field_bytes(self.seller_ay)),
            ("recipientSpendTag", field_bytes(self.recipient_spend_tag)),
        ])
    }
}
