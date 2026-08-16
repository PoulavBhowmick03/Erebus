"""I1.1 deterministic policy tests without MCP, an event loop, or a mock client."""

from __future__ import annotations

import time

import pytest
from erebus_mcp.interface import ChannelState, Offer, OfferStatus, OfferTerms

from erebus_agents.policy import BuyerPolicy, NegotiationAction, SellerPolicy

TOKEN = "0xtoken"


def _offer(proposer: str, amount: int, offer_id: str = "o1", status: OfferStatus = OfferStatus.PROPOSED,
           deadline_delta: int = 3600) -> Offer:
    return Offer(
        offer_id=offer_id,
        channel_id="ch_x",
        proposer=proposer,
        terms=OfferTerms(amount=amount, token=TOKEN, deadline=int(time.time()) + deadline_delta, memo_hash=0),
        status=status,
        created_at=int(time.time()),
    )


def test_buyer_opens_with_no_offers_on_the_table():
    policy = BuyerPolicy(identity="buyer", budget=1000, deadline_seconds=3600, max_rounds=3)
    decision = policy.decide(
        ChannelState(offers=[]), round_index=0, token=TOKEN, spendable_total=1000
    )

    assert decision.action == NegotiationAction.PROPOSE
    assert decision.terms is not None
    assert decision.terms.amount <= 1000


def test_buyer_accepts_a_counter_within_budget():
    policy = BuyerPolicy(identity="buyer", budget=1000, deadline_seconds=3600, max_rounds=3)
    state = ChannelState(offers=[_offer("seller", amount=900)])

    decision = policy.decide(state, round_index=1, token=TOKEN, spendable_total=1000)

    assert decision.action == NegotiationAction.ACCEPT
    assert decision.reply_to == "o1"


def test_buyer_counters_when_above_budget_and_rounds_remain():
    policy = BuyerPolicy(identity="buyer", budget=1000, deadline_seconds=3600, max_rounds=3)
    state = ChannelState(offers=[_offer("seller", amount=1500)])

    decision = policy.decide(state, round_index=1, token=TOKEN, spendable_total=1000)

    assert decision.action == NegotiationAction.COUNTER
    assert decision.terms.amount == 1000
    assert decision.reply_to == "o1"


def test_buyer_walks_once_max_rounds_is_hit():
    policy = BuyerPolicy(identity="buyer", budget=1000, deadline_seconds=3600, max_rounds=2)
    state = ChannelState(offers=[_offer("seller", amount=1500)])

    decision = policy.decide(state, round_index=2, token=TOKEN, spendable_total=1000)

    assert decision.action == NegotiationAction.WALK


def test_buyer_ignores_its_own_open_offer_when_deciding():
    # Only the counterparty's open offers should drive the decision; a buyer looking at its
    # own still-open proposal must not try to "accept" itself.
    policy = BuyerPolicy(identity="buyer", budget=1000, deadline_seconds=3600, max_rounds=3)
    state = ChannelState(offers=[_offer("buyer", amount=100)])

    decision = policy.decide(state, round_index=1, token=TOKEN, spendable_total=1000)

    assert decision.action == NegotiationAction.PROPOSE  # treated as if nothing to react to


def test_buyer_treats_an_expired_counter_as_not_there():
    # `read_channel_state` marks expired offers before policy evaluation (ARCHITECTURE §4).
    # The policy trusts that status instead of checking the deadline again.
    policy = BuyerPolicy(identity="buyer", budget=1000, deadline_seconds=3600, max_rounds=3)
    state = ChannelState(
        offers=[_offer("seller", amount=900, deadline_delta=-1, status=OfferStatus.EXPIRED)]
    )

    decision = policy.decide(state, round_index=1, token=TOKEN, spendable_total=1000)

    assert decision.action == NegotiationAction.PROPOSE


def test_seller_confirms_an_offer_at_reserve_without_becoming_the_payer():
    policy = SellerPolicy(identity="seller", reserve=800, deadline_seconds=3600, max_rounds=3)
    state = ChannelState(offers=[_offer("buyer", amount=800)])

    decision = policy.decide(state, round_index=0)

    assert decision.action == NegotiationAction.COUNTER
    assert decision.reply_to == "o1"
    assert decision.terms is not None
    assert decision.terms.amount == 800


def test_buyer_accepts_inexact():
    policy = BuyerPolicy(identity="buyer", budget=1000, deadline_seconds=3600, max_rounds=3)
    state = ChannelState(offers=[_offer("seller", amount=700)])

    decision = policy.decide(state, round_index=1, token=TOKEN, spendable_total=1000)

    assert decision.action == NegotiationAction.ACCEPT
    assert decision.reply_to == "o1"


def test_buyer_counters_capped():
    policy = BuyerPolicy(identity="buyer", budget=1000, deadline_seconds=3600, max_rounds=3)
    state = ChannelState(offers=[_offer("seller", amount=700)])

    decision = policy.decide(state, round_index=1, token=TOKEN, spendable_total=500)

    assert decision.action == NegotiationAction.COUNTER
    assert decision.terms is not None
    assert decision.terms.amount == 500


def test_seller_counters_at_reserve_when_offer_is_below_it():
    policy = SellerPolicy(identity="seller", reserve=800, deadline_seconds=3600, max_rounds=3)
    state = ChannelState(offers=[_offer("buyer", amount=500)])

    decision = policy.decide(state, round_index=0)

    assert decision.action == NegotiationAction.COUNTER
    assert decision.terms.amount == 800
    assert decision.terms.token == TOKEN  # carries the same token as the offer it replies to


def test_seller_walks_once_max_rounds_is_hit():
    policy = SellerPolicy(identity="seller", reserve=800, deadline_seconds=3600, max_rounds=1)
    state = ChannelState(offers=[_offer("buyer", amount=500)])

    decision = policy.decide(state, round_index=1)

    assert decision.action == NegotiationAction.WALK


def test_seller_never_opens_and_raises_on_an_empty_channel():
    policy = SellerPolicy(identity="seller", reserve=800, deadline_seconds=3600, max_rounds=3)

    with pytest.raises(ValueError):
        policy.decide(ChannelState(offers=[]), round_index=0)
