//! Pre-flight inspection of an operator's configuration, identity, and chain state.
//!
//! Every setup failure in this stack presents the same way: `apply_actions` reverts with a
//! bare `Contract error` naming nothing, after a proof has already been generated and paid
//! for. A wrong chain id derives storage slots nobody wrote to. A missing allowance reverts
//! inside `collect_fee` before a single action is applied. An unregistered counterparty fails
//! at `open_channel` with no hint about which side is unregistered. See friction.md F20.
//!
//! This module answers those questions before anything is spent, and every failure carries a
//! repair instruction rather than a diagnosis. A report that says "allowance is 0" is a
//! restatement of the problem; one that says "run `erebus-cli approve`" is a fix.
//!
//! Nothing here writes. Every check is a read, so running `doctor` is always safe.

use serde::Serialize;

/// Outcome of one inspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// The check passed.
    Pass,
    /// Usable now, but it will stop working. A warning must never block an operation.
    Warn,
    /// A write will fail until this is repaired.
    Fail,
    /// The check could not run, usually because something it depends on failed first.
    Skipped,
}

/// One inspection and, when it did not pass, how to repair it.
///
/// Serialize only. `name` is a fixed identifier chosen at compile time so callers can match
/// on it; accepting an arbitrary string back would defeat that.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    /// Stable identifier, safe to match on.
    pub name: &'static str,
    /// Outcome.
    pub status: Status,
    /// What was observed. States the reading, not the remedy.
    pub detail: String,
    /// One direct action that repairs it. Present only when the status is not `Pass`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repair: Option<String>,
}

impl Check {
    /// A passing check. Carries its reading so a report is useful even when nothing is wrong.
    pub fn pass(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: Status::Pass,
            detail: detail.into(),
            repair: None,
        }
    }

    /// A check that will not block a write today but will later.
    pub fn warn(name: &'static str, detail: impl Into<String>, repair: impl Into<String>) -> Self {
        Self {
            name,
            status: Status::Warn,
            detail: detail.into(),
            repair: Some(repair.into()),
        }
    }

    /// A check that blocks writes until repaired.
    pub fn fail(name: &'static str, detail: impl Into<String>, repair: impl Into<String>) -> Self {
        Self {
            name,
            status: Status::Fail,
            detail: detail.into(),
            repair: Some(repair.into()),
        }
    }

    /// A check whose precondition failed, so its result would be meaningless.
    pub fn skipped(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: Status::Skipped,
            detail: detail.into(),
            repair: None,
        }
    }
}

/// The full inspection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    /// Every check, in the order it ran.
    pub checks: Vec<Check>,
}

impl Report {
    /// Whether a proof-bearing write can be attempted.
    ///
    /// A skipped check does not make this true. If a check could not run, the thing it would
    /// have verified is unverified, and reporting that as healthy is how a `doctor` becomes
    /// worse than no `doctor`.
    pub fn ready(&self) -> bool {
        self.checks
            .iter()
            .all(|check| matches!(check.status, Status::Pass | Status::Warn))
    }

    /// Checks that block a write, in the order they should be repaired.
    pub fn blocking(&self) -> impl Iterator<Item = &Check> {
        self.checks
            .iter()
            .filter(|check| matches!(check.status, Status::Fail | Status::Skipped))
    }

    /// One repair instruction per unhealthy check.
    pub fn repairs(&self) -> Vec<&str> {
        self.checks
            .iter()
            .filter(|check| check.status != Status::Pass)
            .filter_map(|check| check.repair.as_deref())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_with_only_passes_and_warnings_is_ready() {
        let report = Report {
            checks: vec![
                Check::pass("rpc", "block 13095252"),
                Check::warn(
                    "allowance",
                    "covers 2 more writes",
                    "top up before the next run",
                ),
            ],
        };
        assert!(report.ready());
        assert_eq!(report.blocking().count(), 0);
        assert_eq!(report.repairs(), vec!["top up before the next run"]);
    }

    /// A check that could not run has verified nothing. Treating that as healthy would let an
    /// operator spend a proof on a configuration `doctor` never actually inspected.
    #[test]
    fn a_skipped_check_is_not_ready() {
        let report = Report {
            checks: vec![
                Check::pass("rpc", "block 1"),
                Check::skipped(
                    "allowance",
                    "token address unreadable, so the read was not run",
                ),
            ],
        };
        assert!(!report.ready());
        assert_eq!(report.blocking().count(), 1);
    }

    #[test]
    fn every_unhealthy_check_carries_a_repair_and_every_pass_omits_one() {
        let report = Report {
            checks: vec![
                Check::pass("rpc", "block 1"),
                Check::fail(
                    "allowance",
                    "0 against a 2 STRK fee",
                    "run erebus-cli approve",
                ),
                Check::warn("balance", "0.5 STRK", "fund the account"),
            ],
        };
        assert!(!report.ready());
        assert_eq!(report.repairs().len(), 2, "one per unhealthy check");
        for check in &report.checks {
            assert_eq!(
                check.repair.is_some(),
                check.status != Status::Pass,
                "{} carries the wrong repair shape",
                check.name
            );
        }
    }

    #[test]
    fn a_pass_serializes_without_a_repair_field() {
        let json = serde_json::to_value(Check::pass("rpc", "block 1")).expect("serializes");
        assert!(json.get("repair").is_none());
        assert_eq!(json["status"], "pass");
    }
}
