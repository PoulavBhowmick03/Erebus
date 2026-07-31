"""I1.2 — two reference agents running the offer/counter/accept loop against the mock.

Direct calls to `MockErebusClient`, no MCP — see the plan this was built from for why:
`docs/ishita.md` writes I1.2 as "against the mock," and I1.3 (the MCP server) is a
separate, independently-verified way to reach the same `erebus_mcp` package, not something
I1.2's reference agents are required to route through.
"""

from __future__ import annotations

import json
import logging
from pathlib import Path

from erebus_mcp.interface import AgentId, DisclosedRecord
from erebus_mcp.mock_client import MockErebusClient

from erebus_agents.policy import BuyerPolicy, NegotiationAction, SellerPolicy

logger = logging.getLogger("erebus_agents")


def _log_event(event: str, **fields: object) -> None:
    logger.info(json.dumps({"event": event, **fields}))


async def run_negotiation(
    *,
    buyer_client: MockErebusClient,
    seller_client: MockErebusClient,
    buyer_policy: BuyerPolicy,
    seller_policy: SellerPolicy,
    buyer_address: AgentId,
    seller_address: AgentId,
    token: str,
    store_path: Path,
    auditor_address: AgentId = "auditor",
    max_rounds: int = 3,
) -> DisclosedRecord:
    """Opens a channel, negotiates to agreement or walk-away, settles if agreed, grants a
    viewing key, and reveals as a third party would — proving the record reconstructs from
    the shared store alone, not from either agent's memory.
    """
    handle = await buyer_client.open_channel(seller_address)
    await seller_client.open_channel(buyer_address)  # both sides derive the same channel
    _log_event("channel_opened", channel_handle=handle, buyer=buyer_address, seller=seller_address)

    # `max_rounds` here is a hard stop on the loop itself; each policy also carries its own
    # `max_rounds` and returns WALK once it's hit (that's the real cutoff — see policy.py).
    # Belt-and-suspenders: pass the same value to both so they agree, but this loop bound is
    # what actually prevents a runaway if a policy ever fails to WALK on schedule.
    settled = False
    for round_index in range(max_rounds + 1):
        buyer_state = await buyer_client.read_channel_state(handle)
        buyer_decision = buyer_policy.decide(buyer_state, round_index, token)
        _log_event("buyer_decision", round=round_index, action=buyer_decision.action.value)

        if buyer_decision.action == NegotiationAction.WALK:
            _log_event("buyer_walked", round=round_index)
            break
        if buyer_decision.action == NegotiationAction.ACCEPT:
            assert buyer_decision.reply_to is not None
            receipt = await buyer_client.accept_and_settle(handle, buyer_decision.reply_to)
            _log_event("settled", by="buyer", tx_hash=receipt.tx_hash, offer_id=buyer_decision.reply_to)
            settled = True
            break
        if buyer_decision.action == NegotiationAction.PROPOSE:
            offer_id = await buyer_client.propose_offer(handle, buyer_decision.terms)
            _log_event("proposed", by="buyer", offer_id=offer_id, amount=buyer_decision.terms.amount)
        elif buyer_decision.action == NegotiationAction.COUNTER:
            offer_id = await buyer_client.counter_offer(handle, buyer_decision.reply_to, buyer_decision.terms)
            _log_event("countered", by="buyer", offer_id=offer_id, amount=buyer_decision.terms.amount)

        seller_state = await seller_client.read_channel_state(handle)
        seller_decision = seller_policy.decide(seller_state, round_index)
        _log_event("seller_decision", round=round_index, action=seller_decision.action.value)

        if seller_decision.action == NegotiationAction.WALK:
            _log_event("seller_walked", round=round_index)
            break
        if seller_decision.action == NegotiationAction.ACCEPT:
            assert seller_decision.reply_to is not None
            receipt = await seller_client.accept_and_settle(handle, seller_decision.reply_to)
            _log_event("settled", by="seller", tx_hash=receipt.tx_hash, offer_id=seller_decision.reply_to)
            settled = True
            break
        if seller_decision.action == NegotiationAction.COUNTER:
            offer_id = await seller_client.counter_offer(handle, seller_decision.reply_to, seller_decision.terms)
            _log_event("countered", by="seller", offer_id=offer_id, amount=seller_decision.terms.amount)

    if not settled:
        _log_event("negotiation_ended_without_settlement", channel_handle=handle)

    # One agent grants a viewing key (docs/ishita.md I1.2); doesn't matter which side, so
    # the buyer does it. Then reveal as a genuine third party — a fresh client with no
    # relationship to either agent's identity, pointed only at the shared store and the
    # grant — to demonstrate ARCHITECTURE §3's claim that no grantor-local state is needed.
    grant = await buyer_client.grant_viewing_key(handle, auditor_address)
    _log_event("viewing_key_granted", channel_handle=handle, grantee=auditor_address)

    auditor_client = MockErebusClient(identity=auditor_address, store_path=store_path, latency_seconds=0)
    record = await auditor_client.reveal(grant)
    _log_event(
        "revealed",
        channel_handle=record.channel_id,
        participants=record.participants,
        offer_count=len(record.offers),
        settled=record.settlement is not None,
    )
    return record
