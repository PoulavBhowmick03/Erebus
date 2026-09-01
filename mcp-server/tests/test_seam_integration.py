"""MCP tool call through the Python seam into the real ``erebus-cli``.

Every other test in this repository stops short of this. ``test_tools.py`` drives the mock
backend, ``test_seam_client.py`` drives a stub holding payloads captured from a live run on
2026-07-31, and ``sdk/py/tests`` mocks the subprocess or reaches the binary only through
argument-parse failures. So nothing has been checking that the Rust binary still answers in
the shape the Python layers expect, and the captured payloads cannot notice when it stops:
they are frozen bytes from before wire v2 ran live, before change notes, and before amounts
and memo hashes became strings.

These tests need no chain, no prover, no keys with value, and no funds. ``doctor`` is what
makes that possible. It is read-only and reports faults instead of raising, so pointing it
at an unreachable RPC exercises the whole path -- tool registration, config marshalling,
key-file paths, the one-envelope protocol, and report translation -- and returns in
milliseconds.

What is deliberately not here: anything that writes. Those need a funded identity and a
proving service, so they stay in the runbook as a manual pass.
"""

from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path

import pytest
from mcp import ClientSession
from mcp.client.stdio import StdioServerParameters, stdio_client

REPO_ROOT = Path(__file__).resolve().parents[2]
SERVER_PATH = REPO_ROOT / "mcp-server" / "src" / "server.py"
CLI = REPO_ROOT / "sdk" / "rs" / "target" / "debug" / "erebus-cli"

pytestmark = pytest.mark.skipif(
    not CLI.exists(),
    reason="erebus-cli not built; run `cargo build --bin erebus-cli` in sdk/rs",
)

#: Reserved as "no listener" by RFC 6335, so the RPC and prover legs fail immediately
#: instead of waiting out a timeout.
UNREACHABLE = "http://127.0.0.1:1"


def _seam_params(tmp_path: Path, role: str = "payer") -> StdioServerParameters:
    """Configures the server against the real binary and a deliberately dead endpoint."""
    pool_key = tmp_path / "pool.key"
    pool_key.write_text("0x1234567890abcdef\n")
    pool_key.chmod(0o600)
    account_key = tmp_path / "account.key"
    account_key.write_text("0xfedcba0987654321\n")
    account_key.chmod(0o600)
    state = tmp_path / "state"
    state.mkdir(mode=0o700)

    return StdioServerParameters(
        command="uv",
        args=["run", "python", str(SERVER_PATH)],
        cwd=str(REPO_ROOT),
        env={
            **os.environ,
            "EREBUS_BACKEND": "seam",
            "EREBUS_NETWORK": "sepolia",
            "EREBUS_CLI": str(CLI),
            "AGENT_ADDRESS": "0xa11ce",
            "PROVING_SERVICE_URL": UNREACHABLE,
            "STARKNET_RPC_URL": UNREACHABLE,
            "TOKEN_ADDRESS": "0x7042",
            "POOL_KEY_FILE": str(pool_key),
            "ACCOUNT_KEY_FILE": str(account_key),
            "EREBUS_STATE_DIR": str(state),
            "EREBUS_SETTLEMENT_ROLE": role,
        },
    )


def _structured(result) -> dict:
    if result.structured_content is not None:
        return result.structured_content
    return json.loads(result.content[0].text)


def test_doctor_reaches_the_rust_binary_and_its_report_survives_both_layers(tmp_path):
    """The end-to-end path, with everything downstream deliberately broken.

    A fault report is a successful call. The binary distinguishes "I looked and found
    problems" from "I could not look", and that distinction has to survive the seam and the
    MCP envelope or an operator cannot tell a broken pool from a broken client.
    """

    async def run():
        async with stdio_client(_seam_params(tmp_path)) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                body = _structured(await session.call_tool("doctor", {}))

                assert body["ok"] is True, f"a fault report is still a successful call: {body}"
                assert body["backend"] == "seam"
                assert body["network"] == "sepolia"
                result = body["result"]
                assert result["ready"] is False, "an unreachable RPC cannot be ready"

                checks = {c["name"]: c for c in result["checks"]}
                # Named individually rather than by count: a check disappearing is exactly
                # the drift this test exists to catch, and a count assertion would pass if
                # one were swapped for another.
                for name in ("pool_key_file", "account_key_file", "state_dir", "rpc"):
                    assert name in checks, f"{name} missing from {sorted(checks)}"

                assert checks["rpc"]["status"] == "fail"
                assert result["repairs"], "an unhealthy report must say what to do"

    asyncio.run(run())


def test_the_seam_passes_key_paths_and_never_key_contents(tmp_path):
    """Custody boundary, checked against the real binary rather than a stub.

    Python holds the paths and Rust opens the files. A report that echoed key material back
    would put secrets in every MCP transcript, and MCP transcripts are the one place an
    agent's whole session is written down.
    """

    async def run():
        async with stdio_client(_seam_params(tmp_path)) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                rendered = json.dumps(_structured(await session.call_tool("doctor", {})))

                assert "1234567890abcdef" not in rendered, "pool key material crossed the seam"
                assert "fedcba0987654321" not in rendered, "account key material crossed the seam"

    asyncio.run(run())


def test_a_failure_downstream_of_the_binary_arrives_as_structure(tmp_path):
    """An unreachable RPC has to surface as a typed error, not a crash or a prose string.

    `open_channel` needs the chain, so this is the shape an agent sees when the network is
    down: `ok:false` with a code it can branch on, and `is_error` false because the MCP call
    itself succeeded.
    """

    async def run():
        async with stdio_client(_seam_params(tmp_path)) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                result = await session.call_tool(
                    "open_channel",
                    {
                        "operation_id": "op_" + "ab" * 32,
                        "counterparty": "0x" + "b0" * 16,
                    },
                )
                body = _structured(result)

                assert body["ok"] is False
                assert isinstance(body["error"]["code"], str)
                assert isinstance(body["error"]["retryable"], bool)
                assert result.is_error is False, (
                    "the tool call succeeded; the protocol operation is what failed"
                )

    asyncio.run(run())


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-q"]))
