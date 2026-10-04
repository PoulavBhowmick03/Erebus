"""Two agents drive native negotiation and settlement, each through its own Metropolis MCP server.

Prices, keys, and settlement mode are fixed by each operator's files, so an agent supplies only
the shared operation ID and sequences the calls. The one rule an agent must hold itself: after a
settlement attempt, never call `settle_deal` again. Uncertainty is resolved with `recover_deal`,
which observes and cannot submit. A pending result is not a reason to pay twice. When the buyer's
server also exposes access, failed delivery is retried by retrieving again, never by paying.

Timings use the local monotonic clock from the start of the settlement call to first observed
inclusion, finalized verification, and delivery. Chain timestamps are diagnostics, not latency.
`stages_ms` marks a stage null when the mode reports no such stage (public-bound has no proof)
or when the driver did not report it.

The operator-selected `x402-exact` profile is separate and has no fallback: the buyer's server
exposes only `negotiate_deal` and `retrieve_service_access`. The first retrieval signs one local
Permit2 authorization and the seller submits it; every retry is another retrieval that reuses the
retained permit. Nothing on that path reports independent payment verification: a verified
resource hash and the seller's finality claim are not chain evidence. An independent auditor
verifies payment from the disclosure grant.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import sys
import time
from contextlib import AsyncExitStack
from pathlib import Path
from typing import Any, Awaitable, Callable, Protocol

from mcp import ClientSession
from mcp.client.stdio import StdioServerParameters, stdio_client

BUYER_TOOLS = {"negotiate_deal", "check_settlement_funding", "settle_deal", "recover_deal"}
SELLER_TOOLS = {"negotiate_deal"}
ACCESS_TOOL = "retrieve_service_access"
X402_BUYER_TOOLS = {"negotiate_deal", ACCESS_TOOL}


class HarnessError(RuntimeError):
    """The loop stopped without verified payment; the operation ID is still the recovery handle."""


class ToolSession(Protocol):
    async def call_tool(self, name: str, arguments: dict[str, Any]) -> Any: ...


def _structured(result: Any) -> dict[str, Any]:
    if getattr(result, "structured_content", None) is not None:
        return result.structured_content
    if isinstance(result, dict):
        return result
    return json.loads(result.content[0].text)


async def _call(session: ToolSession, name: str, operation_ref: str) -> dict[str, Any]:
    return _structured(await session.call_tool(name, {"operation_ref": operation_ref}))


async def drive_deal(buyer: ToolSession, seller: ToolSession, operation_ref: str, *,
                     poll_seconds: float = 1.0, max_polls: int = 120, deliver: bool = False,
                     sleep: Callable[[float], Awaitable[None]] = asyncio.sleep,
                     clock: Callable[[], float] = time.monotonic) -> dict[str, Any]:
    """Negotiate on both sides at once, settle at most once, observe until finalized, then deliver."""
    buyer_deal, seller_deal = await asyncio.gather(_call(buyer, "negotiate_deal", operation_ref),
                                                   _call(seller, "negotiate_deal", operation_ref))
    if not (buyer_deal.get("ok") and seller_deal.get("ok")):
        raise HarnessError("negotiation did not authorize on both sides")
    commitment = buyer_deal["result"]["deal_commitment"]
    if seller_deal["result"]["deal_commitment"] != commitment:
        raise HarnessError("participants authorized different deals")
    record: dict[str, Any] = {
        "operation_ref": operation_ref, "deal_commitment": commitment,
        "negotiation_ms": {"buyer": buyer_deal["result"]["measurements_ms"]["negotiation"],
                           "seller": seller_deal["result"]["measurements_ms"]["negotiation"]},
        "settle_calls": 0, "recover_calls": 0,
    }
    funding = await _call(buyer, "check_settlement_funding", operation_ref)
    if not funding.get("ok"):
        raise HarnessError("buyer is not funded for this deal")

    submitted = clock()
    settled = await _call(buyer, "settle_deal", operation_ref)
    record["settle_calls"] = 1
    record["settlement"] = settled.get("result")
    outcome = settled.get("result") or {}
    first_inclusion: float | None = None

    def note_inclusion(candidate: dict[str, Any]) -> None:
        nonlocal first_inclusion
        if first_inclusion is None and (candidate.get("chain_times") or {}).get("inclusion_block") is not None:
            first_inclusion = round(clock() - submitted, 3)

    note_inclusion(outcome)
    while not outcome.get("payment_verified"):
        if outcome.get("status") == "closed_unpaid":
            raise HarnessError("deal closed unpaid")
        if record["recover_calls"] >= max_polls:
            raise HarnessError("payment not finalized within the polling budget; recover later by observation")
        await sleep(poll_seconds)
        observed = await _call(buyer, "recover_deal", operation_ref)
        record["recover_calls"] += 1
        # A failed observation call is retried by observation too, never by settling again.
        outcome = observed.get("result") or {}
        note_inclusion(outcome)
    verified_after = round(clock() - submitted, 3)
    record.update(payment_verified=True, stage=outcome.get("stage"),
                  winning_commitment=outcome.get("winning_commitment"),
                  measurements_ms=(record["settlement"] or {}).get("measurements_ms", {}),
                  latency_s={"first_inclusion_after": first_inclusion,
                             "payment_verified_after": verified_after})
    record["chain_times"] = outcome.get("chain_times", {})
    if record["winning_commitment"] != commitment:
        raise HarnessError("finalized payment does not match the negotiated deal")
    if deliver:
        record["delivery"] = await _deliver(buyer, Path(buyer_deal["result"]["evidence_file"]).name,
                                            poll_seconds=poll_seconds, max_polls=max_polls, sleep=sleep, clock=clock)
        record["latency_s"]["delivery"] = record["delivery"].pop("seconds")
    measurements = record["measurements_ms"]
    record["stages_ms"] = {
        "negotiation_buyer": record["negotiation_ms"]["buyer"],
        "negotiation_seller": record["negotiation_ms"]["seller"],
        "proof": measurements.get("proof_preparation"),
        "signing": measurements.get("local_signing"),
        "submission": measurements.get("submission"),
        "first_inclusion_after": None if first_inclusion is None else round(first_inclusion * 1000, 3),
        "finalized_verification_after": round(verified_after * 1000, 3),
        "delivery": None if "delivery" not in record["latency_s"] else round(record["latency_s"]["delivery"] * 1000, 3),
    }
    record["stages_note"] = ("null means the mode reports no proving stage (public-bound) or the "
                             "driver did not report that stage; block timestamps are diagnostics only")
    return record


async def _deliver(buyer: ToolSession, evidence_name: str, *, poll_seconds: float, max_polls: int,
                   sleep: Callable[[float], Awaitable[None]], clock: Callable[[], float]) -> dict[str, Any]:
    """Retrieve the paid resource. A pending or failed retrieval is retried by retrieval only."""
    started = clock()
    for attempt in range(1, max_polls + 1):
        reply = _structured(await buyer.call_tool(ACCESS_TOOL, {"evidence_name": evidence_name}))
        if reply.get("ok"):
            receipt = reply["result"]["result"]
            if not receipt.get("resource_verified"):
                raise HarnessError("retrieved resource failed verification")
            return {"attempts": attempt, "resource_sha256": receipt["resource_sha256"],
                    "resource_bytes": receipt["resource_bytes"], "seconds": round(clock() - started, 3)}
        await sleep(poll_seconds)
    raise HarnessError("resource not delivered within the polling budget; retrieve again later, do not pay again")


async def drive_x402_deal(buyer: ToolSession, seller: ToolSession, operation_ref: str, *,
                          poll_seconds: float = 1.0, max_polls: int = 120,
                          sleep: Callable[[float], Awaitable[None]] = asyncio.sleep,
                          clock: Callable[[], float] = time.monotonic) -> dict[str, Any]:
    """Negotiate on both sides, then retrieve; the first retrieval is the payment request.

    Retries never call anything but retrieval, so a dropped response or a seller restart can
    only reuse the retained permit.
    """
    started = clock()
    buyer_deal, seller_deal = await asyncio.gather(_call(buyer, "negotiate_deal", operation_ref),
                                                   _call(seller, "negotiate_deal", operation_ref))
    negotiated = clock()
    if not (buyer_deal.get("ok") and seller_deal.get("ok")):
        raise HarnessError("negotiation did not authorize on both sides")
    commitment = buyer_deal["result"]["deal_commitment"]
    if seller_deal["result"]["deal_commitment"] != commitment:
        raise HarnessError("participants authorized different deals")
    evidence_name = Path(buyer_deal["result"]["evidence_file"]).name
    states: list[str] = []
    for attempt in range(1, max_polls + 1):
        reply = _structured(await buyer.call_tool(ACCESS_TOOL, {"evidence_name": evidence_name}))
        result = reply.get("result") or {}
        states.append(result.get("status") or (reply.get("error") or {}).get("code") or "unknown")
        if reply.get("ok"):
            receipt = result["result"]
            if not receipt.get("resource_verified"):
                raise HarnessError("retrieved resource failed verification")
            if receipt.get("payment_verified") or receipt.get("delivery_verified"):
                raise HarnessError("access must not claim independent payment or delivery verification")
            finished = clock()
            return {
                "profile": "x402-exact", "operation_ref": operation_ref, "deal_commitment": commitment,
                "retrieval_attempts": attempt, "retrieval_states": states,
                "resource_sha256": receipt["resource_sha256"], "resource_bytes": receipt["resource_bytes"],
                "seller_reported_payment_finalized": receipt.get("seller_reported_payment_finalized"),
                "payment_verified": False, "payment_verification": "independent auditor only",
                "stages_ms": {"negotiation": round((negotiated - started) * 1000),
                              "first_request_to_resource": round((finished - negotiated) * 1000)},
            }
        if reply.get("ok") is False and result.get("retry_without_payment") is False:
            raise HarnessError("access reported a non-retryable failure")
        await sleep(poll_seconds)
    raise HarnessError("resource not delivered within the polling budget; retrieve again later, do not pay again")


def server_params(python: str, negotiation_config: str, *, payment_config: str | None = None,
                  negotiation_cli: str | None = None, payment_cli: str | None = None,
                  access: dict[str, str] | None = None, server_command: str | None = None) -> StdioServerParameters:
    """`access` holds the operator's EREBUS_ACCESS_* variables; it adds retrieval to a buyer."""
    env = {"EREBUS_BACKEND": "metropolis", "EREBUS_NEGOTIATION_CONFIG": negotiation_config, **(access or {})}
    for name, value in (("EREBUS_PAYMENT_CONFIG", payment_config), ("EREBUS_NEGOTIATION_CLI", negotiation_cli),
                        ("EREBUS_PAYMENT_CLI", payment_cli)):
        if value is not None:
            env[name] = value
    if server_command:
        # An installed server resolves only installed native commands, never a checkout's venv.
        env["PATH"] = os.pathsep.join([str(Path(server_command).parent), "/usr/bin", "/bin"])
        return StdioServerParameters(command=server_command, args=[], env=env)
    return StdioServerParameters(command=python, args=["-m", "erebus_mcp.server"], env=env)


