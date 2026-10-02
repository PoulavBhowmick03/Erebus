//! Local signing of settlement transactions, with every field chosen by the caller.
//!
//! [`build_signed`] is a pure function: prepared settlement, key, nonce, fees, and gas limit in;
//! raw EIP-1559 bytes and their hash out. Nothing is filled from the network, so the caller can
//! persist the exact bytes (and the hash they will be mined under) before broadcasting them, and
//! a fee bump is a second call with the same nonce. Signing is RFC 6979 deterministic, so the
//! same inputs always produce the same bytes.

use alloy::consensus::{SignableTransaction, TxEip1559, TxEnvelope};
use alloy::eips::eip2718::{Decodable2718, Encodable2718};
use alloy::primitives::{keccak256, Address, Bytes, TxKind, U256};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::SignerSync;

use erebus_core::settlement::PreparedSettlement;

use crate::backend::validate_prepared;
use crate::deployment::{EvmDeployment, ADDRESS_BYTES};
use crate::error::EvmError;

/// Minimum fee increase, in percent, for a same-nonce replacement.
///
/// This is a local policy, not a verified Monad acceptance rule. A node can reject a
/// replacement that meets this floor; the coordinator must handle that rejection.
pub const REPLACEMENT_BUMP_PERCENT: u128 = 10;

/// A gas-paying transaction key, independent of the deal authorization role.
pub struct TransactionKey(PrivateKeySigner);

impl TransactionKey {
    /// Wraps a secp256k1 secret key.
    pub fn from_bytes(key: &[u8; 32]) -> Result<Self, EvmError> {
        PrivateKeySigner::from_slice(key)
            .map(Self)
            .map_err(|_| EvmError::InvalidSigningKey)
    }

    /// The address transactions signed with this key are sent from.
    #[must_use]
    pub fn address(&self) -> [u8; ADDRESS_BYTES] {
        self.0.address().0 .0
    }
}

impl std::fmt::Debug for TransactionKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TransactionKey")
            .field("address", &format!("{:#x}", self.0.address()))
            .finish_non_exhaustive()
    }
}

/// EIP-1559 fee caps, with the priority fee never above the max fee.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Eip1559Fees {
    max_fee_per_gas: u128,
    max_priority_fee_per_gas: u128,
}

impl Eip1559Fees {
    /// Builds fee caps. This adapter rejects zero max fees and priority above max fee.
    pub fn new(max_fee_per_gas: u128, max_priority_fee_per_gas: u128) -> Result<Self, EvmError> {
        if max_fee_per_gas == 0 {
            return Err(EvmError::InvalidFees("max fee per gas is zero"));
        }
        if max_priority_fee_per_gas > max_fee_per_gas {
            return Err(EvmError::InvalidFees("priority fee exceeds max fee"));
        }
        Ok(Self {
            max_fee_per_gas,
            max_priority_fee_per_gas,
        })
    }

    /// The max fee per gas, in wei.
    #[must_use]
    pub const fn max_fee_per_gas(&self) -> u128 {
        self.max_fee_per_gas
    }

    /// The max priority fee per gas, in wei.
    #[must_use]
    pub const fn max_priority_fee_per_gas(&self) -> u128 {
        self.max_priority_fee_per_gas
    }

    /// The lowest fees a same-nonce replacement of a transaction with these fees should carry:
    /// both caps raised by [`REPLACEMENT_BUMP_PERCENT`], rounded up.
    pub fn replacement_floor(&self) -> Result<Self, EvmError> {
        let bump = |value: u128| -> Result<u128, EvmError> {
            let increase = (value / 100) * REPLACEMENT_BUMP_PERCENT
                + ((value % 100) * REPLACEMENT_BUMP_PERCENT).div_ceil(100);
            value
                .checked_add(increase)
                .ok_or(EvmError::InvalidFees("replacement fee overflows"))
        };
        Self::new(
            bump(self.max_fee_per_gas)?,
            bump(self.max_priority_fee_per_gas)?,
        )
    }

    /// Reports whether these fees meet the replacement floor of `previous`.
    #[must_use]
    pub fn can_replace(&self, previous: &Self) -> bool {
        previous.replacement_floor().is_ok_and(|floor| {
            self.max_fee_per_gas >= floor.max_fee_per_gas
                && self.max_priority_fee_per_gas >= floor.max_priority_fee_per_gas
        })
    }
}

