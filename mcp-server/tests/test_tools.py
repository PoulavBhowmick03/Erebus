"""I1.3 tests through the official `mcp` client, independent of `agents/` code.

The tests start `server.py` as a subprocess over stdio.
"""

from __future__ import annotations

import asyncio
import json
import secrets
import stat
import tempfile
import time
from pathlib import Path

import pytest
from mcp import ClientSession
from mcp.client.stdio import StdioServerParameters, stdio_client

REPO_ROOT = Path(__file__).resolve().parents[2]
SERVER_PATH = REPO_ROOT / "mcp-server" / "src" / "server.py"


@pytest.fixture(autouse=True)
def protocol_4_operation_ids(monkeypatch: pytest.MonkeyPatch):
    """Existing behavior tests all cross the protocol-4 caller-id boundary."""
    original = ClientSession.call_tool

    async def call_tool(self, name, arguments=None, **kwargs):  # type: ignore[no-untyped-def]
        arguments = dict(arguments or {})
        if name in {
            "open_channel",
            "propose_offer",
            "counter_offer",
            "accept_and_settle",
            "grant_viewing_key",
        }:
            arguments.setdefault("operation_id", "op_" + secrets.token_hex(32))
        return await original(self, name, arguments, **kwargs)

    monkeypatch.setattr(ClientSession, "call_tool", call_tool)


def _server_params(
    store_path: Path,
    role: str = "payer",
    identity: str = "0xbuyer",
    spendable_notes: str = "100,150",
    extra_env: dict[str, str] | None = None,
) -> StdioServerParameters:
    env = {
        "AGENT_ADDRESS": identity,
        "PROVING_SERVICE_URL": "http://unused.invalid",
        "EREBUS_MOCK_STORE_PATH": str(store_path),
        "EREBUS_MOCK_LATENCY_SECONDS": "0",
        "EREBUS_MOCK_SPENDABLE_NOTES": spendable_notes,
        "EREBUS_SETTLEMENT_ROLE": role,
        "UV_CACHE_DIR": str(Path(tempfile.gettempdir()) / "erebus-uv-cache"),
    }
    if extra_env:
        env.update(extra_env)
    return StdioServerParameters(
        command="uv",
        args=["run", "python", str(SERVER_PATH)],
        cwd=str(REPO_ROOT),
        env=env,
    )


def _structured(result) -> dict:
    if result.structured_content is not None:
        return result.structured_content
    try:
        return json.loads(result.content[0].text)
    except json.JSONDecodeError as error:
        raise AssertionError(repr(result.content[0].text)) from error


def test_the_protocol_and_payment_planning_methods_are_exposed_with_descriptions(tmp_path):
    """Checks the seven §4 tools and the polling helper.

    The protocol has no push notification. Server-side polling uses one agent tool call.
    The separate assertion detects changes to the protocol method set.
    """

    async def run():
        async with stdio_client(_server_params(tmp_path / "store.json")) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                listed = await session.list_tools()
                names = {t.name for t in listed.tools}
                assert names >= {
                    "open_channel",
                    "propose_offer",
                    "counter_offer",
                    "read_channel_state",
                    "accept_and_settle",
                    "grant_viewing_key",
                    "reveal",
                }
                assert names - {
                    "wait_for_offers",
                    "get_note_balance",
                    "doctor",
                    "reconcile",
                    "resume_operation",
                    "rebuild_state",
                } == {
                    "open_channel",
                    "propose_offer",
                    "counter_offer",
                    "read_channel_state",
                    "accept_and_settle",
                    "grant_viewing_key",
                    "reveal",
                }, "an unexpected tool appeared, or a protocol method vanished"
                assert all(t.description for t in listed.tools)

    asyncio.run(run())


def test_balance_total(tmp_path):
    async def run():
        async with stdio_client(_server_params(tmp_path / "store.json")) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                balance = _structured(await session.call_tool("get_note_balance", {}))
                assert balance["result"]["spendable_notes"] == ["150", "100"]
                assert balance["result"]["total"] == "250"

    asyncio.run(run())


def test_doctor_reports_ready(tmp_path):
    async def run():
        async with stdio_client(_server_params(tmp_path / "store.json")) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                report = _structured(await session.call_tool("doctor", {}))
                assert report["ok"] is True
                assert report["result"]["ready"] is True
                assert report["result"]["checks"]
                assert report["result"]["repairs"] == []

    asyncio.run(run())


