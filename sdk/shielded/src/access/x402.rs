//! Seller-facilitated exact payments with durable authorization and broadcast fencing.

use base64::{engine::general_purpose::STANDARD, Engine};
use std::{path::PathBuf, sync::Arc};

use erebus_core::{settlement::PreparedSettlement, terms::AgreementTerms};
use erebus_evm::{
    chain::{
        Eip1559Fees, EvmChain, NonceError, SignedTransaction, SignerJournal, SigningPlan,
        TransactionKey,
    },
    x402::{validate_permit_fields, verify_permit_signature, EXACT_PERMIT2_PROXY},
};
use erebus_journal::{FaultHook, JournalRecord, NoFaults, Store};
use erebus_transport::disclosure::SelectedAgreement;
use serde::{Deserialize, Serialize};

use super::{AccessBackend, AccessError, AccessPolicy, IssuanceId, PaidAccess, X402Payment};

/// x402 v2 exact requirements for the already negotiated agreement; no automatic repricing.
pub fn payment_requirements(terms: &AgreementTerms) -> Result<serde_json::Value, AccessError> {
    let deployment = erebus_evm::deployment::EvmDeployment::new(
        terms.domain.namespace.clone(),
        EXACT_PERMIT2_PROXY,
        terms.domain.verifier_version,
        "http://127.0.0.1:1",
    )
    .map_err(|_| AccessError::Agreement)?;
    deployment
        .matches_domain(&terms.domain)
        .map_err(|_| AccessError::Agreement)?;
    let token = deployment
        .token_address(&terms.asset)
        .map_err(|_| AccessError::Agreement)?;
    if terms.payment_recipient.as_bytes().len() != 20 {
        return Err(AccessError::Agreement);
    }
    Ok(
        serde_json::json!({"scheme":"exact","network":terms.domain.namespace.to_string(),
        "amount":terms.amount.get().to_string(),"asset":format!("0x{}",hex::encode(token)),
        "payTo":format!("0x{}",hex::encode(terms.payment_recipient.as_bytes())),"maxTimeoutSeconds":120,
        "extra":{"assetTransferMethod":"permit2"}}),
    )
}

/// Encodes the standard PAYMENT-SIGNATURE header, alongside the signed Erebus access request.
pub fn payment_signature_header(
    terms: &AgreementTerms,
    payment: &X402Payment,
) -> Result<String, AccessError> {
    let deal =
        erebus_core::commitment::deal_nullifier(terms).map_err(|_| AccessError::Agreement)?;
    let payload = serde_json::json!({"x402Version":2,"accepted":payment_requirements(terms)?,
        "payload":{"signature":format!("0x{}",hex::encode(&payment.signature)),"permit2Authorization":{
            "permitted":{"token":format!("0x{}",hex::encode(payment.token)),"amount":payment.amount.to_string()},
            "from":format!("0x{}",hex::encode(terms.buyer_authorization_key.as_bytes())),
            "spender":format!("0x{}",hex::encode(EXACT_PERMIT2_PROXY)),
            "nonce":num_bigint::BigUint::from_bytes_be(deal.as_bytes()).to_string(),"deadline":payment.deadline.to_string(),
            "witness":{"to":format!("0x{}",hex::encode(payment.to)),"validAfter":payment.valid_after.to_string()}}}});
    Ok(STANDARD.encode(serde_json::to_vec(&payload).map_err(|_| AccessError::Agreement)?))
}

/// Checks the header matches every field bound into the buyer's signed access request.
pub fn verify_payment_header(
    header: &str,
    terms: &AgreementTerms,
    payment: &X402Payment,
) -> Result<(), AccessError> {
    if header.len() > 8192 {
        return Err(AccessError::Authentication);
    }
    let decode = |value: &str| -> Result<serde_json::Value, AccessError> {
        serde_json::from_slice(
            &STANDARD
                .decode(value)
                .map_err(|_| AccessError::Authentication)?,
        )
        .map_err(|_| AccessError::Authentication)
    };
    if decode(header)? != decode(&payment_signature_header(terms, payment)?)? {
        return Err(AccessError::Authentication);
    }
    Ok(())
}

