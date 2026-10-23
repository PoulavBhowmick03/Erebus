"""Policy-bound wallet MCP tools: explicit actions, created references, and one funding flow.

Rust owns keys, proofs, nonces and retained bytes. These tools only choose which bounded native
action to run next, wait for it to finish, and keep the references an agent needs to retry.
"""

from __future__ import annotations

import asyncio
import secrets
import time
from typing import Any

from erebus import MetropolisError, MetropolisWallet
from mcp.server import MCPServer

from erebus_mcp.metropolis_errors import failure, refused
from erebus_mcp.operation_refs import OperationRefs

DEFAULT_WAIT_SECONDS = 120
MAX_WAIT_SECONDS = 900
FUND_WAIT_SECONDS = 540
CATEGORY = "WALLET_UNAVAILABLE"
# Any valid digest; check() validates shape and policy only, it never uses the value.
_PLACEHOLDER = "11" * 32


async def _call(wallet: MetropolisWallet, method: str, **arguments: Any) -> dict[str, Any]:
    try:
        result = await asyncio.to_thread(wallet.execute, method, **arguments)
    except MetropolisError as error:
        return failure(error, CATEGORY)
    reply: dict[str, Any] = {"ok": result["status"] == "ok", "result": result}
    if result["status"] == "pending":
        reply["next_action"] = "history or the operation is still pending; repeat with the same operation_ref"
    return reply


def _inner(reply: dict[str, Any]) -> dict[str, Any]:
    result = (reply.get("result") or {}).get("result")
    return result if isinstance(result, dict) else {}


async def _operations(wallet: MetropolisWallet) -> list[dict[str, Any]]:
    operations = _inner(await _call(wallet, "notes")).get("operations")
    return operations if isinstance(operations, list) else []


def _completed(operations: list[dict[str, Any]], identity: str | None) -> bool:
    return identity is not None and any(
        operation.get("identity") == identity and operation.get("state") == "completed" for operation in operations)


