#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "erebus-mcp-server==0.2.0",
#     "openai-agents[litellm]==0.22.0",
# ]
#
# [tool.uv.sources]
# # erebus-mcp-server has no PyPI account (README "Install"): releases live on a static
# # PEP 503 index on GitHub Pages instead. This pin becomes installable when v0.2.0 is
# # published; release-candidate checks install locally built wheels into a clean venv.
# erebus-mcp-server = { index = "erebus" }
#
# [[tool.uv.index]]
# name = "erebus"
# url = "https://poulavbhowmick03.github.io/Erebus/simple"
# ///
"""OpenAI Agents SDK quickstart (roadmap 9.4 / plan.md task 7).

Two GPT-backed agents negotiate and settle over Erebus's MCP server, driven entirely
through the OpenAI Agents SDK's own MCP tool-calling loop rather than the deterministic
BuyerPolicy/SellerPolicy in erebus_agents. This is the point of the exercise: proving a
mainstream agent framework can drive the whole loop through the published MCP surface
with no Erebus-specific glue code beyond an MCP server launch and a system prompt.

Depends only on published packages (`erebus-mcp-server`, `openai-agents`), not this
checkout — `uv run --script quickstart.py` resolves both from PyPI on its own. Spawns the
installed `erebus-mcp-server` console script, the same one a `pip install
erebus-mcp-server` gives an outsider; it does not import anything from `agents/` or
`mcp-server/` in this repository.

Governing principle (CLAUDE.md constraint 6, extended in roadmap Phase 9): a model may
decide what to offer, but it must never be the thing that authorizes value movement. The
model here is free to negotiate — propose, counter, walk — but the role guard
(`accept_and_settle` refuses a payee) and any configured spending caps are enforced inside
the MCP server itself, not in this script or in the model's instructions. A prompt-injected
or simply overconfident model cannot talk its way past either; see
skills/erebus/evals/unsafe-behavior.md evals 2, 6, and 7 for adversarial cases against
exactly this boundary.

Requires OPENAI_API_KEY in the environment for the negotiation to actually run — this
script does not carry one. Run with --check-only to verify the MCP wiring (server starts,
tools resolve through the SDK's own MCP client) with a local scripted test model and no
provider request.

Protocol 4 operation IDs are not model inputs. This script removes them from the model's
tool schemas, persists each canonical write intent in the identity state directory, then
injects the stable ID at the MCP transport boundary. Reuse --run-id after a restart. A
different write is refused while an earlier write in that run remains unresolved.

Real by default, mock only on explicit opt-in: --buyer-env and --seller-env are required
unless you pass --mock. Each is an identity env file in the same KEY=VALUE format
scripts/agent.sh and scripts/erebus-request.py already use (the repo .env, or a
~/.erebus-*/env from scripts/new-identity.sh). Both identities must already be registered,
funded, approved, and holding spendable notes before this script runs — per D13
(docs/roadmap.md), funding is an explicit operator step done with `scripts/agent.sh fund`
beforehand, never something an agent tool does. This script only negotiates and settles;
it never shields or approves. --mock exists only for a no-cost wiring/behavior dry run and
is never the implicit default.

Model: --model selects which LLM drives both agents. Left unset, it uses the SDK's own
OpenAI default and needs OPENAI_API_KEY, same as before. A bare name ("gpt-4o") is passed
straight through as an OpenAI model. A "provider/model" name (litellm's convention, e.g.
"anthropic/claude-sonnet-4-20250514") is routed through the SDK's LiteLLM extension
instead, so this quickstart is not an OpenAI-only demo despite the framework's name — the
orchestration is OpenAI Agents SDK, the model is a separate choice. litellm reads that
provider's own credential env var itself (ANTHROPIC_API_KEY, GEMINI_API_KEY, ...); this
script never touches it.

Usage:
    # Real Sepolia run, two already-funded identities, default OpenAI model:
    OPENAI_API_KEY=sk-... uv run --script quickstart.py \\
        --buyer-env ~/.erebus-buyer/env --seller-env ~/.erebus-seller/env \\
        --run-id sepolia-demo-1

    # Same run, driven by Claude instead:
    ANTHROPIC_API_KEY=sk-ant-... uv run --script quickstart.py \\
        --buyer-env ~/.erebus-buyer/env --seller-env ~/.erebus-seller/env \\
        --run-id sepolia-demo-1 \\
        --model anthropic/claude-sonnet-4-20250514

    uv run --script quickstart.py --check-only          # no API key, no identities needed
    OPENAI_API_KEY=sk-... uv run --script quickstart.py --mock   # explicit no-cost dry run
"""

