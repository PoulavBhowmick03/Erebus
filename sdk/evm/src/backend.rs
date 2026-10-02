//! The public-bound EVM settlement backend.
//!
//! Preparation is local and fails before any network call. This adapter estimates funding
//! but never signs, submits, or declares payment final. Use the coordinator and
//! [`crate::chain::EvmChain::broadcast_journaled`] for submission, followed by paired
//! finalized chain evidence for reconciliation.
//!
//! The adapter never trusts itself: it verifies both authorizations locally, and the contract
//! re-verifies them from the terms opening, so a bug here cannot produce a payment the signers
//! did not authorize. A successful RPC response is not payment evidence.

use alloy::network::TransactionBuilder;
use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;

use erebus_core::auth::{verify_authorization_signature, Authorization, Role};
use erebus_core::commitment::{commit_agreement, deal_nullifier, CommitmentBlinding};
use erebus_core::settlement::{
    check_capabilities, BackendCapabilities, PreparedSettlement, SettlementContext,
};
use erebus_core::suite::EVM_SECP256K1_KECCAK_SUITE_ID;
use erebus_core::terms::{AgreementTerms, Guarantee, GuaranteeSet, SettlementMode};

use crate::abi;
use crate::deployment::{EvmDeployment, ADDRESS_BYTES};
use crate::error::EvmError;
use crate::evidence::{SettlementEvidence, SIGNATURE_BYTES};

/// A configured connection to one public-bound EVM settlement deployment.
pub struct EvmSettlementBackend {
    deployment: EvmDeployment,
    provider: DynProvider,
    signer_address: Address,
}

impl std::fmt::Debug for EvmSettlementBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EvmSettlementBackend")
            .field("deployment", &self.deployment)
            .field("signer", &format!("{:#x}", self.signer_address))
            .finish_non_exhaustive()
    }
}

/// What the gas payer needs before a relayer can submit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FundingDiagnostics {
    /// Estimated gas for the settlement transaction.
    pub gas: u64,
    /// Current gas price in wei.
    pub gas_price: u128,
    /// `gas * gas_price`, the native balance the signer must hold.
    pub required: u128,
    /// The signer's current native balance.
    pub balance: u128,
}

impl FundingDiagnostics {
    /// The native shortfall, zero when the signer is funded.
    #[must_use]
    pub const fn shortfall(&self) -> u128 {
        self.required.saturating_sub(self.balance)
    }

    /// Reports whether the signer can pay for the estimated transaction.
    #[must_use]
    pub const fn is_funded(&self) -> bool {
        self.balance >= self.required
    }
}

/// What a caller needs to know before submitting: allowance, balance, and gas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettlementEstimate {
    /// The total the buyer must have approved this deployment for (amount plus fee).
    pub required_allowance: u128,
    /// The buyer's current allowance to the settlement contract.
    pub allowance: u128,
    /// The buyer's current token balance.
    pub buyer_balance: u128,
    /// Estimated gas for the settlement transaction.
    pub gas: u64,
}

impl SettlementEstimate {
    /// Reports whether the buyer has approved enough.
    #[must_use]
    pub const fn allowance_is_sufficient(&self) -> bool {
        self.allowance >= self.required_allowance
    }

    /// Reports whether the buyer holds enough tokens.
    #[must_use]
    pub const fn balance_is_sufficient(&self) -> bool {
        self.buyer_balance >= self.required_allowance
    }

    /// The allowance shortfall, zero when sufficient.
    #[must_use]
    pub const fn allowance_shortfall(&self) -> u128 {
        self.required_allowance.saturating_sub(self.allowance)
    }

    /// The balance shortfall, zero when sufficient.
    #[must_use]
    pub const fn balance_shortfall(&self) -> u128 {
        self.required_allowance.saturating_sub(self.buyer_balance)
    }
}

impl EvmSettlementBackend {
    /// Connects for preparation and estimates, deriving the gas payer's address from its key.
    /// The provider does not retain the key or have a signing wallet.
    pub fn connect(deployment: EvmDeployment, signing_key: &[u8; 32]) -> Result<Self, EvmError> {
        let signer =
            PrivateKeySigner::from_slice(signing_key).map_err(|_| EvmError::InvalidSigningKey)?;
        let signer_address = signer.address();
        let url: alloy::transports::http::reqwest::Url = deployment
            .rpc_url
            .parse()
            .map_err(|error| EvmError::Rpc(format!("invalid RPC URL: {error}")))?;
        let provider = ProviderBuilder::new().connect_http(url).erased();
        Ok(Self {
            deployment,
            provider,
            signer_address,
        })
    }

