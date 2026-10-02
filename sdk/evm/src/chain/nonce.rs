//! One unresolved operation per chain and gas-paying sender, across deployments.
//!
//! This allocation boundary never clears a claim on timeout, local expiry, or an observed
//! pending nonce increase. Finalized reconciliation must be integrated before claims can clear.

use super::observation::FinalizedNonce;
use super::{settlement_calldata, Eip1559Fees, SigningPlan, TransactionParams};
use crate::{deployment::EvmDeployment, error::EvmError};
use alloy::primitives::keccak256;
use erebus_core::settlement::PreparedSettlement;
use erebus_journal::{FaultHook, JournalRecord, NoFaults, RecordId, Store};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Arc};

/// Nonce allocation refused. None of these errors establishes non-payment.
#[derive(Debug, thiserror::Error)]
pub enum NonceError {
    /// The state cannot be trusted. Restore or reconcile; do not reset the journal.
    #[error("signer journal unavailable or invalid; retain pending operation")]
    Storage,
    /// Another unresolved operation holds this sender on this chain.
    #[error("sender already has an unresolved operation on this chain")]
    Busy,
    /// An operation ID was reused with different payment or transaction parameters.
    #[error("nonce claim differs from the durable operation")]
    Conflict,
    /// Finalized evidence does not show the claimed nonce consumed.
    #[error("claimed nonce is not consumed at a finalized block")]
    NotConsumed,
    /// Agreement or transaction validation failed before allocation.
    #[error(transparent)]
    Invalid(#[from] EvmError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct AccountId(String);
impl std::fmt::Display for AccountId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl RecordId for AccountId {
    fn as_file_stem(&self) -> &str {
        &self.0
    }
    fn from_file_stem(stem: &str) -> Option<Self> {
        let (chain, sender) = stem.strip_prefix("eip155-")?.split_once('-')?;
        let parsed = chain.parse::<u64>().ok()?;
        if parsed.to_string() != chain
            || sender.len() != 40
            || !sender
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return None;
        }
        Some(Self(stem.into()))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Account {
    version: u32,
    id: AccountId,
    claim: Option<Claim>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Claim {
    operation_ref: [u8; 32],
    contract: [u8; 20],
    calldata_hash: [u8; 32],
    plan: Vec<u8>,
}
impl JournalRecord for Account {
    type Id = AccountId;
    const CURRENT_VERSION: u32 = 1;
    const OLDEST_READABLE_VERSION: u32 = 1;
    fn version(&self) -> u32 {
        self.version
    }
    fn record_id(&self) -> &AccountId {
        &self.id
    }
    fn attempt_count(&self) -> usize {
        1
    }
}

/// Durable single-operation gate for a gas-paying account.
/// All deployments and processes using that account must share this root.
/// A different root or an external wallet can bypass this local coordination.
pub struct SignerJournal {
    store: Store<Account>,
    id: AccountId,
    chain_id: u64,
    sender: [u8; 20],
}
impl std::fmt::Debug for SignerJournal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignerJournal").finish_non_exhaustive()
    }
}

impl SignerJournal {
    pub(crate) fn account(&self) -> (u64, [u8; 20]) {
        (self.chain_id, self.sender)
    }

    /// Gas-paying address for finalized nonce observation.
    #[must_use]
    pub const fn account_address(&self) -> [u8; 20] {
        self.sender
    }

    /// Opens an account record in a private journal shared across settlement deployments.
    pub fn open(
        root: impl AsRef<Path>,
        chain_id: u64,
        sender: [u8; 20],
    ) -> Result<Self, NonceError> {
        Self::open_with_faults(root, chain_id, sender, Arc::new(NoFaults))
    }

    /// Opens with deterministic storage failure injection.
    pub fn open_with_faults(
        root: impl AsRef<Path>,
        chain_id: u64,
        sender: [u8; 20],
        faults: Arc<dyn FaultHook>,
    ) -> Result<Self, NonceError> {
        let this = Self {
            store: Store::open_with_fault_hook(root.as_ref().to_path_buf(), faults)
                .map_err(|_| NonceError::Storage)?,
            id: AccountId(format!("eip155-{chain_id}-{}", hex::encode(sender))),
            chain_id,
            sender,
        };
        let _lock = this
            .store
            .lock_identity()
            .map_err(|_| NonceError::Storage)?;
        let marker = this.store.blob_path(&this.id, 0);
        if !marker.try_exists().map_err(|_| NonceError::Storage)?
            && !this
                .store
                .record_path(&this.id)
                .try_exists()
                .map_err(|_| NonceError::Storage)?
        {
            let mut state = Account {
                version: 1,
                id: this.id.clone(),
                claim: None,
            };
            this.store
                .write_blob_then_record::<erebus_journal::StoreError<AccountId>>(
                    &mut state,
                    0,
                    this.id.0.as_bytes(),
                    |_| Ok(()),
                )
                .map_err(|_| NonceError::Storage)?;
        }
        this.load()?;
        Ok(this)
    }

    /// Reserves an explicit observed nonce before transaction signing.
    /// The caller must obtain the nonce from the configured chain for this sender.
    /// This method does not perform RPC verification. Existing claims retain their nonce
    /// even if the supplied observation advances. Retries cannot change fees or gas.
    pub fn reserve(
        &self,
        deployment: &EvmDeployment,
        prepared: &PreparedSettlement,
        observed_nonce: u64,
        fees: Eip1559Fees,
        gas_limit: u64,
    ) -> Result<SigningPlan, NonceError> {
        let calldata = settlement_calldata(deployment, prepared)?;
        self.reserve_call(
            deployment,
            prepared,
            deployment.settlement_contract,
            &calldata,
            observed_nonce,
            fees,
            gas_limit,
        )
    }

    /// Reserves the shared gas-payer nonce for a backend-validated contract call.
    /// The backend must first verify the target and calldata against `prepared`.
    #[allow(clippy::too_many_arguments)]
    pub fn reserve_call(
        &self,
        deployment: &EvmDeployment,
        prepared: &PreparedSettlement,
        target: [u8; 20],
        calldata: &[u8],
        observed_nonce: u64,
        fees: Eip1559Fees,
        gas_limit: u64,
    ) -> Result<SigningPlan, NonceError> {
        if deployment.chain_id != self.chain_id || prepared.operation_ref == [0; 32] {
            return Err(NonceError::Conflict);
        }
        deployment.matches_domain(&prepared.domain)?;
        if target != deployment.settlement_contract {
            return Err(NonceError::Conflict);
        }
        let plan = SigningPlan::new(
            self.sender,
            TransactionParams {
                nonce: observed_nonce,
                fees,
                gas_limit,
            },
        )?;
        let _lock = self
            .store
            .lock_identity()
            .map_err(|_| NonceError::Storage)?;
        let mut state = self.load()?;
        if let Some(claim) = &state.claim {
            if claim.operation_ref != prepared.operation_ref {
                return Err(NonceError::Busy);
            }
            let saved = SigningPlan::decode(&claim.plan)?;
            if claim.contract != target
                || claim.calldata_hash != keccak256(calldata).0
                || saved.params().fees != fees
                || saved.params().gas_limit != gas_limit
            {
                return Err(NonceError::Conflict);
            }
            // Complete an uncertain prior rename before returning a usable claim.
            self.store.write(&state).map_err(|_| NonceError::Storage)?;
            return Ok(saved);
        }
        state.claim = Some(Claim {
            operation_ref: prepared.operation_ref,
            contract: target,
            calldata_hash: keccak256(calldata).0,
            plan: plan.encode(),
        });
        self.store.write(&state).map_err(|_| NonceError::Storage)?;
        Ok(plan)
    }

    /// Releases the claim once finalized evidence proves the claimed nonce was consumed.
    ///
    /// A claimed nonce can never be included once the account's finalized next-nonce is
    /// strictly greater, so a new operation may allocate. The observer may belong to any
    /// settlement deployment on the chain, because an account nonce is account-wide.
    /// This releases the signer slot only: it never proves payment or non-payment, and deal
    /// reconciliation is separate. Releasing with no claim is idempotent.
    pub fn release(&self, evidence: &FinalizedNonce) -> Result<(), NonceError> {
        if evidence.sender() != self.sender || evidence.chain_id() != self.chain_id {
            return Err(NonceError::Conflict);
        }
        let _lock = self
            .store
            .lock_identity()
            .map_err(|_| NonceError::Storage)?;
        let mut state = self.load()?;
        let Some(claim) = &state.claim else {
            return Ok(());
        };
        let plan = SigningPlan::decode(&claim.plan).map_err(|_| NonceError::Storage)?;
        if evidence.nonce() <= plan.params().nonce {
            return Err(NonceError::NotConsumed);
        }
        state.claim = None;
        self.store.write(&state).map_err(|_| NonceError::Storage)?;
        Ok(())
    }

    /// Recovers and syncs an existing claim without a new RPC nonce or remembered fee values.
    /// Returns `None` only when this account has no claim; other operations return `Busy`.
    pub fn resume(
        &self,
        deployment: &EvmDeployment,
        prepared: &PreparedSettlement,
    ) -> Result<Option<SigningPlan>, NonceError> {
        let calldata = settlement_calldata(deployment, prepared)?;
        self.resume_call(
            deployment,
            prepared,
            deployment.settlement_contract,
            &calldata,
        )
    }

    /// Restores a nonce claim after the backend revalidates the same call.
    pub fn resume_call(
        &self,
        deployment: &EvmDeployment,
        prepared: &PreparedSettlement,
        target: [u8; 20],
        calldata: &[u8],
    ) -> Result<Option<SigningPlan>, NonceError> {
        if deployment.chain_id != self.chain_id || prepared.operation_ref == [0; 32] {
            return Err(NonceError::Conflict);
        }
        deployment.matches_domain(&prepared.domain)?;
        if target != deployment.settlement_contract {
            return Err(NonceError::Conflict);
        }
        let _lock = self
            .store
            .lock_identity()
            .map_err(|_| NonceError::Storage)?;
        let state = self.load()?;
        let Some(claim) = &state.claim else {
            return Ok(None);
        };
        if claim.operation_ref != prepared.operation_ref {
            return Err(NonceError::Busy);
        }
        if claim.contract != target || claim.calldata_hash != keccak256(calldata).0 {
            return Err(NonceError::Conflict);
        }
        let plan = SigningPlan::decode(&claim.plan)?;
        self.store.write(&state).map_err(|_| NonceError::Storage)?;
        Ok(Some(plan))
    }

    fn load(&self) -> Result<Account, NonceError> {
        let marker_path = self.store.blob_path(&self.id, 0);
        let marker = std::fs::symlink_metadata(&marker_path).map_err(|_| NonceError::Storage)?;
        if !marker.is_file()
            || marker.len() != self.id.0.len() as u64
            || self
                .store
                .read_blob(&self.id, 0)
                .map_err(|_| NonceError::Storage)?
                != self.id.0.as_bytes()
        {
            return Err(NonceError::Storage);
        }
        let meta = std::fs::symlink_metadata(self.store.record_path(&self.id))
            .map_err(|_| NonceError::Storage)?;
        if !meta.is_file() || meta.len() > 4096 {
            return Err(NonceError::Storage);
        }
        let state = self
            .store
            .read(&self.id)
            .map_err(|_| NonceError::Storage)?
            .ok_or(NonceError::Storage)?;
        if let Some(claim) = &state.claim {
            let plan = SigningPlan::decode(&claim.plan).map_err(|_| NonceError::Storage)?;
            if claim.operation_ref == [0; 32] || plan.sender() != self.sender {
                return Err(NonceError::Storage);
            }
        }
        Ok(state)
    }
}
