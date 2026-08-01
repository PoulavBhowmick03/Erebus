"""I1.3's acceptance criterion, verified literally: "Verify from a real MCP client, not
just your own agents." Spawns `server.py` as a real subprocess over stdio and drives it
with the official `mcp` SDK's client — independent of anything in `agents/`.
"""

from __future__ import annotations

import asyncio
import json
from pathlib import Path

import pytest
from mcp import ClientSession
from mcp.client.stdio import StdioServerParameters, stdio_client

REPO_ROOT = Path(__file__).resolve().parents[2]
SERVER_PATH = REPO_ROOT / "mcp-server" / "src" / "server.py"


def _server_params(store_path: Path) -> StdioServerParameters:
    return StdioServerParameters(
        command="uv",
        args=["run", "python", str(SERVER_PATH)],
        cwd=str(REPO_ROOT),
        env={
            "AGENT_ADDRESS": "0xbuyer",
            "PROVING_SERVICE_URL": "http://unused.invalid",
            "EREBUS_MOCK_STORE_PATH": str(store_path),
            "EREBUS_MOCK_LATENCY_SECONDS": "0",
        },
    )


def _structured(result) -> dict:
    if result.structured_content is not None:
        return result.structured_content
    return json.loads(result.content[0].text)


def test_all_seven_tools_are_listed_with_descriptions(tmp_path):
    async def run():
        async with stdio_client(_server_params(tmp_path / "store.json")) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                listed = await session.list_tools()
                names = {t.name for t in listed.tools}
                assert names == {
                    "open_channel",
                    "propose_offer",
                    "counter_offer",
                    "read_channel_state",
                    "accept_and_settle",
                    "grant_viewing_key",
                    "reveal",
                }
                assert all(t.description for t in listed.tools)

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

    asyncio.run(run())


def test_a_settlement_error_comes_back_as_parseable_structured_json(tmp_path):
    """I1.3: 'Tool errors must carry the SettlementErrorCode through — a failure that
    arrives as an opaque string makes the whole failure-handling path untestable.'"""

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
                # And critically: this did NOT come back as an MCP-protocol-level error —
                # the call succeeded, the payload carries the failure.
                assert result.is_error is False

    asyncio.run(run())


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-q"]))
