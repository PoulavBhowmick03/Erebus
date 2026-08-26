"""Durable caller intent for state-changing MCP calls.

Rust durably records execution after a request reaches it. This store covers the earlier
gap: the caller records the operation id and exact tool arguments before sending the MCP
request. A restarted agent can therefore repeat the same logical step with the same id.
"""

from __future__ import annotations

import json
import os
import secrets
import tempfile
from pathlib import Path
from typing import Any


class IntentConflict(ValueError):
    """A logical step was reused for different tool arguments."""


class IntentStore:
    def __init__(self, path: Path) -> None:
        self._path = path

    def prepare(self, key: str, tool: str, arguments: dict[str, Any]) -> dict[str, Any]:
        """Persist intent before a call and return its stable record."""
        data = self._read()
        canonical = json.loads(json.dumps(arguments, sort_keys=True, separators=(",", ":")))
        existing = data["intents"].get(key)
        if existing is not None:
            if existing["tool"] != tool or existing["arguments"] != canonical:
                raise IntentConflict(f"intent {key!r} is already bound to different arguments")
            return existing
        record = {
            "operation_id": "op_" + secrets.token_hex(32),
            "tool": tool,
            "arguments": canonical,
            "state": "prepared",
        }
        data["intents"][key] = record
        self._write(data)
        return record

    def complete(self, key: str, result: Any) -> None:
        data = self._read()
        record = data["intents"][key]
        record["state"] = "completed"
        record["result"] = result
        self._write(data)

    def _read(self) -> dict[str, Any]:
        if not self._path.exists():
            return {"version": 1, "intents": {}}
        with self._path.open(encoding="utf-8") as stream:
            data = json.load(stream)
        if data.get("version") != 1 or not isinstance(data.get("intents"), dict):
            raise ValueError(f"unsupported caller-intent store {self._path}")
        return data

    def _write(self, data: dict[str, Any]) -> None:
        self._path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        os.chmod(self._path.parent, 0o700)
        descriptor, temporary = tempfile.mkstemp(
            prefix=f".{self._path.name}.", dir=self._path.parent
        )
        try:
            os.chmod(temporary, 0o600)
            with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
                descriptor = -1
                json.dump(data, stream, sort_keys=True, separators=(",", ":"))
                stream.write("\n")
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(temporary, self._path)
        finally:
            if descriptor >= 0:
                os.close(descriptor)
            try:
                os.unlink(temporary)
            except FileNotFoundError:
                pass
