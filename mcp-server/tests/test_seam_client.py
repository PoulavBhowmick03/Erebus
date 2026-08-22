"""Tests for the subprocess-to-§4 adapter.

This layer converts field names, error codes, and 128-bit integers. A stub binding drives
shape and translation tests without chain access.

The stub returns trimmed payloads from live ``erebus-cli`` runs on 2026-07-31.
"""

from __future__ import annotations

import asyncio
from typing import Any

import pytest
from erebus import ErebusError as SeamError

from erebus_mcp.interface import (
    ErebusError,
    OfferStatus,
    OfferTerms,
    SettlementErrorCode,
    ViewingKeyGrant,
)
from erebus_mcp.seam_client import SeamErebusClient, _wire_terms

CHANNEL = "ch_" + "a8" * 32
OFFER = {
    "offer_id": f"{CHANNEL}:us:0",
    "deal_id": "18446744073709551615",
    "channel_id": CHANNEL,
    "proposer": "0x32bb394a452d4bdd24c4c0cdd76ea9d7c140b9a28287a9b81dcb25703bdc805",
    "status": "countered",
    "created_at": 1785479790,
    "reply_to": None,
    "terms": {
        "amount": 500000000000000000,
        "token": "0x4718f5a",
        "deadline": 1785566189,
        "memo_hash": 4660,
    },
}


class StubSeam:
    """Records what it was asked and answers with recorded CLI payloads."""

    def __init__(self, **answers: Any) -> None:
        self.answers = answers
        self.calls: list[tuple[str, tuple[Any, ...]]] = []

    def __getattr__(self, name: str) -> Any:
        def method(*args: Any) -> Any:
            self.calls.append((name, args))
            answer = self.answers[name]
            if isinstance(answer, Exception):
                raise answer
            return answer

        return method


def run(coro: Any) -> Any:
    return asyncio.run(coro)


def test_offers_map_onto_the_interface_dataclasses() -> None:
    seam = StubSeam(read_channel_state={"channel_id": CHANNEL, "offers": [OFFER], "settled": False})
    state = run(SeamErebusClient(seam).read_channel_state(CHANNEL))

    assert len(state.offers) == 1
    offer = state.offers[0]
    assert offer.status is OfferStatus.COUNTERED
    assert offer.deal_id == 18_446_744_073_709_551_615
    assert offer.terms.amount == 500000000000000000
    assert offer.reply_to is None


def test_balance_amount_strings_map_to_note_denominations() -> None:
    seam = StubSeam(balance={"notes": ["100", "150"], "total": "250", "pending": ["25"]})
    balance = run(SeamErebusClient(seam).note_balance())

    assert balance.spendable == [100, 150]
    assert balance.pending == [25]
    assert balance.total == 250


def test_read_channel_state_carries_no_settlement_object() -> None:
    """The CLI reports `settled` as a boolean here and only reconstructs a settlement in
    `reveal`. A participant still sees the outcome through the accepted offer's status, so
    this is a real difference between the two calls rather than a dropped field."""
    seam = StubSeam(read_channel_state={"channel_id": CHANNEL, "offers": [], "settled": True})
    state = run(SeamErebusClient(seam).read_channel_state(CHANNEL))

    assert state.settlement is None


def test_accept_and_settle_parses_selected_input_and_change() -> None:
    seam = StubSeam(
        accept_and_settle={
            "offer_id": f"{CHANNEL}:them:0",
            "tx_hash": "0xabc",
            "nullifiers": ["0xdef"],
            "proved_at": 13095252,
            "selected_input": "5000000000000000000",
            "change": "2000000000000000000",
        }
    )
    receipt = run(SeamErebusClient(seam).accept_and_settle(CHANNEL, f"{CHANNEL}:them:0"))

    assert receipt.selected_input == 5_000_000_000_000_000_000
    assert receipt.change == 2_000_000_000_000_000_000


def test_accept_and_settle_leaves_missing_change_fields_as_none() -> None:
    seam = StubSeam(
        accept_and_settle={
            "offer_id": f"{CHANNEL}:them:0",
            "tx_hash": "0xabc",
            "nullifiers": ["0xdef"],
            "proved_at": 13095252,
        }
    )
    receipt = run(SeamErebusClient(seam).accept_and_settle(CHANNEL, f"{CHANNEL}:them:0"))

    assert receipt.selected_input is None
    assert receipt.change is None


def test_deal_grant_arguments_and_capsule_pass_through_unchanged() -> None:
    capsule = {"version": 3, "ciphertext": [1, 2, 3]}
    seam = StubSeam(
        grant_viewing_key={
            "channel_id": CHANNEL,
            "grantee": "0xa0d17",
            "deal_id": "18446744073709551615",
            "expires_at": 1_800_000_000,
            "viewing_key": capsule,
        }
    )
    grant = run(
        SeamErebusClient(seam).grant_viewing_key(
            CHANNEL, "18446744073709551615", "0xa0d17", 1_800_000_000
        )
    )

    assert seam.calls == [
        (
            "grant_viewing_key",
            (CHANNEL, "18446744073709551615", "0xa0d17", 1_800_000_000),
        )
    ]
    assert grant.viewing_key is capsule
    assert grant.deal_id == "18446744073709551615"


