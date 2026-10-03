"""Path-only native negotiation and payment. Rust owns consent, proofs, and recovery."""

from __future__ import annotations

import json
import re
import shutil
import subprocess
from pathlib import Path
from typing import Any


class MetropolisError(RuntimeError):
    """Retain the operation and recover by observation, never by creating another payment."""


def _digest(value: Any) -> bool:
    return isinstance(value, str) and re.fullmatch(r"[a-f0-9]{64}", value, re.ASCII) is not None and value != "0" * 64


class MetropolisSeam:
    """Operator-fixed configurations, role, and mode; agents supply only operation IDs."""

    def __init__(self, *, negotiation_config: str, state_root: str, role: str,
                 payment_config: str | None = None, mode: str | None = None,
                 negotiation_binary: str | None = None, payment_binary: str | None = None,
                 timeout: int = 600) -> None:
        if role not in {"buyer", "seller"} or type(timeout) is not int or not 1 <= timeout <= 900:
            raise MetropolisError("configure a participant role and bounded native timeout")
        paths = [negotiation_config, state_root] + ([payment_config] if payment_config is not None else [])
        if any(not isinstance(path, str) or len(path) > 4096 or not Path(path).is_absolute() for path in paths):
            raise MetropolisError("configure absolute operator paths")
        if (role == "seller" and payment_config is not None) or (role == "buyer" and (payment_config is None or mode not in {"public-bound", "shielded"})):
            raise MetropolisError("only the configured buyer may use a fixed payment mode")
        self._negotiation = negotiation_binary or shutil.which("erebus-negotiate")
        self._payment = payment_binary or shutil.which("erebus-payment")
        if not self._negotiation or (role == "buyer" and not self._payment):
            raise MetropolisError("install the Metropolis native binaries")
        self._negotiation_config, self._payment_config = negotiation_config, payment_config
        self._state_root, self._role, self._mode, self._timeout = Path(state_root), role, mode, timeout

    def _run(self, binary: str, request: dict[str, Any]) -> tuple[int, dict[str, Any]]:
        try:
            completed = subprocess.run([binary], input=json.dumps(request), capture_output=True,
                                       text=True, check=False, timeout=self._timeout)
        except (OSError, subprocess.TimeoutExpired):
            raise MetropolisError("native operation unavailable; retain its ID and recover by observation") from None
        if completed.returncode not in {0, 2} or completed.stderr or len(completed.stdout) > 32_768:
            raise MetropolisError("native operation failed; retain its ID and recover by observation")
        try:
            response = json.loads(completed.stdout)
        except (ValueError, TypeError):
            raise MetropolisError("invalid native response; retain operation state") from None
        if not isinstance(response, dict):
            raise MetropolisError("invalid native response; retain operation state")
        return completed.returncode, response

    def version(self) -> dict[str, Any]:
        """Check native protocols without reading participant keys or making RPC calls."""
        code, response = self._run(self._negotiation, {"method": "version"})
        expected = {"status": "ok", "protocol_version": 1, "service": "erebus-negotiate", "settlement_submission": False}
        if code != 0 or response != expected or type(response.get("protocol_version")) is not int or type(response.get("settlement_submission")) is not bool:
            raise MetropolisError("negotiation binary protocol mismatch")
        if self._role == "buyer":
            code, response = self._run(self._payment, {"method": "version"})
            expected = {"status": "ok", "protocol_version": 1, "service": "erebus-payment",
                        "modes": ["public-bound", "shielded"], "automatic_rebroadcast": False}
            if code != 0 or response != expected or type(response.get("protocol_version")) is not int or type(response.get("automatic_rebroadcast")) is not bool:
                raise MetropolisError("payment binary protocol mismatch")
        return {"protocol_version": 1, "role": self._role, "mode": self._mode, "automatic_rebroadcast": False}

    @staticmethod
    def _operation(operation_ref: str) -> None:
        if not _digest(operation_ref):
            raise MetropolisError("operation ID must be 64 nonzero lowercase hex digits")

    def negotiate(self, operation_ref: str, *, freeze_only: bool = False) -> dict[str, Any]:
        """Negotiate against the fixed service policy. This cannot submit a payment."""
        self._operation(operation_ref)
        if type(freeze_only) is not bool:
            raise MetropolisError("freeze_only must be a boolean")
        code, response = self._run(self._negotiation, {"method": "negotiate", "config_file": self._negotiation_config,
                                                       "operation_ref": operation_ref, "freeze_only": freeze_only})
        status = "frozen" if freeze_only else "authorized"
        fields = {"protocol_version", "status", "operation_ref", "deal_id", "deal_commitment", "deal_nullifier", "payment_verified", "delivery_verified", "measurements_ms"}
        if not freeze_only:
            fields |= {"agreement_verified", "evidence_file"}
        expected_file = self._state_root / "agent" / f"{operation_ref}.0.tx"
        if (code != 0 or set(response) != fields or response.get("status") != status
            or type(response.get("protocol_version")) is not int or response["protocol_version"] != 1
            or response.get("operation_ref") != operation_ref
            or not isinstance(response.get("deal_id"), str) or re.fullmatch(r"[a-f0-9]{32}", response["deal_id"], re.ASCII) is None
            or not all(_digest(response.get(field)) for field in ("deal_commitment", "deal_nullifier"))
            or response.get("payment_verified") is not False or response.get("delivery_verified") is not False
            or (not freeze_only and (response.get("agreement_verified") is not True or response.get("evidence_file") != str(expected_file)))
            or not isinstance(response.get("measurements_ms"), dict) or set(response["measurements_ms"]) != {"negotiation"}
            or type(response["measurements_ms"]["negotiation"]) is not int or not 0 <= response["measurements_ms"]["negotiation"] <= 2**64 - 1):
            raise MetropolisError("invalid negotiation result or verification claims")
        return response

    def payment(self, operation_ref: str, *, method: str) -> dict[str, Any]:
        """Funding checks, first settlement, or observation. No automatic payment retry."""
        self._operation(operation_ref)
        if self._role != "buyer" or method not in {"funding", "settle", "observe"}:
            raise MetropolisError("payment requires the configured buyer and an allowed method")
        code, response = self._run(self._payment, {"method": method, "config_file": self._payment_config, "operation_ref": operation_ref})
        required = {"status", "mode", "agreement_verified", "payment_verified", "delivery_verified", "operation_ref",
                    "deal_commitment", "deal_nullifier", "submitted_this_call", "retry_without_new_payment", "measurements_ms"}
        optional = {"history_pending", "next_log_block", "through", "ancestry_block", "stage", "broadcast_attempts",
                    "winning_commitment", "nonce_cleanup_pending", "wallet_release_required", "funding", "proof_required",
                    "transaction_hash", "locally_signed_transaction_hash", "downloaded_bytes"}
        status = response.get("status")
        if (not required <= set(response) or not set(response) <= required | optional
            or status not in {"ok", "ready", "pending", "funding_required", "closed_unpaid"}
            or (code == 0) != (status in {"ok", "ready"}) or response.get("mode") != self._mode
            or response.get("operation_ref") != operation_ref or response.get("agreement_verified") is not True
            or response.get("delivery_verified") is not False or response.get("retry_without_new_payment") is not True
            or not all(_digest(response.get(field)) for field in ("deal_commitment", "deal_nullifier"))):
            raise MetropolisError("invalid payment result or configured mode mismatch")
        for field in ("payment_verified", "submitted_this_call", "history_pending", "nonce_cleanup_pending", "wallet_release_required", "proof_required"):
            if field in response and type(response[field]) is not bool:
                raise MetropolisError("invalid payment boolean")
        if response["submitted_this_call"] and method != "settle":
            raise MetropolisError("read-only operation claimed submission")
        if response["payment_verified"] and (status != "ok" or response.get("stage") != "Finalized" or response.get("winning_commitment") != response["deal_commitment"]):
            raise MetropolisError("payment lacks finalized matching evidence")
        if "winning_commitment" in response and not _digest(response["winning_commitment"]):
            raise MetropolisError("invalid winning commitment")
        for field in ("transaction_hash", "locally_signed_transaction_hash"):
            if field in response and (not isinstance(response[field], str) or re.fullmatch(r"0x[a-f0-9]{64}", response[field], re.ASCII) is None):
                raise MetropolisError("invalid transaction hash")
        stages = {"Reserved", "AuthorizationUnknown", "BuyerAuthorized", "Authorized", "Prepared", "Signed", "BroadcastUnknown", "Submitted", "Finalized", "ClosedUnpaid"}
        if "stage" in response and (not isinstance(response["stage"], str) or response["stage"] not in stages):
            raise MetropolisError("invalid durable stage")
        for field in ("next_log_block", "through", "ancestry_block", "broadcast_attempts", "downloaded_bytes"):
            if field in response and (type(response[field]) is not int or not 0 <= response[field] <= 2**64 - 1):
                raise MetropolisError("invalid bounded diagnostic")
        metrics = response["measurements_ms"]
        metric_names = {"deployment_authentication", "observation", "funding", "artifact_installation", "proof_preparation", "local_signing", "submission"}
        if not isinstance(metrics, dict) or not set(metrics) <= metric_names or any(type(value) is not int or not 0 <= value <= 2**64 - 1 for value in metrics.values()):
            raise MetropolisError("invalid payment timing metadata")
        if "funding" in response:
            funding = response["funding"]
            decimal = {"allowance_shortfall", "balance_shortfall", "signer_shortfall"}
            boolean = {"funded_note_available", "gas_limit_sufficient"}
            numeric = {"estimated_gas"}
            if not isinstance(funding, dict) or not set(funding) <= decimal | boolean | numeric:
                raise MetropolisError("unexpected funding metadata")
            for name, value in funding.items():
                valid = (name in decimal and isinstance(value, str) and re.fullmatch(r"0|[1-9][0-9]{0,38}", value, re.ASCII) is not None)
                valid |= name in boolean and type(value) is bool
                valid |= name in numeric and type(value) is int and 0 <= value <= 2**64 - 1
                if not valid:
                    raise MetropolisError("invalid funding diagnostic")
        return response
