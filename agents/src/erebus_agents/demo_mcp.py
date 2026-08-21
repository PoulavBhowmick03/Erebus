"""Run the same negotiation as demo.py, but over real MCP servers.

Spawns three `server.py` subprocesses (buyer, seller, auditor) instead of calling
MockErebusClient directly.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import logging
import sys
import tempfile
from pathlib import Path

from erebus_agents.mcp_loop import run_negotiation_over_mcp, server_params
from erebus_agents.policy import BuyerPolicy, SellerPolicy

BUYER_ADDRESS = "0xbuyer"
SELLER_ADDRESS = "0xseller"
AUDITOR_ADDRESS = "0xauditor"
TOKEN = "0xtoken"


def _parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rounds", type=int, default=3, help="max negotiation rounds before walking away")
    parser.add_argument("--budget", type=int, default=1000, help="buyer's budget, token base units")
    parser.add_argument("--reserve", type=int, default=800, help="seller's reserve, token base units")
    return parser.parse_args()


async def _main() -> dict:
    args = _parse_args()
    logging.basicConfig(level=logging.INFO, format="%(message)s", stream=sys.stdout)

    with tempfile.TemporaryDirectory() as tmp:
        store_path = Path(tmp) / "erebus-mock-store.json"
        record = await run_negotiation_over_mcp(
            buyer_params=server_params(
                store_path, "payer", BUYER_ADDRESS, spendable_notes=str(args.budget)
            ),
            seller_params=server_params(store_path, "payee", SELLER_ADDRESS),
            auditor_params=server_params(store_path, "both", AUDITOR_ADDRESS),
            buyer_policy=BuyerPolicy(
                identity=BUYER_ADDRESS, budget=args.budget, deadline_seconds=3600, max_rounds=args.rounds
            ),
            seller_policy=SellerPolicy(
                identity=SELLER_ADDRESS, reserve=args.reserve, deadline_seconds=3600, max_rounds=args.rounds
            ),
            buyer_address=BUYER_ADDRESS,
            seller_address=SELLER_ADDRESS,
            auditor_address=AUDITOR_ADDRESS,
            grant_export_path=Path(tmp) / "viewing-grant.json",
            token=TOKEN,
            max_rounds=args.rounds,
        )

    print("\n--- final revealed record ---")
    print(json.dumps(record, indent=2))
    return record


if __name__ == "__main__":
    asyncio.run(_main())
