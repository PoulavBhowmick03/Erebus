"""Local proving tools with operator-fixed artifact and witness directories."""

from __future__ import annotations

import asyncio
import os
import re
import stat
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from erebus import LocalProvingError, LocalProvingSeam
from mcp.server import MCPServer

from erebus_mcp.config import ConfigError


@dataclass(frozen=True)
class ProvingSettings:
    witness_root: Path
    manifest_file: Path
    manifest_sha256: str
    cache_root: Path
    binary: str | None = None
    allow_test_artifacts: bool = False
    allow_loopback_http: bool = False

    @classmethod
    def from_env(cls) -> ProvingSettings:
        names = ("EREBUS_WITNESS_DIR", "EREBUS_ARTIFACT_MANIFEST", "EREBUS_ARTIFACT_MANIFEST_SHA256", "EREBUS_ARTIFACT_CACHE")
        values = [os.environ.get(name) for name in names]
        if not all(values):
            raise ConfigError("configure witness directory, trusted artifact manifest and digest, and cache")
        witness, manifest, digest, cache = values
        assert witness and manifest and digest and cache
        root = Path(witness).expanduser()
        try:
            info = root.lstat()
        except OSError:
            raise ConfigError("local witness directory is unavailable") from None
        if not stat.S_ISDIR(info.st_mode) or (os.name == "posix" and info.st_mode & 0o077):
            raise ConfigError("local witness directory must be a real owner-only directory")
        if not re.fullmatch(r"[a-f0-9]{64}", digest):
            raise ConfigError("artifact manifest digest must be independently trusted lowercase SHA-256")
        exceptions = [os.environ.get(name, "0") for name in ("EREBUS_ALLOW_TEST_ARTIFACTS", "EREBUS_ALLOW_LOOPBACK_ARTIFACT_HTTP")]
        if any(value not in {"0", "1"} for value in exceptions):
            raise ConfigError("artifact development exceptions must be 0 or 1")
        return cls(root.resolve(), Path(manifest).expanduser(), digest, Path(cache).expanduser(),
                   os.environ.get("EREBUS_LOCAL_PROVER_CLI"), exceptions[0] == "1", exceptions[1] == "1")

    def witness(self, name: str) -> str:
        if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,95}", name, flags=re.ASCII):
            raise LocalProvingError("witness name must be a single bounded filename")
        return str(self.witness_root / name)


def build_proving_server(settings: ProvingSettings | None = None) -> MCPServer:
    """Python forwards paths only; Rust installs public artifacts and proves locally."""
    settings = settings or ProvingSettings.from_env()
    try:
        seam = LocalProvingSeam(binary=settings.binary)
        seam.version()
    except LocalProvingError:
        raise ConfigError("configured local proving binary is unavailable or incompatible") from None
    server = MCPServer(name="erebus-local-prover", instructions=(
        "Generate a local proof for an already prepared witness and expected public transition. "
        "Public artifacts are downloaded and hash-checked against operator-fixed configuration. "
        "Private witness files remain local. A proof alone is not a payment or delivery receipt. "
        "No keys are generated, no authorization is signed, and no transaction is sent."
    ))

    @server.tool()
    async def prove_local_transition(witness_name: str, circuit: str, expected_public: list[str]) -> dict[str, Any]:
        """Prove a prepared local witness; return only public calldata and separate timings."""
        try:
            response = await asyncio.to_thread(
                seam.prove, manifest_file=str(settings.manifest_file), manifest_sha256=settings.manifest_sha256,
                circuit=circuit, cache_root=str(settings.cache_root), witness_file=settings.witness(witness_name),
                expected_public=expected_public, allow_test_artifacts=settings.allow_test_artifacts,
                allow_loopback_http=settings.allow_loopback_http,
            )
            return {"ok": True, "result": response}
        except LocalProvingError as error:
            return {"ok": False, "error": {"code": "LOCAL_PROVING_UNAVAILABLE", "message": str(error)}}
    return server
