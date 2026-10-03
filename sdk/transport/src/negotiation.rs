//! Typed offchain negotiation and the boundary before final authorizations.
//!
//! Profile 1 negotiates price for an otherwise fixed service agreement. Offers and counters
//! contain canonical terms with a zero transcript root. Both participants then append an
//! acceptance of the same proposal digest. Only after both accept is the resulting transcript
//! sealed and its root inserted into the final terms. Final signatures travel separately:
//! including them in the root they sign would create a hash cycle.
//!
//! This layer interprets authenticated messages; it does not authenticate a socket, reserve
//! funds, or sign payments. Those remain transport and settlement-coordinator responsibilities.

use core::fmt;
use erebus_core::{
    auth::{verify_authorization_signature, Authorization, Role},
    commitment::{commit_agreement, CommitmentBlinding, DealCommitment},
    encoding::{Reader, Writer},
    ids::BaseUnits,
    terms::AgreementTerms,
};

use crate::{
    hashing::{self, TRANSCRIPT_HASH_VERSION},
    limits::MAX_BODY_BYTES,
    message::{Message, MessageType},
    store::{FileTranscriptStore, StoreError, TranscriptStore},
    transcript::Transcript,
};

const PROFILE_VERSION: u16 = 1;
const PROPOSAL_DOMAIN: &[u8] = b"EREBUS_NEGOTIATION_PROPOSAL_V1";

/// A negotiation input or durable record failed validation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NegotiationError {
    /// Invalid canonical bytes, terms, suite mapping, or nonzero draft root.
    #[error("invalid negotiation proposal")]
    Proposal,
    /// A proposal changed fields outside this price-only session.
    #[error("proposal changes the fixed agreement context")]
    Context,
    /// A stale, out-of-turn, mismatched, or post-acceptance transition.
    #[error("invalid negotiation transition")]
    Transition,
    /// The session's agreement deadline has passed.
    #[error("negotiation agreement has expired")]
    Expired,
    /// The caller requested final terms before bilateral acceptance.
    #[error("negotiation is not frozen")]
    NotFrozen,
    /// A detached final signature does not authorize this frozen agreement.
    #[error("final authorization does not match the frozen agreement")]
    Authorization,
    /// Persistence or transcript validation failed.
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Canonical proposed terms and their private commitment opening.
#[derive(Clone, PartialEq, Eq)]
pub struct Proposal {
    terms: AgreementTerms,
    blinding: CommitmentBlinding,
}

impl fmt::Debug for Proposal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Proposal(<redacted>)")
    }
}

impl Proposal {
    /// Checks a draft before it enters a negotiation session.
    pub fn new(
        terms: AgreementTerms,
        blinding: CommitmentBlinding,
    ) -> Result<Self, NegotiationError> {
        if terms.transcript_root != [0; 32] {
            return Err(NegotiationError::Proposal);
        }
        commit_agreement(&terms, &blinding).map_err(|_| NegotiationError::Proposal)?;
        let proposal = Self { terms, blinding };
        if proposal.encode()?.len() > MAX_BODY_BYTES - 40 {
            return Err(NegotiationError::Proposal);
        }
        Ok(proposal)
    }

    /// Proposed terms; they remain private until a deliberate disclosure.
    #[must_use]
    pub fn terms(&self) -> &AgreementTerms {
        &self.terms
    }

    /// Shared private blinding, used by the eventual accepted commitment.
    #[must_use]
    pub fn blinding(&self) -> &CommitmentBlinding {
        &self.blinding
    }

    /// Canonical private bytes, not a model-visible diagnostic.
    pub fn encode(&self) -> Result<Vec<u8>, NegotiationError> {
        let terms = self
            .terms
            .encode()
            .map_err(|_| NegotiationError::Proposal)?;
        if terms.len() > MAX_BODY_BYTES - 74 {
            return Err(NegotiationError::Proposal);
        }
        let mut writer = Writer::new();
        writer.bytes_bounded(&terms, MAX_BODY_BYTES - 74);
        writer.fixed(self.blinding.as_bytes());
        Ok(writer.finish())
    }

