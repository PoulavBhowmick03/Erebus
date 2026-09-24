//! Minimal ABI encoding for the settlement call and constructor.
//!
//! The adapter deliberately hand-encodes the two calls it makes instead of generating bindings.
//! There are exactly two fixed shapes, they are ABI-specified, and hand-encoding keeps the
//! dependency surface and the review surface small. A generated binding would encode the same
//! bytes; the Foundry tests are the proof that these bytes are the ones the contract expects.

use sha3::{Digest, Keccak256};

/// The `settle(bytes,bytes32,bytes,bytes,address)` selector.
#[must_use]
pub fn settle_selector() -> [u8; 4] {
    selector("settle(bytes,bytes32,bytes,bytes,address)")
}

/// Topic zero for `DealSettled(bytes32,bytes32,address,address,address,uint256,uint256)`.
#[must_use]
pub fn deal_settled_topic() -> [u8; 32] {
    Keccak256::digest(b"DealSettled(bytes32,bytes32,address,address,address,uint256,uint256)")
        .into()
}

/// Computes a four-byte function selector.
#[must_use]
pub fn selector(signature: &str) -> [u8; 4] {
    let digest: [u8; 32] = Keccak256::digest(signature.as_bytes()).into();
    [digest[0], digest[1], digest[2], digest[3]]
}

/// Encodes the `settle` calldata: `(bytes terms, bytes32 blinding, bytes buyer, bytes seller,
/// address token)`.
#[must_use]
pub fn encode_settle_call(
    terms: &[u8],
    blinding: &[u8; 32],
    buyer_signature: &[u8],
    seller_signature: &[u8],
    token: &[u8; 20],
) -> Vec<u8> {
    let mut encoder = Encoder::new(5);
    encoder.push_dynamic(terms);
    encoder.push_static(blinding);
    encoder.push_dynamic(buyer_signature);
    encoder.push_dynamic(seller_signature);
    encoder.push_static(&left_pad(token));
    let mut out = settle_selector().to_vec();
    out.extend_from_slice(&encoder.finish());
    out
}

/// Encodes the `(uint256,uint32)` constructor arguments for `ErebusSettlement`.
#[must_use]
pub fn encode_settlement_constructor(chain_id: u64, verifier_version: u32) -> Vec<u8> {
    let mut encoder = Encoder::new(2);
    let mut encoded_chain_id = [0u8; 32];
    encoded_chain_id[24..].copy_from_slice(&chain_id.to_be_bytes());
    encoder.push_static(&encoded_chain_id);
    let mut version = [0u8; 32];
    version[28..].copy_from_slice(&verifier_version.to_be_bytes());
    encoder.push_static(&version);
    encoder.finish()
}

/// Encodes the `(string,string)` constructor arguments for the test token.
#[must_use]
pub fn encode_token_constructor(name: &str, symbol: &str) -> Vec<u8> {
    let mut encoder = Encoder::new(2);
    encoder.push_dynamic(name.as_bytes());
    encoder.push_dynamic(symbol.as_bytes());
    encoder.finish()
}

/// Encodes the `mint(address,uint256)` call used to fund the buyer in tests.
#[must_use]
pub fn encode_mint_call(to: &[u8; 20], value: u128) -> Vec<u8> {
    let mut out = selector("mint(address,uint256)").to_vec();
    out.extend_from_slice(&left_pad(to));
    let mut amount = [0u8; 32];
    amount[16..].copy_from_slice(&value.to_be_bytes());
    out.extend_from_slice(&amount);
    out
}

/// Encodes the `approve(address,uint256)` call.
#[must_use]
pub fn encode_approve_call(spender: &[u8; 20], value: u128) -> Vec<u8> {
    let mut out = selector("approve(address,uint256)").to_vec();
    out.extend_from_slice(&left_pad(spender));
    let mut amount = [0u8; 32];
    amount[16..].copy_from_slice(&value.to_be_bytes());
    out.extend_from_slice(&amount);
    out
}

/// Encodes the `balanceOf(address)` calldata for an `eth_call`.
#[must_use]
pub fn encode_balance_of_call(account: &[u8; 20]) -> Vec<u8> {
    let mut out = selector("balanceOf(address)").to_vec();
    out.extend_from_slice(&left_pad(account));
    out
}

/// Encodes the `allowance(address,address)` calldata for an `eth_call`.
#[must_use]
pub fn encode_allowance_call(owner: &[u8; 20], spender: &[u8; 20]) -> Vec<u8> {
    let mut out = selector("allowance(address,address)").to_vec();
    out.extend_from_slice(&left_pad(owner));
    out.extend_from_slice(&left_pad(spender));
    out
}

struct Encoder {
    head: Vec<[u8; 32]>,
    tail: Vec<u8>,
    tail_base: usize,
}

impl Encoder {
    fn new(parameters: usize) -> Self {
        Self {
            head: Vec::with_capacity(parameters),
            tail: Vec::new(),
            tail_base: parameters * 32,
        }
    }

    fn push_static(&mut self, word: &[u8; 32]) {
        self.head.push(*word);
    }

    fn push_dynamic(&mut self, data: &[u8]) {
        let offset = self.tail_base + self.tail.len();
        let mut word = [0u8; 32];
        word[24..].copy_from_slice(&(offset as u64).to_be_bytes());
        self.head.push(word);
        let mut length = [0u8; 32];
        length[24..].copy_from_slice(&(data.len() as u64).to_be_bytes());
        self.tail.extend_from_slice(&length);
        self.tail.extend_from_slice(data);
        let padding = (32 - (data.len() % 32)) % 32;
        self.tail.resize(self.tail.len() + padding, 0);
    }

    fn finish(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.head.len() * 32 + self.tail.len());
        for word in self.head {
            out.extend_from_slice(&word);
        }
        out.extend_from_slice(&self.tail);
        out
    }
}

fn left_pad(bytes: &[u8; 20]) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[12..].copy_from_slice(bytes);
    word
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settle_selector_is_the_keccak_prefix() {
        let digest: [u8; 32] =
            Keccak256::digest(b"settle(bytes,bytes32,bytes,bytes,address)").into();
        assert_eq!(settle_selector(), digest[..4]);
    }

    #[test]
    fn deal_settled_topic_is_the_full_event_digest() {
        let digest: [u8; 32] = Keccak256::digest(
            b"DealSettled(bytes32,bytes32,address,address,address,uint256,uint256)",
        )
        .into();
        assert_eq!(deal_settled_topic(), digest);
    }

    #[test]
    fn dynamic_offsets_are_relative_to_the_head() {
        let encoded = encode_settle_call(b"abcd", &[0u8; 32], b"ef", b"gh", &[0u8; 20]);
        // Head starts after the selector: terms offset is 5 * 32.
        let terms_offset = u64::from_be_bytes(encoded[4 + 24..4 + 32].try_into().unwrap());
        assert_eq!(terms_offset, 160);
        let buyer_offset = u64::from_be_bytes(encoded[4 + 64 + 24..4 + 96].try_into().unwrap());
        assert_eq!(buyer_offset, 160 + 64);
    }

    #[test]
    fn constructor_arguments_are_head_then_tail() {
        let encoded = encode_settlement_constructor(31_337, 1);
        let chain_id = u64::from_be_bytes(encoded[24..32].try_into().unwrap());
        assert_eq!(chain_id, 31_337);
        let version = u32::from_be_bytes(encoded[60..64].try_into().unwrap());
        assert_eq!(version, 1);
        assert_eq!(encoded.len(), 64);
    }
}
