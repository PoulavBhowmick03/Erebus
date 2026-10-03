//! x402 `exact` payments over Permit2 (decision DM8-13).
//!
//! The buyer signs a Permit2 `PermitWitnessTransferFrom` whose spender is the canonical
//! `x402ExactPermit2Proxy` and whose witness fixes the recipient. Erebus binds the payment to
//! one deal by using the deal nullifier as the Permit2 nonce: Permit2 consumes each
//! `(owner, nonce)` once, so `nonceBitmap` becomes this rail's consumed-deal flag. The types make
//! that binding the only way to build an authorization.
//!
//! Every byte here is pinned to the reference x402 SDK by `tests/x402_vectors.rs`.

use erebus_core::commitment::DealNullifier;
use erebus_transport::identity::AuthorizationIdentity;
use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
use sha3::{Digest, Keccak256};

use crate::abi::{left_pad, selector, Encoder};

/// Canonical Permit2, identical on every supported chain.
pub const PERMIT2: [u8; 20] = hex_address(b"000000000022d473030f116ddee9f6b43ac78ba3");

/// Canonical `x402ExactPermit2Proxy`, the only spender x402 `exact` accepts.
pub const EXACT_PERMIT2_PROXY: [u8; 20] = hex_address(b"402085c248eea27d92e8b30b2c58ed07f9e20001");

const TOKEN_PERMISSIONS_TYPE: &[u8] = b"TokenPermissions(address token,uint256 amount)";
const WITNESS_TYPE: &[u8] = b"Witness(address to,uint256 validAfter)";
// EIP-712 appends referenced types in alphabetical order: TokenPermissions, then Witness.
const PERMIT_WITNESS_TYPE: &[u8] = b"PermitWitnessTransferFrom(TokenPermissions permitted,address spender,uint256 nonce,uint256 deadline,Witness witness)TokenPermissions(address token,uint256 amount)Witness(address to,uint256 validAfter)";
const DOMAIN_TYPE: &[u8] = b"EIP712Domain(string name,uint256 chainId,address verifyingContract)";

/// One buyer authorization for one deal. The nonce is the deal nullifier by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DealPermit {
    /// Token the agreement is denominated in.
    pub token: [u8; 20],
    /// Exact agreed amount in base units.
    pub amount: u128,
    /// The deal this authorization pays; also the Permit2 nonce.
    pub deal: DealNullifier,
    /// Permit2 deadline, unix seconds.
    pub deadline: u64,
    /// Recipient fixed by the witness: the seller's `payTo`.
    pub to: [u8; 20],
    /// Earliest settlement time, unix seconds.
    pub valid_after: u64,
}

impl DealPermit {
    /// The EIP-712 digest the buyer signs on `chain_id`.
    #[must_use]
    pub fn digest(&self, chain_id: u64) -> [u8; 32] {
        let mut preimage = Vec::with_capacity(66);
        preimage.extend_from_slice(b"\x19\x01");
        preimage.extend_from_slice(&domain_separator(chain_id));
        preimage.extend_from_slice(&self.struct_hash());
        keccak(&preimage)
    }

    /// `hashStruct(PermitWitnessTransferFrom)` with the proxy as spender.
    #[must_use]
    pub fn struct_hash(&self) -> [u8; 32] {
        let permitted = keccak(
            &[
                keccak(TOKEN_PERMISSIONS_TYPE),
                left_pad(&self.token),
                uint128(self.amount),
            ]
            .concat(),
        );
        let witness = keccak(
            &[
                keccak(WITNESS_TYPE),
                left_pad(&self.to),
                uint64(self.valid_after),
            ]
            .concat(),
        );
        keccak(
            &[
                keccak(PERMIT_WITNESS_TYPE),
                permitted,
                left_pad(&EXACT_PERMIT2_PROXY),
                *self.deal.as_bytes(),
                uint64(self.deadline),
                witness,
            ]
            .concat(),
        )
    }

    /// Signs for `chain_id` as `r || s || v` with `v` in {27, 28}, the form Permit2's
    /// `ecrecover` path accepts.
    #[must_use]
    pub fn sign(&self, chain_id: u64, owner: &AuthorizationIdentity) -> [u8; 65] {
        let mut signature = owner.sign_digest(&self.digest(chain_id));
        signature[64] += 27;
        signature
    }

    /// The address that signed `signature` for `chain_id`, if it is a well-formed low-`s`
    /// signature with `v` in {27, 28}.
    #[must_use]
    pub fn recover_owner(&self, chain_id: u64, signature: &[u8; 65]) -> Option<[u8; 20]> {
        let recovery = RecoveryId::from_byte(signature[64].checked_sub(27)?)?;
        let parsed = Signature::from_slice(&signature[..64]).ok()?;
        if parsed.normalize_s().is_some() {
            return None;
        }
        let key =
            VerifyingKey::recover_from_prehash(&self.digest(chain_id), &parsed, recovery).ok()?;
        let point = key.to_encoded_point(false);
        let hash = keccak(&point.as_bytes()[1..]);
        let mut address = [0u8; 20];
        address.copy_from_slice(&hash[12..]);
        Some(address)
    }