    /// Decodes strictly, including rejection of trailing data.
    pub fn decode(bytes: &[u8]) -> Result<Self, NegotiationError> {
        let result = (|| {
            let mut reader = Reader::new(bytes);
            let terms = AgreementTerms::decode(reader.bytes("terms", 1, MAX_BODY_BYTES - 74)?)
                .map_err(|_| NegotiationError::Proposal)?;
            let blinding = CommitmentBlinding::from_bytes(reader.fixed("blinding")?);
            reader.finish()?;
            Self::new(terms, blinding)
        })();
        result
    }

    /// Recovers a draft from an authenticated first buyer offer, without a shared draft file.
    /// This checks canonical bytes and the initial transition, not socket authentication.
    /// The caller must still check its service policy and deployment before persisting it.
    pub fn from_initial_offer(message: &Message, now: u64) -> Result<Self, NegotiationError> {
        if message.author != Role::Buyer
            || message.message_type != MessageType::Offer
            || message.revision != 1
            || message.sequence != 1
            || message.parent_hash != [0; 32]
        {
            return Err(NegotiationError::Transition);
        }
        let Event::Proposal { parent, proposal } = Event::decode(&message.body)? else {
            return Err(NegotiationError::Proposal);
        };
        if parent != [0; 32] || proposal.terms.deal_id != message.deal_id {
            return Err(NegotiationError::Transition);
        }
        let mut state = Negotiation::new((*proposal).clone())?;
        state.check_live(now)?;
        state.append(message)?;
        Ok(*proposal)
    }

    /// Domain-separated digest of the draft, before any transcript root is inserted.
    pub fn digest(&self) -> Result<[u8; 32], NegotiationError> {
        Ok(hashing::hash(&[PROPOSAL_DOMAIN, &self.encode()?]))
    }

    /// Creates the next price revision, retaining every other session field.
    pub fn counter(&self, amount: BaseUnits) -> Result<Self, NegotiationError> {
        let mut terms = self.terms.clone();
        terms.revision = terms
            .revision
            .checked_add(1)
            .ok_or(NegotiationError::Transition)?;
        terms.amount = amount;
        Self::new(terms, self.blinding.clone())
    }
}

impl From<erebus_core::encoding::EncodingError> for NegotiationError {
    fn from(_: erebus_core::encoding::EncodingError) -> Self {
        Self::Proposal
    }
}

#[derive(Clone)]
enum Event {
    Proposal {
        parent: [u8; 32],
        proposal: Box<Proposal>,
    },
    Accept([u8; 32]),
    FinalAuthorization(Authorization),
}

impl Event {
    fn encode(&self) -> Result<Vec<u8>, NegotiationError> {
        let mut writer = Writer::new();
        writer.u16(PROFILE_VERSION);
        match self {
            Self::Proposal { parent, proposal } => {
                writer.u8(1);
                writer.fixed(parent);
                writer.bytes_bounded(&proposal.encode()?, MAX_BODY_BYTES - 40);
            }
            Self::Accept(digest) => {
                writer.u8(2);
                writer.fixed(digest);
            }
            Self::FinalAuthorization(authorization) => {
                writer.u8(3);
                writer.bytes(
                    &authorization
                        .encode()
                        .map_err(|_| NegotiationError::Authorization)?,
                );
            }
        }
        Ok(writer.finish())
    }

    fn decode(bytes: &[u8]) -> Result<Self, NegotiationError> {
        let mut reader = Reader::new(bytes);
        if reader.u16("profile_version")? != PROFILE_VERSION {
            return Err(NegotiationError::Proposal);
        }
        let event = match reader.u8("event")? {
            1 => Self::Proposal {
                parent: reader.fixed("proposal_parent")?,
                proposal: Box::new(Proposal::decode(reader.bytes(
                    "proposal",
                    1,
                    MAX_BODY_BYTES - 40,
                )?)?),
            },
            2 => Self::Accept(reader.fixed("proposal_digest")?),
            3 => Self::FinalAuthorization(
                Authorization::decode(reader.bytes("authorization", 1, 1024)?)
                    .map_err(|_| NegotiationError::Authorization)?,
            ),
            _ => return Err(NegotiationError::Proposal),
        };
        reader.finish()?;
        Ok(event)
    }
}

