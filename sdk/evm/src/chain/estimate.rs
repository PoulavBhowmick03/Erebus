//! Gas estimates for exact calls, including a native value. Estimates never sign or send.

use super::EvmChain;
use crate::error::EvmError;
use alloy::{
    primitives::{Address, Bytes, U256},
    providers::Provider,
    rpc::types::TransactionRequest,
};

impl EvmChain {
    /// Estimates one exact call from `sender`, carrying `value` wei, at the latest block.
    ///
    /// A wrap must be estimated with its value: without it the wrapper's deposit path is not
    /// the one that runs. The result is only an input to a gas-limit policy check; nothing is
    /// signed, persisted, or sent, and no field of a later transaction is filled from it.
    pub async fn estimate_call_gas_with_value(
        &self,
        sender: [u8; 20],
        target: [u8; 20],
        calldata: &[u8],
        value: u128,
    ) -> Result<u64, EvmError> {
        self.check_chain().await?;
        let request = TransactionRequest::default()
            .from(Address::from(sender))
            .to(Address::from(target))
            .value(U256::from(value))
            .input(Bytes::copy_from_slice(calldata).into());
        tokio::time::timeout(self.timeout, self.provider.estimate_gas(request))
            .await
            .map_err(|_| EvmError::Rpc("gas estimate timed out".into()))?
            .map_err(|_| EvmError::Rpc("gas estimate failed".into()))
    }
}