def test_payer_cannot_write_an_offer_it_cannot_later_settle(tmp_path):
    async def run():
        async with stdio_client(_server_params(tmp_path / "store.json")) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                opened = await session.call_tool("open_channel", {"counterparty": "0xseller"})
                handle = _structured(opened)["result"]["channel_handle"]
                result = await session.call_tool(
                    "propose_offer",
                    {
                        "channel_handle": handle,
                        "amount": 300,
                        "token": "0xtoken",
                        "deadline": 9999999999,
                        "memo_hash": 0,
                    },
                )
                body = _structured(result)
                assert body["ok"] is False
                assert body["error"]["code"] == "INSUFFICIENT_NOTES"

                state = _structured(
                    await session.call_tool("read_channel_state", {"channel_handle": handle})
                )
                assert state["result"]["offers"] == []

    asyncio.run(run())


def test_payee_server_structurally_refuses_to_accept(tmp_path):
    async def run():
        async with stdio_client(_server_params(tmp_path / "store.json", role="payee")) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                opened = await session.call_tool("open_channel", {"counterparty": "0xbuyer"})
                handle = _structured(opened)["result"]["channel_handle"]
                result = await session.call_tool(
                    "accept_and_settle", {"channel_handle": handle, "offer_id": "anything"}
                )
                body = _structured(result)
                assert body["ok"] is False
                assert body["error"]["code"] == "INVALID_REQUEST"
                assert "configured as payee" in body["error"]["message"]

    asyncio.run(run())


def test_two_mcp_servers_settle_with_the_buyer_as_payer(tmp_path):
    """The production topology: two independent MCP processes, one shared pool state."""

    async def run():
        store = tmp_path / "store.json"
        seller_params = _server_params(store, role="payee", identity="0xseller")
        buyer_params = _server_params(store, role="payer", identity="0xbuyer")
        async with stdio_client(seller_params) as (seller_read, seller_write):
            async with ClientSession(seller_read, seller_write) as seller:
                await seller.initialize()
                opened = await seller.call_tool("open_channel", {"counterparty": "0xbuyer"})
                handle = _structured(opened)["result"]["channel_handle"]
                proposed = await seller.call_tool(
                    "propose_offer",
                    {
                        "channel_handle": handle,
                        "amount": 150,
                        "token": "0xtoken",
                        "deadline": 9999999999,
                        "memo_hash": 0,
                    },
                )
                offer_id = _structured(proposed)["result"]["offer_id"]

                async with stdio_client(buyer_params) as (buyer_read, buyer_write):
                    async with ClientSession(buyer_read, buyer_write) as buyer:
                        await buyer.initialize()
                        # The buyer settles with its own handle. A handle is a key into one
                        # client's own state, so the seller's does not resolve here.
                        buyer_opened = await buyer.call_tool(
                            "open_channel", {"counterparty": "0xseller"}
                        )
                        buyer_handle = _structured(buyer_opened)["result"]["channel_handle"]
                        payable = _structured(await buyer.call_tool("get_note_balance", {}))
                        assert int(payable["result"]["total"]) >= 150

                        settled = _structured(
                            await buyer.call_tool(
                                "accept_and_settle",
                                {"channel_handle": buyer_handle, "offer_id": offer_id},
                            )
                        )
                        assert settled["ok"] is True
                        assert settled["result"]["selected_input"] == "150"
                        assert settled["result"]["change"] == "0"

                        balance = _structured(await buyer.call_tool("get_note_balance", {}))
                        assert balance["result"]["spendable_notes"] == ["100"]

                state = _structured(
                    await seller.call_tool("read_channel_state", {"channel_handle": handle})
                )
                assert state["result"]["settlements"][0]["agreed_amount"] == "150"

    asyncio.run(run())


