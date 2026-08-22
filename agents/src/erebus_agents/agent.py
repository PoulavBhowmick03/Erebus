"""Two I1.2 reference agents running the negotiation loop against the mock.

These agents call `MockErebusClient` directly. The MCP server in I1.3 provides a separate
route to the same `erebus_mcp` package. See `docs/ishita.md`.
"""

from __future__ import annotations

import json
import logging
from erebus_mcp.interface import AgentId, ChannelState
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
    max_rounds: int = 3,
) -> ChannelState:
    """Run a wire-v3 negotiation and return the buyer's final participant view."""
    buyer_handle = await buyer_client.open_channel(seller_address)
    seller_handle = await seller_client.open_channel(buyer_address)
    # Two directional channels, two handles. A handle resolves inside one client's own state
    # directory, so neither side can use the other's: see ARCHITECTURE §3.
    _log_event(
        "channel_opened",
        buyer_handle=buyer_handle,
        seller_handle=seller_handle,
        buyer=buyer_address,
        seller=seller_address,
    )

    # Policies normally return WALK at `max_rounds`. This bound also stops a policy that
    # fails to do so.
    settled = False
    for round_index in range(max_rounds + 1):
        buyer_state = await buyer_client.read_channel_state(buyer_handle)
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
            receipt = await buyer_client.accept_and_settle(buyer_handle, buyer_decision.reply_to)
            _log_event("settled", by="buyer", tx_hash=receipt.tx_hash, offer_id=buyer_decision.reply_to)
            settled = True
            break
        if buyer_decision.action == NegotiationAction.PROPOSE:
            offer_id = await buyer_client.propose_offer(buyer_handle, buyer_decision.terms)
            _log_event("proposed", by="buyer", offer_id=offer_id, amount=buyer_decision.terms.amount)
        elif buyer_decision.action == NegotiationAction.COUNTER:
            offer_id = await buyer_client.counter_offer(
                buyer_handle, buyer_decision.reply_to, buyer_decision.terms
            )
            _log_event("countered", by="buyer", offer_id=offer_id, amount=buyer_decision.terms.amount)

        seller_state = await seller_client.read_channel_state(seller_handle)
        seller_decision = seller_policy.decide(seller_state, round_index)
        _log_event("seller_decision", round=round_index, action=seller_decision.action.value)

        if seller_decision.action == NegotiationAction.WALK:
            _log_event("seller_walked", round=round_index)
            break
        if seller_decision.action == NegotiationAction.ACCEPT:
            raise RuntimeError("seller policy cannot accept: the accepting identity pays")
        if seller_decision.action == NegotiationAction.COUNTER:
            offer_id = await seller_client.counter_offer(
                seller_handle, seller_decision.reply_to, seller_decision.terms
            )
            _log_event("countered", by="seller", offer_id=offer_id, amount=seller_decision.terms.amount)

    if not settled:
        _log_event("negotiation_ended_without_settlement", channel_handle=buyer_handle)

    state = await buyer_client.read_channel_state(buyer_handle)
    _log_event(
        "final_state",
        channel_handle=buyer_handle,
        offer_count=len(state.offers),
        settled=state.settlement is not None,
        disclosure="available_as_an_explicit_recipient_bound_operator_step",
    )
    return state
