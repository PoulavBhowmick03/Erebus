"""Unit tests for DisclosedSettlement.consistency() (roadmap 9.2).

The defect this fixes: `paid_amount is None` used to report the settlement as consistent,
so a tool that could not obtain payment evidence answered "yes" instead of "I could not
check." These pin the fail-closed behavior directly, without needing a live client.
"""

from __future__ import annotations

from erebus_mcp.interface import Consistency, DisclosedSettlement


def test_matching_paid_and_agreed_amounts_are_consistent() -> None:
    settlement = DisclosedSettlement(acceptance="a", agreed_amount=100, paid_amount=100)
    assert settlement.consistency() is Consistency.CONSISTENT


def test_differing_paid_and_agreed_amounts_are_inconsistent() -> None:
    settlement = DisclosedSettlement(acceptance="a", agreed_amount=100, paid_amount=1)
    assert settlement.consistency() is Consistency.INCONSISTENT


def test_missing_paid_amount_is_unknown_not_consistent() -> None:
    """The fix: absent evidence must not read as a pass."""
    settlement = DisclosedSettlement(acceptance="a", agreed_amount=100, paid_amount=None)
    assert settlement.consistency() is Consistency.UNKNOWN