/// The caller-chosen fields of one transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransactionParams {
    /// Account nonce. The caller allocates it; nothing here reads it from the chain.
    pub nonce: u64,
    /// Fee caps.
    pub fees: Eip1559Fees,
    /// Gas limit.
    pub gas_limit: u64,
}

/// A signed EIP-1559 transaction and the fields it commits to.
///
/// [`Self::from_raw`] derives the transaction fields from the raw bytes. The caller must also
/// persist the expected settlement intent and check it with [`Self::validate_against`].
#[derive(Clone, PartialEq, Eq)]
pub struct SignedTransaction {
    raw: Vec<u8>,
    hash: [u8; 32],
    sender: [u8; ADDRESS_BYTES],
    chain_id: u64,
    params: TransactionParams,
    to: [u8; ADDRESS_BYTES],
    input: Vec<u8>,
}

impl std::fmt::Debug for SignedTransaction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SignedTransaction")
            .field("hash", &self.hash)
            .field("chain_id", &self.chain_id)
            .field("params", &self.params)
            .finish_non_exhaustive()
    }
}

impl SignedTransaction {
    /// Decodes raw bytes produced by [`build_signed`], for example after a restart.
    ///
    /// Accepts only a canonically encoded, signed EIP-1559 contract call with zero value. The
    /// hash is recomputed and the sender recovered from the signature; neither is trusted from
    /// storage. Rejects nonempty access lists, zero gas, and high-s signatures. This only
    /// decodes the transaction; use [`Self::validate_against`] before rebroadcast.
    pub fn from_raw(raw: &[u8]) -> Result<Self, EvmError> {
        let envelope = TxEnvelope::decode_2718_exact(raw)
            .map_err(|error| EvmError::TransactionEncoding(error.to_string()))?;
        if envelope.encoded_2718() != raw {
            return Err(EvmError::TransactionEncoding(
                "encoding is not canonical".to_owned(),
            ));
        }
        let TxEnvelope::Eip1559(signed) = envelope else {
            return Err(EvmError::TransactionEncoding(
                "not an EIP-1559 transaction".to_owned(),
            ));
        };
        let transaction = signed.tx();
        if transaction.gas_limit == 0
            || !transaction.access_list.0.is_empty()
            || signed.signature().normalize_s().is_some()
        {
            return Err(EvmError::TransactionEncoding(
                "zero gas, nonempty access list, or high-s signature".to_owned(),
            ));
        }
        let TxKind::Call(to) = transaction.to else {
            return Err(EvmError::TransactionEncoding(
                "contract creation, not a call".to_owned(),
            ));
        };
        if !transaction.value.is_zero() {
            return Err(EvmError::TransactionEncoding(
                "settlement transactions carry no value".to_owned(),
            ));
        }
        let sender = signed
            .signature()
            .recover_address_from_prehash(&transaction.signature_hash())
            .map_err(|error| EvmError::TransactionEncoding(error.to_string()))?;
        let fees = Eip1559Fees::new(
            transaction.max_fee_per_gas,
            transaction.max_priority_fee_per_gas,
        )?;
        Ok(Self {
            raw: raw.to_vec(),
            hash: signed.hash().0,
            sender: sender.0 .0,
            chain_id: transaction.chain_id,
            params: TransactionParams {
                nonce: transaction.nonce,
                fees,
                gas_limit: transaction.gas_limit,
            },
            to: to.0 .0,
            input: transaction.input.to_vec(),
        })
    }

