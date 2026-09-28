//! Circomlib-compatible suite-2 key and signature encoding.
//!
//! The shared agreement suite uses these primitives. The M5 circuit
//! consumes unpacked `R8x`, `R8y`, and `S` fields, each encoded here as 32-byte big-endian.

use ark_bn254::Fr;
use serde_json::Value;
use zeroize::Zeroizing;

use crate::shielded::{canonical_field, field_bytes};
use crate::{
    auth::{Authorization, Role},
    commitment::{CommitmentBlinding, DealCommitment, DealNullifier},
    shielded::{ShieldedDeal, ShieldedMapError},
    suite::SHIELDED_POSEIDON_EDDSA_SUITE_ID,
    terms::AgreementTerms,
};

/// Two BabyJubJub point coordinates, each a canonical BN254 field element.
pub const SHIELDED_KEY_BYTES: usize = 64;
/// `R8x || R8y || S`, each a 32-byte big-endian integer.
pub const SHIELDED_SIGNATURE_BYTES: usize = 96;

/// A suite-2 key or signature could not be derived or verified.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShieldedAuthError {
    /// A field is not a canonical BN254 element.
    #[error("invalid suite-2 field: {0}")]
    Field(&'static str),
    /// A signature or key has the wrong byte length.
    #[error("invalid suite-2 key or signature length")]
    Length,
    /// The EdDSA implementation rejected the request or produced malformed output.
    #[error("BabyJubJub operation failed")]
    Primitive,
    /// The signature does not verify for the supplied role message and key.
    #[error("suite-2 signature does not verify")]
    InvalidSignature,
    /// The agreement opening or fixed suite-2 shape is invalid.
    #[error(transparent)]
    Agreement(#[from] ShieldedMapError),
    /// The supplied authorizations do not cover the required roles and commitment.
    #[error("suite-2 authorizations do not cover this agreement")]
    AuthorizationMismatch,
    /// The agreement is expired for settlement.
    #[error("suite-2 agreement has expired")]
    Expired,
}

/// Verified role consent and note identity for one fixed-shape shielded agreement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedShieldedAgreement {
    /// The agreement commitment both sides signed.
    pub commitment: DealCommitment,
    /// The one-time deal identity shared across revisions.
    pub nullifier: DealNullifier,
    /// Recipient note expected from the accepted agreement.
    pub payment_note: [u8; 32],
}

fn result(json: &str) -> Result<Value, ShieldedAuthError> {
    let value: Value = serde_json::from_str(json).map_err(|_| ShieldedAuthError::Primitive)?;
    if value["success"] != true || !value["result"].is_object() {
        return Err(ShieldedAuthError::Primitive);
    }
    Ok(value)
}

fn output_field(value: &Value, name: &'static str) -> Result<[u8; 32], ShieldedAuthError> {
    let decimal = value[name].as_str().ok_or(ShieldedAuthError::Primitive)?;
    let field = decimal
        .parse::<Fr>()
        .map_err(|_| ShieldedAuthError::Field(name))?;
    if field.to_string() != decimal {
        return Err(ShieldedAuthError::Field(name));
    }
    Ok(field_bytes(field))
}

fn input_field(bytes: &[u8], label: &'static str) -> Result<String, ShieldedAuthError> {
    let field = canonical_field(bytes, label).map_err(|_| ShieldedAuthError::Field(label))?;
    Ok(field.to_string())
}

/// Derives the circuit's `(Ax, Ay)` key from a 32-byte seed.
pub fn derive_key(seed: &[u8; 32]) -> Result<[u8; SHIELDED_KEY_BYTES], ShieldedAuthError> {
    let encoded = Zeroizing::new(hex::encode(seed));
    let request = Zeroizing::new(format!(
        r#"{{"operation":"derivePublicKey","data":{{"privateKeyHex":"{}"}}}}"#,
        encoded.as_str()
    ));
    let response =
        rust_eddsa_helper::sign_eddsa(&request).map_err(|_| ShieldedAuthError::Primitive)?;
    let output = result(&response)?;
    let mut key = [0u8; SHIELDED_KEY_BYTES];
    key[..32].copy_from_slice(&output_field(&output["result"], "Ax")?);
    key[32..].copy_from_slice(&output_field(&output["result"], "Ay")?);
    Ok(key)
}

