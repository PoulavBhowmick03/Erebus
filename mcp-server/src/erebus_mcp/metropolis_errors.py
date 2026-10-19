"""Stable agent-facing error codes for Metropolis native failures.

The native commands report one short public diagnostic per failure. This table turns the ones
an agent can act on into a stable `code` and a `next_action`; anything else keeps the blanket
category code. Only messages in this table are forwarded as `native_error` (F64). `tests/test_metropolis_errors.py` checks that
every mapped message still exists in the Rust source, so a renamed message fails CI instead of
silently falling back to the blanket code.
"""

from __future__ import annotations

from typing import Any

from erebus import MetropolisError

# (code, next_action, retry the same operation_ref) for each known native or seam message.
_KNOWN: dict[str, tuple[str, str, bool]] = {
    # Negotiation transport: the counterparty is not there yet or went away.
    "negotiation connection deadline exceeded": (
        "PEER_UNAVAILABLE", "the counterparty did not connect in time; start its negotiate_deal with the same "
        "operation_ref, then retry this call with that operation_ref", True),
    "negotiation endpoint unavailable": (
        "PEER_UNAVAILABLE", "the counterparty is not listening; start its negotiate_deal with the same "
        "operation_ref, then retry", True),
    "peer connection unavailable or timed out": (
        "PEER_UNAVAILABLE", "the connection dropped; retry with the same operation_ref", True),
    "offer delivery uncertain; retain this operation and reconnect": (
        "PEER_UNAVAILABLE", "the offer may have been delivered; retry with the same operation_ref, never a new one", True),
    "negotiation listener unavailable": (
        "LISTENER_UNAVAILABLE", "another process holds this participant's negotiation port; the operator must "
        "stop it before retrying", True),
    # Identity material the operator must renew.
    "local discovery descriptor is invalid or expired": (
        "DESCRIPTOR_EXPIRED", "this participant's discovery descriptor expired; the operator renews it with "
        "erebus-init --metropolis quickstart renew (keys and funds are kept), or for separate machines re-runs "
        "init and connect and shares the new descriptor", False),
    "peer discovery descriptor is invalid or expired": (
        "DESCRIPTOR_EXPIRED", "the counterparty's descriptor expired; ask its operator for a fresh share", False),
    "invalid operator configuration": (
        "OPERATOR_CONFIG_INVALID", "the operator configuration was refused; the operator must fix it", False),
    "invalid operation reference": (
        "INVALID_OPERATION_REF", "pass 64 lowercase hex digits, or omit operation_ref to have one created", False),
    "operation ID must be 64 nonzero lowercase hex digits": (
        "INVALID_OPERATION_REF", "pass 64 lowercase hex digits, or omit operation_ref to have one created", False),
    # Signing and settlement.
    "local signing failed; retain nonce and reservations": (
        "SIGNER_CLAIM_HELD", "a wallet transaction still holds the signer; call sync_wallet until nothing is "
        "pending, then retry settle_deal with the same operation_ref", True),
    # Providers.
    "RPC provider rate limited; retain reservations and retry later": (
        "RATE_LIMITED", "an RPC provider is throttling; wait, then retry with the same operation_ref", True),
    "RPC provider rate limited; retry later or lower its requests_per_second": (
        "RATE_LIMITED", "an RPC provider is throttling; wait, then retry; the operator can lower "
        "rpc_requests_per_second", True),
    "paired deployment authentication failed": (
        "PROVIDER_UNAVAILABLE", "an RPC provider failed to authenticate the deployment; retry later", True),
    "paired pool evidence unavailable; retain reservations": (
        "PROVIDER_UNAVAILABLE", "the two RPC providers could not supply agreeing evidence; retry later with the "
        "same operation_ref", True),
    "paired finalized wallet recovery failed; retain reservations": (
        "PROVIDER_UNAVAILABLE", "wallet history could not be confirmed by both providers; retry later", True),
    "paired wallet recovery failed": (
        "PROVIDER_UNAVAILABLE", "wallet history could not be confirmed by both providers; retry sync_wallet later", True),
    # Wallet policy and funding (seam or native).
    "wallet amount exceeds operator policy": (
        "POLICY_REFUSED", "the amount exceeds the operator's per-action cap; use a smaller amount", False),
    "wallet action is not authorized by operator policy": (
        "POLICY_REFUSED", "this wallet action is not enabled by the operator", False),
    "deposit allowance insufficient; run approve first": (
        "ALLOWANCE_REQUIRED", "approve at least this amount, wait until it completes, then deposit with the same "
        "operation_ref", True),
    "another deposit is retained; resume its original identity": (
        "OPERATION_IN_PROGRESS", "an earlier deposit is still retained; sync_wallet until it completes", True),
}

# Diagnostics that come from the Python seam itself, before any native call.
SEAM_MESSAGES = frozenset({
    "wallet amount exceeds operator policy", "wallet action is not authorized by operator policy",
    "operation ID must be 64 nonzero lowercase hex digits",
})


def failure(error: MetropolisError, category: str, retry_field: str = "retry_without_new_payment") -> dict[str, Any]:
    """One structured failure: a stable code, the next step, and the native text when there is one."""
    message = error.native_error or (str(error) if str(error) in _KNOWN else None)
    code, next_action, same_ref = _KNOWN.get(message or "", (
        category, "retain the operation ID and recover by observation", True))
    body: dict[str, Any] = {"code": code, "category": category, "message": str(error),
                            "next_action": next_action, "retry_same_operation_ref": same_ref,
                            retry_field: True}
    # Only allowlisted native diagnostics cross into agent context (F64): an unrecognized native
    # message could carry private details, so it is never forwarded.
    if error.native_error in _KNOWN:
        body["native_error"] = error.native_error
    elif error.native_error is not None:
        body["next_action"] = ("an unrecognized native failure; retain the operation ID and recover by observation; "
                               "the operator can run the native command directly to see its error")
    return {"ok": False, "error": body}


# Server-decided refusals that clear on their own or once the agent finishes the named operation; retrying
# them with the same operation_ref is the intended next step. Anything else needs a changed request.
_RETRY_SAME_REF = frozenset({"WALLET_OPERATION_PENDING", "OPERATION_IN_PROGRESS", "FUNDS_REQUIRED"})


def refused(code: str, category: str, next_action: str, **extra: Any) -> dict[str, Any]:
    """A failure decided by this server before any native call."""
    return {"ok": False, "error": {"code": code, "category": category, "message": next_action,
                                   "next_action": next_action, "retry_same_operation_ref": code in _RETRY_SAME_REF,
                                   "retry_without_new_payment": True, **extra}}


def known_messages() -> frozenset[str]:
    return frozenset(_KNOWN)
