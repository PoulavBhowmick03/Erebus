"""Agent-facing flow: created references, readable errors, bounded waits, one funding tool.

The native commands are stateful fakes (tests/metropolis_fakes.py) that count simulated
broadcasts, so each test can assert that a retry or a forgotten reference never sends twice.
"""

import asyncio
import json
import os
import re
import stat
import sys
from pathlib import Path

import pytest
from mcp import ClientSession
from mcp.client.stdio import StdioServerParameters, stdio_client

from erebus_mcp.metropolis_errors import SEAM_MESSAGES, known_messages
from erebus_mcp.operation_refs import OperationRefs

ROOT = Path(__file__).resolve().parents[2]
FAKES = Path(__file__).with_name("metropolis_fakes.py")


def operator_file(path: Path, data: dict) -> Path:
    path.write_text(json.dumps(data))
    path.chmod(0o600)
    return path


def fake(tmp_path: Path, name: str) -> Path:
    path = tmp_path / f"fake-{name}"
    path.write_text(f"#!/bin/sh\nexec {sys.executable} {FAKES} {name}\n")
    path.chmod(0o700)
    return path


def environment(tmp_path: Path, role: str = "buyer", methods=("approve", "wrap", "deposit", "notes", "sync", "status"),
                maximum: str = "1000", **extra: str) -> dict:
    state = tmp_path / "state"
    negotiation = operator_file(tmp_path / "negotiation.json", {"role": role, "state_root": str(state)})
    env = {"EREBUS_BACKEND": "metropolis", "EREBUS_NEGOTIATION_CONFIG": str(negotiation),
           "EREBUS_NEGOTIATION_CLI": str(fake(tmp_path, "negotiate")), "FAKE_STATE": str(tmp_path / "fake.json"),
           "FAKE_STATE_ROOT": str(state), "PATH": os.environ["PATH"], **extra}
    if role == "buyer":
        env |= {"EREBUS_PAYMENT_CONFIG": str(operator_file(tmp_path / "payment.json", {"mode": "shielded", "state_root": str(state)})),
                "EREBUS_PAYMENT_CLI": str(fake(tmp_path, "payment")),
                "EREBUS_WALLET_CONFIG": str(operator_file(tmp_path / "wallet.json", {"state_root": str(state)})),
                "EREBUS_WALLET_POLICY": str(operator_file(tmp_path / "wallet-policy.json", {"version": 1,
                    "maximum_amount": maximum, "allowed_methods": list(methods), "withdrawal_recipients": []})),
                "EREBUS_WALLET_CLI": str(fake(tmp_path, "wallet"))}
    return env


def fake_state(tmp_path: Path) -> dict:
    path = tmp_path / "fake.json"
    return json.loads(path.read_text()) if path.exists() else {"sends": 0, "ops": {}}


def session(env: dict, script):
    params = StdioServerParameters(command=sys.executable, args=["-m", "erebus_mcp.server"], cwd=str(ROOT), env=env)

    async def run():
        async with stdio_client(params) as (read, write):
            async with ClientSession(read, write) as client:
                await client.initialize()

                async def call(name, **arguments):
                    response = await client.call_tool(name, arguments)
                    return response.structured_content or json.loads(response.content[0].text)

                names = {tool.name for tool in (await client.list_tools()).tools}
                return await script(call, names)
    return asyncio.run(run())


def test_fund_wallet_wraps_approves_deposits_and_waits_for_a_spendable_note(tmp_path):
    env = environment(tmp_path)

    async def script(call, names):
        assert "fund_wallet" in names
        first = await call("fund_wallet", amount="300", wait_seconds=120)
        assert first["ok"] is True, first
        result = first["result"]
        assert [step["kind"] for step in result["steps"]] == ["wrap", "approve", "deposit"]
        assert all(step["state"] == "completed" for step in result["steps"])
        assert result["operation_ref_created"] is True and re.fullmatch(r"[a-f0-9]{64}", result["operation_ref"])
        assert fake_state(tmp_path)["sends"] == 3
        notes = await call("wallet_notes")
        assert [note["commitment"] for note in notes["result"]["result"]["notes"]] == [result["spendable_commitment"]]
        # Continuing the same funding never repeats a finished step.
        again = await call("fund_wallet", amount="300", operation_ref=result["operation_ref"])
        assert again["ok"] is True and fake_state(tmp_path)["sends"] == 3
    session(env, script)


