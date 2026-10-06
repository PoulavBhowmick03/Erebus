"""Procurement scenario: a buyer's requisition, a supplier's catalog, and an auditor's review.

Unit prices turn into the amounts the offer policies already negotiate. The auditor checks a
disclosed deal against the buyer's authorized ceiling. No MCP or I/O.
"""

from __future__ import annotations

from dataclasses import dataclass

from erebus_mcp.interface import AgentId, Consistency, DisclosedRecord

from erebus_agents.policy import BuyerPolicy, SellerPolicy


@dataclass(frozen=True)
class Requisition:
    item: str
    quantity: int
    max_unit_price: int

    def __post_init__(self) -> None:
        if self.quantity <= 0 or self.max_unit_price <= 0:
            raise ValueError("quantity and max_unit_price must be positive")

    @property
    def ceiling(self) -> int:
        return self.quantity * self.max_unit_price


@dataclass(frozen=True)
class Catalog:
    item: str
    floor_unit_price: int

    def __post_init__(self) -> None:
        if self.floor_unit_price <= 0:
            raise ValueError("floor_unit_price must be positive")

    def reserve(self, quantity: int) -> int:
        return quantity * self.floor_unit_price


def buyer_policy(identity: AgentId, requisition: Requisition, *, deadline_seconds: int = 3600,
                 max_rounds: int = 3) -> BuyerPolicy:
    return BuyerPolicy(identity, requisition.ceiling, deadline_seconds, max_rounds)


def supplier_policy(identity: AgentId, catalog: Catalog, requisition: Requisition, *,
                    deadline_seconds: int = 3600, max_rounds: int = 3) -> SellerPolicy:
    if catalog.item != requisition.item:
        raise ValueError("catalog does not list the requested item")
    return SellerPolicy(identity, catalog.reserve(requisition.quantity), deadline_seconds, max_rounds)


@dataclass(frozen=True)
class AuditReview:
    compliant: bool
    reasons: tuple[str, ...]


def review_disclosure(record: DisclosedRecord, requisition: Requisition) -> AuditReview:
    """A disclosed deal passes only if it settled, paid what was agreed, and stayed under the ceiling.

    Missing evidence is a failure, never a pass.
    """
    settlement = record.settlement
    if settlement is None:
        return AuditReview(False, ("no settlement in the disclosed record",))
    reasons = []
    if settlement.consistency() is not Consistency.CONSISTENT:
        reasons.append(f"paid amount is {settlement.consistency().value} with the agreed amount")
    paid = settlement.paid_amount
    if paid is None or paid > requisition.ceiling:
        reasons.append(f"paid {paid} against a ceiling of {requisition.ceiling}")
    return AuditReview(not reasons, tuple(reasons))
