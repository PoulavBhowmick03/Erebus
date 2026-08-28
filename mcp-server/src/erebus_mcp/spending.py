"""Crash-safe spending reservations enforced below the agent.

The Rust operation journal is authoritative about whether a settlement had an effect and
when its accepted Starknet block was produced. This file owns only the policy projection:
an atomic reservation before Rust starts, retention while the chain outcome is uncertain,
and daily accounting by the chain acceptance timestamp.
"""

from __future__ import annotations

import datetime
import json
import os
import tempfile
from contextlib import contextmanager, suppress
from pathlib import Path
from typing import Iterator

from erebus_mcp.config import SpendingLimits

try:
    import fcntl
except ImportError:  # Windows: no advisory locking. Single-process use only there.
    fcntl = None  # type: ignore[assignment]

_STATE_VERSION = 2
_PER_DEAL_MESSAGE = (
    "this settlement exceeds an operator-configured per-deal spending limit and was "
    "refused. Do not retry at a smaller amount to route around it — a daily cumulative "
    "limit applies across deals too. Contact the operator if this amount is expected."
)
_DAILY_MESSAGE = (
    "this settlement would exceed an operator-configured daily spending limit for this "
    "token and was refused. Do not retry at a smaller amount to route around it. Contact "
    "the operator if this amount is expected."
)


def _utc_date(timestamp: int) -> str:
    return datetime.datetime.fromtimestamp(timestamp, datetime.timezone.utc).date().isoformat()


def _now() -> int:
    return int(datetime.datetime.now(datetime.timezone.utc).timestamp())