def test_accept_and_settle_refuses_a_settlement_above_the_configured_per_deal_cap(tmp_path):
    """9.1: the cap lives below the agent, at the MCP layer, not in agent policy."""

    async def run():
        store = tmp_path / "store.json"
        seller_params = _server_params(store, role="payee", identity="0xseller")
        buyer_params = _server_params(
            store,
            role="payer",
            identity="0xbuyer",
            extra_env={"EREBUS_SPENDING_LIMITS": '{"0xtoken": {"per_deal": "100"}}'},
        )
        async with stdio_client(seller_params) as (seller_read, seller_write):
            async with ClientSession(seller_read, seller_write) as seller:
                await seller.initialize()
                opened = await seller.call_tool("open_channel", {"counterparty": "0xbuyer"})
                handle = _structured(opened)["result"]["channel_handle"]
                proposed = await seller.call_tool(
                    "propose_offer",
                    {
                        "channel_handle": handle,
                        "amount": 150,  # above the 100 per-deal cap
                        "token": "0xtoken",
                        "deadline": 9999999999,
                        "memo_hash": 0,
                    },
                )
                offer_id = _structured(proposed)["result"]["offer_id"]

                async with stdio_client(buyer_params) as (buyer_read, buyer_write):
                    async with ClientSession(buyer_read, buyer_write) as buyer:
                        await buyer.initialize()
                        # Protocol 3 handles are opaque and owner-scoped, so the payer
                        # settles through its own handle, not the proposer's. Both map to
                        # the same channel; only the handle string differs.
                        buyer_handle = _structured(
                            await buyer.call_tool(
                                "open_channel", {"counterparty": "0xseller"}
                            )
                        )["result"]["channel_handle"]

                        refused = _structured(
                            await buyer.call_tool(
                                "accept_and_settle",
                                {"channel_handle": buyer_handle, "offer_id": offer_id},
                            )
                        )
                        assert refused["ok"] is False
                        assert refused["error"]["code"] == "SPENDING_LIMIT_EXCEEDED"
                        assert refused["error"]["retryable"] is False
                        # The message must never leak the configured threshold: a cap an
                        # agent can read is a cap an agent can plan around.
                        assert "100" not in refused["error"]["message"]
                        assert "150" not in refused["error"]["message"]

                # The blocked call must never have spent: the offer is still settleable,
                # not consumed.
                state = _structured(
                    await seller.call_tool("read_channel_state", {"channel_handle": handle})
                )
                assert state["result"]["settlements"] == []

    asyncio.run(run())


def test_spending_cap_is_enforced_across_a_server_restart(tmp_path):
    """9.1 exit criterion: restarting the server does not reset daily spend."""

    async def run():
        store = tmp_path / "store.json"
        spend_state = tmp_path / "spend.json"
        buyer_env = {
            "EREBUS_SPENDING_LIMITS": '{"0xtoken": {"daily": "100"}}',
            "EREBUS_SPENDING_STATE_PATH": str(spend_state),
        }

        # First buyer process settles 60 against its 100 daily cap, then exits.
        seller_one = _server_params(store, role="payee", identity="0xseller1")
        async with stdio_client(seller_one) as (seller_read, seller_write):
            async with ClientSession(seller_read, seller_write) as seller:
                await seller.initialize()
                opened = await seller.call_tool("open_channel", {"counterparty": "0xbuyer"})
                handle_one = _structured(opened)["result"]["channel_handle"]
                proposed = await seller.call_tool(
                    "propose_offer",
                    {
                        "channel_handle": handle_one,
                        "amount": 60,
                        "token": "0xtoken",
                        "deadline": 9999999999,
                        "memo_hash": 0,
                    },
                )
                offer_one = _structured(proposed)["result"]["offer_id"]

                buyer_one = _server_params(
                    store, role="payer", identity="0xbuyer", extra_env=buyer_env
                )
                async with stdio_client(buyer_one) as (buyer_read, buyer_write):
                    async with ClientSession(buyer_read, buyer_write) as buyer:
                        await buyer.initialize()
                        buyer_handle_one = _structured(
                            await buyer.call_tool(
                                "open_channel", {"counterparty": "0xseller1"}
                            )
                        )["result"]["channel_handle"]
                        settled = _structured(
                            await buyer.call_tool(
                                "accept_and_settle",
                                {"channel_handle": buyer_handle_one, "offer_id": offer_one},
                            )
                        )
                        assert settled["ok"] is True

        # A brand new buyer process, same identity and spending-state path: simulates a
        # restart. 60 already spent today + 60 more would exceed the 100 daily cap.
        seller_two = _server_params(store, role="payee", identity="0xseller2")
        async with stdio_client(seller_two) as (seller_read, seller_write):
            async with ClientSession(seller_read, seller_write) as seller:
                await seller.initialize()
                opened = await seller.call_tool("open_channel", {"counterparty": "0xbuyer"})
                handle_two = _structured(opened)["result"]["channel_handle"]
                proposed = await seller.call_tool(
                    "propose_offer",
                    {
                        "channel_handle": handle_two,
                        "amount": 60,
                        "token": "0xtoken",
                        "deadline": 9999999999,
                        "memo_hash": 0,
                    },
                )
                offer_two = _structured(proposed)["result"]["offer_id"]

                buyer_two = _server_params(
                    store, role="payer", identity="0xbuyer", extra_env=buyer_env
                )
                async with stdio_client(buyer_two) as (buyer_read, buyer_write):
                    async with ClientSession(buyer_read, buyer_write) as buyer:
                        await buyer.initialize()
                        buyer_handle_two = _structured(
                            await buyer.call_tool(
                                "open_channel", {"counterparty": "0xseller2"}
                            )
                        )["result"]["channel_handle"]
                        refused = _structured(
                            await buyer.call_tool(
                                "accept_and_settle",
                                {"channel_handle": buyer_handle_two, "offer_id": offer_two},
                            )
                        )
                        assert refused["ok"] is False
                        assert refused["error"]["code"] == "SPENDING_LIMIT_EXCEEDED"

    asyncio.run(run())


