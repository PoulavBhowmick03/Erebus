"""The auditor's side of a settled deal, through the disclosure MCP server.

The auditor holds a scoped grant for one deal. Agreement verification is offline. Payment
verification needs the operator's deployment configuration, so the caller opts in. Delivery is never verified, and a result that claims it is rejected.
"""

from __future__ import annotations

from typing import Any

from erebus_agents.metropolis_loop import HarnessError, ToolSession, _structured

FLAGS = ("agreement_verified", "payment_verified", "delivery_verified", "deal_commitment")


async def drive_audit(auditor: ToolSession, grant_name: str, expected_issuer: str, *,
                      verify_payment: bool = False, expected_commitment: str | None = None) -> dict[str, Any]:
    arguments = {"grant_name": grant_name, "expected_issuer": expected_issuer}
    agreement = _structured(await auditor.call_tool("verify_disclosed_agreement", arguments))
    if not agreement.get("ok") or agreement["result"].get("agreement_verified") is not True:
        raise HarnessError("grant did not verify as an agreement")
    result = agreement["result"]
    if verify_payment:
        payment = _structured(await auditor.call_tool("verify_disclosed_payment", arguments))
        if not payment.get("ok"):
            raise HarnessError("payment verification unavailable; pending is not evidence of non-payment")
        result = payment["result"]
    if result.get("delivery_verified"):
        raise HarnessError("disclosure never proves delivery")
    if expected_commitment is not None and result.get("deal_commitment") != expected_commitment:
        raise HarnessError("disclosed deal is not the negotiated deal")
    return {flag: result.get(flag) for flag in FLAGS}
