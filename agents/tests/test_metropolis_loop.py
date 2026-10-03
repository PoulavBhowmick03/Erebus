"""Agent sequencing: settle at most once, recover only by observation."""

import asyncio

import pytest

from erebus_agents.metropolis_loop import HarnessError, drive_deal

OPERATION = "ab" * 32
COMMITMENT = "cd" * 32


def deal(commitment=COMMITMENT):
    return {"ok": True, "result": {"deal_commitment": commitment, "measurements_ms": {"negotiation": 5}}}


def payment(status, verified=False, **extra):
    return {"ok": status in {"ok", "ready"}, "result": {"status": status, "payment_verified": verified, **extra}}


FINAL = payment("ok", True, stage="Finalized", winning_commitment=COMMITMENT, measurements_ms={"submission": 3})


class Session:
    def __init__(self, **replies):
        self.replies = {name: list(values) for name, values in replies.items()}
        self.calls = []

    async def call_tool(self, name, arguments):
        assert arguments == {"operation_ref": OPERATION}
        self.calls.append(name)
        return self.replies[name].pop(0)


async def no_sleep(_):
    pass


def run(buyer, seller, **options):
    return asyncio.run(drive_deal(buyer, seller, OPERATION, sleep=no_sleep, **options))


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