def _register_wallet_tools(server: MCPServer, wallet: MetropolisWallet, methods: list[str],
                           refs: OperationRefs, writes: asyncio.Lock, asset: dict[str, Any] | None = None) -> None:
    """`asset` describes the configured pool asset: its `address`, `signer`, and whether it has a native
    `wrapper` (WMON). Without one the signer must already hold the asset; fund_wallet never wraps it."""
    async def action(kind: str, amount: str, operation_ref: str | None) -> dict[str, Any]:
        created = False
        if operation_ref is None:
            try:
                wallet.check(kind, amount=amount, operation_ref=_PLACEHOLDER)
            except MetropolisError as error:
                return failure(error, CATEGORY)
            operations = await _operations(wallet)
            resolved = refs.resolve(kind, amount, lambda entry: _completed(operations, entry.get("identity")))
            if isinstance(resolved, dict):
                return refused("OPERATION_IN_PROGRESS", CATEGORY,
                               f"an earlier {kind} for {resolved.get('amount')} is unfinished; retry it with its "
                               "operation_ref, or sync_wallet until it completes", operation_ref=resolved["ref"])
            operation_ref, created = resolved
        reply = await _call(wallet, kind, amount=amount, operation_ref=operation_ref)
        result = _inner(reply)
        identity = result.get("identity") or result.get("commitment")
        if isinstance(identity, str):
            refs.update(operation_ref, identity=identity)
        if result.get("already_completed") is True:
            refs.update(operation_ref, done=True)
        return {**reply, "operation_ref": operation_ref, "operation_ref_created": created}

    @server.tool()
    async def wallet_notes() -> dict[str, Any]:
        """List this configured wallet's public note status without opening keys in Python."""
        return await _call(wallet, "notes")

    @server.tool()
    async def sync_wallet() -> dict[str, Any]:
        """Resume paired finalized history; never sends a transaction."""
        return await _call(wallet, "sync")

    if "status" in methods:
        @server.tool()
        async def wallet_status() -> dict[str, Any]:
            """Read paired finalized balances, pool allowance and checkpoint lag; no spend or sync."""
            return await _call(wallet, "status")

    if "approve" in methods:
        @server.tool()
        async def approve_wallet(amount: str, operation_ref: str | None = None) -> dict[str, Any]:
            """Bounded approval for this configured pool and asset only.

            Omit operation_ref to have one created and returned; repeating the call without one
            retries the same unfinished approval instead of sending another.
            """
            async with writes:
                return await action("approve", amount, operation_ref)

    if "wrap" in methods:
        @server.tool()
        async def wrap_wallet(amount: str, operation_ref: str | None = None) -> dict[str, Any]:
            """Bounded wrapping; an omitted operation_ref is created, saved and returned."""
            async with writes:
                return await action("wrap", amount, operation_ref)

    if "deposit" in methods:
        @server.tool()
        async def deposit_wallet(amount: str, operation_ref: str | None = None) -> dict[str, Any]:
            """Fund one shielded note under a durable reference; an omitted one is created and saved.

            After it is submitted, sync_wallet until the note is spendable before settling.
            """
            async with writes:
                return await action("deposit", amount, operation_ref)

    if "withdraw" in methods:
        @server.tool()
        async def withdraw_wallet(commitment: str, recipient: str) -> dict[str, Any]:
            """Withdraw one bounded note only to an operator-authorized public recipient."""
            async with writes:
                return await _call(wallet, "withdraw", commitment=commitment, recipient=recipient)

    if "unwrap" in methods:
        @server.tool()
        async def unwrap_wallet(amount: str, operation_ref: str | None = None) -> dict[str, Any]:
            """Bounded unwrap; payout address and wrapper come from fixed configuration."""
            async with writes:
                return await action("unwrap", amount, operation_ref)

    if "resume" in methods:
        @server.tool()
        async def resume_wallet(identity: str) -> dict[str, Any]:
            """Explicitly resend only the retained bytes for this wallet operation."""
            async with writes:
                return await _call(wallet, "resume", identity=identity)

    if "replace" in methods:
        @server.tool()
        async def replace_wallet(identity: str) -> dict[str, Any]:
            """Explicit fee replacement preserving intent; fees are fixed by native operator policy."""
            async with writes:
                return await _call(wallet, "replace", identity=identity)

    if {"approve", "deposit", "status"} <= set(methods):
        @server.tool()
        async def fund_wallet(amount: str, operation_ref: str | None = None,
                              wait_seconds: int = FUND_WAIT_SECONDS) -> dict[str, Any]:
            """Fund one spendable shielded note: wrap if needed (WMON only), approve if needed, deposit, then sync.

            Each step is an ordinary policy-capped wallet action under its own saved reference. The next
            step starts once the previous one is submitted; the flow then waits until every step is final
            and the note is spendable. If wait_seconds (0-900) runs out, call this again with the returned
            operation_ref: submitted or finished steps are never repeated.
            """
            if type(wait_seconds) is not int or not 0 <= wait_seconds <= MAX_WAIT_SECONDS:
                return refused("INVALID_REQUEST", CATEGORY, f"wait_seconds must be 0-{MAX_WAIT_SECONDS}")
            async with writes:
                return await _fund(wallet, methods, refs, amount, operation_ref, wait_seconds, asset)


def _shortfall(asset: dict[str, Any] | None, balance: int, wanted: int, operation_ref: str) -> dict[str, Any]:
    """FUNDS_REQUIRED for a balance below the funding amount."""
    if asset is None or asset.get("wrapper") is not None:
        # The wrapped-native pool: the operator can allow wrap instead of sending the asset.
        return refused("FUNDS_REQUIRED", CATEGORY, f"the signer holds {balance} of the asset; send at least "
                       f"{wanted - balance} more, or ask the operator to allow wrap", operation_ref=operation_ref)
    token, signer = asset.get("address"), asset.get("signer")
    return refused("FUNDS_REQUIRED", CATEGORY,
                   f"the signer {signer} holds {balance} base units of the pool asset {token} and this funding needs "
                   f"{wanted}; send the signer at least {wanted - balance} more base units of that token (this pool "
                   "cannot wrap MON), then call fund_wallet again with this operation_ref",
                   operation_ref=operation_ref, asset=token, signer=signer, asset_balance=str(balance),
                   required=str(wanted), shortfall=str(wanted - balance))