async def run_over_mcp(buyer_params: StdioServerParameters, seller_params: StdioServerParameters,
                       operation_ref: str, *, profile: str = "settlement", **options: Any) -> dict[str, Any]:
    if profile not in {"settlement", "x402-exact"}:
        raise HarnessError("profile must be settlement or x402-exact")
    buyer_allowed = (X402_BUYER_TOOLS,) if profile == "x402-exact" else (BUYER_TOOLS, BUYER_TOOLS | {ACCESS_TOOL})
    async with AsyncExitStack() as stack:
        sessions = []
        for params, allowed in ((buyer_params, buyer_allowed), (seller_params, (SELLER_TOOLS,))):
            read, write = await stack.enter_async_context(stdio_client(params))
            session = await stack.enter_async_context(ClientSession(read, write))
            await session.initialize()
            tools = {tool.name for tool in (await session.list_tools()).tools}
            if tools not in allowed:
                raise HarnessError(f"unexpected tool surface: {sorted(tools)}")
            sessions.append((session, tools))
        (buyer, buyer_tools), (seller, _) = sessions
        if profile == "x402-exact":
            return await drive_x402_deal(buyer, seller, operation_ref, **options)
        return await drive_deal(buyer, seller, operation_ref, deliver=ACCESS_TOOL in buyer_tools, **options)


