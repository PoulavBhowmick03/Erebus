"""A persisted lifetime cap on the charged writes one identity may start.

Every `open_channel`, `propose_offer`, `counter_offer` and `accept_and_settle` is a proved
`apply_actions` transaction that pays the live pool fee from public STRK. The allowance an
operator approved already bounds that spend on chain; this counter refuses the write above
the operator's cap before anything is recorded or proved, so an agent that loops cannot
spend the allowance it was given for other deals.

An operation id is counted once, when it is first claimed. Retrying the same operation id
(the documented recovery for an interrupted write) never counts again. A claimed write that
then fails still counts: the counter cannot know whether Rust reached the chain, and the
Rust journal, not this file, is authoritative about effects.
"""

from __future__ import annotations

import json
import os
import tempfile
from contextlib import suppress
from pathlib import Path

from erebus_mcp.intent import _FileLock

_VERSION = 1


class WriteBudget:
    """``max_writes=None`` never refuses; the counter is still kept for ``status``."""

    def __init__(self, path: Path, max_writes: int | None) -> None:
        self._path = path
        self._lock_path = path.with_name(path.name + ".lock")
        self._max = max_writes

    @property
    def maximum(self) -> int | None:
        return self._max

    def used(self) -> int:
        return len(self._read().get("operations", {}))

    def claim(self, operation_id: str, tool: str) -> str | None:
        """Count ``operation_id`` and return ``None``, or return why it is refused."""
        self._path.parent.mkdir(parents=True, exist_ok=True)
        with _FileLock(self._lock_path):
            data = self._read()
            operations = data.setdefault("operations", {})
            if operation_id in operations:
                return None
            if self._max is not None and len(operations) >= self._max:
                return (
                    f"this identity has started {len(operations)} of its "
                    f"{self._max} operator-allowed charged writes (EREBUS_MAX_WRITES) and "
                    f"{tool} was refused before anything was proved or sent. Do not retry; "
                    "ask the operator to renew this identity's write budget."
                )
            operations[operation_id] = tool
            self._write(data)
            return None

    def _read(self) -> dict:
        try:
            raw = self._path.read_text(encoding="utf-8")
        except FileNotFoundError:
            return {"version": _VERSION, "operations": {}}
        data = json.loads(raw)
        if data.get("version") != _VERSION or not isinstance(data.get("operations"), dict):
            raise ValueError(f"unrecognized write budget file {self._path}")
        return data

    def _write(self, data: dict) -> None:
        descriptor, temporary = tempfile.mkstemp(
            prefix=f".{self._path.name}.", dir=self._path.parent
        )
        try:
            os.chmod(temporary, 0o600)
            with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
                descriptor = -1
                json.dump(data, stream, sort_keys=True)
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(temporary, self._path)
        finally:
            if descriptor >= 0:
                os.close(descriptor)
            with suppress(FileNotFoundError):
                os.unlink(temporary)


def budget_path(intent_state_dir: Path) -> Path:
    """One file per identity, beside its intent records."""
    return intent_state_dir / "write-budget.json"
