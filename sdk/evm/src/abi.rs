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

/// The `consumedDeals(bytes32)` selector: the public getter for the contract's replay map.
#[must_use]
pub fn consumed_deals_selector() -> [u8; 4] {
    selector("consumedDeals(bytes32)")
}

/// Encodes the `consumedDeals(bytes32)` calldata for an `eth_call`.
#[must_use]
pub fn encode_consumed_deals_call(deal_nullifier: &[u8; 32]) -> Vec<u8> {
    let mut out = consumed_deals_selector().to_vec();
    out.extend_from_slice(deal_nullifier);
    out
}

/// Decodes one ABI `bool` return word.
///
/// Exactly 32 bytes whose value is 0 or 1; anything else is not a `bool` the contract returned.
#[must_use]
pub fn decode_bool_word(data: &[u8]) -> Option<bool> {
    let word: &[u8; 32] = data.try_into().ok()?;
    if word[..31] != [0u8; 31] {
        return None;
    }
    match word[31] {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

/// ABI data could not be decoded into the expected shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("ABI decode error: {0}")]
pub struct AbiDecodeError(pub &'static str);

/// The arguments of one `settle(bytes,bytes32,bytes,bytes,address)` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettleCall {
    /// Canonical agreement terms bytes.
    pub terms: Vec<u8>,
    /// Commitment blinding.
    pub blinding: [u8; 32],
    /// Buyer authorization signature as submitted.
    pub buyer_signature: Vec<u8>,
    /// Seller authorization signature as submitted.
    pub seller_signature: Vec<u8>,
    /// ERC-20 token argument.
    pub token: [u8; 20],
}

/// Decodes `settle` calldata the way the Solidity ABI decoder accepts it.
///
/// A foreign submitter may use any encoder, so this does not require the canonical layout
/// [`encode_settle_call`] produces: dynamic offsets are followed with bounds checks, as solc
/// does. It does require what solc enforces before the function body runs: the selector, a
/// complete head, in-bounds dynamic data, and a clean address word. Trailing calldata is
/// ignored, as it is on chain.
pub fn decode_settle_call(calldata: &[u8]) -> Result<SettleCall, AbiDecodeError> {
    let selector_bytes = calldata
        .get(..4)
        .ok_or(AbiDecodeError("calldata shorter than a selector"))?;
    if selector_bytes != settle_selector() {
        return Err(AbiDecodeError("not a settle call"));
    }
    let arguments = &calldata[4..];
    let terms = dynamic_argument(arguments, 0)?.to_vec();
    let blinding = *word(arguments, 1)?;
    let buyer_signature = dynamic_argument(arguments, 2)?.to_vec();
    let seller_signature = dynamic_argument(arguments, 3)?.to_vec();
    let token = address_word(word(arguments, 4)?)?;
    Ok(SettleCall {
        terms,
        blinding,
        buyer_signature,
        seller_signature,
        token,
    })
}

/// The decoded fields of one `DealSettled` log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DealSettledFields {
    /// Indexed deal commitment (topic 1).
    pub commitment: [u8; 32],
    /// Indexed deal nullifier (topic 2).
    pub deal_nullifier: [u8; 32],
    /// Indexed buyer, the payer (topic 3).
    pub buyer: [u8; 20],
    /// Payment recipient.
    pub payment_recipient: [u8; 20],
    /// ERC-20 token.
    pub token: [u8; 20],
    /// Amount paid to the recipient.
    pub amount: u128,
    /// Fee paid to the fee recipient.
    pub fee: u128,
}

/// Decodes a `DealSettled` log from its topics and data.
///
/// The amounts are `uint256` in the event but `u128` in the agreement, so a value that does not
/// fit is rejected rather than truncated.
pub fn decode_deal_settled(
    topics: &[[u8; 32]],
    data: &[u8],
) -> Result<DealSettledFields, AbiDecodeError> {
    let [topic0, commitment, deal_nullifier, buyer] = topics else {
        return Err(AbiDecodeError("DealSettled has four topics"));
    };
    if *topic0 != deal_settled_topic() {
        return Err(AbiDecodeError("not a DealSettled log"));
    }
    if data.len() != 128 {
        return Err(AbiDecodeError("DealSettled data is four words"));
    }
    Ok(DealSettledFields {
        commitment: *commitment,
        deal_nullifier: *deal_nullifier,
        buyer: address_word(buyer)?,
        payment_recipient: address_word(word(data, 0)?)?,
        token: address_word(word(data, 1)?)?,
        amount: u128_word(word(data, 2)?)?,
        fee: u128_word(word(data, 3)?)?,
    })
}

fn word(data: &[u8], index: usize) -> Result<&[u8; 32], AbiDecodeError> {
    let start = index
        .checked_mul(32)
        .ok_or(AbiDecodeError("word index overflow"))?;
    data.get(start..start + 32)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(AbiDecodeError("truncated head"))
}

fn usize_word(word: &[u8; 32]) -> Result<usize, AbiDecodeError> {
    if word[..24] != [0u8; 24] {
        return Err(AbiDecodeError("offset or length out of range"));
    }
    let mut low = [0u8; 8];
    low.copy_from_slice(&word[24..]);
    usize::try_from(u64::from_be_bytes(low)).map_err(|_| AbiDecodeError("offset out of range"))
}

fn dynamic_argument(arguments: &[u8], index: usize) -> Result<&[u8], AbiDecodeError> {
    let offset = usize_word(word(arguments, index)?)?;
    let length_end = offset
        .checked_add(32)
        .ok_or(AbiDecodeError("offset overflow"))?;
    let length_word: &[u8; 32] = arguments
        .get(offset..length_end)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(AbiDecodeError("dynamic offset out of bounds"))?;
    let length = usize_word(length_word)?;
    let end = length_end
        .checked_add(length)
        .ok_or(AbiDecodeError("length overflow"))?;
    arguments
        .get(length_end..end)
        .ok_or(AbiDecodeError("dynamic data out of bounds"))
}

fn address_word(word: &[u8; 32]) -> Result<[u8; 20], AbiDecodeError> {
    if word[..12] != [0u8; 12] {
        return Err(AbiDecodeError("address word has dirty high bytes"));
    }
    let mut address = [0u8; 20];
    address.copy_from_slice(&word[12..]);
    Ok(address)
}

fn u128_word(word: &[u8; 32]) -> Result<u128, AbiDecodeError> {
    if word[..16] != [0u8; 16] {
        return Err(AbiDecodeError("amount exceeds u128"));
    }
    let mut low = [0u8; 16];
    low.copy_from_slice(&word[16..]);
    Ok(u128::from_be_bytes(low))
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
