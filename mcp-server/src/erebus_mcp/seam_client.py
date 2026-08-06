"""Real :class:`~erebus_mcp.interface.ErebusClient` over the ``sdk/py`` subprocess binding.

This adapter replaces :class:`~erebus_mcp.mock_client.MockErebusClient`. It runs the blocking
binding outside the event loop and converts dictionaries to §4 dataclasses. Hashing, felt
arithmetic, and salt encoding remain in Rust.

``erebus-cli`` blocks during preflight, a proof of about twenty seconds, fee estimation,
submission, and receipt polling. ``asyncio.to_thread`` keeps the MCP event loop available
while the subprocess waits.

``sdk/py`` raises a frozen ``ErebusError`` with a string code. The interface error uses
``SettlementErrorCode`` and is not frozen (see interface.py). This adapter converts between
them so agent code catches one type.
"""

from __future__ import annotations

import asyncio
from typing import Any

from erebus import ErebusError as SeamError
from erebus import Seam

from erebus_mcp.interface import (
    AgentId,
    ChannelHandle,
    ChannelState,
    DisclosedRecord,
    DisclosedSettlement,
    ErebusError,
    NoteBalance,
    Offer,
    OfferId,
    OfferStatus,
    OfferTerms,
    PublicKey,
    SettlementErrorCode,
    SettlementReceipt,
    ViewingKeyGrant,
)


def _translate(exc: SeamError) -> ErebusError:
    """Seam error to interface error.

    An unknown code becomes ``PROOF_FAILED``. Rust defines the code set, so an unknown code
    means that this adapter is stale. The Rust ``retryable`` value still controls retries.
    """
    try:
        code = SettlementErrorCode(exc.code)
    except ValueError:
        code = SettlementErrorCode.PROOF_FAILED
    return ErebusError(code=code, message=exc.message, retryable=exc.retryable)


def _terms(raw: dict[str, Any]) -> OfferTerms:
    return OfferTerms(
        amount=raw["amount"],
        token=raw["token"],
        deadline=raw["deadline"],
        memo_hash=raw["memo_hash"],
    )


def _offer(raw: dict[str, Any]) -> Offer:
    return Offer(
        offer_id=raw["offer_id"],
        channel_id=raw["channel_id"],
        proposer=raw["proposer"],
        terms=_terms(raw["terms"]),
        status=OfferStatus(raw["status"]),
        created_at=raw["created_at"],
        reply_to=raw.get("reply_to"),
    )


def _settlement(raw: dict[str, Any] | None) -> DisclosedSettlement | None:
    if not raw:
        return None
    return DisclosedSettlement(
        acceptance=raw["acceptance"],
        agreed_amount=raw["agreed_amount"],
        accepted_offer=raw.get("accepted_offer"),
        paid_amount=raw.get("paid_amount"),
    )


class SeamErebusClient:
    """Drives one identity through ``erebus-cli``.

    Use one instance per agent. Two identities in one process put both pool keys in the same
    heap. ``docs/ishita.md`` requires separate MCP servers.
    """

    def __init__(self, seam: Seam) -> None:
        self._seam = seam

    async def _run(self, fn: Any, *args: Any) -> dict[str, Any]:
        try:
            return await asyncio.to_thread(fn, *args)
        except SeamError as exc:
            raise _translate(exc) from exc

    async def open_channel(self, counterparty: AgentId) -> ChannelHandle:
        result = await self._run(self._seam.open_channel, counterparty)
        return result["channel_handle"]

    async def propose_offer(self, handle: ChannelHandle, terms: OfferTerms) -> OfferId:
        result = await self._run(self._seam.propose_offer, handle, _wire_terms(terms))
        return result["offer_id"]

    async def counter_offer(
        self, handle: ChannelHandle, reply_to: OfferId, terms: OfferTerms
    ) -> OfferId:
        result = await self._run(
            self._seam.counter_offer, handle, reply_to, _wire_terms(terms)
        )
        return result["offer_id"]

    async def read_channel_state(self, handle: ChannelHandle) -> ChannelState:
        result = await self._run(self._seam.read_channel_state, handle)
        # read_channel_state reports `settled` as a boolean. Only `reveal` reconstructs a
        # settlement object. Participants see the result in the accepted offer status.
        return ChannelState(
            offers=[_offer(o) for o in result.get("offers", [])],
            settlement=_settlement(result.get("settlement")),
        )

    async def note_balance(self) -> NoteBalance:
        result = await self._run(self._seam.balance)
        return NoteBalance(
            spendable=[int(amount) for amount in result.get("notes", [])],
            pending=[int(amount) for amount in result.get("pending", [])],
        )

    async def accept_and_settle(
        self, handle: ChannelHandle, offer_id: OfferId
    ) -> SettlementReceipt:
        result = await self._run(self._seam.accept_and_settle, handle, offer_id)
        return SettlementReceipt(
            tx_hash=result["tx_hash"],
            nullifiers=result.get("nullifiers", []),
            proved_at=result["proved_at"],
            offer_id=result.get("offer_id"),
        )

    async def grant_viewing_key(
        self, handle: ChannelHandle, grantee: PublicKey
    ) -> ViewingKeyGrant:
        result = await self._run(self._seam.grant_viewing_key, handle, grantee)
        # The bearer grant reads one relationship. Pass it through without logging or
        # widening its scope.
        return ViewingKeyGrant(
            channel_id=result["channel_id"],
            grantee=result["grantee"],
            viewing_key=result["viewing_key"],
        )

    async def reveal(self, viewing_key: ViewingKeyGrant) -> DisclosedRecord:
        grant = {
            "channel_id": viewing_key.channel_id,
            "grantee": viewing_key.grantee,
            "viewing_key": viewing_key.viewing_key,
        }
        result = await self._run(self._seam.reveal, grant)
        return DisclosedRecord(
            channel_id=result["channel_id"],
            participants=result.get("participants", []),
            offers=[_offer(o) for o in result.get("offers", [])],
            settlement=_settlement(result.get("settlement")),
        )

    async def shield(self, amount: int) -> SettlementReceipt:
        """Administrative funding, outside the seven interface methods.

        An identity needs a shielded note before settlement. Its first action set also
        registers it with the pool.
        """
        result = await self._run(self._seam.shield, str(amount))
        return SettlementReceipt(
            tx_hash=result["tx_hash"],
            nullifiers=result.get("nullifiers", []),
            proved_at=result["proved_at"],
            offer_id=result.get("offer_id"),
        )


def _wire_terms(terms: OfferTerms) -> dict[str, Any]:
    """Terms as the CLI wants them.

    Amounts and memo hashes cross as strings because JSON numbers are doubles. A 1e18 amount
    survives conversion, but a memo hash near 2^128 does not.
    ``deadline`` stays a number because the CLI parses it as one and would reject a string.
    """
    return {
        "amount": str(terms.amount),
        "token": terms.token,
        "deadline": terms.deadline,
        "memo_hash": str(terms.memo_hash),
    }
