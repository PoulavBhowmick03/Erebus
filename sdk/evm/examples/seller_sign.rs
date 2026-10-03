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
use zeroize::Zeroizing;

const LIMIT: usize = 64 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Proposal {
    terms: String,
    blinding: String,
    seller_key_file: std::path::PathBuf,
}

fn main() {
    let mut input = Zeroizing::new(String::new());
    if std::io::stdin()
        .take((LIMIT + 1) as u64)
        .read_to_string(&mut input)
        .is_err()
        || input.len() > LIMIT
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
    let flushed = std::io::stdout().flush().is_ok();
    std::process::exit(i32::from(failed || !flushed));
}

fn sign(proposal: &Proposal) -> Result<serde_json::Value, &'static str> {
    let terms = AgreementTerms::decode(&hex::decode(&proposal.terms).map_err(|_| "invalid terms")?)
        .map_err(|_| "invalid terms")?;
    let blinding_bytes: [u8; 32] = hex::decode(&proposal.blinding)
        .map_err(|_| "invalid blinding")?
        .try_into()
        .map_err(|_| "invalid blinding")?;
    let blinding = CommitmentBlinding::from_bytes(blinding_bytes);
    let metadata = std::fs::symlink_metadata(&proposal.seller_key_file)
        .map_err(|_| "cannot read seller key")?;
    if !metadata.is_file() || metadata.len() > 128 {
        return Err("seller key must be a bounded regular file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("seller key must be owner-only");
        }
    }
    let mut text = Zeroizing::new(String::new());
    std::fs::File::open(&proposal.seller_key_file)
        .map_err(|_| "cannot read seller key")?
        .take(129)
        .read_to_string(&mut text)
        .map_err(|_| "cannot read seller key")?;
    if text.len() > 128 {
        return Err("seller key must be a bounded regular file");
    }
    let decoded = Zeroizing::new(
        hex::decode(text.trim().strip_prefix("0x").unwrap_or(text.trim()))
            .map_err(|_| "invalid seller key")?,
    );
    if decoded.len() != 32 {
        return Err("seller key must be 32 bytes");
    }
    let key = SigningKey::from_slice(&decoded).map_err(|_| "invalid seller key")?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal(path: std::path::PathBuf) -> Proposal {
        // Invalid terms are rejected before any private key is opened.
        Proposal {
            terms: "00".into(),
            blinding: "00".repeat(32),
            seller_key_file: path,
        }
    }

    #[test]
    fn malformed_proposal_does_not_read_an_arbitrary_key() {
        let input = proposal("/missing/private-key".into());
        assert_eq!(sign(&input).unwrap_err(), "invalid terms");
    }

    #[test]
    fn seller_signing_requires_a_private_regular_key_and_the_named_identity() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../core/tests/fixtures/agreement-v1-vectors.json"
        ))
        .unwrap();
        let vector = &fixture["vectors"][0];
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("seller.key");
        std::fs::write(&path, hex::encode([2; 32])).unwrap();
        let input = Proposal {
            terms: vector["expected"]["canonicalHex"].as_str().unwrap().into(),
            blinding: vector["blindingHex"].as_str().unwrap().into(),
            seller_key_file: path.clone(),
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::{symlink, PermissionsExt};
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(sign(&input).unwrap_err(), "seller key must be owner-only");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            let link = directory.path().join("link");
            symlink(&path, &link).unwrap();
            let linked = Proposal {
                seller_key_file: link,
                ..proposal(path.clone())
            };
            let linked = Proposal {
                terms: input.terms.clone(),
                blinding: input.blinding.clone(),
                ..linked
            };
            assert_eq!(
                sign(&linked).unwrap_err(),
                "seller key must be a bounded regular file"
            );
        }
        let response = sign(&input).unwrap();
        assert_eq!(
            response["seller_address"],
            format!(
                "0x{}",
                vector["terms"]["sellerAuthorizationKeyHex"]
                    .as_str()
                    .unwrap()
            )
        );
        std::fs::write(&path, hex::encode([3; 32])).unwrap();
        assert_eq!(
            sign(&input).unwrap_err(),
            "proposal names a different seller key"
        );
    }
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
