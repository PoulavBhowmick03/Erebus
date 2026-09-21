//! Canonical decoding is strict.
//!
//! Every prefix of a valid encoding is rejected, appending a byte is rejected, and the
//! vectors in `tests/fixtures/agreement-v1-vectors.json` are the reference bytes. This is
//! what keeps two encoders from silently agreeing on a malformed message.

use erebus_core::terms::AgreementTerms;

const FIXTURE: &str = include_str!("fixtures/agreement-v1-vectors.json");

fn canonical_encodings() -> Vec<(String, Vec<u8>)> {
    let fixture: serde_json::Value = serde_json::from_str(FIXTURE).expect("fixture parses");
    fixture["vectors"]
        .as_array()
        .expect("vectors array")
        .iter()
        .filter(|vector| vector["terms"]["settlementMode"] != "shielded")
        .map(|vector| {
            let name = vector["name"].as_str().expect("name").to_owned();
            let hex = vector["expected"]["canonicalHex"]
                .as_str()
                .expect("canonical hex");
            (name, hex::decode(hex).expect("fixture hex"))
        })
        .collect()
}

#[test]
fn every_truncation_is_rejected() {
    for (name, bytes) in canonical_encodings() {
        for end in 0..bytes.len() {
            assert!(
                AgreementTerms::decode(&bytes[..end]).is_err(),
                "{name}: prefix of {end} bytes decoded"
            );
        }
    }
}

#[test]
fn trailing_bytes_are_rejected() {
    for (name, bytes) in canonical_encodings() {
        let mut extended = bytes;
        extended.push(0x00);
        assert!(
            AgreementTerms::decode(&extended).is_err(),
            "{name}: trailing byte was accepted"
        );
    }
}

#[test]
fn the_empty_encoding_is_rejected() {
    assert!(AgreementTerms::decode(&[]).is_err());
}

#[test]
fn full_encodings_still_decode() {
    for (name, bytes) in canonical_encodings() {
        AgreementTerms::decode(&bytes).unwrap_or_else(|error| panic!("{name}: {error}"));
    }
}
