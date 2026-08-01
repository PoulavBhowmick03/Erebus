"""The subprocess seam to ``erebus-cli``.

Decided 2026-07-30 over PyO3 (ARCHITECTURE §3). Everything in this module is transport:
build a JSON request, run the binary, parse one JSON envelope, raise or return.

**Nothing here computes anything.** No hashing, no felt arithmetic, no salt encoding, no
entropy. If a change to this file needs a known-answer test, the change is wrong — see the
tripwire in ``erebus/__init__.py``.

Key material never passes through this process. :class:`SeamConfig` carries *paths* to two
key files; the Rust binary opens them. That is the reason subprocess won: the agent's Python
heap, where model-driven and third-party framework code runs, never holds a pool private key
at all.

Protocol 2, 2026-08-01. Every method except ``version`` and ``generate_pool_key`` carries
the same nine-field config block, because ``erebus-cli`` is one-shot and holds nothing
between invocations. Channel state lives in ``state_dir`` and is addressed by an opaque
handle.
"""

from __future__ import annotations

import json
import shutil
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Any

__all__ = ["ErebusError", "Seam", "SeamConfig", "SeamUnavailable"]

#: How long a call may take. A write is a preflight, a proof (~20 s), a fee estimate, a
#: submission and a receipt wait, so this is generous. Short enough that a hung child does
#: not hang an agent forever.
DEFAULT_TIMEOUT_SECONDS = 300


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


@dataclass(frozen=True)
class SeamConfig:
    """Operator configuration, re-sent on every call.

    The two ``*_key_file`` fields are **paths**. Putting a key value here would defeat the
    reason this seam is a subprocess, and no code path accepts one.

    ``rpc_url`` deserves care. The ``compile_actions`` preflight sends the pool private key
    as calldata, so a public third-party RPC sees it. Acceptable for a throwaway testnet
    identity, not for anything else.
    """

    rpc_url: str
    prover_url: str
    pool_address: str
    chain_id: str
    account_address: str
    pool_key_file: str | Path
    account_key_file: str | Path
    state_dir: str | Path
    token: str

    def as_params(self) -> dict[str, str]:
        return {
            "rpc_url": self.rpc_url,
            "prover_url": self.prover_url,
            "pool_address": self.pool_address,
            "chain_id": self.chain_id,
            "account_address": self.account_address,
            "pool_key_file": str(self.pool_key_file),
            "account_key_file": str(self.account_key_file),
            "state_dir": str(self.state_dir),
            "token": self.token,
        }


class Seam:
    """Runs ``erebus-cli``, one request per invocation.

    :param config: operator configuration attached to every protocol call.
    :param binary: path to ``erebus-cli``. Defaults to whatever is on ``PATH``.
    :param timeout: seconds before a call is abandoned.
    """

    def __init__(
        self,
        config: SeamConfig | None = None,
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
        self._config = config

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
            raise SeamUnavailable(f"{method} exceeded {self._timeout}s") from exc

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

    def _with_config(self, params: dict[str, Any]) -> dict[str, Any]:
        if self._config is None:
            raise SeamUnavailable(
                "this Seam was constructed without a SeamConfig, so it can only call "
                "version() and generate_pool_key()"
            )
        return {"config": self._config.as_params(), **params}

    # --- Calls that carry no operator configuration -------------------------------

    def version(self) -> dict[str, Any]:
        """Liveness check. Touches no key material, so it is safe to call at startup."""
        return self.call("version")

    def generate_pool_key(self, path: str | Path) -> dict[str, Any]:
        """Creates a pool identity key file and returns its path and public half.

        The private value is never returned and never crosses this process. Entropy comes
        from the Rust binary: a key generated here would be a cryptographic decision taken
        in the wrong place.
        """
        return self.call("generate_pool_key", {"path": str(path)})

    # --- The seven interface methods, plus the administrative shield ---------------

    def open_channel(self, counterparty: str) -> dict[str, Any]:
        return self.call("open_channel", self._with_config({"counterparty": counterparty}))

    def propose_offer(self, handle: str, terms: dict[str, Any]) -> dict[str, Any]:
        return self.call("propose_offer", self._with_config({"handle": handle, "terms": terms}))

    def counter_offer(
        self, handle: str, reply_to: str, terms: dict[str, Any]
    ) -> dict[str, Any]:
        return self.call(
            "counter_offer",
            self._with_config({"handle": handle, "reply_to": reply_to, "terms": terms}),
        )

    def read_channel_state(self, handle: str) -> dict[str, Any]:
        return self.call("read_channel_state", self._with_config({"handle": handle}))

    def accept_and_settle(self, handle: str, offer_id: str) -> dict[str, Any]:
        return self.call(
            "accept_and_settle", self._with_config({"handle": handle, "offer_id": offer_id})
        )

    def grant_viewing_key(self, handle: str, grantee: str) -> dict[str, Any]:
        return self.call(
            "grant_viewing_key", self._with_config({"handle": handle, "grantee": grantee})
        )

    def reveal(self, viewing_key: dict[str, Any]) -> dict[str, Any]:
        """Reconstructs a record from a bearer grant.

        The grant travels through opaquely. Reading or reshaping it here would make this
        package a second opinion on the disclosure format.
        """
        return self.call("reveal", self._with_config({"viewing_key": viewing_key}))

    def shield(self, amount: str) -> dict[str, Any]:
        """Administrative funding helper. Outside the seven negotiation methods."""
        return self.call("shield", self._with_config({"amount": amount}))
