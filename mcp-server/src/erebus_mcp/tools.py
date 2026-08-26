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
import json
import os
import tempfile
import time
from pathlib import Path
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
from erebus_mcp.intent import IntentConflict, IntentStore
from erebus_mcp.spending import SpendGuard


#: Poll interval. Writes take about 20 s, and each read needs one RPC round-trip per note.
POLL_INTERVAL_SECONDS = 5.0


def _save_grant(path_value: str, grant: ViewingKeyGrant) -> Path:
    """Atomically publish a new mode-0600 grant file without overwriting a prior grant."""
    path = Path(path_value).expanduser()
    encoded = json.dumps(
        {
            "channel_id": grant.channel_id,
            "grantee": grant.grantee,
            "deal_id": grant.deal_id,
            "expires_at": grant.expires_at,
            "viewing_key": grant.viewing_key,
        },
        separators=(",", ":"),
    ) + "\n"
    descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        os.chmod(temporary, 0o600)
        with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
            descriptor = -1
            stream.write(encoded)
            stream.flush()
            os.fsync(stream.fileno())
        os.link(temporary, path)
        return path
    finally:
        if descriptor >= 0:
            os.close(descriptor)
        try:
            os.unlink(temporary)
        except FileNotFoundError:
            pass


def _write_private_json(path: Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        os.chmod(temporary, 0o600)
        with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
            descriptor = -1
            json.dump(value, stream, sort_keys=True, separators=(",", ":"))
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if descriptor >= 0:
            os.close(descriptor)
        try:
            os.unlink(temporary)
        except FileNotFoundError:
            pass


def _grant_export_intent(
    operation_id: str,
    output_path: str,
    request: dict[str, Any],
) -> tuple[Path, dict[str, Any] | None]:
    """Claim a grant export or return its completed public result."""
    path = Path(output_path).expanduser()
    journal = path.with_name(path.name + ".erebus-export.json")
    expected = {"operation_id": operation_id, "request": request}
    if journal.exists():
        record = json.loads(journal.read_text(encoding="utf-8"))
        if record.get("operation_id") != operation_id or record.get("request") != request:
            raise ValueError("grant export operation is already bound to different parameters")
        if record.get("state") == "completed":
            if not path.exists():
                raise ValueError("completed grant export is missing its grant file")
            return journal, record["result"]
        if path.exists():
            raw = json.loads(path.read_text(encoding="utf-8"))
            result = {
                "channel_id": raw["channel_id"],
                "deal_id": raw.get("deal_id"),
                "grantee": raw["grantee"],
                "expires_at": raw.get("expires_at"),
                "grant_path": str(path),
            }
            _write_private_json(journal, {**expected, "state": "completed", "result": result})
            return journal, result
        return journal, None
    _write_private_json(journal, {**expected, "state": "prepared"})
    return journal, None


def register_tools(
    server: MCPServer,
    client: ErebusClient,
    settlement_role: SettlementRole,
    spend_guard: SpendGuard,
    intent_store: IntentStore,
    backend: str,
    network: str,
) -> None:
    """Registers the tools against one identity-bound client.

    `spend_guard` enforces per-token, per-deal, and daily-cumulative spending caps below
    the agent (roadmap 9.1). An unconfigured guard (no `EREBUS_SPENDING_LIMITS`) never
    refuses anything, so passing one is always safe.

    `intent_store` persists an operation id and its canonical parameters before each
    chain-writing call, so a process killed mid-call leaves a record instead of silence
    (plan.md, Ishita task 1). It is not yet threaded into the seam call itself — that
    needs protocol 4 — so today it only protects against this process's own crash, not
    against chain-level ambiguity; see erebus_mcp/intent.py.

    `backend` ("mock" or "seam") and `network` are stamped onto every tool result,
    success or failure, so a model cannot mistake a mock run for a live one, or one
    network for another, from the transcript alone (roadmap 9.2).
    """

    def _envelope(
        ok: bool, *, result: Any = None, error: dict[str, Any] | None = None
    ) -> dict[str, Any]:
        payload: dict[str, Any] = {"ok": ok, "backend": backend, "network": network}
        if ok:
            payload["result"] = result
        else:
            payload["error"] = error
        return payload

    async def _call(coro: Any) -> dict[str, Any]:
        try:
            result = await coro
        except ErebusError as e:
            return _envelope(
                False,
                error={"code": e.code.value, "message": e.message, "retryable": e.retryable},
            )
        return _envelope(True, result=result)

    async def _write_call(
        tool: str, params: dict[str, Any], operation_id: str, coro: Any
    ) -> dict[str, Any]:
        """Wraps a chain-writing seam call with durable caller intent (plan.md task 1):
        persist before the call, resolve once it has returned to this process. `_call`
        already returning — `ok` true or a caught `ErebusError` — is itself proof this
        process did not crash; only a killed process leaves the record on disk."""
        try:
            operation_id = intent_store.begin(tool, params, operation_id)
        except IntentConflict as error:
            return _error(SettlementErrorCode.INVALID_REQUEST, str(error))
        try:
            outcome = await _call(coro)
            outcome["operation_id"] = operation_id
            return outcome
        finally:
            intent_store.resolve(operation_id)

    def _error(code: SettlementErrorCode, message: str) -> dict[str, Any]:
        return _envelope(
            False, error={"code": code.value, "message": message, "retryable": False}
        )

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

    async def _reconcile_spending(
        findings: list[dict[str, Any]] | None = None,
    ) -> list[dict[str, Any]]:
        """Rebuild local cap counters from Rust-authoritative committed settlements."""
        if findings is None:
            findings = await client.reconcile()
        for finding in findings:
            if finding.get("operation") != "accept_and_settle":
                continue
            if finding.get("outcome") != "effect":
                continue
            request = finding.get("request") or {}
            if request.get("method") != "accept_and_settle":
                continue
            state = await client.read_channel_state(request["handle"])
            target = next(
                (offer for offer in state.offers if offer.offer_id == request["offer_id"]),
                None,
            )
            if target is not None:
                spend_guard.record(
                    target.terms.token,
                    target.terms.amount,
                    finding["operation_id"],
                )
        return findings

    @server.tool()
    async def open_channel(operation_id: str, counterparty: str) -> dict[str, Any]:
        """Establish a channel with a counterparty (their address). Returns a
        channel_handle to use in every other call for this relationship.

        This call itself is not private: it writes the counterparty's address to public
        chain calldata (F38). Negotiation content and settlement amounts are hidden once
        the channel is open; that you opened one with this counterparty is not.

        This is an on-chain write: simulate, prove, submit. Expect 1-3 minutes
        of silence and do not abort or retry while it runs; the operation is not
        stuck below five minutes, and abandoning it does not cancel the
        transaction it may already have submitted."""
        outcome = await _write_call(
            "open_channel",
            {"counterparty": counterparty},
            operation_id,
            client.open_channel(operation_id, counterparty),
        )
        if outcome["ok"]:
            outcome["result"] = {"channel_handle": outcome["result"]}
        return outcome

    @server.tool()
    async def propose_offer(
        operation_id: str,
        channel_handle: str,
        amount: int | str,
        token: str,
        deadline: int,
        memo_hash: int | str,
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
        as change. Payee offers are asks and do not spend the payee's notes.

        This is an on-chain write: simulate, prove, submit. Expect 1-3 minutes
        of silence and do not abort or retry while it runs; the operation is not
        stuck below five minutes, and abandoning it does not cancel the
        transaction it may already have submitted."""
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
        outcome = await _write_call(
            "propose_offer",
            {
                "channel_handle": channel_handle,
                "amount": parsed_amount,
                "token": token,
                "deadline": deadline,
                "memo_hash": memo,
            },
            operation_id,
            client.propose_offer(operation_id, channel_handle, terms),
        )
        if outcome["ok"]:
            outcome["result"] = {"offer_id": outcome["result"]}
        return outcome

    @server.tool()
    async def counter_offer(
        operation_id: str,
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
        propose_offer.

        This is an on-chain write: simulate, prove, submit. Expect 1-3 minutes
        of silence and do not abort or retry while it runs; the operation is not
        stuck below five minutes, and abandoning it does not cancel the
        transaction it may already have submitted."""
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
        outcome = await _write_call(
            "counter_offer",
            {
                "channel_handle": channel_handle,
                "reply_to": reply_to,
                "amount": parsed_amount,
                "token": token,
                "deadline": deadline,
                "memo_hash": memo,
            },
            operation_id,
            client.counter_offer(operation_id, channel_handle, reply_to, terms),
        )
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
                "settlements": [_settlement_to_json(item) for item in state.settlements],
            }
        return outcome

    @server.tool()
    async def accept_and_settle(
        operation_id: str, channel_handle: str, offer_id: str
    ) -> dict[str, Any]:
        """PAYER ONLY: accept an offer and pay it from this calling identity's private
        notes, atomically; any excess comes back as a change note in the payer's own
        channel. A supplier/payee must never call this; leave a final payee-authored offer
        for the buyer to accept.

        This settles only the selected wire-v3 deal. The same channel pair can start a new
        deal after settlement.

        This is an on-chain write: simulate, prove, submit. Expect 2-4 minutes
        of silence and do not abort or retry while it runs; the operation is not
        stuck below five minutes, and abandoning it does not cancel the
        transaction it may already have submitted."""
        if settlement_role is SettlementRole.PAYEE:
            return _error(
                SettlementErrorCode.INVALID_REQUEST,
                "accept_and_settle is disabled: this MCP identity is configured as payee. "
                "Counter at the agreed price and leave that offer for the payer to accept.",
            )

        try:
            await _reconcile_spending()
        except ErebusError as error:
            return _envelope(
                False,
                error={
                    "code": error.code.value,
                    "message": error.message,
                    "retryable": error.retryable,
                },
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
            denial = spend_guard.check(target.terms.token, target.terms.amount)
            if denial is not None:
                return _error(SettlementErrorCode.SPENDING_LIMIT_EXCEEDED, denial)
        outcome = await _write_call(
            "accept_and_settle",
            {"channel_handle": channel_handle, "offer_id": offer_id},
            operation_id,
            client.accept_and_settle(operation_id, channel_handle, offer_id),
        )
        if outcome["ok"]:
            if target is not None:
                spend_guard.record(
                    target.terms.token, target.terms.amount, operation_id
                )
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
    async def reconcile() -> dict[str, Any]:
        """Read the Rust operation journal and classify every write against the chain.
        This tool never submits, resumes, or changes local state."""
        outcome = await _call(client.reconcile())
        if outcome["ok"]:
            try:
                outcome["result"] = await _reconcile_spending(outcome["result"])
            except ErebusError as error:
                return _envelope(
                    False,
                    error={
                        "code": error.code.value,
                        "message": error.message,
                        "retryable": error.retryable,
                    },
                )
        return outcome

    @server.tool()
    async def resume_operation(operation_id: str) -> dict[str, Any]:
        """Explicitly resume one operation after reconcile names its next action.
        Reuse the original operation_id. Never mint a replacement for an uncertain write."""
        return await _call(client.resume_operation(operation_id))

    @server.tool()
    async def rebuild_state() -> dict[str, Any]:
        """Add missing local channel records reconstructed from chain data. Existing
        records are retained and this operation never overwrites them."""
        return await _call(client.rebuild_state())

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
        if expected_count <= 0:
            return _error(
                SettlementErrorCode.INVALID_REQUEST,
                f"expected_count must be positive, got {expected_count}",
            )
        if timeout_seconds <= 0:
            return _error(
                SettlementErrorCode.INVALID_REQUEST,
                f"timeout_seconds must be positive, got {timeout_seconds}",
            )
        deadline = time.monotonic() + timeout_seconds
        while True:
            outcome = await _call(client.read_channel_state(channel_handle))
            if not outcome["ok"]:
                return outcome
            state = outcome["result"]
            if len(state.offers) >= expected_count or time.monotonic() >= deadline:
                return _envelope(
                    True,
                    result={
                        "offers": [_offer_to_json(o) for o in state.offers],
                        "settlements": [
                            _settlement_to_json(item) for item in state.settlements
                        ],
                        "timed_out": len(state.offers) < expected_count,
                    },
                )
            # Each read uses O(notes) RPC round-trips. A write takes ~20 s, so faster polling
            # only adds RPC load.
            await asyncio.sleep(POLL_INTERVAL_SECONDS)

    @server.tool()
    async def grant_viewing_key(
        operation_id: str,
        channel_handle: str,
        deal_id: str,
        grantee: str,
        expires_at: int,
        output_path: str,
    ) -> dict[str, Any]:
        """Encrypt one wire-v3 deal grant to a registered recipient and save it to a new
        mode-0600 file. The secret capsule never enters the MCP result or transcript.
        ``expires_at`` is a Unix timestamp chosen by the operator. Reuse ``operation_id``
        after interruption; the completed export is returned without replacing the file."""
        request = {
            "channel_handle": channel_handle,
            "deal_id": deal_id,
            "grantee": grantee,
            "expires_at": expires_at,
            "output_path": str(Path(output_path).expanduser()),
        }
        try:
            export_journal, completed = _grant_export_intent(
                operation_id, output_path, request
            )
        except (OSError, json.JSONDecodeError, KeyError, TypeError, ValueError) as error:
            return _error(SettlementErrorCode.INVALID_REQUEST, f"could not claim grant export: {error}")
        if completed is not None:
            return _envelope(True, result=completed)
        outcome = await _call(
            client.grant_viewing_key(channel_handle, deal_id, grantee, expires_at)
        )
        if outcome["ok"]:
            grant = outcome["result"]
            try:
                path = _save_grant(output_path, grant)
            except (OSError, TypeError, ValueError) as error:
                return _error(SettlementErrorCode.INVALID_REQUEST, f"could not save grant: {error}")
            outcome["result"] = {
                "channel_id": grant.channel_id,
                "deal_id": grant.deal_id,
                "grantee": grant.grantee,
                "expires_at": grant.expires_at,
                "grant_path": str(path),
            }
            _write_private_json(
                export_journal,
                {
                    "operation_id": operation_id,
                    "request": request,
                    "state": "completed",
                    "result": outcome["result"],
                },
            )
        return outcome

    @server.tool()
    async def reveal(grant_path: str) -> dict[str, Any]:
        """Open a recipient-bound grant file and reconstruct only its selected deal from
        chain data. The recipient's configured pool key must match the encrypted capsule."""
        try:
            raw = json.loads(Path(grant_path).expanduser().read_text(encoding="utf-8"))
            grant = ViewingKeyGrant(
                channel_id=raw["channel_id"],
                grantee=raw["grantee"],
                deal_id=raw.get("deal_id"),
                expires_at=raw.get("expires_at"),
                viewing_key=raw["viewing_key"],
            )
        except (OSError, json.JSONDecodeError, KeyError, TypeError) as error:
            return _error(SettlementErrorCode.INVALID_REQUEST, f"could not read grant: {error}")
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
        # A wire-v3 deal id can exceed JavaScript's exact integer range.
        "deal_id": str(offer.deal_id),
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
        "consistency": settlement.consistency().value,
    }