from __future__ import annotations

import argparse
import asyncio
import copy
import hashlib
import json
import os
import secrets
import shutil
import tempfile
from pathlib import Path
from typing import Any

from agents import Agent, Model, Runner
from agents.extensions.models.litellm_model import LitellmModel
from agents.mcp import MCPServerStdio
from agents.testing import ScriptedModel, assistant_message, function_call

BUYER_ADDRESS = "0xbuyer"
SELLER_ADDRESS = "0xseller"
TOKEN = "0xtoken"

# Every write tool's own docstring says the same thing: "expect 1-3 minutes of silence...
# the operation is not stuck below five minutes." The SDK's MCPServerStdio defaults its
# session timeout to 5 seconds, which is fine for reads but fails every real write before
# proving even starts. 360s gives margin past Erebus's own five-minute floor. Mock writes
# finish in well under a second, so this costs nothing on the --mock path either.
_MCP_SESSION_TIMEOUT_SECONDS = 360.0
_WRITE_TOOLS = {
    "open_channel",
    "propose_offer",
    "counter_offer",
    "accept_and_settle",
    "grant_viewing_key",
}
_EXPECTED_TOOLS = {
    "open_channel",
    "propose_offer",
    "counter_offer",
    "get_note_balance",
    "doctor",
    "read_channel_state",
    "accept_and_settle",
    "reconcile",
    "resume_operation",
    "rebuild_state",
    "wait_for_offers",
    "grant_viewing_key",
    "reveal",
}


class CallerIntentStore:
    """Durably bind one logical MCP write to its operation ID before transport."""

    def __init__(self, path: Path) -> None:
        self.path = path

    def prepare(self, scope: str, tool: str, arguments: dict[str, Any]) -> tuple[str, dict[str, Any]]:
        canonical = json.loads(json.dumps(arguments, sort_keys=True, separators=(",", ":")))
        fingerprint = hashlib.sha256(
            json.dumps(
                {"scope": scope, "tool": tool, "arguments": canonical},
                sort_keys=True,
                separators=(",", ":"),
            ).encode()
        ).hexdigest()
        data = self._read()
        existing = data["intents"].get(fingerprint)
        if existing is not None:
            if existing["tool"] != tool or existing["arguments"] != canonical:
                raise RuntimeError("caller-intent fingerprint collision")
            return fingerprint, existing

        unresolved = [
            record
            for record in data["intents"].values()
            if record["scope"] == scope and record["state"] == "prepared"
        ]
        if unresolved:
            operation_id = unresolved[0]["operation_id"]
            raise RuntimeError(
                f"{scope} has unresolved operation {operation_id}; call reconcile and resume "
                "that ID before creating a different write"
            )
        record = {
            "scope": scope,
            "operation_id": "op_" + secrets.token_hex(32),
            "tool": tool,
            "arguments": canonical,
            "state": "prepared",
        }
        data["intents"][fingerprint] = record
        self._write(data)
        return fingerprint, record

    def complete(self, fingerprint: str) -> None:
        data = self._read()
        data["intents"][fingerprint]["state"] = "completed"
        self._write(data)

    def _read(self) -> dict[str, Any]:
        if not self.path.exists():
            return {"version": 1, "intents": {}}
        data = json.loads(self.path.read_text())
        if data.get("version") != 1 or not isinstance(data.get("intents"), dict):
            raise RuntimeError(f"unsupported caller-intent store: {self.path}")
        return data

    def _write(self, data: dict[str, Any]) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        os.chmod(self.path.parent, 0o700)
        descriptor, temporary = tempfile.mkstemp(prefix=f".{self.path.name}.", dir=self.path.parent)
        try:
            os.chmod(temporary, 0o600)
            with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
                descriptor = -1
                json.dump(data, stream, sort_keys=True, separators=(",", ":"))
                stream.write("\n")
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(temporary, self.path)
        finally:
            if descriptor >= 0:
                os.close(descriptor)
            try:
                os.unlink(temporary)
            except FileNotFoundError:
                pass