def test_reveal_reconstructs_the_settlement() -> None:
    seam = StubSeam(
        reveal={
            "channel_id": CHANNEL,
            "participants": ["0xa11ce", "0xb0b"],
            "offers": [OFFER],
            "settlement": {
                "acceptance": f"{CHANNEL}:us:1",
                "accepted_offer": f"{CHANNEL}:them:0",
                "agreed_amount": 1000000000000000000,
                "paid_amount": 1000000000000000000,
            },
        }
    )
    grant = ViewingKeyGrant(channel_id=CHANNEL, grantee="0xa0d17", viewing_key="vk_opaque")
    record = run(SeamErebusClient(seam).reveal(grant))

    assert record.participants == ["0xa11ce", "0xb0b"]
    assert record.settlement is not None
    assert record.settlement.is_consistent()


def test_settlement_amounts_arrive_as_strings_above_the_json_number_range() -> None:
    """F40. A settlement above u64::MAX cannot cross the seam as a JSON number, so the CLI
    sends decimal strings. The fixtures above keep the legacy numeric form on purpose: both
    shapes must parse, or an archived disclosure stops reading."""
    huge = 30000000000000000000  # 30 STRK, above u64::MAX
    seam = StubSeam(
        reveal={
            "channel_id": CHANNEL,
            "participants": [],
            "offers": [],
            "settlement": {
                "acceptance": f"{CHANNEL}:us:1",
                "accepted_offer": None,
                "agreed_amount": str(huge),
                "paid_amount": str(huge),
            },
        }
    )
    grant = ViewingKeyGrant(channel_id=CHANNEL, grantee="0x0", viewing_key="vk")
    record = run(SeamErebusClient(seam).reveal(grant))

    assert record.settlement is not None
    assert record.settlement.agreed_amount == huge
    assert record.settlement.paid_amount == huge
    assert record.settlement.is_consistent()


def test_an_absent_payment_note_is_not_a_zero_payment() -> None:
    """None means no payment note was found; 0 would mean one was found and paid nothing."""
    seam = StubSeam(
        reveal={
            "channel_id": CHANNEL,
            "participants": [],
            "offers": [],
            "settlement": {"acceptance": f"{CHANNEL}:us:1", "agreed_amount": "5", "paid_amount": None},
        }
    )
    grant = ViewingKeyGrant(channel_id=CHANNEL, grantee="0x0", viewing_key="vk")
    record = run(SeamErebusClient(seam).reveal(grant))

    assert record.settlement is not None
    assert record.settlement.paid_amount is None


def test_a_disagreeing_settlement_is_reported_not_hidden() -> None:
    """Atomicity guarantees both legs land, not that they describe the same trade (F23).
    A reader must still check, so the adapter must carry both numbers through unchanged."""
    seam = StubSeam(
        reveal={
            "channel_id": CHANNEL,
            "participants": [],
            "offers": [],
            "settlement": {
                "acceptance": f"{CHANNEL}:us:1",
                "agreed_amount": 1000000000000000000,
                "paid_amount": 1,
            },
        }
    )
    grant = ViewingKeyGrant(channel_id=CHANNEL, grantee="0x0", viewing_key="vk")
    record = run(SeamErebusClient(seam).reveal(grant))

    assert record.settlement is not None
    assert not record.settlement.is_consistent()


def test_seam_errors_become_interface_errors() -> None:
    seam = StubSeam(
        accept_and_settle=SeamError(code="ALREADY_SETTLED", message="terminal", retryable=False)
    )

    with pytest.raises(ErebusError) as caught:
        run(SeamErebusClient(seam).accept_and_settle(CHANNEL, "x"))

    assert caught.value.code is SettlementErrorCode.ALREADY_SETTLED
    assert caught.value.retryable is False


def test_an_unknown_code_degrades_instead_of_crashing() -> None:
    """A code this enum has not caught up with means the file is stale, not that the agent
    should take an exception it cannot catch. The retryable flag still carries the only
    decision the agent needs."""
    seam = StubSeam(
        open_channel=SeamError(code="SOMETHING_NEW", message="from a newer cli", retryable=True)
    )

    with pytest.raises(ErebusError) as caught:
        run(SeamErebusClient(seam).open_channel("0xb0b"))

    assert caught.value.code is SettlementErrorCode.PROOF_FAILED
    assert caught.value.retryable is True


def test_wide_integers_cross_as_strings() -> None:
    """JSON numbers are doubles. A 1e18 amount survives that; a memo hash near 2^128 does
    not, and would arrive at the CLI silently rounded."""
    memo = (1 << 128) - 1
    wire = _wire_terms(OfferTerms(amount=10**18, token="0x7042", deadline=1785566189, memo_hash=memo))

    assert wire["amount"] == "1000000000000000000"
    assert wire["memo_hash"] == str(memo)
    assert isinstance(wire["deadline"], int), "the CLI parses deadline as a number, not a string"


def test_the_grant_is_passed_through_without_reshaping() -> None:
    """The disclosure format belongs to Rust. Reading into the grant here would make this
    file a second opinion on it, and a wrong one the day the format changes."""
    seam = StubSeam(reveal={"channel_id": CHANNEL, "participants": [], "offers": []})
    grant = ViewingKeyGrant(channel_id=CHANNEL, grantee="0xa0d17", viewing_key="vk_opaque")
    run(SeamErebusClient(seam).reveal(grant))

    _, args = seam.calls[0]
    assert args[0] == {
        "channel_id": CHANNEL,
        "grantee": "0xa0d17",
        "viewing_key": "vk_opaque",
    }
