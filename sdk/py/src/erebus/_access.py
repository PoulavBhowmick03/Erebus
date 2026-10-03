"""Path-only buyer access. Rust signs locally and verifies resource bytes."""

from __future__ import annotations

import json
import re
import shutil
import subprocess
from pathlib import Path
from typing import Any


class AccessError(RuntimeError):
    """Retrieval failed. Never authorize another payment from this error."""


class AccessSeam:
    """Forward local paths; return content metadata, not private keys or resource contents."""

    def __init__(self, binary: str | Path | None = None, timeout: int = 120) -> None:
        resolved = str(binary) if binary is not None else shutil.which("erebus-access")
        if not resolved or type(timeout) is not int or not 1 <= timeout <= 600:
            raise AccessError("configure an access binary and bounded timeout")
        self._binary = resolved
        self._timeout = timeout

    def _run(self, request: dict[str, Any] | None) -> tuple[int, dict[str, Any]]:
        try:
            output = subprocess.run(
                [self._binary] + (["--version"] if request is None else []),
                input=None if request is None else json.dumps(request),
                capture_output=True, text=True, timeout=self._timeout, check=False,
            )
        except (OSError, subprocess.TimeoutExpired):
            raise AccessError("retrieval unavailable; retain state and retry without another payment") from None
        if output.returncode not in {0, 2} or output.stderr or len(output.stdout) > 16_384:
            raise AccessError("retrieval failed; retain state and retry without another payment")
        try:
            response = json.loads(output.stdout)
        except (ValueError, TypeError):
            raise AccessError("invalid access response") from None
        if not isinstance(response, dict):
            raise AccessError("invalid access response")
        return output.returncode, response

    def version(self) -> dict[str, Any]:
        """Check the binary protocol without reading keys or accessing the service."""
        code, response = self._run(None)
        if code != 0 or response != {"status": "ok", "protocol": 1, "methods": ["retrieve"]} or type(response["protocol"]) is not int:
            raise AccessError("access binary protocol mismatch")
        return response

    def retrieve(self, *, evidence_file: str, buyer_key_file: str, service_url: str,
                 service_id: str, cache_root: str, allow_loopback_http: bool = False) -> dict[str, Any]:
        """Authenticate locally and retrieve once. Pending is not verified payment."""
        if (
            any(not isinstance(path, str) or not path or len(path) > 4096 for path in (evidence_file, buyer_key_file, cache_root, service_url))
            or not isinstance(service_id, str) or not re.fullmatch(r"[a-f0-9]{64}", service_id)
            or service_id == "0" * 64 or type(allow_loopback_http) is not bool
        ):
            raise AccessError("invalid access request")
        code, response = self._run({
            "method": "retrieve", "evidence_file": evidence_file, "buyer_key_file": buyer_key_file,
            "service_url": service_url, "service_id": service_id, "cache_root": cache_root,
            "allow_loopback_http": allow_loopback_http,
        })
        if code == 2:
            status = response.get("status")
            if not isinstance(status, str) or status not in {"pending", "paid_but_undelivered"}:
                raise AccessError("invalid recoverable access response")
            expected = {"status": status, "retry_without_payment": True, "payment_verified": False,
                        "resource_verified": False, "delivery_verified": False}
            if status == "paid_but_undelivered":
                expected["seller_reported_payment_finalized"] = True
            if response != expected or any(type(response[field]) is not bool for field in expected if field != "status"):
                raise AccessError("invalid pending access response")
            return response
        if set(response) != {"status", "result"} or response["status"] != "retrieved" or not isinstance(response["result"], dict):
            raise AccessError("invalid retrieval response")
        result = response["result"]
        fields = {"deal_commitment", "issuance_id", "resource_sha256", "resource_bytes", "resource_file", "cached",
                  "seller_reported_payment_finalized", "payment_verified", "resource_verified", "delivery_verified"}
        if set(result) != fields:
            raise AccessError("unexpected retrieval response fields")
        if (
            any(not isinstance(result[name], str) or not re.fullmatch(r"[a-f0-9]{64}", result[name]) for name in ("deal_commitment", "issuance_id", "resource_sha256"))
            or type(result["resource_bytes"]) is not int or not 0 <= result["resource_bytes"] <= 1024 * 1024
            or not isinstance(result["resource_file"], str) or not 1 <= len(result["resource_file"]) <= 4096
            or any(type(result[name]) is not bool for name in ("cached", "seller_reported_payment_finalized", "payment_verified", "resource_verified", "delivery_verified"))
            or not result["seller_reported_payment_finalized"] or result["payment_verified"]
            or not result["resource_verified"] or result["delivery_verified"]
        ):
            raise AccessError("invalid retrieval metadata or verification claims")
        return response
