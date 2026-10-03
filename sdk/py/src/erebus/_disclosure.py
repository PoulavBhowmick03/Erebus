"""Path-only subprocess binding for Metropolis deal disclosure. No cryptography."""

from __future__ import annotations

import json
import shutil
import subprocess
from pathlib import Path
from typing import Any


class DisclosureError(RuntimeError):
    """Disclosure failed; this does not establish payment or non-payment."""


class DisclosureSeam:
    """Run a local disclosure binary. Python passes paths and never opens key files."""

    _parameters = {
        "version": set(),
        "keygen": {"key_file"},
        "key_info": {"key_file"},
        "select": {"state_root", "operation_ref", "store_root", "namespace",
                   "transcript_hash_version", "evidence_file"},
        "export": {"evidence_file", "issuer_key_file", "recipient_public_key",
                   "grant_file", "expires_at"},
        "verify_agreement": {"grant_file", "key_file", "expected_issuer"},
        "verify_payment": {"grant_file", "key_file", "expected_issuer", "deployment"},
    }
    _public_fields = {
        "status", "protocol", "methods", "mode", "recipient_public_key", "evidence_saved",
        "grant_saved", "deal_id", "revision", "deal_commitment", "deal_nullifier", "issuer",
        "agreement_verified", "payment_verified", "delivery_verified",
        "next_log_block", "ancestry_block",
    }

    def __init__(self, binary: str | Path | None = None, timeout: int = 60) -> None:
        resolved = str(binary) if binary is not None else shutil.which("erebus-disclosure")
        if not resolved or timeout <= 0:
            raise DisclosureError("configure a disclosure binary and a positive timeout")
        self._binary = resolved
        self._timeout = timeout

    def call(self, method: str, **parameters: Any) -> dict[str, Any]:
        """Forward one typed request; reject unexpected output rather than exposing it."""
        if method not in self._parameters or set(parameters) != self._parameters[method]:
            raise DisclosureError("invalid disclosure request fields")
        request = {"method": method, **parameters}
        try:
            completed = subprocess.run(
                [self._binary], input=json.dumps(request), capture_output=True,
                text=True, timeout=self._timeout, check=False,
            )
        except (OSError, subprocess.TimeoutExpired):
            raise DisclosureError("disclosure process unavailable; retain evidence and retry") from None
        try:
            response = json.loads(completed.stdout)
        except (ValueError, TypeError):
            raise DisclosureError("invalid disclosure response") from None
        pending = (
            isinstance(response, dict) and method == "verify_payment"
            and response.get("status") == "pending" and completed.returncode == 2
        )
        if not pending and (
            completed.returncode != 0 or not isinstance(response, dict)
            or response.get("status") != "ok"
        ):
            raise DisclosureError("disclosure verification or storage failed; retain evidence")
        if completed.stderr or set(response) - self._public_fields:
            raise DisclosureError("unexpected disclosure response fields")
        # No nested plaintext, key material, or private evidence belongs in a model result.
        for name, value in response.items():
            if name == "methods":
                if not isinstance(value, list) or any(
                    not isinstance(item, str) or item not in self._parameters for item in value
                ):
                    raise DisclosureError("invalid disclosure capabilities")
            elif type(value) not in (str, int, bool) or (isinstance(value, str) and len(value) > 256):
                raise DisclosureError("invalid disclosure response shape")
        if method in {"verify_agreement", "verify_payment"} and (
            response.get("agreement_verified") is not True
            or response.get("payment_verified") is not (method == "verify_payment" and not pending)
            or response.get("delivery_verified") is not False
        ):
            raise DisclosureError("invalid disclosure verification claims")
        if pending and (
            type(response.get("ancestry_block")) is not int
            or not 0 <= response["ancestry_block"] <= 2**64 - 1
            or ("next_log_block" in response and (
                type(response["next_log_block"]) is not int
                or not 0 <= response["next_log_block"] <= 2**64 - 1
            ))
        ):
            raise DisclosureError("invalid disclosure observation progress")
        if not pending and {"next_log_block", "ancestry_block"} & response.keys():
            raise DisclosureError("unexpected disclosure observation progress")
        return response