def test_fund_wallet_skips_steps_already_satisfied_and_resumes_after_its_wait_runs_out(tmp_path):
    env = environment(tmp_path, FAKE_BALANCE="500")

    async def script(call, names):
        started = await call("fund_wallet", amount="200", wait_seconds=0)
        assert started["ok"] is False and started["result"]["status"] == "pending"
        assert "fund_wallet again" in started["next_action"]
        reference = started["operation_ref"]
        assert [step["kind"] for step in started["result"]["steps"]] == ["approve", "deposit"]
        assert fake_state(tmp_path)["sends"] == 0
        # An omitted reference continues the unfinished funding instead of planning another.
        resumed = await call("fund_wallet", amount="200", wait_seconds=120)
        assert resumed["ok"] is True and resumed["operation_ref"] == reference
        assert fake_state(tmp_path)["sends"] == 2
        other = await call("fund_wallet", amount="50", wait_seconds=0)
        assert other["ok"] is False or other["operation_ref"] != reference
    session(env, script)


def test_fund_wallet_syncs_a_lagging_history_instead_of_retrying_the_send(tmp_path):
    env = environment(tmp_path, FAKE_HISTORY_LAG="3")

    async def script(call, names):
        funded = await call("fund_wallet", amount="300", wait_seconds=120)
        assert funded["ok"] is True, funded
        state = fake_state(tmp_path)
        assert state["sends"] == 3 and state["lag"] == 0
    session(env, script)


def test_fund_wallet_starts_each_step_once_the_previous_is_submitted(tmp_path):
    env = environment(tmp_path)

    async def script(call, names):
        funded = await call("fund_wallet", amount="300", wait_seconds=120)
        assert funded["ok"] is True, funded
        state = fake_state(tmp_path)
        assert state["sends"] == 3
        # Approve was tried while the wrap still held the signer (busy), and the deposit while the approval
        # was not visible yet (allowance): each synced once and retried under the same reference. No step
        # waited for the previous one's finality before its first attempt.
        assert state["busy_refusals"] == 1
        sends = [call for call in state["wallet_calls"] if call in {"wrap", "approve", "deposit"}]
        assert sends == ["wrap", "approve", "approve", "deposit", "deposit"]
        assert [op["kind"] for op in state["ops"].values()] == ["wrap", "approve", "deposit"]
    session(env, script)


def test_fund_wallet_syncs_and_retries_a_deposit_whose_balance_is_not_visible_yet(tmp_path):
    env = environment(tmp_path, FAKE_HIDDEN_BALANCE="2")

    async def script(call, names):
        funded = await call("fund_wallet", amount="300", wait_seconds=120)
        assert funded["ok"] is True, funded
        state = fake_state(tmp_path)
        assert state["balance_refusals"] == 2 and state["sends"] == 3
        assert all(step["state"] == "completed" for step in funded["result"]["steps"])
    session(env, script)


@pytest.mark.parametrize("kind", ["wrap", "approve", "deposit"])
def test_fund_wallet_resumes_after_the_server_crashes_right_after_each_send(tmp_path, kind):
    env = environment(tmp_path, FAKE_CRASH_AFTER=kind)

    async def crashing(call, names):
        try:
            await asyncio.wait_for(call("fund_wallet", amount="300", wait_seconds=120), 60)
        except Exception:  # the server died mid-call; the client sees the closed stream
            return "crashed"
        return "survived"

    try:
        outcome = session(env, crashing)
    except BaseException:  # noqa: BLE001 - the stdio session may surface the dead server on exit
        outcome = "crashed"
    assert outcome == "crashed"
    state = fake_state(tmp_path)
    assert state["crashed"] is True and state["ops"][next(iter(state["ops"]))]["kind"] == "wrap"
    sent_before = state["sends"]

    async def resumed(call, names):
        # The omitted reference resolves to the unfinished funding saved before the crash.
        return await call("fund_wallet", amount="300", wait_seconds=120)

    result = session(env, resumed)
    assert result["ok"] is True, result
    assert result["result"]["operation_ref_created"] is False
    state = fake_state(tmp_path)
    # The step sent before the crash is never sent again; only the remaining steps are.
    assert state["sends"] == 3 and sent_before == ["wrap", "approve", "deposit"].index(kind) + 1
    assert [op["kind"] for op in state["ops"].values()] == ["wrap", "approve", "deposit"]


