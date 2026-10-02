//! What chain evidence proves about one deal and each of its signed revisions.
//!
//! Every signed revision of a deal shares one consumed deal identity, and each revision stays
//! executable until its own expiry or until the deal is consumed (agreement §7). Settlement has
//! no sender check, so our own transaction's fate is not the payment's fate: the seller, a
//! relayer, or a copy of our calldata may have settled the deal (M6 decisions DM6-1). The
//! failure this module prevents is reporting a deal as unpaid when it is not, which would
//! release a spending reservation or invite renegotiation.
//!
//! The rules, pinned by the table tests below:
//!
//! - A revision is proven unable to pay only by an unconsumed deal identity at a final block
//!   whose timestamp is at or after that revision's expiry. This relies on block timestamps
//!   being monotonic (DM6-5).
//! - Consumption does not identify the winner. Until the backend verifies the winning
//!   settlement, the deal is unresolved and no reservation moves, including the reservation of
//!   a revision that has itself expired.
//! - Another revision's settlement supersedes this one only once it is final.
//! - A deal closes unpaid only when every revision that may have been signed is proven unable
//!   to pay and nothing consumed the deal.
//! - A read that failed, timed out, or disagreed decides nothing. Timeouts, reverts, advanced
//!   nonces, allowance changes, and local cancellation are not inputs at all.
//!
//! Reservation decisions come only from [`assess_deal`], because releasing a superseded
//! revision is safe only when the winner is a revision whose reservation is committed in its
//! place. [`NoEffectProof`] and [`FinalPayment`] have no public constructor, so the ledger's
//! proof-taking methods cannot be reached from any other state.
//!
//! The module is pure. A backend assembles [`DealEvidence`] from reads it anchored to specific
//! blocks; the classifier trusts those reads and checks only that they cohere. The proof types
//! are a type-level fence against category errors, not a cryptographic proof: a backend that
//! fabricates reads can still fabricate a release.

use std::collections::BTreeSet;

use crate::commitment::{CommitmentBlinding, CommitmentError, DealCommitment, DealNullifier};
use crate::ids::BaseUnits;
use crate::policy::ReservationId;
use crate::terms::AgreementTerms;

/// The values of one signed revision that its settlement evidence is checked against.
///
/// Every revision of a deal carries the same deal identity and its own commitment, expiry,
/// amount, and fee.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedRevision {
    deal_nullifier: DealNullifier,
    commitment: DealCommitment,
    expiry: u64,
    amount: BaseUnits,
    fee: BaseUnits,
    total: BaseUnits,
}

/// A signed revision could not be described.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RevisionError {
    /// The opening did not produce a commitment or deal identity.
    #[error(transparent)]
    Commitment(#[from] CommitmentError),
    /// The amount plus fee overflowed 128 bits.
    #[error("revision amount plus fee overflowed 128 bits")]
    AmountOverflow,
}

impl SignedRevision {
    /// Describes a revision from its opening, so the values cannot disagree with the
    /// commitment that was signed.
    pub fn from_opening(
        terms: &AgreementTerms,
        blinding: &CommitmentBlinding,
    ) -> Result<Self, RevisionError> {
        let commitment = crate::commitment::commit_agreement(terms, blinding)?;
        let deal_nullifier = crate::commitment::deal_nullifier(terms)?;
        Self::new(
            deal_nullifier,
            commitment,
            terms.expiry,
            terms.amount,
            terms.fee_policy.fee,
        )
    }

    /// Describes a revision from persisted values. Prefer `from_opening` when the opening is
    /// at hand.
    pub fn new(
        deal_nullifier: DealNullifier,
        commitment: DealCommitment,
        expiry: u64,
        amount: BaseUnits,
        fee: BaseUnits,
    ) -> Result<Self, RevisionError> {
        let total = amount
            .get()
            .checked_add(fee.get())
            .ok_or(RevisionError::AmountOverflow)?;
        Ok(Self {
            deal_nullifier,
            commitment,
            expiry,
            amount,
            fee,
            total: BaseUnits::new(total),
        })
    }

    /// Consumed identity shared by every revision of the deal.
    #[must_use]
    pub const fn deal_nullifier(&self) -> DealNullifier {
        self.deal_nullifier
    }

    /// The commitment this revision's authorizations sign.
    #[must_use]
    pub const fn commitment(&self) -> DealCommitment {
        self.commitment
    }

    /// Unix timestamp from which this revision cannot settle.
    #[must_use]
    pub const fn expiry(&self) -> u64 {
        self.expiry
    }

    /// Payment amount.
    #[must_use]
    pub const fn amount(&self) -> BaseUnits {
        self.amount
    }

    /// Settlement fee.
    #[must_use]
    pub const fn fee(&self) -> BaseUnits {
        self.fee
    }

    /// Amount plus fee: what this revision reserves and what it spends if it wins.
    #[must_use]
    pub const fn total(&self) -> BaseUnits {
        self.total
    }
}

/// Chain evidence about one deal identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DealEvidence {
    /// A read failed, timed out, or disagreed with another read. Nothing is decided from it.
    Unknown,
    /// Every read succeeded and agreed.
    Observed(DealReads),
}

/// Reads for one deal identity, anchored to two blocks the backend chose: a final anchor and
/// a head anchor at or after it.
///
/// The backend reads `consumed_at_final` at the final anchor and `consumed_at_head` at the
/// head anchor, and searches for the winning settlement up to the head anchor. A winner is
/// final exactly when its block is at or before the final anchor. Reads that break these
/// relations are incoherent and classify as unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DealReads {
    /// The deal identity every read was keyed by.
    pub deal_nullifier: DealNullifier,
    /// Timestamp of the final anchor block.
    pub final_anchor_timestamp: u64,
    /// Whether the deal identity was consumed at the final anchor.
    pub consumed_at_final: bool,
    /// Whether the deal identity was consumed at the head anchor.
    pub consumed_at_head: bool,
    /// The verified winning settlement, when the backend found and verified it.
    pub winner: Option<WinningSettlement>,
}