class SpendGuard:
    """Persists atomic reservations and chain-authoritative committed spend."""

    def __init__(self, limits: SpendingLimits, state_path: Path) -> None:
        self._limits = limits
        self._state_path = state_path
        self._lock_path = state_path.with_name(state_path.name + ".lock")

    def check(self, token: str, amount: int, *, at: int | None = None) -> str | None:
        """Returns the current policy decision without reserving.

        Production settlement uses :meth:`reserve`, which makes the same decision and
        persists it under one lock. This method is for diagnostics and tests only.
        """
        with self._locked_state() as data:
            return self._denial(data, token, amount, at=_now() if at is None else at)

    def reserve(
        self, token: str, amount: int, operation_id: str, *, at: int | None = None
    ) -> str | None:
        """Atomically checks the caps and reserves ``amount`` for ``operation_id``."""
        key = token.strip().lower()
        with self._locked_state() as data:
            prior = data["operations"].get(operation_id)
            if prior is not None:
                self._assert_binding(prior, key, amount, operation_id)
                return None
            denial = self._denial(data, key, amount, at=_now() if at is None else at)
            if denial is not None:
                return denial
            data["operations"][operation_id] = {
                "token": key,
                "amount": str(amount),
                "status": "reserved",
            }
            self._write_state(data)
            return None

    def observe(
        self,
        token: str,
        amount: int,
        operation_id: str,
        *,
        outcome: str,
        accepted_at: int | None = None,
    ) -> None:
        """Projects one Rust reconciliation fact into the policy ledger.

        ``effect`` commits only with a chain acceptance timestamp. Without it, and for
        ``pending`` or ``unknown``, the amount remains reserved. Proven ``no_effect`` and
        ``reverted`` outcomes release it.
        """
        key = token.strip().lower()
        with self._locked_state() as data:
            prior = data["operations"].get(operation_id)
            if prior is not None:
                self._assert_binding(prior, key, amount, operation_id)

            if outcome in {"no_effect", "reverted"}:
                if prior is not None and prior["status"] == "reserved":
                    del data["operations"][operation_id]
                    self._write_state(data)
                return

            entry = prior or {
                "token": key,
                "amount": str(amount),
                "status": "reserved",
            }
            if outcome == "effect" and accepted_at is not None:
                entry = {**entry, "status": "committed", "accepted_at": str(accepted_at)}
            elif entry.get("status") != "committed":
                entry = {"token": key, "amount": str(amount), "status": "reserved"}
            data["operations"][operation_id] = entry
            self._write_state(data)

    def release_unjournalled(self, journalled_operation_ids: set[str]) -> None:
        """Releases Python-only reservations after an exclusive Rust reconciliation."""
        with self._locked_state() as data:
            stale = [
                operation_id
                for operation_id, entry in data["operations"].items()
                if entry["status"] == "reserved"
                and operation_id not in journalled_operation_ids
            ]
            if not stale:
                return
            for operation_id in stale:
                del data["operations"][operation_id]
            self._write_state(data)

    def release(self, operation_id: str) -> None:
        """Releases one reservation after Rust proves that it had no effect."""
        with self._locked_state() as data:
            entry = data["operations"].get(operation_id)
            if entry is None or entry["status"] != "reserved":
                return
            del data["operations"][operation_id]
            self._write_state(data)

    def reserved_operation_ids(self) -> set[str]:
        with self._locked_state() as data:
            return {
                operation_id
                for operation_id, entry in data["operations"].items()
                if entry["status"] == "reserved"
            }

    def _denial(self, data: dict, token: str, amount: int, *, at: int) -> str | None:
        key = token.strip().lower()
        cap = self._limits.for_token(key)
        if cap.per_deal is not None and amount > cap.per_deal:
            return _PER_DEAL_MESSAGE
        if cap.daily is None:
            return None

        today = _utc_date(at)
        total = int(data["legacy_reserved"].get(key, 0))
        for entry in data["operations"].values():
            if entry["token"] != key:
                continue
            if entry["status"] == "reserved":
                total += int(entry["amount"])
            elif _utc_date(int(entry["accepted_at"])) == today:
                total += int(entry["amount"])
        return _DAILY_MESSAGE if total + amount > cap.daily else None

    @staticmethod
    def _assert_binding(entry: dict, token: str, amount: int, operation_id: str) -> None:
        if entry["token"] != token or int(entry["amount"]) != amount:
            raise ValueError(f"operation {operation_id} is already bound to a different spend")

    @contextmanager
    def _locked_state(self) -> Iterator[dict]:
        self._state_path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        self._lock_path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        with open(self._lock_path, "a", encoding="utf-8") as lock_file:
            os.chmod(self._lock_path, 0o600)
            if fcntl is not None:
                fcntl.flock(lock_file, fcntl.LOCK_EX)
            try:
                yield self._read_state()
            finally:
                if fcntl is not None:
                    fcntl.flock(lock_file, fcntl.LOCK_UN)

    def _read_state(self) -> dict:
        if not self._state_path.exists() or not self._state_path.read_text().strip():
            return {"version": _STATE_VERSION, "operations": {}, "legacy_reserved": {}}
        data = json.loads(self._state_path.read_text())
        if data.get("version") == _STATE_VERSION:
            return {
                "version": _STATE_VERSION,
                "operations": dict(data.get("operations", {})),
                "legacy_reserved": {
                    key: int(value) for key, value in data.get("legacy_reserved", {}).items()
                },
            }
        return self._migrate_v1(data)

    @staticmethod
    def _migrate_v1(data: dict) -> dict:
        operations = {
            operation_id: {
                "token": entry["token"].strip().lower(),
                "amount": str(entry["amount"]),
                "status": "reserved",
            }
            for operation_id, entry in data.get("operations", {}).items()
        }
        attributed: dict[str, int] = {}
        for entry in operations.values():
            token = entry["token"]
            attributed[token] = attributed.get(token, 0) + int(entry["amount"])
        legacy_reserved = {
            token.strip().lower(): max(int(amount) - attributed.get(token.strip().lower(), 0), 0)
            for token, amount in data.get("spent", {}).items()
            if int(amount) > attributed.get(token.strip().lower(), 0)
        }
        return {
            "version": _STATE_VERSION,
            "operations": operations,
            "legacy_reserved": legacy_reserved,
        }

    def _write_state(self, data: dict) -> None:
        fd, tmp_name = tempfile.mkstemp(
            dir=self._state_path.parent, prefix=".spending-", suffix=".json"
        )
        try:
            os.fchmod(fd, 0o600)
            with os.fdopen(fd, "w", encoding="utf-8") as tmp:
                json.dump(data, tmp, sort_keys=True)
                tmp.flush()
                os.fsync(tmp.fileno())
            os.replace(tmp_name, self._state_path)
            directory = os.open(self._state_path.parent, os.O_RDONLY)
            try:
                os.fsync(directory)
            finally:
                os.close(directory)
        except BaseException:
            with suppress(FileNotFoundError):
                os.remove(tmp_name)
            raise
