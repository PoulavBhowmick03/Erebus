//! Durable intent, authorization fencing, and spending reservations.
//!
//! One private local directory belongs to one buyer identity across its sessions. Every
//! writer holds the journal identity lock. Canonical intent and its reservation are one
//! atomic record, committed before a signing or proving callback can run. An uncertain
//! signature or payment never releases capacity. This crate does not yet broadcast or
//! independently verify chain evidence; its caller must provide verified backend facts.
#![forbid(unsafe_code)]

mod broadcast;
mod record;

pub use broadcast::{
    AttemptDiagnostic, AttemptToken, BroadcastAttempt, BroadcastOutcome, SignedTransaction,
};

use std::{io::Read, path::Path, sync::Arc};

use erebus_core::{
    auth::{verify_authorization_signature, Authorization, Role},
    commitment::{commit_agreement, CommitmentBlinding, DealCommitment},
    deal_state::{DealAssessment, DealEvidence, SignedRevision},
    ids::{KeyBytes, MAX_KEY_BYTES},
    policy::{ReservationId, ReservationLedger, SpendingPolicy},
    settlement::{check_capabilities, BackendCapabilities, PreparedSettlement, SettlementContext},
    terms::AgreementTerms,
};
use erebus_journal::{FaultHook, NoFaults, Store};

use record::{Intent, State, StateId, MAX_STATE_BYTES};

/// A local coordinator operation failed. Diagnostics contain no private terms or signatures.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Storage failed or contains invalid data. Do not treat this as an unpaid operation.
    #[error("coordinator storage is unavailable or invalid; keep reservations and reconcile")]
    Storage,
    /// Peer, backend, buyer identity, or prepared evidence does not match the fixed context.
    #[error("settlement context or buyer identity does not match")]
    Context,
    /// Canonical terms, commitment, or signature is invalid.
    #[error("agreement or authorization is invalid")]
    Agreement,
    /// An operation ID is missing, duplicated, or reused for different intent.
    #[error("operation identity conflicts with durable intent")]
    Conflict,
    /// The policy refuses the reservation or reconciliation is inconsistent.
    #[error("spending reservation denied or inconsistent; existing reservations retained")]
    Policy,
    /// A callback cannot run from the current durable stage.
    #[error("operation stage does not permit this action")]
    Stage,
    /// The revision has expired. This is not evidence of non-payment.
    #[error("revision expired locally; reconcile before releasing capacity")]
    Expired,
    /// A local signer or prover failed. Its reservation remains held.
    #[error("local signer or preparation failed; reservation retained")]
    Backend,
}

/// Redacted progress of a local revision. A stage is not proof of payment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Policy approved and intent is durable; no signer callback has started.
    Reserved,
    /// A signer may have run. Reopening cannot assume no authorization exists.
    AuthorizationUnknown,
    /// The buyer authorization is durable; the seller authorization is still missing.
    BuyerAuthorized,
    /// Both authorizations are verified and durable.
    Authorized,
    /// Backend evidence is durable; no signed transaction is stored yet.
    Prepared,
    /// Signed transaction bytes are durable. This is not evidence of broadcast or payment.
    Signed,
    /// At least one submission may have reached the network without a known response.
    BroadcastUnknown,
    /// A backend acknowledged submission. This is not proof of payment or finality.
    Submitted,
    /// Verified backend evidence finalized this payment and committed its reservation.
    Finalized,
    /// Verified backend evidence released the reservation (expired or superseded).
    ClosedUnpaid,
}

/// Redacted status safe for an operator diagnostic, without agreement terms or key material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Diagnostic {
    /// Local operation ID, not a payment or onchain identity.
    pub operation_ref: [u8; 32],
    /// Durable progress.
    pub stage: Stage,
    /// Number of retained broadcast attempts, including uncertain attempts.
    pub broadcast_attempts: usize,
    /// Attempts without a conclusive local response. These survive retries.
    pub unknown_attempts: usize,
    /// Last local response, without hashes, payloads, or backend error text.
    pub last_broadcast_outcome: Option<BroadcastOutcome>,
    /// Retained replacement plans, including a fenced plan awaiting signed bytes.
    pub replacements: usize,
}

/// One fixed settlement session backed by an identity-wide durable state store.
///
/// The caller must obtain `peer_context` through its authenticated peer transport.
/// Equality here does not authenticate an arbitrary context supplied by an application.
pub struct Coordinator {
    store: Store<State>,
    buyer: KeyBytes,
    context: SettlementContext,
    policy: SpendingPolicy,
}

