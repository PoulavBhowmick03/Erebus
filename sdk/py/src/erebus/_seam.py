"""Subprocess binding to ``erebus-cli``.

The project chose this design over PyO3 on 2026-07-30 (ARCHITECTURE §3). This module builds
a JSON request, runs the binary, and parses one JSON envelope.

This module contains no hashing, felt arithmetic, salt encoding, or entropy generation. A
known-answer test here means that protocol logic crossed the binding boundary. See
``erebus/__init__.py``.

:class:`SeamConfig` contains paths to two key files. The Rust binary opens them. The Python
heap never holds a pool private key.

Protocol 4, 2026-08-26. Every method except ``version`` and ``generate_pool_key`` carries
the same configuration because ``erebus-cli`` keeps no process state. Channel
state lives in ``state_dir`` behind an opaque handle.
"""

from __future__ import annotations

import json
import shutil
import subprocess
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from erebus._network import Network, network_preset

__all__ = ["ErebusError", "Seam", "SeamConfig", "SeamUnavailable"]

#: A write includes preflight, a ~20 s proof, fee estimation, submission, and receipt polling.
DEFAULT_TIMEOUT_SECONDS = 300

#: The request/response contract this binding speaks. The binary reports its own on every
#: envelope; ``call`` refuses a mismatch by name instead of failing on a changed shape.
PROTOCOL = 4


class SeamUnavailable(RuntimeError):
    """The ``erebus-cli`` binary could not be found or could not be run."""


@dataclass(frozen=True)
class ErebusError(Exception):
    """Structured failure from the Rust client.

    ``code`` is a ``SettlementErrorCode`` (ARCHITECTURE §4). ``retryable`` is the only
    field for retry decisions. Agents do not need separate retry logic for every code.
    """

    code: str
    message: str
    retryable: bool

    def __str__(self) -> str:
        return f"{self.code}: {self.message}"


