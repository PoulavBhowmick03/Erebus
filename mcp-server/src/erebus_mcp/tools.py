"""The 7 tools from `docs/ishita.md` I1.3, wrapping an `ErebusClient`.

Flat primitive arguments, not a nested `OfferTerms` object — "an agent should understand
when to call each without reading source" reads better against `amount: int, token: str,
deadline: int, memo_hash: int` than against an opaque struct.

**Error shape, checked against the installed `mcp==2.0.0`:** letting `ErebusError` escape a
tool function does *not* produce a structured error. `Tool.run` catches it, wraps it as
`ToolError(f"Error executing tool {name}: {e}")`, and the server's `_handle_call_tool`
catches that and returns `CallToolResult(content=[TextContent(text=str(e))], is_error=True)`
— a prose string with the code embedded in it, exactly the "opaque string" the doc says a
tool error must not be. So every tool here catches `ErebusError` itself and returns a
plain JSON-serializable dict with an explicit `ok` flag — the call succeeds at the MCP
protocol level either way, and the payload carries the structured `SettlementErrorCode`
for the caller to branch on.
"""

from __future__ import annotations

from typing import Any

from mcp.server import MCPServer

from erebus_mcp.interface import ErebusClient, ErebusError, OfferTerms, ViewingKeyGrant


def register_tools(server: MCPServer, client: ErebusClient) -> None:
    """Registers all 7 tools against `server`, each closing over `client`."""

    async def _call(coro: Any) -> dict[str, Any]:
        try:
            result = await coro
        except ErebusError as e:
            return {"ok": False, "error": {"code": e.code.value, "message": e.message, "retryable": e.retryable}}
        return {"ok": True, "result": result}

    @server.tool()
    async def open_channel(counterparty: str) -> dict[str, Any]:
        """Establish a private channel with a counterparty (their address). Returns a
        channel_handle to use in every other call for this relationship."""
        outcome = await _call(client.open_channel(counterparty))
        if outcome["ok"]:
            outcome["result"] = {"channel_handle": outcome["result"]}
        return outcome

    @server.tool()
    async def propose_offer(
        channel_handle: str, amount: int, token: str, deadline: int, memo_hash: int
    ) -> dict[str, Any]:
        """Write a new offer into the channel: amount in token base units, token address,
        deadline as a unix timestamp, memo_hash as a 128-bit int (truncate your own hash to
        the low 128 bits before calling — this field does not fit a full digest)."""
        terms = OfferTerms(amount=amount, token=token, deadline=deadline, memo_hash=memo_hash)
        outcome = await _call(client.propose_offer(channel_handle, terms))
        if outcome["ok"]:
            outcome["result"] = {"offer_id": outcome["result"]}
        return outcome

    @server.tool()
    async def counter_offer(
        channel_handle: str,
        reply_to: str,
        amount: int,
        token: str,
        deadline: int,
        memo_hash: int,
    ) -> dict[str, Any]:
        """Write a counter-offer replying to a specific offer_id from the other party.
        Does not withdraw the offer you're replying to — it stays acceptable until it
        expires or is settled. Use a short deadline instead of trying to retract."""
        terms = OfferTerms(amount=amount, token=token, deadline=deadline, memo_hash=memo_hash)
        outcome = await _call(client.counter_offer(channel_handle, reply_to, terms))
        if outcome["ok"]:
            outcome["result"] = {"offer_id": outcome["result"]}
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
        """Accept an offer from the other party AND settle payment atomically — one state
        transition, not two. This closes the channel to further offers: one channel is one
        deal. You cannot accept your own offer."""
        outcome = await _call(client.accept_and_settle(channel_handle, offer_id))
        if outcome["ok"]:
            receipt = outcome["result"]
            outcome["result"] = {
                "offer_id": receipt.offer_id,
                "tx_hash": receipt.tx_hash,
                "nullifiers": receipt.nullifiers,
                "proved_at": receipt.proved_at,
            }
        return outcome

    @server.tool()
    async def grant_viewing_key(channel_handle: str, grantee: str) -> dict[str, Any]:
        """Export a self-contained bearer grant that lets `grantee` reconstruct this
        channel's full record via reveal(). Treat the returned viewing_key as a secret —
        whoever holds it can read the relationship and token; delivery is your
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
        needed — this works from the grant alone."""
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


def _offer_to_json(offer: Any) -> dict[str, Any]:
    return {
        "offer_id": offer.offer_id,
        "proposer": offer.proposer,
        "status": offer.status.value,
        "reply_to": offer.reply_to,
        "created_at": offer.created_at,
        "terms": {
            "amount": offer.terms.amount,
            "token": offer.terms.token,
            "deadline": offer.terms.deadline,
            "memo_hash": offer.terms.memo_hash,
        },
    }


def _settlement_to_json(settlement: Any) -> dict[str, Any]:
    return {
        "acceptance": settlement.acceptance,
        "accepted_offer": settlement.accepted_offer,
        "agreed_amount": settlement.agreed_amount,
        "paid_amount": settlement.paid_amount,
        "is_consistent": settlement.is_consistent(),
    }
