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


def test_latency_uses_local_clock_not_chain_timestamp_distance():
    buyer = Session(negotiate_deal=[deal()], check_settlement_funding=[payment("ready")], settle_deal=[payment("pending")],
                    recover_deal=[FINAL])
    record = run(buyer, Session(negotiate_deal=[deal()]))
    assert record["latency_s"] == {"first_inclusion_after": 1, "payment_verified_after": 2}
    assert record["stages_ms"]["proof"] is None
    assert record["stages_ms"]["first_inclusion_after"] == 1000
    assert record["stages_ms"]["finalized_verification_after"] == 2000
    assert record["chain_times"]["finalized_anchor_unix"] == 104


def test_failed_delivery_is_retried_by_retrieval_never_by_paying():
    unavailable = {"ok": False, "error": {"code": "ACCESS_UNAVAILABLE", "retry_without_payment": True}}
    buyer = Session(negotiate_deal=[deal()], check_settlement_funding=[payment("ready")], settle_deal=[FINAL],
                    retrieve_service_access=[unavailable, unavailable, RETRIEVED])
    record = run(buyer, Session(negotiate_deal=[deal()]), deliver=True)
    assert record["delivery"] == {"attempts": 3, "resource_sha256": "ef" * 32, "resource_bytes": 37}
    assert "delivery" in record["latency_s"]
    assert record["stages_ms"]["delivery"] == record["latency_s"]["delivery"] * 1000
    assert buyer.calls.count("settle_deal") == 1


def test_unverified_resource_is_not_accepted():
    bad = {"ok": True, "result": {"status": "retrieved", "result": {"resource_verified": False}}}
    buyer = Session(negotiate_deal=[deal()], check_settlement_funding=[payment("ready")], settle_deal=[FINAL],
                    retrieve_service_access=[bad])
    with pytest.raises(HarnessError, match="verification"):
        run(buyer, Session(negotiate_deal=[deal()]), deliver=True)


def x402_retrieved(**overrides):
    receipt = {"resource_verified": True, "payment_verified": False, "delivery_verified": False,
               "resource_sha256": "ef" * 32, "resource_bytes": 37, "seller_reported_payment_finalized": True, **overrides}
    return {"ok": True, "result": {"status": "retrieved", "result": receipt}}


def x402_pending(status="payment_pending"):
    return {"ok": False, "result": {"status": status, "retry_without_payment": True, "payment_verified": False}}


def run_x402(buyer, seller, **options):
    from erebus_agents.metropolis_loop import drive_x402_deal

    ticks = iter(range(100, 1000))
    return asyncio.run(drive_x402_deal(buyer, seller, OPERATION, sleep=no_sleep, clock=lambda: float(next(ticks)), **options))


def test_x402_first_retrieval_pays_and_retries_only_retrieve():
    buyer = Session(negotiate_deal=[deal()], retrieve_service_access=[
        {"ok": False, "error": {"code": "ACCESS_UNAVAILABLE", "retry_without_payment": True}},
        x402_pending(), x402_pending("paid_but_undelivered"), x402_retrieved()])
    record = run_x402(buyer, Session(negotiate_deal=[deal()]))
    assert set(buyer.calls) == {"negotiate_deal", "retrieve_service_access"}
    assert record["retrieval_attempts"] == 4
    assert record["retrieval_states"] == ["ACCESS_UNAVAILABLE", "payment_pending", "paid_but_undelivered", "retrieved"]
    assert record["payment_verified"] is False and record["payment_verification"] == "independent auditor only"


@pytest.mark.parametrize("claim", [{"payment_verified": True}, {"delivery_verified": True}, {"resource_verified": False}])
def test_x402_access_never_becomes_payment_or_delivery_verification(claim):
    buyer = Session(negotiate_deal=[deal()], retrieve_service_access=[x402_retrieved(**claim)])
    with pytest.raises(HarnessError):
        run_x402(buyer, Session(negotiate_deal=[deal()]))


def test_x402_stops_on_non_retryable_failure_and_polling_budget():
    buyer = Session(negotiate_deal=[deal()], retrieve_service_access=[
        {"ok": False, "result": {"status": "rejected", "retry_without_payment": False}}])
    with pytest.raises(HarnessError, match="non-retryable"):
        run_x402(buyer, Session(negotiate_deal=[deal()]))
    buyer = Session(negotiate_deal=[deal()], retrieve_service_access=[x402_pending()] * 2)
    with pytest.raises(HarnessError, match="polling budget"):
        run_x402(buyer, Session(negotiate_deal=[deal()]), max_polls=2)


def test_x402_profile_requires_the_exact_tool_surface():
    from erebus_agents.metropolis_loop import BUYER_TOOLS, X402_BUYER_TOOLS

    assert X402_BUYER_TOOLS == {"negotiate_deal", "retrieve_service_access"}
    assert not X402_BUYER_TOOLS & (BUYER_TOOLS - {"negotiate_deal"})


def test_harness_errors_are_found_inside_task_group_wrappers():
    from erebus_agents.metropolis_loop import _harness_error

    inner = HarnessError("resource not delivered")
    wrapped = BaseExceptionGroup("tasks", [ExceptionGroup("inner", [ValueError("x"), inner])])
    assert _harness_error(wrapped) is inner
    assert _harness_error(BaseExceptionGroup("tasks", [KeyboardInterrupt()])) is None


def test_installed_servers_see_only_installed_commands():
    from erebus_agents.metropolis_loop import server_params

    params = server_params("/unused/python", "/operator/negotiation.json", server_command="/opt/erebus/bin/erebus-mcp-server")
    assert params.command == "/opt/erebus/bin/erebus-mcp-server" and params.args == []
    assert params.env["PATH"].split(":")[0] == "/opt/erebus/bin"
    assert ".venv" not in params.env["PATH"]