    /// The EIP-2718 encoded signed transaction, as sent to `eth_sendRawTransaction`.
    #[must_use]
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }

    /// The transaction hash, `keccak256(raw)`: the hash it is mined under.
    #[must_use]
    pub const fn hash(&self) -> [u8; 32] {
        self.hash
    }

    /// The signing address.
    #[must_use]
    pub const fn sender(&self) -> [u8; ADDRESS_BYTES] {
        self.sender
    }

    /// The EIP-155 chain id the signature is bound to.
    #[must_use]
    pub const fn chain_id(&self) -> u64 {
        self.chain_id
    }

    /// Nonce, fees, and gas limit.
    #[must_use]
    pub const fn params(&self) -> TransactionParams {
        self.params
    }

    /// The call destination.
    #[must_use]
    pub const fn to(&self) -> [u8; ADDRESS_BYTES] {
        self.to
    }

    /// The calldata.
    #[must_use]
    pub fn input(&self) -> &[u8] {
        &self.input
    }

    /// Checks decoded bytes against the trusted journal intent before rebroadcast.
    /// Decoding alone establishes no relationship to a particular settlement.
    pub fn validate_against(
        &self,
        deployment: &EvmDeployment,
        prepared: &PreparedSettlement,
        sender: [u8; ADDRESS_BYTES],
        params: TransactionParams,
    ) -> Result<(), EvmError> {
        let calldata = settlement_calldata(deployment, prepared)?;
        if self.chain_id != deployment.chain_id
            || self.to != deployment.settlement_contract
            || self.sender != sender
            || self.params != params
            || self.input != calldata
        {
            return Err(EvmError::SignedIntentMismatch);
        }
        Ok(())
    }

    /// Checks a zero-value call against a backend-validated target and calldata.
    pub fn validate_call(
        &self,
        chain_id: u64,
        to: [u8; ADDRESS_BYTES],
        input: &[u8],
        sender: [u8; ADDRESS_BYTES],
        params: TransactionParams,
    ) -> Result<(), EvmError> {
        if self.chain_id != chain_id
            || self.to != to
            || self.input != input
            || self.sender != sender
            || self.params != params
        {
            return Err(EvmError::SignedIntentMismatch);
        }
        Ok(())
    }

    /// Requires the same payment, signer, nonce, and gas limit, with increased fee caps.
    /// This local policy does not guarantee that a node will accept the replacement.
    pub fn validate_replacement(&self, previous: &Self) -> Result<(), EvmError> {
        if self.chain_id != previous.chain_id
            || self.sender != previous.sender
            || self.to != previous.to
            || self.input != previous.input
            || self.params.nonce != previous.params.nonce
            || self.params.gas_limit != previous.params.gas_limit
            || !self.params.fees.can_replace(&previous.params.fees)
        {
            return Err(EvmError::SignedIntentMismatch);
        }
        Ok(())
    }
}

/// The `settle` calldata for a prepared settlement, after re-deriving the commitment and
/// nullifier from its evidence (DM3-2). A coordinator can compare it with
/// [`SignedTransaction::input`] of reloaded bytes.
pub fn settlement_calldata(
    deployment: &EvmDeployment,
    prepared: &PreparedSettlement,
) -> Result<Vec<u8>, EvmError> {
    Ok(validate_prepared(deployment, prepared)?.calldata)
}

/// Signs a settlement transaction locally, without any network I/O.
///
/// The prepared settlement is validated against the deployment exactly as the M3 path does
/// before submission, the call targets the deployment's settlement contract, and the signature
/// is bound to the deployment's chain id. The result is deterministic in its inputs.
pub fn build_signed(
    deployment: &EvmDeployment,
    prepared: &PreparedSettlement,
    key: &TransactionKey,
    params: TransactionParams,
) -> Result<SignedTransaction, EvmError> {
    let calldata = settlement_calldata(deployment, prepared)?;
    sign_call(
        key,
        deployment.chain_id,
        deployment.settlement_contract,
        calldata,
        params,
    )
}

/// Signs a backend-validated zero-value EVM call without network I/O.
/// The caller must bind `to` and `input` to its accepted agreement first.
pub fn build_signed_call(
    key: &TransactionKey,
    chain_id: u64,
    to: [u8; ADDRESS_BYTES],
    input: Vec<u8>,
    params: TransactionParams,
) -> Result<SignedTransaction, EvmError> {
    sign_call(key, chain_id, to, input, params)
}