class DurableMCPServer(MCPServerStdio):
    """Hide operation IDs from the model and inject a caller-persisted ID for writes."""

    def __init__(self, *args: Any, intent_store: CallerIntentStore, intent_scope: str, **kwargs: Any):
        super().__init__(*args, **kwargs)
        self.intent_store = intent_store
        self.intent_scope = intent_scope
        self._write_lock = asyncio.Lock()

    async def list_tools(self, run_context=None, agent=None):
        tools = await super().list_tools(run_context, agent)
        visible = []
        for tool in tools:
            if tool.name not in _WRITE_TOOLS:
                visible.append(tool)
                continue
            schema = copy.deepcopy(tool.input_schema)
            schema.get("properties", {}).pop("operation_id", None)
            schema["required"] = [
                name for name in schema.get("required", []) if name != "operation_id"
            ]
            visible.append(tool.model_copy(update={"input_schema": schema}))
        return visible

    async def call_tool(self, tool_name: str, arguments: dict[str, Any] | None, meta=None):
        if tool_name not in _WRITE_TOOLS:
            return await super().call_tool(tool_name, arguments, meta)
        async with self._write_lock:
            canonical = dict(arguments or {})
            if "operation_id" in canonical:
                raise RuntimeError("operation_id is owned by the durable caller, not the model")
            fingerprint, record = self.intent_store.prepare(self.intent_scope, tool_name, canonical)
            result = await super().call_tool(
                tool_name,
                {"operation_id": record["operation_id"], **canonical},
                meta,
            )
            payload = json.loads(result.content[0].text) if result.content else {}
            if payload.get("ok") is True:
                self.intent_store.complete(fingerprint)
            return result

BUYER_INSTRUCTIONS = """\
You are an autonomous buyer agent operating one Erebus MCP identity, configured as payer.
You are negotiating a single purchase over a private channel with one counterparty.

Rules:
- Your budget is {budget} base units of token {token}. Never propose or accept above it.
- Call get_note_balance before naming or accepting any price; you cannot pay more than
  your spendable note total regardless of budget.
- Use wait_for_offers to wait for the seller's replies instead of polling in a loop.
- If the seller's offer is at or below your budget, accept it with accept_and_settle.
- If not, counter at a price between your budget and the seller's ask, moving toward
  agreement each round.
- If no agreement is reached within {max_rounds} rounds, stop negotiating and say so —
  do not keep countering forever.
- You have already opened a channel; its handle is {channel_handle}. Do not open another.
- Take one turn now: read the channel state, then either counter, accept, or declare you
  are walking away. Do not call any tool more than the minimum needed for this turn.
"""

SELLER_INSTRUCTIONS = """\
You are an autonomous seller agent operating one Erebus MCP identity, configured as
payee. You are negotiating a single sale over a private channel with one counterparty.

Rules:
- Your reserve price is {reserve} base units of token {token}. Never counter below it.
- You must never call accept_and_settle — a payee calling it is not the deal; the server
  refuses it anyway, but do not attempt it. Your only move is to counter and wait.
- Use wait_for_offers to wait for the buyer's replies instead of polling in a loop.
- Counter toward the buyer's offer each round, but never below your reserve.
- If no agreement is reached within {max_rounds} rounds, stop negotiating and say so.
- You have already opened a channel; its handle is {channel_handle}. Do not open another.
- Take one turn now: read the channel state, then either counter or declare you are
  walking away. Do not call any tool more than the minimum needed for this turn.
"""


