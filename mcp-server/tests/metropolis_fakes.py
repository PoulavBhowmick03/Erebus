"""Stateful stand-ins for erebus-wallet, erebus-negotiate and erebus-payment.

They speak the native JSON envelopes the seam validates, keep state in FAKE_STATE, and count
every simulated broadcast, so tests can prove the MCP layer never sends twice. Operations finish
after two syncs, like a transaction that needs a few blocks to become final.
"""

import json
import os
import sys
from pathlib import Path

STATE = Path(os.environ["FAKE_STATE"])


def load():
    if STATE.exists():
        return json.loads(STATE.read_text())
    return {"sends": 0, "ops": {}, "notes": [], "balance": int(os.environ.get("FAKE_BALANCE", "0")),
            "lag": int(os.environ.get("FAKE_HISTORY_LAG", "0")),
            "allowance": 0, "negotiate_failures": int(os.environ.get("FAKE_NEGOTIATE_FAILURES", "0")),
            "settled_with_pending": None, "calls": []}


def save(state):
    STATE.write_text(json.dumps(state))


def reply(value, code=0):
    stream = sys.stdout if code in (0, 2) and "error" not in value else sys.stderr
    if os.environ.get("FAKE_ERRORS_ON_STDOUT") == "1":
        stream = sys.stdout
    stream.write(json.dumps(value))
    sys.exit(code)


def operations(state):
    return [{"identity": op["identity"], "kind": op["kind"], "state": op["state"]} for op in state["ops"].values()]


def wallet(request, state):
    method = request["method"]
    if method == "version":
        reply({"status": "ok", "result": {"protocol": 1, "methods": ["approve", "wrap", "deposit", "notes", "sync",
               "status", "unwrap", "withdraw", "resume", "replace"], "deposit_operation_references": True,
               "funding_operation_references": True, "paired_account_diagnostics": True}})
    if method == "sync":
        # Each sync closes one step of a lagging history; sends refuse until it reaches the head.
        state["lag"] = max(0, state.get("lag", 0) - 1)
        completed = []
        for op in state["ops"].values():
            if op["state"] == "submitted":
                op["polls"] += 1
                if op["polls"] >= 2:
                    op["state"] = "completed"
                    completed.append(op["identity"])
                    amount = int(op["amount"])
                    if op["kind"] == "wrap":
                        state["balance"] += amount
                    elif op["kind"] == "approve":
                        state["allowance"] = amount
                    elif op["kind"] == "deposit":
                        state["balance"] -= amount
                        state["allowance"] -= amount
                        state["notes"].append({"commitment": op["identity"], "amount": op["amount"], "included": True,
                                               "consumed": False, "reserved": False})
        save(state)
        pending = [op["identity"] for op in state["ops"].values() if op["state"] == "submitted"]
        reply({"status": "ok", "result": {"completed": completed, "pending": pending, "operations": operations(state),
                                          "nonce_released": not pending, "scanned_through": 7}})
    if method == "notes":
        reply({"status": "ok", "result": {"notes": state["notes"], "operations": operations(state)}})
    if method == "status":
        reply({"status": "ok", "result": {"notes": state["notes"], "operations": operations(state),
               "account": {"native_balance": "5", "asset_balance": str(state["balance"]),
                           "allowance": str(state["allowance"]), "anchor_block": 7, "anchor_hash": "aa" * 32,
                           "anchor_timestamp": 1}, "history_scope": "checkpoint", "history_from_block": 3}})
    kind, ref, amount = method, request["operation_ref"], request["amount"]
    known = state["ops"].get(ref)
    if known is not None:
        if known["state"] == "completed":
            reply({"status": "ok", "result": {"kind": kind, "already_completed": True}})
        reply({"status": "ok", "result": submitted(known)})
    if state.get("lag", 0) > 0:
        reply({"status": "pending", "error": "verified pool history is not complete; resume sync",
               "result": {"history_pending": True, "next_log_block": 5, "through": 7,
                          "retry_without_new_payment": True}}, 2)
    if kind == "deposit" and state["allowance"] < int(amount):
        reply({"status": "error", "error": "deposit allowance insufficient; run approve first"}, 1)
    identity = format(len(state["ops"]) + 1, "064x")
    op = {"kind": kind, "amount": amount, "identity": identity, "state": "submitted", "polls": 0}
    state["ops"][ref] = op
    state["sends"] += 1
    save(state)
    reply({"status": "ok", "result": submitted(op)})