USDC_TOKEN = "0x534b2f3a21130d7a60830c2df862319e593943a3"
BUYER = "0x" + "aa" * 20
USDC_METHODS = ("approve", "deposit", "notes", "sync", "status")


def pool_wallet(tmp_path: Path, wrapper: str | None) -> None:
    """The wallet.json `init` writes: the pool asset, the gas signer, and a wrapper only on WMON."""
    operator_file(tmp_path / "wallet.json", {"state_root": str(tmp_path / "state"), "asset": USDC_TOKEN if wrapper is None
                                             else wrapper, "wrapper": wrapper, "signer_address": BUYER})


def test_fund_wallet_on_usdc_approves_and_deposits_without_wrapping(tmp_path):
    env = environment(tmp_path, methods=USDC_METHODS, FAKE_BALANCE="300000", maximum="300000")
    pool_wallet(tmp_path, None)

    async def script(call, names):
        assert "fund_wallet" in names and "wrap_wallet" not in names
        funded = await call("fund_wallet", amount="300000", wait_seconds=120)
        assert funded["ok"] is True, funded
        assert [step["kind"] for step in funded["result"]["steps"]] == ["approve", "deposit"]
        state = fake_state(tmp_path)
        assert state["sends"] == 2 and {op["kind"] for op in state["ops"].values()} == {"approve", "deposit"}
        assert state["balance"] == 0
    session(env, script)


def test_fund_wallet_on_usdc_names_the_exact_shortfall_and_resumes_once_funded(tmp_path):
    env = environment(tmp_path, methods=USDC_METHODS, FAKE_BALANCE="120000", maximum="300000")
    pool_wallet(tmp_path, None)

    async def script(call, names):
        short = await call("fund_wallet", amount="300000", wait_seconds=0)
        error = short["error"]
        assert short["ok"] is False and error["code"] == "FUNDS_REQUIRED" and error["retry_same_operation_ref"] is True
        assert (error["asset"], error["signer"]) == (USDC_TOKEN, BUYER)
        assert (error["asset_balance"], error["required"], error["shortfall"]) == ("120000", "300000", "180000")
        assert "180000 more base units" in error["next_action"] and "allow wrap" not in error["next_action"]
        assert fake_state(tmp_path)["sends"] == 0
        # The operator sends the shortfall; the same reference then plans approve and deposit only.
        assert not (tmp_path / "fake.json").exists()
        (tmp_path / "fake.json").write_text(json.dumps({"sends": 0, "ops": {}, "notes": [], "balance": 300000,
            "allowance": 0, "negotiate_failures": 0, "settled_with_pending": None, "calls": []}))
        resumed = await call("fund_wallet", amount="300000", operation_ref=error["operation_ref"], wait_seconds=120)
        assert resumed["ok"] is True, resumed
        assert [step["kind"] for step in resumed["result"]["steps"]] == ["approve", "deposit"]
        assert fake_state(tmp_path)["sends"] == 2
    session(env, script)


def test_fund_wallet_on_wmon_without_wrap_keeps_its_wrap_hint(tmp_path):
    env = environment(tmp_path, methods=USDC_METHODS, FAKE_BALANCE="10")
    pool_wallet(tmp_path, "0x" + "fb" * 20)

    async def script(call, names):
        short = await call("fund_wallet", amount="300", wait_seconds=0)
        assert short["error"]["code"] == "FUNDS_REQUIRED"
        assert short["error"]["next_action"] == ("the signer holds 10 of the asset; send at least 290 more, or ask "
                                                 "the operator to allow wrap")
        assert "shortfall" not in short["error"]
    session(env, script)


