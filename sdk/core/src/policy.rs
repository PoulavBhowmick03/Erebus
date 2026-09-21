//! Spending policy evaluation and reservation accounting.
//!
//! Policy runs below the agent layer (decisions D03): the agent decides what it wants, and
//! this module decides what the configured operator permits. Evaluation is a pure function
//! over caller-supplied state, so denial cases are testable without an agent, a prompt, or a
//! chain.
//!
//! Two rules are specified here and pinned by tests:
//!
//! - The aggregate window is the half-open interval `(now - window, now]`. A record exactly
//!   `window` seconds old is outside the window.
//! - A reservation keeps consuming capacity until it is committed or released. An uncertain
//!   payment therefore stays reserved until reconciliation decides it (roadmap M6).
//!
//! Durable persistence of the ledger is coordinator work. This module is the accounting rule
//! the coordinator must implement, not the store.

use std::collections::{BTreeMap, BTreeSet};

use crate::ids::{AssetId, BaseUnits, KeyBytes};
use crate::terms::AgreementTerms;

/// An aggregate spending ceiling over a rolling window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AggregateLimit {
    /// Maximum committed plus reserved spend allowed in the window.
    pub max: BaseUnits,
    /// Window length in seconds.
    pub window_seconds: u64,
}

/// The operator's configured limits, below the agent layer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpendingPolicy {
    /// Maximum payment plus settlement fee for any single deal.
    pub per_deal_max: BaseUnits,
    /// Assets the operator permits at all. Empty denies every asset.
    pub allowed_assets: BTreeSet<AssetId>,
    /// Per-asset aggregate ceilings. An asset with no entry has no aggregate ceiling.
    pub per_asset_limits: BTreeMap<AssetId, AggregateLimit>,
    /// When present, only these counterparties are permitted.
    pub counterparty_allowlist: Option<BTreeSet<KeyBytes>>,
    /// Counterparties that are never permitted. Denial wins over the allowlist.
    pub counterparty_denylist: BTreeSet<KeyBytes>,
}

/// Spend already accounted against a policy, as supplied by the caller's ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AggregateSpend {
    /// Settled spend inside the policy's window.
    pub committed: BaseUnits,
    /// Reserved spend that has not been committed or released.
    pub reserved: BaseUnits,
}

/// Why a policy denied an agreement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenyReason {
    /// The asset is not in the permitted set.
    AssetNotPermitted {
        /// The rejected asset.
        asset: AssetId,
    },
    /// The deal exceeds the per-deal maximum.
    DealLimitExceeded {
        /// Requested amount.
        amount: u128,
        /// Configured maximum.
        limit: u128,
    },
    /// The aggregate ceiling would be exceeded.
    AggregateLimitExceeded {
        /// The asset at its ceiling.
        asset: AssetId,
        /// Committed plus reserved plus requested.
        total: u128,
        /// Configured ceiling.
        limit: u128,
    },
    /// The counterparty is on the denylist.
    CounterpartyDenied {
        /// The rejected counterparty key.
        counterparty: KeyBytes,
    },
    /// The counterparty is not on a configured allowlist.
    CounterpartyNotAllowed {
        /// The rejected counterparty key.
        counterparty: KeyBytes,
    },
    /// Adding the amounts overflowed 128 bits.
    ArithmeticOverflow {
        /// The asset being accounted.
        asset: AssetId,
    },
}

/// The policy's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDecision {
    /// The agreement is permitted.
    Allow,
    /// The agreement is denied, with the first failing rule.
    Deny(DenyReason),
}

impl PolicyDecision {
    /// Reports whether the decision permits the agreement.
    #[must_use]
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow)
    }
}