    /// Calldata for `x402ExactPermit2Proxy.settle(permit, owner, witness, signature)`.
    #[must_use]
    pub fn encode_settle_call(&self, owner: &[u8; 20], signature: &[u8; 65]) -> Vec<u8> {
        // Static tuples are inline: permit (4 words), owner, witness (2 words), then bytes.
        let mut encoder = Encoder::new(8);
        encoder.push_static(&left_pad(&self.token));
        encoder.push_static(&uint128(self.amount));
        encoder.push_static(self.deal.as_bytes());
        encoder.push_static(&uint64(self.deadline));
        encoder.push_static(&left_pad(owner));
        encoder.push_static(&left_pad(&self.to));
        encoder.push_static(&uint64(self.valid_after));
        encoder.push_dynamic(signature);
        let mut out =
            selector("settle(((address,uint256),uint256,uint256),address,(address,uint256),bytes)")
                .to_vec();
        out.extend_from_slice(&encoder.finish());
        out
    }
}

/// The Permit2 EIP-712 domain separator for `chain_id` (no version field).
#[must_use]
pub fn domain_separator(chain_id: u64) -> [u8; 32] {
    keccak(
        &[
            keccak(DOMAIN_TYPE),
            keccak(b"Permit2"),
            uint64(chain_id),
            left_pad(&PERMIT2),
        ]
        .concat(),
    )
}

/// Calldata for `Permit2.nonceBitmap(owner, nonce >> 8)`.
#[must_use]
pub fn encode_nonce_bitmap_call(owner: &[u8; 20], deal: &DealNullifier) -> Vec<u8> {
    let mut word_position = [0u8; 32];
    word_position[1..].copy_from_slice(&deal.as_bytes()[..31]);
    let mut out = selector("nonceBitmap(address,uint256)").to_vec();
    out.extend_from_slice(&left_pad(owner));
    out.extend_from_slice(&word_position);
    out
}

/// Whether the bitmap word returned for [`encode_nonce_bitmap_call`] marks `deal` consumed.
#[must_use]
pub fn nonce_consumed(bitmap_word: &[u8; 32], deal: &DealNullifier) -> bool {
    // Bit `nonce & 0xff` of a big-endian uint256 lives in byte 31 - bit / 8.
    let bit = deal.as_bytes()[31];
    bitmap_word[31 - usize::from(bit / 8)] & (1 << (bit % 8)) != 0
}

/// Topic zero of the deployed proxy's `Settled()`. It carries no data: the spec's reference
/// listing shows `x402PermitTransfer(from, to, amount, asset)`, but the canonical deployment
/// (pinned in `tests/fixtures/x402-canonical-runtime.json`) emits this instead. Payment evidence is
/// therefore the token's `Transfer` in the same transaction, the transaction's `settle` input
/// (whose nonce is the deal), and the Permit2 nonce bit.
#[must_use]
pub fn settled_topic() -> [u8; 32] {
    keccak(b"Settled()")
}

fn keccak(bytes: &[u8]) -> [u8; 32] {
    Keccak256::digest(bytes).into()
}

fn uint64(value: u64) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[24..].copy_from_slice(&value.to_be_bytes());
    word
}

fn uint128(value: u128) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[16..].copy_from_slice(&value.to_be_bytes());
    word
}

const fn hex_address(text: &[u8; 40]) -> [u8; 20] {
    const fn nibble(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            _ => panic!("lowercase hex address"),
        }
    }
    let mut out = [0u8; 20];
    let mut i = 0;
    while i < 20 {
        out[i] = nibble(text[2 * i]) << 4 | nibble(text[2 * i + 1]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deal(last: u8) -> DealNullifier {
        let mut bytes = [0u8; 32];
        bytes[31] = last;
        DealNullifier::from_bytes(bytes)
    }

    #[test]
    fn nonce_bit_matches_permit2_layout() {
        // Permit2: bitPos = uint8(nonce); the set bit is 1 << bitPos in the uint256 word.
        for bit in [0u8, 7, 8, 255] {
            let mut word = [0u8; 32];
            word[31 - usize::from(bit / 8)] = 1 << (bit % 8);
            assert!(nonce_consumed(&word, &deal(bit)));
            assert!(!nonce_consumed(&word, &deal(bit.wrapping_add(1))));
        }
    }

    #[test]
    fn word_position_drops_the_low_byte() {
        let mut bytes = [0xabu8; 32];
        bytes[0] = 0x01;
        let call = encode_nonce_bitmap_call(&[0; 20], &DealNullifier::from_bytes(bytes));
        let word = &call[36..68];
        assert_eq!(word[0], 0);
        assert_eq!(word[1], 0x01);
        assert_eq!(&word[2..], &[0xab; 30]);
    }

    #[test]
    fn signatures_round_trip_and_reject_high_s_or_raw_v() {
        let identity = AuthorizationIdentity::from_bytes(&[9; 32]).unwrap();
        let permit = DealPermit {
            token: [1; 20],
            amount: 70,
            deal: deal(3),
            deadline: 10,
            to: [2; 20],
            valid_after: 0,
        };
        let signature = permit.sign(31337, &identity);
        assert_eq!(
            permit.recover_owner(31337, &signature),
            Some(identity.address())
        );
        assert_ne!(
            permit.recover_owner(1, &signature),
            Some(identity.address())
        );
        let mut raw_v = signature;
        raw_v[64] -= 27;
        assert_eq!(permit.recover_owner(31337, &raw_v), None);
        let parsed = Signature::from_slice(&signature[..64]).unwrap();
        let high_s = Signature::from_scalars(parsed.r(), -*parsed.s()).unwrap();
        let mut malleated = signature;
        malleated[..64].copy_from_slice(&high_s.to_bytes());
        assert_eq!(permit.recover_owner(31337, &malleated), None);
    }
}
