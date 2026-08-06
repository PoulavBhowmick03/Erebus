"""I1.2 integration tests for negotiation and disclosure against the mock."""

from __future__ import annotations

import asyncio

from erebus_mcp.mock_client import MockErebusClient

from erebus_agents.agent import run_negotiation
from erebus_agents.policy import BuyerPolicy, SellerPolicy

TOKEN = "0xtoken"


def _run(*, budget: int, reserve: int, max_rounds: int, tmp_path):
    store_path = tmp_path / "store.json"
    buyer_client = MockErebusClient(
        identity="0xbuyer",
        store_path=store_path,
        latency_seconds=0,
        spendable_notes=[budget],
    )
    seller_client = MockErebusClient(
        identity="0xseller", store_path=store_path, latency_seconds=0, spendable_notes=[]
    )
    buyer_policy = BuyerPolicy(identity="0xbuyer", budget=budget, deadline_seconds=3600, max_rounds=max_rounds)
    seller_policy = SellerPolicy(identity="0xseller", reserve=reserve, deadline_seconds=3600, max_rounds=max_rounds)

    return asyncio.run(
        run_negotiation(
            buyer_client=buyer_client,
            seller_client=seller_client,
            buyer_policy=buyer_policy,
            seller_policy=seller_policy,
            buyer_address="0xbuyer",
            seller_address="0xseller",
            token=TOKEN,
            store_path=store_path,
            auditor_address="0xauditor",
            max_rounds=max_rounds,
        )
    )


def test_negotiation_settles_when_ranges_overlap(tmp_path):
    record = _run(budget=1000, reserve=700, max_rounds=3, tmp_path=tmp_path)

    assert record.settlement is not None
    assert record.settlement.is_consistent()
    assert record.settlement.agreed_amount <= 1000
    assert record.settlement.agreed_amount >= 700
    assert sorted(record.participants) == sorted(["0xbuyer", "0xseller"])
    assert len(record.offers) >= 1


def test_negotiation_walks_away_when_ranges_never_overlap(tmp_path):
    # Buyer's opening anchor (80% of budget = 400) is below the reserve, and neither side's
    # single fixed counter closes the gap, so this should end without a settlement rather
    # than looping forever.
    record = _run(budget=500, reserve=5000, max_rounds=3, tmp_path=tmp_path)

    assert record.settlement is None
    # Disclosure needs a grant, not a successful settlement.
    assert sorted(record.participants) == sorted(["0xbuyer", "0xseller"])


def test_negotiation_reveal_is_from_a_genuine_third_party(tmp_path):
    # The auditor only calls reveal and never joins the channel or proposes an offer.
    record = _run(budget=1000, reserve=700, max_rounds=3, tmp_path=tmp_path)

    assert "0xauditor" not in record.participants
    assert all(o.proposer != "0xauditor" for o in record.offers)
