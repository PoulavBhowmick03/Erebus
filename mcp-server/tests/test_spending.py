"""Unit tests for SpendGuard: per-token, per-deal, and daily-cumulative caps (9.1)."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from erebus_mcp.config import SpendingLimits, TokenSpendingLimit
from erebus_mcp.spending import SpendGuard


def _guard(tmp_path: Path, **caps: TokenSpendingLimit) -> SpendGuard:
    limits = SpendingLimits(by_token=caps)
    return SpendGuard(limits, tmp_path / "spending.json")


def test_operation_reconciliation_counts_a_settlement_once(tmp_path: Path) -> None:
    guard = _guard(tmp_path, **{"0xtoken": TokenSpendingLimit(daily=10)})
    operation_id = "op_" + "ab" * 32

    guard.record("0xtoken", 7, operation_id)
    guard.record("0xtoken", 7, operation_id)

    assert guard.check("0xtoken", 3) is None
    assert guard.check("0xtoken", 4) is not None


def test_operation_reconciliation_rejects_a_different_spend(tmp_path: Path) -> None:
    guard = _guard(tmp_path)
    operation_id = "op_" + "ab" * 32
    guard.record("0xtoken", 7, operation_id)

    with pytest.raises(ValueError):
        guard.record("0xtoken", 8, operation_id)


def test_no_configured_cap_never_refuses(tmp_path: Path) -> None:
    guard = SpendGuard(SpendingLimits(), tmp_path / "spending.json")
    assert guard.check("0xtoken", 10**30) is None


def test_amount_within_per_deal_cap_passes(tmp_path: Path) -> None:
    guard = _guard(tmp_path, **{"0xtoken": TokenSpendingLimit(per_deal=500)})
    assert guard.check("0xtoken", 500) is None  # at the boundary, inclusive


def test_amount_over_per_deal_cap_is_refused(tmp_path: Path) -> None:
    guard = _guard(tmp_path, **{"0xtoken": TokenSpendingLimit(per_deal=500)})
    reason = guard.check("0xtoken", 501)
    assert reason is not None
    assert "500" not in reason  # never leak the configured number to the agent


def test_token_lookup_is_case_insensitive(tmp_path: Path) -> None:
    guard = _guard(tmp_path, **{"0xtoken": TokenSpendingLimit(per_deal=500)})
    assert guard.check("0xTOKEN", 501) is not None


def test_a_different_token_is_unaffected_by_another_tokens_cap(tmp_path: Path) -> None:
    guard = _guard(tmp_path, **{"0xtoken": TokenSpendingLimit(per_deal=500)})
    assert guard.check("0xother", 10**30) is None


def test_daily_cap_blocks_once_cumulative_spend_would_exceed_it(tmp_path: Path) -> None:
    guard = _guard(tmp_path, **{"0xtoken": TokenSpendingLimit(daily=1000)})
    assert guard.check("0xtoken", 600) is None
    guard.record("0xtoken", 600)

    assert guard.check("0xtoken", 500) is not None  # 600 + 500 > 1000
    assert guard.check("0xtoken", 400) is None  # 600 + 400 == 1000, at the boundary


def test_split_deal_evasion_of_a_per_deal_cap_is_caught_by_the_daily_cap(
    tmp_path: Path,
) -> None:
    """A per-deal cap alone doesn't stop N small deals summing past the real limit; the
    daily cumulative cap is what closes that hole (roadmap 9.3's limit-evasion eval)."""
    guard = _guard(tmp_path, **{"0xtoken": TokenSpendingLimit(per_deal=100, daily=250)})

    assert guard.check("0xtoken", 100) is None
    guard.record("0xtoken", 100)
    assert guard.check("0xtoken", 100) is None
    guard.record("0xtoken", 100)
    # A third 100 would total 300, over the 250 daily cap, even though each deal alone
    # clears the per-deal cap of 100.
    assert guard.check("0xtoken", 100) is not None


def test_a_refused_or_failed_attempt_does_not_count_against_the_cap(tmp_path: Path) -> None:
    guard = _guard(tmp_path, **{"0xtoken": TokenSpendingLimit(daily=1000)})
    # check() alone never mutates state; only record() does.
    assert guard.check("0xtoken", 999) is None
    assert guard.check("0xtoken", 999) is None


def test_restart_reloads_persisted_spend_from_disk(tmp_path: Path) -> None:
    state_path = tmp_path / "spending.json"
    limits = SpendingLimits(by_token={"0xtoken": TokenSpendingLimit(daily=1000)})

    first = SpendGuard(limits, state_path)
    first.record("0xtoken", 700)

    second = SpendGuard(limits, state_path)  # simulates a fresh process after a restart
    assert second.check("0xtoken", 400) is not None  # 700 + 400 > 1000
    assert second.check("0xtoken", 300) is None


def test_state_file_persists_amounts_as_decimal_strings(tmp_path: Path) -> None:
    """Base-unit amounts exceed 2**53 routinely; the on-disk format must not round-trip
    through a JSON number (same rule as the wire boundary, F37)."""
    state_path = tmp_path / "spending.json"
    limits = SpendingLimits(by_token={"0xtoken": TokenSpendingLimit()})
    big = 12_345_678_901_234_567_890

    guard = SpendGuard(limits, state_path)
    guard.record("0xtoken", big)

    raw = json.loads(state_path.read_text())
    assert raw["spent"]["0xtoken"] == str(big)


def test_a_new_utc_day_resets_the_counter(tmp_path: Path, monkeypatch) -> None:
    state_path = tmp_path / "spending.json"
    state_path.write_text(
        json.dumps({"date": "2000-01-01", "spent": {"0xtoken": "999999"}})
    )
    limits = SpendingLimits(by_token={"0xtoken": TokenSpendingLimit(daily=1000)})
    guard = SpendGuard(limits, state_path)

    # The stale entry is dated 2000-01-01; today's cumulative spend should read as zero.
    assert guard.check("0xtoken", 1000) is None