def test_mcp_settle_change(tmp_path):
    """The cross-layer case exact-subset settlement couldn't do at all: buyer holds one 5
    note, seller asks 3. Through the real MCP call path, not mock_client directly."""

    async def run():
        store = tmp_path / "store.json"
        seller_params = _server_params(store, role="payee", identity="0xseller", spendable_notes="")
        buyer_params = _server_params(store, role="payer", identity="0xbuyer", spendable_notes="5")
        async with stdio_client(seller_params) as (seller_read, seller_write):
            async with ClientSession(seller_read, seller_write) as seller:
                await seller.initialize()
                opened = await seller.call_tool("open_channel", {"counterparty": "0xbuyer"})
                handle = _structured(opened)["result"]["channel_handle"]
                proposed = await seller.call_tool(
                    "propose_offer",
                    {
                        "channel_handle": handle,
                        "amount": 3,
                        "token": "0xtoken",
                        "deadline": 9999999999,
                        "memo_hash": 0,
                    },
                )
                offer_id = _structured(proposed)["result"]["offer_id"]

                async with stdio_client(buyer_params) as (buyer_read, buyer_write):
                    async with ClientSession(buyer_read, buyer_write) as buyer:
                        await buyer.initialize()
                        # The buyer settles with its own handle. A handle is a key into one
                        # client's own state, so the seller's does not resolve here.
                        buyer_opened = await buyer.call_tool(
                            "open_channel", {"counterparty": "0xseller"}
                        )
                        buyer_handle = _structured(buyer_opened)["result"]["channel_handle"]
                        payable = _structured(await buyer.call_tool("get_note_balance", {}))
                        assert payable["result"]["total"] == "5"

                        settled = _structured(
                            await buyer.call_tool(
                                "accept_and_settle",
                                {"channel_handle": buyer_handle, "offer_id": offer_id},
                            )
                        )
                        assert settled["ok"] is True
                        assert settled["result"]["selected_input"] == "5"
                        assert settled["result"]["change"] == "2"

                        balance = _structured(await buyer.call_tool("get_note_balance", {}))
                        assert balance["result"]["spendable_notes"] == ["2"]

                state = _structured(
                    await seller.call_tool("read_channel_state", {"channel_handle": handle})
                )
                assert state["result"]["settlements"][0]["agreed_amount"] == "3"

    asyncio.run(run())


def test_open_channel_and_propose_offer_round_trip(tmp_path):
    async def run():
        async with stdio_client(_server_params(tmp_path / "store.json")) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()

                opened = await session.call_tool("open_channel", {"counterparty": "0xseller"})
                body = _structured(opened)
                assert body["ok"] is True
                handle = body["result"]["channel_handle"]
                assert handle.startswith("ch_")

                proposed = await session.call_tool(
                    "propose_offer",
                    {
                        "channel_handle": handle,
                        "amount": 100,
                        "token": "0xtoken",
                        "deadline": 9999999999,
                        "memo_hash": 0,
                    },
                )
                proposed_body = _structured(proposed)
                assert proposed_body["ok"] is True
                assert "offer_id" in proposed_body["result"]

                state = _structured(
                    await session.call_tool("read_channel_state", {"channel_handle": handle})
                )
                assert isinstance(state["result"]["offers"][0]["deal_id"], str)

    asyncio.run(run())


