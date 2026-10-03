"""Two agents drive native negotiation and settlement, each through its own Metropolis MCP server.

Prices, keys, and settlement mode are fixed by each operator's files, so an agent supplies only
the shared operation ID and sequences the calls. The one rule an agent must hold itself: after a
settlement attempt, never call `settle_deal` again. Uncertainty is resolved with `recover_deal`,
which observes and cannot submit. A pending result is not a reason to pay twice.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import sys
from contextlib import AsyncExitStack
from typing import Any, Awaitable, Callable, Protocol

from mcp import ClientSession
from mcp.client.stdio import StdioServerParameters, stdio_client

BUYER_TOOLS = {"negotiate_deal", "check_settlement_funding", "settle_deal", "recover_deal"}
SELLER_TOOLS = {"negotiate_deal"}


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
                     poll_seconds: float = 1.0, max_polls: int = 120,
                     sleep: Callable[[float], Awaitable[None]] = asyncio.sleep) -> dict[str, Any]:
    """Negotiate on both sides at once, settle at most once, then observe until finalized."""
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
                  measurements_ms=(record["settlement"] or {}).get("measurements_ms", {}))
    if record["winning_commitment"] != commitment:
        raise HarnessError("finalized payment does not match the negotiated deal")
    return record


def server_params(python: str, negotiation_config: str, *, payment_config: str | None = None,
                  negotiation_cli: str | None = None, payment_cli: str | None = None) -> StdioServerParameters:
    env = {"EREBUS_BACKEND": "metropolis", "EREBUS_NEGOTIATION_CONFIG": negotiation_config}
    for name, value in (("EREBUS_PAYMENT_CONFIG", payment_config), ("EREBUS_NEGOTIATION_CLI", negotiation_cli),
                        ("EREBUS_PAYMENT_CLI", payment_cli)):
        if value is not None:
            env[name] = value
    return StdioServerParameters(command=python, args=["-m", "erebus_mcp.server"], env=env)


async def run_over_mcp(buyer_params: StdioServerParameters, seller_params: StdioServerParameters,
                       operation_ref: str, **options: Any) -> dict[str, Any]:
    async with AsyncExitStack() as stack:
        sessions = []
        for params, expected in ((buyer_params, BUYER_TOOLS), (seller_params, SELLER_TOOLS)):
            read, write = await stack.enter_async_context(stdio_client(params))
            session = await stack.enter_async_context(ClientSession(read, write))
            await session.initialize()
            tools = {tool.name for tool in (await session.list_tools()).tools}
            if tools != expected:
                raise HarnessError(f"unexpected tool surface: {sorted(tools)}")
            sessions.append(session)
        return await drive_deal(*sessions, operation_ref, **options)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--operation", required=True)
    parser.add_argument("--buyer-negotiation", required=True)
    parser.add_argument("--buyer-payment", required=True)
    parser.add_argument("--seller-negotiation", required=True)
    parser.add_argument("--negotiation-cli")
    parser.add_argument("--payment-cli")
    parser.add_argument("--max-polls", type=int, default=120)
    args = parser.parse_args()
    clis = {"negotiation_cli": args.negotiation_cli, "payment_cli": args.payment_cli}
    buyer = server_params(sys.executable, args.buyer_negotiation, payment_config=args.buyer_payment, **clis)
    seller = server_params(sys.executable, args.seller_negotiation, negotiation_cli=args.negotiation_cli)
    try:
        record = asyncio.run(run_over_mcp(buyer, seller, args.operation, max_polls=args.max_polls))
    except HarnessError as error:
        print(json.dumps({"payment_verified": False, "error": str(error)}))
        raise SystemExit(1) from None
    print(json.dumps(record))


if __name__ == "__main__":
    main()
