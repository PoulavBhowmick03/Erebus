"""The Python mirror of ARCHITECTURE.md §4 — the frozen ``ErebusClient`` contract.

Plays the role ``sdk/ts/src/interface.ts`` plays for TypeScript: a transcription of the
normative §4 block, not an implementation. Signatures here are taken strictly from that
block. They are **not** the same as ``sdk/py/src/erebus/_seam.py``'s real ``Seam`` methods,
which speak protocol 2 (extra params: ``key_file``, ``token``, ``channel_index``, ...) —
that gap is documented and deliberate (CLAUDE.md, ARCHITECTURE §4). Anything implementing
``ErebusClient`` is constructed with its own identity once; methods never take an address.

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
    memo_hash: int  # 128-bit — hash of off-chain detail, not the detail itself (F19)


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
        """Read-side check (friction F23): a record may have been written by a client
        that never enforced AMOUNT_MISMATCH on write, so a viewer still has to verify."""
        return self.paid_amount is None or self.paid_amount == self.agreed_amount


@dataclass(frozen=True)
class ChannelState:
    """§4's ``readChannelState`` return type. Not defined in the data-model block, so this
    is the minimal reading: what a participant who already knows the channel and the
    counterparty needs. Distinct from ``DisclosedRecord``, which is a third party's
    reconstruction from a viewing key and therefore also carries ``channel_id`` and
    ``participants`` that the caller wouldn't already have."""

    offers: list[Offer]
    settlement: DisclosedSettlement | None = None


@dataclass(frozen=True)
class DisclosedRecord:
    channel_id: ChannelHandle
    participants: list[AgentId]
    offers: list[Offer]
    settlement: DisclosedSettlement | None = None


class SettlementErrorCode(str, Enum):
    """Frozen 2026-07-30 (ARCHITECTURE §4). Grouped by what the caller should do, which is
    the only distinction agent-layer code needs to branch on."""

    # Do not retry. The offer is wrong; build a different one.
    OFFER_EXPIRED = "OFFER_EXPIRED"
    OFFER_UNKNOWN = "OFFER_UNKNOWN"
    ALREADY_SETTLED = "ALREADY_SETTLED"
    NOT_YOUR_OFFER = "NOT_YOUR_OFFER"
    AMOUNT_MISMATCH = "AMOUNT_MISMATCH"  # acceptance and payment disagree — friction F23
    INSUFFICIENT_NOTES = "INSUFFICIENT_NOTES"
    INDEX_CONFLICT = "INDEX_CONFLICT"  # contiguity/write-once — subchannel isn't what we thought

    # Retry may succeed.
    SCREENING_UNAVAILABLE = "SCREENING_UNAVAILABLE"
    PROVER_UNAVAILABLE = "PROVER_UNAVAILABLE"
    PROOF_EXPIRED = "PROOF_EXPIRED"  # proof validity is 450 blocks
    SUBMIT_FAILED = "SUBMIT_FAILED"

    # Terminal for this counterparty or this deposit.
    SCREENING_REJECTED = "SCREENING_REJECTED"

    # The prover refused and told us nothing — genuinely opaque, not a lazy mapping (F20).
    PROOF_FAILED = "PROOF_FAILED"

    # Seam-level: fail before any protocol code runs, but arrive through the same envelope.
    INVALID_REQUEST = "INVALID_REQUEST"
    IDENTITY_UNAVAILABLE = "IDENTITY_UNAVAILABLE"


@dataclass
class ErebusError(Exception):
    """A structured failure. ``retryable`` is the only field agent logic should branch on.

    Same *fields* as ``sdk/py/src/erebus/_seam.py``'s ``ErebusError`` so a future swap to
    the real seam doesn't change any call site that catches this — deliberately **not**
    ``frozen=True`` like that one, though. Verified earlier in this session:
    ``sdk/py``'s frozen version throws ``dataclasses.FrozenInstanceError`` from inside
    ``pytest.raises``' `__exit__` when it tries to attach a traceback, because a frozen
    dataclass blocks every attribute set including the ones the exception protocol needs.
    Immutability isn't part of the shape any call site actually depends on, so it's not
    worth carrying that landmine over.
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

    async def accept_and_settle(
        self, handle: ChannelHandle, offer_id: OfferId
    ) -> SettlementReceipt:
        """Accept an offer AND settle atomically. One state transition."""
        ...

    async def grant_viewing_key(
        self, handle: ChannelHandle, grantee: PublicKey
    ) -> ViewingKeyGrant:
        """Export a self-contained bearer viewing grant for a third party."""
        ...

    async def reveal(self, viewing_key: ViewingKeyGrant) -> DisclosedRecord:
        """Reconstruct from chain data. No grantor-local handle state is needed."""
        ...
