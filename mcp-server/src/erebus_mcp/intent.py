"""Durable caller intent: persist an operation's identity and parameters before the write
reaches the seam, so a crash mid-call leaves a record instead of silence (plan.md, Ishita
task 1).

Scope, stated precisely so this isn't mistaken for more than it is: this module answers
"did this process attempt this call, and with which exact parameters" for a process-level
crash. It does not talk to the chain and it is not yet consulted by Rust. Whether a write
actually landed is what the Rust journal and `reconcile()` answer (plan.md, decisions 4-5),
and reaching that from Python needs `operation_id` threaded through the seam, which needs
protocol 4 (plan.md, Poulav task 10) — still blocked on this module existing above `sdk/py`.
Until then, a record left on disk after a crash is evidence for an operator or a future
recovery tool to reconcile by hand; nothing here resubmits or infers success.

Decision 1 (plan.md): operation IDs are caller supplied, ``op_`` followed by 64 lowercase
hex characters. Decision 3: the Python binding stays mechanical and never generates one —
this module is the caller above it that does.
"""

from __future__ import annotations

import json
import os
import secrets
import tempfile
from contextlib import suppress
from dataclasses import dataclass
from pathlib import Path
from typing import Any

try:
    import fcntl
except ImportError:  # Windows: no advisory locking. Single-process use only there.
    fcntl = None  # type: ignore[assignment]

_ID_BYTES = 32  # secrets.token_hex(32) -> 64 lowercase hex characters.


def new_operation_id() -> str:
    """A fresh ``op_`` + 64 lowercase hex character id (plan.md decision 1)."""
    return "op_" + secrets.token_hex(_ID_BYTES)


def _canonical(tool: str, params: dict[str, Any]) -> str:
    """Two calls agree on this string only if they are the same request."""
    return json.dumps({"tool": tool, "params": params}, sort_keys=True, separators=(",", ":"))


@dataclass(frozen=True)
class IntentRecord:
    operation_id: str
    tool: str
    params: dict[str, Any]


class IntentStore:
    """One mode-0600 file per in-flight operation, under ``state_dir/pending_operations``.

    ``begin`` must complete before the underlying seam call starts (decision 2: "a crash
    before persistence produces no call"). ``resolve`` removes the record once the call has
    returned to this process — success or a caught, definite error — because that return is
    itself proof the process did not crash; only a killed process leaves a record behind.
    A record still on disk when a new ``IntentStore`` is built over the same directory means
    a previous process began a call and this one never learned how it ended.
    """

    def __init__(self, state_dir: Path) -> None:
        self._dir = state_dir / "pending_operations"
        self._lock_path = state_dir / "pending_operations.lock"

    def begin(self, tool: str, params: dict[str, Any]) -> str:
        """Returns an operation id for this exact ``(tool, params)`` pair.

        Reuses a still-pending id for an identical request rather than minting a new one,
        so a caller retrying after its own restart — before persistence completed, or
        between persistence and the prior call returning — converges on one id instead of
        leaking a fresh one per attempt. This is the "reuse that ID after a restart" half
        of the task.
        """
        self._dir.mkdir(parents=True, exist_ok=True)
        target = _canonical(tool, params)
        with self._locked():
            for record in self._read_all():
                if _canonical(record.tool, record.params) == target:
                    return record.operation_id
            operation_id = new_operation_id()
            self._write(IntentRecord(operation_id, tool, params))
            return operation_id

    def resolve(self, operation_id: str) -> None:
        """Removes the record. Idempotent: resolving twice, or an id never persisted, is
        not an error — the caller is declaring the uncertainty window closed, and it may
        already be closed."""
        with suppress(FileNotFoundError):
            os.remove(self._path_for(operation_id))

    def pending(self) -> list[IntentRecord]:
        """Every record still on disk: operations a previous process began that nothing
        has resolved. Read-only, matching decision 5 — nothing here submits, retries, or
        classifies anything; a caller decides what to do with this list."""
        return self._read_all()

    def _locked(self):
        self._lock_path.parent.mkdir(parents=True, exist_ok=True)
        return _FileLock(self._lock_path)

    def _path_for(self, operation_id: str) -> Path:
        return self._dir / f"{operation_id}.json"

    def _read_all(self) -> list[IntentRecord]:
        if not self._dir.is_dir():
            return []
        records = []
        for entry in sorted(self._dir.glob("*.json")):
            raw = entry.read_text(encoding="utf-8").strip()
            if not raw:
                continue
            data = json.loads(raw)
            records.append(IntentRecord(data["operation_id"], data["tool"], data["params"]))
        return records

    def _write(self, record: IntentRecord) -> None:
        path = self._path_for(record.operation_id)
        encoded = json.dumps(
            {"operation_id": record.operation_id, "tool": record.tool, "params": record.params},
            separators=(",", ":"),
        )
        descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=self._dir)
        try:
            os.chmod(temporary, 0o600)
            with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
                descriptor = -1
                stream.write(encoded)
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(temporary, path)
        finally:
            if descriptor >= 0:
                os.close(descriptor)
            with suppress(FileNotFoundError):
                os.unlink(temporary)


class _FileLock:
    """Advisory exclusive lock so two concurrent `begin` calls on one identity don't both
    decide to mint a fresh id for the same request. No-op on platforms without `fcntl`
    (Windows), matching `spending.py`'s single-process-only fallback there."""

    def __init__(self, path: Path) -> None:
        self._path = path
        self._handle = None

    def __enter__(self) -> None:
        self._handle = open(self._path, "a")
        if fcntl is not None:
            fcntl.flock(self._handle, fcntl.LOCK_EX)

    def __exit__(self, *exc_info: object) -> None:
        assert self._handle is not None
        if fcntl is not None:
            fcntl.flock(self._handle, fcntl.LOCK_UN)
        self._handle.close()