/// Replayed negotiation state. No final agreement signatures are transcript events.
#[derive(Clone)]
pub struct Negotiation {
    initial: Proposal,
    latest: Option<(Role, Proposal)>,
    accepted: [bool; 2],
    transcript: Transcript,
}

impl fmt::Debug for Negotiation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Negotiation(<redacted>)")
    }
}

impl Negotiation {
    /// Starts a price-only session with an operator-selected initial buyer proposal.
    pub fn new(initial: Proposal) -> Result<Self, NegotiationError> {
        if initial.terms.revision != 1 {
            return Err(NegotiationError::Proposal);
        }
        let transcript = Transcript::new(initial.terms.deal_id, TRANSCRIPT_HASH_VERSION)
            .map_err(|_| NegotiationError::Transition)?;
        Ok(Self {
            initial,
            latest: None,
            accepted: [false; 2],
            transcript,
        })
    }

    /// Reconstructs semantic state and the unchanged M2 transcript hash from durable events.
    pub fn replay(initial: Proposal, messages: &[Message]) -> Result<Self, NegotiationError> {
        let mut state = Self::new(initial)?;
        for message in messages {
            state.append(message)?;
        }
        Ok(state)
    }

    /// The current proposal, if an initial offer has been persisted.
    #[must_use]
    pub fn latest(&self) -> Option<&Proposal> {
        self.latest.as_ref().map(|(_, proposal)| proposal)
    }

    /// True only after both peers accepted exactly the latest proposal.
    #[must_use]
    pub fn is_frozen(&self) -> bool {
        self.accepted == [true; 2]
    }

    /// Current authenticated transcript prefix.
    #[must_use]
    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    /// Builds an offer/counter without mutating or persisting state.
    pub fn propose(
        &self,
        session_id: [u8; 32],
        author: Role,
        amount: BaseUnits,
        now: u64,
    ) -> Result<Message, NegotiationError> {
        self.check_live(now)?;
        let (proposal, parent, kind) = if let Some((_, latest)) = &self.latest {
            (
                latest.counter(amount)?,
                latest.digest()?,
                MessageType::Counter,
            )
        } else {
            if amount != self.initial.terms.amount {
                return Err(NegotiationError::Context);
            }
            (self.initial.clone(), [0; 32], MessageType::Offer)
        };
        self.message(
            session_id,
            author,
            proposal.terms.revision,
            kind,
            Event::Proposal {
                parent,
                proposal: Box::new(proposal),
            },
        )
    }

    /// Builds acceptance of the current proposal, without signing the eventual agreement.
    pub fn accept(
        &self,
        session_id: [u8; 32],
        author: Role,
        now: u64,
    ) -> Result<Message, NegotiationError> {
        self.check_live(now)?;
        let latest = self.latest().ok_or(NegotiationError::Transition)?;
        self.message(
            session_id,
            author,
            latest.terms.revision,
            MessageType::Authorization,
            Event::Accept(latest.digest()?),
        )
    }

    fn check_live(&self, now: u64) -> Result<(), NegotiationError> {
        if now >= self.initial.terms.expiry {
            return Err(NegotiationError::Expired);
        }
        Ok(())
    }

    fn message(
        &self,
        session_id: [u8; 32],
        author: Role,
        revision: u32,
        kind: MessageType,
        event: Event,
    ) -> Result<Message, NegotiationError> {
        let message = Message::new(
            session_id,
            self.initial.terms.deal_id,
            revision,
            author,
            self.transcript.next_sequence(author),
            self.transcript.head(author),
            kind,
            event.encode()?,
        )
        .map_err(|_| NegotiationError::Proposal)?;
        let mut probe = self.clone();
        probe.append(&message)?;
        Ok(message)
    }