impl DealReads {
    fn is_coherent(&self) -> bool {
        // A consumption at the final anchor is visible at every later block.
        if self.consumed_at_final && !self.consumed_at_head {
            return false;
        }
        match &self.winner {
            None => true,
            // The winning settlement consumed the deal in its own block, so the head sees it,
            // and it is final exactly when that consumption is.
            Some(winner) => self.consumed_at_head && winner.is_final == self.consumed_at_final,
        }
    }
}

/// The settlement that consumed the deal, found by the deal identity and verified by the
/// backend.
///
/// The submitter may be anyone. Amount and fee come from the public settlement event in
/// public-bound mode, or from the verified opening in shielded mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WinningSettlement {
    /// The commitment the settlement opened.
    pub commitment: DealCommitment,
    /// The amount it paid.
    pub amount: BaseUnits,
    /// The fee it paid.
    pub fee: BaseUnits,
    /// Whether its block is at or before the final anchor.
    pub is_final: bool,
}

/// What the evidence proves about one signed revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RevisionState {
    /// Nothing consumed the deal and this revision has not expired at a final block. It may
    /// still settle.
    Open,
    /// This revision settled in a block that is not yet final.
    PaidIncluded,
    /// This revision settled, and the settlement is final.
    PaidFinalized,
    /// Another revision settled in a block that is not yet final. A reorg could undo it.
    SupersededIncluded,
    /// Another revision settled, and the settlement is final.
    SupersededFinal,
    /// The deal is consumed and the winning settlement is not yet verified.
    ConsumedUnresolved,
    /// The deal was unconsumed at a final block at or after this revision's expiry, so this
    /// revision never paid and never can.
    ExpiredUnpaid,
    /// The evidence was missing, for another deal, or incoherent.
    Unknown,
}

/// Classifies one revision against evidence about its deal.
///
/// This is the per-revision fact, for diagnostics and recovery. Reservation decisions need
/// the whole revision set; take them from [`assess_deal`].
#[must_use]
pub fn classify_revision(revision: &SignedRevision, evidence: &DealEvidence) -> RevisionState {
    let DealEvidence::Observed(reads) = evidence else {
        return RevisionState::Unknown;
    };
    if reads.deal_nullifier != revision.deal_nullifier || !reads.is_coherent() {
        return RevisionState::Unknown;
    }
    // Settlement rejects `timestamp >= expiry`, and timestamps do not decrease, so no block
    // from the final anchor on can settle this revision.
    let expired_at_final = reads.final_anchor_timestamp >= revision.expiry;
    match &reads.winner {
        Some(winner) if winner.commitment == revision.commitment => {
            if winner.amount != revision.amount || winner.fee != revision.fee {
                // The commitment binds amount and fee; evidence that disagrees was not verified.
                RevisionState::Unknown
            } else if winner.is_final {
                RevisionState::PaidFinalized
            } else if expired_at_final {
                // A non-final settlement is after the final anchor, so after this expiry.
                RevisionState::Unknown
            } else {
                RevisionState::PaidIncluded
            }
        }
        Some(winner) if winner.is_final => RevisionState::SupersededFinal,
        // The winner may still be reorganized out, but this revision cannot take its place.
        Some(_) if expired_at_final => RevisionState::ExpiredUnpaid,
        Some(_) => RevisionState::SupersededIncluded,
        None if reads.consumed_at_head => RevisionState::ConsumedUnresolved,
        None if expired_at_final => RevisionState::ExpiredUnpaid,
        None => RevisionState::Open,
    }
}

/// What a coordinator may do about one revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionAction {
    /// The revision may still settle. Submission remains subject to its expiry at submission
    /// time and to the journal's attempt rules. Not submitting does not revoke it.
    MaySettle,
    /// A settlement is included but not final. Wait; do not submit another.
    AwaitFinality,
    /// The deal is consumed: find the settlement for the deal identity and verify it before
    /// anything else.
    ResolveWinner,
    /// Read the evidence again. An unknown outcome is never evidence of non-payment.
    Reconcile,
    /// The revision is decided. Never submit it again.
    Stop,
}

/// Maps a revision's state to what a coordinator may do about it.
#[must_use]
pub fn revision_recovery_action(state: RevisionState) -> RevisionAction {
    match state {
        RevisionState::Open => RevisionAction::MaySettle,
        RevisionState::PaidIncluded | RevisionState::SupersededIncluded => {
            RevisionAction::AwaitFinality
        }
        RevisionState::ConsumedUnresolved => RevisionAction::ResolveWinner,
        RevisionState::Unknown => RevisionAction::Reconcile,
        RevisionState::PaidFinalized
        | RevisionState::SupersededFinal
        | RevisionState::ExpiredUnpaid => RevisionAction::Stop,
    }
}