def _server_params(store_path: Path, role: str, identity: str, spendable_notes: str = "") -> dict[str, Any]:
    """Same shape as erebus_agents.mcp_loop.server_params, but launching the *installed*
    erebus-mcp-server console script rather than `uv run python server.py` from a
    checkout — this is what an outsider who only ran `pip install erebus-mcp-server`
    actually has on PATH."""
    return {
        "command": "erebus-mcp-server",
        "args": [],
        "env": {
            "AGENT_ADDRESS": identity,
            "PROVING_SERVICE_URL": "http://unused.invalid",
            "EREBUS_MOCK_STORE_PATH": str(store_path),
            "EREBUS_MOCK_LATENCY_SECONDS": "0",
            "EREBUS_MOCK_SPENDABLE_NOTES": spendable_notes,
            "EREBUS_SETTLEMENT_ROLE": role,
        },
    }


_SEAM_REQUIRED_KEYS = (
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


def _load_identity_env(path: Path) -> dict[str, str]:
    """Parses a KEY=VALUE identity env file — the same format and the same required keys
    as scripts/erebus-request.py, so a file that already works with scripts/agent.sh works
    here unchanged. Key values (POOL_KEY_FILE, ACCOUNT_KEY_FILE) are file paths in this
    format; the values themselves never cross this parser."""
    values: dict[str, str] = {}
    for raw_line in path.expanduser().read_text().splitlines():
        line = raw_line.strip()
        if line and not line.startswith("#") and "=" in line:
            key, value = line.split("=", 1)
            values[key] = value
    missing = [key for key in _SEAM_REQUIRED_KEYS if not values.get(key)]
    if missing:
        raise SystemExit(f"{path}: missing required values: {', '.join(missing)}")
    return values


def _seam_server_params(env_values: dict[str, str], role: str) -> dict[str, Any]:
    """Real-backend server params: the identity's own env file plus EREBUS_BACKEND=seam
    and the settlement role, nothing fabricated or defaulted. The identity must already be
    registered, funded, and approved — this script only negotiates and settles.

    Also fills EREBUS_CLI when the identity env file doesn't set it: the published
    erebus-mcp-server release this script installs requires that variable explicitly
    rather than discovering the binary on PATH, even though `uv run --script` installs
    erebus-cli into this same environment right alongside it. shutil.which resolves it
    from here since this process shares that PATH; the subprocess would not find it
    itself without being told."""
    env = {**env_values, "EREBUS_BACKEND": "seam", "EREBUS_SETTLEMENT_ROLE": role}
    if not env.get("EREBUS_SPENDING_STATE_PATH"):
        state_dir = Path(env["EREBUS_STATE_DIR"])
        legacy = state_dir / "spending.json"
        nested = state_dir / "mcp" / "spending.json"
        _migrate_auxiliary_file(legacy, nested)
        _migrate_auxiliary_file(legacy.with_name("spending.json.lock"), nested.with_name("spending.json.lock"))
        env["EREBUS_SPENDING_STATE_PATH"] = str(nested)
    if not env.get("EREBUS_CLI"):
        found = shutil.which("erebus-cli")
        if found is None:
            raise SystemExit(
                "erebus-cli not found on PATH — this should have been installed alongside "
                "erebus-mcp-server by `uv run --script`; try deleting the cached "
                "environment and re-running, or set EREBUS_CLI in your identity env file."
            )
        env["EREBUS_CLI"] = found
    return {"command": "erebus-mcp-server", "args": [], "env": env}


def _resolve_model(name: str | None) -> str | Model | None:
    """None keeps the SDK's own OpenAI default (needs OPENAI_API_KEY), unchanged from
    before --model existed. A "provider/model" name is litellm's convention and is routed
    through LitellmModel, which reads that provider's own credential env var itself — this
    function never touches API keys. A bare name with no "/" is passed straight through as
    an OpenAI model string, the same as the SDK's own default path."""
    if name is None:
        return None
    if "/" in name:
        return LitellmModel(model=name)
    return name


def _intent_path(params: dict[str, Any], identity: str) -> Path:
    env = params["env"]
    configured = env.get("EREBUS_CALLER_INTENT_PATH")
    if configured:
        return Path(configured)
    state_dir = env.get("EREBUS_STATE_DIR")
    if state_dir:
        root = Path(state_dir)
        legacy = root / "openai-agents-caller-intents.json"
        nested = root / "caller_intents" / "openai-agents.json"
        _migrate_auxiliary_file(legacy, nested)
        return nested
    mock_store = env.get("EREBUS_MOCK_STORE_PATH")
    if mock_store:
        safe_identity = identity.replace("/", "_")
        return Path(mock_store).parent / f"openai-agents-caller-intents-{safe_identity}.json"
    raise RuntimeError("set EREBUS_CALLER_INTENT_PATH or EREBUS_STATE_DIR for durable writes")


def _migrate_auxiliary_file(legacy: Path, nested: Path) -> None:
    """Keep non-channel JSON below a subdirectory that Rust will not scan as channels."""
    if not legacy.exists():
        return
    nested.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    os.chmod(nested.parent, 0o700)
    if nested.exists():
        if legacy.read_bytes() != nested.read_bytes():
            raise RuntimeError(f"both legacy and nested state exist: {legacy} and {nested}")
        legacy.unlink()
        return
    os.replace(legacy, nested)
    os.chmod(nested, 0o600)


async def _call(server: MCPServerStdio, tool: str, **kwargs: Any) -> dict[str, Any]:
    result = await server.call_tool(tool, kwargs)
    payload = json.loads(result.content[0].text) if result.content else {}
    if not payload.get("ok"):
        raise RuntimeError(f"{tool} failed: {payload.get('error')}")
    return payload["result"]


async def _run_negotiation(
    budget: int,
    reserve: int,
    max_rounds: int,
    buyer_env_path: Path | None,
    seller_env_path: Path | None,
    mock: bool,
    model_name: str | None,
    run_id: str,
    allow_mainnet: bool,
) -> None:
    model = _resolve_model(model_name)
    with tempfile.TemporaryDirectory() as tmp:
        if mock:
            buyer_address, seller_address = BUYER_ADDRESS, SELLER_ADDRESS
            token = TOKEN
            store_path = Path(tmp) / "erebus-mock-store.json"
            buyer_params = _server_params(store_path, "payer", buyer_address, spendable_notes=str(budget))
            seller_params = _server_params(store_path, "payee", seller_address)
            print("--mock: no chain, no keys, no real value moved")
        else:
            assert buyer_env_path is not None and seller_env_path is not None  # enforced in main()
            buyer_env = _load_identity_env(buyer_env_path)
            seller_env = _load_identity_env(seller_env_path)
            buyer_address = buyer_env["AGENT_ADDRESS"]
            seller_address = seller_env["AGENT_ADDRESS"]
            for key in ("STARKNET_CHAIN_ID", "POOL_ADDRESS", "TOKEN_ADDRESS"):
                if buyer_env[key] != seller_env[key]:
                    raise SystemExit(f"buyer and seller identity files disagree on {key}")
            chain_id = buyer_env["STARKNET_CHAIN_ID"]
            if chain_id.lower() in {"sn_main", "0x534e5f4d41494e"} and not allow_mainnet:
                raise SystemExit("mainnet run refused; pass --allow-mainnet only after explicit approval")
            token = buyer_env["TOKEN_ADDRESS"]
            buyer_params = _seam_server_params(buyer_env, "payer")
            seller_params = _seam_server_params(seller_env, "payee")
            print(
                f"real (seam) run: chain={chain_id} buyer={buyer_address} "
                f"seller={seller_address}"
            )

        buyer_store = CallerIntentStore(_intent_path(buyer_params, buyer_address))
        seller_store = CallerIntentStore(_intent_path(seller_params, seller_address))
        async with DurableMCPServer(
            params=buyer_params,
            name="erebus-buyer",
            client_session_timeout_seconds=_MCP_SESSION_TIMEOUT_SECONDS,
            intent_store=buyer_store,
            intent_scope=f"{run_id}:buyer",
        ) as buyer_mcp:
            async with DurableMCPServer(
                params=seller_params,
                name="erebus-seller",
                client_session_timeout_seconds=_MCP_SESSION_TIMEOUT_SECONDS,
                intent_store=seller_store,
                intent_scope=f"{run_id}:seller",
            ) as seller_mcp:
                buyer_handle = (await _call(buyer_mcp, "open_channel", counterparty=seller_address))[
                    "channel_handle"
                ]
                seller_handle = (await _call(seller_mcp, "open_channel", counterparty=buyer_address))[
                    "channel_handle"
                ]
                print(f"channel opened: buyer={buyer_handle} seller={seller_handle}")

                buyer_agent = Agent(
                    name="Erebus buyer",
                    instructions=BUYER_INSTRUCTIONS.format(
                        budget=budget, token=token, max_rounds=max_rounds, channel_handle=buyer_handle
                    ),
                    mcp_servers=[buyer_mcp],
                    model=model,
                )
                seller_agent = Agent(
                    name="Erebus seller",
                    instructions=SELLER_INSTRUCTIONS.format(
                        reserve=reserve, token=token, max_rounds=max_rounds, channel_handle=seller_handle
                    ),
                    mcp_servers=[seller_mcp],
                    model=model,
                )

                for round_index in range(max_rounds + 1):
                    buyer_result = await Runner.run(buyer_agent, f"Round {round_index}: take your turn.")
                    print(f"[buyer  round {round_index}] {buyer_result.final_output}")

                    state = await _call(buyer_mcp, "read_channel_state", channel_handle=buyer_handle)
                    if state["settlements"]:
                        print(f"settled: {json.dumps(state['settlements'][-1], indent=2)}")
                        return

                    seller_result = await Runner.run(seller_agent, f"Round {round_index}: take your turn.")
                    print(f"[seller round {round_index}] {seller_result.final_output}")

                print("no settlement reached within max_rounds")


async def _check_only() -> None:
    """Verifies the MCP wiring without calling a model: starts the installed
    erebus-mcp-server console script exactly as the negotiation path does, and confirms
    the SDK's own MCP client resolves the expected tool set. No OPENAI_API_KEY needed."""
    with tempfile.TemporaryDirectory() as tmp:
        store_path = Path(tmp) / "erebus-mock-store.json"
        params = _server_params(store_path, "payer", BUYER_ADDRESS, spendable_notes="1000")
        intent_path = Path(tmp) / "caller-intents.json"
        intents = CallerIntentStore(intent_path)
        async with DurableMCPServer(
            params=params,
            name="erebus-check",
            intent_store=intents,
            intent_scope="check-only:payer",
        ) as server:
            tools = await server.list_tools()
            names = sorted(t.name for t in tools)
            print(f"connected to erebus-mcp-server; {len(names)} tools resolved:")
            for name in names:
                print(f"  - {name}")
            if set(names) != _EXPECTED_TOOLS:
                raise SystemExit(
                    "MCP tool surface mismatch: "
                    f"missing={sorted(_EXPECTED_TOOLS - set(names))} "
                    f"extra={sorted(set(names) - _EXPECTED_TOOLS)}"
                )
            for tool in tools:
                if tool.name in _WRITE_TOOLS and "operation_id" in tool.input_schema.get(
                    "properties", {}
                ):
                    raise SystemExit(f"{tool.name}: operation_id leaked into the model schema")

            first = await _call(server, "open_channel", counterparty=SELLER_ADDRESS)
            second = await _call(server, "open_channel", counterparty=SELLER_ADDRESS)
            if first != second:
                raise SystemExit("same durable channel intent did not replay the same result")
            records = json.loads(intent_path.read_text())["intents"]
            if len(records) != 1 or next(iter(records.values()))["state"] != "completed":
                raise SystemExit("caller intent was not durably completed")

            scripted = ScriptedModel(
                [
                    [function_call("doctor", {}, call_id="call-doctor")],
                    [assistant_message("mock MCP tool call complete")],
                ]
            )
            agent = Agent(
                name="Erebus wiring check",
                instructions="Call doctor once, then stop.",
                mcp_servers=[server],
                model=scripted,
            )
            result = await Runner.run(agent, "Check the configured Erebus identity.")
            scripted.assert_complete()
            if result.final_output != "mock MCP tool call complete":
                raise SystemExit("the OpenAI Agents SDK did not complete its MCP tool loop")
            print("check-only: Protocol 4 durable replay and Agents SDK mock tool loop OK")


def _parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--budget", type=int, default=1000, help="buyer budget in token base units")
    parser.add_argument("--reserve", type=int, default=800, help="seller reserve in token base units")
    parser.add_argument("--rounds", type=int, default=3)
    parser.add_argument("--buyer-env", type=Path, help="buyer identity env file (required unless --mock)")
    parser.add_argument("--seller-env", type=Path, help="seller identity env file (required unless --mock)")
    parser.add_argument(
        "--run-id",
        help="stable identifier for this negotiation; reuse it after a restart",
    )
    parser.add_argument(
        "--allow-mainnet",
        action="store_true",
        help="explicitly allow SN_MAIN identity files; never implied by real mode",
    )
    parser.add_argument(
        "--model",
        help="model for both agents. Unset uses the SDK's OpenAI default (OPENAI_API_KEY). "
        "A bare name (gpt-4o) is an OpenAI model. A provider/model name (litellm's "
        "convention, e.g. anthropic/claude-sonnet-4-20250514) routes through LiteLLM and "
        "reads that provider's own credential env var — not OpenAI-only despite the SDK's "
        "name.",
    )
    parser.add_argument(
        "--mock",
        action="store_true",
        help="explicit opt-in to a no-cost dry run against EREBUS_BACKEND=mock instead of "
        "a real identity — mock is never the default",
    )
    parser.add_argument(
        "--check-only",
        action="store_true",
        help="verify MCP wiring with a local scripted model, no provider API key required",
    )
    args = parser.parse_args()

    if args.check_only:
        return args
    if args.mock and (args.buyer_env or args.seller_env):
        parser.error("--mock and --buyer-env/--seller-env are mutually exclusive")
    if args.mock and args.allow_mainnet:
        parser.error("--mock and --allow-mainnet are mutually exclusive")
    if not args.mock and not (args.buyer_env and args.seller_env):
        parser.error(
            "real identities are required by default: pass --buyer-env and --seller-env "
            "(scripts/agent.sh-style identity env files, already registered and funded), "
            "or pass --mock for a no-cost dry run against the mock backend"
        )
    if not args.mock and not args.run_id:
        parser.error("--run-id is required for a real run and must be reused after a restart")
    if args.run_id and (
        len(args.run_id) > 100
        or not all(character.isalnum() or character in "-_." for character in args.run_id)
    ):
        parser.error("--run-id must be at most 100 letters, numbers, dots, dashes, or underscores")
    return args


def main() -> None:
    args = _parse_args()
    if args.check_only:
        asyncio.run(_check_only())
    else:
        run_id = args.run_id or "mock-" + secrets.token_hex(8)
        asyncio.run(
            _run_negotiation(
                args.budget,
                args.reserve,
                args.rounds,
                args.buyer_env,
                args.seller_env,
                args.mock,
                args.model,
                run_id,
                args.allow_mainnet,
            )
        )


if __name__ == "__main__":
    main()
