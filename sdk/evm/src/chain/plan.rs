//! Canonical local signing parameters. The agreement supplies chain, target, and calldata.

use super::{
    build_signed, build_signed_call, Eip1559Fees, SignedTransaction, TransactionKey,
    TransactionParams,
};
use crate::{deployment::EvmDeployment, error::EvmError};
use erebus_core::{
    encoding::{Reader, Writer},
    settlement::PreparedSettlement,
};

/// Immutable local transaction intent persisted before signing.
/// This does not allocate a nonce or establish ownership of a nonce reservation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigningPlan {
    sender: [u8; 20],
    params: TransactionParams,
}

impl SigningPlan {
    /// The gas-paying sender bound by this plan.
    #[must_use]
    pub const fn sender(&self) -> [u8; 20] {
        self.sender
    }

    /// The immutable nonce, fee caps, and gas limit.
    #[must_use]
    pub const fn params(&self) -> TransactionParams {
        self.params
    }

    /// Creates a plan using an explicitly selected sender and transaction parameters.
    pub fn new(sender: [u8; 20], params: TransactionParams) -> Result<Self, EvmError> {
        if params.gas_limit == 0 {
            return Err(EvmError::TransactionEncoding("gas limit is zero".into()));
        }
        Ok(Self { sender, params })
    }

    /// Encodes version 1, sender, nonce, max fee, priority fee, and gas limit, in that order.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u8(1);
        w.fixed(&self.sender);
        w.u64(self.params.nonce);
        w.u128(self.params.fees.max_fee_per_gas());
        w.u128(self.params.fees.max_priority_fee_per_gas());
        w.u64(self.params.gas_limit);
        w.finish()
    }

    /// Decodes canonical local metadata; unknown versions and trailing bytes fail closed.
    pub fn decode(bytes: &[u8]) -> Result<Self, EvmError> {
        let mut r = Reader::new(bytes);
        if r.u8("plan_version")? != 1 {
            return Err(EvmError::TransactionEncoding(
                "unknown signing plan version".into(),
            ));
        }
        let sender = r.fixed("sender")?;
        let nonce = r.u64("nonce")?;
        let fees = Eip1559Fees::new(r.u128("max_fee")?, r.u128("priority_fee")?)?;
        let gas_limit = r.u64("gas_limit")?;
        r.finish()?;
        Self::new(
            sender,
            TransactionParams {
                nonce,
                fees,
                gas_limit,
            },
        )
    }

    /// Signs only with the planned gas payer. Does not perform network I/O.
    pub fn sign(
        &self,
        deployment: &EvmDeployment,
        prepared: &PreparedSettlement,
        key: &TransactionKey,
    ) -> Result<SignedTransaction, EvmError> {
        if key.address() != self.sender {
            return Err(EvmError::SignedIntentMismatch);
        }
        build_signed(deployment, prepared, key, self.params)
    }

    /// Signs a backend-validated call with the journaled sender and parameters.
    pub fn sign_call(
        &self,
        chain_id: u64,
        to: [u8; 20],
        input: Vec<u8>,
        key: &TransactionKey,
    ) -> Result<SignedTransaction, EvmError> {
        if key.address() != self.sender {
            return Err(EvmError::SignedIntentMismatch);
        }
        build_signed_call(key, chain_id, to, input, self.params)
    }

    /// Checks restored bytes against both the durable plan and the accepted agreement.
    pub fn validate(
        &self,
        deployment: &EvmDeployment,
        prepared: &PreparedSettlement,
        raw: &[u8],
    ) -> Result<(), EvmError> {
        SignedTransaction::from_raw(raw)?.validate_against(
            deployment,
            prepared,
            self.sender,
            self.params,
        )
    }

    /// Checks restored bytes against backend-validated call data and this stored plan.
    pub fn validate_call(
        &self,
        chain_id: u64,
        to: [u8; 20],
        input: &[u8],
        raw: &[u8],
    ) -> Result<(), EvmError> {
        SignedTransaction::from_raw(raw)?.validate_call(
            chain_id,
            to,
            input,
            self.sender,
            self.params,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plan_encoding_is_strict_and_round_trips() {
        let plan = SigningPlan::new(
            [1; 20],
            TransactionParams {
                nonce: 7,
                fees: Eip1559Fees::new(100, 10).unwrap(),
                gas_limit: 300_000,
            },
        )
        .unwrap();
        let bytes = plan.encode();
        assert_eq!(bytes.len(), 69);
        assert_eq!(SigningPlan::decode(&bytes).unwrap(), plan);
        for end in 0..bytes.len() {
            assert!(SigningPlan::decode(&bytes[..end]).is_err());
        }
        let mut changed = bytes.clone();
        changed.push(0);
        assert!(SigningPlan::decode(&changed).is_err());
        let mut changed = bytes.clone();
        changed[0] = 2;
        assert!(SigningPlan::decode(&changed).is_err());
        let mut changed = bytes.clone();
        changed[61..].fill(0);
        assert!(SigningPlan::decode(&changed).is_err());
        let mut changed = bytes;
        changed[29..45].fill(0);
        assert!(SigningPlan::decode(&changed).is_err());
    }
}
