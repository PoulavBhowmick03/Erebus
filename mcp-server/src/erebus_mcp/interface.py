"""Python mirror of the frozen ``ErebusClient`` contract in ARCHITECTURE.md §4.

This file transcribes the normative §4 block. It does not implement the protocol. ``sdk/py``
carries the calls over the protocol-4 subprocess binding. Each ``ErebusClient`` has one
identity, so methods do not accept a caller address.

``token`` is not a parameter anywhere. It travels inside ``OfferTerms``.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Protocol

ChannelHandle = str
OfferId = str
AgentId = str
PublicKey = str
OperationId = str


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
    deal_id: int  # u64 on wire; serialized to agent-facing JSON as a decimal string
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
    selected_input: int | None = None  # total value of notes spent; absent for shielding
    change: int | None = None  # returned to the payer; 0 if exact, absent for shielding


@dataclass(frozen=True)
class NoteBalance:
    spendable: list[int]
    pending: list[int]

    @property
    def total(self) -> int:
        return sum(self.spendable)


@dataclass(frozen=True)
class ViewingKeyGrant:
    channel_id: ChannelHandle
    grantee: AgentId
    viewing_key: dict[str, Any] | str = field(repr=False)  # Rust-owned encrypted capsule
    deal_id: str | None = None
    expires_at: int | None = None


class Consistency(str, Enum):
    """Whether a settlement's paid amount matches what was agreed (F23).

    ``UNKNOWN`` is not a synonym for ``CONSISTENT``: a caller that could not obtain
    ``paid_amount`` has not verified anything, and reporting that as consistent would be a
    tool answering "yes" when it means "I could not check" (roadmap 9.2).
    """

    CONSISTENT = "consistent"
    INCONSISTENT = "inconsistent"
    UNKNOWN = "unknown"


@dataclass(frozen=True)
class DisclosedSettlement:
    acceptance: OfferId
    agreed_amount: int
    accepted_offer: OfferId | None = None
    paid_amount: int | None = None

    def consistency(self) -> Consistency:
        """Checks records from clients that did not enforce AMOUNT_MISMATCH (F23).

        Fails closed to ``UNKNOWN`` when ``paid_amount`` is missing, rather than treating
        absent evidence as a pass.
        """
        if self.paid_amount is None:
            return Consistency.UNKNOWN
        if self.paid_amount == self.agreed_amount:
            return Consistency.CONSISTENT
        return Consistency.INCONSISTENT


@dataclass(frozen=True)
class ChannelState:
    """Minimal §4 return type for ``readChannelState``.

    A participant already knows the channel and counterparty. ``DisclosedRecord`` also
    contains ``channel_id`` and ``participants`` for a third-party viewer.
    """

    offers: list[Offer]
    settlements: list[DisclosedSettlement] = field(default_factory=list)

    @property
    def settlement(self) -> DisclosedSettlement | None:
        """Latest settlement for source compatibility; protocol 4 serializes the list."""
        return self.settlements[-1] if self.settlements else None


@dataclass(frozen=True)
class DisclosedRecord:
    channel_id: ChannelHandle
    participants: list[AgentId]
    offers: list[Offer]
    settlement: DisclosedSettlement | None = None


@dataclass(frozen=True)
class DoctorCheck:
    name: str
    status: str  # "pass" | "warn" | "fail" | "skipped"
    detail: str
    repair: str | None = None  # present only when status is not "pass"


@dataclass(frozen=True)
class DoctorReport:
    ready: bool  # whether a proof-bearing write can be attempted
    checks: list[DoctorCheck]
    repairs: list[str]


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
    OPERATION_CONFLICT = "OPERATION_CONFLICT"
    INSUFFICIENT_ALLOWANCE = "INSUFFICIENT_ALLOWANCE"
    INSUFFICIENT_BALANCE = "INSUFFICIENT_BALANCE"

    # Retry may succeed.
    SCREENING_UNAVAILABLE = "SCREENING_UNAVAILABLE"
    PROVER_UNAVAILABLE = "PROVER_UNAVAILABLE"
    PROOF_EXPIRED = "PROOF_EXPIRED"  # proof validity is 450 blocks
    SUBMIT_FAILED = "SUBMIT_FAILED"
    RECONCILIATION_REQUIRED = "RECONCILIATION_REQUIRED"

    # Terminal for this counterparty or this deposit.
    SCREENING_REJECTED = "SCREENING_REJECTED"

    # The prover returned no detail (F20).
    PROOF_FAILED = "PROOF_FAILED"

    # Seam-level: fail before any protocol code runs, but arrive through the same envelope.
    INVALID_REQUEST = "INVALID_REQUEST"
    IDENTITY_UNAVAILABLE = "IDENTITY_UNAVAILABLE"

    # MCP-layer: fail before the seam is ever called. Added 2026-08-21 for spending caps
    # (roadmap 9.1). sdk/rs never produces this code; it is synthesized entirely in
    # erebus_mcp/tools.py, below the agent, before accept_and_settle reaches the seam.
    SPENDING_LIMIT_EXCEEDED = "SPENDING_LIMIT_EXCEEDED"


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

    async def open_channel(
        self, operation_id: OperationId, counterparty: AgentId
    ) -> ChannelHandle:
        """Establish a private channel with a counterparty."""
        ...

    async def propose_offer(
        self, operation_id: OperationId, handle: ChannelHandle, terms: OfferTerms
    ) -> OfferId:
        """Write a structured offer into the channel."""
        ...

    async def counter_offer(
        self,
        operation_id: OperationId,
        handle: ChannelHandle,
        reply_to: OfferId,
        terms: OfferTerms,
    ) -> OfferId:
        """Write a counter-offer referencing a prior offer."""
        ...

    async def read_channel_state(self, handle: ChannelHandle) -> ChannelState:
        """Read all offer state visible to this party."""
        ...

    async def note_balance(self) -> NoteBalance:
        """Return exact spendable and pending note denominations for payment planning."""
        ...

    async def doctor(self) -> DoctorReport:
        """Pre-flight inspection of configuration and chain state. Read-only, always safe."""
        ...

    async def accept_and_settle(
        self, operation_id: OperationId, handle: ChannelHandle, offer_id: OfferId
    ) -> SettlementReceipt:
        """Accept an offer and pay it from this caller's notes, atomically."""
        ...

    async def grant_viewing_key(
        self, handle: ChannelHandle, deal_id: str, grantee: AgentId, expires_at: int
    ) -> ViewingKeyGrant:
        """Encrypt one wire-v3 deal capability to a registered recipient."""
        ...

    async def reveal(self, viewing_key: ViewingKeyGrant) -> DisclosedRecord:
        """Reconstruct from chain data. No grantor-local handle state is needed."""
        ...

    async def reconcile(self) -> list[dict[str, Any]]:
        """Classify durable operations without changing state."""
        ...

    async def resume_operation(self, operation_id: OperationId) -> dict[str, Any]:
        """Explicitly resume one classified operation."""
        ...

    async def rebuild_state(self) -> dict[str, Any]:
        """Add missing channel records reconstructed from chain data."""
        ...