def test_deal_grant_uses_a_private_file_and_never_returns_the_capsule(tmp_path):
    async def run():
        store = tmp_path / "store.json"
        grant_path = tmp_path / "deal.grant.json"
        seller_params = _server_params(store, role="payee", identity="0xseller")
        async with stdio_client(seller_params) as (read, write):
            async with ClientSession(read, write) as seller:
                await seller.initialize()
                opened = await seller.call_tool("open_channel", {"counterparty": "0xbuyer"})
                handle = _structured(opened)["result"]["channel_handle"]
                await seller.call_tool(
                    "propose_offer",
                    {
                        "channel_handle": handle,
                        "amount": 100,
                        "token": "0xtoken",
                        "deadline": 9999999999,
                        "memo_hash": 0,
                    },
                )
                state = _structured(
                    await seller.call_tool("read_channel_state", {"channel_handle": handle})
                )
                deal_id = state["result"]["offers"][0]["deal_id"]
                export_id = "op_" + "cd" * 32
                export_args = {
                    "operation_id": export_id,
                    "channel_handle": handle,
                    "deal_id": deal_id,
                    "grantee": "0xauditor",
                    "expires_at": int(time.time()) + 600,
                    "output_path": str(grant_path),
                }
                exported = _structured(
                    await seller.call_tool(
                        "grant_viewing_key",
                        export_args,
                    )
                )
                assert exported["ok"] is True
                assert "viewing_key" not in exported["result"]
                assert "ciphertext" not in json.dumps(exported)
                assert stat.S_IMODE(grant_path.stat().st_mode) == 0o600
                replayed = _structured(
                    await seller.call_tool("grant_viewing_key", export_args)
                )
                assert replayed["result"] == exported["result"]

        auditor_params = _server_params(store, role="payee", identity="0xauditor")
        async with stdio_client(auditor_params) as (read, write):
            async with ClientSession(read, write) as auditor:
                await auditor.initialize()
                revealed = _structured(
                    await auditor.call_tool("reveal", {"grant_path": str(grant_path)})
                )
                assert revealed["ok"] is True
                assert {offer["deal_id"] for offer in revealed["result"]["offers"]} == {deal_id}

        assert grant_path.exists()

    asyncio.run(run())


def test_a_settlement_error_comes_back_as_parseable_structured_json(tmp_path):
    """Checks that I1.3 errors preserve ``SettlementErrorCode`` as structured data."""

    async def run():
        async with stdio_client(_server_params(tmp_path / "store.json")) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()

                opened = await session.call_tool("open_channel", {"counterparty": "0xseller"})
                handle = _structured(opened)["result"]["channel_handle"]

                # Reading state for an offer that doesn't exist -> OFFER_UNKNOWN, surfaced
                # through accept_and_settle.
                result = await session.call_tool(
                    "accept_and_settle", {"channel_handle": handle, "offer_id": "does-not-exist"}
                )
                body = _structured(result)

                assert body["ok"] is False
                assert body["error"]["code"] == "OFFER_UNKNOWN"
                assert isinstance(body["error"]["retryable"], bool)
                # The MCP call succeeds. Its payload carries the protocol failure.
                assert result.is_error is False

    asyncio.run(run())


def test_a_full_128_bit_memo_hash_survives_a_round_trip_as_hex(tmp_path):
    """F37: the documented 128-bit range was unreachable by a conforming JSON caller.

    JSON has one numeric type and it is an IEEE-754 double, so a bare number above 2**53 is
    rounded before the server ever sees it. This value has bits set in the top half and in
    the low bits, so any rounding in either direction changes it.
    """
    memo = 0xDEADBEEF_CAFEBABE_0123456789ABCDEF

    async def run():
        async with stdio_client(_server_params(tmp_path / "store.json")) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                opened = await session.call_tool("open_channel", {"counterparty": "0xseller"})
                handle = _structured(opened)["result"]["channel_handle"]

                written = await session.call_tool(
                    "propose_offer",
                    {
                        "channel_handle": handle,
                        "amount": 100,
                        "token": "0xtoken",
                        "deadline": 9999999999,
                        "memo_hash": f"0x{memo:032x}",
                    },
                )
                assert _structured(written)["ok"] is True

                state = _structured(
                    await session.call_tool("read_channel_state", {"channel_handle": handle})
                )
                returned = state["result"]["offers"][0]["terms"]["memo_hash"]

                # Hex on the way out too: a JSON number would be rounded by the client.
                assert isinstance(returned, str)
                assert int(returned, 16) == memo

    asyncio.run(run())


