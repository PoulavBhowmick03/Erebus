"""Agent sequencing: settle at most once, recover only by observation."""

import asyncio

import pytest

from erebus_agents.metropolis_loop import HarnessError, drive_deal

OPERATION = "ab" * 32
COMMITMENT = "cd" * 32


def deal(commitment=COMMITMENT):
    return {"ok": True, "result": {"deal_commitment": commitment, "measurements_ms": {"negotiation": 5},
                                   "evidence_file": f"/operator/state/agent/{OPERATION}.0.tx"}}


def payment(status, verified=False, **extra):
    return {"ok": status in {"ok", "ready"}, "result": {"status": status, "payment_verified": verified, **extra}}


FINAL = payment("ok", True, stage="Finalized", winning_commitment=COMMITMENT, measurements_ms={"submission": 3},
                chain_times={"finalized_anchor_block": 9, "finalized_anchor_unix": 104, "inclusion_block": 7, "inclusion_unix": 101})
RETRIEVED = {"ok": True, "result": {"status": "retrieved", "result": {"resource_verified": True, "resource_sha256": "ef" * 32,
                                                                     "resource_bytes": 37}}}


class Session:
    def __init__(self, **replies):
        self.replies = {name: list(values) for name, values in replies.items()}
        self.calls = []

    async def call_tool(self, name, arguments):
        expected = {"evidence_name": f"{OPERATION}.0.tx"} if name == "retrieve_service_access" else {"operation_ref": OPERATION}
        assert arguments == expected
        self.calls.append(name)
        return self.replies[name].pop(0)


async def no_sleep(_):
    pass


def run(buyer, seller, **options):
    ticks = iter(range(100, 1000))
    return asyncio.run(drive_deal(buyer, seller, OPERATION, sleep=no_sleep, clock=lambda: float(next(ticks)), **options))


def test_lost_broadcast_is_recovered_by_observation_without_a_second_settle():
    buyer = Session(negotiate_deal=[deal()], check_settlement_funding=[payment("ready")],
                    settle_deal=[payment("pending", submitted_this_call=True)],
                    recover_deal=[{"ok": False, "error": {"code": "METROPOLIS_UNAVAILABLE"}}, payment("pending"), FINAL])
    record = run(buyer, Session(negotiate_deal=[deal()]))
    assert buyer.calls.count("settle_deal") == 1
    assert record["recover_calls"] == 3
    assert record["payment_verified"] is True


def test_immediate_finality_needs_no_recovery():
    buyer = Session(negotiate_deal=[deal()], check_settlement_funding=[payment("ready")], settle_deal=[FINAL])
    assert run(buyer, Session(negotiate_deal=[deal()]))["recover_calls"] == 0


def test_mismatched_deals_never_reach_payment():
    buyer = Session(negotiate_deal=[deal()])
    with pytest.raises(HarnessError, match="different deals"):
        run(buyer, Session(negotiate_deal=[deal("ef" * 32)]))
    assert buyer.calls == ["negotiate_deal"]


def test_unfunded_buyer_does_not_settle():
    buyer = Session(negotiate_deal=[deal()], check_settlement_funding=[payment("funding_required")])
    with pytest.raises(HarnessError, match="funded"):
        run(buyer, Session(negotiate_deal=[deal()]))
    assert "settle_deal" not in buyer.calls


def test_polling_budget_stops_without_paying_again():
    buyer = Session(negotiate_deal=[deal()], check_settlement_funding=[payment("ready")],
                    settle_deal=[payment("pending")], recover_deal=[payment("pending")] * 2)
    with pytest.raises(HarnessError, match="polling budget"):
        run(buyer, Session(negotiate_deal=[deal()]), max_polls=2)
    assert buyer.calls.count("settle_deal") == 1


def test_finality_for_another_deal_is_rejected():
    other = payment("ok", True, stage="Finalized", winning_commitment="11" * 32)
    buyer = Session(negotiate_deal=[deal()], check_settlement_funding=[payment("ready")], settle_deal=[other])
    with pytest.raises(HarnessError, match="does not match"):
        run(buyer, Session(negotiate_deal=[deal()]))


def test_latencies_come_from_block_times_and_the_settle_return():
    buyer = Session(negotiate_deal=[deal()], check_settlement_funding=[payment("ready")], settle_deal=[payment("pending")],
                    recover_deal=[FINAL])
    record = run(buyer, Session(negotiate_deal=[deal()]))
    # settle returned at clock 100, payment verified at 101; inclusion at 101 and anchor at 104.
    assert record["latency_s"] == {"payment_verified_after": 1, "inclusion": 1, "finality": 3}


def test_failed_delivery_is_retried_by_retrieval_never_by_paying():
    unavailable = {"ok": False, "error": {"code": "ACCESS_UNAVAILABLE", "retry_without_payment": True}}
    buyer = Session(negotiate_deal=[deal()], check_settlement_funding=[payment("ready")], settle_deal=[FINAL],
                    retrieve_service_access=[unavailable, unavailable, RETRIEVED])
    record = run(buyer, Session(negotiate_deal=[deal()]), deliver=True)
    assert record["delivery"] == {"attempts": 3, "resource_sha256": "ef" * 32, "resource_bytes": 37}
    assert "delivery" in record["latency_s"]
    assert buyer.calls.count("settle_deal") == 1


def test_unverified_resource_is_not_accepted():
    bad = {"ok": True, "result": {"status": "retrieved", "result": {"resource_verified": False}}}
    buyer = Session(negotiate_deal=[deal()], check_settlement_funding=[payment("ready")], settle_deal=[FINAL],
                    retrieve_service_access=[bad])
    with pytest.raises(HarnessError, match="verification"):
        run(buyer, Session(negotiate_deal=[deal()]), deliver=True)
