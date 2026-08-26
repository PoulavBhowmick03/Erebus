"""I0.2 mock of the frozen ``ErebusClient`` interface.

This replaces the real `sdk/py` to `sdk/rs` binding in deterministic tests and policy
rehearsals. Callers see the dataclasses and error shape from `interface.py`.

The mock stores shared pool state in a JSON file. Two clients read and write the same
channels. Each call reads, changes, and atomically replaces the file. The real CLI uses a
similar directory with locking and cryptography. Mock callers run sequentially, so the mock
does not lock concurrent writers.

The mock cannot produce ``AMOUNT_MISMATCH`` from a normal call. The frozen
``acceptAndSettle(handle, offerId)`` takes no separate payment argument, so there is no
caller value that can differ from ``offer.terms.amount``. Friction F23 describes a Rust bug
where the record and payment used different sources. ``force_error`` injects this code to
test error transport because the interface cannot express mismatched input.
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
    AgentId,
    ChannelHandle,
    ChannelState,
    DisclosedRecord,
    DisclosedSettlement,
    DoctorCheck,
    DoctorReport,
    ErebusError,
    NoteBalance,
    Offer,
    OfferId,
    OfferStatus,
    OfferTerms,
    SettlementErrorCode,
    SettlementReceipt,
    ViewingKeyGrant,
)

# Proof-bearing calls only (ARCHITECTURE / friction F7): each set becomes one
# `apply_actions` transaction on the real pool. Reads and deal-grant creation do not write.
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

    :param identity: this client's address. Bound once and not passed per call. This mirrors
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
        spendable_notes: list[int] | None = None,
        pending_notes: list[int] | None = None,
    ) -> None:
        self._identity = identity
        self._store_path = store_path
        self._latency_seconds = latency_seconds
        self._spendable_notes = list(
            [1_000_000_000_000_000_000] if spendable_notes is None else spendable_notes
        )
        self._pending_notes = list([] if pending_notes is None else pending_notes)
        self._forced_error: ErebusError | None = None
        if not self._store_path.exists():
            self._write_store({"channels": {}, "handles": {}})

    # --- test-only failure injection ------------------------------------------------

    def force_error(self, code: SettlementErrorCode, message: str = "forced for testing") -> None:
        """Makes the next call raise one error and then clears it.

        Use this for prover, screener, and internal errors that normal mock input cannot
        produce: `SCREENING_REJECTED`, `PROVER_UNAVAILABLE`, `PROOF_FAILED`, and
        `AMOUNT_MISMATCH`.
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
        # Real handles belong to one local state store. The two directions must not converge
        # on one value, or the mock hides production bugs that pass one party's handle to the
        # other party's client.
        digest = hashlib.sha256(f"mock-handle:{a}:{b}".encode()).hexdigest()
        return "ch_" + digest

    @staticmethod
    def _pair_id(a: AgentId, b: AgentId) -> str:
        return "|".join(sorted([a, b]))

    def _get_channel(self, store: dict, handle: ChannelHandle, *, require_owner: bool = True) -> dict:
        binding = store.get("handles", {}).get(handle)
        if binding is None or (require_owner and binding["owner"] != self._identity):
            channel = None
        else:
            channel = store["channels"].get(binding["pair"])
        if channel is None:
            # SettlementErrorCode has no channel-not-found value. INDEX_CONFLICT represents
            # a client whose local channel state does not match shared state.
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
            deal_id=d.get("deal_id", 0),
            channel_id=d["channel_id"],
            proposer=d["proposer"],
            terms=terms,
            status=OfferStatus(d["status"]),
            created_at=d["created_at"],
            reply_to=d.get("reply_to"),
        )

    def _effective_status(self, offer: Offer, now: int) -> OfferStatus:
        """Computes the client-enforced deadline status at read time (ARCHITECTURE §4)."""
        if offer.status in (OfferStatus.PROPOSED, OfferStatus.COUNTERED) and now > offer.terms.deadline:
            return OfferStatus.EXPIRED
        return offer.status

    # --- ErebusClient --------------------------------------------------------------

    async def open_channel(
        self, operation_id: str, counterparty: AgentId | None = None
    ) -> ChannelHandle:
        # Direct mock callers predating seam protocol 4 pass only the counterparty. The MCP
        # path always supplies both values; production idempotency lives in Rust.
        if counterparty is None:
            counterparty = operation_id
        self._maybe_raise_forced()
        await asyncio.sleep(self._latency_seconds)
        handle = self._channel_handle(self._identity, counterparty)
        store = self._read_store()
        pair = self._pair_id(self._identity, counterparty)
        if pair not in store["channels"]:
            store["channels"][pair] = {
                "participants": sorted([self._identity, counterparty]),
                "offers": [],
                "settlement": None,
                "next_seq": {},
            }
        store.setdefault("handles", {})[handle] = {"pair": pair, "owner": self._identity}
        self._write_store(store)
        return handle

    async def propose_offer(
        self,
        operation_id: str,
        handle: ChannelHandle | OfferTerms,
        terms: OfferTerms | None = None,
    ) -> OfferId:
        if terms is None:
            terms = handle  # type: ignore[assignment]
            handle = operation_id
        self._maybe_raise_forced()
        await asyncio.sleep(self._latency_seconds)
        store = self._read_store()
        channel = self._get_channel(store, handle, require_owner=False)
        seq = channel["next_seq"].get(self._identity, 0)
        # OfferId uses (author, seq), not a bare index. Under friction F22, a bare index
        # collides across the two directions, because each side numbers its own subchannel
        # from zero.
        offer_id = f"{self._identity}:{seq}"
        deal_id = int.from_bytes(hashlib.sha256(offer_id.encode()).digest()[:8], "big") or 1
        channel["next_seq"][self._identity] = seq + 1
        offer = Offer(
            offer_id=offer_id,
            deal_id=deal_id,
            channel_id=handle,
            proposer=self._identity,
            terms=terms,
            status=OfferStatus.PROPOSED,
            created_at=int(time.time()),
        )
        channel["offers"].append(self._offer_to_dict(offer))
        self._write_store(store)
        return offer_id

    async def counter_offer(
        self,
        operation_id: str,
        handle: ChannelHandle,
        reply_to: OfferId | OfferTerms,
        terms: OfferTerms | None = None,
    ) -> OfferId:
        if terms is None:
            terms = reply_to  # type: ignore[assignment]
            reply_to = handle
            handle = operation_id
        self._maybe_raise_forced()
        await asyncio.sleep(self._latency_seconds)
        store = self._read_store()
        channel = self._get_channel(store, handle)
        offers = [self._offer_from_dict(d) for d in channel["offers"]]
        target = next((o for o in offers if o.offer_id == reply_to), None)
        if target is None:
            raise ErebusError(
                code=SettlementErrorCode.OFFER_UNKNOWN, message=f"no offer {reply_to!r}", retryable=False
            )
        # A reply crosses directions (F22), so a client cannot counter its own offer.
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
            deal_id=target.deal_id,
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
                    deal_id=offer.deal_id,
                    channel_id=offer.channel_id,
                    proposer=offer.proposer,
                    terms=offer.terms,
                    status=self._effective_status(offer, now),
                    created_at=offer.created_at,
                    reply_to=offer.reply_to,
                )
            )
        settlements = (
            [self._settlement_from_dict(channel["settlement"])]
            if channel["settlement"]
            else []
        )
        return ChannelState(offers=offers, settlements=settlements)

    async def note_balance(self) -> NoteBalance:
        self._maybe_raise_forced()
        return NoteBalance(
            spendable=sorted(self._spendable_notes, reverse=True),
            pending=sorted(self._pending_notes, reverse=True),
        )

    async def doctor(self) -> DoctorReport:
        self._maybe_raise_forced()
        return DoctorReport(
            ready=True,
            checks=[
                DoctorCheck(
                    name="mock_backend",
                    status="pass",
                    detail="mock backend has no chain state to inspect",
                )
            ],
            repairs=[],
        )

    async def accept_and_settle(
        self, operation_id: str, handle: ChannelHandle, offer_id: OfferId | None = None
    ) -> SettlementReceipt:
        if offer_id is None:
            offer_id = handle
            handle = operation_id
        self._maybe_raise_forced()
        await asyncio.sleep(self._latency_seconds)
        store = self._read_store()
        channel = self._get_channel(store, handle)
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
        target_status = self._effective_status(target, now)
        if target_status == OfferStatus.EXPIRED:
            raise ErebusError(code=SettlementErrorCode.OFFER_EXPIRED, message=offer_id, retryable=False)
        if target_status not in (OfferStatus.PROPOSED, OfferStatus.COUNTERED):
            raise ErebusError(
                code=SettlementErrorCode.ALREADY_SETTLED,
                message=offer_id,
                retryable=False,
            )

        selected = _select_notes(self._spendable_notes, target.terms.amount)
        if selected is None:
            held = sorted(self._spendable_notes, reverse=True)
            rendered = ", ".join(str(amount) for amount in held) or "none"
            raise ErebusError(
                code=SettlementErrorCode.INSUFFICIENT_NOTES,
                message=(
                    f"total spendable value {sum(held)} is below {target.terms.amount}; "
                    f"holding {len(held)} note(s) ({rendered})"
                ),
                retryable=False,
            )
        indices, change = selected

        # The acceptor is the payer. Consuming its notes here is the piece the original mock
        # omitted, which let a seller-side settlement look valid even though the real Rust
        # client would spend the seller's funds.
        for index in sorted(indices, reverse=True):
            del self._spendable_notes[index]
        if change > 0:
            self._spendable_notes.append(change)

        # Acceptance and payment share one action set and proof (ARCHITECTURE §4). There is
        # no accepted-but-unsettled state.
        receipt = SettlementReceipt(
            offer_id=offer_id,
            tx_hash="0x" + secrets.token_hex(32),
            nullifiers=["0x" + secrets.token_hex(32)],
            proved_at=now,
            selected_input=target.terms.amount + change,
            change=change,
        )
        channel["offers"] = [
            {**offer, "status": OfferStatus.SETTLED.value}
            if offer["offer_id"] == offer_id
            else offer
            for offer in channel["offers"]
        ]
        channel["settlement"] = {
            "acceptance": offer_id,
            "accepted_offer": target.reply_to,
            "agreed_amount": target.terms.amount,
            "paid_amount": target.terms.amount,  # only source of truth; see module docstring
            "receipt": {
                "offer_id": receipt.offer_id,
                "tx_hash": receipt.tx_hash,
                "nullifiers": receipt.nullifiers,
                "proved_at": receipt.proved_at,
            },
        }
        self._write_store(store)
        return receipt

    async def grant_viewing_key(
        self, handle: ChannelHandle, deal_id: str, grantee: AgentId, expires_at: int
    ) -> ViewingKeyGrant:
        self._maybe_raise_forced()
        store = self._read_store()
        channel = self._get_channel(store, handle)
        if expires_at <= int(time.time()):
            raise ErebusError(
                code=SettlementErrorCode.INVALID_REQUEST,
                message="viewing grant expiry must be in the future",
                retryable=False,
            )
        if not any(str(offer["deal_id"]) == deal_id for offer in channel["offers"]):
            raise ErebusError(
                code=SettlementErrorCode.INVALID_REQUEST,
                message=f"deal {deal_id} does not exist in this channel",
                retryable=False,
            )
        checksum = hashlib.sha256(
            f"{handle}:{deal_id}:{grantee}:{expires_at}".encode()
        ).hexdigest()[:16]
        return ViewingKeyGrant(
            channel_id=handle,
            grantee=grantee,
            deal_id=deal_id,
            expires_at=expires_at,
            viewing_key={"version": 3, "mock_checksum": checksum},
        )

    async def reveal(self, viewing_key: ViewingKeyGrant) -> DisclosedRecord:
        self._maybe_raise_forced()
        if isinstance(viewing_key.viewing_key, dict):
            if viewing_key.grantee != self._identity:
                raise ErebusError(
                    code=SettlementErrorCode.INVALID_REQUEST,
                    message="viewing grant belongs to a different recipient",
                    retryable=False,
                )
            if viewing_key.expires_at is None or int(time.time()) > viewing_key.expires_at:
                raise ErebusError(
                    code=SettlementErrorCode.INVALID_REQUEST,
                    message="viewing grant expired",
                    retryable=False,
                )
            if viewing_key.deal_id is None:
                raise ErebusError(
                    code=SettlementErrorCode.INVALID_REQUEST,
                    message="viewing grant omitted its deal id",
                    retryable=False,
                )
            checksum = hashlib.sha256(
                f"{viewing_key.channel_id}:{viewing_key.deal_id}:{viewing_key.grantee}:"
                f"{viewing_key.expires_at}".encode()
            ).hexdigest()[:16]
            if viewing_key.viewing_key.get("mock_checksum") != checksum:
                raise ErebusError(
                    code=SettlementErrorCode.INVALID_REQUEST,
                    message="viewing grant authentication failed",
                    retryable=False,
                )
            store = self._read_store()
            # A grant recipient never owns the granter's handle. The recipient binding
            # and expiry checked above are the access control, not handle ownership.
            channel = self._get_channel(store, viewing_key.channel_id, require_owner=False)
            offers = [
                self._offer_from_dict(offer)
                for offer in channel["offers"]
                if str(offer["deal_id"]) == viewing_key.deal_id
            ]
            settlement = (
                self._settlement_from_dict(channel["settlement"])
                if channel["settlement"]
                and any(
                    offer.offer_id == channel["settlement"]["acceptance"] for offer in offers
                )
                else None
            )
            return DisclosedRecord(
                channel_id=viewing_key.channel_id,
                participants=list(channel["participants"]),
                offers=offers,
                settlement=settlement,
            )
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
        # Same as the v3 path: the grant's checksum is the access check, not ownership.
        channel = self._get_channel(store, handle, require_owner=False)
        offers = [self._offer_from_dict(d) for d in channel["offers"]]
        settlement = self._settlement_from_dict(channel["settlement"]) if channel["settlement"] else None
        return DisclosedRecord(
            channel_id=handle,
            participants=list(channel["participants"]),
            offers=offers,
            settlement=settlement,
        )

    async def reconcile(self) -> list[dict]:
        """The mock has no durable Rust journal."""
        return []

    async def resume_operation(self, operation_id: str) -> dict:
        raise ErebusError(
            code=SettlementErrorCode.INVALID_REQUEST,
            message=f"mock backend has no durable operation {operation_id}",
            retryable=False,
        )

    async def rebuild_state(self) -> dict:
        return {"scanned": 0, "rebuilt": 0, "retained": 0}

    @staticmethod
    def _settlement_from_dict(d: dict) -> DisclosedSettlement:
        return DisclosedSettlement(
            acceptance=d["acceptance"],
            accepted_offer=d.get("accepted_offer"),
            agreed_amount=d["agreed_amount"],
            paid_amount=d.get("paid_amount"),
        )


def _select_notes(notes: list[int], target: int) -> tuple[list[int], int] | None:
    """Returns indices covering ``target`` plus the surplus, or ``None`` if short."""
    if target <= 0:
        return [], 0
    if sum(notes) < target:
        return None
    reachable: dict[int, list[int]] = {0: []}
    for index, note in enumerate(notes):
        additions: dict[int, list[int]] = {}
        for subtotal, selected in reachable.items():
            candidate = subtotal + note
            if candidate in reachable or candidate in additions:
                continue
            additions[candidate] = [*selected, index]
        reachable.update(additions)
    covering = [total for total in reachable if total >= target]
    best_total = min(covering)
    return reachable[best_total], best_total - target
