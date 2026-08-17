"""Runs the negotiation loop through real MCP subprocess servers, not the mock directly."""

from __future__ import annotations

import asyncio

from erebus_agents.mcp_loop import run_negotiation_over_mcp, server_params
from erebus_agents.policy import BuyerPolicy, SellerPolicy

BUYER_ADDRESS = "0xbuyer"
SELLER_ADDRESS = "0xseller"
AUDITOR_ADDRESS = "0xauditor"
TOKEN = "0xtoken"


def test_negotiation_settles_over_real_mcp_servers(tmp_path):
    store = tmp_path / "store.json"

    async def run():
        return await run_negotiation_over_mcp(
            buyer_params=server_params(store, "payer", BUYER_ADDRESS, spendable_notes="1000"),
            seller_params=server_params(store, "payee", SELLER_ADDRESS),
            auditor_params=server_params(store, "both", AUDITOR_ADDRESS),
            buyer_policy=BuyerPolicy(
                identity=BUYER_ADDRESS, budget=1000, deadline_seconds=3600, max_rounds=3
            ),
            seller_policy=SellerPolicy(
                identity=SELLER_ADDRESS, reserve=800, deadline_seconds=3600, max_rounds=3
            ),
            buyer_address=BUYER_ADDRESS,
            seller_address=SELLER_ADDRESS,
            auditor_address=AUDITOR_ADDRESS,
            token=TOKEN,
        )

    record = asyncio.run(run())

    assert sorted(record["participants"]) == sorted([BUYER_ADDRESS, SELLER_ADDRESS])
    assert record["settlement"] is not None
    assert record["settlement"]["is_consistent"] is True