/// Signs an already-Poseidon-hashed, canonical role message with a local seed.
pub fn sign_message(
    seed: &[u8; 32],
    message: &[u8; 32],
) -> Result<([u8; SHIELDED_KEY_BYTES], [u8; SHIELDED_SIGNATURE_BYTES]), ShieldedAuthError> {
    let message = input_field(message, "authorization message")?;
    let encoded = Zeroizing::new(hex::encode(seed));
    let request = Zeroizing::new(format!(
        r#"{{"operation":"sign","data":{{"msgHash":"{}","privateKeyHex":"{}"}}}}"#,
        message,
        encoded.as_str()
    ));
    let response =
        rust_eddsa_helper::sign_eddsa(&request).map_err(|_| ShieldedAuthError::Primitive)?;
    let output = result(&response)?;
    let fields = &output["result"];
    let mut key = [0u8; SHIELDED_KEY_BYTES];
    key[..32].copy_from_slice(&output_field(fields, "Ax")?);
    key[32..].copy_from_slice(&output_field(fields, "Ay")?);
    let mut signature = [0u8; SHIELDED_SIGNATURE_BYTES];
    signature[..32].copy_from_slice(&output_field(fields, "R8x")?);
    signature[32..64].copy_from_slice(&output_field(fields, "R8y")?);
    signature[64..].copy_from_slice(&output_field(fields, "S")?);
    Ok((key, signature))
}

/// Verifies the exact three-field signature passed to `EdDSAPoseidonVerifier`.
pub fn verify_message(
    key: &[u8],
    message: &[u8; 32],
    signature: &[u8],
) -> Result<(), ShieldedAuthError> {
    if key.len() != SHIELDED_KEY_BYTES || signature.len() != SHIELDED_SIGNATURE_BYTES {
        return Err(ShieldedAuthError::Length);
    }
    let request = serde_json::json!({
        "operation": "verify",
        "data": {
            "msgHash": input_field(message, "authorization message")?,
            "publicKeyAx": input_field(&key[..32], "Ax")?,
            "publicKeyAy": input_field(&key[32..], "Ay")?,
            "R8x": input_field(&signature[..32], "R8x")?,
            "R8y": input_field(&signature[32..64], "R8y")?,
            "S": input_field(&signature[64..], "S")?,
        }
    });
    let response = rust_eddsa_helper::verify_eddsa(&request.to_string())
        .map_err(|_| ShieldedAuthError::Primitive)?;
    let output: Value =
        serde_json::from_str(&response).map_err(|_| ShieldedAuthError::Primitive)?;
    if output["success"] == true && output["result"] == true {
        Ok(())
    } else {
        Err(ShieldedAuthError::InvalidSignature)
    }
}

/// Checks the opening and distinct buyer/seller signatures before a private settlement.
///
/// This does not select a backend, construct a witness, or assert that funds exist.
pub fn verify_agreement(
    terms: &AgreementTerms,
    blinding: &CommitmentBlinding,
    buyer: &Authorization,
    seller: &Authorization,
    now: u64,
) -> Result<VerifiedShieldedAgreement, ShieldedAuthError> {
    let deal = ShieldedDeal::from_terms(terms)?;
    let commitment = deal.commitment(blinding)?;
    for (role, authorization) in [(Role::Buyer, buyer), (Role::Seller, seller)] {
        if authorization.role != role
            || authorization.suite_id != SHIELDED_POSEIDON_EDDSA_SUITE_ID
            || authorization.commitment != commitment
        {
            return Err(ShieldedAuthError::AuthorizationMismatch);
        }
        crate::auth::verify_authorization_signature(terms, &commitment, blinding, authorization)
            .map_err(|_| ShieldedAuthError::AuthorizationMismatch)?;
    }
    if now >= terms.expiry {
        return Err(ShieldedAuthError::Expired);
    }
    Ok(VerifiedShieldedAgreement {
        commitment,
        nullifier: deal.deal_nullifier()?,
        payment_note: deal.payment_note_commitment()?,
    })
}