def submitted(op):
    view = {"kind": op["kind"], "amount": op["amount"], "submission": {"state": "submitted"}}
    if op["kind"] == "deposit":
        view["commitment"] = op["identity"]
    else:
        view["identity"] = op["identity"]
    if op["kind"] == "approve":
        view["spender"] = "0x8868f35140f3602c80e2c76132740b6f0c0b409d"
    return view


def fail(state, message):
    save(state)
    reply({"protocol_version": 1, "status": "error", "error": message,
           "payment_verified": False, "delivery_verified": False}, 1)


def negotiate(request, state):
    if request["method"] == "version":
        reply({"status": "ok", "protocol_version": 1, "service": "erebus-negotiate", "settlement_submission": False})
    # A many-buyer seller: FAKE_PEERS names the configured peers, FAKE_CONNECTING the order in
    # which buyers connect (each connects once), FAKE_UNCERTAIN peers whose first exchange binds
    # the operation and then fails as uncertain delivery.
    configured = [peer for peer in os.environ.get("FAKE_PEERS", "").split(",") if peer]
    if request["method"] == "list_peers":
        reply({"protocol_version": 1, "status": "ok", "config_version": 2, "peers": [
            {"peer": peer, "enabled": True, "expired": False, "live": True, "resource": f"data.{peer}.v1",
             "price": "70", "minimum_price": "65", "expires_at": 4102444800, "legacy_transport": False}
            for peer in configured]})
    state["calls"].append(request["operation_ref"])
    state.setdefault("requests", []).append({key: value for key, value in request.items() if key != "config_file"})
    peer = None
    if configured:
        connecting = state.setdefault("connecting", [p for p in os.environ.get("FAKE_CONNECTING", "").split(",") if p])
        bound = state.setdefault("bound", {})
        ref, requested = request["operation_ref"], request.get("peer")
        if requested is not None and requested not in configured:
            fail(state, "negotiation peer is not configured")
        if ref in bound and requested is not None and requested != bound[ref]:
            fail(state, "operation is bound to another negotiation peer")
        peer = bound.get(ref) or requested
        if peer is None or peer not in bound.values() or bound.get(ref) != peer:
            candidates = [p for p in connecting if p == peer] if peer else list(connecting)
            if not candidates:
                fail(state, "no configured peer connected before the deadline")
            peer = candidates[0]
            connecting.remove(peer)
            bound[ref] = peer
        uncertain = state.setdefault("uncertain", [p for p in os.environ.get("FAKE_UNCERTAIN", "").split(",") if p])
        if peer in uncertain:
            uncertain.remove(peer)
            connecting.append(peer)
            fail(state, "offer delivery uncertain; retain this operation and reconnect")
    if state["negotiate_failures"] > 0:
        state["negotiate_failures"] -= 1
        save(state)
        reply({"protocol_version": 1, "status": "error", "error": "negotiation endpoint unavailable",
               "payment_verified": False, "delivery_verified": False}, 1)
    save(state)
    ref = request["operation_ref"]
    reply({"protocol_version": 1, "status": "authorized", "operation_ref": ref, "deal_id": "ab" * 16,
           "deal_commitment": ref[:62] + "cd", "deal_nullifier": "ef" * 32, "agreement_verified": True,
           "evidence_file": str(Path(os.environ["FAKE_STATE_ROOT"]) / "agent" / f"{ref}.0.tx"),
           "payment_verified": False, "delivery_verified": False, "measurements_ms": {"negotiation": 5},
           **({"peer": peer} if peer else {})})


def payment(request, state):
    if request["method"] == "version":
        reply({"status": "ok", "protocol_version": 1, "service": "erebus-payment",
               "modes": ["public-bound", "shielded"], "automatic_rebroadcast": False})
    pending = [op["identity"] for op in state["ops"].values() if op["state"] == "submitted"]
    settle = request["method"] == "settle"
    if settle:
        state["settled_with_pending"] = pending
        state["sends"] += 1
        save(state)
    reply({"status": "pending" if settle else "ready", "mode": "shielded", "agreement_verified": True,
           "payment_verified": False, "delivery_verified": False, "operation_ref": request["operation_ref"],
           "deal_commitment": "cd" * 32, "deal_nullifier": "ef" * 32, "submitted_this_call": settle,
           "retry_without_new_payment": True, "measurements_ms": {}, **({"stage": "Submitted"} if settle else {})},
          2 if settle else 0)


if __name__ == "__main__":
    request = json.loads(sys.stdin.read())
    current = load()
    {"wallet": wallet, "negotiate": negotiate, "payment": payment}[sys.argv[1]](request, current)