    /// Applies one authenticated event; failure leaves the state unchanged.
    pub fn append(&mut self, message: &Message) -> Result<(), NegotiationError> {
        self.transcript
            .accepts(message)
            .map_err(|_| NegotiationError::Transition)?;
        if self.is_frozen() {
            return Err(NegotiationError::Transition);
        }
        match Event::decode(&message.body)? {
            Event::Proposal { parent, proposal } => {
                if self.accepted != [false; 2] {
                    return Err(NegotiationError::Transition);
                }
                let mut expected_context = proposal.terms.clone();
                expected_context.amount = self.initial.terms.amount;
                expected_context.revision = 1;
                if expected_context != self.initial.terms
                    || proposal.blinding != self.initial.blinding
                {
                    return Err(NegotiationError::Context);
                }
                if message.revision != proposal.terms.revision {
                    return Err(NegotiationError::Transition);
                }
                if let Some((previous_author, previous)) = &self.latest {
                    if message.message_type != MessageType::Counter
                        || *previous_author == message.author
                        || parent != previous.digest()?
                        || previous.terms.revision.checked_add(1) != Some(proposal.terms.revision)
                    {
                        return Err(NegotiationError::Transition);
                    }
                } else if message.message_type != MessageType::Offer
                    || message.author != Role::Buyer
                    || parent != [0; 32]
                    || *proposal != self.initial
                {
                    return Err(NegotiationError::Transition);
                }
                self.transcript
                    .append(message)
                    .map_err(|_| NegotiationError::Transition)?;
                self.latest = Some((message.author, *proposal));
            }
            Event::Accept(digest) => {
                let (proposer, latest) =
                    self.latest.as_ref().ok_or(NegotiationError::Transition)?;
                let index = usize::from(message.author.tag() - 1);
                if message.message_type != MessageType::Authorization
                    || message.revision != latest.terms.revision
                    || digest != latest.digest()?
                    || self.accepted[index]
                    || (self.accepted == [false; 2] && message.author == *proposer)
                {
                    return Err(NegotiationError::Transition);
                }
                self.transcript
                    .append(message)
                    .map_err(|_| NegotiationError::Transition)?;
                self.accepted[index] = true;
            }
            Event::FinalAuthorization(_) => return Err(NegotiationError::Transition),
        }
        Ok(())
    }

    /// Returns final terms only after both acceptance events are in the frozen prefix.
    pub fn agreement(
        &self,
    ) -> Result<(AgreementTerms, CommitmentBlinding, DealCommitment), NegotiationError> {
        if !self.is_frozen() {
            return Err(NegotiationError::NotFrozen);
        }
        let latest = self.latest().ok_or(NegotiationError::NotFrozen)?;
        let mut terms = latest.terms.clone();
        terms.transcript_root = self
            .transcript
            .root()
            .map_err(|_| NegotiationError::Transition)?;
        let commitment =
            commit_agreement(&terms, &latest.blinding).map_err(|_| NegotiationError::Proposal)?;
        Ok((terms, latest.blinding.clone(), commitment))
    }

    /// Builds an encrypted-channel envelope that must not be appended to negotiation storage.
    /// Persist the verified authorization in settlement state before acknowledging it.
    pub fn authorization_message(
        &self,
        session_id: [u8; 32],
        authorization: &Authorization,
        now: u64,
    ) -> Result<Message, NegotiationError> {
        let terms = self.agreement()?.0;
        let message = Message::new(
            session_id,
            terms.deal_id,
            terms.revision,
            authorization.role,
            self.transcript.next_sequence(authorization.role),
            self.transcript.head(authorization.role),
            MessageType::Authorization,
            Event::FinalAuthorization(authorization.clone()).encode()?,
        )
        .map_err(|_| NegotiationError::Authorization)?;
        self.verify_final_authorization(&message, now)?;
        Ok(message)
    }