    /// Connects read-only: no signing key, so it can only prepare, estimate, and read.
    ///
    /// `signer_address` is the gas payer whose native funding diagnostics are reported. This
    /// constructor cannot submit; the journaled coordinator path owns signing and broadcast.
    pub fn read_only(
        deployment: EvmDeployment,
        signer_address: [u8; ADDRESS_BYTES],
    ) -> Result<Self, EvmError> {
        let url: alloy::transports::http::reqwest::Url = deployment
            .rpc_url
            .parse()
            .map_err(|error| EvmError::Rpc(format!("invalid RPC URL: {error}")))?;
        let provider = ProviderBuilder::new().connect_http(url).erased();
        Ok(Self {
            deployment,
            provider,
            signer_address: Address::from(signer_address),
        })
    }

    /// The configured deployment.
    #[must_use]
    pub fn deployment(&self) -> &EvmDeployment {
        &self.deployment
    }

    /// The address transactions are signed from. It pays gas; it is not the payer of the deal.
    #[must_use]
    pub fn signer_address(&self) -> [u8; ADDRESS_BYTES] {
        self.signer_address.0 .0
    }

    /// What this backend provides.
    ///
    /// Public-bound settlement hides neither amount nor recipient, so neither privacy guarantee
    /// is declared. Agreement-bound settlement is declared because the contract recomputes the
    /// commitment and verifies both authorizations before moving funds.
    #[must_use]
    pub fn capabilities(&self) -> BackendCapabilities {
        public_bound_capabilities()
    }

    /// Checks a session context against this backend before any funds are touched.
    pub fn check(&self, context: &SettlementContext) -> Result<(), EvmError> {
        check_context(&self.deployment, context)
    }

    /// Validates an agreement and its authorizations, and produces the backend evidence.
    ///
    /// Everything here is local. A failure means no transaction is built and no RPC call is
    /// made, which is the property `sdk/rs/tests/capabilities.rs` established for the legacy
    /// adapter and this crate preserves.
    pub fn prepare(
        &self,
        terms: &AgreementTerms,
        blinding: &CommitmentBlinding,
        buyer: &Authorization,
        seller: &Authorization,
        operation_ref: [u8; 32],
    ) -> Result<PreparedSettlement, EvmError> {
        terms.validate()?;
        validate_terms(&self.deployment, terms)?;

        if buyer.role != Role::Buyer {
            return Err(EvmError::WrongRole {
                found: buyer.role.name(),
                expected: Role::Buyer.name(),
            });
        }
        if seller.role != Role::Seller {
            return Err(EvmError::WrongRole {
                found: seller.role.name(),
                expected: Role::Seller.name(),
            });
        }
        let commitment = commit_agreement(terms, blinding)?;
        verify_authorization_signature(terms, &commitment, blinding, buyer)?;
        verify_authorization_signature(terms, &commitment, blinding, seller)?;
        let nullifier = deal_nullifier(terms)?;

        let evidence = SettlementEvidence {
            terms: terms.encode()?,
            blinding: *blinding.as_bytes(),
            buyer_signature: to_signature(buyer)?,
            seller_signature: to_signature(seller)?,
        };
        Ok(PreparedSettlement {
            operation_ref,
            deal_commitment: commitment,
            deal_nullifier: nullifier,
            domain: terms.domain.clone(),
            mode: terms.settlement_mode,
            required_guarantees: terms.required_guarantees,
            backend_evidence: evidence.encode(),
        })
    }

    /// Decodes the agreement out of prepared backend evidence.
    pub fn evidence(&self, prepared: &PreparedSettlement) -> Result<SettlementEvidence, EvmError> {
        Ok(SettlementEvidence::decode(&prepared.backend_evidence)?)
    }