/// Standard challenge for this fixed negotiated payment. It is not a new offer.
pub fn payment_required_header(terms: &AgreementTerms) -> Result<String, AccessError> {
    Ok(STANDARD.encode(
        serde_json::to_vec(
            &serde_json::json!({"x402Version":2,"accepts":[payment_requirements(terms)?]}),
        )
        .map_err(|_| AccessError::Agreement)?,
    ))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PaymentRecord {
    version: u32,
    id: IssuanceId,
    authorization: Vec<u8>,
    plan: Option<Vec<u8>>,
    raw: Option<Vec<u8>>,
    attempted: bool,
}

impl JournalRecord for PaymentRecord {
    type Id = IssuanceId;
    const CURRENT_VERSION: u32 = 1;
    const OLDEST_READABLE_VERSION: u32 = 1;
    fn version(&self) -> u32 {
        self.version
    }
    fn record_id(&self) -> &Self::Id {
        &self.id
    }
    fn attempt_count(&self) -> usize {
        1
    }
}

/// Fixed seller gas policy. All users of this gas account must share its nonce journal.
pub struct Facilitator {
    primary: AccessBackend,
    peer: AccessBackend,
    key: TransactionKey,
    signer: SignerJournal,
    payments: Store<PaymentRecord>,
    fees: Eip1559Fees,
    gas_limit: u64,
}

impl Facilitator {
    /// The hash of the exact persisted transaction; does not itself prove payment.
    pub fn transaction_hash(&self, commitment: &str) -> Result<[u8; 32], AccessError> {
        if commitment.len() != 64
            || !commitment
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(AccessError::Authentication);
        }
        let id = IssuanceId(commitment.into());
        let _lock = self
            .payments
            .lock_record(&id)
            .map_err(|_| AccessError::Storage)?;
        let record = self
            .payments
            .read(&id)
            .map_err(|_| AccessError::Storage)?
            .ok_or(AccessError::Pending)?;
        if !record.attempted {
            return Err(AccessError::Pending);
        }
        Ok(
            SignedTransaction::from_raw(record.raw.as_deref().ok_or(AccessError::Storage)?)
                .map_err(|_| AccessError::Storage)?
                .hash(),
        )
    }
    /// Opens durable service state. Callers authenticate both chain runtimes before use.
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        primary: EvmChain,
        peer: EvmChain,
        key: TransactionKey,
        state_root: PathBuf,
        signer_root: PathBuf,
        fees: Eip1559Fees,
        gas_limit: u64,
    ) -> Result<Self, AccessError> {
        Self::with_faults(
            primary,
            peer,
            key,
            state_root,
            signer_root,
            fees,
            gas_limit,
            Arc::new(NoFaults),
        )
    }

    /// Opens with durable-write fault injection for crash recovery tests.
    #[allow(clippy::too_many_arguments)]
    pub fn with_faults(
        primary: EvmChain,
        peer: EvmChain,
        key: TransactionKey,
        state_root: PathBuf,
        signer_root: PathBuf,
        fees: Eip1559Fees,
        gas_limit: u64,
        faults: Arc<dyn FaultHook>,
    ) -> Result<Self, AccessError> {
        primary
            .check_peer(&peer)
            .map_err(|_| AccessError::Agreement)?;
        if primary.deployment().settlement_contract != EXACT_PERMIT2_PROXY
            || peer.deployment().settlement_contract != EXACT_PERMIT2_PROXY
            || primary.deployment().namespace != peer.deployment().namespace
            || primary.deployment().verifier_version != peer.deployment().verifier_version
            || gas_limit == 0
        {
            return Err(AccessError::Agreement);
        }
        let signer = SignerJournal::open(signer_root, primary.deployment().chain_id, key.address())
            .map_err(|_| AccessError::Storage)?;
        let payments =
            Store::open_with_fault_hook(state_root, faults).map_err(|_| AccessError::Storage)?;
        Ok(Self {
            primary: AccessBackend::X402Exact { chain: primary },
            peer: AccessBackend::X402Exact { chain: peer },
            key,
            signer,
            payments,
            fees,
            gas_limit,
        })
    }

    fn chains(&self) -> (&EvmChain, &EvmChain) {
        let AccessBackend::X402Exact { chain: primary } = &self.primary else {
            unreachable!()
        };
        let AccessBackend::X402Exact { chain: peer } = &self.peer else {
            unreachable!()
        };
        (primary, peer)
    }

    /// Validate, durably sign once, fence one broadcast, and verify finalized payment at both RPCs.
    /// After a fence exists, retries only observe. Timeout, rejection, or expiry never clears it.
    pub async fn settle_and_verify(
        &self,
        policy: &AccessPolicy,
        evidence: SelectedAgreement,
        payment: &X402Payment,
        now: u64,
    ) -> Result<PaidAccess, AccessError> {
        let agreement = policy.check(&evidence)?;
        let (chain, peer) = self.chains();
        chain
            .deployment()
            .matches_domain(&evidence.terms.domain)
            .map_err(|_| AccessError::Agreement)?;
        let permit = payment.permit(agreement.nullifier);
        let signature = payment.signature()?;
        validate_permit_fields(chain.deployment(), &evidence.terms, &permit)
            .map_err(|_| AccessError::Authentication)?;
        verify_permit_signature(chain.deployment(), &evidence.terms, &permit, &signature)
            .map_err(|_| AccessError::Authentication)?;
        let owner = evidence
            .terms
            .buyer_authorization_key
            .as_bytes()
            .try_into()
            .map_err(|_| AccessError::Authentication)?;
        let calldata = permit.encode_settle_call(&owner, &signature);
        let prepared = PreparedSettlement {
            operation_ref: *agreement.commitment.as_bytes(),
            deal_commitment: agreement.commitment,
            deal_nullifier: agreement.nullifier,
            domain: evidence.terms.domain.clone(),
            mode: evidence.terms.settlement_mode,
            required_guarantees: evidence.terms.required_guarantees,
            backend_evidence: calldata.clone(),
        };
        let id = IssuanceId(agreement.commitment.to_hex());
        let authorization = payment.encode();
        let existing = {
            let _lock = self
                .payments
                .lock_record(&id)
                .map_err(|_| AccessError::Storage)?;
            let saved = self.payments.read(&id).map_err(|_| AccessError::Storage)?;
            let record = saved.unwrap_or(PaymentRecord {
                version: 1,
                id: id.clone(),
                authorization: authorization.clone(),
                plan: None,
                raw: None,
                attempted: false,
            });
            if record.authorization != authorization || (record.attempted && record.raw.is_none()) {
                return Err(AccessError::Authentication);
            }
            self.payments
                .write(&record)
                .map_err(|_| AccessError::Storage)?;
            record
        };
        if !existing.attempted {
            if now >= permit.deadline || now < permit.valid_after {
                return Err(AccessError::Pending);
            }
            // Preflight before creating the durable broadcast fence. No approval is sent here.
            let estimate = chain
                .estimate_call_gas(self.key.address(), EXACT_PERMIT2_PROXY, &calldata)
                .await
                .map_err(|_| AccessError::Observation)?;
            let budget = self
                .fees
                .max_fee_per_gas()
                .checked_mul(u128::from(self.gas_limit))
                .ok_or(AccessError::Agreement)?;
            if estimate > self.gas_limit
                || chain
                    .gas_payer_balance(self.key.address())
                    .await
                    .map_err(|_| AccessError::Observation)?
                    < budget
            {
                return Err(AccessError::Pending);
            }
            let plan = chain
                .reserve_nonce_for_call(
                    &self.signer,
                    &prepared,
                    EXACT_PERMIT2_PROXY,
                    &calldata,
                    self.fees,
                    self.gas_limit,
                )
                .await
                .map_err(|_| AccessError::Storage)?;
            let signed = plan
                .sign_call(
                    chain.deployment().chain_id,
                    EXACT_PERMIT2_PROXY,
                    calldata.clone(),
                    &self.key,
                )
                .map_err(|_| AccessError::Authentication)?;
            let should_send = {
                let _lock = self
                    .payments
                    .lock_record(&id)
                    .map_err(|_| AccessError::Storage)?;
                let mut record = self
                    .payments
                    .read(&id)
                    .map_err(|_| AccessError::Storage)?
                    .ok_or(AccessError::Storage)?;
                if record.authorization != authorization {
                    return Err(AccessError::Authentication);
                }
                if let Some(saved) = &record.plan {
                    if saved != &plan.encode() {
                        return Err(AccessError::Storage);
                    }
                }
                if let Some(saved) = &record.raw {
                    if saved != signed.raw() {
                        return Err(AccessError::Storage);
                    }
                }
                record.plan = Some(plan.encode());
                record.raw = Some(signed.raw().to_vec());
                self.payments
                    .write(&record)
                    .map_err(|_| AccessError::Storage)?;
                if record.attempted {
                    false
                } else {
                    record.attempted = true;
                    self.payments
                        .write(&record)
                        .map_err(|_| AccessError::Storage)?;
                    true
                }
            };
            if should_send {
                // The fence survives cancellation or a lost response. Never automatically resend.
                let _ = chain
                    .broadcast_call(&plan, signed.raw(), EXACT_PERMIT2_PROXY, &calldata)
                    .await;
            }
        }
        let (plan, transaction) = {
            let _lock = self
                .payments
                .lock_record(&id)
                .map_err(|_| AccessError::Storage)?;
            let record = self
                .payments
                .read(&id)
                .map_err(|_| AccessError::Storage)?
                .ok_or(AccessError::Storage)?;
            if record.authorization != authorization || !record.attempted {
                return Err(AccessError::Pending);
            }
            let plan = SigningPlan::decode(record.plan.as_deref().ok_or(AccessError::Storage)?)
                .map_err(|_| AccessError::Storage)?;
            let raw = record.raw.ok_or(AccessError::Storage)?;
            plan.validate_call(
                chain.deployment().chain_id,
                EXACT_PERMIT2_PROXY,
                &calldata,
                &raw,
            )
            .map_err(|_| AccessError::Storage)?;
            if plan.sender() != self.key.address()
                || plan.params().fees != self.fees
                || plan.params().gas_limit != self.gas_limit
            {
                return Err(AccessError::Storage);
            }
            (
                plan,
                SignedTransaction::from_raw(&raw).map_err(|_| AccessError::Storage)?,
            )
        };
        let _ = plan;
        self.peer
            .verify_x402(
                policy,
                evidence.clone(),
                &permit,
                &signature,
                transaction.hash(),
            )
            .await?;
        let paid = self
            .primary
            .verify_x402(policy, evidence, &permit, &signature, transaction.hash())
            .await?;
        let nonce = chain
            .verified_finalized_nonce_agreed(peer, self.key.address())
            .await
            .map_err(|_| AccessError::Observation)?;
        match self.signer.resume_call(
            chain.deployment(),
            &prepared,
            EXACT_PERMIT2_PROXY,
            &calldata,
        ) {
            Ok(Some(_)) => self
                .signer
                .release(&nonce)
                .map_err(|_| AccessError::Storage)?,
            Ok(None) | Err(NonceError::Busy) => {}
            Err(_) => return Err(AccessError::Storage),
        }
        Ok(paid)
    }
}