@dataclass(frozen=True)
class SeamConfig:
    """Operator configuration, re-sent on every call.

    The two ``*_key_file`` fields are paths. No code path accepts key values.

    The ``compile_actions`` preflight sends the pool private key to ``rpc_url`` as calldata.
    Use an operator-controlled endpoint outside throwaway testnet use.
    """

    rpc_url: str = field(repr=False)
    prover_url: str = field(repr=False)
    pool_address: str
    chain_id: str
    account_address: str
    pool_key_file: str | Path
    account_key_file: str | Path
    state_dir: str | Path
    token: str
    wire_version: str = "v3"

    @classmethod
    def for_network(
        cls,
        network: Network | str,
        *,
        rpc_url: str,
        prover_url: str,
        account_address: str,
        pool_key_file: str | Path,
        account_key_file: str | Path,
        state_dir: str | Path,
        token: str,
        wire_version: str = "v3",
    ) -> SeamConfig:
        """Build a configuration with the canonical chain and pool for ``network``."""

        preset = network_preset(network)
        return cls(
            rpc_url=rpc_url,
            prover_url=prover_url,
            pool_address=preset.pool_address,
            chain_id=preset.chain_id,
            account_address=account_address,
            pool_key_file=pool_key_file,
            account_key_file=account_key_file,
            state_dir=state_dir,
            token=token,
            wire_version=wire_version,
        )

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
            "wire_version": self.wire_version,
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

    def call(self, method: str, params: dict[str, Any] | None = None) -> Any:
        """Invokes ``method`` and returns its result.

        :raises ErebusError: the Rust side returned a structured failure.
        :raises SeamUnavailable: the binary could not be run, or answered with something
            that is not a single JSON envelope. That is a broken install rather than a
            protocol error, so it uses a different exception type.
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
            # The envelope is authoritative. Non-JSON stdout indicates the wrong or broken
            # binary, regardless of exit status.
            raise SeamUnavailable(
                f"{method} did not return a JSON envelope "
                f"(exit {completed.returncode}): {completed.stdout!r}{completed.stderr!r}"
            ) from exc

        # Envelopes carry the contract version they speak. A mismatch means the binary and
        # this binding were installed from different releases; failing here, by name, beats
        # the alternative — a shape error deep inside whichever field changed. Envelopes
        # from binaries that predate the field pass, because protocol 2 omitted it.
        spoken = envelope.get("protocol", PROTOCOL)
        if spoken != PROTOCOL:
            raise SeamUnavailable(
                f"{self._binary} speaks seam protocol {spoken}, this binding speaks "
                f"{PROTOCOL}. Install erebus-cli and erebus-sdk from the same release, "
                "and restart any long-running server that spawned this binding."
            )

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
        """Checks liveness without reading key material."""
        return self.call("version")

    def generate_pool_key(self, path: str | Path) -> dict[str, Any]:
        """Creates a pool identity key file and returns its path and public half.

        The private value stays on disk. The Rust binary supplies the entropy.
        """
        return self.call("generate_pool_key", {"path": str(path)})

    # --- The seven interface methods, plus administrative/read helpers --------------

    def open_channel(self, operation_id: str, counterparty: str) -> dict[str, Any]:
        return self.call(
            "open_channel",
            self._with_config({"operation_id": operation_id, "counterparty": counterparty}),
        )

    def propose_offer(
        self, operation_id: str, handle: str, terms: dict[str, Any]
    ) -> dict[str, Any]:
        return self.call(
            "propose_offer",
            self._with_config(
                {"operation_id": operation_id, "handle": handle, "terms": terms}
            ),
        )

    def counter_offer(
        self, operation_id: str, handle: str, reply_to: str, terms: dict[str, Any]
    ) -> dict[str, Any]:
        return self.call(
            "counter_offer",
            self._with_config(
                {
                    "operation_id": operation_id,
                    "handle": handle,
                    "reply_to": reply_to,
                    "terms": terms,
                }
            ),
        )

    def read_channel_state(self, handle: str) -> dict[str, Any]:
        return self.call("read_channel_state", self._with_config({"handle": handle}))

    def balance(self) -> dict[str, Any]:
        """Returns note denominations from Rust without computing them in Python."""
        return self.call("balance", self._with_config({}))

    def accept_and_settle(
        self, operation_id: str, handle: str, offer_id: str
    ) -> dict[str, Any]:
        return self.call(
            "accept_and_settle",
            self._with_config(
                {"operation_id": operation_id, "handle": handle, "offer_id": offer_id}
            ),
        )

    def grant_viewing_key(
        self, handle: str, deal_id: str, grantee: str, expires_at: int
    ) -> dict[str, Any]:
        return self.call(
            "grant_viewing_key",
            self._with_config(
                {
                    "handle": handle,
                    "deal_id": deal_id,
                    "grantee": grantee,
                    "expires_at": expires_at,
                }
            ),
        )

    def reveal(self, viewing_key: dict[str, Any]) -> dict[str, Any]:
        """Reconstructs a record from a bearer grant.

        This method passes the grant without reading or changing its format.
        """
        return self.call("reveal", self._with_config({"viewing_key": viewing_key}))

    def shield(self, operation_id: str, amount: str) -> dict[str, Any]:
        """Administrative funding helper. Outside the seven negotiation methods."""
        return self.call(
            "shield", self._with_config({"operation_id": operation_id, "amount": amount})
        )

    # --- Operator health and repair -------------------------------------------------

    def doctor(self) -> dict[str, Any]:
        """Inspects the operator configuration and returns what blocks a write.

        Returns ``ready``, ``checks``, and ``repairs``. A report full of faults is still a
        successful call: ``ok:false`` is reserved for the inspection itself failing, so a
        caller can tell "I looked and found problems" apart from "I could not look".

        Each unhealthy check carries a ``repair`` string naming one direct action.
        """
        return self.call("doctor", self._with_config({}))

    def allowance(self) -> dict[str, Any]:
        """Reads the pool's ERC-20 allowance and the per-write fee.

        Both values are decimal strings. `apply_actions` charges a fee before applying
        anything, so an exhausted allowance reverts with a bare `Contract error`.
        """
        return self.call("allowance", self._with_config({}))

    def approve(self, operation_id: str, amount: str) -> dict[str, Any]:
        """Grants the pool an ERC-20 allowance. Submits a transaction and costs gas.

        `amount` is a decimal string for the same reason every other amount is: a u128 does
        not survive a JSON number.
        """
        return self.call(
            "approve", self._with_config({"operation_id": operation_id, "amount": amount})
        )

    def reconcile(self) -> list[dict[str, Any]]:
        """Classify every durable Rust operation without changing chain or local state."""
        return self.call("reconcile", self._with_config({}))

    def resume_operation(self, operation_id: str) -> dict[str, Any]:
        """Explicitly resume or finish one reconciled operation."""
        return self.call(
            "resume_operation", self._with_config({"operation_id": operation_id})
        )

    def rebuild_state(self) -> dict[str, Any]:
        """Add missing channel records reconstructed from chain data."""
        return self.call("rebuild_state", self._with_config({}))