    /// Estimates allowance, balance, and gas before submission.
    pub async fn estimate(
        &self,
        terms: &AgreementTerms,
        blinding: &CommitmentBlinding,
        buyer: &Authorization,
        seller: &Authorization,
        operation_ref: [u8; 32],
    ) -> Result<SettlementEstimate, EvmError> {
        let prepared = self.prepare(terms, blinding, buyer, seller, operation_ref)?;
        self.check_live_chain().await?;
        let token = self.deployment.token_address(&terms.asset)?;
        let payer = key_address(terms, Role::Buyer)?;
        let total = terms
            .amount
            .get()
            .checked_add(terms.fee_policy.fee.get())
            .ok_or(EvmError::UnsupportedAsset(
                "payment total overflow".to_owned(),
            ))?;

        let allowance = self
            .read_token_word(
                &token,
                &abi::encode_allowance_call(&payer, &self.deployment.settlement_contract),
            )
            .await?;
        let balance = self
            .read_token_word(&token, &abi::encode_balance_of_call(&payer))
            .await?;

        let settlement = Address::from(self.deployment.settlement_contract);
        let evidence = self.evidence(&prepared)?;
        let calldata = abi::encode_settle_call(
            &evidence.terms,
            &evidence.blinding,
            &evidence.buyer_signature,
            &evidence.seller_signature,
            &token,
        );
        let request = TransactionRequest::default()
            .with_from(self.signer_address)
            .with_to(settlement)
            .with_input(Bytes::from(calldata));
        let gas = self
            .provider
            .estimate_gas(request)
            .await
            .map_err(|error| EvmError::Rpc(error.to_string()))?;

        Ok(SettlementEstimate {
            required_allowance: total,
            allowance,
            buyer_balance: balance,
            gas,
        })
    }

    /// Reports the gas payer's native funding against the estimated transaction cost.
    ///
    /// This is a relayer diagnostic, not a payment check: it never inspects the buyer's tokens
    /// and it never sends a transaction.
    pub async fn funding_diagnostics(
        &self,
        prepared: &PreparedSettlement,
    ) -> Result<FundingDiagnostics, EvmError> {
        self.check_live_chain().await?;
        let request = self.transaction_request(prepared)?;
        let gas = self
            .provider
            .estimate_gas(request)
            .await
            .map_err(|error| EvmError::Rpc(error.to_string()))?;
        let gas_price = self
            .provider
            .get_gas_price()
            .await
            .map_err(|error| EvmError::Rpc(error.to_string()))?;
        let balance = self
            .provider
            .get_balance(self.signer_address)
            .await
            .map_err(|error| EvmError::Rpc(error.to_string()))?;
        let required = U256::from(gas).saturating_mul(U256::from(gas_price));
        self.check_live_chain().await?;
        Ok(FundingDiagnostics {
            gas,
            gas_price,
            required: u128::try_from(required)
                .map_err(|_| EvmError::Rpc("required funding exceeds u128".to_owned()))?,
            balance: u128::try_from(balance)
                .map_err(|_| EvmError::Rpc("balance exceeds u128".to_owned()))?,
        })
    }

    fn transaction_request(
        &self,
        prepared: &PreparedSettlement,
    ) -> Result<TransactionRequest, EvmError> {
        let validated = self.validate_prepared(prepared)?;
        Ok(TransactionRequest::default()
            .with_from(self.signer_address)
            .with_to(Address::from(self.deployment.settlement_contract))
            .with_input(Bytes::from(validated.calldata)))
    }

    fn validate_prepared(
        &self,
        prepared: &PreparedSettlement,
    ) -> Result<ValidatedPrepared, EvmError> {
        validate_prepared(&self.deployment, prepared)
    }

    async fn check_live_chain(&self) -> Result<(), EvmError> {
        let found = self
            .provider
            .get_chain_id()
            .await
            .map_err(|error| EvmError::Rpc(error.to_string()))?;
        if found != self.deployment.chain_id {
            return Err(EvmError::ChainIdMismatch {
                expected: self.deployment.chain_id,
                found,
            });
        }
        Ok(())
    }

    async fn read_token_word(
        &self,
        token: &[u8; ADDRESS_BYTES],
        calldata: &[u8],
    ) -> Result<u128, EvmError> {
        let request = TransactionRequest::default()
            .with_to(Address::from(*token))
            .with_input(Bytes::from(calldata.to_vec()));
        let result = self
            .provider
            .call(request)
            .await
            .map_err(|error| EvmError::Rpc(error.to_string()))?;
        if result.len() < 32 {
            return Err(EvmError::Rpc("token call returned a short word".to_owned()));
        }
        let value = U256::from_be_slice(&result[..32]);
        u128::try_from(value).map_err(|_| EvmError::Rpc("token value exceeds u128".to_owned()))
    }
}

