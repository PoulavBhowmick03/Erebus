"""I0.2 acceptance tests for every mock method and error-code group.

Each test uses `asyncio.run()` instead of adding pytest-asyncio for sequential calls.
"""

from __future__ import annotations

import asyncio
import dataclasses
import hashlib
import time

import pytest
from erebus_mcp.interface import (
    Consistency,
    ErebusError,
    OfferTerms,
    SettlementErrorCode,
    ViewingKeyGrant,
)
from erebus_mcp.mock_client import MockErebusClient


def _terms(amount: int = 100, deadline_delta: int = 3600) -> OfferTerms:
    return OfferTerms(amount=amount, token="0xtoken", deadline=int(time.time()) + deadline_delta, memo_hash=0)


@pytest.fixture
def store_path(tmp_path):
    return tmp_path / "store.json"


@pytest.fixture
def clients(store_path):
    buyer = MockErebusClient(
        identity="0xbuyer", store_path=store_path, latency_seconds=0, spendable_notes=[100, 150]
    )
    seller = MockErebusClient(
        identity="0xseller", store_path=store_path, latency_seconds=0, spendable_notes=[]
    )
    return buyer, seller


def test_happy_path_end_to_end(clients, store_path):
    buyer, seller = clients

    async def run():
        handle = await buyer.open_channel("0xseller")
        seller_handle = await seller.open_channel("0xbuyer")
        # Each direction has its own handle. They must not converge, or the mock would hide
        # a client that was handed the other party's handle.
        assert handle != seller_handle

        offer_id = await buyer.propose_offer(handle, _terms(amount=100))
        state = await seller.read_channel_state(seller_handle)
        assert len(state.offers) == 1
        assert state.offers[0].offer_id == offer_id

        counter_id = await seller.counter_offer(seller_handle, offer_id, _terms(amount=150))
        state = await buyer.read_channel_state(handle)
        proposed_by_seller = [o for o in state.offers if o.offer_id == counter_id]
        assert proposed_by_seller[0].status.value == "proposed"
        original = [o for o in state.offers if o.offer_id == offer_id][0]
        assert original.status.value == "countered"  # doesn't revoke, just moves state

        receipt = await buyer.accept_and_settle(handle, counter_id)
        assert receipt.tx_hash.startswith("0x")

        state = await buyer.read_channel_state(handle)
        deal_id = str(state.offers[0].deal_id)
        grant = await buyer.grant_viewing_key(
            handle, deal_id, "0xauditor", int(time.time()) + 600
        )
        auditor = MockErebusClient(identity="0xauditor", store_path=store_path, latency_seconds=0)
        record = await auditor.reveal(grant)
        assert {offer.deal_id for offer in record.offers} == {int(deal_id)}
        assert record.settlement is not None

        # Historical grants remain readable. Construct the mock's legacy fixture directly;
        # new wire-v3 channels cannot export one.
        checksum = hashlib.sha256(handle.encode()).hexdigest()[:16]
        legacy_grant = ViewingKeyGrant(
            channel_id=handle,
            grantee="0xauditor",
            viewing_key=f"vk1.{handle}.{checksum}",
        )
        record = await auditor.reveal(legacy_grant)
        assert record.channel_id == handle
        assert sorted(record.participants) == sorted(["0xbuyer", "0xseller"])
        assert record.settlement is not None
        assert record.settlement.agreed_amount == 150
        assert record.settlement.paid_amount == 150
        assert record.settlement.consistency() is Consistency.CONSISTENT

    asyncio.run(run())


def test_same_pair_can_complete_two_deals(clients):
    buyer, seller = clients

    async def run():
        handle = await buyer.open_channel("0xseller")
        first = await seller.propose_offer(handle, _terms(amount=100))
        await buyer.accept_and_settle(handle, first)
        second = await seller.propose_offer(handle, _terms(amount=150))
        await buyer.accept_and_settle(handle, second)

        state = await buyer.read_channel_state(handle)
        assert [offer.status.value for offer in state.offers] == ["settled", "settled"]
        assert state.offers[0].deal_id != state.offers[1].deal_id

        grant = await buyer.grant_viewing_key(
            handle, str(state.offers[0].deal_id), "0xauditor", int(time.time()) + 600
        )
        auditor = MockErebusClient(
            identity="0xauditor", store_path=buyer._store_path, latency_seconds=0
        )
        disclosed = await auditor.reveal(grant)
        assert {offer.deal_id for offer in disclosed.offers} == {state.offers[0].deal_id}

        wrong = MockErebusClient(
            identity="0xwrong", store_path=buyer._store_path, latency_seconds=0
        )
        with pytest.raises(ErebusError) as wrong_recipient:
            await wrong.reveal(grant)
        assert wrong_recipient.value.code == SettlementErrorCode.INVALID_REQUEST

        with pytest.raises(ErebusError) as expired:
            await auditor.reveal(dataclasses.replace(grant, expires_at=int(time.time()) - 1))
        assert expired.value.code == SettlementErrorCode.INVALID_REQUEST

    asyncio.run(run())