def test_an_oversized_or_malformed_memo_hash_is_refused_before_any_write(tmp_path):
    async def run():
        async with stdio_client(_server_params(tmp_path / "store.json")) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                opened = await session.call_tool("open_channel", {"counterparty": "0xseller"})
                handle = _structured(opened)["result"]["channel_handle"]

                for bad in (f"0x{1 << 128:x}", "not-hex", ""):
                    body = _structured(
                        await session.call_tool(
                            "propose_offer",
                            {
                                "channel_handle": handle,
                                "amount": 100,
                                "token": "0xtoken",
                                "deadline": 9999999999,
                                "memo_hash": bad,
                            },
                        )
                    )
                    assert body["ok"] is False, f"{bad!r} should be refused"
                    assert body["error"]["code"] == "INVALID_REQUEST"

                state = _structured(
                    await session.call_tool("read_channel_state", {"channel_handle": handle})
                )
                assert state["result"]["offers"] == []

    asyncio.run(run())


def test_a_large_amount_survives_a_round_trip_as_a_decimal_string(tmp_path):
    """Same problem as memo_hash: 1 STRK is 1e18 base units, well past 2**53."""
    amount = 1_500_000_000_000_000_000

    async def run():
        async with stdio_client(
            _server_params(tmp_path / "store.json", spendable_notes=str(amount))
        ) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                opened = await session.call_tool("open_channel", {"counterparty": "0xseller"})
                handle = _structured(opened)["result"]["channel_handle"]

                written = await session.call_tool(
                    "propose_offer",
                    {
                        "channel_handle": handle,
                        "amount": str(amount),
                        "token": "0xtoken",
                        "deadline": 9999999999,
                        "memo_hash": 0,
                    },
                )
                assert _structured(written)["ok"] is True

                state = _structured(
                    await session.call_tool("read_channel_state", {"channel_handle": handle})
                )
                returned = state["result"]["offers"][0]["terms"]["amount"]

                assert isinstance(returned, str)
                assert int(returned) == amount

    asyncio.run(run())


def test_an_invalid_amount_is_refused_before_any_write(tmp_path):
    async def run():
        async with stdio_client(_server_params(tmp_path / "store.json")) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                opened = await session.call_tool("open_channel", {"counterparty": "0xseller"})
                handle = _structured(opened)["result"]["channel_handle"]

                for bad in ("0", "-5", "not-a-number", ""):
                    body = _structured(
                        await session.call_tool(
                            "propose_offer",
                            {
                                "channel_handle": handle,
                                "amount": bad,
                                "token": "0xtoken",
                                "deadline": 9999999999,
                                "memo_hash": 0,
                            },
                        )
                    )
                    assert body["ok"] is False, f"{bad!r} should be refused"
                    assert body["error"]["code"] == "INVALID_REQUEST"

                state = _structured(
                    await session.call_tool("read_channel_state", {"channel_handle": handle})
                )
                assert state["result"]["offers"] == []

    asyncio.run(run())


def test_every_result_names_its_backend_and_network(tmp_path):
    """9.2: a model must be able to tell mock from a live network from the transcript
    alone, on both success and failure results."""

    async def run():
        async with stdio_client(_server_params(tmp_path / "store.json")) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()

                ok = _structured(await session.call_tool("get_note_balance", {}))
                assert ok["backend"] == "mock"
                assert ok["network"] == "mock"

                failure = _structured(
                    await session.call_tool(
                        "accept_and_settle",
                        {"channel_handle": "ch_doesnotexist", "offer_id": "anything"},
                    )
                )
                assert failure["ok"] is False
                assert failure["backend"] == "mock"
                assert failure["network"] == "mock"

    asyncio.run(run())