/// Evaluates a policy for one agreement and counterparty.
///
/// Rules are checked in a fixed order so the reported reason is deterministic: asset
/// permission, counterparty permission, per-deal maximum, then the aggregate ceiling.
pub fn evaluate(
    policy: &SpendingPolicy,
    terms: &AgreementTerms,
    counterparty: &KeyBytes,
    spent: AggregateSpend,
) -> PolicyDecision {
    if !policy.allowed_assets.contains(&terms.asset) {
        return PolicyDecision::Deny(DenyReason::AssetNotPermitted {
            asset: terms.asset.clone(),
        });
    }
    if policy.counterparty_denylist.contains(counterparty) {
        return PolicyDecision::Deny(DenyReason::CounterpartyDenied {
            counterparty: counterparty.clone(),
        });
    }
    if let Some(allowlist) = &policy.counterparty_allowlist {
        if !allowlist.contains(counterparty) {
            return PolicyDecision::Deny(DenyReason::CounterpartyNotAllowed {
                counterparty: counterparty.clone(),
            });
        }
    }
    let Some(amount) = terms.amount.get().checked_add(terms.fee_policy.fee.get()) else {
        return PolicyDecision::Deny(DenyReason::ArithmeticOverflow {
            asset: terms.asset.clone(),
        });
    };
    if amount > policy.per_deal_max.get() {
        return PolicyDecision::Deny(DenyReason::DealLimitExceeded {
            amount,
            limit: policy.per_deal_max.get(),
        });
    }
    if let Some(limit) = policy.per_asset_limits.get(&terms.asset) {
        let Some(total) = spent
            .committed
            .get()
            .checked_add(spent.reserved.get())
            .and_then(|total| total.checked_add(amount))
        else {
            return PolicyDecision::Deny(DenyReason::ArithmeticOverflow {
                asset: terms.asset.clone(),
            });
        };
        if total > limit.max.get() {
            return PolicyDecision::Deny(DenyReason::AggregateLimitExceeded {
                asset: terms.asset.clone(),
                total,
                limit: limit.max.get(),
            });
        }
    }
    PolicyDecision::Allow
}

/// A caller-generated identifier for one reservation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReservationId([u8; 32]);

impl ReservationId {
    /// Wraps caller-generated bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Returns the raw bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Where a reservation stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservationState {
    /// Capacity is held for a possible settlement.
    Reserved,
    /// The settlement has chain evidence; the amount is committed spend.
    Committed,
    /// The operation provably had no effect; the capacity is free again.
    Released,
}

/// One reservation of spending capacity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    /// Caller-chosen identity, stable across restarts.
    pub id: ReservationId,
    /// The reserved asset.
    pub asset: AssetId,
    /// The reserved amount.
    pub amount: BaseUnits,
    /// Current state.
    pub state: ReservationState,
    /// When the reservation was created.
    pub created_at: u64,
    /// When it was committed, if it was.
    pub settled_at: Option<u64>,
}

/// A reservation could not be recorded or transitioned.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyError {
    /// The policy refused to reserve this agreement.
    #[error("spending policy denied reservation: {0:?}")]
    Denied(Box<DenyReason>),
    /// A reservation with this id already exists.
    #[error("reservation {0:?} already exists")]
    DuplicateReservation(ReservationId),
    /// No reservation with this id exists.
    #[error("reservation {0:?} is unknown")]
    UnknownReservation(ReservationId),
    /// The requested transition is not allowed from the current state.
    #[error("reservation {id:?} cannot move from {from:?} to {to:?}")]
    InvalidTransition {
        /// The reservation.
        id: ReservationId,
        /// Current state.
        from: ReservationState,
        /// Requested state.
        to: ReservationState,
    },
    /// Summing the amounts overflowed 128 bits.
    #[error("reservation totals overflowed 128 bits")]
    ArithmeticOverflow,
}

/// In-memory reservation accounting for one policy.
///
/// The coordinator persists this ledger; the rules for which transitions are legal live
/// here so they cannot drift between call sites.
#[derive(Debug, Clone, Default)]
pub struct ReservationLedger {
    reservations: Vec<Reservation>,
}

impl ReservationLedger {
    /// Creates an empty ledger.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Checks policy and reserves payment plus fee in one exclusive ledger operation.
    ///
    /// The caller must persist the ledger before submission and serialize access across
    /// processes. Uncertain operations remain reserved, regardless of the rolling window.
    pub fn reserve_agreement(
        &mut self,
        id: ReservationId,
        policy: &SpendingPolicy,
        terms: &AgreementTerms,
        counterparty: &KeyBytes,
        now: u64,
    ) -> Result<(), PolicyError> {
        let committed = match policy.per_asset_limits.get(&terms.asset) {
            Some(limit) => self.committed_in_window(&terms.asset, now, limit.window_seconds)?,
            None => BaseUnits::new(0),
        };
        let spent = AggregateSpend {
            committed,
            reserved: self.reserved_total(&terms.asset)?,
        };
        if let PolicyDecision::Deny(reason) = evaluate(policy, terms, counterparty, spent) {
            return Err(PolicyError::Denied(Box::new(reason)));
        }
        let amount = terms
            .amount
            .get()
            .checked_add(terms.fee_policy.fee.get())
            .ok_or(PolicyError::ArithmeticOverflow)?;
        self.reserve(id, terms.asset.clone(), BaseUnits::new(amount), now)
    }

