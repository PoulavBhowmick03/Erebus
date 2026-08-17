"""Protocol tools and payment-planning helpers over an `ErebusClient`.

Tools accept primitive fields instead of an `OfferTerms` object. Their schemas show
`amount`, `token`, `deadline`, and `memo_hash` directly.

With installed `mcp==2.0.0`, an uncaught `ErebusError` does not produce structured output.
`Tool.run` catches it and wraps it as
`ToolError(f"Error executing tool {name}: {e}")`, and the server's `_handle_call_tool`
catches that and returns `CallToolResult(content=[TextContent(text=str(e))], is_error=True)`
with a prose string. Each tool catches `ErebusError` and returns a JSON object with an `ok`
flag. The payload carries the structured `SettlementErrorCode`.
"""

from __future__ import annotations

import asyncio
import time
from typing import Any

from mcp.server import MCPServer

from erebus_mcp.config import SettlementRole
from erebus_mcp.interface import (
    ErebusClient,
    ErebusError,
    NoteBalance,
    OfferTerms,
    SettlementErrorCode,
    ViewingKeyGrant,
)


#: Poll interval. Writes take about 20 s, and each read needs one RPC round-trip per note.
POLL_INTERVAL_SECONDS = 5.0


def register_tools(
    server: MCPServer, client: ErebusClient, settlement_role: SettlementRole
) -> None:
    """Registers the tools against one identity-bound client."""

    async def _call(coro: Any) -> dict[str, Any]:
        try:
            result = await coro
        except ErebusError as e:
            return {"ok": False, "error": {"code": e.code.value, "message": e.message, "retryable": e.retryable}}
        return {"ok": True, "result": result}

    def _error(code: SettlementErrorCode, message: str) -> dict[str, Any]:
        return {
            "ok": False,
            "error": {"code": code.value, "message": message, "retryable": False},
        }

    def _balance_json(balance: NoteBalance) -> dict[str, Any]:
        return {
            "spendable_notes": [str(n) for n in balance.spendable],
            "total": str(balance.total),
            "pending_notes": [str(n) for n in balance.pending],
        }

    def _parse_memo_hash(value: int | str) -> int:
        """Accepts a memo hash as a hex string or an int, returns the int the seam wants.

        JSON has one numeric type and it is an IEEE-754 double, so a client that emits a
        large `memo_hash` as a bare number loses precision above 2**53 and Pydantic rejects
        what arrives. The top of the documented 128-bit range was therefore unreachable by a
        conforming JSON-RPC caller. A hex string carries all 128 bits exactly. See F37.
        """
        if isinstance(value, str):
            text = value.strip()
            if not text:
                raise ValueError("memo_hash is empty")
            parsed = int(text, 16)
        else:
            parsed = value
        if parsed < 0:
            raise ValueError(f"memo_hash must not be negative, got {parsed}")
        if parsed >= 1 << 128:
            raise ValueError(
                f"memo_hash must fit 128 bits, got a value of {parsed.bit_length()} bits"
            )
        return parsed

    def _memo_hash_or_error(value: int | str) -> tuple[int | None, dict[str, Any] | None]:
        try:
            return _parse_memo_hash(value), None
        except ValueError as e:
            return None, _error(SettlementErrorCode.INVALID_REQUEST, str(e))

    def _parse_amount(value: int | str) -> int:
        """Same precision problem as memo_hash: a base-unit amount routinely exceeds
        2**53, so a bare JSON number can silently round. Accepts a decimal string or int."""
        if isinstance(value, str):
            text = value.strip()
            if not text:
                raise ValueError("amount is empty")
            parsed = int(text)
        else:
            parsed = value
        if parsed <= 0:
            raise ValueError(f"amount must be positive, got {parsed}")
        return parsed

    def _amount_or_error(value: int | str) -> tuple[int | None, dict[str, Any] | None]:
        try:
            return _parse_amount(value), None
        except ValueError as e:
            return None, _error(SettlementErrorCode.INVALID_REQUEST, str(e))

    async def _require_payable(amount: int) -> dict[str, Any] | None:
        outcome = await _call(client.note_balance())
        if not outcome["ok"]:
            return outcome
        balance = outcome["result"]
        if 0 < amount <= balance.total:
            return None
        pending = (
            f"; pending notes: {balance.pending}" if balance.pending else ""
        )
        return _error(
            SettlementErrorCode.INSUFFICIENT_NOTES,
            "this payer cannot name or accept an unpayable amount: "
            f"total spendable value {balance.total} is below {amount}{pending}. "
            "Use get_note_balance before negotiating.",
        )

    @server.tool()
    async def open_channel(counterparty: str) -> dict[str, Any]:
        """Establish a channel with a counterparty (their address). Returns a
        channel_handle to use in every other call for this relationship.

        This call itself is not private: it writes the counterparty's address to public
        chain calldata (F38). Negotiation content and settlement amounts are hidden once
        the channel is open; that you opened one with this counterparty is not."""
        outcome = await _call(client.open_channel(counterparty))
        if outcome["ok"]:
            outcome["result"] = {"channel_handle": outcome["result"]}
        return outcome

    @server.tool()
    async def propose_offer(
        channel_handle: str, amount: int | str, token: str, deadline: int, memo_hash: int | str
    ) -> dict[str, Any]:
        """Write a new offer into the channel: amount as a decimal string in token base
        units (JSON numbers lose precision above 2**53, well under a real STRK amount),
        token address, deadline as a unix timestamp, memo_hash as a hex string such as
        "0x9f2c..." holding the low 128 bits of your own hash.

        Truncating a digest to 128 bits leaves about 2**64 collision resistance, so
        memo_hash commits to off-chain detail and is not a substitute for the document
        itself.

        If this server is configured as payer, the amount must not exceed its total
        spendable note value (call get_note_balance first); settlement returns any excess
        as change. Payee offers are asks and do not spend the payee's notes."""
        parsed_amount, failure = _amount_or_error(amount)
        if failure is not None:
            return failure
        memo, failure = _memo_hash_or_error(memo_hash)
        if failure is not None:
            return failure
        if settlement_role is SettlementRole.PAYER:
            failure = await _require_payable(parsed_amount)
            if failure is not None:
                return failure
        terms = OfferTerms(amount=parsed_amount, token=token, deadline=deadline, memo_hash=memo)
        outcome = await _call(client.propose_offer(channel_handle, terms))
        if outcome["ok"]:
            outcome["result"] = {"offer_id": outcome["result"]}
        return outcome

    @server.tool()
    async def counter_offer(
        channel_handle: str,
        reply_to: str,
        amount: int | str,
        token: str,
        deadline: int,
        memo_hash: int | str,
    ) -> dict[str, Any]:
        """Write a counter-offer replying to a specific offer_id from the other party.
        Does not withdraw the offer you're replying to. It stays acceptable until it
        expires or is settled. Use a short deadline instead of trying to retract. A payer
        may only counter at or below its total spendable note value; a payee uses this to
        leave the final seller-authored offer that the payer will accept.

        Pass amount as a decimal string and memo_hash as a hex string, as in
        propose_offer."""
        parsed_amount, failure = _amount_or_error(amount)
        if failure is not None:
            return failure
        memo, failure = _memo_hash_or_error(memo_hash)
        if failure is not None:
            return failure
        if settlement_role is SettlementRole.PAYER:
            failure = await _require_payable(parsed_amount)
            if failure is not None:
                return failure
        terms = OfferTerms(amount=parsed_amount, token=token, deadline=deadline, memo_hash=memo)
        outcome = await _call(client.counter_offer(channel_handle, reply_to, terms))
        if outcome["ok"]:
            outcome["result"] = {"offer_id": outcome["result"]}
        return outcome

    @server.tool()
    async def get_note_balance() -> dict[str, Any]:
        """Read this identity's private payment denominations without moving value.

        Any positive amount up to `total` is payable — settlement returns the excess as
        change. A payer must call this before naming or accepting a price. Pending notes
        are already on-chain but cannot be used until they reach the proving block."""
        outcome = await _call(client.note_balance())
        if outcome["ok"]:
            outcome["result"] = _balance_json(outcome["result"])
        return outcome

    @server.tool()
    async def doctor() -> dict[str, Any]:
        """Pre-flight inspection of this identity's configuration and chain state. Always
        safe to call: every check is a read. `ready: false` means a write will fail right
        now; each unhealthy check carries a `repair` string naming one direct action."""
        outcome = await _call(client.doctor())
        if outcome["ok"]:
            report = outcome["result"]
            outcome["result"] = {
                "ready": report.ready,
                "checks": [
                    {"name": c.name, "status": c.status, "detail": c.detail, "repair": c.repair}
                    for c in report.checks
                ],
                "repairs": report.repairs,
            }
        return outcome

    @server.tool()
    async def read_channel_state(channel_handle: str) -> dict[str, Any]:
        """Read every offer visible in this channel (proposed/countered/expired/settled)
        plus the settlement, if any. Call this before deciding whether to accept, counter,
        or walk away."""
        outcome = await _call(client.read_channel_state(channel_handle))
        if outcome["ok"]:
            state = outcome["result"]
            outcome["result"] = {
                "offers": [_offer_to_json(o) for o in state.offers],
                "settlement": _settlement_to_json(state.settlement) if state.settlement else None,
            }
        return outcome

    @server.tool()
    async def accept_and_settle(channel_handle: str, offer_id: str) -> dict[str, Any]:
        """PAYER ONLY: accept an offer and pay it from this calling identity's private
        notes, atomically; any excess comes back as a change note in the payer's own
        channel. A supplier/payee must never call this; leave a final payee-authored offer
        for the buyer to accept.

        This closes the channel: one channel is one deal."""
        if settlement_role is SettlementRole.PAYEE:
            return _error(
                SettlementErrorCode.INVALID_REQUEST,
                "accept_and_settle is disabled: this MCP identity is configured as payee. "
                "Counter at the agreed price and leave that offer for the payer to accept.",
            )

        state_outcome = await _call(client.read_channel_state(channel_handle))
        if not state_outcome["ok"]:
            return state_outcome
        target = next(
            (offer for offer in state_outcome["result"].offers if offer.offer_id == offer_id),
            None,
        )
        if target is not None:
            failure = await _require_payable(target.terms.amount)
            if failure is not None:
                return failure
        outcome = await _call(client.accept_and_settle(channel_handle, offer_id))
        if outcome["ok"]:
            receipt = outcome["result"]
            outcome["result"] = {
                "offer_id": receipt.offer_id,
                "tx_hash": receipt.tx_hash,
                "nullifiers": receipt.nullifiers,
                "proved_at": receipt.proved_at,
                "selected_input": _amount_str(receipt.selected_input),
                "change": _amount_str(receipt.change),
            }
        return outcome

    @server.tool()
    async def wait_for_offers(
        channel_handle: str, expected_count: int, timeout_seconds: int = 300
    ) -> dict[str, Any]:
        """Block until the channel holds at least `expected_count` offers, then return them.

        Use this instead of calling read_channel_state in a loop. There is no push
        notification in the protocol, so somebody has to poll; doing it here costs one tool
        call rather than one per attempt, and the waiting happens while the agent is idle.

        Returns the same shape as read_channel_state, plus `timed_out`. A timeout is not an
        error: the counterparty may still be thinking, and the caller decides whether to
        wait again or move on."""
        deadline = time.monotonic() + timeout_seconds
        while True:
            outcome = await _call(client.read_channel_state(channel_handle))
            if not outcome["ok"]:
                return outcome
            state = outcome["result"]
            if len(state.offers) >= expected_count or time.monotonic() >= deadline:
                return {
                    "ok": True,
                    "result": {
                        "offers": [_offer_to_json(o) for o in state.offers],
                        "settlement": (
                            _settlement_to_json(state.settlement) if state.settlement else None
                        ),
                        "timed_out": len(state.offers) < expected_count,
                    },
                }
            # Each read uses O(notes) RPC round-trips. A write takes ~20 s, so faster polling
            # only adds RPC load.
            await asyncio.sleep(POLL_INTERVAL_SECONDS)

    @server.tool()
    async def grant_viewing_key(channel_handle: str, grantee: str) -> dict[str, Any]:
        """Export a self-contained bearer grant that lets `grantee` reconstruct this
        channel's full record via reveal(). Treat the returned viewing_key as a secret.
        Whoever holds it can read the relationship and token. Delivery is your
        responsibility, not this tool's."""
        outcome = await _call(client.grant_viewing_key(channel_handle, grantee))
        if outcome["ok"]:
            grant = outcome["result"]
            outcome["result"] = {
                "channel_id": grant.channel_id,
                "grantee": grant.grantee,
                "viewing_key": grant.viewing_key,
            }
        return outcome

    @server.tool()
    async def reveal(channel_id: str, grantee: str, viewing_key: str) -> dict[str, Any]:
        """Reconstruct a channel's full record (offers, counters, acceptance, settlement)
        from a viewing key grant returned by grant_viewing_key. No prior local state is
        needed. The grant is self-contained."""
        grant = ViewingKeyGrant(channel_id=channel_id, grantee=grantee, viewing_key=viewing_key)
        outcome = await _call(client.reveal(grant))
        if outcome["ok"]:
            record = outcome["result"]
            outcome["result"] = {
                "channel_id": record.channel_id,
                "participants": record.participants,
                "offers": [_offer_to_json(o) for o in record.offers],
                "settlement": _settlement_to_json(record.settlement) if record.settlement else None,
            }
        return outcome


def _amount_str(value: int | None) -> str | None:
    return str(value) if value is not None else None


def _offer_to_json(offer: Any) -> dict[str, Any]:
    return {
        "offer_id": offer.offer_id,
        "proposer": offer.proposer,
        "status": offer.status.value,
        "reply_to": offer.reply_to,
        "created_at": offer.created_at,
        "terms": {
            "amount": str(offer.terms.amount),
            "token": offer.terms.token,
            "deadline": offer.terms.deadline,
            # Hex out for the same reason it comes in as hex: a 128-bit value emitted as a
            # JSON number is silently rounded by any IEEE-754 client. See F37.
            "memo_hash": f"0x{offer.terms.memo_hash:032x}",
        },
    }


def _settlement_to_json(settlement: Any) -> dict[str, Any]:
    return {
        "acceptance": settlement.acceptance,
        "accepted_offer": settlement.accepted_offer,
        "agreed_amount": str(settlement.agreed_amount),
        "paid_amount": _amount_str(settlement.paid_amount),
        "is_consistent": settlement.is_consistent(),
    }