    /// Verifies a detached final authorization against the frozen root without changing it.
    /// The authenticated socket must also enforce the peer's author role and session id.
    pub fn verify_final_authorization(
        &self,
        message: &Message,
        now: u64,
    ) -> Result<Authorization, NegotiationError> {
        let (terms, blinding, commitment) = self.agreement()?;
        self.check_live(now)?;
        self.transcript
            .accepts(message)
            .map_err(|_| NegotiationError::Authorization)?;
        if message.message_type != MessageType::Authorization || message.revision != terms.revision
        {
            return Err(NegotiationError::Authorization);
        }
        let Event::FinalAuthorization(authorization) = Event::decode(&message.body)? else {
            return Err(NegotiationError::Authorization);
        };
        if authorization.role != message.author {
            return Err(NegotiationError::Authorization);
        }
        verify_authorization_signature(&terms, &commitment, &blinding, &authorization)
            .map_err(|_| NegotiationError::Authorization)?;
        Ok(authorization)
    }
}

/// Durable, idempotent delivery of profile-1 events under the transcript store lock.
#[derive(Clone)]
pub struct NegotiationStore {
    store: FileTranscriptStore,
    namespace: String,
    initial: Proposal,
}

impl fmt::Debug for NegotiationStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NegotiationStore(<redacted>)")
    }
}

impl NegotiationStore {
    /// Opens a negotiation using an operator-pinned draft and private transcript storage.
    pub fn new(
        store: FileTranscriptStore,
        namespace: String,
        initial: Proposal,
    ) -> Result<Self, NegotiationError> {
        Negotiation::new(initial.clone())?;
        let state = Self {
            store,
            namespace,
            initial,
        };
        state.load()?;
        Ok(state)
    }

    /// Replays durable events and repairs an interrupted freeze-record write.
    pub fn load(&self) -> Result<Negotiation, NegotiationError> {
        let messages = self
            .store
            .messages(&self.namespace, self.initial.terms.deal_id)?;
        let state = Negotiation::replay(self.initial.clone(), &messages)?;
        if state.is_frozen() {
            self.store.freeze(
                &self.namespace,
                self.initial.terms.deal_id,
                TRANSCRIPT_HASH_VERSION,
                state
                    .transcript
                    .root()
                    .map_err(|_| NegotiationError::Transition)?,
                state
                    .latest()
                    .ok_or(NegotiationError::NotFrozen)?
                    .digest()?,
            )?;
        } else if self
            .store
            .frozen(
                &self.namespace,
                self.initial.terms.deal_id,
                TRANSCRIPT_HASH_VERSION,
            )?
            .is_some()
        {
            return Err(NegotiationError::Transition);
        }
        Ok(state)
    }

    /// Persists before acknowledgement; exact re-delivery after a fresh session is idempotent.
    ///
    /// A changed message at an existing sequence is still rejected. Callers authenticate
    /// `message.author` against the remote socket role before invoking this method.
    pub fn append(&self, message: &Message) -> Result<Negotiation, NegotiationError> {
        self.store.append_checked(
            &self.namespace,
            self.initial.terms.deal_id,
            TRANSCRIPT_HASH_VERSION,
            message,
            true,
            |stored| {
                let mut state = Negotiation::replay(self.initial.clone(), stored)
                    .map_err(|_| StoreError::Transition)?;
                if !stored
                    .iter()
                    .any(|previous| previous.encode_body() == message.encode_body())
                {
                    state.append(message).map_err(|_| StoreError::Transition)?;
                }
                if state.is_frozen() {
                    Ok(Some(
                        state
                            .latest()
                            .ok_or(StoreError::Transition)?
                            .digest()
                            .map_err(|_| StoreError::Transition)?,
                    ))
                } else {
                    Ok(None)
                }
            },
        )?;
        self.load()
    }
}

#[cfg(test)]
pub(crate) mod tests;

pub mod bootstrap;
pub mod peer;
