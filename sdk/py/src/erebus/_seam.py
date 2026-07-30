"""The subprocess seam to ``erebus-cli``.

Decided 2026-07-30 over PyO3 (ARCHITECTURE §3). Everything in this module is transport:
build a JSON request, run the binary, parse one JSON envelope, raise or return.

**Nothing here computes anything.** No hashing, no felt arithmetic, no salt encoding, no
entropy. If a change to this file needs a known-answer test, the change is wrong — see the
tripwire in ``erebus/__init__.py``.

Key material never passes through this process. Requests carry a *path* to a key file; the
Rust binary opens it. That is the reason subprocess won: the agent's Python heap, where
model-driven and third-party framework code runs, never holds a pool private key at all.
"""

from __future__ import annotations

import json
import shutil
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Any

__all__ = ["ErebusError", "Seam", "SeamUnavailable"]

#: How long a call may take. Generous because proving is ~29 s per transaction and the
#: binary may be doing one; short enough that a hung child does not hang an agent forever.
DEFAULT_TIMEOUT_SECONDS = 120


class SeamUnavailable(RuntimeError):
    """The ``erebus-cli`` binary could not be found or could not be run."""


@dataclass(frozen=True)
class ErebusError(Exception):
    """A structured failure from the Rust client.

    ``code`` is a ``SettlementErrorCode`` (ARCHITECTURE §4). ``retryable`` is the only
    field agent logic should branch on — an agent cannot sensibly act on twelve distinct
    codes, but it can always act on "is another attempt worth making".
    """

    code: str
    message: str
    retryable: bool

    def __str__(self) -> str:
        return f"{self.code}: {self.message}"


class Seam:
    """Runs ``erebus-cli``, one request per invocation.

    :param binary: path to ``erebus-cli``. Defaults to whatever is on ``PATH``.
    :param timeout: seconds before a call is abandoned.
    """

    def __init__(
        self,
        binary: str | Path | None = None,
        timeout: int = DEFAULT_TIMEOUT_SECONDS,
    ) -> None:
        resolved = str(binary) if binary else shutil.which("erebus-cli")
        if not resolved:
            raise SeamUnavailable(
                "erebus-cli not found on PATH. Build it with "
                "`cargo build --release --bin erebus-cli` and pass its path, or put it "
                "on PATH."
            )
        self._binary = resolved
        self._timeout = timeout

    def call(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        """Invokes ``method`` and returns its result.

        :raises ErebusError: the Rust side returned a structured failure.
        :raises SeamUnavailable: the binary could not be run, or answered with something
            that is not a single JSON envelope. That is a broken install rather than a
            protocol error, so it is deliberately a different exception type.
        """
        request: dict[str, Any] = {"method": method}
        if params is not None:
            request["params"] = params

        try:
            completed = subprocess.run(
                [self._binary],
                input=json.dumps(request),
                capture_output=True,
                text=True,
                timeout=self._timeout,
                check=False,
            )
        except OSError as exc:
            raise SeamUnavailable(f"could not run {self._binary}: {exc}") from exc
        except subprocess.TimeoutExpired as exc:
            raise SeamUnavailable(
                f"{method} exceeded {self._timeout}s"
            ) from exc

        try:
            envelope = json.loads(completed.stdout)
        except json.JSONDecodeError as exc:
            # Exit status is deliberately not consulted here: the envelope is
            # authoritative, and a non-JSON stdout means the binary is not the one we
            # think it is.
            raise SeamUnavailable(
                f"{method} did not return a JSON envelope "
                f"(exit {completed.returncode}): {completed.stdout!r}{completed.stderr!r}"
            ) from exc

        if envelope.get("ok"):
            return envelope.get("result", {})

        error = envelope.get("error") or {}
        raise ErebusError(
            code=error.get("code", "PROOF_FAILED"),
            message=error.get("message", "the client returned no detail"),
            retryable=bool(error.get("retryable", False)),
        )

    def version(self) -> dict[str, Any]:
        """Liveness check. Touches no key material, so it is safe to call at startup."""
        return self.call("version")

    def open_channel(
        self,
        *,
        address: str,
        key_file: str | Path,
        counterparty_address: str,
        counterparty_public_key: str,
        token: str,
        channel_index: int = 0,
        subchannel_index: int = 0,
        register: bool = False,
    ) -> dict[str, Any]:
        """Derives a channel and builds its setup action set.

        ``key_file`` is a **path**, never a key. Passing key material through this function
        would defeat the reason this seam is a subprocess.
        """
        return self.call(
            "open_channel",
            {
                "address": address,
                "key_file": str(key_file),
                "counterparty_address": counterparty_address,
                "counterparty_public_key": counterparty_public_key,
                "token": token,
                "channel_index": channel_index,
                "subchannel_index": subchannel_index,
                "register": register,
            },
        )
