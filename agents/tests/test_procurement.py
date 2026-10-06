"""Procurement policy, auditor review, and the mock rehearsal."""

import asyncio

import pytest
from erebus_mcp.interface import DisclosedRecord, DisclosedSettlement

from erebus_agents.procurement import Catalog, Requisition, review_disclosure, supplier_policy
from erebus_agents.procurement_demo import rehearse

REQUISITION = Requisition("gpu-hours", quantity=100, max_unit_price=12)
CATALOG = Catalog("gpu-hours", floor_unit_price=9)


def record(settlement):
    return DisclosedRecord(channel_id="ch", participants=["a", "b"], offers=[], settlement=settlement)


def test_ceiling_is_quantity_times_unit_price():
    assert REQUISITION.ceiling == 1200
    assert CATALOG.reserve(100) == 900


@pytest.mark.parametrize("quantity,price", [(0, 5), (5, 0), (-1, 5)])
def test_requisition_rejects_non_positive_values(quantity, price):
    with pytest.raises(ValueError):
        Requisition("x", quantity, price)


def test_supplier_will_not_quote_an_item_it_does_not_list():
    with pytest.raises(ValueError, match="does not list"):
        supplier_policy("s", Catalog("storage", 1), REQUISITION)


def test_review_passes_a_consistent_deal_under_the_ceiling():
    settlement = DisclosedSettlement(acceptance="o1", agreed_amount=1000, paid_amount=1000)
    assert review_disclosure(record(settlement), REQUISITION).compliant


@pytest.mark.parametrize("settlement", [
    None,
    DisclosedSettlement(acceptance="o1", agreed_amount=1000, paid_amount=None),
    DisclosedSettlement(acceptance="o1", agreed_amount=1000, paid_amount=900),
    DisclosedSettlement(acceptance="o1", agreed_amount=1300, paid_amount=1300),
])
def test_review_fails_on_missing_evidence_mismatch_or_overspend(settlement):
    review = review_disclosure(record(settlement), REQUISITION)
    assert not review.compliant and review.reasons


def run(requisition, catalog, tmp_path):
    return asyncio.run(rehearse(requisition, catalog, store_dir=tmp_path))


def test_rehearsal_settles_and_the_auditor_finds_it_compliant(tmp_path):
    result = run(REQUISITION, CATALOG, tmp_path)
    assert result["settled"] and result["audit"]["compliant"]
    assert 900 <= result["agreed_amount"] <= REQUISITION.ceiling


def test_rehearsal_ends_without_settlement_when_the_supplier_floor_is_above_the_ceiling(tmp_path):
    result = run(REQUISITION, Catalog("gpu-hours", floor_unit_price=50), tmp_path)
    assert result["settled"] is False