impl std::fmt::Debug for Coordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Coordinator").finish_non_exhaustive()
    }
}

impl Coordinator {
    /// Opens a fixed session, refusing backend downgrade or a differing peer context.
    /// All sessions for this buyer must share the same private root to share policy limits.
    pub fn open(
        root: impl AsRef<Path>,
        buyer: KeyBytes,
        context: SettlementContext,
        peer_context: &SettlementContext,
        capabilities: &BackendCapabilities,
        policy: SpendingPolicy,
    ) -> Result<Self, Error> {
        Self::open_with_faults(
            root,
            buyer,
            context,
            peer_context,
            capabilities,
            policy,
            Arc::new(NoFaults),
        )
    }

    /// Opens with deterministic failure injection at each durable storage boundary.
    #[allow(clippy::too_many_arguments)]
    pub fn open_with_faults(
        root: impl AsRef<Path>,
        buyer: KeyBytes,
        context: SettlementContext,
        peer_context: &SettlementContext,
        capabilities: &BackendCapabilities,
        policy: SpendingPolicy,
        faults: Arc<dyn FaultHook>,
    ) -> Result<Self, Error> {
        if &context != peer_context {
            return Err(Error::Context);
        }
        check_capabilities(&context, capabilities).map_err(|_| Error::Context)?;
        let store = Store::open_with_fault_hook(root.as_ref().to_path_buf(), faults)
            .map_err(|_| Error::Storage)?;
        let this = Self {
            store,
            buyer,
            context,
            policy,
        };
        let _lock = this.store.lock_identity().map_err(|_| Error::Storage)?;
        let marker = this.store.blob_path(&StateId, 0);
        if !this
            .store
            .record_path(&StateId)
            .try_exists()
            .map_err(|_| Error::Storage)?
            && !marker.try_exists().map_err(|_| Error::Storage)?
        {
            let mut state = State::new(this.buyer.as_bytes().to_vec());
            this.store
                .write_blob_then_record::<erebus_journal::StoreError<StateId>>(
                    &mut state,
                    0,
                    &this.initialization_marker(),
                    |_| Ok(()),
                )
                .map_err(|_| Error::Storage)?;
        }
        this.load()?;
        Ok(this)
    }

    /// Reserves policy capacity and persists the exact agreement opening before signing.
    /// Repeating the same operation and opening is idempotent, even after a restart.
    pub fn record_intent(
        &self,
        operation_ref: [u8; 32],
        terms: &AgreementTerms,
        blinding: &CommitmentBlinding,
        now: u64,
    ) -> Result<DealCommitment, Error> {
        self.check_terms(terms)?;
        if operation_ref == [0; 32] {
            return Err(Error::Conflict);
        }
        let commitment = commit_agreement(terms, blinding).map_err(|_| Error::Agreement)?;
        let encoded = terms.encode().map_err(|_| Error::Agreement)?;
        let _lock = self.store.lock_identity().map_err(|_| Error::Storage)?;
        let mut state = self.load()?;
        if let Some(existing) = state
            .intents
            .iter()
            .find(|i| i.operation_ref == operation_ref)
        {
            if existing.terms != encoded || &existing.blinding != blinding.as_bytes() {
                return Err(Error::Conflict);
            }
            return Ok(commitment);
        }
        if now >= terms.expiry {
            return Err(Error::Expired);
        }
        let mut ledger = state.ledger()?;
        ledger
            .reserve_agreement(
                ReservationId::for_revision(&commitment),
                &self.policy,
                terms,
                &terms.seller_authorization_key,
                now,
            )
            .map_err(|_| Error::Policy)?;
        state.intents.push(Intent::new(
            operation_ref,
            encoded,
            *blinding.as_bytes(),
            now,
        ));
        self.save(&state)?;
        Ok(commitment)
    }