def test_an_omitted_reference_retries_instead_of_sending_again(tmp_path):
    env = environment(tmp_path)

    async def script(call, names):
        wrap = await call("wrap_wallet", amount="100")
        assert wrap["ok"] is True and wrap["operation_ref_created"] is True
        retry = await call("wrap_wallet", amount="100")
        assert retry["operation_ref"] == wrap["operation_ref"] and retry["operation_ref_created"] is False
        assert fake_state(tmp_path)["sends"] == 1
        conflict = await call("wrap_wallet", amount="7")
        assert conflict["ok"] is False and conflict["error"]["code"] == "OPERATION_IN_PROGRESS"
        assert conflict["error"]["operation_ref"] == wrap["operation_ref"]
        assert fake_state(tmp_path)["sends"] == 1
        # Once it is final, the next omitted reference is a new action.
        await call("sync_wallet")
        await call("sync_wallet")
        fresh = await call("wrap_wallet", amount="100")
        assert fresh["operation_ref"] != wrap["operation_ref"] and fake_state(tmp_path)["sends"] == 2
    session(env, script)
    journal = tmp_path / "state" / "agent" / "mcp-operation-refs.json"
    assert stat.S_IMODE(journal.stat().st_mode) == 0o600


def test_errors_carry_a_stable_code_next_action_and_native_text(tmp_path):
    env = environment(tmp_path)

    async def script(call, names):
        approve = await call("approve_wallet", amount="10", operation_ref="ab" * 32)
        # The approve reply names its spender; that public field must not hide a sent approval.
        assert approve["ok"] is True, approve
        missing = await call("deposit_wallet", amount="999", operation_ref="cd" * 32)
        assert missing["error"]["code"] == "ALLOWANCE_REQUIRED"
        assert missing["error"]["native_error"] == "deposit allowance insufficient; run approve first"
        assert missing["error"]["category"] == "WALLET_UNAVAILABLE" and missing["error"]["next_action"]
        over = await call("deposit_wallet", amount="1001")
        assert over["error"]["code"] == "POLICY_REFUSED" and over["error"]["retry_same_operation_ref"] is False
        bad = await call("negotiate_deal", operation_ref="../outside", wait_seconds=0)
        assert bad["error"]["code"] == "INVALID_OPERATION_REF"
        assert fake_state(tmp_path)["sends"] == 1
    session(env, script)
    # A refused request leaves no reference behind to be resumed later.
    entries = json.loads((tmp_path / "state" / "agent" / "mcp-operation-refs.json").read_text())["entries"]
    assert all(entry["amount"] != "1001" for entry in entries)


def test_the_seller_waits_for_the_buyer_with_one_reference_and_returns_the_evidence_name(tmp_path):
    env = environment(tmp_path, role="seller", FAKE_NEGOTIATE_FAILURES="2")

    async def script(call, names):
        assert names == {"negotiate_deal", "list_peers"}
        deal = await call("negotiate_deal", wait_seconds=30)
        assert deal["ok"] is True, deal
        assert deal["result"]["evidence_name"] == f"{deal['operation_ref']}.0.tx"
        calls = json.loads((tmp_path / "fake.json").read_text())["calls"]
        assert calls == [deal["operation_ref"]] * 3
        # Without a wait the same absence is a readable, retryable error.
        env_state = json.loads((tmp_path / "fake.json").read_text())
        env_state["negotiate_failures"] = 1
        (tmp_path / "fake.json").write_text(json.dumps(env_state))
        absent = await call("negotiate_deal", wait_seconds=0)
        assert absent["error"]["code"] == "PEER_UNAVAILABLE" and absent["error"]["retry_same_operation_ref"]
        assert absent["operation_ref"] != deal["operation_ref"]
    session(env, script)


