//! Backend evidence carried from preparation to submission.
//!
//! `PreparedSettlement::backend_evidence` is opaque to the core, so this is where the EVM
//! adapter puts what the contract needs and nothing else: the canonical terms, the commitment
//! blinding, and both role authorizations. No spending key and no transaction key is here.
//!
//! The encoding is the same canonical form the rest of the protocol uses: integers big-endian,
//! variable fields length-prefixed, unknown tags and trailing bytes rejected.

use erebus_core::encoding::{EncodingError, Reader, Writer};

/// Evidence protocol version.
pub const EVIDENCE_PROTOCOL_VERSION: u16 = 1;
/// Length of a commitment blinding.
pub const BLINDING_BYTES: usize = 32;
/// Length of a suite-1 authorization signature.
pub const SIGNATURE_BYTES: usize = 65;

/// Evidence could not be decoded or was not produced by this adapter.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EvidenceError {
    /// A canonical field could not be read.
    #[error(transparent)]
    Encoding(#[from] EncodingError),
    /// The evidence was written by a different protocol version.
    #[error("evidence version {0} is not supported")]
    UnsupportedVersion(u16),
}

/// Everything the settlement contract needs to verify one agreement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementEvidence {
    /// Canonical agreement terms bytes (the contract recomputes the commitment from them).
    pub terms: Vec<u8>,
    /// The commitment blinding.
    pub blinding: [u8; BLINDING_BYTES],
    /// Buyer authorization signature, `r || s || v`.
    pub buyer_signature: [u8; SIGNATURE_BYTES],
    /// Seller authorization signature, `r || s || v`.
    pub seller_signature: [u8; SIGNATURE_BYTES],
}

impl SettlementEvidence {
    /// Encodes the evidence canonically.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.u16(EVIDENCE_PROTOCOL_VERSION);
        writer.bytes(&self.terms);
        writer.fixed(&self.blinding);
        writer.fixed(&self.buyer_signature);
        writer.fixed(&self.seller_signature);
        writer.finish()
    }

    /// Decodes evidence produced by [`Self::encode`].
    pub fn decode(bytes: &[u8]) -> Result<Self, EvidenceError> {
        let mut reader = Reader::new(bytes);
        let version = reader.u16("evidence_version")?;
        if version != EVIDENCE_PROTOCOL_VERSION {
            return Err(EvidenceError::UnsupportedVersion(version));
        }
        let terms = reader.bytes("terms", 1, 4096)?.to_vec();
        let blinding = reader.fixed::<BLINDING_BYTES>("blinding")?;
        let buyer_signature = reader.fixed::<SIGNATURE_BYTES>("buyer_signature")?;
        let seller_signature = reader.fixed::<SIGNATURE_BYTES>("seller_signature")?;
        reader.finish()?;
        Ok(Self {
            terms,
            blinding,
            buyer_signature,
            seller_signature,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SettlementEvidence {
        SettlementEvidence {
            terms: vec![1, 2, 3, 4],
            blinding: [0x11; BLINDING_BYTES],
            buyer_signature: [0x22; SIGNATURE_BYTES],
            seller_signature: [0x33; SIGNATURE_BYTES],
        }
    }

    #[test]
    fn evidence_round_trips() {
        let evidence = sample();
        assert_eq!(
            SettlementEvidence::decode(&evidence.encode()).expect("decode"),
            evidence
        );
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let mut encoded = sample().encode();
        encoded.push(0);
        assert!(matches!(
            SettlementEvidence::decode(&encoded),
            Err(EvidenceError::Encoding(EncodingError::TrailingBytes))
        ));
    }

    #[test]
    fn unknown_versions_are_rejected() {
        let mut encoded = sample().encode();
        encoded[1] = 9;
        assert_eq!(
            SettlementEvidence::decode(&encoded),
            Err(EvidenceError::UnsupportedVersion(9))
        );
    }
}
