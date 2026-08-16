"""Deterministic negotiation policy for I1.1. No MCP or I/O.

Both policies make one concession: an opening anchor followed by one counter toward the
caller's limit. They then accept or walk. See docs/ishita.md.

The policy owns the round limit because it decides whether another ~29 s proof is worth
running. See friction F7.
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

    def decide(
        self,
        state: ChannelState,
        round_index: int,
        token: str,
        spendable_total: int,
        memo_hash: int = 0,
    ) -> NegotiationDecision:
        counter = _latest_open_offer(state, self.identity)
        now = int(time.time())
        deadline = now + self.deadline_seconds

        affordable = min(self.budget, spendable_total)
        if affordable <= 0:
            return NegotiationDecision(action=NegotiationAction.WALK)

        if counter is not None:
            if counter.terms.amount <= affordable:
                return NegotiationDecision(action=NegotiationAction.ACCEPT, reply_to=counter.offer_id)
            if round_index >= self.max_rounds:
                return NegotiationDecision(action=NegotiationAction.WALK)
            amount = min(counter.terms.amount, affordable)
            terms = OfferTerms(amount=amount, token=token, deadline=deadline, memo_hash=memo_hash)
            return NegotiationDecision(
                action=NegotiationAction.COUNTER, terms=terms, reply_to=counter.offer_id
            )

        # No seller offer exists yet, so open the negotiation.
        if round_index >= self.max_rounds:
            return NegotiationDecision(action=NegotiationAction.WALK)
        opening_amount = max(1, min(affordable, int(affordable * 0.8)))
        terms = OfferTerms(
            amount=opening_amount,
            token=token,
            deadline=deadline,
            memo_hash=memo_hash,
        )
        return NegotiationDecision(action=NegotiationAction.PROPOSE, terms=terms)


class SellerPolicy:
    """Has a reserve price and leaves seller-authored terms for the buyer to settle.

    The seller never returns ``ACCEPT``: in Erebus the accepting identity pays. Agreement
    with a buyer bid is represented by countering at that same amount, after which the
    buyer accepts the seller-authored offer and funds the atomic settlement.
    """

    def __init__(self, identity: AgentId, reserve: int, deadline_seconds: int, max_rounds: int = 3) -> None:
        self.identity = identity
        self.reserve = reserve
        self.deadline_seconds = deadline_seconds
        self.max_rounds = max_rounds

    def decide(self, state: ChannelState, round_index: int, memo_hash: int = 0) -> NegotiationDecision:
        offer = _latest_open_offer(state, self.identity)
        if offer is None:
            raise ValueError(
                "SellerPolicy.decide called with no open offer to react to. "
                "the buyer proposes first; this policy never opens"
            )

        if offer.terms.amount >= self.reserve:
            deadline = int(time.time()) + self.deadline_seconds
            terms = OfferTerms(
                amount=offer.terms.amount,
                token=offer.terms.token,
                deadline=deadline,
                memo_hash=memo_hash,
            )
            return NegotiationDecision(
                action=NegotiationAction.COUNTER,
                terms=terms,
                reply_to=offer.offer_id,
            )
        if round_index >= self.max_rounds:
            return NegotiationDecision(action=NegotiationAction.WALK)
        # A subchannel is token-specific, so a counter cannot change the token.
        deadline = int(time.time()) + self.deadline_seconds
        terms = OfferTerms(amount=self.reserve, token=offer.terms.token, deadline=deadline, memo_hash=memo_hash)
        return NegotiationDecision(
            action=NegotiationAction.COUNTER, terms=terms, reply_to=offer.offer_id
        )