    /// Records already-approved capacity, including fees. Prefer `reserve_agreement`.
    pub fn reserve(
        &mut self,
        id: ReservationId,
        asset: AssetId,
        amount: BaseUnits,
        now: u64,
    ) -> Result<(), PolicyError> {
        if self
            .reservations
            .iter()
            .any(|reservation| reservation.id == id)
        {
            return Err(PolicyError::DuplicateReservation(id));
        }
        self.reservations.push(Reservation {
            id,
            asset,
            amount,
            state: ReservationState::Reserved,
            created_at: now,
            settled_at: None,
        });
        Ok(())
    }

    /// Converts a reservation into committed spend.
    pub fn commit(&mut self, id: ReservationId, now: u64) -> Result<(), PolicyError> {
        let reservation = self.find_mut(id)?;
        if reservation.state != ReservationState::Reserved {
            return Err(PolicyError::InvalidTransition {
                id,
                from: reservation.state,
                to: ReservationState::Committed,
            });
        }
        reservation.state = ReservationState::Committed;
        reservation.settled_at = Some(now);
        Ok(())
    }

    /// Releases a reservation after proving the operation had no effect.
    pub fn release(&mut self, id: ReservationId) -> Result<(), PolicyError> {
        let reservation = self.find_mut(id)?;
        if reservation.state != ReservationState::Reserved {
            return Err(PolicyError::InvalidTransition {
                id,
                from: reservation.state,
                to: ReservationState::Released,
            });
        }
        reservation.state = ReservationState::Released;
        Ok(())
    }

    /// Returns a reservation's state, when it exists.
    #[must_use]
    pub fn state(&self, id: ReservationId) -> Option<ReservationState> {
        self.reservations
            .iter()
            .find(|reservation| reservation.id == id)
            .map(|reservation| reservation.state)
    }

    /// Sums reserved amounts for one asset.
    pub fn reserved_total(&self, asset: &AssetId) -> Result<BaseUnits, PolicyError> {
        let mut total: u128 = 0;
        for reservation in &self.reservations {
            if reservation.state == ReservationState::Reserved && &reservation.asset == asset {
                total = total
                    .checked_add(reservation.amount.get())
                    .ok_or(PolicyError::ArithmeticOverflow)?;
            }
        }
        Ok(BaseUnits::new(total))
    }

    /// Sums committed spend for one asset inside `(now - window, now]`.
    pub fn committed_in_window(
        &self,
        asset: &AssetId,
        now: u64,
        window_seconds: u64,
    ) -> Result<BaseUnits, PolicyError> {
        let mut total: u128 = 0;
        for reservation in &self.reservations {
            if reservation.state != ReservationState::Committed || &reservation.asset != asset {
                continue;
            }
            let Some(settled_at) = reservation.settled_at else {
                continue;
            };
            if settled_at <= now && now - settled_at < window_seconds {
                total = total
                    .checked_add(reservation.amount.get())
                    .ok_or(PolicyError::ArithmeticOverflow)?;
            }
        }
        Ok(BaseUnits::new(total))
    }