    /// Persists an authorization fence before invoking a local buyer signer.
    /// If signing or persistence fails, capacity remains reserved. Reopening never retries
    /// an uncertain signer automatically; the caller explicitly invokes this method again.
    pub fn authorize_buyer<E>(
        &self,
        operation_ref: [u8; 32],
        now: u64,
        signer: impl FnOnce(&AgreementTerms, &CommitmentBlinding) -> Result<Authorization, E>,
    ) -> Result<Authorization, Error> {
        let _lock = self.store.lock_identity().map_err(|_| Error::Storage)?;
        let mut state = self.load()?;
        let index = state.index(operation_ref)?;
        let (terms, blinding) = state.intents[index].opening()?;
        self.check_terms(&terms)?;
        if now >= terms.expiry {
            return Err(Error::Expired);
        }
        if state.intents[index].accounting != record::Accounting::Reserved {
            return Err(Error::Stage);
        }
        if let Some(bytes) = &state.intents[index].buyer_auth {
            return Authorization::decode(bytes).map_err(|_| Error::Storage);
        }
        state.intents[index].authorization_started = true;
        self.save(&state)?;
        let auth = signer(&terms, &blinding).map_err(|_| Error::Backend)?;
        self.check_authorization(&terms, &blinding, &auth, Role::Buyer)?;
        state.intents[index].buyer_auth = Some(auth.encode().map_err(|_| Error::Agreement)?);
        self.save(&state)?;
        Ok(auth)
    }

    /// Attaches the seller's authorization to an existing, policy-reserved intent.
    pub fn accept_seller(
        &self,
        operation_ref: [u8; 32],
        auth: &Authorization,
    ) -> Result<(), Error> {
        let _lock = self.store.lock_identity().map_err(|_| Error::Storage)?;
        let mut state = self.load()?;
        let index = state.index(operation_ref)?;
        let (terms, blinding) = state.intents[index].opening()?;
        self.check_terms(&terms)?;
        if state.intents[index].accounting != record::Accounting::Reserved {
            return Err(Error::Stage);
        }
        self.check_authorization(&terms, &blinding, auth, Role::Seller)?;
        let bytes = auth.encode().map_err(|_| Error::Agreement)?;
        if state.intents[index]
            .seller_auth
            .as_ref()
            .is_some_and(|old| old != &bytes)
        {
            return Err(Error::Conflict);
        }
        state.intents[index].seller_auth = Some(bytes);
        self.save(&state)
    }

    /// Calls the selected local backend only after durable intent and both authorizations.
    /// No witness belongs in returned backend evidence. That contract is backend-owned;
    /// the coordinator checks routing and identity, not proof validity or chain state.
    pub fn prepare<E>(
        &self,
        operation_ref: [u8; 32],
        now: u64,
        backend: impl FnOnce(
            &AgreementTerms,
            &CommitmentBlinding,
            &Authorization,
            &Authorization,
        ) -> Result<PreparedSettlement, E>,
    ) -> Result<PreparedSettlement, Error> {
        let _lock = self.store.lock_identity().map_err(|_| Error::Storage)?;
        let mut state = self.load()?;
        let index = state.index(operation_ref)?;
        let intent = &state.intents[index];
        let (terms, blinding) = intent.opening()?;
        self.check_terms(&terms)?;
        if intent.accounting != record::Accounting::Reserved {
            return Err(Error::Stage);
        }
        if now >= terms.expiry {
            return Err(Error::Expired);
        }
        let buyer = Authorization::decode(intent.buyer_auth.as_ref().ok_or(Error::Stage)?)
            .map_err(|_| Error::Storage)?;
        let seller = Authorization::decode(intent.seller_auth.as_ref().ok_or(Error::Stage)?)
            .map_err(|_| Error::Storage)?;
        let expected = intent.prepared_identity()?;
        if let Some(evidence) = &intent.prepared {
            return Ok(PreparedSettlement {
                backend_evidence: evidence.clone(),
                ..expected
            });
        }
        let prepared = backend(&terms, &blinding, &buyer, &seller).map_err(|_| Error::Backend)?;
        if prepared.operation_ref != expected.operation_ref
            || prepared.domain != expected.domain
            || prepared.mode != expected.mode
            || prepared.required_guarantees != expected.required_guarantees
            || prepared.deal_commitment != expected.deal_commitment
            || prepared.deal_nullifier != expected.deal_nullifier
        {
            return Err(Error::Context);
        }
        if prepared.backend_evidence.is_empty()
            || prepared.backend_evidence.len() > record::MAX_EVIDENCE_BYTES
        {
            return Err(Error::Agreement);
        }
        state.intents[index].prepared = Some(prepared.backend_evidence.clone());
        self.save(&state)?;
        Ok(prepared)
    }

