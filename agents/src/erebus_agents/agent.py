"""Two I1.2 reference agents running the negotiation loop against the mock.

These agents call `MockErebusClient` directly. The MCP server in I1.3 provides a separate
route to the same `erebus_mcp` package. See `docs/ishita.md`.
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
    """Run a negotiation and reconstruct its disclosed record from the shared store."""
    handle = await buyer_client.open_channel(seller_address)
    await seller_client.open_channel(buyer_address)  # both sides derive the same channel
    _log_event("channel_opened", channel_handle=handle, buyer=buyer_address, seller=seller_address)

    # Policies normally return WALK at `max_rounds`. This bound also stops a policy that
    # fails to do so.
    settled = False
    for round_index in range(max_rounds + 1):
        buyer_state = await buyer_client.read_channel_state(handle)
        buyer_balance = await buyer_client.note_balance()
        buyer_decision = buyer_policy.decide(
            buyer_state,
            round_index,
            token,
            buyer_balance.total,
        )
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
            raise RuntimeError("seller policy cannot accept: the accepting identity pays")
        if seller_decision.action == NegotiationAction.COUNTER:
            offer_id = await seller_client.counter_offer(handle, seller_decision.reply_to, seller_decision.terms)
            _log_event("countered", by="seller", offer_id=offer_id, amount=seller_decision.terms.amount)

    if not settled:
        _log_event("negotiation_ended_without_settlement", channel_handle=handle)

    # A fresh auditor client receives only the grant and shared store. This checks the
    # ARCHITECTURE §3 claim that disclosure needs no grantor-local state.
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