def test_accepting_an_expired_offer_is_rejected(clients):
    buyer, seller = clients

    async def run():
        handle = await buyer.open_channel("0xseller")
        offer_id = await seller.propose_offer(handle, _terms(deadline_delta=-1))

        with pytest.raises(ErebusError) as excinfo:
            await buyer.accept_and_settle(handle, offer_id)
        assert excinfo.value.code == SettlementErrorCode.OFFER_EXPIRED

    asyncio.run(run())


def test_already_settled_channel_rejects_a_second_settlement(clients):
    buyer, seller = clients

    async def run():
        handle = await buyer.open_channel("0xseller")
        offer_id = await seller.propose_offer(handle, _terms())
        await buyer.accept_and_settle(handle, offer_id)

        with pytest.raises(ErebusError) as excinfo:
            await buyer.accept_and_settle(handle, offer_id)
        assert excinfo.value.code == SettlementErrorCode.ALREADY_SETTLED

    asyncio.run(run())


def test_acceptor_is_the_payer_and_exact_notes_are_consumed(clients):
    buyer, seller = clients

    async def run():
        handle = await buyer.open_channel("0xseller")
        seller_handle = await seller.open_channel("0xbuyer")
        buyer_offer = await buyer.propose_offer(handle, _terms(amount=100))

        with pytest.raises(ErebusError) as excinfo:
            await seller.accept_and_settle(seller_handle, buyer_offer)
        assert excinfo.value.code == SettlementErrorCode.INSUFFICIENT_NOTES

        seller_offer = await seller.counter_offer(seller_handle, buyer_offer, _terms(amount=150))
        receipt = await buyer.accept_and_settle(handle, seller_offer)
        assert receipt.selected_input == 150
        assert receipt.change == 0
        balance = await buyer.note_balance()
        assert balance.spendable == [100]

    asyncio.run(run())


def test_settlement_change(clients):
    buyer, seller = clients

    async def run():
        handle = await buyer.open_channel("0xseller")
        offer_id = await seller.propose_offer(handle, _terms(amount=120))
        receipt = await buyer.accept_and_settle(handle, offer_id)
        assert receipt.selected_input == 150
        assert receipt.change == 30
        balance = await buyer.note_balance()
        assert sorted(balance.spendable) == [30, 100]

    asyncio.run(run())


def test_accepting_your_own_offer_is_rejected(clients):
    buyer, _ = clients

    async def run():
        handle = await buyer.open_channel("0xseller")
        offer_id = await buyer.propose_offer(handle, _terms())

        with pytest.raises(ErebusError) as excinfo:
            await buyer.accept_and_settle(handle, offer_id)
        assert excinfo.value.code == SettlementErrorCode.NOT_YOUR_OFFER

    asyncio.run(run())


def test_countering_an_unknown_offer_is_rejected(clients):
    buyer, seller = clients

    async def run():
        await buyer.open_channel("0xseller")
        seller_handle = await seller.open_channel("0xbuyer")

        with pytest.raises(ErebusError) as excinfo:
            await seller.counter_offer(seller_handle, "0xbuyer:999", _terms())
        assert excinfo.value.code == SettlementErrorCode.OFFER_UNKNOWN

    asyncio.run(run())


@pytest.mark.parametrize(
    "code",
    [
        # One per ARCHITECTURE §4 group, per I0.2's acceptance criterion.
        SettlementErrorCode.SCREENING_REJECTED,  # terminal
        SettlementErrorCode.PROVER_UNAVAILABLE,  # retry may succeed
        SettlementErrorCode.PROOF_FAILED,  # opaque
        SettlementErrorCode.AMOUNT_MISMATCH,  # do not retry; see mock_client.py's module
        # docstring: not derivable from real input at this interface, so force_error is
        # the only way to exercise it.
    ],
)
def test_forced_failures_carry_the_code_and_retryable_flag(clients, code):
    buyer, _ = clients
    buyer.force_error(code)

    async def run():
        with pytest.raises(ErebusError) as excinfo:
            await buyer.open_channel("0xseller")
        assert excinfo.value.code == code
        assert isinstance(excinfo.value.retryable, bool)

    asyncio.run(run())

    # The hook clears after one use. The next call succeeds.
    async def run_again():
        handle = await buyer.open_channel("0xseller")
        assert handle.startswith("ch_")

    asyncio.run(run_again())