    /// Persists one immutable signed transaction before returning bytes for broadcast.
    ///
    /// Both callbacks must be local: neither may broadcast. The backend validator must check
    /// the complete transaction against `prepared` and trusted sender, nonce, and fee intent.
    /// This crate checks size and storage ordering, not backend transaction semantics.
    /// `plan` is immutable backend metadata, persisted before the local signer runs.
    /// The callbacks receive the durable plan, not reconstructed caller parameters.
    /// On retry, the validator runs again but the signer does not. No replacement is allowed.
    /// A storage error can occur after rename: reload and validate, never assume no transaction.
    /// Expired or closed operations cannot use this method; reconcile them instead.
    pub fn sign_transaction<E>(
        &self,
        operation_ref: [u8; 32],
        now: u64,
        plan: &[u8],
        signer: impl FnOnce(&PreparedSettlement, &[u8]) -> Result<Vec<u8>, E>,
        validate: impl FnOnce(&PreparedSettlement, &[u8], &[u8]) -> Result<(), E>,
    ) -> Result<Vec<u8>, Error> {
        let _lock = self.store.lock_identity().map_err(|_| Error::Storage)?;
        let mut state = self.load()?;
        let index = state.index(operation_ref)?;
        let intent = &state.intents[index];
        let (terms, _) = intent.opening()?;
        self.check_terms(&terms)?;
        if intent.accounting != record::Accounting::Reserved {
            return Err(Error::Stage);
        }
        if now >= terms.expiry {
            return Err(Error::Expired);
        }
        let prepared = PreparedSettlement {
            backend_evidence: intent.prepared.clone().ok_or(Error::Stage)?,
            ..intent.prepared_identity()?
        };
        if plan.is_empty() || plan.len() > record::MAX_TRANSACTION_PLAN_BYTES {
            return Err(Error::Agreement);
        }
        if intent
            .transaction_plan
            .as_ref()
            .is_some_and(|old| old != plan)
        {
            return Err(Error::Conflict);
        }
        state.intents[index].transaction_plan = Some(plan.to_vec());
        // Sync before any signing callback, including retries after an uncertain rename.
        self.save(&state)?;
        let intent = &state.intents[index];
        let plan = intent.transaction_plan.as_deref().ok_or(Error::Storage)?;
        if let Some(raw) = &intent.signed_transaction {
            validate(&prepared, plan, raw).map_err(|_| Error::Backend)?;
            return Ok(raw.clone());
        }
        let raw = signer(&prepared, plan).map_err(|_| Error::Backend)?;
        if raw.is_empty() || raw.len() > record::MAX_TRANSACTION_BYTES {
            return Err(Error::Agreement);
        }
        validate(&prepared, plan, &raw).map_err(|_| Error::Backend)?;
        state.intents[index].signed_transaction = Some(raw.clone());
        self.save(&state)?;
        Ok(raw)
    }

    /// Reads immutable backend signing metadata, including after local expiry or closure.
    /// This is private recovery data, not an operator diagnostic or payment proof.
    pub fn transaction_plan(&self, operation_ref: [u8; 32]) -> Result<Option<Vec<u8>>, Error> {
        let _lock = self.store.lock_identity().map_err(|_| Error::Storage)?;
        let state = self.load()?;
        let intent = &state.intents[state.index(operation_ref)?];
        self.check_terms(&intent.opening()?.0)?;
        Ok(intent.transaction_plan.clone())
    }

    /// Restores a prepared transition after restart without invoking the backend again.
    /// This is private recovery data, not an operator diagnostic or payment receipt.
    pub fn prepared_settlement(
        &self,
        operation_ref: [u8; 32],
    ) -> Result<PreparedSettlement, Error> {
        let _lock = self.store.lock_identity().map_err(|_| Error::Storage)?;
        let state = self.load()?;
        let intent = &state.intents[state.index(operation_ref)?];
        self.check_terms(&intent.opening()?.0)?;
        Ok(PreparedSettlement {
            backend_evidence: intent.prepared.clone().ok_or(Error::Stage)?,
            ..intent.prepared_identity()?
        })
    }

