"""The real :class:`~erebus_mcp.interface.ErebusClient`, backed by ``sdk/py``'s subprocess seam.

Swaps in for :class:`~erebus_mcp.mock_client.MockErebusClient`. Everything here is
adaptation: run the blocking seam off the event loop, and reshape its dicts into the §4
dataclasses. No hashing, no felt arithmetic, no salt encoding — those live in Rust, where
there is exactly one implementation to be wrong.

**Why the thread.** ``erebus-cli`` is a blocking subprocess and a write is a preflight, a
proof of about twenty seconds, a fee estimate, a submission and a receipt wait. Calling it
directly from a coroutine would stall the MCP server's event loop for that whole period, so
a second tool call could not even be parsed until the first settled. ``asyncio.to_thread``
keeps the server responsive; the subprocess is doing the waiting anyway.

**Two error types on purpose.** ``sdk/py`` raises a frozen ``ErebusError`` carrying a plain
string code. The interface's ``ErebusError`` carries a ``SettlementErrorCode`` and is
deliberately not frozen (a frozen exception breaks ``pytest.raises``, see interface.py).
Translating at this boundary keeps agent-layer code catching one type.
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

    An unrecognised code becomes ``PROOF_FAILED`` rather than raising, because a client that
    crashes on an unknown code turns a recoverable protocol failure into an outage. The
    Rust side is the authority on the code set; a mismatch means this file is stale, and
    the ``retryable`` flag it sent still carries the only decision an agent needs.
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

    One instance per agent. Two identities in one process would put both pool keys in the
    same heap, which is the arrangement ``docs/ishita.md`` rejected when it settled on two
    MCP servers rather than one multi-tenant one.
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
        # read_channel_state reports `settled` as a boolean and carries no settlement
        # object; only `reveal` reconstructs one. A participant can still see the outcome
        # because the accepted offer's status is `settled`.
        return ChannelState(
            offers=[_offer(o) for o in result.get("offers", [])],
            settlement=_settlement(result.get("settlement")),
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
        # The grant is a bearer secret: whoever holds it can read this one relationship.
        # It is carried, never logged, and never widened.
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

        An identity needs one shielded note before it can settle, and its first action set
        is also what registers it with the pool.
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

    Amounts and the memo hash cross as strings because they are 128-bit values and JSON
    numbers are doubles; a 1e18 amount survives that, a memo hash near 2^128 does not.
    ``deadline`` stays a number because the CLI parses it as one and would reject a string.
    """
    return {
        "amount": str(terms.amount),
        "token": terms.token,
        "deadline": terms.deadline,
        "memo_hash": str(terms.memo_hash),
    }
