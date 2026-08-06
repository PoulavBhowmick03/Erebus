"""Python mirror of the frozen ``ErebusClient`` contract in ARCHITECTURE.md §4.

This file transcribes the normative §4 block. It does not implement the protocol. ``sdk/py``
carries the calls over the protocol-2 subprocess binding. Each ``ErebusClient`` has one
identity, so methods do not accept a caller address.

``token`` is not a parameter anywhere. It travels inside ``OfferTerms``.
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from typing import Protocol

ChannelHandle = str
OfferId = str
AgentId = str
PublicKey = str


class OfferStatus(str, Enum):
    PROPOSED = "proposed"
    COUNTERED = "countered"
    EXPIRED = "expired"
    SETTLED = "settled"


@dataclass(frozen=True)
class OfferTerms:
    amount: int  # token base units
    token: str  # ERC-20 address
    deadline: int  # unix seconds
    memo_hash: int  # 128-bit hash of off-chain detail, not the detail itself (F19)


@dataclass(frozen=True)
class Offer:
    offer_id: OfferId
    channel_id: ChannelHandle
    proposer: AgentId
    terms: OfferTerms
    status: OfferStatus
    created_at: int
    reply_to: OfferId | None = None


@dataclass(frozen=True)
class SettlementReceipt:
    tx_hash: str
    nullifiers: list[str]
    proved_at: int
    offer_id: OfferId | None = None  # absent only for the administrative shield helper


@dataclass(frozen=True)
class NoteBalance:
    """Private note denominations available to the payer.

    Settlement consumes an exact subset and creates no change note. ``total`` and
    :meth:`can_pay` answer different questions. One 1 STRK note cannot pay 0.7 STRK exactly.
    """

    spendable: list[int]
    pending: list[int]

    @property
    def total(self) -> int:
        return sum(self.spendable)

    def can_pay(self, amount: int) -> bool:
        """Whether some exact subset of spendable notes sums to ``amount``."""
        if amount <= 0:
            return False
        # Match sdk/rs/src/client.rs::select_exact_notes and its resource caps. The planner
        # must not claim that Rust can pay an amount that its bounded search rejects.
        reachable = {0}
        for note in self.spendable[:256]:
            additions = {
                subtotal + note
                for subtotal in reachable
                if subtotal + note <= amount and subtotal + note not in reachable
            }
            reachable |= additions
            if amount in reachable:
                return True
            if len(reachable) >= 100_000:
                return False
        return False


@dataclass(frozen=True)
class ViewingKeyGrant:
    channel_id: ChannelHandle
    grantee: PublicKey  # metadata in MVP v1; the grant remains a bearer secret
    viewing_key: str  # versioned and checksummed


@dataclass(frozen=True)
class DisclosedSettlement:
    acceptance: OfferId
    agreed_amount: int
    accepted_offer: OfferId | None = None
    paid_amount: int | None = None

    def is_consistent(self) -> bool:
        """Checks records from clients that did not enforce AMOUNT_MISMATCH (F23)."""
        return self.paid_amount is None or self.paid_amount == self.agreed_amount


@dataclass(frozen=True)
class ChannelState:
    """Minimal §4 return type for ``readChannelState``.

    A participant already knows the channel and counterparty. ``DisclosedRecord`` also
    contains ``channel_id`` and ``participants`` for a third-party viewer.
    """

    offers: list[Offer]
    settlement: DisclosedSettlement | None = None


@dataclass(frozen=True)
class DisclosedRecord:
    channel_id: ChannelHandle
    participants: list[AgentId]
    offers: list[Offer]
    settlement: DisclosedSettlement | None = None


class SettlementErrorCode(str, Enum):
    """Error codes frozen on 2026-07-30 and grouped by caller action (ARCHITECTURE §4)."""

    # Do not retry. The offer is wrong; build a different one.
    OFFER_EXPIRED = "OFFER_EXPIRED"
    OFFER_UNKNOWN = "OFFER_UNKNOWN"
    ALREADY_SETTLED = "ALREADY_SETTLED"
    NOT_YOUR_OFFER = "NOT_YOUR_OFFER"
    AMOUNT_MISMATCH = "AMOUNT_MISMATCH"  # acceptance and payment differ; friction F23
    INSUFFICIENT_NOTES = "INSUFFICIENT_NOTES"
    INDEX_CONFLICT = "INDEX_CONFLICT"  # unexpected contiguity or write-once state

    # Retry may succeed.
    SCREENING_UNAVAILABLE = "SCREENING_UNAVAILABLE"
    PROVER_UNAVAILABLE = "PROVER_UNAVAILABLE"
    PROOF_EXPIRED = "PROOF_EXPIRED"  # proof validity is 450 blocks
    SUBMIT_FAILED = "SUBMIT_FAILED"

    # Terminal for this counterparty or this deposit.
    SCREENING_REJECTED = "SCREENING_REJECTED"

    # The prover returned no detail (F20).
    PROOF_FAILED = "PROOF_FAILED"

    # Seam-level: fail before any protocol code runs, but arrive through the same envelope.
    INVALID_REQUEST = "INVALID_REQUEST"
    IDENTITY_UNAVAILABLE = "IDENTITY_UNAVAILABLE"


@dataclass
class ErebusError(Exception):
    """A structured failure. ``retryable`` is the only field agent logic should branch on.

    This has the same fields as ``sdk/py/src/erebus/_seam.py``'s ``ErebusError``, so callers
    can use either backend. It does not use ``frozen=True``. Testing found that
    ``sdk/py``'s frozen version throws ``dataclasses.FrozenInstanceError`` from inside
    ``pytest.raises`` when it attaches a traceback. The exception protocol needs to set
    attributes.
    """

    code: SettlementErrorCode
    message: str
    retryable: bool

    def __str__(self) -> str:
        return f"{self.code.value}: {self.message}"


class ErebusClient(Protocol):
    """Mirrors ARCHITECTURE.md §4's normative TypeScript block, method for method."""

    async def open_channel(self, counterparty: AgentId) -> ChannelHandle:
        """Establish a private channel with a counterparty."""
        ...

    async def propose_offer(self, handle: ChannelHandle, terms: OfferTerms) -> OfferId:
        """Write a structured offer into the channel."""
        ...

    async def counter_offer(
        self, handle: ChannelHandle, reply_to: OfferId, terms: OfferTerms
    ) -> OfferId:
        """Write a counter-offer referencing a prior offer."""
        ...

    async def read_channel_state(self, handle: ChannelHandle) -> ChannelState:
        """Read all offer state visible to this party."""
        ...

    async def note_balance(self) -> NoteBalance:
        """Return exact spendable and pending note denominations for payment planning."""
        ...

    async def accept_and_settle(
        self, handle: ChannelHandle, offer_id: OfferId
    ) -> SettlementReceipt:
        """Accept an offer and pay it from this caller's notes, atomically."""
        ...

    async def grant_viewing_key(
        self, handle: ChannelHandle, grantee: PublicKey
    ) -> ViewingKeyGrant:
        """Export a self-contained bearer viewing grant for a third party."""
        ...

    async def reveal(self, viewing_key: ViewingKeyGrant) -> DisclosedRecord:
        """Reconstruct from chain data. No grantor-local handle state is needed."""
        ...
