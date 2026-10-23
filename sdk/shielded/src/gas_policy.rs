//! Per-method gas limits for the buyer's own sends (operator policy, not live estimates).
//!
//! Monad charges the full gas limit, so one shared 3,000,000 limit made every approve cost as
//! much as a settlement. Each method now signs at its own fixed limit. A live `eth_estimateGas`
//! of the exact call is only a precheck: if the estimate plus [`ESTIMATE_MARGIN_PERCENT`] does not
//! fit under the method's limit, the send is refused before any nonce claim is written or any
//! byte is signed. The limit itself never follows the estimate.
//!
//! The configured `gas_limit` stays as the legacy cap: a claim or signed plan saved before an
//! upgrade (at the old shared limit) resumes at its saved limit when that is within the cap.

/// Native wrap (`deposit()` on the wrapper, with value). Measured on Monad testnet: 51,221.
pub const WRAP_GAS_LIMIT: u64 = 65_000;
/// ERC-20 approval of the pool. Measured on Monad testnet: 52,699.
pub const APPROVE_GAS_LIMIT: u64 = 65_000;
/// Native unwrap (`withdraw(amount)` on the wrapper).
pub const UNWRAP_GAS_LIMIT: u64 = 65_000;
/// Shielded deposit with its proof. Measured on Monad testnet: 1,892,644.
pub const DEPOSIT_GAS_LIMIT: u64 = 2_100_000;
/// Shielded settlement transfer with its proof. Measured on Monad testnet: 2,733,865.
pub const SETTLE_GAS_LIMIT: u64 = 3_010_000;
/// Headroom an estimate must leave under a method's limit.
pub const ESTIMATE_MARGIN_PERCENT: u64 = 5;

/// The limit a send signs at, and the cap a resumed legacy plan must stay within.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GasPolicy {
    /// Gas limit for a new claim of this method.
    pub limit: u64,
    /// Highest saved limit an existing claim or plan may resume at (the configured `gas_limit`).
    pub legacy_cap: u64,
}

impl GasPolicy {
    /// True when `estimate` plus the margin fits under this method's limit.
    #[must_use]
    pub fn admits(&self, estimate: u64) -> bool {
        let needed = u128::from(estimate) * u128::from(100 + ESTIMATE_MARGIN_PERCENT);
        needed <= u128::from(self.limit) * 100
    }

    /// True when a plan saved at `saved` may be resumed: it is this method's limit, or a legacy
    /// limit within the configured cap.
    #[must_use]
    pub const fn resumes(&self, saved: u64) -> bool {
        saved == self.limit || saved <= self.legacy_cap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn margin_is_five_percent_and_exact_at_the_boundary() {
        let policy = GasPolicy {
            limit: 2_100_000,
            legacy_cap: 3_000_000,
        };
        assert!(policy.admits(2_000_000));
        assert!(!policy.admits(2_000_001));
        assert!(policy.admits(1_892_644));
        assert!(!policy.admits(u64::MAX));
    }

    #[test]
    fn measured_monad_estimates_fit_each_method_limit() {
        for (limit, estimate) in [
            (WRAP_GAS_LIMIT, 51_221),
            (APPROVE_GAS_LIMIT, 52_699),
            (DEPOSIT_GAS_LIMIT, 1_892_644),
            (SETTLE_GAS_LIMIT, 2_733_865),
        ] {
            assert!(GasPolicy {
                limit,
                legacy_cap: 3_000_000
            }
            .admits(estimate));
        }
    }

    #[test]
    fn legacy_plans_resume_only_within_the_cap() {
        let policy = GasPolicy {
            limit: SETTLE_GAS_LIMIT,
            legacy_cap: 3_000_000,
        };
        assert!(policy.resumes(3_000_000));
        assert!(policy.resumes(SETTLE_GAS_LIMIT));
        assert!(!policy.resumes(3_005_000));
        assert!(!policy.resumes(30_000_000));
    }
}
