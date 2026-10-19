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
        assert names == {"negotiate_deal"}
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


def test_settlement_waits_for_pending_wallet_transactions_to_finish(tmp_path):
    env = environment(tmp_path)

    async def script(call, names):
        await call("wrap_wallet", amount="100")
        assert fake_state(tmp_path)["ops"] and any(
            op["state"] == "submitted" for op in fake_state(tmp_path)["ops"].values())
        deal = await call("negotiate_deal")
        settled = await call("settle_deal", operation_ref=deal["operation_ref"])
        assert settled["result"]["stage"] == "Submitted"
        assert fake_state(tmp_path)["settled_with_pending"] == []
        # The settled deal is finished; the next omitted negotiation reference is a new deal.
        assert (await call("negotiate_deal"))["operation_ref"] != deal["operation_ref"]
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