    /// Returns the accepted context and locally recorded openings for one deal.
    ///
    /// This contains private payment terms; callers must not log or expose it as diagnostics.
    /// Chain observers use these openings only to identify an already verified pool event.
    pub fn recorded_deal(
        &self,
        operation_ref: [u8; 32],
    ) -> Result<(SettlementContext, Vec<SignedRevision>), Error> {
        let _lock = self.store.lock_identity().map_err(|_| Error::Storage)?;
        let state = self.load()?;
        let target = state.intents[state.index(operation_ref)?]
            .revision()?
            .deal_nullifier();
        let mut revisions = Vec::new();
        for intent in &state.intents {
            let (terms, blinding) = intent.opening()?;
            self.check_terms(&terms)?;
            let revision =
                SignedRevision::from_opening(&terms, &blinding).map_err(|_| Error::Storage)?;
            if revision.deal_nullifier() == target {
                revisions.push(revision);
            }
        }
        Ok((self.context.clone(), revisions))
    }

    /// Reconciles every locally recorded revision of the operation's deal in one write.
    /// Only independently verified backend reads may enter here. A caller-provided receipt
    /// flag, timeout, or local expiry is not valid evidence of payment or non-payment.
    pub fn reconcile(
        &self,
        operation_ref: [u8; 32],
        evidence: &DealEvidence,
        now: u64,
    ) -> Result<DealAssessment, Error> {
        let _lock = self.store.lock_identity().map_err(|_| Error::Storage)?;
        let mut state = self.load()?;
        let index = state.index(operation_ref)?;
        self.check_terms(&state.intents[index].opening()?.0)?;
        let target = state.intents[index].revision()?.deal_nullifier();
        let revisions = state
            .intents
            .iter()
            .map(Intent::revision)
            .collect::<Result<Vec<_>, _>>()?;
        let revisions: Vec<SignedRevision> = revisions
            .into_iter()
            .filter(|r| r.deal_nullifier() == target)
            .collect();
        let mut ledger = state.ledger()?;
        let assessment = ledger
            .reconcile_deal(&revisions, evidence, now)
            .map_err(|_| Error::Policy)?;
        state.apply_ledger(&ledger)?;
        self.save(&state)?;
        Ok(assessment)
    }

    /// Reads a complete, checked policy snapshot under the identity lock.
    pub fn ledger(&self) -> Result<ReservationLedger, Error> {
        let _lock = self.store.lock_identity().map_err(|_| Error::Storage)?;
        self.load()?.ledger()
    }

    /// Reads redacted progress. This exposes neither terms nor backend calldata.
    pub fn diagnostics(&self) -> Result<Vec<Diagnostic>, Error> {
        let _lock = self.store.lock_identity().map_err(|_| Error::Storage)?;
        Ok(self
            .load()?
            .intents
            .iter()
            .map(Intent::diagnostic)
            .collect())
    }

    fn check_terms(&self, terms: &AgreementTerms) -> Result<(), Error> {
        terms.validate().map_err(|_| Error::Agreement)?;
        if terms.buyer_authorization_key != self.buyer
            || terms.domain != self.context.domain
            || terms.suite_id != self.context.suite_id
            || terms.asset != self.context.asset
            || terms.settlement_mode != self.context.mode
            || terms.required_guarantees != self.context.required_guarantees
        {
            return Err(Error::Context);
        }
        Ok(())
    }

    fn check_authorization(
        &self,
        terms: &AgreementTerms,
        blinding: &CommitmentBlinding,
        auth: &Authorization,
        role: Role,
    ) -> Result<(), Error> {
        if auth.role != role {
            return Err(Error::Agreement);
        }
        let commitment = commit_agreement(terms, blinding).map_err(|_| Error::Agreement)?;
        verify_authorization_signature(terms, &commitment, blinding, auth)
            .map_err(|_| Error::Agreement)
    }

    fn load(&self) -> Result<State, Error> {
        let marker = self.initialization_marker();
        let meta = std::fs::symlink_metadata(self.store.blob_path(&StateId, 0))
            .map_err(|_| Error::Storage)?;
        if !meta.is_file()
            || meta.len() != marker.len() as u64
            || self
                .store
                .read_blob(&StateId, 0)
                .map_err(|_| Error::Storage)?
                != marker
        {
            return Err(Error::Storage);
        }
        let path = self.store.record_path(&StateId);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if !meta.is_file() || meta.len() > MAX_STATE_BYTES => {
                return Err(Error::Storage)
            }
            Ok(_) => {}
            Err(_) => return Err(Error::Storage),
        }
        let state = self
            .store
            .read(&StateId)
            .map_err(|_| Error::Storage)?
            .ok_or(Error::Storage)?;
        if state.buyer != self.buyer.as_bytes() {
            return Err(Error::Context);
        }
        state.validate()?;
        Ok(state)
    }

    fn initialization_marker(&self) -> Vec<u8> {
        [
            b"EREBUS_COORDINATOR_STATE_V1".as_slice(),
            self.buyer.as_bytes(),
        ]
        .concat()
    }

    fn save(&self, state: &State) -> Result<(), Error> {
        state.validate()?;
        if serde_json::to_vec(state).map_err(|_| Error::Storage)?.len() as u64 > MAX_STATE_BYTES {
            return Err(Error::Storage);
        }
        self.store.write(state).map_err(|_| Error::Storage)
    }
}

