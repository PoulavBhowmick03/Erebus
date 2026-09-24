//! The public-bound EVM settlement backend.
//!
//! It fills the M1 backend contract (`prepare`, `submit`, `verify`) with a real chain
//! interaction. `prepare` is local and fails before any network call; `submit` signs and sends;
//! `verify` reads chain evidence and normalizes it into a [`SettlementReceipt`].
//!
//! The adapter never trusts itself: it verifies both authorizations locally, and the contract
//! re-verifies them from the terms opening, so a bug here cannot produce a payment the signers
//! did not authorize. On-chain state is the only source of payment truth: a receipt is built
//! from a mined transaction, not from a successful RPC response.

use std::time::{SystemTime, UNIX_EPOCH};

use alloy::consensus::Transaction;
use alloy::network::{EthereumWallet, TransactionBuilder};
use alloy::primitives::{Address, Bytes, B256, U256};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;

use erebus_core::auth::{verify_authorization_signature, Authorization, Role};
use erebus_core::commitment::{commit_agreement, deal_nullifier, CommitmentBlinding};
use erebus_core::settlement::{
    check_capabilities, BackendCapabilities, DeliveryStatus, Finality, PaymentStatus,
    PreparedSettlement, SettlementContext, SettlementReceipt,
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
    confirmations: u64,
}

