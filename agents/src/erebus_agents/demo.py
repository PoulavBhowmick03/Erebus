"""I1.2's acceptance criterion: `uv run python agents/demo.py` completes a full negotiation
against the mock.

Wires two dummy identities against one shared mock store, runs the negotiation, and prints
each transition plus the final revealed record. Fast by default (latency=0.2s/round) so
this is a normal dev-loop command; `--latency 29` rehearses the real per-round cost before
recording the demo video (I2.2 — not built here, but this flag is what unblocks it).
"""

from __future__ import annotations

import argparse
import asyncio
import json
import logging
import sys
import tempfile
from pathlib import Path

from erebus_mcp.interface import DisclosedRecord
from erebus_mcp.mock_client import MockErebusClient

from erebus_agents.agent import run_negotiation
from erebus_agents.policy import BuyerPolicy, SellerPolicy

BUYER_ADDRESS = "0xbuyer"
SELLER_ADDRESS = "0xseller"
AUDITOR_ADDRESS = "0xauditor"
TOKEN = "0xtoken"


def _parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--latency", type=float, default=0.2, help="simulated seconds per proof-bearing call (real ~29s)"
    )
    parser.add_argument("--rounds", type=int, default=3, help="max negotiation rounds before walking away")
    parser.add_argument("--budget", type=int, default=1000, help="buyer's budget, token base units")
    parser.add_argument("--reserve", type=int, default=800, help="seller's reserve, token base units")
    return parser.parse_args()


async def _main() -> DisclosedRecord:
    args = _parse_args()
    logging.basicConfig(level=logging.INFO, format="%(message)s", stream=sys.stdout)

    with tempfile.TemporaryDirectory() as tmp:
        store_path = Path(tmp) / "erebus-mock-store.json"
        buyer_client = MockErebusClient(identity=BUYER_ADDRESS, store_path=store_path, latency_seconds=args.latency)
        seller_client = MockErebusClient(identity=SELLER_ADDRESS, store_path=store_path, latency_seconds=args.latency)
        buyer_policy = BuyerPolicy(
            identity=BUYER_ADDRESS, budget=args.budget, deadline_seconds=3600, max_rounds=args.rounds
        )
        seller_policy = SellerPolicy(
            identity=SELLER_ADDRESS, reserve=args.reserve, deadline_seconds=3600, max_rounds=args.rounds
        )

        record = await run_negotiation(
            buyer_client=buyer_client,
            seller_client=seller_client,
            buyer_policy=buyer_policy,
            seller_policy=seller_policy,
            buyer_address=BUYER_ADDRESS,
            seller_address=SELLER_ADDRESS,
            token=TOKEN,
            store_path=store_path,
            auditor_address=AUDITOR_ADDRESS,
            max_rounds=args.rounds,
        )

    print("\n--- final revealed record ---")
    print(
        json.dumps(
            {
                "channel_id": record.channel_id,
                "participants": record.participants,
                "offers": [
                    {
                        "offer_id": o.offer_id,
                        "proposer": o.proposer,
                        "status": o.status.value,
                        "amount": o.terms.amount,
                    }
                    for o in record.offers
                ],
                "settlement": (
                    {
                        "agreed_amount": record.settlement.agreed_amount,
                        "paid_amount": record.settlement.paid_amount,
                        "is_consistent": record.settlement.is_consistent(),
                    }
                    if record.settlement
                    else None
                ),
            },
            indent=2,
        )
    )
    return record


if __name__ == "__main__":
    asyncio.run(_main())