/// Prefix of the initialization marker, before the buyer key bytes.
const INITIALIZATION_MARKER_PREFIX: &[u8] = b"EREBUS_COORDINATOR_STATE_V1";

/// Reads the durable opening and both authorizations for one recorded revision.
///
/// This recovery accessor needs no session configuration and never queries or submits to a
/// chain. It does not write snapshots, but opening the existing private directory tightens its
/// permissions and acquires a journal lock. A missing marker, buyer mismatch, or invalid
/// snapshot fails closed. The caller must control the directory and its parent directories.
///
/// The result is disclosure material: terms, blinding, and signatures. Snapshot validation
/// verifies stored authorization signatures. `SelectedAgreement::from_store` also verifies
/// their binding to the retained transcript. Callers must encrypt the result before export
/// and must never log it or expose it as a diagnostic.
pub fn read_disclosure_opening(
    root: impl AsRef<Path>,
    operation_ref: [u8; 32],
) -> Result<
    (
        AgreementTerms,
        CommitmentBlinding,
        Authorization,
        Authorization,
    ),
    Error,
> {
    let root = root.as_ref();
    if !std::fs::symlink_metadata(root)
        .map_err(|_| Error::Storage)?
        .is_dir()
    {
        return Err(Error::Storage);
    }
    let store: Store<State> = Store::open(root.to_path_buf()).map_err(|_| Error::Storage)?;
    let _lock = store.lock_identity().map_err(|_| Error::Storage)?;
    let marker_path = store.blob_path(&StateId, 0);
    let maximum_marker_bytes = (INITIALIZATION_MARKER_PREFIX.len() + MAX_KEY_BYTES) as u64;
    // Check before opening: a FIFO must not block recovery, and a corrupt blob must not
    // cause an unbounded allocation. Recheck the opened file and bound the actual read too.
    let blob = std::fs::symlink_metadata(&marker_path).map_err(|_| Error::Storage)?;
    if !blob.is_file() || blob.len() > maximum_marker_bytes {
        return Err(Error::Storage);
    }
    let file = std::fs::File::open(marker_path).map_err(|_| Error::Storage)?;
    let opened = file.metadata().map_err(|_| Error::Storage)?;
    if !opened.is_file() || opened.len() != blob.len() {
        return Err(Error::Storage);
    }
    let mut marker = Vec::new();
    file.take(maximum_marker_bytes + 1)
        .read_to_end(&mut marker)
        .map_err(|_| Error::Storage)?;
    if marker.len() <= INITIALIZATION_MARKER_PREFIX.len()
        || marker.len() as u64 != opened.len()
        || &marker[..INITIALIZATION_MARKER_PREFIX.len()] != INITIALIZATION_MARKER_PREFIX
    {
        return Err(Error::Storage);
    }
    let path = store.record_path(&StateId);
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if !meta.is_file() || meta.len() > MAX_STATE_BYTES => return Err(Error::Storage),
        Ok(_) => {}
        Err(_) => return Err(Error::Storage),
    }
    let state = store
        .read(&StateId)
        .map_err(|_| Error::Storage)?
        .ok_or(Error::Storage)?;
    if state.buyer.as_slice() != &marker[INITIALIZATION_MARKER_PREFIX.len()..] {
        return Err(Error::Context);
    }
    state.validate()?;
    let intent = &state.intents[state.index(operation_ref)?];
    let (terms, blinding) = intent.opening()?;
    let buyer = Authorization::decode(intent.buyer_auth.as_ref().ok_or(Error::Stage)?)
        .map_err(|_| Error::Storage)?;
    let seller = Authorization::decode(intent.seller_auth.as_ref().ok_or(Error::Stage)?)
        .map_err(|_| Error::Storage)?;
    Ok((terms, blinding, buyer, seller))
}
