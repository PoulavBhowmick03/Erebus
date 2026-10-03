"""Buyer access with operator-fixed endpoint, key path, and private evidence directory."""

from __future__ import annotations

import asyncio
import os
import re
import stat
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from erebus import AccessError, AccessSeam
from mcp.server import MCPServer

from erebus_mcp.config import ConfigError


@dataclass(frozen=True)
class AccessSettings:
    evidence_root: Path
    buyer_key_file: Path
    service_url: str
    service_id: str
    cache_root: Path
    binary: str | None = None
    allow_loopback_http: bool = False

    @classmethod
    def from_env(cls) -> AccessSettings:
        names = ("EREBUS_ACCESS_EVIDENCE_DIR", "EREBUS_ACCESS_BUYER_KEY_FILE", "EREBUS_ACCESS_SERVICE_URL", "EREBUS_ACCESS_SERVICE_ID", "EREBUS_ACCESS_CACHE")
        values = [os.environ.get(name) for name in names]
        if not all(values):
            raise ConfigError("configure private evidence, buyer key path, trusted access service, and cache")
        root, key, url, service_id, cache = values
        assert root and key and url and service_id and cache
        directory = Path(root).expanduser()
        key_file = Path(key).expanduser()
        try:
            info = directory.lstat()
            key_info = key_file.lstat()
        except OSError:
            raise ConfigError("access evidence directory or buyer key file is unavailable") from None
        if not stat.S_ISDIR(info.st_mode) or (os.name == "posix" and info.st_mode & 0o077):
            raise ConfigError("access evidence directory must be a real owner-only directory")
        if not stat.S_ISREG(key_info.st_mode) or key_info.st_size > 128 or (os.name == "posix" and key_info.st_mode & 0o077):
            raise ConfigError("buyer key must be an owner-only regular file")
        if not re.fullmatch(r"[a-f0-9]{64}", service_id) or service_id == "0" * 64:
            raise ConfigError("configure a trusted nonzero access service identity")
        development = os.environ.get("EREBUS_ALLOW_LOOPBACK_ACCESS_HTTP", "0")
        if development not in {"0", "1"}:
            raise ConfigError("loopback access exception must be 0 or 1")
        return cls(directory.resolve(), key_file, url, service_id, Path(cache).expanduser(), os.environ.get("EREBUS_ACCESS_CLI"), development == "1")

    def evidence(self, name: str) -> str:
        if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,95}", name, flags=re.ASCII):
            raise AccessError("evidence name must be a single bounded filename")
        return str(self.evidence_root / name)


def build_access_server(settings: AccessSettings | None = None) -> MCPServer:
    """Rust handles key access, signatures, payload verification, and durable storage."""
    settings = settings or AccessSettings.from_env()
    try:
        seam = AccessSeam(binary=settings.binary)
        seam.version()
    except AccessError:
        raise ConfigError("configured access binary is unavailable or incompatible") from None
    server = MCPServer(name="erebus-access", instructions=(
        "Retrieve one immutable snapshot using the existing buyer agreement key. "
        "The endpoint, key path, and private evidence directory are fixed by the operator. "
        "Private keys and payloads stay in local files. Rust verifies content against the signed agreement. "
        "The seller's payment claim is not independent chain verification or a delivery audit. "
        "Pending or failed access must not trigger another payment. This mode never submits a transaction."
    ))

    @server.tool()
    async def retrieve_service_access(evidence_name: str) -> dict[str, Any]:
        """Return a verified local resource path or pending access, without paying again."""
        try:
            response = await asyncio.to_thread(
                seam.retrieve, evidence_file=settings.evidence(evidence_name), buyer_key_file=str(settings.buyer_key_file),
                service_url=settings.service_url, service_id=settings.service_id, cache_root=str(settings.cache_root),
                allow_loopback_http=settings.allow_loopback_http,
            )
            return {"ok": response["status"] == "retrieved", "result": response}
        except AccessError as error:
            return {"ok": False, "error": {"code": "ACCESS_UNAVAILABLE", "message": str(error), "retry_without_payment": True}}
    return server
