use std::collections::BTreeSet;

use erebus_core::{
    auth::{verify_authorization_signature, Authorization, Role},
    commitment::{commit_agreement, deal_nullifier, CommitmentBlinding},
    deal_state::SignedRevision,
    policy::{Reservation, ReservationId, ReservationLedger, ReservationState},
    settlement::PreparedSettlement,
    terms::AgreementTerms,
};
use erebus_journal::{JournalRecord, RecordId};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::broadcast::{AttemptRecord, BroadcastOutcome, Replacement};
use crate::{Diagnostic, Error, Stage};

pub(crate) const MAX_STATE_BYTES: u64 = 16 * 1024 * 1024;
pub(crate) const MAX_EVIDENCE_BYTES: usize = 64 * 1024;
pub(crate) const MAX_TRANSACTION_BYTES: usize = 128 * 1024;
pub(crate) const MAX_TRANSACTION_PLAN_BYTES: usize = 1024;
pub(crate) const MAX_BROADCAST_ATTEMPTS: usize = 1024;
pub(crate) const MAX_REPLACEMENTS: usize = 16;
const MAX_INTENTS: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StateId;
impl std::fmt::Display for StateId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("coordinator")
    }
}
impl RecordId for StateId {
    fn as_file_stem(&self) -> &str {
        "coordinator"
    }
    fn from_file_stem(stem: &str) -> Option<Self> {
        (stem == "coordinator").then_some(Self)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct State {
    version: u32,
    id: StateId,
    pub(crate) buyer: Vec<u8>,
    pub(crate) intents: Vec<Intent>,
}

impl JournalRecord for State {
    type Id = StateId;
    const CURRENT_VERSION: u32 = 1;
    const OLDEST_READABLE_VERSION: u32 = 1;
    fn version(&self) -> u32 {
        self.version
    }
    fn record_id(&self) -> &StateId {
        &self.id
    }
    // This singleton record is the one identity-wide accounting generation.
    fn attempt_count(&self) -> usize {
        1
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Accounting {
    Reserved,
    Committed(u64),
    Released,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Intent {
    pub(crate) operation_ref: [u8; 32],
    pub(crate) terms: Vec<u8>,
    pub(crate) blinding: [u8; 32],
    created_at: u64,
    pub(crate) accounting: Accounting,
    pub(crate) authorization_started: bool,
    pub(crate) buyer_auth: Option<Vec<u8>>,
    pub(crate) seller_auth: Option<Vec<u8>>,
    pub(crate) prepared: Option<Vec<u8>>,
    #[serde(default)]
    pub(crate) signed_transaction: Option<Vec<u8>>,
    #[serde(default)]
    pub(crate) transaction_plan: Option<Vec<u8>>,
    #[serde(default)]
    pub(crate) broadcast_attempts: Vec<AttemptRecord>,
    #[serde(default)]
    pub(crate) replacements: Vec<Replacement>,
}

impl Drop for Intent {
    fn drop(&mut self) {
        self.terms.zeroize();
        self.blinding.zeroize();
        if let Some(bytes) = &mut self.buyer_auth {
            bytes.zeroize();
        }
        if let Some(bytes) = &mut self.seller_auth {
            bytes.zeroize();
        }
        if let Some(bytes) = &mut self.prepared {
            bytes.zeroize();
        }
        if let Some(bytes) = &mut self.signed_transaction {
            bytes.zeroize();
        }
        if let Some(bytes) = &mut self.transaction_plan {
            bytes.zeroize();
        }
    }
}

impl Intent {
    pub(crate) fn new(
        operation_ref: [u8; 32],
        terms: Vec<u8>,
        blinding: [u8; 32],
        created_at: u64,
    ) -> Self {
        Self {
            operation_ref,
            terms,
            blinding,
            created_at,
            accounting: Accounting::Reserved,
            authorization_started: false,
            buyer_auth: None,
            seller_auth: None,
            prepared: None,
            signed_transaction: None,
            transaction_plan: None,
            broadcast_attempts: Vec::new(),
            replacements: Vec::new(),
        }
    }

    pub(crate) fn opening(&self) -> Result<(AgreementTerms, CommitmentBlinding), Error> {
        Ok((
            AgreementTerms::decode(&self.terms).map_err(|_| Error::Storage)?,
            CommitmentBlinding::from_bytes(self.blinding),
        ))
    }

    pub(crate) fn revision(&self) -> Result<SignedRevision, Error> {
        let (terms, blinding) = self.opening()?;
        SignedRevision::from_opening(&terms, &blinding).map_err(|_| Error::Storage)
    }

    pub(crate) fn prepared_identity(&self) -> Result<PreparedSettlement, Error> {
        let (terms, blinding) = self.opening()?;
        Ok(PreparedSettlement {
            operation_ref: self.operation_ref,
            deal_commitment: commit_agreement(&terms, &blinding).map_err(|_| Error::Storage)?,
            deal_nullifier: deal_nullifier(&terms).map_err(|_| Error::Storage)?,
            domain: terms.domain,
            mode: terms.settlement_mode,
            required_guarantees: terms.required_guarantees,
            backend_evidence: Vec::new(),
        })
    }

    pub(crate) fn diagnostic(&self) -> Diagnostic {
        let unknown_attempts = self
            .broadcast_attempts
            .iter()
            .filter(|a| a.outcome == BroadcastOutcome::Unknown)
            .count();
        let stage = match self.accounting {
            Accounting::Committed(_) => Stage::Finalized,
            Accounting::Released => Stage::ClosedUnpaid,
            Accounting::Reserved if unknown_attempts > 0 => Stage::BroadcastUnknown,
            Accounting::Reserved
                if self
                    .broadcast_attempts
                    .iter()
                    .any(|a| a.outcome == BroadcastOutcome::Submitted) =>
            {
                Stage::Submitted
            }
            Accounting::Reserved if !self.broadcast_attempts.is_empty() => Stage::BroadcastUnknown,
            Accounting::Reserved if self.signed_transaction.is_some() => Stage::Signed,
            Accounting::Reserved if self.prepared.is_some() => Stage::Prepared,
            Accounting::Reserved if self.buyer_auth.is_some() && self.seller_auth.is_some() => {
                Stage::Authorized
            }
            Accounting::Reserved if self.buyer_auth.is_some() => Stage::BuyerAuthorized,
            Accounting::Reserved if self.authorization_started => Stage::AuthorizationUnknown,
            Accounting::Reserved => Stage::Reserved,
        };
        Diagnostic {
            operation_ref: self.operation_ref,
            stage,
            broadcast_attempts: self.broadcast_attempts.len(),
            unknown_attempts,
            last_broadcast_outcome: self.broadcast_attempts.last().map(|a| a.outcome),
            replacements: self.replacements.len(),
        }
    }
}

impl State {
    pub(crate) fn new(buyer: Vec<u8>) -> Self {
        Self {
            version: 1,
            id: StateId,
            buyer,
            intents: Vec::new(),
        }
    }

    pub(crate) fn index(&self, operation_ref: [u8; 32]) -> Result<usize, Error> {
        self.intents
            .iter()
            .position(|i| i.operation_ref == operation_ref)
            .ok_or(Error::Conflict)
    }

    pub(crate) fn validate(&self) -> Result<(), Error> {
        if self.version != 1 || self.intents.len() > MAX_INTENTS {
            return Err(Error::Storage);
        }
        let mut ids = BTreeSet::new();
        let mut tokens = BTreeSet::new();
        for intent in &self.intents {
            if intent.replacements.len() > MAX_REPLACEMENTS
                || (!intent.replacements.is_empty() && intent.signed_transaction.is_none())
            {
                return Err(Error::Storage);
            }
            for (index, replacement) in intent.replacements.iter().enumerate() {
                if replacement.plan.is_empty()
                    || replacement.plan.len() > MAX_TRANSACTION_PLAN_BYTES
                    || replacement
                        .raw
                        .as_ref()
                        .is_some_and(|r| r.is_empty() || r.len() > MAX_TRANSACTION_BYTES)
                    || (replacement.raw.is_none() && index + 1 != intent.replacements.len())
                    || intent.transaction_plan.as_ref() == Some(&replacement.plan)
                    || intent.replacements[..index]
                        .iter()
                        .any(|r| r.plan == replacement.plan)
                {
                    return Err(Error::Storage);
                }
            }
            if intent.broadcast_attempts.len() > MAX_BROADCAST_ATTEMPTS
                || (!intent.broadcast_attempts.is_empty() && intent.signed_transaction.is_none())
                || intent
                    .broadcast_attempts
                    .iter()
                    .any(|a| a.token == [0; 32] || !tokens.insert(a.token))
                || intent.broadcast_attempts.iter().any(|a| {
                    a.transaction_index > intent.replacements.len()
                        || (a.transaction_index > 0
                            && intent.replacements[a.transaction_index - 1].raw.is_none())
                })
            {
                return Err(Error::Storage);
            }
            if intent.operation_ref == [0; 32] || !ids.insert(intent.operation_ref) {
                return Err(Error::Storage);
            }
            let (terms, blinding) = intent.opening()?;
            if terms.buyer_authorization_key.as_bytes() != self.buyer
                || intent.created_at >= terms.expiry
            {
                return Err(Error::Storage);
            }
            let commitment = commit_agreement(&terms, &blinding).map_err(|_| Error::Storage)?;
            for (bytes, role) in [
                (&intent.buyer_auth, Role::Buyer),
                (&intent.seller_auth, Role::Seller),
            ] {
                if let Some(bytes) = bytes {
                    let auth = Authorization::decode(bytes).map_err(|_| Error::Storage)?;
                    if auth.role != role {
                        return Err(Error::Storage);
                    }
                    verify_authorization_signature(&terms, &commitment, &blinding, &auth)
                        .map_err(|_| Error::Storage)?;
                }
            }
            if intent.buyer_auth.is_some() && !intent.authorization_started {
                return Err(Error::Storage);
            }
            if let Some(bytes) = &intent.prepared {
                if bytes.is_empty()
                    || bytes.len() > MAX_EVIDENCE_BYTES
                    || intent.buyer_auth.is_none()
                    || intent.seller_auth.is_none()
                {
                    return Err(Error::Storage);
                }
            }
            if let Some(bytes) = &intent.signed_transaction {
                if bytes.is_empty()
                    || bytes.len() > MAX_TRANSACTION_BYTES
                    || intent.prepared.is_none()
                    || intent.transaction_plan.is_none()
                {
                    return Err(Error::Storage);
                }
            }
            if let Some(plan) = &intent.transaction_plan {
                if plan.is_empty()
                    || plan.len() > MAX_TRANSACTION_PLAN_BYTES
                    || intent.prepared.is_none()
                {
                    return Err(Error::Storage);
                }
            }
        }
        self.ledger()?;
        Ok(())
    }

    pub(crate) fn ledger(&self) -> Result<ReservationLedger, Error> {
        let mut entries = Vec::with_capacity(self.intents.len());
        for intent in &self.intents {
            let (terms, _) = intent.opening()?;
            let revision = intent.revision()?;
            let (state, settled_at) = match intent.accounting {
                Accounting::Reserved => (ReservationState::Reserved, None),
                Accounting::Committed(at) => (ReservationState::Committed, Some(at)),
                Accounting::Released => (ReservationState::Released, None),
            };
            entries.push(Reservation {
                id: ReservationId::for_revision(&revision.commitment()),
                asset: terms.asset,
                amount: revision.total(),
                state,
                created_at: intent.created_at,
                settled_at,
            });
        }
        ReservationLedger::restore(&entries).map_err(|_| Error::Storage)
    }

    pub(crate) fn apply_ledger(&mut self, ledger: &ReservationLedger) -> Result<(), Error> {
        if self.intents.len() != ledger.reservations().len() {
            return Err(Error::Storage);
        }
        for (intent, entry) in self.intents.iter_mut().zip(ledger.reservations()) {
            if ReservationId::for_revision(&intent.revision()?.commitment()) != entry.id {
                return Err(Error::Storage);
            }
            intent.accounting = match (entry.state, entry.settled_at) {
                (ReservationState::Reserved, None) => Accounting::Reserved,
                (ReservationState::Released, None) => Accounting::Released,
                (ReservationState::Committed, Some(at)) => Accounting::Committed(at),
                _ => return Err(Error::Storage),
            };
        }
        Ok(())
    }
}