/// What public-bound EVM settlement provides.
///
/// Public-bound settlement hides neither amount nor recipient, so neither privacy guarantee is
/// declared. Agreement-bound settlement is declared because the contract recomputes the
/// commitment and verifies both authorizations before moving funds.
/// What the public-bound EVM backend provides, without a connection.
///
/// Public-bound settlement hides neither amount nor recipient, so neither privacy guarantee
/// is declared. Agreement-bound settlement is declared because the contract recomputes the
/// commitment and verifies both authorizations before moving funds.
#[must_use]
pub fn public_bound_capabilities() -> BackendCapabilities {
    let mut modes = std::collections::BTreeSet::new();
    modes.insert(SettlementMode::PublicBound);
    BackendCapabilities {
        suites: [EVM_SECP256K1_KECCAK_SUITE_ID].into_iter().collect(),
        modes,
        guarantees: GuaranteeSet::from_guarantee(Guarantee::AgreementBoundSettlement),
        local_proving: true,
    }
}

fn check_context(deployment: &EvmDeployment, context: &SettlementContext) -> Result<(), EvmError> {
    if deployment.namespace.to_string() != format!("eip155:{}", deployment.chain_id) {
        return Err(EvmError::DeploymentMismatch);
    }
    check_capabilities(context, &public_bound_capabilities())?;
    deployment.matches_domain(&context.domain)?;
    Ok(())
}

/// Checks that decoded terms are settleable on this deployment: capabilities, domain, and a
/// usable token address.
pub(crate) fn validate_terms(
    deployment: &EvmDeployment,
    terms: &AgreementTerms,
) -> Result<(), EvmError> {
    let context = SettlementContext {
        require_local_proving: false,
        mode: terms.settlement_mode,
        domain: terms.domain.clone(),
        suite_id: terms.suite_id,
        asset: terms.asset.clone(),
        required_guarantees: terms.required_guarantees,
    };
    check_context(deployment, &context)?;
    // Resolve the token now so an unusable asset identifier fails before any RPC call.
    deployment.token_address(&terms.asset)?;
    Ok(())
}

/// Re-derives everything in a prepared settlement from its canonical evidence.
///
/// `PreparedSettlement` is untrusted persisted data (DM3-2): the commitment and nullifier are
/// recomputed from the terms opening and compared with the routing fields. Both authorizations
/// are checked before calldata is produced. Expiry is not checked here: receipt verification
/// must remain possible after expiry; the contract enforces expiry at execution.
pub(crate) fn validate_prepared(
    deployment: &EvmDeployment,
    prepared: &PreparedSettlement,
) -> Result<ValidatedPrepared, EvmError> {
    let evidence = SettlementEvidence::decode(&prepared.backend_evidence)?;
    let terms = AgreementTerms::decode(&evidence.terms)?;
    validate_terms(deployment, &terms)?;
    let blinding = CommitmentBlinding::from_bytes(evidence.blinding);
    let commitment = commit_agreement(&terms, &blinding)?;
    let nullifier = deal_nullifier(&terms)?;
    if commitment != prepared.deal_commitment
        || nullifier != prepared.deal_nullifier
        || terms.domain != prepared.domain
        || terms.settlement_mode != prepared.mode
        || terms.required_guarantees != prepared.required_guarantees
    {
        return Err(EvmError::PreparedMismatch);
    }
    let token = deployment.token_address(&terms.asset)?;
    for (role, signature) in [
        (Role::Buyer, evidence.buyer_signature),
        (Role::Seller, evidence.seller_signature),
    ] {
        let authorization = Authorization {
            role,
            suite_id: terms.suite_id,
            commitment,
            signature: erebus_core::ids::SignatureBytes::new(signature.to_vec())?,
        };
        verify_authorization_signature(&terms, &commitment, &blinding, &authorization)?;
    }
    let calldata = abi::encode_settle_call(
        &evidence.terms,
        &evidence.blinding,
        &evidence.buyer_signature,
        &evidence.seller_signature,
        &token,
    );
    Ok(ValidatedPrepared { token, calldata })
}

pub(crate) struct ValidatedPrepared {
    pub(crate) token: [u8; ADDRESS_BYTES],
    pub(crate) calldata: Vec<u8>,
}

fn to_signature(authorization: &Authorization) -> Result<[u8; SIGNATURE_BYTES], EvmError> {
    let bytes = authorization.signature.as_bytes();
    bytes
        .try_into()
        .map_err(|_| EvmError::SignatureLength(bytes.len()))
}

fn key_address(terms: &AgreementTerms, role: Role) -> Result<[u8; ADDRESS_BYTES], EvmError> {
    let key = match role {
        Role::Buyer => &terms.buyer_authorization_key,
        Role::Seller => &terms.seller_authorization_key,
    };
    key.as_bytes()
        .try_into()
        .map_err(|_| EvmError::SignatureLength(key.as_bytes().len()))
}
