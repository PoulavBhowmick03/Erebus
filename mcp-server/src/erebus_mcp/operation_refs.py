"""Durable operation references created for agents that omit one.

An agent that forgets its reference must not create a second payment or funding action by
retrying. So a created reference is written here, fsynced, before any native call uses it, and an
omitted reference resumes the newest unfinished one of the same kind and amount. A different
amount while one is unfinished is refused; an explicit reference always bypasses this file.
"""

from __future__ import annotations

import fcntl
import json
import os
import secrets
import time
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Iterator

MAX_ENTRIES = 512


class OperationRefs:
    def __init__(self, state_root: str) -> None:
        self._path = Path(state_root) / "agent" / "mcp-operation-refs.json"

    @contextmanager
    def _locked(self) -> Iterator[list[dict[str, Any]]]:
        self._path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        with open(self._path.with_suffix(".lock"), "a") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            entries = self._read()
            yield entries
            self._write(entries[-MAX_ENTRIES:])

    def _read(self) -> list[dict[str, Any]]:
        try:
            data = json.loads(self._path.read_text())
        except FileNotFoundError:
            return []
        if not isinstance(data, dict) or data.get("version") != 1 or not isinstance(data.get("entries"), list):
            raise ValueError("operation reference journal is malformed")
        return data["entries"]

    def _write(self, entries: list[dict[str, Any]]) -> None:
        temporary = self._path.with_name(f".{self._path.name}.{os.getpid()}.tmp")
        descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(descriptor, "w") as handle:
            json.dump({"version": 1, "entries": entries}, handle)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, self._path)
        directory = os.open(self._path.parent, os.O_RDONLY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)

    def resolve(self, kind: str, amount: str | None, finished: Any = None) -> tuple[str, bool] | dict[str, Any]:
        """The reference to use for an omitted one: (ref, created), or the conflicting entry.

        `finished(entry)` reports whether an unfinished-looking entry has since completed.
        """
        with self._locked() as entries:
            # Only the newest reference of a kind can be unfinished: a new one is never created
            # while it is.
            latest = next((entry for entry in reversed(entries) if entry["kind"] == kind), None)
            if latest is not None and not latest.get("done"):
                if finished is not None and finished(latest):
                    latest["done"] = True
                elif latest.get("amount") == amount:
                    return latest["ref"], False
                else:
                    return dict(latest)
            reference = secrets.token_hex(32)
            entries.append({"kind": kind, "ref": reference, "amount": amount, "created": int(time.time())})
            return reference, True

    def update(self, reference: str, **fields: Any) -> None:
        with self._locked() as entries:
            for entry in entries:
                if entry["ref"] == reference:
                    entry.update(fields)

    def get(self, reference: str) -> dict[str, Any] | None:
        with self._locked() as entries:
            return next((dict(entry) for entry in entries if entry["ref"] == reference), None)

    def record(self, entry: dict[str, Any]) -> None:
        with self._locked() as entries:
            entries.append(entry)
