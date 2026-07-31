"""``MockErebusClient`` — I0.2's mock of the frozen ``ErebusClient`` interface.

Stands in for the real binding (`sdk/py` -> `sdk/rs`, still on protocol 1 and only 2 of 7
methods deep — see the plan this was built from) until the shared integration pass makes a
swap possible. Mocks the *binding* surface: `interface.py`'s dataclasses and error shape are
what a caller sees either way.

State lives in a small JSON file, not just in memory, because the mock represents the
*shared* pool — two independent `MockErebusClient` instances (one per agent, each its own
process in the MCP-server case) read and write the same channel the way two agents both
read and write the same on-chain storage. Read-modify-write per call, atomic replacement on
write (temp file + rename) so a failed write can't truncate the record — the same shape as
`erebus-cli`'s real state directory, minus locking and crypto: this mock's callers act
sequentially (one agent's call completes before the other's starts), so there is no
concurrent-writer case to guard against.

One thing this mock cannot organically produce: ``AMOUNT_MISMATCH``. The frozen
``acceptAndSettle(handle, offerId)`` takes no separate payment argument, so there is no
caller-supplied value that could disagree with ``offer.terms.amount`` — the payment *is*
the offer's amount, always. Friction F23 is about a Rust-internal bug where the acceptance
record and the payment note were computed from two different places and drifted; a
correctly-written client (this one included) has only one source of truth. So
``AMOUNT_MISMATCH`` here is exercised only via ``force_error``, which is honest: it tests
that the error carries through the stack, not that some validation branch catches a bad
input, because no such input is expressible at this interface.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import secrets
import time
from dataclasses import asdict
from pathlib import Path

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

# Proof-bearing calls only (ARCHITECTURE / friction F7): everything that would be one
# `apply_actions` transaction on the real pool. `grant_viewing_key` is a local export with
# no chain transaction (ARCHITECTURE §3, "disclosure is the intentional exception"); the two
# reads don't write anything either.
_PROOF_BEARING_METHODS = frozenset(
    {"open_channel", "propose_offer", "counter_offer", "accept_and_settle"}
)

# Which SettlementErrorCode values are retryable, per ARCHITECTURE §4's grouping. Used to
# fill in `ErebusError.retryable` for codes force_error() injects.
_RETRYABLE_CODES = frozenset(
    {
        SettlementErrorCode.SCREENING_UNAVAILABLE,
        SettlementErrorCode.PROVER_UNAVAILABLE,
        SettlementErrorCode.PROOF_EXPIRED,
        SettlementErrorCode.SUBMIT_FAILED,
    }
)


class MockErebusClient:
    """In-memory-shaped fake of `ErebusClient`, backed by a shared JSON file.

    :param identity: this client's own address. Bound once, not passed per call — mirrors
        how a real client is constructed with a key file rather than taking an address on
        every method (ARCHITECTURE §4).
    :param store_path: path to the shared JSON store. Two clients pointed at the same path
        see the same channels.
    :param latency_seconds: simulated per-proof latency. Real figure is ~29s
        (friction F7); default here is fast for tests and iteration.
    """

    def __init__(
        self,
        identity: AgentId,
        store_path: Path,
        latency_seconds: float = 0.2,
    ) -> None:
        self._identity = identity
        self._store_path = store_path
        self._latency_seconds = latency_seconds
        self._forced_error: ErebusError | None = None
        if not self._store_path.exists():
            self._write_store({"channels": {}})

    # --- test-only failure injection ------------------------------------------------

    def force_error(self, code: SettlementErrorCode, message: str = "forced for testing") -> None:
        """The next call raises this error instead of doing anything, then clears.

        For codes this mock cannot derive from any real condition — `SCREENING_REJECTED`,
        `PROVER_UNAVAILABLE`, `PROOF_FAILED`, `AMOUNT_MISMATCH` — this is the only way to
        exercise the failure path, and that's honest: those are prover/screener/(buggy
        internal) conditions, not things a correct mock organically hits.
        """
        self._forced_error = ErebusError(code=code, message=message, retryable=code in _RETRYABLE_CODES)

    def _maybe_raise_forced(self) -> None:
        if self._forced_error is not None:
            error, self._forced_error = self._forced_error, None
            raise error

    # --- store -------------------------------------------------------------------------

    def _read_store(self) -> dict:
        return json.loads(self._store_path.read_text())

    def _write_store(self, store: dict) -> None:
        tmp = self._store_path.with_suffix(self._store_path.suffix + ".tmp")
        tmp.write_text(json.dumps(store))
        tmp.replace(self._store_path)

    @staticmethod
    def _channel_handle(a: AgentId, b: AgentId) -> ChannelHandle:
        # Deterministic and symmetric so both parties converge on the same handle whether
        # they open the channel first or second. Real handles are opaque random ids
        # (ARCHITECTURE §3); nothing in §4 requires that shape, only that it's a string.
        return "ch_" + "_".join(sorted([a, b]))

    def _get_channel(self, store: dict, handle: ChannelHandle) -> dict:
        channel = store["channels"].get(handle)
        if channel is None:
            # No code in SettlementErrorCode names "channel not found" — closest fit is
            # INDEX_CONFLICT ("the subchannel state is not what we thought"), which is what
            # this is: a client acting on a channel it never set up.
            raise ErebusError(
                code=SettlementErrorCode.INDEX_CONFLICT,
                message=f"no channel at handle {handle!r}",
                retryable=False,
            )
        return channel

    # --- serialization -------------------------------------------------------------

    @staticmethod
    def _offer_to_dict(offer: Offer) -> dict:
        d = asdict(offer)
        d["status"] = offer.status.value
        return d

    @staticmethod
    def _offer_from_dict(d: dict) -> Offer:
        terms = OfferTerms(**d["terms"])
        return Offer(
            offer_id=d["offer_id"],
            channel_id=d["channel_id"],
            proposer=d["proposer"],
            terms=terms,
            status=OfferStatus(d["status"]),
            created_at=d["created_at"],
            reply_to=d.get("reply_to"),
        )

    def _effective_status(self, offer: Offer, now: int) -> OfferStatus:
        """Deadlines are enforced client-side only (ARCHITECTURE §4: 'every transition here
        is enforced client-side and nowhere else') — computed at read time, not stored."""
        if offer.status in (OfferStatus.PROPOSED, OfferStatus.COUNTERED) and now > offer.terms.deadline:
            return OfferStatus.EXPIRED
        return offer.status

    # --- ErebusClient --------------------------------------------------------------

    async def open_channel(self, counterparty: AgentId) -> ChannelHandle:
        self._maybe_raise_forced()
        await asyncio.sleep(self._latency_seconds)
        handle = self._channel_handle(self._identity, counterparty)
        store = self._read_store()
        if handle not in store["channels"]:
            store["channels"][handle] = {
                "participants": sorted([self._identity, counterparty]),
                "offers": [],
                "settlement": None,
                "next_seq": {},
            }
            self._write_store(store)
        return handle

    async def propose_offer(self, handle: ChannelHandle, terms: OfferTerms) -> OfferId:
        self._maybe_raise_forced()
        await asyncio.sleep(self._latency_seconds)
        store = self._read_store()
        channel = self._get_channel(store, handle)
        if channel["settlement"] is not None:
            raise ErebusError(
                code=SettlementErrorCode.INDEX_CONFLICT,
                message="channel already settled — one subchannel is one deal",
                retryable=False,
            )
        seq = channel["next_seq"].get(self._identity, 0)
        # OfferId keyed by (author, seq), not a bare index — friction F22: a bare index
        # collides across the two directions, because each side numbers its own subchannel
        # from zero.
        offer_id = f"{self._identity}:{seq}"
        channel["next_seq"][self._identity] = seq + 1
        offer = Offer(
            offer_id=offer_id,
            channel_id=handle,
            proposer=self._identity,
            terms=terms,
            status=OfferStatus.PROPOSED,
            created_at=int(time.time()),
        )
        channel["offers"].append(self._offer_to_dict(offer))
        self._write_store(store)
        return offer_id

    async def counter_offer(self, handle: ChannelHandle, reply_to: OfferId, terms: OfferTerms) -> OfferId:
        self._maybe_raise_forced()
        await asyncio.sleep(self._latency_seconds)
        store = self._read_store()
        channel = self._get_channel(store, handle)
        if channel["settlement"] is not None:
            raise ErebusError(
                code=SettlementErrorCode.INDEX_CONFLICT,
                message="channel already settled — one subchannel is one deal",
                retryable=False,
            )
        offers = [self._offer_from_dict(d) for d in channel["offers"]]
        target = next((o for o in offers if o.offer_id == reply_to), None)
        if target is None:
            raise ErebusError(
                code=SettlementErrorCode.OFFER_UNKNOWN, message=f"no offer {reply_to!r}", retryable=False
            )
        # A reply always crosses the table (F22) — countering your own offer isn't a thing.
        if target.proposer == self._identity:
            raise ErebusError(
                code=SettlementErrorCode.NOT_YOUR_OFFER,
                message="cannot counter your own offer",
                retryable=False,
            )
        now = int(time.time())
        target_status = self._effective_status(target, now)
        if target_status == OfferStatus.EXPIRED:
            raise ErebusError(code=SettlementErrorCode.OFFER_EXPIRED, message=reply_to, retryable=False)
        if target_status not in (OfferStatus.PROPOSED, OfferStatus.COUNTERED):
            raise ErebusError(
                code=SettlementErrorCode.INDEX_CONFLICT,
                message=f"offer {reply_to!r} is not open (status={target_status.value})",
                retryable=False,
            )

        seq = channel["next_seq"].get(self._identity, 0)
        offer_id = f"{self._identity}:{seq}"
        channel["next_seq"][self._identity] = seq + 1
        new_offer = Offer(
            offer_id=offer_id,
            channel_id=handle,
            proposer=self._identity,
            terms=terms,
            status=OfferStatus.PROPOSED,
            created_at=now,
            reply_to=reply_to,
        )
        # Countering proposes; it does not revoke (ARCHITECTURE §4). The replied-to offer
        # moves to `countered` for observability, but stays acceptable.
        updated_offers = []
        for d in channel["offers"]:
            if d["offer_id"] == reply_to:
                d = dict(d)
                d["status"] = OfferStatus.COUNTERED.value
            updated_offers.append(d)
        updated_offers.append(self._offer_to_dict(new_offer))
        channel["offers"] = updated_offers
        self._write_store(store)
        return offer_id

    async def read_channel_state(self, handle: ChannelHandle) -> ChannelState:
        self._maybe_raise_forced()
        store = self._read_store()
        channel = self._get_channel(store, handle)
        now = int(time.time())
        offers = []
        for d in channel["offers"]:
            offer = self._offer_from_dict(d)
            offers.append(
                Offer(
                    offer_id=offer.offer_id,
                    channel_id=offer.channel_id,
                    proposer=offer.proposer,
                    terms=offer.terms,
                    status=self._effective_status(offer, now),
                    created_at=offer.created_at,
                    reply_to=offer.reply_to,
                )
            )
        settlement = self._settlement_from_dict(channel["settlement"]) if channel["settlement"] else None
        return ChannelState(offers=offers, settlement=settlement)

    async def accept_and_settle(self, handle: ChannelHandle, offer_id: OfferId) -> SettlementReceipt:
        self._maybe_raise_forced()
        await asyncio.sleep(self._latency_seconds)
        store = self._read_store()
        channel = self._get_channel(store, handle)
        if channel["settlement"] is not None:
            raise ErebusError(code=SettlementErrorCode.ALREADY_SETTLED, message=handle, retryable=False)
        offers = [self._offer_from_dict(d) for d in channel["offers"]]
        target = next((o for o in offers if o.offer_id == offer_id), None)
        if target is None:
            raise ErebusError(
                code=SettlementErrorCode.OFFER_UNKNOWN, message=f"no offer {offer_id!r}", retryable=False
            )
        if target.proposer == self._identity:
            raise ErebusError(
                code=SettlementErrorCode.NOT_YOUR_OFFER,
                message="cannot accept your own offer",
                retryable=False,
            )
        now = int(time.time())
        if self._effective_status(target, now) == OfferStatus.EXPIRED:
            raise ErebusError(code=SettlementErrorCode.OFFER_EXPIRED, message=offer_id, retryable=False)

        # Acceptance and payment in one action set, one proof (ARCHITECTURE §4) — there is
        # no accepted-but-not-settled state, so this is the only place `settlement` is set.
        receipt = SettlementReceipt(
            offer_id=offer_id,
            tx_hash="0x" + secrets.token_hex(32),
            nullifiers=["0x" + secrets.token_hex(32)],
            proved_at=now,
        )
        channel["settlement"] = {
            "acceptance": offer_id,
            "accepted_offer": target.reply_to,
            "agreed_amount": target.terms.amount,
            "paid_amount": target.terms.amount,  # only source of truth — see module docstring
            "receipt": {
                "offer_id": receipt.offer_id,
                "tx_hash": receipt.tx_hash,
                "nullifiers": receipt.nullifiers,
                "proved_at": receipt.proved_at,
            },
        }
        self._write_store(store)
        return receipt

    async def grant_viewing_key(self, handle: ChannelHandle, grantee: PublicKey) -> ViewingKeyGrant:
        self._maybe_raise_forced()
        store = self._read_store()
        self._get_channel(store, handle)  # raises INDEX_CONFLICT if unknown
        checksum = hashlib.sha256(handle.encode()).hexdigest()[:16]
        return ViewingKeyGrant(channel_id=handle, grantee=grantee, viewing_key=f"vk1.{handle}.{checksum}")

    async def reveal(self, viewing_key: ViewingKeyGrant) -> DisclosedRecord:
        self._maybe_raise_forced()
        parts = viewing_key.viewing_key.split(".")
        if len(parts) != 3 or parts[0] != "vk1":
            raise ErebusError(
                code=SettlementErrorCode.INVALID_REQUEST, message="malformed viewing key grant", retryable=False
            )
        _, handle, checksum = parts
        if checksum != hashlib.sha256(handle.encode()).hexdigest()[:16]:
            raise ErebusError(
                code=SettlementErrorCode.INVALID_REQUEST,
                message="viewing key grant failed its checksum",
                retryable=False,
            )
        store = self._read_store()
        channel = self._get_channel(store, handle)
        offers = [self._offer_from_dict(d) for d in channel["offers"]]
        settlement = self._settlement_from_dict(channel["settlement"]) if channel["settlement"] else None
        return DisclosedRecord(
            channel_id=handle,
            participants=list(channel["participants"]),
            offers=offers,
            settlement=settlement,
        )

    @staticmethod
    def _settlement_from_dict(d: dict) -> DisclosedSettlement:
        return DisclosedSettlement(
            acceptance=d["acceptance"],
            accepted_offer=d.get("accepted_offer"),
            agreed_amount=d["agreed_amount"],
            paid_amount=d.get("paid_amount"),
        )