    fn find_mut(&mut self, id: ReservationId) -> Result<&mut Reservation, PolicyError> {
        self.reservations
            .iter_mut()
            .find(|reservation| reservation.id == id)
            .ok_or(PolicyError::UnknownReservation(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fees_count_toward_both_limits_and_cannot_overflow() {
        let mut terms = crate::terms::tests::example_terms();
        let counterparty = terms.seller_authorization_key.clone();
        let mut policy = policy(&terms.asset, &counterparty);
        terms.amount = BaseUnits::new(1);
        terms.fee_policy.fee = BaseUnits::new(1_000_000);
        terms.fee_policy.recipient = Some(counterparty.clone());
        let spent = AggregateSpend::default();
        assert!(matches!(
            evaluate(&policy, &terms, &counterparty, spent),
            PolicyDecision::Deny(DenyReason::AggregateLimitExceeded {
                total: 1_000_001,
                ..
            })
        ));
        policy.per_deal_max = BaseUnits::new(10);
        assert!(matches!(
            evaluate(&policy, &terms, &counterparty, spent),
            PolicyDecision::Deny(DenyReason::DealLimitExceeded {
                amount: 1_000_001,
                ..
            })
        ));
        policy.per_asset_limits.clear();
        terms.fee_policy.fee = BaseUnits::new(u128::MAX);
        assert!(matches!(
            evaluate(&policy, &terms, &counterparty, spent),
            PolicyDecision::Deny(DenyReason::ArithmeticOverflow { .. })
        ));
    }

    #[test]
    fn agreement_reservations_hold_fees_until_reconciled() {
        let mut terms = crate::terms::tests::example_terms();
        let counterparty = terms.seller_authorization_key.clone();
        let policy = policy(&terms.asset, &counterparty);
        terms.amount = BaseUnits::new(1);
        terms.fee_policy.fee = BaseUnits::new(999_999);
        terms.fee_policy.recipient = Some(counterparty.clone());
        let mut ledger = ReservationLedger::new();
        let first = ReservationId::from_bytes([1; 32]);
        let second = ReservationId::from_bytes([2; 32]);
        ledger
            .reserve_agreement(first, &policy, &terms, &counterparty, 10)
            .unwrap();
        assert_eq!(
            ledger.reserved_total(&terms.asset).unwrap().get(),
            1_000_000
        );
        assert!(matches!(
            ledger.reserve_agreement(second, &policy, &terms, &counterparty, 1_000_000),
            Err(PolicyError::Denied(reason)) if matches!(*reason, DenyReason::AggregateLimitExceeded { .. })
        ));
        assert_eq!(ledger.state(second), None);
        ledger.commit(first, 1_000_000).unwrap();
        assert_eq!(
            ledger
                .committed_in_window(&terms.asset, 1_000_000, 86400)
                .unwrap()
                .get(),
            1_000_000
        );
        ledger
            .reserve_agreement(second, &policy, &terms, &counterparty, 1_086_400)
            .unwrap();
    }

    #[test]
    fn windows_include_epoch_but_exclude_future_and_zero_length() {
        let asset = crate::terms::tests::example_terms().asset;
        let mut ledger = ReservationLedger::new();
        for (id, time) in [(1, 0), (2, 11)] {
            let id = ReservationId::from_bytes([id; 32]);
            ledger
                .reserve(id, asset.clone(), BaseUnits::new(5), time)
                .unwrap();
            ledger.commit(id, time).unwrap();
        }
        assert_eq!(ledger.committed_in_window(&asset, 1, 10).unwrap().get(), 5);
        assert_eq!(ledger.committed_in_window(&asset, 10, 10).unwrap().get(), 0);
        assert_eq!(ledger.committed_in_window(&asset, 11, 0).unwrap().get(), 0);
    }

    fn policy(asset: &AssetId, counterparty: &KeyBytes) -> SpendingPolicy {
        let mut allowed = BTreeSet::new();
        allowed.insert(asset.clone());
        let mut allowlist = BTreeSet::new();
        allowlist.insert(counterparty.clone());
        let mut limits = BTreeMap::new();
        limits.insert(
            asset.clone(),
            AggregateLimit {
                max: BaseUnits::new(1_000_000),
                window_seconds: 86_400,
            },
        );
        SpendingPolicy {
            per_deal_max: BaseUnits::new(2_000_000),
            allowed_assets: allowed,
            per_asset_limits: limits,
            counterparty_allowlist: Some(allowlist),
            counterparty_denylist: BTreeSet::new(),
        }
    }

    fn asset() -> AssetId {
        AssetId::parse("eip155:10143/erc20:0x00000000000000000000000000000000000000aa")
            .expect("valid asset")
    }

    fn counterparty() -> KeyBytes {
        KeyBytes::new(vec![0x55; 20]).expect("valid key")
    }

    #[test]
    fn allow_and_each_deny_reason() {
        let terms = crate::terms::tests::example_terms();
        let policy = policy(&asset(), &counterparty());
        assert_eq!(
            evaluate(&policy, &terms, &counterparty(), AggregateSpend::default()),
            PolicyDecision::Allow
        );

        let mut denied = policy.clone();
        denied.allowed_assets.clear();
        assert_eq!(
            evaluate(&denied, &terms, &counterparty(), AggregateSpend::default()),
            PolicyDecision::Deny(DenyReason::AssetNotPermitted { asset: asset() })
        );

        let mut denied = policy.clone();
        denied.counterparty_denylist.insert(counterparty());
        assert_eq!(
            evaluate(&denied, &terms, &counterparty(), AggregateSpend::default()),
            PolicyDecision::Deny(DenyReason::CounterpartyDenied {
                counterparty: counterparty()
            })
        );

        let mut denied = policy.clone();
        denied.counterparty_allowlist = Some(BTreeSet::new());
        assert_eq!(
            evaluate(&denied, &terms, &counterparty(), AggregateSpend::default()),
            PolicyDecision::Deny(DenyReason::CounterpartyNotAllowed {
                counterparty: counterparty()
            })
        );

        let mut denied = policy.clone();
        denied.per_deal_max = BaseUnits::new(terms.amount.get() - 1);
        assert_eq!(
            evaluate(&denied, &terms, &counterparty(), AggregateSpend::default()),
            PolicyDecision::Deny(DenyReason::DealLimitExceeded {
                amount: terms.amount.get(),
                limit: terms.amount.get() - 1
            })
        );

        let mut denied = policy;
        denied.per_asset_limits.insert(
            asset(),
            AggregateLimit {
                max: BaseUnits::new(terms.amount.get()),
                window_seconds: 86_400,
            },
        );
        assert_eq!(
            evaluate(
                &denied,
                &terms,
                &counterparty(),
                AggregateSpend {
                    committed: BaseUnits::new(1),
                    reserved: BaseUnits::new(0),
                }
            ),
            PolicyDecision::Deny(DenyReason::AggregateLimitExceeded {
                asset: asset(),
                total: terms.amount.get() + 1,
                limit: terms.amount.get()
            })
        );
    }

    #[test]
    fn limits_are_inclusive_at_the_boundary() {
        let terms = crate::terms::tests::example_terms();
        let mut policy = policy(&asset(), &counterparty());
        policy.per_deal_max = terms.amount;
        policy.per_asset_limits.insert(
            asset(),
            AggregateLimit {
                max: terms.amount,
                window_seconds: 86_400,
            },
        );
        assert_eq!(
            evaluate(&policy, &terms, &counterparty(), AggregateSpend::default()),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn overflow_is_a_denial_not_a_wrap() {
        let terms = crate::terms::tests::example_terms();
        let mut policy = policy(&asset(), &counterparty());
        policy.per_asset_limits.insert(
            asset(),
            AggregateLimit {
                max: BaseUnits::new(u128::MAX),
                window_seconds: 86_400,
            },
        );
        assert_eq!(
            evaluate(
                &policy,
                &terms,
                &counterparty(),
                AggregateSpend {
                    committed: BaseUnits::new(u128::MAX),
                    reserved: BaseUnits::new(0),
                }
            ),
            PolicyDecision::Deny(DenyReason::ArithmeticOverflow { asset: asset() })
        );
    }

    #[test]
    fn reservations_transition_once() {
        let mut ledger = ReservationLedger::new();
        let id = ReservationId::from_bytes([0x01; 32]);
        ledger
            .reserve(id, asset(), BaseUnits::new(500), 100)
            .expect("reserves");
        assert_eq!(
            ledger.reserve(id, asset(), BaseUnits::new(500), 100),
            Err(PolicyError::DuplicateReservation(id))
        );
        assert_eq!(ledger.state(id), Some(ReservationState::Reserved));
        ledger.commit(id, 200).expect("commits");
        assert_eq!(
            ledger.release(id),
            Err(PolicyError::InvalidTransition {
                id,
                from: ReservationState::Committed,
                to: ReservationState::Released
            })
        );
        assert_eq!(
            ledger.reserved_total(&asset()).expect("sums"),
            BaseUnits::new(0)
        );
        assert_eq!(
            ledger
                .committed_in_window(&asset(), 300, 200)
                .expect("sums"),
            BaseUnits::new(500)
        );
    }

    #[test]
    fn reservation_windows_are_half_open() {
        let mut ledger = ReservationLedger::new();
        let id = ReservationId::from_bytes([0x02; 32]);
        ledger
            .reserve(id, asset(), BaseUnits::new(500), 0)
            .expect("reserves");
        ledger.commit(id, 1_000).expect("commits");
        // At now = 1_000 with window 100, the window is (900, 1000]; the record is inside.
        assert_eq!(
            ledger
                .committed_in_window(&asset(), 1_000, 100)
                .expect("sums"),
            BaseUnits::new(500)
        );
        // At now = 1_100, the window is (1000, 1100]; a record exactly at 1000 is outside.
        assert_eq!(
            ledger
                .committed_in_window(&asset(), 1_100, 100)
                .expect("sums"),
            BaseUnits::new(0)
        );
    }

    #[test]
    fn released_capacity_is_available_again() {
        let mut ledger = ReservationLedger::new();
        let id = ReservationId::from_bytes([0x03; 32]);
        ledger
            .reserve(id, asset(), BaseUnits::new(700), 10)
            .expect("reserves");
        assert_eq!(
            ledger.reserved_total(&asset()).expect("sums"),
            BaseUnits::new(700)
        );
        ledger.release(id).expect("releases");
        assert_eq!(
            ledger.reserved_total(&asset()).expect("sums"),
            BaseUnits::new(0)
        );
    }
}