def test_settlement_does_not_sync_the_wallet_first(tmp_path):
    env = environment(tmp_path)

    async def script(call, names):
        deal = await call("negotiate_deal")
        settled = await call("settle_deal", operation_ref=deal["operation_ref"])
        assert settled["result"]["stage"] == "Submitted", settled
        state = fake_state(tmp_path)
        assert state.get("wallet_calls", []) == [] and state["payment_calls"] == ["settle"]
        assert state["sends"] == 1
        # The settled deal is finished; the next omitted negotiation reference is a new deal.
        assert (await call("negotiate_deal"))["operation_ref"] != deal["operation_ref"]
    session(env, script)


def test_a_held_signer_gets_one_sync_and_one_retry_then_settles(tmp_path):
    env = environment(tmp_path)

    async def script(call, names):
        await call("wrap_wallet", amount="100")
        await call("sync_wallet")  # the wrap is one sync away from final
        deal = await call("negotiate_deal")
        settled = await call("settle_deal", operation_ref=deal["operation_ref"])
        assert settled["result"]["stage"] == "Submitted", settled
        state = fake_state(tmp_path)
        assert state["payment_calls"] == ["settle", "settle"]
        assert state["wallet_calls"][-1] == "sync" and state["wallet_calls"].count("sync") == 2
        assert state["settled_with_pending"] == [] and state["sends"] == 2
    session(env, script)


def test_a_still_held_signer_returns_the_retryable_refusal_after_one_retry(tmp_path):
    env = environment(tmp_path)

    async def script(call, names):
        await call("wrap_wallet", amount="100")
        deal = await call("negotiate_deal")
        held = await call("settle_deal", operation_ref=deal["operation_ref"])
        assert held["ok"] is False and held["error"]["code"] == "WALLET_OPERATION_PENDING", held
        assert held["error"]["retry_same_operation_ref"] is True and held["operation_ref"] == deal["operation_ref"]
        state = fake_state(tmp_path)
        assert state["payment_calls"] == ["settle", "settle"] and state["wallet_calls"].count("sync") == 1
        assert state["sends"] == 1  # only the wrap; nothing was settled
        # The agent syncs as told and retries the same reference.
        await call("sync_wallet")
        settled = await call("settle_deal", operation_ref=deal["operation_ref"])
        assert settled["result"]["stage"] == "Submitted" and fake_state(tmp_path)["sends"] == 2
    session(env, script)


def test_a_deposit_in_flight_reports_the_note_held_then_settles_after_one_sync(tmp_path):
    env = environment(tmp_path, FAKE_BALANCE="500")

    async def script(call, names):
        await call("approve_wallet", amount="100")
        await call("sync_wallet")
        await call("sync_wallet")
        await call("deposit_wallet", amount="100")
        await call("sync_wallet")
        deal = await call("negotiate_deal")
        settled = await call("settle_deal", operation_ref=deal["operation_ref"])
        assert settled["result"]["stage"] == "Submitted", settled
        assert fake_state(tmp_path)["payment_calls"] == ["settle", "settle"]
    session(env, script)


def test_an_unfunded_buyer_gets_funding_required_with_a_next_action_and_nothing_sent(tmp_path):
    env = environment(tmp_path, FAKE_BALANCE="500")

    async def script(call, names):
        await call("approve_wallet", amount="100")
        await call("sync_wallet")
        await call("sync_wallet")
        await call("deposit_wallet", amount="100")
        deal = await call("negotiate_deal")
        # The deposit needs two more syncs: one sync and one retry are not enough.
        held = await call("settle_deal", operation_ref=deal["operation_ref"])
        assert held["error"]["code"] == "WALLET_OPERATION_PENDING", held
        checked = await call("check_settlement_funding", operation_ref=deal["operation_ref"])
        assert checked["ok"] is True and checked["result"]["status"] == "ready", checked
        assert fake_state(tmp_path)["sends"] == 2
    session(env, script)


def test_every_mapped_message_exists_where_it_is_produced():
    rust = "".join(path.read_text() for path in (ROOT / "sdk/shielded/src").rglob("*.rs"))
    seam = "".join(path.read_text() for path in (ROOT / "sdk/py/src/erebus").glob("*.py"))
    for message in known_messages():
        source = seam if message in SEAM_MESSAGES else rust
        assert f'"{message}"' in source, message


