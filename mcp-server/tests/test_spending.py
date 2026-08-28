"""Reservation-state tests for the MCP spending boundary."""

from __future__ import annotations

import json
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import pytest

from erebus_mcp.config import SpendingLimits, TokenSpendingLimit
from erebus_mcp.spending import SpendGuard

DAY_ONE = 1_700_000_000
DAY_TWO = DAY_ONE + 86_400


def _guard(tmp_path: Path, **caps: TokenSpendingLimit) -> SpendGuard:
    return SpendGuard(SpendingLimits(by_token=caps), tmp_path / "spending.json")


def _id(byte: str) -> str:
    return "op_" + byte * 64


def test_reservation_counts_before_rust_starts(tmp_path: Path) -> None:
    guard = _guard(tmp_path, **{"0xtoken": TokenSpendingLimit(daily=10)})
    assert guard.reserve("0xtoken", 7, _id("a"), at=DAY_ONE) is None
    assert guard.check("0xtoken", 4, at=DAY_ONE) is not None
    assert guard.check("0xtoken", 3, at=DAY_ONE) is None


def test_epoch_timestamp_is_not_replaced_with_local_time(tmp_path: Path) -> None:
    guard = _guard(tmp_path, **{"0xtoken": TokenSpendingLimit(daily=10)})
    operation_id = _id("a")
    assert guard.reserve("0xtoken", 7, operation_id, at=0) is None
    guard.observe("0xtoken", 7, operation_id, outcome="effect", accepted_at=0)
    assert guard.check("0xtoken", 4, at=0) is not None


def test_two_concurrent_reservations_cannot_both_cross_the_cap(tmp_path: Path) -> None:
    guard = _guard(tmp_path, **{"0xtoken": TokenSpendingLimit(daily=10)})

    with ThreadPoolExecutor(max_workers=2) as pool:
        decisions = list(
            pool.map(
                lambda item: guard.reserve("0xtoken", 7, item, at=DAY_ONE),
                [_id("a"), _id("b")],
            )
        )

    assert sum(decision is None for decision in decisions) == 1
    assert sum(decision is not None for decision in decisions) == 1


def test_same_operation_is_idempotent_but_cannot_change_binding(tmp_path: Path) -> None:
    guard = _guard(tmp_path)
    operation_id = _id("a")
    assert guard.reserve("0xtoken", 7, operation_id, at=DAY_ONE) is None
    assert guard.reserve("0xTOKEN", 7, operation_id, at=DAY_ONE) is None
    with pytest.raises(ValueError):
        guard.reserve("0xtoken", 8, operation_id, at=DAY_ONE)


def test_effect_commits_once_by_chain_acceptance_day(tmp_path: Path) -> None:
    guard = _guard(tmp_path, **{"0xtoken": TokenSpendingLimit(daily=10)})
    operation_id = _id("a")
    guard.reserve("0xtoken", 7, operation_id, at=DAY_ONE)
    guard.observe(
        "0xtoken", 7, operation_id, outcome="effect", accepted_at=DAY_ONE
    )
    guard.observe(
        "0xtoken", 7, operation_id, outcome="effect", accepted_at=DAY_ONE
    )
    assert guard.check("0xtoken", 4, at=DAY_ONE) is not None
    assert guard.check("0xtoken", 10, at=DAY_TWO) is None


@pytest.mark.parametrize("outcome", ["pending", "unknown", "effect"])
def test_uncertain_or_untimed_effect_stays_reserved(tmp_path: Path, outcome: str) -> None:
    guard = _guard(tmp_path, **{"0xtoken": TokenSpendingLimit(daily=10)})
    operation_id = _id("a")
    guard.reserve("0xtoken", 7, operation_id, at=DAY_ONE)
    guard.observe("0xtoken", 7, operation_id, outcome=outcome)
    assert guard.check("0xtoken", 4, at=DAY_TWO) is not None


@pytest.mark.parametrize("outcome", ["no_effect", "reverted"])
def test_proven_absence_releases_the_reservation(tmp_path: Path, outcome: str) -> None:
    guard = _guard(tmp_path, **{"0xtoken": TokenSpendingLimit(daily=10)})
    operation_id = _id("a")
    guard.reserve("0xtoken", 7, operation_id, at=DAY_ONE)
    guard.observe("0xtoken", 7, operation_id, outcome=outcome)
    assert guard.check("0xtoken", 10, at=DAY_ONE) is None


def test_only_python_reservations_are_released_after_exclusive_snapshot(
    tmp_path: Path,
) -> None:
    guard = _guard(tmp_path)
    absent = _id("a")
    present = _id("b")
    guard.reserve("0xtoken", 7, absent, at=DAY_ONE)
    guard.reserve("0xtoken", 8, present, at=DAY_ONE)
    guard.release_unjournalled({present})
    assert guard.reserved_operation_ids() == {present}


def test_restart_reloads_a_reservation_from_disk(tmp_path: Path) -> None:
    state_path = tmp_path / "spending.json"
    limits = SpendingLimits(by_token={"0xtoken": TokenSpendingLimit(daily=1000)})
    SpendGuard(limits, state_path).reserve("0xtoken", 700, _id("a"), at=DAY_ONE)
    restarted = SpendGuard(limits, state_path)
    assert restarted.check("0xtoken", 400, at=DAY_ONE) is not None


def test_state_file_uses_decimal_strings_and_mode_0600(tmp_path: Path) -> None:
    state_path = tmp_path / "spending.json"
    big = 12_345_678_901_234_567_890
    guard = SpendGuard(SpendingLimits(), state_path)
    guard.reserve("0xtoken", big, _id("a"), at=DAY_ONE)
    raw = json.loads(state_path.read_text())
    assert raw["operations"][_id("a")]["amount"] == str(big)
    assert state_path.stat().st_mode & 0o777 == 0o600


def test_old_counter_migrates_to_a_fail_closed_reservation(tmp_path: Path) -> None:
    state_path = tmp_path / "spending.json"
    state_path.write_text(
        json.dumps({"date": "2000-01-01", "spent": {"0xtoken": "999"}})
    )
    guard = SpendGuard(
        SpendingLimits(by_token={"0xtoken": TokenSpendingLimit(daily=1000)}), state_path
    )
    assert guard.check("0xtoken", 2, at=DAY_TWO) is not None


def test_per_deal_cap_does_not_leak_its_value(tmp_path: Path) -> None:
    guard = _guard(tmp_path, **{"0xtoken": TokenSpendingLimit(per_deal=500)})
    reason = guard.reserve("0xTOKEN", 501, _id("a"), at=DAY_ONE)
    assert reason is not None
    assert "500" not in reason


def test_other_tokens_and_unconfigured_caps_are_unaffected(tmp_path: Path) -> None:
    guard = _guard(tmp_path, **{"0xtoken": TokenSpendingLimit(per_deal=1)})
    assert guard.reserve("0xother", 10**30, _id("a"), at=DAY_ONE) is None
