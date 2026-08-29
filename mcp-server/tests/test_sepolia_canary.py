"""Opt-in canary against real Sepolia: the gap `test_seam_integration.py` names explicitly
and leaves open ("Those need a funded identity and a proving service, so they stay in the
runbook as a manual pass").

Every other test in this repository either drives the mock backend or points the real
`erebus-cli` at a dead RPC endpoint (`UNREACHABLE = "http://127.0.0.1:1"` in
`test_seam_integration.py`). Both are correct for what they check, but neither can catch a
regression that only shows up against a live chain: a real RPC dropping `proof_facts` (see
`docs/local-prover.md`), a pool ABI drifting out from under a pinned prover tag (roadmap Q4),
or a genuinely unreachable prover that a dead-endpoint test can't distinguish from a
deliberately-dead one.

**Opt-in and read-only, on purpose.** This never calls a write tool. `doctor` and
`get_note_balance` are enough to prove the whole live pipeline -- RPC, prover reachability,
registration, allowance, balance -- actually answers, without spending a fee or a proof on
every CI run. A write canary belongs in the runbook's manual pass, not here, until there's a
throwaway identity budgeted for CI burning fees on every push.

Skipped by default: set every variable below (a funded Sepolia identity's env file, same
shape as `scripts/agent.sh` and `agents/examples/openai-agents-quickstart/quickstart.py`
already use) to opt in.
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

_REQUIRED_ENV = (
    "STARKNET_RPC_URL",
    "PROVING_SERVICE_URL",
    "POOL_ADDRESS",
    "STARKNET_CHAIN_ID",
    "AGENT_ADDRESS",
    "POOL_KEY_FILE",
    "ACCOUNT_KEY_FILE",
    "EREBUS_STATE_DIR",
    "TOKEN_ADDRESS",
)

_missing = [key for key in _REQUIRED_ENV if not os.environ.get(key)]
_reasons = []
if _missing:
    _reasons.append(f"missing {', '.join(_missing)}")
if not CLI.exists():
    _reasons.append("erebus-cli not built (cargo build --bin erebus-cli in sdk/rs)")

pytestmark = pytest.mark.skipif(
    bool(_reasons),
    reason="opt-in Sepolia canary, not run by default: " + "; ".join(_reasons) if _reasons else "",
)


def _params() -> StdioServerParameters:
    env = {
        **os.environ,
        "EREBUS_BACKEND": "seam",
        "EREBUS_SETTLEMENT_ROLE": os.environ.get("EREBUS_SETTLEMENT_ROLE", "payer"),
    }
    env.setdefault("EREBUS_CLI", str(CLI))
    return StdioServerParameters(command="uv", args=["run", "python", str(SERVER_PATH)], cwd=str(REPO_ROOT), env=env)


def _structured(result) -> dict:
    if result.structured_content is not None:
        return result.structured_content
    return json.loads(result.content[0].text)


def test_doctor_reports_ready_against_live_sepolia() -> None:
    """The identity named by the env vars above must actually be usable: registered,
    allowance covering the live fee, RPC and prover both reachable. Anything less and a
    write attempt through this same identity would fail later, more expensively."""

    async def run():
        async with stdio_client(_params()) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                body = _structured(await session.call_tool("doctor", {}))
                assert body["ok"] is True, body
                result = body["result"]
                failing = [c for c in result["checks"] if c["status"] != "pass"]
                assert result["ready"] is True, (
                    f"identity not ready against live Sepolia: {failing}; repairs: {result['repairs']}"
                )

    asyncio.run(run())


def test_note_balance_reads_from_the_real_chain() -> None:
    """A read-only proof that discovery actually resolves this identity's notes over the
    live RPC, not just that the tool call round-trips against a mock or a dead endpoint."""

    async def run():
        async with stdio_client(_params()) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                body = _structured(await session.call_tool("get_note_balance", {}))
                assert body["ok"] is True, body
                assert isinstance(body["result"]["spendable"], list)

    asyncio.run(run())


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-q"]))
