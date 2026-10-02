//! An independent seller process: signs one proposed agreement, nothing else.
//!
//! It reads a proposal (canonical terms and blinding) and its own key, verifies the terms,
//! signs the seller authorization digest, and prints the encoded authorization. It never sees
//! the buyer key, never contacts a chain, and never submits.
//!
//! Request on stdin:
//! ```json
//! {"terms":"<hex canonical AgreementTerms>","blinding":"<hex 32 bytes>",
//!  "seller_key_file":"<owner-only 32-byte hex key>"}
//! ```

use std::io::{Read, Write};

use erebus_core::auth::{authorization_digest, Authorization, Role};
use erebus_core::commitment::{commit_agreement, CommitmentBlinding};
use erebus_core::ids::SignatureBytes;
use erebus_core::terms::AgreementTerms;
use k256::ecdsa::SigningKey;
use serde::Deserialize;
use serde_json::json;

const LIMIT: usize = 64 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Proposal {
    terms: String,
    blinding: String,
    seller_key_file: std::path::PathBuf,
}

fn main() {
    let mut input = String::new();
    if std::io::stdin()
        .take(LIMIT as u64)
        .read_to_string(&mut input)
        .is_err()
    {
        fail("cannot read proposal");
    }
    let proposal: Proposal = match serde_json::from_str(&input) {
        Ok(proposal) => proposal,
        Err(_) => fail("invalid proposal"),
    };
    let result = sign(&proposal);
    let failed = result.is_err();
    let response = result.unwrap_or_else(|error| json!({"status": "error", "error": error}));
    println!("{response}");
    let _ = std::io::stdout().flush();
    std::process::exit(i32::from(failed));
}

fn sign(proposal: &Proposal) -> Result<serde_json::Value, &'static str> {
    let terms = AgreementTerms::decode(&hex::decode(&proposal.terms).map_err(|_| "invalid terms")?)
        .map_err(|_| "invalid terms")?;
    let blinding_bytes: [u8; 32] = hex::decode(&proposal.blinding)
        .map_err(|_| "invalid blinding")?
        .try_into()
        .map_err(|_| "invalid blinding")?;
    let blinding = CommitmentBlinding::from_bytes(blinding_bytes);
    let key_bytes: [u8; 32] = hex::decode(
        std::fs::read_to_string(&proposal.seller_key_file)
            .map_err(|_| "cannot read seller key")?
            .trim()
            .trim_start_matches("0x"),
    )
    .map_err(|_| "invalid seller key")?
    .try_into()
    .map_err(|_| "seller key must be 32 bytes")?;
    let key = SigningKey::from_slice(&key_bytes).map_err(|_| "invalid seller key")?;
    let seller_address = address_of(&key);
    if terms.seller_authorization_key.as_bytes() != seller_address {
        return Err("proposal names a different seller key");
    }
    let commitment = commit_agreement(&terms, &blinding).map_err(|_| "invalid proposal")?;
    let digest = authorization_digest(&terms.domain, Role::Seller, &commitment, terms.suite_id)
        .map_err(|_| "invalid proposal")?;
    let (signature, recovery_id) = key
        .sign_prehash_recoverable(&digest)
        .map_err(|_| "cannot sign")?;
    let mut bytes = [0u8; 65];
    bytes[..64].copy_from_slice(&signature.to_bytes());
    bytes[64] = recovery_id.to_byte();
    let authorization = Authorization {
        role: Role::Seller,
        suite_id: terms.suite_id,
        commitment,
        signature: SignatureBytes::new(bytes.to_vec()).map_err(|_| "invalid signature")?,
    };
    Ok(json!({
        "status": "ok",
        "seller_address": format!("0x{}", hex::encode(seller_address)),
        "deal_commitment": commitment.to_hex(),
        "seller_authorization": hex::encode(authorization.encode().map_err(|_| "cannot encode")?),
    }))
}

fn address_of(key: &SigningKey) -> [u8; 20] {
    use sha3::Digest;
    let point = key.verifying_key().to_encoded_point(false);
    let digest: [u8; 32] = sha3::Keccak256::digest(&point.as_bytes()[1..]).into();
    let mut address = [0u8; 20];
    address.copy_from_slice(&digest[12..]);
    address
}

fn fail(message: &str) -> ! {
    println!("{}", json!({"status": "error", "error": message}));
    std::process::exit(1);
}