/// Final evidence that one revision paid. Only [`assess_deal`] constructs it, and only for a
/// [`RevisionState::PaidFinalized`] revision.
///
/// Callers can inspect classifier-issued evidence through its public accessors:
///
/// ```
/// use erebus_core::commitment::DealCommitment;
/// use erebus_core::deal_state::FinalPayment;
/// use erebus_core::ids::BaseUnits;
/// use erebus_core::policy::ReservationId;
/// fn inspect(payment: &FinalPayment) -> (DealCommitment, BaseUnits, ReservationId) {
///     (payment.commitment(), payment.total(), payment.reservation_id())
/// }
/// ```
///
/// It has no public constructor. The field names in this example are correct; the literal
/// fails because the fields are private:
///
/// ```compile_fail
/// use erebus_core::commitment::DealCommitment;
/// use erebus_core::deal_state::FinalPayment;
/// use erebus_core::ids::BaseUnits;
/// let _forged = FinalPayment {
///     commitment: DealCommitment::from_bytes([0; 32]),
///     total: BaseUnits::new(1),
/// };
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct FinalPayment {
    commitment: DealCommitment,
    total: BaseUnits,
}

impl FinalPayment {
    /// The revision that paid.
    #[must_use]
    pub const fn commitment(&self) -> DealCommitment {
        self.commitment
    }

    /// The amount plus fee it paid.
    #[must_use]
    pub const fn total(&self) -> BaseUnits {
        self.total
    }

    /// The reservation this payment commits.
    #[must_use]
    pub const fn reservation_id(&self) -> ReservationId {
        ReservationId::for_revision(&self.commitment)
    }
}

/// Why a revision provably had no effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoEffectReason {
    /// The deal was unconsumed at a final block at or after the revision's expiry.
    Expired,
    /// Another revision in the same set settled, and that settlement is final.
    Superseded,
}

/// Evidence that one revision never paid and never can. Only [`assess_deal`] constructs it,
/// and only for an [`RevisionState::ExpiredUnpaid`] or [`RevisionState::SupersededFinal`]
/// revision of a deal whose evidence the revision set explains.
///
/// A timeout, a revert, an advanced nonce, an allowance change, or a local cancellation
/// cannot produce one, because it has no public constructor. The accessors below compile:
///
/// ```
/// use erebus_core::commitment::DealCommitment;
/// use erebus_core::deal_state::{NoEffectProof, NoEffectReason};
/// fn inspect(proof: &NoEffectProof) -> (DealCommitment, NoEffectReason) {
///     (proof.commitment(), proof.reason())
/// }
/// ```
///
/// and the struct literal does not, because the fields are private. Stable rustdoc does not
/// check the error code, so the field names here are kept identical to the definition:
///
/// ```compile_fail
/// use erebus_core::commitment::DealCommitment;
/// use erebus_core::deal_state::{NoEffectProof, NoEffectReason};
/// let _forged = NoEffectProof {
///     commitment: DealCommitment::from_bytes([0; 32]),
///     reason: NoEffectReason::Expired,
/// };
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct NoEffectProof {
    commitment: DealCommitment,
    reason: NoEffectReason,
}

impl NoEffectProof {
    /// The revision that had no effect.
    #[must_use]
    pub const fn commitment(&self) -> DealCommitment {
        self.commitment
    }

    /// Why it had no effect.
    #[must_use]
    pub const fn reason(&self) -> NoEffectReason {
        self.reason
    }

    /// The reservation this proof releases.
    #[must_use]
    pub const fn reservation_id(&self) -> ReservationId {
        ReservationId::for_revision(&self.commitment)
    }
}

/// What may happen to one revision's spending reservation.
#[derive(Debug, PartialEq, Eq)]
pub enum ReservationDecision {
    /// Keep the reservation; it still counts against limits.
    Hold,
    /// Commit the paid amount plus fee.
    Commit(FinalPayment),
    /// Release the reservation.
    Release(NoEffectProof),
}

/// What the evidence proves about a deal across every revision that may have been signed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DealState {
    /// Nothing consumed the deal and at least one revision may still settle.
    Open,
    /// A revision in the set settled in a block that is not yet final.
    PaidIncluded {
        /// The winning revision.
        commitment: DealCommitment,
    },
    /// A revision in the set settled, and the settlement is final.
    PaidFinalized {
        /// The winning revision.
        commitment: DealCommitment,
    },
    /// The deal is consumed and the winning settlement is not yet verified.
    ConsumedUnresolved,
    /// Every revision in the set is proven unable to pay, and nothing consumed the deal.
    ClosedUnpaid,
    /// A settlement consumed the deal that no revision in the set explains. The set of
    /// possibly-signed revisions is incomplete, or the evidence is wrong.
    UnexplainedConsumption,
    /// The evidence was missing or incoherent, or the revision set was empty or repeated a
    /// commitment.
    Unknown,
}

/// One revision's share of a deal assessment.
#[derive(Debug, PartialEq, Eq)]
pub struct RevisionAssessment {
    /// The revision.
    pub commitment: DealCommitment,
    /// What the evidence proves about it alone.
    pub state: RevisionState,
    /// What may happen to its reservation, given the whole deal.
    pub reservation: ReservationDecision,
}

/// A deal's state and the per-revision reservation decisions it permits.
#[derive(Debug, PartialEq, Eq)]
pub struct DealAssessment {
    /// The deal-level state.
    pub state: DealState,
    /// One entry per supplied revision, in the supplied order.
    pub revisions: Vec<RevisionAssessment>,
}

/// Assesses a deal from every revision that may have been signed and one set of reads.
///
/// `revisions` must include every revision either party may hold a signature for, because a
/// deal closes unpaid only when all of them are proven unable to pay. When the deal state is
/// unknown or unexplained, every reservation is held, whatever the individual revisions show.
#[must_use]
pub fn assess_deal(revisions: &[SignedRevision], evidence: &DealEvidence) -> DealAssessment {
    let states: Vec<RevisionState> = revisions
        .iter()
        .map(|revision| classify_revision(revision, evidence))
        .collect();
    let state = deal_state(revisions, &states, evidence);
    let decided = !matches!(
        state,
        DealState::Unknown | DealState::UnexplainedConsumption
    );
    let revisions = revisions
        .iter()
        .zip(states)
        .map(|(revision, state)| RevisionAssessment {
            commitment: revision.commitment,
            state,
            reservation: if decided {
                reservation_decision(revision, state)
            } else {
                ReservationDecision::Hold
            },
        })
        .collect();
    DealAssessment { state, revisions }
}

