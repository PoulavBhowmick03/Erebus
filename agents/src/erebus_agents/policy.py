"""Negotiation policy — I1.1. Pure, deterministic, no MCP, no I/O.

"Keep this simple. A threshold rule is enough for the MVP — do not build a sophisticated
bargaining strategy" (docs/ishita.md). Both policies below make exactly one concession: an
opening anchor, one counter toward the caller's limit, then accept or walk. No concession
curve, no modeling of the other side.

Round-limit cutoff lives here, not in the orchestration loop, because it's the direct
consequence of a real cost (~29s/proof round, friction F7): the policy is the thing
deciding whether another round is worth ~29s, so the cutoff belongs where that judgment is
made.
"""

from __future__ import annotations

import time
from dataclasses import dataclass
from enum import Enum

from erebus_mcp.interface import AgentId, ChannelState, Offer, OfferId, OfferStatus, OfferTerms


class NegotiationAction(str, Enum):
    PROPOSE = "propose"
    ACCEPT = "accept"
    COUNTER = "counter"
    WALK = "walk"


@dataclass(frozen=True)
class NegotiationDecision:
    action: NegotiationAction
    terms: OfferTerms | None = None  # for PROPOSE / COUNTER
    reply_to: OfferId | None = None  # for COUNTER (offer being countered) / ACCEPT (offer being accepted)


_OPEN_STATUSES = (OfferStatus.PROPOSED, OfferStatus.COUNTERED)


def _latest_open_offer(state: ChannelState, own_identity: AgentId) -> Offer | None:
    open_from_counterparty = [
        o for o in state.offers if o.proposer != own_identity and o.status in _OPEN_STATUSES
    ]
    if not open_from_counterparty:
        return None
    return max(open_from_counterparty, key=lambda o: o.created_at)


class BuyerPolicy:
    """Has a budget and a task; proposes, evaluates counters, accepts within budget, walks
    away otherwise (docs/ishita.md I1.1)."""

    def __init__(self, identity: AgentId, budget: int, deadline_seconds: int, max_rounds: int = 3) -> None:
        self.identity = identity
        self.budget = budget
        self.deadline_seconds = deadline_seconds
        self.max_rounds = max_rounds

    def decide(self, state: ChannelState, round_index: int, token: str, memo_hash: int = 0) -> NegotiationDecision:
        counter = _latest_open_offer(state, self.identity)
        now = int(time.time())
        deadline = now + self.deadline_seconds

        if counter is not None:
            if counter.terms.amount <= self.budget:
                return NegotiationDecision(action=NegotiationAction.ACCEPT, reply_to=counter.offer_id)
            if round_index >= self.max_rounds:
                return NegotiationDecision(action=NegotiationAction.WALK)
            terms = OfferTerms(amount=self.budget, token=token, deadline=deadline, memo_hash=memo_hash)
            return NegotiationDecision(
                action=NegotiationAction.COUNTER, terms=terms, reply_to=counter.offer_id
            )

        # No offer from the seller yet — this is the opening round.
        if round_index >= self.max_rounds:
            return NegotiationDecision(action=NegotiationAction.WALK)
        opening_amount = int(self.budget * 0.8)  # anchor below budget, leave room to move
        terms = OfferTerms(amount=opening_amount, token=token, deadline=deadline, memo_hash=memo_hash)
        return NegotiationDecision(action=NegotiationAction.PROPOSE, terms=terms)


class SellerPolicy:
    """Has a reserve price; evaluates offers, counters once, accepts or declines
    (docs/ishita.md I1.1). Only ever reacts — the buyer opens."""

    def __init__(self, identity: AgentId, reserve: int, deadline_seconds: int, max_rounds: int = 3) -> None:
        self.identity = identity
        self.reserve = reserve
        self.deadline_seconds = deadline_seconds
        self.max_rounds = max_rounds

    def decide(self, state: ChannelState, round_index: int, memo_hash: int = 0) -> NegotiationDecision:
        offer = _latest_open_offer(state, self.identity)
        if offer is None:
            raise ValueError(
                "SellerPolicy.decide called with no open offer to react to — "
                "the buyer proposes first; this policy never opens"
            )

        if offer.terms.amount >= self.reserve:
            return NegotiationDecision(action=NegotiationAction.ACCEPT, reply_to=offer.offer_id)
        if round_index >= self.max_rounds:
            return NegotiationDecision(action=NegotiationAction.WALK)
        # Same token as the offer being countered — a subchannel is a token, so a counter
        # can't switch which one it's in.
        deadline = int(time.time()) + self.deadline_seconds
        terms = OfferTerms(amount=self.reserve, token=offer.terms.token, deadline=deadline, memo_hash=memo_hash)
        return NegotiationDecision(
            action=NegotiationAction.COUNTER, terms=terms, reply_to=offer.offer_id
        )