def test_created_references_resume_or_conflict_and_survive_a_restart(tmp_path):
    refs = OperationRefs(str(tmp_path))
    first, created = refs.resolve("deposit", "5")
    assert created and refs.resolve("deposit", "5") == (first, False)
    assert OperationRefs(str(tmp_path)).resolve("deposit", "5") == (first, False)
    assert refs.resolve("deposit", "6")["ref"] == first
    second, created = refs.resolve("deposit", "6", finished=lambda entry: True)
    assert created and second != first
    assert refs.resolve("wrap", "5")[1] is True


def test_the_seam_keeps_only_short_public_native_errors(tmp_path):
    from erebus._metropolis import _native_error
    assert _native_error('{"status":"error","error":"negotiation endpoint unavailable"}') == "negotiation endpoint unavailable"
    assert _native_error("not json", '{"error":"paired wallet recovery failed"}') == "paired wallet recovery failed"
    assert _native_error('{"error":"see https://rpc.example/key"}') is None
    assert _native_error('{"error":"' + "x" * 201 + '"}') is None
    assert _native_error('{"error":"tab\\there"}') is None


@pytest.mark.parametrize("value", [-1, 901])
def test_waits_are_bounded(tmp_path, value):
    env = environment(tmp_path)

    async def script(call, names):
        assert (await call("negotiate_deal", wait_seconds=value))["error"]["code"] == "INVALID_REQUEST"
        assert (await call("fund_wallet", amount="1", wait_seconds=value))["error"]["code"] == "INVALID_REQUEST"
    session(env, script)


def test_unrecognized_native_text_never_reaches_the_agent():
    from erebus import MetropolisError
    from erebus_mcp.metropolis_errors import failure
    reply = failure(MetropolisError("native operation failed", native_error="private note opening 1234"), "WALLET_UNAVAILABLE")
    assert reply["error"]["code"] == "WALLET_UNAVAILABLE" and "native_error" not in reply["error"]
    assert "private" not in json.dumps(reply)
    known = failure(MetropolisError("x", native_error="negotiation endpoint unavailable"), "METROPOLIS_UNAVAILABLE")
    assert known["error"]["native_error"] == "negotiation endpoint unavailable"


def test_server_refusals_say_whether_the_same_reference_may_be_retried():
    from erebus_mcp.metropolis_errors import refused
    assert refused("WALLET_OPERATION_PENDING", "WALLET_UNAVAILABLE", "sync")["error"]["retry_same_operation_ref"] is True
    assert refused("OPERATION_IN_PROGRESS", "WALLET_UNAVAILABLE", "finish")["error"]["retry_same_operation_ref"] is True
    assert refused("INVALID_REQUEST", "WALLET_UNAVAILABLE", "fix it")["error"]["retry_same_operation_ref"] is False


def seller_environment(tmp_path: Path, **extra: str) -> dict:
    return environment(tmp_path, role="seller", FAKE_PEERS="b1,b2", **extra)


def test_a_seller_negotiates_with_each_connecting_buyer_in_turn(tmp_path):
    env = seller_environment(tmp_path, FAKE_CONNECTING="b2,b1")

    async def script(call, names):
        assert {"negotiate_deal", "list_peers"} <= names
        listed = await call("list_peers")
        assert [peer["peer"] for peer in listed["result"]["peers"]] == ["b1", "b2"]
        first = await call("negotiate_deal", wait_seconds=0)
        second = await call("negotiate_deal", wait_seconds=0)
        assert (first["ok"], second["ok"]) == (True, True), (first, second)
        assert (first["result"]["peer"], second["result"]["peer"]) == ("b2", "b1")
        assert first["operation_ref"] != second["operation_ref"]
        assert first["result"]["deal_commitment"] != second["result"]["deal_commitment"]
        # Nobody else is connecting: a retryable answer for the same, new reference.
        idle = await call("negotiate_deal", wait_seconds=0)
        assert idle["ok"] is False and idle["error"]["code"] == "NO_PEER_CONNECTED", idle
        assert idle["error"]["retry_same_operation_ref"] is True
        assert idle["error"]["native_error"] == "no configured peer connected before the deadline"
        again = await call("negotiate_deal", wait_seconds=0)
        assert again["operation_ref"] == idle["operation_ref"] and again["operation_ref_created"] is False
    session(env, script)


