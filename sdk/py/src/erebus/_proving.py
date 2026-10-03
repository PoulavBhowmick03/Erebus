"""Path-only local proving binding. Private witness files are opened only by Rust."""

from __future__ import annotations

import json
import re
import shutil
import subprocess
from pathlib import Path
from typing import Any


class LocalProvingError(RuntimeError):
    """Local proving failed. This never establishes payment or delivery."""


class LocalProvingSeam:
    """Install public artifacts and prove locally, without implementing cryptography in Python."""

    def __init__(self, binary: str | Path | None = None, timeout: int = 600) -> None:
        resolved = str(binary) if binary is not None else shutil.which("erebus-local-prove")
        if not resolved or type(timeout) is not int or not 1 <= timeout <= 600:
            raise LocalProvingError("configure a local proving binary and a bounded timeout")
        self._binary = resolved
        self._timeout = timeout

    def _run(self, request: dict[str, Any] | None) -> dict[str, Any]:
        try:
            output = subprocess.run(
                [self._binary] + (["--version"] if request is None else []),
                input=None if request is None else json.dumps(request),
                capture_output=True, text=True, timeout=self._timeout, check=False,
            )
        except (OSError, subprocess.TimeoutExpired):
            raise LocalProvingError("local proving unavailable; retain the witness and retry") from None
        if output.returncode != 0 or output.stderr or len(output.stdout) > 16_384:
            raise LocalProvingError("local proving or artifact installation failed")
        try:
            response = json.loads(output.stdout)
        except (ValueError, TypeError):
            raise LocalProvingError("invalid local proving response") from None
        if not isinstance(response, dict) or response.get("status") != "ok":
            raise LocalProvingError("invalid local proving response")
        return response

    def version(self) -> dict[str, Any]:
        """Check binary protocol without reading keys or witnesses."""
        response = self._run(None)
        if response != {"status": "ok", "protocol": 1, "methods": ["prove"]} or type(response["protocol"]) is not int:
            raise LocalProvingError("local proving binary protocol mismatch")
        return response

    def prove(
        self, *, manifest_file: str, manifest_sha256: str, circuit: str,
        cache_root: str, witness_file: str, expected_public: list[str],
        allow_test_artifacts: bool = False, allow_loopback_http: bool = False,
    ) -> dict[str, Any]:
        """Forward local paths; return only public proof coordinates and measurements."""
        count = {"deposit": 6, "transfer": 11, "withdraw": 8}.get(circuit) if isinstance(circuit, str) else None
        if (
            count is None or not isinstance(expected_public, list) or len(expected_public) != count
            or any(not isinstance(value, str) or not re.fullmatch(r"0|[1-9][0-9]{0,77}", value) for value in expected_public)
            or not isinstance(manifest_sha256, str) or not re.fullmatch(r"[a-f0-9]{64}", manifest_sha256)
            or type(allow_test_artifacts) is not bool or type(allow_loopback_http) is not bool
            or any(not isinstance(path, str) or not path or len(path) > 4096 for path in (manifest_file, cache_root, witness_file))
        ):
            raise LocalProvingError("invalid local proving request")
        response = self._run({
            "manifest_file": manifest_file, "manifest_sha256": manifest_sha256, "circuit": circuit,
            "cache_root": cache_root, "witness_file": witness_file, "expected_public": expected_public,
            "allow_test_artifacts": allow_test_artifacts, "allow_loopback_http": allow_loopback_http,
        })
        if set(response) != {"status", "calldata", "downloaded_bytes", "cache_hits", "installation_ms", "proving_ms"}:
            raise LocalProvingError("unexpected local proving response fields")
        for name, maximum in (("downloaded_bytes", 3 * 256 * 1024 * 1024), ("cache_hits", 3), ("installation_ms", 2**64 - 1), ("proving_ms", 2**64 - 1)):
            if type(response[name]) is not int or not 0 <= response[name] <= maximum:
                raise LocalProvingError("invalid local proving measurements")
        calldata = response["calldata"]
        if not isinstance(calldata, list) or len(calldata) != 4:
            raise LocalProvingError("invalid public proof shape")
        a, b, c, signals = calldata
        if (
            not isinstance(a, list) or len(a) != 2 or not isinstance(c, list) or len(c) != 2
            or not isinstance(b, list) or len(b) != 2
            or any(not isinstance(pair, list) or len(pair) != 2 for pair in b)
            or signals != expected_public
            or any(not isinstance(value, str) or not re.fullmatch(r"0|[1-9][0-9]{0,77}", value) for value in a + b[0] + b[1] + c)
        ):
            raise LocalProvingError("invalid public proof shape or expected transition")
        return response
