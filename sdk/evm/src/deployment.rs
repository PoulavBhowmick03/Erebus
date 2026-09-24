//! The EVM deployment an agreement is authorized against.
//!
//! A deployment fixes the chain namespace, the chain id inside it, the settlement contract, and
//! the verifier version. The contract checks all three against its own configuration; the
//! adapter checks them before preparing funds, so a caller gets a local error rather than a
//! reverted transaction.

use erebus_core::domain::DeploymentDomain;
use erebus_core::ids::{AssetId, ChainNamespace};

use crate::error::EvmError;

/// Length of an EVM address.
pub const ADDRESS_BYTES: usize = 20;

/// One configured public-bound EVM deployment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmDeployment {
    /// CAIP-2 namespace, which must be `eip155:<numeric chain id>`.
    pub namespace: ChainNamespace,
    /// The chain id parsed out of the namespace.
    pub chain_id: u64,
    /// The deployed settlement contract.
    pub settlement_contract: [u8; ADDRESS_BYTES],
    /// Verifier version the deployment enforces.
    pub verifier_version: u32,
    /// JSON-RPC endpoint.
    pub rpc_url: String,
}

impl EvmDeployment {
    /// Builds a deployment, requiring an EVM namespace with a numeric chain id.
    pub fn new(
        namespace: ChainNamespace,
        settlement_contract: [u8; ADDRESS_BYTES],
        verifier_version: u32,
        rpc_url: impl Into<String>,
    ) -> Result<Self, EvmError> {
        let chain_id = chain_id_of(&namespace)?;
        Ok(Self {
            namespace,
            chain_id,
            settlement_contract,
            verifier_version,
            rpc_url: rpc_url.into(),
        })
    }

    /// Checks that an agreement's domain is exactly this deployment.
    pub fn matches_domain(&self, domain: &DeploymentDomain) -> Result<(), EvmError> {
        if domain.namespace != self.namespace {
            return Err(EvmError::DeploymentMismatch);
        }
        let Some(contract) = &domain.settlement_contract else {
            return Err(EvmError::DeploymentMismatch);
        };
        if contract.as_bytes() != self.settlement_contract {
            return Err(EvmError::DeploymentMismatch);
        }
        if domain.verifier_version != self.verifier_version {
            return Err(EvmError::DeploymentMismatch);
        }
        Ok(())
    }

    /// Resolves the ERC-20 address named by an asset identifier.
    ///
    /// The accepted form is exactly the form the contract parses: the asset must be on this
    /// deployment's chain, use the `erc20` asset namespace, and carry a lowercase `0x`-prefixed
    /// 20-byte address. Any other form is rejected here rather than by a revert on chain.
    pub fn token_address(&self, asset: &AssetId) -> Result<[u8; ADDRESS_BYTES], EvmError> {
        if asset.namespace() != &self.namespace || asset.asset_namespace() != "erc20" {
            return Err(EvmError::UnsupportedAsset(asset.to_string()));
        }
        parse_lowercase_address(asset.asset_reference())
            .ok_or_else(|| EvmError::UnsupportedAsset(asset.to_string()))
    }
}

fn chain_id_of(namespace: &ChainNamespace) -> Result<u64, EvmError> {
    if namespace.family() != "eip155" {
        return Err(EvmError::NotEvmNamespace(namespace.to_string()));
    }
    namespace
        .reference()
        .parse::<u64>()
        .map_err(|_| EvmError::NotEvmNamespace(namespace.to_string()))
}

/// Parses exactly `0x` plus 40 lowercase hex characters.
#[must_use]
pub fn parse_lowercase_address(value: &str) -> Option<[u8; ADDRESS_BYTES]> {
    let digits = value.strip_prefix("0x")?;
    if digits.len() != 40
        || !digits
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let bytes = hex::decode(digits).ok()?;
    bytes.try_into().ok()
}

/// Renders a lowercase `0x` address, the form the contract requires.
#[must_use]
pub fn format_lowercase_address(address: &[u8; ADDRESS_BYTES]) -> String {
    format!("0x{}", hex::encode(address))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_chain_ids_must_be_numeric() {
        let namespace = ChainNamespace::new("eip155", "31337").expect("namespace");
        let deployment = EvmDeployment::new(namespace, [0x11; 20], 1, "http://127.0.0.1:8545")
            .expect("deployment");
        assert_eq!(deployment.chain_id, 31337);

        let solana = ChainNamespace::new("solana", "mainnet").expect("namespace");
        assert!(matches!(
            EvmDeployment::new(solana, [0x11; 20], 1, "http://127.0.0.1:8545"),
            Err(EvmError::NotEvmNamespace(_))
        ));
    }

    #[test]
    fn asset_references_must_be_lowercase_addresses() {
        let namespace = ChainNamespace::new("eip155", "31337").expect("namespace");
        let deployment =
            EvmDeployment::new(namespace.clone(), [0x11; 20], 1, "http://127.0.0.1:8545")
                .expect("deployment");
        let good = AssetId::parse("eip155:31337/erc20:0x00000000000000000000000000000000000000aa")
            .expect("asset");
        let mut expected = [0u8; ADDRESS_BYTES];
        expected[ADDRESS_BYTES - 1] = 0xaa;
        assert_eq!(deployment.token_address(&good).expect("token"), expected);

        for bad in [
            "eip155:31337/erc20:0x00000000000000000000000000000000000000AA",
            "eip155:1/erc20:0x00000000000000000000000000000000000000aa",
            "eip155:31337/slip44:0x00000000000000000000000000000000000000aa",
        ] {
            let asset = AssetId::parse(bad).expect("asset");
            assert!(deployment.token_address(&asset).is_err(), "accepted {bad}");
        }
    }
}