def test_seller_peer_errors_are_stable_and_not_retryable(tmp_path):
    env = seller_environment(tmp_path, FAKE_CONNECTING="b1")

    async def script(call, names):
        unknown = await call("negotiate_deal", peer="b9", wait_seconds=0)
        assert unknown["error"]["code"] == "PEER_NOT_CONFIGURED"
        assert unknown["error"]["retry_same_operation_ref"] is False
        malformed = await call("negotiate_deal", peer="../b1", wait_seconds=0)
        assert malformed["error"]["code"] == "PEER_NOT_CONFIGURED"
        bound = await call("negotiate_deal", wait_seconds=0)
        assert bound["ok"] is True and bound["result"]["peer"] == "b1"
        other = await call("negotiate_deal", operation_ref=bound["operation_ref"], peer="b2", wait_seconds=0)
        assert other["error"]["code"] == "OPERATION_BOUND_TO_OTHER_PEER"
        assert other["error"]["retry_same_operation_ref"] is False
    session(env, script)


def test_omitted_references_resume_per_peer(tmp_path):
    # b1 connects first, binds its operation natively, and the exchange is then uncertain.
    env = seller_environment(tmp_path, FAKE_CONNECTING="b1,b2", FAKE_UNCERTAIN="b1")

    async def script(call, names):
        uncertain = await call("negotiate_deal", wait_seconds=0)
        assert uncertain["error"]["code"] == "PEER_UNAVAILABLE", uncertain
        first_ref = uncertain["operation_ref"]
        # A call for b2 does not take b1's operation: it gets a new one.
        second = await call("negotiate_deal", peer="b2", wait_seconds=0)
        assert second["ok"] is True and second["result"]["peer"] == "b2", second
        assert second["operation_ref"] != first_ref
        # A call for b2 again resumes nothing of b1's and finds no b2 connecting.
        idle = await call("negotiate_deal", peer="b2", wait_seconds=0)
        assert idle["error"]["code"] == "NO_PEER_CONNECTED" and idle["operation_ref"] not in {first_ref, second["operation_ref"]}
        # A call naming no peer resumes b1's original operation.
        resumed = await call("negotiate_deal", wait_seconds=0)
        assert resumed["ok"] is True and resumed["operation_ref"] == first_ref, resumed
        assert resumed["result"]["peer"] == "b1"
    session(env, script)


def test_a_buyer_has_no_peer_tools(tmp_path):
    env = environment(tmp_path)

    async def script(call, names):
        assert "list_peers" not in names
        reply = await call("negotiate_deal", wait_seconds=0)
        assert reply["ok"] is True and "peer" not in reply["result"]
    session(env, script)


def test_seller_references_resolve_by_peer(tmp_path):
    refs = OperationRefs(str(tmp_path))
    unbound, created = refs.resolve_peer("negotiate", None)
    assert created and refs.resolve_peer("negotiate", "b1") == (unbound, False)
    refs.update(unbound, peer="b1")
    for_b2, created = refs.resolve_peer("negotiate", "b2")
    assert created and for_b2 != unbound
    assert refs.resolve_peer("negotiate", "b1") == (unbound, False)
    # With no peer named, the oldest bound unfinished entry wins.
    assert refs.resolve_peer("negotiate", None) == (unbound, False)
    refs.update(unbound, done=True)
    assert refs.resolve_peer("negotiate", None) == (for_b2, False)
    refs.update(for_b2, peer_unknown=True)
    assert refs.resolve_peer("negotiate", "b2")[1] is True
    assert refs.resolve_peer("negotiate", None) == (for_b2, False)
