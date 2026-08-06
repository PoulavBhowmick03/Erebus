"""I0.2 acceptance tests for every mock method and error-code group.

Each test uses `asyncio.run()` instead of adding pytest-asyncio for sequential calls.
"""

from __future__ import annotations

import asyncio
import time

import pytest
from erebus_mcp.interface import ErebusError, OfferTerms, SettlementErrorCode
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
        assert handle == await seller.open_channel("0xbuyer")  # both converge on one handle

        offer_id = await buyer.propose_offer(handle, _terms(amount=100))
        state = await seller.read_channel_state(handle)
        assert len(state.offers) == 1
        assert state.offers[0].offer_id == offer_id

        counter_id = await seller.counter_offer(handle, offer_id, _terms(amount=150))
        state = await buyer.read_channel_state(handle)
        proposed_by_seller = [o for o in state.offers if o.offer_id == counter_id]
        assert proposed_by_seller[0].status.value == "proposed"
        original = [o for o in state.offers if o.offer_id == offer_id][0]
        assert original.status.value == "countered"  # doesn't revoke, just moves state

        receipt = await buyer.accept_and_settle(handle, counter_id)
        assert receipt.tx_hash.startswith("0x")

        grant = await buyer.grant_viewing_key(handle, "0xauditor")
        auditor = MockErebusClient(identity="0xauditor", store_path=store_path, latency_seconds=0)
        record = await auditor.reveal(grant)
        assert record.channel_id == handle
        assert sorted(record.participants) == sorted(["0xbuyer", "0xseller"])
        assert record.settlement is not None
        assert record.settlement.agreed_amount == 150
        assert record.settlement.paid_amount == 150
        assert record.settlement.is_consistent()

    asyncio.run(run())


def test_post_settle_write_is_rejected(clients):
    buyer, seller = clients

    async def run():
        handle = await buyer.open_channel("0xseller")
        offer_id = await seller.propose_offer(handle, _terms())
        await buyer.accept_and_settle(handle, offer_id)

        with pytest.raises(ErebusError) as excinfo:
            await seller.propose_offer(handle, _terms())
        assert excinfo.value.code == SettlementErrorCode.INDEX_CONFLICT

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
        buyer_offer = await buyer.propose_offer(handle, _terms(amount=100))

        with pytest.raises(ErebusError) as excinfo:
            await seller.accept_and_settle(handle, buyer_offer)
        assert excinfo.value.code == SettlementErrorCode.INSUFFICIENT_NOTES

        seller_offer = await seller.counter_offer(handle, buyer_offer, _terms(amount=150))
        await buyer.accept_and_settle(handle, seller_offer)
        balance = await buyer.note_balance()
        assert balance.spendable == [100]

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
        handle = await buyer.open_channel("0xseller")

        with pytest.raises(ErebusError) as excinfo:
            await seller.counter_offer(handle, "0xbuyer:999", _terms())
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
