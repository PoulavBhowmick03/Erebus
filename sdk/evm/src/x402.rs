//! x402 `exact` payments over Permit2 (decision DM8-13).
//!
//! The buyer signs a Permit2 `PermitWitnessTransferFrom` whose spender is the canonical
//! `x402ExactPermit2Proxy` and whose witness fixes the recipient. Erebus binds the payment to
//! one deal by using the deal nullifier as the Permit2 nonce: Permit2 consumes each
//! `(owner, nonce)` once, so `nonceBitmap` becomes this rail's consumed-deal flag. The types make
//! that binding the only way to build an authorization.
//!
//! Every byte here is pinned to the reference x402 SDK by `tests/x402_vectors.rs`.

use erebus_core::commitment::{deal_nullifier, DealNullifier};
use erebus_core::terms::{AgreementTerms, SettlementMode};
use erebus_transport::identity::AuthorizationIdentity;
use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
use sha3::{Digest, Keccak256};

use crate::abi::{left_pad, selector, Encoder};
use crate::deployment::EvmDeployment;

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

/// Decodes canonical `settle(permit, owner, witness, signature)` calldata into the signed
/// fields. The deployed proxy's `Settled()` event carries no fields, so an independent auditor
/// recovers the permit from the finalized transaction input instead of trusting a copy.
#[must_use]
pub fn decode_settle_call(calldata: &[u8]) -> Option<(DealPermit, [u8; 20], [u8; 65])> {
    let expected =
        selector("settle(((address,uint256),uint256,uint256),address,(address,uint256),bytes)");
    if calldata.len() < 4 + 8 * 32 || calldata[..4] != expected {
        return None;
    }
    let word = |index: usize| -> &[u8] { &calldata[4 + index * 32..4 + (index + 1) * 32] };
    let address = |bytes: &[u8]| -> Option<[u8; 20]> {
        if bytes[..12].iter().any(|byte| *byte != 0) {
            return None;
        }
        bytes[12..].try_into().ok()
    };
    let word_u64 = |bytes: &[u8]| -> Option<u64> {
        if bytes[..24].iter().any(|byte| *byte != 0) {
            return None;
        }
        Some(u64::from_be_bytes(bytes[24..].try_into().ok()?))
    };
    if word(1)[..16].iter().any(|byte| *byte != 0) {
        return None;
    }
    let permit = DealPermit {
        token: address(word(0))?,
        amount: u128::from_be_bytes(word(1)[16..].try_into().ok()?),
        deal: DealNullifier::from_bytes(word(2).try_into().ok()?),
        deadline: word_u64(word(3))?,
        to: address(word(5))?,
        valid_after: word_u64(word(6))?,
    };
    let owner = address(word(4))?;
    let offset = usize::try_from(word_u64(word(7))?).ok()?;
    if offset != 8 * 32 {
        return None;
    }
    let start = 4usize.checked_add(offset)?;
    let signature_start = start.checked_add(32)?;
    let length = usize::try_from(word_u64(calldata.get(start..signature_start)?)?).ok()?;
    if length != 65 {
        return None;
    }
    let signature_end = signature_start.checked_add(65)?;
    let signature: [u8; 65] = calldata
        .get(signature_start..signature_end)?
        .try_into()
        .ok()?;
    if calldata != permit.encode_settle_call(&owner, &signature) {
        return None;
    }
    Some((permit, owner, signature))
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

/// `Transfer(address,address,uint256)`, the token movement a settlement must include.
#[must_use]
pub fn transfer_topic() -> [u8; 32] {
    keccak(b"Transfer(address,address,uint256)")
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

/// One observed x402 `exact` settlement, assembled by a chain observer from finalized evidence.
///
/// A consumed Permit2 nonce is necessary but not sufficient: `finalized`, the transaction target
/// and calldata, and the token transfer must all match the same authorized permit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct X402ExactEvidence {
    /// Whether the containing transaction is at or below the finalized anchor.
    pub finalized: bool,
    /// Transaction recipient; must be the canonical exact proxy.
    pub transaction_to: [u8; 20],
    /// Transaction input; must equal the expected `settle` calldata byte for byte.
    pub calldata: Vec<u8>,
    /// Token `Transfer.from`.
    pub transfer_from: [u8; 20],
    /// Token `Transfer.to`.
    pub transfer_to: [u8; 20],
    /// Token `Transfer` contract.
    pub transfer_token: [u8; 20],
    /// Token `Transfer.value`.
    pub transfer_amount: u128,
    /// The `Permit2.nonceBitmap(owner, deal >> 8)` word read at the same finalized anchor.
    pub nonce_bit: [u8; 32],
}

/// An x402 `exact` settlement did not match the authorized agreement or its evidence.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum X402VerificationError {
    /// The agreement terms were invalid.
    #[error(transparent)]
    Terms(#[from] erebus_core::terms::TermsError),
    /// The deal identity could not be derived.
    #[error(transparent)]
    Commitment(#[from] erebus_core::commitment::CommitmentError),
    /// This rail is only for public-bound settlement.
    #[error("x402 exact is a public-bound rail")]
    UnsupportedMode,
    /// x402 exact pays one amount to one recipient; a fee would need a second rail.
    #[error("x402 exact cannot cover a non-zero agreement fee")]
    NonZeroFee,
    /// A permit field disagreed with the authorized agreement.
    #[error("permit field `{0}` does not match the authorized agreement")]
    PermitField(&'static str),
    /// The signature was not produced by the agreement's buyer key.
    #[error("permit signature was not produced by the authorized buyer key")]
    OwnerMismatch,
    /// The transaction is not final.
    #[error("x402 settlement is not finalized")]
    NotFinalized,
    /// The transaction did not call the canonical exact proxy.
    #[error("x402 transaction did not call the canonical exact proxy")]
    WrongTarget,
    /// The calldata was not the expected settle call.
    #[error("x402 transaction calldata does not match the authorized permit")]
    WrongCalldata,
    /// The Permit2 nonce bit was not consumed.
    #[error("x402 Permit2 nonce for the deal is not consumed")]
    NonceNotConsumed,
    /// The token transfer did not pay the authorized amount to the authorized recipient.
    #[error("x402 token transfer does not match the authorized payment")]
    TransferMismatch,
}

/// Validates every permit field against the authorized agreement. Call this before signing and
/// again before trusting an observed settlement.
///
/// The deal nullifier is the Permit2 nonce, so one deal can consume at most one permit; a
/// non-zero agreement fee is rejected because this rail cannot pay it.
pub fn validate_permit_fields(
    deployment: &EvmDeployment,
    terms: &AgreementTerms,
    permit: &DealPermit,
) -> Result<(), X402VerificationError> {
    terms.validate()?;
    if terms.settlement_mode != SettlementMode::PublicBound {
        return Err(X402VerificationError::UnsupportedMode);
    }
    if terms.fee_policy.fee.get() != 0 {
        return Err(X402VerificationError::NonZeroFee);
    }
    if deal_nullifier(terms)? != permit.deal {
        return Err(X402VerificationError::PermitField("deal"));
    }
    let token = deployment
        .token_address(&terms.asset)
        .map_err(|_| X402VerificationError::PermitField("asset"))?;
    if permit.token != token {
        return Err(X402VerificationError::PermitField("token"));
    }
    if permit.amount != terms.amount.get() {
        return Err(X402VerificationError::PermitField("amount"));
    }
    let to: [u8; 20] = terms
        .payment_recipient
        .as_bytes()
        .try_into()
        .map_err(|_| X402VerificationError::PermitField("payment_recipient"))?;
    if permit.to != to {
        return Err(X402VerificationError::PermitField("to"));
    }
    if permit.deadline == 0
        || permit.deadline > terms.expiry
        || permit.valid_after >= permit.deadline
    {
        return Err(X402VerificationError::PermitField("deadline"));
    }
    Ok(())
}

/// Verifies the buyer signature over the exact permit fields. Call after
/// [`validate_permit_fields`] and again on every observed settlement.
pub fn verify_permit_signature(
    deployment: &EvmDeployment,
    terms: &AgreementTerms,
    permit: &DealPermit,
    signature: &[u8; 65],
) -> Result<(), X402VerificationError> {
    let owner: [u8; 20] = terms
        .buyer_authorization_key
        .as_bytes()
        .try_into()
        .map_err(|_| X402VerificationError::PermitField("buyer_authorization_key"))?;
    if permit.recover_owner(deployment.chain_id, signature) != Some(owner) {
        return Err(X402VerificationError::OwnerMismatch);
    }
    Ok(())
}

/// Validates the unsigned fields, then the signature. Kept as the single pre-signing entry
/// point; callers that need the split can call the two functions directly.
pub fn validate_permit_against_agreement(
    deployment: &EvmDeployment,
    terms: &AgreementTerms,
    permit: &DealPermit,
    signature: &[u8; 65],
) -> Result<(), X402VerificationError> {
    validate_permit_fields(deployment, terms, permit)?;
    verify_permit_signature(deployment, terms, permit, signature)
}

/// Verifies a finalized x402 `exact` settlement against the authorized agreement.
///
/// The nonce bit alone is not payment evidence. This requires a finalized transaction that calls
/// the canonical exact proxy with exactly the expected calldata, a consumed Permit2 nonce for
/// the deal, and a token `Transfer` of the authorized amount to the authorized recipient.
pub fn verify_x402_exact(
    deployment: &EvmDeployment,
    terms: &AgreementTerms,
    permit: &DealPermit,
    signature: &[u8; 65],
    evidence: &X402ExactEvidence,
) -> Result<(), X402VerificationError> {
    validate_permit_against_agreement(deployment, terms, permit, signature)?;
    if !evidence.finalized {
        return Err(X402VerificationError::NotFinalized);
    }
    if evidence.transaction_to != EXACT_PERMIT2_PROXY {
        return Err(X402VerificationError::WrongTarget);
    }
    let owner: [u8; 20] = terms
        .buyer_authorization_key
        .as_bytes()
        .try_into()
        .map_err(|_| X402VerificationError::PermitField("buyer_authorization_key"))?;
    if evidence.calldata != permit.encode_settle_call(&owner, signature).as_slice() {
        return Err(X402VerificationError::WrongCalldata);
    }
    if !nonce_consumed(&evidence.nonce_bit, &permit.deal) {
        return Err(X402VerificationError::NonceNotConsumed);
    }
    if evidence.transfer_from != owner
        || evidence.transfer_to != permit.to
        || evidence.transfer_token != permit.token
        || evidence.transfer_amount != permit.amount
    {
        return Err(X402VerificationError::TransferMismatch);
    }
    Ok(())
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
    fn settle_calldata_round_trips_through_the_auditor_decoder() {
        let permit = DealPermit {
            token: [0x11; 20],
            amount: 70,
            deal: deal(9),
            deadline: 1_800_000_000,
            to: [0x22; 20],
            valid_after: 1_700_000_000,
        };
        let owner = [0x33; 20];
        let signature = [0x44; 65];
        let calldata = permit.encode_settle_call(&owner, &signature);
        assert_eq!(
            decode_settle_call(&calldata),
            Some((permit, owner, signature))
        );
        let mut tampered = calldata.clone();
        tampered[4 + 32 + 31] ^= 1;
        assert_eq!(
            decode_settle_call(&tampered),
            Some((
                DealPermit {
                    amount: 71,
                    ..permit
                },
                owner,
                signature
            ))
        );
        let mut noncanonical = calldata.clone();
        noncanonical[4 + 32] = 1;
        assert_eq!(decode_settle_call(&noncanonical), None);
        assert_eq!(decode_settle_call(&calldata[..36]), None);
        let mut oversized_offset = calldata.clone();
        oversized_offset[4 + 7 * 32 + 24..4 + 8 * 32]
            .copy_from_slice(&(u64::MAX - 31).to_be_bytes());
        assert_eq!(decode_settle_call(&oversized_offset), None);
        let mut trailing = calldata.clone();
        trailing.push(0);
        assert_eq!(decode_settle_call(&trailing), None);
        let mut dirty_padding = calldata.clone();
        *dirty_padding.last_mut().unwrap() = 1;
        assert_eq!(decode_settle_call(&dirty_padding), None);
        let mut wrong_selector = calldata;
        wrong_selector[0] ^= 1;
        assert_eq!(decode_settle_call(&wrong_selector), None);
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

#[cfg(test)]
mod verification_tests {
    use super::*;
    use erebus_core::domain::DeploymentDomain;
    use erebus_core::ids::{AddressBytes, AssetId, BaseUnits, ChainNamespace, KeyBytes};
    use erebus_core::service::ServiceRecord;
    use erebus_core::terms::{FeePolicy, Guarantee, GuaranteeSet, CURRENT_PROTOCOL_VERSION};

    const TOKEN: [u8; 20] = [0xaa; 20];
    const SELLER: [u8; 20] = [0x22; 20];
    const AMOUNT: u128 = 70;
    const CHAIN_ID: u64 = 31337;

    struct Fixture {
        terms: AgreementTerms,
        deployment: EvmDeployment,
        permit: DealPermit,
        signature: [u8; 65],
        buyer: [u8; 20],
    }

    fn fixture() -> Fixture {
        let identity = AuthorizationIdentity::from_bytes(&[9; 32]).expect("key");
        let buyer = identity.address();
        let namespace = ChainNamespace::new("eip155", "31337").expect("namespace");
        let terms = AgreementTerms {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            suite_id: 1,
            domain: DeploymentDomain {
                namespace: namespace.clone(),
                settlement_contract: Some(AddressBytes::new(vec![0x55; 20]).expect("address")),
                pool: None,
                verifier_version: 1,
            },
            deal_id: [7; 16],
            revision: 1,
            transcript_root: [0; 32],
            buyer_authorization_key: KeyBytes::new(buyer.to_vec()).expect("key"),
            seller_authorization_key: KeyBytes::new(SELLER.to_vec()).expect("key"),
            payment_recipient: KeyBytes::new(SELLER.to_vec()).expect("key"),
            asset: AssetId::new(namespace, "erc20", &format!("0x{}", hex::encode(TOKEN)))
                .expect("asset"),
            amount: BaseUnits::new(AMOUNT),
            expiry: 10_000,
            fee_policy: FeePolicy::none(),
            settlement_mode: SettlementMode::PublicBound,
            required_guarantees: GuaranteeSet::from_guarantee(Guarantee::AgreementBoundSettlement),
            settlement_nonce: [0x77; 32],
            service: ServiceRecord {
                resource: "x".to_owned(),
                quantity: BaseUnits::new(1),
                unit: "u".to_owned(),
                access_recipient: KeyBytes::new(buyer.to_vec()).expect("key"),
                delivery_deadline: 10_000,
                fulfillment_method: "http-access".to_owned(),
                fulfillment_digest: [0; 32],
            },
        };
        let permit = DealPermit {
            token: TOKEN,
            amount: AMOUNT,
            deal: deal_nullifier(&terms).expect("nullifier"),
            deadline: 9_000,
            to: SELLER,
            valid_after: 0,
        };
        let signature = permit.sign(CHAIN_ID, &identity);
        let deployment = EvmDeployment::new(
            ChainNamespace::new("eip155", "31337").expect("namespace"),
            [0x55; 20],
            1,
            "http://127.0.0.1:1",
        )
        .expect("deployment");
        Fixture {
            terms,
            deployment,
            permit,
            signature,
            buyer,
        }
    }

    fn nonce_bit(deal: &DealNullifier) -> [u8; 32] {
        let mut word = [0u8; 32];
        let bit = deal.as_bytes()[31];
        word[31 - usize::from(bit / 8)] |= 1 << (bit % 8);
        word
    }

    fn evidence(fixture: &Fixture, calldata: &[u8]) -> X402ExactEvidence {
        X402ExactEvidence {
            finalized: true,
            transaction_to: EXACT_PERMIT2_PROXY,
            calldata: calldata.to_vec(),
            transfer_from: fixture.buyer,
            transfer_to: SELLER,
            transfer_token: TOKEN,
            transfer_amount: AMOUNT,
            nonce_bit: nonce_bit(&fixture.permit.deal),
        }
    }

    #[test]
    fn a_finalized_matching_settlement_verifies() {
        let f = fixture();
        let call = f.permit.encode_settle_call(&f.buyer, &f.signature);
        validate_permit_against_agreement(&f.deployment, &f.terms, &f.permit, &f.signature)
            .expect("fields match");
        verify_x402_exact(
            &f.deployment,
            &f.terms,
            &f.permit,
            &f.signature,
            &evidence(&f, &call),
        )
        .expect("finalized evidence matches");
    }

    #[test]
    fn every_mismatch_is_rejected() {
        let f = fixture();
        let call = f.permit.encode_settle_call(&f.buyer, &f.signature);
        let check = |mutate: &dyn Fn(&mut Fixture, &mut X402ExactEvidence)| {
            let mut fixture = fixture();
            let mut evidence = evidence(&fixture, &call);
            mutate(&mut fixture, &mut evidence);
            let result = verify_x402_exact(
                &fixture.deployment,
                &fixture.terms,
                &fixture.permit,
                &fixture.signature,
                &evidence,
            );
            assert!(result.is_err(), "mutation was accepted");
            result
        };

        // Not final.
        assert_eq!(
            check(&|_, e| e.finalized = false),
            Err(X402VerificationError::NotFinalized)
        );
        // Wrong transaction target.
        assert_eq!(
            check(&|_, e| e.transaction_to = [0x99; 20]),
            Err(X402VerificationError::WrongTarget)
        );
        // Nonce not consumed.
        assert_eq!(
            check(&|_, e| e.nonce_bit = [0u8; 32]),
            Err(X402VerificationError::NonceNotConsumed)
        );
        // Transfer to the wrong recipient.
        assert_eq!(
            check(&|_, e| e.transfer_to = [0x99; 20]),
            Err(X402VerificationError::TransferMismatch)
        );
        // Transfer of the wrong amount.
        assert_eq!(
            check(&|_, e| e.transfer_amount = AMOUNT + 1),
            Err(X402VerificationError::TransferMismatch)
        );
        // Permit amount changed after signing.
        assert_eq!(
            check(&|fixture, _| fixture.permit.amount = AMOUNT + 1),
            Err(X402VerificationError::PermitField("amount"))
        );
        // Recipient changed after signing.
        assert_eq!(
            check(&|fixture, _| fixture.permit.to = [0x99; 20]),
            Err(X402VerificationError::PermitField("to"))
        );
        // Permit signed by a different key.
        let stranger = AuthorizationIdentity::from_bytes(&[8; 32]).expect("key");
        assert_eq!(
            check(&|fixture, _| fixture.signature = fixture.permit.sign(CHAIN_ID, &stranger)),
            Err(X402VerificationError::OwnerMismatch)
        );

        // Wrong calldata: a settle call for a different owner, verified inline because the
        // evidence borrows the alternate calldata for the duration of the call.
        let fixture = fixture();
        let other = fixture
            .permit
            .encode_settle_call(&[0x99; 20], &fixture.signature);
        let other_evidence = evidence(&fixture, &other);
        assert_eq!(
            verify_x402_exact(
                &fixture.deployment,
                &fixture.terms,
                &fixture.permit,
                &fixture.signature,
                &other_evidence,
            ),
            Err(X402VerificationError::WrongCalldata)
        );
    }

    #[test]
    fn unsigned_fields_validate_before_a_signature_exists() {
        let f = fixture();
        validate_permit_fields(&f.deployment, &f.terms, &f.permit).expect("fields match");
        let mut wrong = f.permit;
        wrong.amount = AMOUNT + 1;
        assert_eq!(
            validate_permit_fields(&f.deployment, &f.terms, &wrong),
            Err(X402VerificationError::PermitField("amount"))
        );
        let stranger = AuthorizationIdentity::from_bytes(&[8; 32]).expect("key");
        let foreign = f.permit.sign(CHAIN_ID, &stranger);
        assert_eq!(
            verify_permit_signature(&f.deployment, &f.terms, &f.permit, &foreign),
            Err(X402VerificationError::OwnerMismatch)
        );
    }

    #[test]
    fn a_nonzero_fee_is_not_a_valid_x402_exact_deal() {
        let mut f = fixture();
        f.terms.fee_policy = FeePolicy {
            fee: BaseUnits::new(1),
            recipient: Some(KeyBytes::new(SELLER.to_vec()).expect("key")),
        };
        assert_eq!(
            validate_permit_against_agreement(&f.deployment, &f.terms, &f.permit, &f.signature),
            Err(X402VerificationError::NonZeroFee)
        );
    }
}