/// Signs one zero-value EIP-1559 call.
fn sign_call(
    key: &TransactionKey,
    chain_id: u64,
    to: [u8; ADDRESS_BYTES],
    input: Vec<u8>,
    params: TransactionParams,
) -> Result<SignedTransaction, EvmError> {
    if params.gas_limit == 0 {
        return Err(EvmError::TransactionEncoding(
            "gas limit is zero".to_owned(),
        ));
    }
    let transaction = TxEip1559 {
        chain_id,
        nonce: params.nonce,
        gas_limit: params.gas_limit,
        max_fee_per_gas: params.fees.max_fee_per_gas,
        max_priority_fee_per_gas: params.fees.max_priority_fee_per_gas,
        to: TxKind::Call(Address::from(to)),
        value: U256::ZERO,
        access_list: Default::default(),
        input: Bytes::from(input.clone()),
    };
    let signature = key
        .0
        .sign_hash_sync(&transaction.signature_hash())
        .map_err(|_| EvmError::InvalidSigningKey)?;
    let signed = transaction.into_signed(signature);
    let hash = signed.hash().0;
    let raw = TxEnvelope::from(signed).encoded_2718();
    if keccak256(&raw).0 != hash {
        return Err(EvmError::TransactionEncoding(
            "transaction hash is not the hash of its encoding".to_owned(),
        ));
    }
    Ok(SignedTransaction {
        raw,
        hash,
        sender: key.address(),
        chain_id,
        params,
        to,
        input,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Anvil account 1. Public test-chain material, not a secret.
    const ANVIL_KEY_1: &str = "59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";

    fn key() -> TransactionKey {
        let bytes: [u8; 32] = hex::decode(ANVIL_KEY_1)
            .expect("hex")
            .try_into()
            .expect("32 bytes");
        TransactionKey::from_bytes(&bytes).expect("key")
    }

    fn kat_params() -> TransactionParams {
        TransactionParams {
            nonce: 7,
            fees: Eip1559Fees::new(3_000_000_000, 1_000_000_000).expect("fees"),
            gas_limit: 300_000,
        }
    }

    fn kat_target() -> [u8; ADDRESS_BYTES] {
        hex::decode("5fbdb2315678afecb367f032d93f642f64180aa3")
            .expect("hex")
            .try_into()
            .expect("20 bytes")
    }

    fn kat_input() -> Vec<u8> {
        let mut input = vec![0xdf, 0xd8, 0xbf, 0x75];
        input.extend_from_slice(&[0xab; 32]);
        input
    }

    /// Produced independently by Foundry, offline:
    /// `cast mktx --private-key <anvil key 1> --chain 31337 --nonce 7 --gas-limit 300000
    ///  --gas-price 3000000000 --priority-gas-price 1000000000
    ///  0x5fbdb2315678afecb367f032d93f642f64180aa3 0xdfd8bf75abab...ab`
    const CAST_RAW: &str = "02f891827a6907843b9aca0084b2d05e00830493e0945fbdb2315678afecb367f032d93f642f64180aa380a4dfd8bf75ababababababababababababababababababababababababababababababababc080a09c74792d1c291d7105fe3a52101d94906f8efe1514ddd37aba17b359f11deff0a01bb8838d500c3c1200a9606bcdcea05c3b5d5b14870a3fb5495a3c83b9a2c2ed";
    /// `cast keccak <CAST_RAW>`.
    const CAST_HASH: &str = "8adda5c0344b2bdffe6045f5e415300eeee88be9e86f4c9a3b1eb06d64122a13";

    #[test]
    fn local_signing_matches_foundry_byte_for_byte() {
        let signed =
            sign_call(&key(), 31_337, kat_target(), kat_input(), kat_params()).expect("sign");
        assert_eq!(hex::encode(signed.raw()), CAST_RAW);
        assert_eq!(hex::encode(signed.hash()), CAST_HASH);
        assert_eq!(
            hex::encode(signed.sender()),
            "70997970c51812dc3a010c7d01b50e0d17dc79c8"
        );
    }

    #[test]
    fn signing_is_deterministic_and_round_trips() {
        let first =
            sign_call(&key(), 31_337, kat_target(), kat_input(), kat_params()).expect("sign");
        let second =
            sign_call(&key(), 31_337, kat_target(), kat_input(), kat_params()).expect("sign");
        assert_eq!(first, second);
        assert_eq!(
            SignedTransaction::from_raw(first.raw()).expect("decode"),
            first
        );

        let bumped = TransactionParams {
            fees: kat_params().fees.replacement_floor().expect("bump"),
            ..kat_params()
        };
        let replacement =
            sign_call(&key(), 31_337, kat_target(), kat_input(), bumped).expect("sign");
        assert_ne!(replacement.hash(), first.hash());
        assert_eq!(replacement.params().nonce, first.params().nonce);
        assert_eq!(replacement.input(), first.input());
    }

    #[test]
    fn raw_decoding_rejects_non_canonical_and_foreign_shapes() {
        let raw = hex::decode(CAST_RAW).expect("hex");
        let mut trailing = raw.clone();
        trailing.push(0);
        assert!(SignedTransaction::from_raw(&trailing).is_err());
        let mut tampered = raw.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 1;
        // A flipped signature byte either fails recovery or recovers a different sender; it
        // never decodes to the original transaction.
        if let Ok(decoded) = SignedTransaction::from_raw(&tampered) {
            assert_ne!(decoded.sender(), key().address());
        }
        assert!(SignedTransaction::from_raw(&raw[1..]).is_err());
    }

    #[test]
    fn fees_reject_impossible_caps_and_bump_by_ten_percent() {
        assert!(Eip1559Fees::new(0, 0).is_err());
        assert!(Eip1559Fees::new(10, 11).is_err());
        let fees = Eip1559Fees::new(1_000, 101).expect("fees");
        let floor = fees.replacement_floor().expect("floor");
        assert_eq!(floor.max_fee_per_gas(), 1_100);
        // 101 * 10% = 10.1, rounded up to 11.
        assert_eq!(floor.max_priority_fee_per_gas(), 112);
        assert!(floor.can_replace(&fees));
        assert!(!fees.can_replace(&fees));
        let short = Eip1559Fees::new(1_100, 111).expect("fees");
        assert!(!short.can_replace(&fees));
        assert!(Eip1559Fees::new(u128::MAX, 1)
            .expect("fees")
            .replacement_floor()
            .is_err());
    }

    #[test]
    fn the_key_debug_output_does_not_contain_the_secret() {
        let rendered = format!("{:?}", key());
        assert!(!rendered.contains(ANVIL_KEY_1));
        assert!(rendered.contains("70997970c51812dc3a010c7d01b50e0d17dc79c8"));
    }

    #[test]
    fn replacement_rejects_changed_intent_even_with_sufficient_fees() {
        let first = sign_call(&key(), 31_337, kat_target(), kat_input(), kat_params()).unwrap();
        let bumped = TransactionParams {
            fees: kat_params().fees.replacement_floor().unwrap(),
            ..kat_params()
        };
        let replacement = sign_call(&key(), 31_337, kat_target(), kat_input(), bumped).unwrap();
        replacement.validate_replacement(&first).unwrap();
        assert!(first.validate_replacement(&first).is_err());
        for changed in [
            sign_call(&key(), 1, kat_target(), kat_input(), bumped).unwrap(),
            sign_call(&key(), 31_337, [1; 20], kat_input(), bumped).unwrap(),
            sign_call(&key(), 31_337, kat_target(), vec![1], bumped).unwrap(),
            sign_call(
                &key(),
                31_337,
                kat_target(),
                kat_input(),
                TransactionParams { nonce: 8, ..bumped },
            )
            .unwrap(),
            sign_call(
                &key(),
                31_337,
                kat_target(),
                kat_input(),
                TransactionParams {
                    gas_limit: 400_000,
                    ..bumped
                },
            )
            .unwrap(),
            sign_call(
                &TransactionKey::from_bytes(&[1; 32]).unwrap(),
                31_337,
                kat_target(),
                kat_input(),
                bumped,
            )
            .unwrap(),
        ] {
            assert!(changed.validate_replacement(&first).is_err());
        }
    }

    #[test]
    fn bounds_and_debug_are_safe() {
        let params = TransactionParams {
            gas_limit: 0,
            ..kat_params()
        };
        assert!(sign_call(&key(), 31_337, kat_target(), kat_input(), params).is_err());
        let large = Eip1559Fees::new(u128::MAX / 2, 1).unwrap();
        assert!(large.replacement_floor().is_ok());
        let signed = sign_call(&key(), 31_337, kat_target(), kat_input(), kat_params()).unwrap();
        let debug = format!("{signed:?}");
        assert!(!debug.contains("input"));
        assert!(!debug.contains("raw"));
        assert!(!debug.contains(&format!("{:?}", kat_input())));
    }
}