def _harness_error(error: BaseException) -> HarnessError | None:
    if isinstance(error, HarnessError):
        return error
    if isinstance(error, BaseExceptionGroup):
        for inner in error.exceptions:
            if (found := _harness_error(inner)) is not None:
                return found
    return None


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--operation", required=True)
    parser.add_argument("--buyer-negotiation", required=True)
    parser.add_argument("--buyer-payment", help="ordinary settlement profile only")
    parser.add_argument("--profile", choices=["settlement", "x402-exact"], default="settlement")
    parser.add_argument("--server-command", help="installed erebus-mcp-server; default: this Python's module")
    parser.add_argument("--seller-negotiation", required=True)
    parser.add_argument("--negotiation-cli")
    parser.add_argument("--payment-cli")
    parser.add_argument("--max-polls", type=int, default=120)
    parser.add_argument("--buyer-access", help="JSON file of the buyer operator's EREBUS_ACCESS_* variables")
    args = parser.parse_args()
    if (args.profile == "x402-exact") == bool(args.buyer_payment):
        parser.error("x402-exact takes --buyer-access and no --buyer-payment; settlement requires --buyer-payment")
    clis = {"negotiation_cli": args.negotiation_cli, "payment_cli": args.payment_cli}
    access = json.loads(Path(args.buyer_access).read_text()) if args.buyer_access else None
    buyer = server_params(sys.executable, args.buyer_negotiation, payment_config=args.buyer_payment, access=access,
                          server_command=args.server_command, **clis)
    seller = server_params(sys.executable, args.seller_negotiation, negotiation_cli=args.negotiation_cli,
                           server_command=args.server_command)
    try:
        record = asyncio.run(run_over_mcp(buyer, seller, args.operation, profile=args.profile, max_polls=args.max_polls))
    except BaseException as error:
        # The MCP sessions' task group wraps a HarnessError in an ExceptionGroup.
        found = _harness_error(error)
        if found is None:
            raise
        print(json.dumps({"payment_verified": False, "error": str(found)}))
        raise SystemExit(1) from None
    print(json.dumps(record))


if __name__ == "__main__":
    main()