impl std::fmt::Debug for EvmSettlementBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EvmSettlementBackend")
            .field("deployment", &self.deployment)
            .field("signer", &format!("{:#x}", self.signer_address))
            .field("confirmations", &self.confirmations)
            .finish_non_exhaustive()
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
    /// Connects to a deployment with a local transaction signing key.
    pub fn connect(deployment: EvmDeployment, signing_key: &[u8; 32]) -> Result<Self, EvmError> {
        let signer =
            PrivateKeySigner::from_slice(signing_key).map_err(|_| EvmError::InvalidSigningKey)?;
        let signer_address = signer.address();
        let wallet = EthereumWallet::from(signer);
        let url: alloy::transports::http::reqwest::Url = deployment
            .rpc_url
            .parse()
            .map_err(|error| EvmError::Rpc(format!("invalid RPC URL: {error}")))?;
        let provider = ProviderBuilder::new()
            .wallet(wallet)
            .connect_http(url)
            .erased();
        Ok(Self {
            deployment,
            provider,
            signer_address,
            confirmations: 1,
        })
    }

    /// Requires this many confirmations before a receipt is final.
    #[must_use]
    pub const fn with_confirmations(mut self, confirmations: u64) -> Self {
        self.confirmations = confirmations;
        self
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
        let mut modes = std::collections::BTreeSet::new();
        modes.insert(SettlementMode::PublicBound);
        BackendCapabilities {
            suites: [EVM_SECP256K1_KECCAK_SUITE_ID].into_iter().collect(),
            modes,
            guarantees: GuaranteeSet::from_guarantee(Guarantee::AgreementBoundSettlement),
            local_proving: true,
        }
    }

    /// Checks a session context against this backend before any funds are touched.
    pub fn check(&self, context: &SettlementContext) -> Result<(), EvmError> {
        check_capabilities(context, &self.capabilities())?;
        self.deployment.matches_domain(&context.domain)?;
        Ok(())
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
        self.validate_terms(terms)?;

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

    /// Signs and submits the prepared settlement, returning the transaction hash.
    pub async fn submit(&self, prepared: &PreparedSettlement) -> Result<Vec<u8>, EvmError> {
        self.check_live_chain().await?;
        let request = self.transaction_request(prepared)?;
        let pending = self
            .provider
            .send_transaction(request)
            .await
            .map_err(|error| EvmError::Rpc(error.to_string()))?;
        Ok(pending.tx_hash().0.to_vec())
    }

    /// Reads chain evidence and normalizes it into a receipt.
    pub async fn verify(
        &self,
        prepared: &PreparedSettlement,
        transaction_ref: &[u8],
    ) -> Result<SettlementReceipt, EvmError> {
        self.check_live_chain().await?;
        let validated = self.validate_prepared(prepared)?;
        let hash: [u8; 32] = transaction_ref
            .try_into()
            .map_err(|_| EvmError::Rpc("transaction reference is not 32 bytes".to_owned()))?;
        let receipt = self
            .provider
            .get_transaction_receipt(B256::from(hash))
            .await
            .map_err(|error| EvmError::Rpc(error.to_string()))?
            .ok_or(EvmError::MissingReceipt)?;

        let transaction = self
            .provider
            .get_transaction_by_hash(B256::from(hash))
            .await
            .map_err(|error| EvmError::Rpc(error.to_string()))?
            .ok_or(EvmError::MissingTransaction)?;
        let settlement = Address::from(self.deployment.settlement_contract);
        if receipt.to != Some(settlement)
            || transaction.to() != Some(settlement)
            || transaction.chain_id() != Some(self.deployment.chain_id)
            || transaction.input().as_ref() != validated.calldata.as_slice()
        {
            return Err(EvmError::ReceiptMismatch);
        }

        let success = receipt.status();
        let block_number = receipt.block_number.ok_or(EvmError::ReceiptMismatch)?;
        if receipt.block_hash.is_none() {
            return Err(EvmError::ReceiptMismatch);
        }
        let latest = self
            .provider
            .get_block_number()
            .await
            .map_err(|error| EvmError::Rpc(error.to_string()))?;
        let confirmations = latest.saturating_sub(block_number);
        let finality = if !success {
            Finality::Reverted
        } else if confirmations >= self.confirmations {
            Finality::Finalized
        } else {
            Finality::Included
        };

        if success
            && !receipt_has_expected_event(
                &receipt,
                prepared,
                &validated.terms,
                self.deployment.settlement_contract,
                validated.token,
            )
        {
            return Err(EvmError::ReceiptMismatch);
        }

        Ok(SettlementReceipt {
            operation_ref: prepared.operation_ref,
            deal_commitment: prepared.deal_commitment,
            deal_nullifier: prepared.deal_nullifier,
            domain: prepared.domain.clone(),
            mode: prepared.mode,
            transaction_ref: transaction_ref.to_vec(),
            block_ref: Some(block_number.to_be_bytes().to_vec()),
            finality,
            verified_guarantees: GuaranteeSet::from_guarantee(Guarantee::AgreementBoundSettlement),
            payment: if success {
                PaymentStatus::Settled
            } else {
                PaymentStatus::Reverted
            },
            delivery: DeliveryStatus::NotStarted,
            observed_at: unix_now()?,
        })
    }

    /// Submits and waits for a receipt, returning the normalized result.
    pub async fn settle(
        &self,
        prepared: &PreparedSettlement,
    ) -> Result<SettlementReceipt, EvmError> {
        self.check_live_chain().await?;
        let request = self.transaction_request(prepared)?;
        let pending = self
            .provider
            .send_transaction(request)
            .await
            .map_err(|error| EvmError::Rpc(error.to_string()))?;
        let transaction_ref = pending.tx_hash().0;
        pending
            .get_receipt()
            .await
            .map_err(|error| EvmError::Rpc(error.to_string()))?;
        self.verify(prepared, &transaction_ref).await
    }

    fn transaction_request(
        &self,
        prepared: &PreparedSettlement,
    ) -> Result<TransactionRequest, EvmError> {
        let validated = self.validate_prepared(prepared)?;
        Ok(TransactionRequest::default()
            .with_to(Address::from(self.deployment.settlement_contract))
            .with_input(Bytes::from(validated.calldata)))
    }

    fn validate_terms(&self, terms: &AgreementTerms) -> Result<(), EvmError> {
        let context = SettlementContext {
            require_local_proving: false,
            mode: terms.settlement_mode,
            domain: terms.domain.clone(),
            suite_id: terms.suite_id,
            asset: terms.asset.clone(),
            required_guarantees: terms.required_guarantees,
        };
        self.check(&context)?;
        // Resolve the token now so an unusable asset identifier fails before any RPC call.
        self.deployment.token_address(&terms.asset)?;
        Ok(())
    }

    fn validate_prepared(
        &self,
        prepared: &PreparedSettlement,
    ) -> Result<ValidatedPrepared, EvmError> {
        let evidence = self.evidence(prepared)?;
        let terms = AgreementTerms::decode(&evidence.terms)?;
        self.validate_terms(&terms)?;
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
        let token = self.deployment.token_address(&terms.asset)?;
        let calldata = abi::encode_settle_call(
            &evidence.terms,
            &evidence.blinding,
            &evidence.buyer_signature,
            &evidence.seller_signature,
            &token,
        );
        Ok(ValidatedPrepared {
            terms,
            token,
            calldata,
        })
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

struct ValidatedPrepared {
    terms: AgreementTerms,
    token: [u8; ADDRESS_BYTES],
    calldata: Vec<u8>,
}

fn receipt_has_expected_event(
    receipt: &alloy::rpc::types::TransactionReceipt,
    prepared: &PreparedSettlement,
    terms: &AgreementTerms,
    settlement_contract: [u8; ADDRESS_BYTES],
    token: [u8; ADDRESS_BYTES],
) -> bool {
    let Ok(buyer) = key_address(terms, Role::Buyer) else {
        return false;
    };
    let recipient: [u8; ADDRESS_BYTES] = match terms.payment_recipient.as_bytes().try_into() {
        Ok(recipient) => recipient,
        Err(_) => return false,
    };
    let expected_topics = [
        B256::from(abi::deal_settled_topic()),
        B256::from(*prepared.deal_commitment.as_bytes()),
        B256::from(*prepared.deal_nullifier.as_bytes()),
        address_topic(buyer),
    ];
    let expected_contract = Address::from(settlement_contract);
    let mut matches = 0usize;
    for log in receipt.logs() {
        if log.removed || log.address() != expected_contract || log.topics() != expected_topics {
            continue;
        }
        let data = log.data().data.as_ref();
        if data.len() != 128
            || data[..12] != [0u8; 12]
            || data[32..44] != [0u8; 12]
            || data[12..32] != recipient
            || data[44..64] != token
            || U256::from_be_slice(&data[64..96]) != U256::from(terms.amount.get())
            || U256::from_be_slice(&data[96..128]) != U256::from(terms.fee_policy.fee.get())
        {
            continue;
        }
        matches += 1;
    }
    matches == 1
}

fn address_topic(address: [u8; ADDRESS_BYTES]) -> B256 {
    let mut topic = [0u8; 32];
    topic[12..].copy_from_slice(&address);
    B256::from(topic)
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

fn unix_now() -> Result<u64, EvmError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| EvmError::Clock)
}
