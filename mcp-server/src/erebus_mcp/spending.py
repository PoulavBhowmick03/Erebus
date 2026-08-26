"""Spending limits enforced at the MCP layer, below the agent (roadmap 9.1).

The component being constrained must not be the component enforcing the constraint.
``BuyerPolicy.budget`` in ``agents/src/erebus_agents/policy.py`` is agent-owned and stays
that way: it is negotiation strategy, a sane opening anchor, not a safety boundary. The
safety boundary is here, underneath the model, where it cannot be read, reasoned past, or
argued out of by a counterparty's offer text.

Refusal messages never state the configured amount. A cap an agent can read is a cap an
agent can plan around, including by splitting one deal into several that each individually
clear a per-deal cap; the daily cumulative cap is what catches that, so both messages point
at it rather than inviting a smaller retry.

Enforcement is check-then-record, not reserve-then-rollback: ``check`` runs before the seam
call is made, ``record`` runs only once ``accept_and_settle`` has actually succeeded. Two
concurrent settlements on one server could both pass ``check`` before either calls
``record``, which is a narrow, accepted race: nothing upstream of this serializes
proof-bearing writes for one identity yet either (roadmap Phase 3). Tighten this if that
lands first.
"""

from __future__ import annotations

import datetime
import json
import os
import tempfile
from contextlib import suppress
from pathlib import Path

from erebus_mcp.config import SpendingLimits

try:
    import fcntl
except ImportError:  # Windows: no advisory locking. Single-process use only there.
    fcntl = None  # type: ignore[assignment]

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


def _today() -> str:
    return datetime.datetime.now(datetime.timezone.utc).date().isoformat()


class SpendGuard:
    """Persists and enforces per-token per-deal and daily-cumulative spending caps."""

    def __init__(self, limits: SpendingLimits, state_path: Path) -> None:
        self._limits = limits
        self._state_path = state_path
        self._lock_path = state_path.with_name(state_path.name + ".lock")

    def check(self, token: str, amount: int) -> str | None:
        """Returns a refusal reason, or None if `amount` clears every configured cap for
        `token`. Read-only. Call before the seam call is made."""
        cap = self._limits.for_token(token)
        if cap.per_deal is not None and amount > cap.per_deal:
            return _PER_DEAL_MESSAGE
        if cap.daily is not None:
            spent_today = self._read_spent().get(token.strip().lower(), 0)
            if spent_today + amount > cap.daily:
                return _DAILY_MESSAGE
        return None

    def record(self, token: str, amount: int, operation_id: str | None = None) -> None:
        """Adds `amount` to today's cumulative spend for `token`. Call only after the
        settlement it guards has actually succeeded, so a refused or failed attempt never
        counts against the cap."""
        self._state_path.parent.mkdir(parents=True, exist_ok=True)
        self._lock_path.parent.mkdir(parents=True, exist_ok=True)
        with open(self._lock_path, "a") as lock_file:
            if fcntl is not None:
                fcntl.flock(lock_file, fcntl.LOCK_EX)
            try:
                data = self._read_state()
                spent = data["spent"]
                key = token.strip().lower()
                if operation_id is not None:
                    prior = data["operations"].get(operation_id)
                    expected = {"token": key, "amount": str(amount)}
                    if prior is not None:
                        if prior != expected:
                            raise ValueError(
                                f"operation {operation_id} is already bound to a different spend"
                            )
                        return
                    data["operations"][operation_id] = expected
                spent[key] = spent.get(key, 0) + amount
                self._write_state(data)
            finally:
                if fcntl is not None:
                    fcntl.flock(lock_file, fcntl.LOCK_UN)

    def _read_spent(self) -> dict[str, int]:
        return self._read_state()["spent"]

    def _read_state(self) -> dict:
        if not self._state_path.exists():
            return {"spent": {}, "operations": {}}
        raw = self._state_path.read_text().strip()
        if not raw:
            return {"spent": {}, "operations": {}}
        data = json.loads(raw)
        if data.get("date") != _today():
            # Rolling UTC day: yesterday's counters don't apply to today's cap.
            return {"spent": {}, "operations": {}}
        return {
            "spent": {k: int(v) for k, v in data.get("spent", {}).items()},
            "operations": dict(data.get("operations", {})),
        }

    def _write_state(self, data: dict) -> None:
        # Atomic replace, matching the write pattern ARCHITECTURE.md documents for Rust
        # state: a torn write must never be visible to a concurrent reader.
        fd, tmp_name = tempfile.mkstemp(
            dir=self._state_path.parent, prefix=".spending-", suffix=".json"
        )
        try:
            with os.fdopen(fd, "w") as tmp:
                json.dump(
                    {
                        "date": _today(),
                        "spent": {k: str(v) for k, v in data["spent"].items()},
                        "operations": data["operations"],
                    },
                    tmp,
                )
            os.replace(tmp_name, self._state_path)
        except BaseException:
            with suppress(FileNotFoundError):
                os.remove(tmp_name)
            raise
