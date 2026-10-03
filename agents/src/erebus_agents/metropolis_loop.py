"""Two agents drive native negotiation and settlement, each through its own Metropolis MCP server.

Prices, keys, and settlement mode are fixed by each operator's files, so an agent supplies only
the shared operation ID and sequences the calls. The one rule an agent must hold itself: after a
settlement attempt, never call `settle_deal` again. Uncertainty is resolved with `recover_deal`,
which observes and cannot submit. A pending result is not a reason to pay twice. When the buyer's
server also exposes access, failed delivery is retried by retrieving again, never by paying.

Timings: `inclusion` and `finality` come from block timestamps (whole seconds) against this
process's clock when `settle_deal` returned, so they are coarse and can read 0 or slightly negative
on a fast chain. `payment_verified_after` is bounded below by the polling interval.
"""

from __future__ import annotations

import argparse
import asyncio
import json
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


def _latencies(times: dict[str, int], submitted: float, verified: float) -> dict[str, float]:
    latencies = {"payment_verified_after": round(verified - submitted, 3)}
    if "inclusion_unix" in times:
        latencies["inclusion"] = round(times["inclusion_unix"] - submitted, 3)
        latencies["finality"] = times["finalized_anchor_unix"] - times["inclusion_unix"]
    return latencies


async def drive_deal(buyer: ToolSession, seller: ToolSession, operation_ref: str, *,
                     poll_seconds: float = 1.0, max_polls: int = 120, deliver: bool = False,
                     sleep: Callable[[float], Awaitable[None]] = asyncio.sleep,
                     clock: Callable[[], float] = time.time) -> dict[str, Any]:
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

    settled = await _call(buyer, "settle_deal", operation_ref)
    submitted = clock()
    record["settle_calls"] = 1
    record["settlement"] = settled.get("result")
    outcome = settled.get("result") or {}
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
    record.update(payment_verified=True, stage=outcome.get("stage"),
                  winning_commitment=outcome.get("winning_commitment"),
                  measurements_ms=(record["settlement"] or {}).get("measurements_ms", {}),
                  latency_s=_latencies(outcome.get("chain_times", {}), submitted, clock()))
    if record["winning_commitment"] != commitment:
        raise HarnessError("finalized payment does not match the negotiated deal")
    if deliver:
        record["delivery"] = await _deliver(buyer, Path(buyer_deal["result"]["evidence_file"]).name,
                                            poll_seconds=poll_seconds, max_polls=max_polls, sleep=sleep, clock=clock)
        record["latency_s"]["delivery"] = record["delivery"].pop("seconds")
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


def server_params(python: str, negotiation_config: str, *, payment_config: str | None = None,
                  negotiation_cli: str | None = None, payment_cli: str | None = None,
                  access: dict[str, str] | None = None) -> StdioServerParameters:
    """`access` holds the operator's EREBUS_ACCESS_* variables; it adds retrieval to a buyer."""
    env = {"EREBUS_BACKEND": "metropolis", "EREBUS_NEGOTIATION_CONFIG": negotiation_config, **(access or {})}
    for name, value in (("EREBUS_PAYMENT_CONFIG", payment_config), ("EREBUS_NEGOTIATION_CLI", negotiation_cli),
                        ("EREBUS_PAYMENT_CLI", payment_cli)):
        if value is not None:
            env[name] = value
    return StdioServerParameters(command=python, args=["-m", "erebus_mcp.server"], env=env)


async def run_over_mcp(buyer_params: StdioServerParameters, seller_params: StdioServerParameters,
                       operation_ref: str, **options: Any) -> dict[str, Any]:
    async with AsyncExitStack() as stack:
        sessions = []
        for params, allowed in ((buyer_params, (BUYER_TOOLS, BUYER_TOOLS | {ACCESS_TOOL})), (seller_params, (SELLER_TOOLS,))):
            read, write = await stack.enter_async_context(stdio_client(params))
            session = await stack.enter_async_context(ClientSession(read, write))
            await session.initialize()
            tools = {tool.name for tool in (await session.list_tools()).tools}
            if tools not in allowed:
                raise HarnessError(f"unexpected tool surface: {sorted(tools)}")
            sessions.append((session, tools))
        (buyer, buyer_tools), (seller, _) = sessions
        return await drive_deal(buyer, seller, operation_ref, deliver=ACCESS_TOOL in buyer_tools, **options)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--operation", required=True)
    parser.add_argument("--buyer-negotiation", required=True)
    parser.add_argument("--buyer-payment", required=True)
    parser.add_argument("--seller-negotiation", required=True)
    parser.add_argument("--negotiation-cli")
    parser.add_argument("--payment-cli")
    parser.add_argument("--max-polls", type=int, default=120)
    parser.add_argument("--buyer-access", help="JSON file of the buyer operator's EREBUS_ACCESS_* variables")
    args = parser.parse_args()
    clis = {"negotiation_cli": args.negotiation_cli, "payment_cli": args.payment_cli}
    access = json.loads(Path(args.buyer_access).read_text()) if args.buyer_access else None
    buyer = server_params(sys.executable, args.buyer_negotiation, payment_config=args.buyer_payment, access=access, **clis)
    seller = server_params(sys.executable, args.seller_negotiation, negotiation_cli=args.negotiation_cli)
    try:
        record = asyncio.run(run_over_mcp(buyer, seller, args.operation, max_polls=args.max_polls))
    except HarnessError as error:
        print(json.dumps({"payment_verified": False, "error": str(error)}))
        raise SystemExit(1) from None
    print(json.dumps(record))


if __name__ == "__main__":
    main()