async def _fund(wallet: MetropolisWallet, methods: list[str], refs: OperationRefs, amount: str,
                operation_ref: str | None, wait_seconds: int, asset: dict[str, Any] | None = None) -> dict[str, Any]:
    try:
        for kind in ("approve", "deposit"):
            wallet.check(kind, amount=amount, operation_ref=_PLACEHOLDER)
    except MetropolisError as error:
        return failure(error, CATEGORY)
    created = False
    if operation_ref is None:
        resolved = refs.resolve("fund", amount)
        if isinstance(resolved, dict):
            return refused("OPERATION_IN_PROGRESS", CATEGORY,
                           f"an earlier funding of {resolved.get('amount')} is unfinished; continue it with its "
                           "operation_ref", operation_ref=resolved["ref"])
        operation_ref, created = resolved
    plan = refs.get(operation_ref)
    if plan is not None and plan.get("amount") != amount:
        return refused("OPERATION_IN_PROGRESS", CATEGORY, "this funding reference was created for another amount",
                       operation_ref=operation_ref)
    if plan is None or "steps" not in plan:
        # Decide every step once, from finalized balances, and save it before anything is sent.
        status = await _call(wallet, "status")
        account = _inner(status).get("account")
        if not status.get("ok") or not isinstance(account, dict):
            return {**status, "operation_ref": operation_ref}
        balance, allowance, wanted = int(account["asset_balance"]), int(account["allowance"]), int(amount)
        steps = []
        if balance < wanted:
            if "wrap" not in methods:
                return _shortfall(asset, balance, wanted, operation_ref)
            steps.append({"kind": "wrap", "amount": str(wanted - balance)})
        if allowance < wanted:
            steps.append({"kind": "approve", "amount": amount})
        steps.append({"kind": "deposit", "amount": amount})
        for step in steps:
            step["ref"], step["state"] = secrets.token_hex(32), "planned"
        if plan is None:
            refs.record({"kind": "fund", "ref": operation_ref, "amount": amount, "created": int(time.time()), "steps": steps})
        else:
            refs.update(operation_ref, steps=steps)
        plan = refs.get(operation_ref)
    deadline = time.monotonic() + wait_seconds
    steps = plan["steps"]
    waiting: str | None = None

    def progress(complete: bool, **extra: Any) -> dict[str, Any]:
        view = [{"kind": step["kind"], "amount": step["amount"], "operation_ref": step["ref"], "state": step["state"]}
                for step in steps]
        body = {"operation_ref": operation_ref, "operation_ref_created": created, "complete": complete, "steps": view, **extra}
        if complete:
            return {"ok": True, "result": body, "operation_ref": operation_ref}
        if waiting is not None:
            body["waiting_for"] = waiting
        return {"ok": False, "result": {"status": "pending", **body}, "operation_ref": operation_ref,
                "next_action": f"funding is still in progress; call fund_wallet again with operation_ref {operation_ref}"}

    for step in steps:
        # Start each step as soon as the previous one is submitted: the next send's own sync completes
        # finalized steps and releases the signer. A refusal that only means "not final or visible yet"
        # (signer busy, history behind, allowance or balance not visible) gets one sync, then a retry
        # with the same reference. Nothing here sends a step twice: each retry reuses its reference.
        while step["state"] == "planned":
            if time.monotonic() >= deadline:
                return progress(False)
            reply = await _call(wallet, step["kind"], amount=step["amount"], operation_ref=step["ref"])
            result = _inner(reply)
            if "error" in reply and not reply["error"].get("retry_same_operation_ref"):
                return {**progress(False), "error": reply["error"]}
            if result.get("already_completed") is True:
                step["state"] = "completed"
            elif reply.get("ok"):
                step["state"], step["identity"] = "submitted", result.get("identity") or result.get("commitment")
            refs.update(operation_ref, steps=steps)
            if step["state"] == "planned":
                waiting = ("history" if result.get("history_pending") is True
                           else (reply.get("error") or {}).get("code") or "pending")
                if not (await _call(wallet, "sync")).get("ok"):
                    await asyncio.sleep(1)
    # Every step is submitted. One sync settles the lane; then wait until each step is final and the
    # deposit's note is spendable so settlement can use it.
    deposit = steps[-1].get("identity")
    await _call(wallet, "sync")
    while True:
        listed = _inner(await _call(wallet, "notes"))
        operations = listed.get("operations") if isinstance(listed.get("operations"), list) else []
        changed = False
        for step in steps:
            if step["state"] == "submitted" and _completed(operations, step.get("identity")):
                step["state"], changed = "completed", True
        if changed:
            refs.update(operation_ref, steps=steps)
        notes = listed.get("notes") or []
        spendable = [note for note in notes if note.get("commitment") == deposit and note.get("included") is True
                     and note.get("consumed") is False and not note.get("reserved")]
        if all(step["state"] == "completed" for step in steps) and (spendable or deposit is None):
            refs.update(operation_ref, done=True)
            return progress(True, spendable_commitment=deposit)
        if time.monotonic() >= deadline:
            waiting = "finality"
            return progress(False)
        await asyncio.sleep(2)
        await _call(wallet, "sync")
