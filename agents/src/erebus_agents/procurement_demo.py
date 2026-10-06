"""Mock rehearsal of the procurement scenario: negotiate, settle, grant an auditor one deal, review it."""

from __future__ import annotations

import argparse
import asyncio
import json
import logging
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

from erebus_mcp.mock_client import MockErebusClient

from erebus_agents.agent import run_negotiation
from erebus_agents.procurement import Catalog, Requisition, buyer_policy, review_disclosure, supplier_policy

BUYER, SELLER, AUDITOR = "0xbuyer", "0xsupplier", "0xauditor"
TOKEN = "0xtoken"


async def rehearse(requisition: Requisition, catalog: Catalog, *, latency: float = 0.0,
                   max_rounds: int = 3, store_dir: Path) -> dict[str, Any]:
    store = store_dir / "erebus-mock-store.json"
    buyer = MockErebusClient(identity=BUYER, store_path=store, latency_seconds=latency,
                             spendable_notes=[requisition.ceiling])
    supplier = MockErebusClient(identity=SELLER, store_path=store, latency_seconds=latency, spendable_notes=[])
    auditor = MockErebusClient(identity=AUDITOR, store_path=store, latency_seconds=latency, spendable_notes=[])
    state = await run_negotiation(
        buyer_client=buyer, seller_client=supplier, buyer_address=BUYER, seller_address=SELLER,
        buyer_policy=buyer_policy(BUYER, requisition, max_rounds=max_rounds),
        seller_policy=supplier_policy(SELLER, catalog, requisition, max_rounds=max_rounds),
        token=TOKEN, max_rounds=max_rounds)
    if state.settlement is None:
        return {"settled": False, "offers": len(state.offers)}
    accepted = next(offer for offer in state.offers if offer.offer_id == state.settlement.acceptance)
    handle = await buyer.open_channel(SELLER)
    grant = await buyer.grant_viewing_key(handle, str(accepted.deal_id), AUDITOR, int(time.time()) + 3600)
    review = review_disclosure(await auditor.reveal(grant), requisition)
    return {"settled": True, "agreed_amount": state.settlement.agreed_amount,
            "ceiling": requisition.ceiling, "audit": {"compliant": review.compliant, "reasons": review.reasons}}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--item", default="gpu-hours")
    parser.add_argument("--quantity", type=int, default=100)
    parser.add_argument("--max-unit-price", type=int, default=12, help="buyer's ceiling per unit")
    parser.add_argument("--floor-unit-price", type=int, default=9, help="supplier's floor per unit")
    parser.add_argument("--rounds", type=int, default=3)
    parser.add_argument("--latency", type=float, default=0.2)
    args = parser.parse_args()
    logging.basicConfig(level=logging.INFO, format="%(message)s", stream=sys.stdout)
    requisition = Requisition(args.item, args.quantity, args.max_unit_price)
    with tempfile.TemporaryDirectory() as tmp:
        record = asyncio.run(rehearse(requisition, Catalog(args.item, args.floor_unit_price),
                                      latency=args.latency, max_rounds=args.rounds, store_dir=Path(tmp)))
    print(json.dumps(record, indent=2))


if __name__ == "__main__":
    main()