fn deal_state(
    revisions: &[SignedRevision],
    states: &[RevisionState],
    evidence: &DealEvidence,
) -> DealState {
    let DealEvidence::Observed(reads) = evidence else {
        return DealState::Unknown;
    };
    let distinct: BTreeSet<[u8; 32]> = revisions
        .iter()
        .map(|revision| *revision.commitment.as_bytes())
        .collect();
    // An empty set would close unpaid vacuously.
    if revisions.is_empty()
        || distinct.len() != revisions.len()
        || states.contains(&RevisionState::Unknown)
    {
        return DealState::Unknown;
    }
    if let Some(winner) = &reads.winner {
        let commitment = winner.commitment;
        if !distinct.contains(commitment.as_bytes()) {
            return DealState::UnexplainedConsumption;
        }
        return if winner.is_final {
            DealState::PaidFinalized { commitment }
        } else {
            DealState::PaidIncluded { commitment }
        };
    }
    if reads.consumed_at_head {
        return DealState::ConsumedUnresolved;
    }
    if states
        .iter()
        .all(|state| *state == RevisionState::ExpiredUnpaid)
    {
        DealState::ClosedUnpaid
    } else {
        DealState::Open
    }
}

/// The only place the two proof types are constructed.
fn reservation_decision(revision: &SignedRevision, state: RevisionState) -> ReservationDecision {
    let commitment = revision.commitment;
    match state {
        RevisionState::PaidFinalized => ReservationDecision::Commit(FinalPayment {
            commitment,
            total: revision.total,
        }),
        RevisionState::ExpiredUnpaid => ReservationDecision::Release(NoEffectProof {
            commitment,
            reason: NoEffectReason::Expired,
        }),
        RevisionState::SupersededFinal => ReservationDecision::Release(NoEffectProof {
            commitment,
            reason: NoEffectReason::Superseded,
        }),
        RevisionState::Open
        | RevisionState::PaidIncluded
        | RevisionState::SupersededIncluded
        | RevisionState::ConsumedUnresolved
        | RevisionState::Unknown => ReservationDecision::Hold,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{PolicyError, ReservationLedger, ReservationState};
    use DealState as D;
    use RevisionState as R;

    const EXPIRY: u64 = 1_800_000_000;
    const DEAL: DealNullifier = DealNullifier::from_bytes([0xd0; 32]);
    const THIS: DealCommitment = DealCommitment::from_bytes([0xa1; 32]);
    const OTHER: DealCommitment = DealCommitment::from_bytes([0xb2; 32]);

    /// The revision under test.
    fn this() -> SignedRevision {
        revision(THIS, EXPIRY, 1_000_000)
    }

    /// Another revision of the same deal that never expires in these tests, so it can win.
    fn other() -> SignedRevision {
        revision(OTHER, u64::MAX, 1_200_000)
    }

    fn revision(commitment: DealCommitment, expiry: u64, amount: u128) -> SignedRevision {
        SignedRevision::new(
            DEAL,
            commitment,
            expiry,
            BaseUnits::new(amount),
            BaseUnits::new(2_500),
        )
        .unwrap()
    }

    fn settled_by(revision: &SignedRevision, is_final: bool) -> WinningSettlement {
        WinningSettlement {
            commitment: revision.commitment(),
            amount: revision.amount(),
            fee: revision.fee(),
            is_final,
        }
    }

    fn reads(
        final_anchor_timestamp: u64,
        consumed_at_final: bool,
        consumed_at_head: bool,
        winner: Option<WinningSettlement>,
    ) -> DealEvidence {
        DealEvidence::Observed(DealReads {
            deal_nullifier: DEAL,
            final_anchor_timestamp,
            consumed_at_final,
            consumed_at_head,
            winner,
        })
    }

    fn unconsumed_at(final_anchor_timestamp: u64) -> DealEvidence {
        reads(final_anchor_timestamp, false, false, None)
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    enum Winner {
        Absent,
        ThisFinal,
        ThisIncluded,
        OtherFinal,
        OtherIncluded,
    }

    /// Where the final anchor's timestamp falls against this revision's expiry.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    enum Anchor {
        Before,
        At,
        After,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Reserve {
        Hold,
        Commit,
        ReleaseExpired,
        ReleaseSuperseded,
    }

    use Anchor::{After, At, Before};
    use Reserve::{Commit, Hold, ReleaseExpired, ReleaseSuperseded};
    use Winner::{Absent, OtherFinal, OtherIncluded, ThisFinal, ThisIncluded};

    const PAID_BY_THIS: DealState = D::PaidFinalized { commitment: THIS };
    const PAID_BY_OTHER: DealState = D::PaidFinalized { commitment: OTHER };
    const INCLUDED_THIS: DealState = D::PaidIncluded { commitment: THIS };
    const INCLUDED_OTHER: DealState = D::PaidIncluded { commitment: OTHER };

    /// Every combination of the reads. Columns: consumed at the final anchor, consumed at the
    /// head anchor, winner, final anchor against this revision's expiry; then this revision's
    /// state, its reservation decision in the deal `[this, other]`, and that deal's state.
    #[rustfmt::skip]
    const TABLE: &[(bool, bool, Winner, Anchor, RevisionState, Reserve, DealState)] = &[
        // Unconsumed everywhere: open until this revision's own expiry is final.
        (false, false, Absent,        Before, R::Open,               Hold,              D::Open),
        (false, false, Absent,        At,     R::ExpiredUnpaid,      ReleaseExpired,    D::Open),
        (false, false, Absent,        After,  R::ExpiredUnpaid,      ReleaseExpired,    D::Open),
        // A winner the head does not see as consumed is incoherent.
        (false, false, ThisFinal,     Before, R::Unknown,            Hold,              D::Unknown),
        (false, false, ThisFinal,     At,     R::Unknown,            Hold,              D::Unknown),
        (false, false, ThisFinal,     After,  R::Unknown,            Hold,              D::Unknown),
        (false, false, ThisIncluded,  Before, R::Unknown,            Hold,              D::Unknown),
        (false, false, ThisIncluded,  At,     R::Unknown,            Hold,              D::Unknown),
        (false, false, ThisIncluded,  After,  R::Unknown,            Hold,              D::Unknown),
        (false, false, OtherFinal,    Before, R::Unknown,            Hold,              D::Unknown),
        (false, false, OtherFinal,    At,     R::Unknown,            Hold,              D::Unknown),
        (false, false, OtherFinal,    After,  R::Unknown,            Hold,              D::Unknown),
        (false, false, OtherIncluded, Before, R::Unknown,            Hold,              D::Unknown),
        (false, false, OtherIncluded, At,     R::Unknown,            Hold,              D::Unknown),
        (false, false, OtherIncluded, After,  R::Unknown,            Hold,              D::Unknown),
        // Consumed at the head only. Without a verified winner nothing moves, even for a
        // revision whose own expiry is final.
        (false, true,  Absent,        Before, R::ConsumedUnresolved, Hold,              D::ConsumedUnresolved),
        (false, true,  Absent,        At,     R::ConsumedUnresolved, Hold,              D::ConsumedUnresolved),
        (false, true,  Absent,        After,  R::ConsumedUnresolved, Hold,              D::ConsumedUnresolved),
        // A final winner needs a final consumption.
        (false, true,  ThisFinal,     Before, R::Unknown,            Hold,              D::Unknown),
        (false, true,  ThisFinal,     At,     R::Unknown,            Hold,              D::Unknown),
        (false, true,  ThisFinal,     After,  R::Unknown,            Hold,              D::Unknown),
        // This revision cannot have settled after a final anchor at or past its expiry.
        (false, true,  ThisIncluded,  Before, R::PaidIncluded,       Hold,              INCLUDED_THIS),
        (false, true,  ThisIncluded,  At,     R::Unknown,            Hold,              D::Unknown),
        (false, true,  ThisIncluded,  After,  R::Unknown,            Hold,              D::Unknown),
        (false, true,  OtherFinal,    Before, R::Unknown,            Hold,              D::Unknown),
        (false, true,  OtherFinal,    At,     R::Unknown,            Hold,              D::Unknown),
        (false, true,  OtherFinal,    After,  R::Unknown,            Hold,              D::Unknown),
        // A non-final winner supersedes nothing, but this revision's own final expiry releases it.
        (false, true,  OtherIncluded, Before, R::SupersededIncluded, Hold,              INCLUDED_OTHER),
        (false, true,  OtherIncluded, At,     R::ExpiredUnpaid,      ReleaseExpired,    INCLUDED_OTHER),
        (false, true,  OtherIncluded, After,  R::ExpiredUnpaid,      ReleaseExpired,    INCLUDED_OTHER),
        // A final consumption the head does not see is incoherent.
        (true,  false, Absent,        Before, R::Unknown,            Hold,              D::Unknown),
        (true,  false, Absent,        At,     R::Unknown,            Hold,              D::Unknown),
        (true,  false, Absent,        After,  R::Unknown,            Hold,              D::Unknown),
        (true,  false, ThisFinal,     Before, R::Unknown,            Hold,              D::Unknown),
        (true,  false, ThisFinal,     At,     R::Unknown,            Hold,              D::Unknown),
        (true,  false, ThisFinal,     After,  R::Unknown,            Hold,              D::Unknown),
        (true,  false, ThisIncluded,  Before, R::Unknown,            Hold,              D::Unknown),
        (true,  false, ThisIncluded,  At,     R::Unknown,            Hold,              D::Unknown),
        (true,  false, ThisIncluded,  After,  R::Unknown,            Hold,              D::Unknown),
        (true,  false, OtherFinal,    Before, R::Unknown,            Hold,              D::Unknown),
        (true,  false, OtherFinal,    At,     R::Unknown,            Hold,              D::Unknown),
        (true,  false, OtherFinal,    After,  R::Unknown,            Hold,              D::Unknown),
        (true,  false, OtherIncluded, Before, R::Unknown,            Hold,              D::Unknown),
        (true,  false, OtherIncluded, At,     R::Unknown,            Hold,              D::Unknown),
        (true,  false, OtherIncluded, After,  R::Unknown,            Hold,              D::Unknown),
        // Consumed at a final block. The winner decides, and expiry no longer matters.
        (true,  true,  Absent,        Before, R::ConsumedUnresolved, Hold,              D::ConsumedUnresolved),
        (true,  true,  Absent,        At,     R::ConsumedUnresolved, Hold,              D::ConsumedUnresolved),
        (true,  true,  Absent,        After,  R::ConsumedUnresolved, Hold,              D::ConsumedUnresolved),
        (true,  true,  ThisFinal,     Before, R::PaidFinalized,      Commit,            PAID_BY_THIS),
        (true,  true,  ThisFinal,     At,     R::PaidFinalized,      Commit,            PAID_BY_THIS),
        (true,  true,  ThisFinal,     After,  R::PaidFinalized,      Commit,            PAID_BY_THIS),
        // A final consumption cannot belong to a non-final winner.
        (true,  true,  ThisIncluded,  Before, R::Unknown,            Hold,              D::Unknown),
        (true,  true,  ThisIncluded,  At,     R::Unknown,            Hold,              D::Unknown),
        (true,  true,  ThisIncluded,  After,  R::Unknown,            Hold,              D::Unknown),
        (true,  true,  OtherFinal,    Before, R::SupersededFinal,    ReleaseSuperseded, PAID_BY_OTHER),
        (true,  true,  OtherFinal,    At,     R::SupersededFinal,    ReleaseSuperseded, PAID_BY_OTHER),
        (true,  true,  OtherFinal,    After,  R::SupersededFinal,    ReleaseSuperseded, PAID_BY_OTHER),
        (true,  true,  OtherIncluded, Before, R::Unknown,            Hold,              D::Unknown),
        (true,  true,  OtherIncluded, At,     R::Unknown,            Hold,              D::Unknown),
        (true,  true,  OtherIncluded, After,  R::Unknown,            Hold,              D::Unknown),
    ];

    fn row_evidence(
        consumed_at_final: bool,
        consumed_at_head: bool,
        winner: Winner,
        anchor: Anchor,
    ) -> DealEvidence {
        let final_anchor_timestamp = match anchor {
            Before => EXPIRY - 1,
            At => EXPIRY,
            After => EXPIRY + 1,
        };
        let winner = match winner {
            Absent => None,
            ThisFinal => Some(settled_by(&this(), true)),
            ThisIncluded => Some(settled_by(&this(), false)),
            OtherFinal => Some(settled_by(&other(), true)),
            OtherIncluded => Some(settled_by(&other(), false)),
        };
        reads(
            final_anchor_timestamp,
            consumed_at_final,
            consumed_at_head,
            winner,
        )
    }

    fn reserve_kind(decision: &ReservationDecision) -> Reserve {
        match decision {
            ReservationDecision::Hold => Hold,
            ReservationDecision::Commit(payment) => {
                assert_eq!(payment.commitment(), THIS);
                assert_eq!(payment.total(), this().total());
                Commit
            }
            ReservationDecision::Release(proof) => {
                assert_eq!(proof.commitment(), THIS);
                match proof.reason() {
                    NoEffectReason::Expired => ReleaseExpired,
                    NoEffectReason::Superseded => ReleaseSuperseded,
                }
            }
        }
    }

    #[test]
    fn the_table_covers_every_combination_of_reads_exactly_once() {
        let keys: BTreeSet<_> = TABLE
            .iter()
            .map(|(at_final, at_head, winner, anchor, ..)| (*at_final, *at_head, *winner, *anchor))
            .collect();
        assert_eq!(keys.len(), TABLE.len(), "a combination is repeated");
        assert_eq!(keys.len(), 2 * 2 * 5 * 3, "a combination is missing");
    }

    #[test]
    fn every_combination_of_reads_classifies_as_the_table_says() {
        for &(at_final, at_head, winner, anchor, state, reserve, deal) in TABLE {
            let row = (at_final, at_head, winner, anchor);
            let evidence = row_evidence(at_final, at_head, winner, anchor);
            assert_eq!(classify_revision(&this(), &evidence), state, "{row:?}");

            let assessment = assess_deal(&[this(), other()], &evidence);
            assert_eq!(assessment.state, deal, "{row:?}");
            assert_eq!(assessment.revisions[0].commitment, THIS, "{row:?}");
            assert_eq!(assessment.revisions[0].state, state, "{row:?}");
            assert_eq!(
                reserve_kind(&assessment.revisions[0].reservation),
                reserve,
                "{row:?}"
            );
        }
    }

    #[test]
    fn only_final_payment_commits_and_only_proven_no_effect_releases() {
        for &(at_final, at_head, winner, anchor, state, reserve, _) in TABLE {
            let row = (at_final, at_head, winner, anchor);
            let can_commit = state == R::PaidFinalized;
            let can_release = matches!(state, R::ExpiredUnpaid | R::SupersededFinal);
            assert_eq!(reserve == Commit, can_commit, "{row:?}");
            assert_eq!(
                matches!(reserve, ReleaseExpired | ReleaseSuperseded),
                can_release,
                "{row:?}"
            );
        }
    }

    #[test]
    fn unknown_evidence_decides_nothing() {
        assert_eq!(
            classify_revision(&this(), &DealEvidence::Unknown),
            R::Unknown
        );
        let assessment = assess_deal(&[this(), other()], &DealEvidence::Unknown);
        assert_eq!(assessment.state, D::Unknown);
        assert!(assessment
            .revisions
            .iter()
            .all(|revision| revision.reservation == ReservationDecision::Hold));
    }

    #[test]
    fn evidence_for_another_deal_decides_nothing() {
        let DealEvidence::Observed(mut reads) = unconsumed_at(EXPIRY + 1) else {
            unreachable!()
        };
        reads.deal_nullifier = DealNullifier::from_bytes([0xd1; 32]);
        let evidence = DealEvidence::Observed(reads);
        assert_eq!(classify_revision(&this(), &evidence), R::Unknown);
        let assessment = assess_deal(&[this()], &evidence);
        assert_eq!(assessment.state, D::Unknown);
        assert_eq!(
            assessment.revisions[0].reservation,
            ReservationDecision::Hold
        );
    }

    #[test]
    fn a_winner_whose_amounts_disagree_with_its_commitment_is_unknown() {
        for (amount, fee) in [(1, 0), (0, 1)] {
            let mut winner = settled_by(&this(), true);
            winner.amount = BaseUnits::new(winner.amount.get() + amount);
            winner.fee = BaseUnits::new(winner.fee.get() + fee);
            let evidence = reads(EXPIRY - 1, true, true, Some(winner));
            assert_eq!(classify_revision(&this(), &evidence), R::Unknown);
            let assessment = assess_deal(&[this(), other()], &evidence);
            assert_eq!(assessment.state, D::Unknown);
            assert_eq!(
                assessment.revisions[0].reservation,
                ReservationDecision::Hold
            );
        }
    }

    #[test]
    fn an_earlier_revision_with_a_later_expiry_stays_open_after_a_newer_one_expires() {
        let first = revision(THIS, 2_000, 1_000_000);
        let counter = revision(OTHER, 1_500, 900_000);
        let assessment = assess_deal(&[first.clone(), counter.clone()], &unconsumed_at(1_600));
        assert_eq!(assessment.state, D::Open);
        assert_eq!(assessment.revisions[0].state, R::Open);
        assert_eq!(
            assessment.revisions[0].reservation,
            ReservationDecision::Hold
        );
        assert_eq!(
            revision_recovery_action(assessment.revisions[0].state),
            RevisionAction::MaySettle
        );
        assert_eq!(assessment.revisions[1].state, R::ExpiredUnpaid);
        assert!(matches!(
            assessment.revisions[1].reservation,
            ReservationDecision::Release(ref proof) if proof.reason() == NoEffectReason::Expired
        ));

        // Once the earlier revision's expiry is final as well, the deal closes unpaid.
        let assessment = assess_deal(&[first, counter], &unconsumed_at(2_000));
        assert_eq!(assessment.state, D::ClosedUnpaid);
        assert!(assessment
            .revisions
            .iter()
            .all(|revision| matches!(revision.reservation, ReservationDecision::Release(_))));
    }

    #[test]
    fn consumption_at_the_head_but_not_at_a_final_block_holds_every_revision() {
        // `this` has expired at the final anchor, and still nothing moves until the winner
        // is verified.
        let evidence = reads(EXPIRY + 1, false, true, None);
        let assessment = assess_deal(&[this(), other()], &evidence);
        assert_eq!(assessment.state, D::ConsumedUnresolved);
        for revision in &assessment.revisions {
            assert_eq!(revision.state, R::ConsumedUnresolved);
            assert_eq!(revision.reservation, ReservationDecision::Hold);
            assert_eq!(
                revision_recovery_action(revision.state),
                RevisionAction::ResolveWinner
            );
        }
    }

    #[test]
    fn another_revision_supersedes_only_once_its_settlement_is_final() {
        let included = reads(EXPIRY - 1, false, true, Some(settled_by(&other(), false)));
        assert_eq!(classify_revision(&this(), &included), R::SupersededIncluded);
        let final_ = reads(EXPIRY - 1, true, true, Some(settled_by(&other(), true)));
        assert_eq!(classify_revision(&this(), &final_), R::SupersededFinal);
    }

    #[test]
    fn a_winner_outside_the_revision_set_holds_every_reservation() {
        let stranger = revision(DealCommitment::from_bytes([0xc3; 32]), u64::MAX, 5);
        // A final stranger: each revision alone looks superseded, but releasing them would
        // leave a real payment with no reservation.
        let evidence = reads(EXPIRY + 1, true, true, Some(settled_by(&stranger, true)));
        let assessment = assess_deal(&[this(), other()], &evidence);
        assert_eq!(assessment.state, D::UnexplainedConsumption);
        for revision in &assessment.revisions {
            assert_eq!(revision.state, R::SupersededFinal);
            assert_eq!(revision.reservation, ReservationDecision::Hold);
        }
        // A non-final stranger after every revision expired must not close the deal unpaid.
        let evidence = reads(EXPIRY + 1, false, true, Some(settled_by(&stranger, false)));
        let assessment = assess_deal(&[this()], &evidence);
        assert_eq!(assessment.revisions[0].state, R::ExpiredUnpaid);
        assert_eq!(assessment.state, D::UnexplainedConsumption);
        assert_eq!(
            assessment.revisions[0].reservation,
            ReservationDecision::Hold
        );
    }

    #[test]
    fn a_deal_never_closes_unpaid_from_an_empty_or_repeated_revision_set() {
        let evidence = unconsumed_at(EXPIRY + 1);
        let assessment = assess_deal(&[], &evidence);
        assert_eq!(assessment.state, D::Unknown);
        assert!(assessment.revisions.is_empty());

        let assessment = assess_deal(&[this(), this()], &evidence);
        assert_eq!(assessment.state, D::Unknown);
        for revision in &assessment.revisions {
            assert_eq!(revision.state, R::ExpiredUnpaid);
            assert_eq!(revision.reservation, ReservationDecision::Hold);
        }
    }

    #[test]
    fn recovery_actions_follow_revision_state() {
        for (state, action) in [
            (R::Open, RevisionAction::MaySettle),
            (R::PaidIncluded, RevisionAction::AwaitFinality),
            (R::SupersededIncluded, RevisionAction::AwaitFinality),
            (R::ConsumedUnresolved, RevisionAction::ResolveWinner),
            (R::Unknown, RevisionAction::Reconcile),
            (R::PaidFinalized, RevisionAction::Stop),
            (R::SupersededFinal, RevisionAction::Stop),
            (R::ExpiredUnpaid, RevisionAction::Stop),
        ] {
            assert_eq!(revision_recovery_action(state), action, "{state:?}");
        }
    }

    #[test]
    fn revisions_derive_from_their_opening() {
        let mut terms = crate::terms::tests::example_terms();
        terms.fee_policy.fee = BaseUnits::new(2_500);
        terms.fee_policy.recipient = Some(terms.seller_authorization_key.clone());
        let blinding = CommitmentBlinding::from_bytes([0x99; 32]);
        let revision = SignedRevision::from_opening(&terms, &blinding).unwrap();
        assert_eq!(
            revision.commitment(),
            crate::commitment::commit_agreement(&terms, &blinding).unwrap()
        );
        assert_eq!(
            revision.deal_nullifier(),
            crate::commitment::deal_nullifier(&terms).unwrap()
        );
        assert_eq!(revision.expiry(), terms.expiry);
        assert_eq!(revision.total(), BaseUnits::new(1_002_500));

        assert_eq!(
            SignedRevision::new(
                DEAL,
                THIS,
                EXPIRY,
                BaseUnits::new(u128::MAX),
                BaseUnits::new(1)
            ),
            Err(RevisionError::AmountOverflow)
        );
    }

    #[test]
    fn proofs_move_only_their_own_revisions_reservation() {
        let asset = crate::terms::tests::example_terms().asset;
        let unrelated = ReservationId::from_bytes([0xee; 32]);
        let mut ledger = ReservationLedger::new();
        for (id, amount) in [
            (ReservationId::for_revision(&THIS), this().total()),
            (ReservationId::for_revision(&OTHER), other().total()),
            (unrelated, BaseUnits::new(7)),
        ] {
            ledger.reserve(id, asset.clone(), amount, 10).unwrap();
        }

        let evidence = reads(EXPIRY - 1, true, true, Some(settled_by(&other(), true)));
        for revision in assess_deal(&[this(), other()], &evidence).revisions {
            match revision.reservation {
                ReservationDecision::Commit(payment) => {
                    ledger.commit_payment(&payment, 20).unwrap();
                }
                ReservationDecision::Release(proof) => ledger.release_unpaid(&proof).unwrap(),
                ReservationDecision::Hold => {}
            }
        }
        assert_eq!(
            ledger.state(ReservationId::for_revision(&THIS)),
            Some(ReservationState::Released)
        );
        assert_eq!(
            ledger.state(ReservationId::for_revision(&OTHER)),
            Some(ReservationState::Committed)
        );
        assert_eq!(ledger.state(unrelated), Some(ReservationState::Reserved));
        assert_eq!(ledger.reserved_total(&asset).unwrap(), BaseUnits::new(7));
        assert_eq!(
            ledger.committed_in_window(&asset, 20, 100).unwrap(),
            other().total()
        );
    }

    #[test]
    fn reconciliation_cannot_release_a_loser_before_accounting_for_the_winner() {
        let asset = crate::terms::tests::example_terms().asset;
        let loser = ReservationId::for_revision(&THIS);
        let winner = ReservationId::for_revision(&OTHER);
        let mut ledger = ReservationLedger::new();
        ledger
            .reserve(loser, asset.clone(), this().total(), 10)
            .unwrap();
        let evidence = reads(EXPIRY - 1, true, true, Some(settled_by(&other(), true)));
        let before = ledger.reservations().to_vec();
        assert!(matches!(
            ledger.reconcile_deal(&[this(), other()], &evidence, 20),
            Err(PolicyError::UnknownReservation(id)) if id == winner
        ));
        assert_eq!(ledger.reservations(), before);

        ledger.reserve(winner, asset, other().total(), 10).unwrap();
        ledger
            .reconcile_deal(&[this(), other()], &evidence, 20)
            .unwrap();
        assert_eq!(ledger.state(loser), Some(ReservationState::Released));
        assert_eq!(ledger.state(winner), Some(ReservationState::Committed));
        let committed = ledger.reservations().to_vec();
        ledger
            .reconcile_deal(&[this(), other()], &evidence, 30)
            .unwrap();
        assert_eq!(
            ledger.reservations(),
            committed,
            "retry retains original accounting time"
        );
    }

    #[test]
    fn failed_release_rolls_back_the_winners_commit_too() {
        let asset = crate::terms::tests::example_terms().asset;
        let winner = ReservationId::for_revision(&OTHER);
        let mut ledger = ReservationLedger::new();
        ledger.reserve(winner, asset, other().total(), 10).unwrap();
        let evidence = reads(EXPIRY - 1, true, true, Some(settled_by(&other(), true)));
        let before = ledger.reservations().to_vec();
        assert!(ledger
            .reconcile_deal(&[this(), other()], &evidence, 20)
            .is_err());
        assert_eq!(ledger.reservations(), before);
    }

    #[test]
    fn a_final_payment_must_match_the_reserved_amount() {
        let asset = crate::terms::tests::example_terms().asset;
        let id = ReservationId::for_revision(&THIS);
        let mut ledger = ReservationLedger::new();
        ledger
            .reserve(id, asset, BaseUnits::new(this().amount().get()), 10)
            .unwrap();
        let evidence = reads(EXPIRY - 1, true, true, Some(settled_by(&this(), true)));
        let mut assessment = assess_deal(&[this()], &evidence);
        let ReservationDecision::Commit(payment) = assessment.revisions.remove(0).reservation
        else {
            panic!("a final payment commits");
        };
        assert_eq!(
            ledger.commit_payment(&payment, 20),
            Err(PolicyError::PaymentMismatch {
                id,
                reserved: this().amount().get(),
                paid: this().total().get(),
            })
        );
        assert_eq!(ledger.state(id), Some(ReservationState::Reserved));
    }
}