def test_a_write_leaves_no_pending_intent_once_it_has_returned(tmp_path):
    """Durable caller intent (plan.md, Ishita task 1): the record `IntentStore.begin`
    persists before a chain-writing call must be gone once the call has returned to this
    process, success or a caught error, because that return is proof the process did not
    crash. A record surviving past the call would mean every ordinary write leaked one."""

    async def run():
        intents_dir = tmp_path / "intents"
        params = _server_params(
            tmp_path / "store.json", extra_env={"EREBUS_INTENT_STATE_DIR": str(intents_dir)}
        )
        async with stdio_client(params) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()

                opened = await session.call_tool("open_channel", {"counterparty": "0xseller"})
                assert _structured(opened)["ok"] is True
                handle = _structured(opened)["result"]["channel_handle"]

                # A real channel with no matching offer: unlike an unknown handle, this
                # reaches the seam's `accept_and_settle` itself, exercising the
                # begin/resolve pair around a call that raises rather than one that
                # never starts.
                failed = await session.call_tool(
                    "accept_and_settle",
                    {"channel_handle": handle, "offer_id": "offer_doesnotexist"},
                )
                assert _structured(failed)["ok"] is False

        pending_dir = intents_dir / "pending_operations"
        leftover = list(pending_dir.glob("*.json")) if pending_dir.is_dir() else []
        assert leftover == []

    asyncio.run(run())


def test_wait_for_offers_rejects_a_non_positive_expected_count(tmp_path):
    async def run():
        async with stdio_client(_server_params(tmp_path / "store.json")) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                opened = await session.call_tool("open_channel", {"counterparty": "0xseller"})
                handle = _structured(opened)["result"]["channel_handle"]

                for bad in (0, -1):
                    body = _structured(
                        await session.call_tool(
                            "wait_for_offers",
                            {"channel_handle": handle, "expected_count": bad},
                        )
                    )
                    assert body["ok"] is False
                    assert body["error"]["code"] == "INVALID_REQUEST"

    asyncio.run(run())


def test_wait_for_offers_rejects_a_non_positive_timeout(tmp_path):
    async def run():
        async with stdio_client(_server_params(tmp_path / "store.json")) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                opened = await session.call_tool("open_channel", {"counterparty": "0xseller"})
                handle = _structured(opened)["result"]["channel_handle"]

                body = _structured(
                    await session.call_tool(
                        "wait_for_offers",
                        {
                            "channel_handle": handle,
                            "expected_count": 1,
                            "timeout_seconds": 0,
                        },
                    )
                )
                assert body["ok"] is False
                assert body["error"]["code"] == "INVALID_REQUEST"

    asyncio.run(run())


def test_grant_viewing_key_refuses_to_overwrite_an_existing_export(tmp_path):
    """`_save_grant` publishes via `os.link`, which fails closed rather than clobbering a
    file that might be a different, earlier grant. Not covered by
    test_deal_grant_uses_a_private_file_and_never_returns_the_capsule, which only exercises
    the fresh-path case."""

    async def run():
        store = tmp_path / "store.json"
        output_path = tmp_path / "deal.grant.json"
        output_path.write_text('{"already": "here"}')
        async with stdio_client(_server_params(store, role="payee")) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                opened = await session.call_tool("open_channel", {"counterparty": "0xbuyer"})
                handle = _structured(opened)["result"]["channel_handle"]
                await session.call_tool(
                    "propose_offer",
                    {
                        "channel_handle": handle,
                        "amount": 100,
                        "token": "0xtoken",
                        "deadline": 9999999999,
                        "memo_hash": 0,
                    },
                )
                state = _structured(
                    await session.call_tool("read_channel_state", {"channel_handle": handle})
                )
                deal_id = state["result"]["offers"][0]["deal_id"]

                body = _structured(
                    await session.call_tool(
                        "grant_viewing_key",
                        {
                            "channel_handle": handle,
                            "deal_id": deal_id,
                            "grantee": "0xauditor",
                            "expires_at": int(time.time()) + 600,
                            "output_path": str(output_path),
                        },
                    )
                )
                assert body["ok"] is False
                assert body["error"]["code"] == "INVALID_REQUEST"
                assert json.loads(output_path.read_text()) == {"already": "here"}

    asyncio.run(run())


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-q"]))
