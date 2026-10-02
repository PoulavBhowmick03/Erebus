"""Metropolis disclosure-only MCP surface over local Rust commands."""

from __future__ import annotations

import asyncio
import json
import os
import re
import stat
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from erebus import DisclosureError, DisclosureSeam
from mcp.server import MCPServer

from erebus_mcp.config import ConfigError


@dataclass(frozen=True)
class DisclosureSettings:
    artifacts: Path
    key_file: Path
    binary: str | None = None
    state_root: str | None = None
    store_root: str | None = None
    namespace: str | None = None
    issuer_key_file: str | None = None
    deployment: dict[str, Any] | None = field(default=None, repr=False)

    @classmethod
    def from_env(cls) -> DisclosureSettings:
        directory = os.environ.get("EREBUS_DISCLOSURE_DIR")
        if not directory:
            raise ConfigError("EREBUS_DISCLOSURE_DIR must name an existing private directory")
        root = Path(directory).expanduser()
        try:
            info = root.lstat()
        except OSError:
            raise ConfigError("disclosure directory is unavailable") from None
        if not stat.S_ISDIR(info.st_mode) or (os.name == "posix" and info.st_mode & 0o077):
            raise ConfigError("disclosure directory must be a real owner-only directory")
        issuer = [os.environ.get(name) for name in (
            "EREBUS_DISCLOSURE_STATE_ROOT", "EREBUS_DISCLOSURE_STORE_ROOT",
            "EREBUS_DISCLOSURE_NAMESPACE", "EREBUS_DISCLOSURE_ISSUER_KEY_FILE",
        )]
        if any(issuer) and not all(issuer):
            raise ConfigError("configure all four issuer disclosure settings or none")
        deployment = None
        if encoded := os.environ.get("EREBUS_DISCLOSURE_DEPLOYMENT"):
            try:
                deployment = json.loads(encoded)
            except ValueError:
                raise ConfigError("invalid disclosure deployment JSON") from None
            if not isinstance(deployment, dict):
                raise ConfigError("disclosure deployment must be an object")
        root = root.resolve()
        return cls(
            artifacts=root,
            key_file=Path(os.environ.get("EREBUS_DISCLOSURE_KEY_FILE", str(root / "auditor.key"))),
            binary=os.environ.get("EREBUS_DISCLOSURE_CLI"),
            state_root=issuer[0], store_root=issuer[1], namespace=issuer[2], issuer_key_file=issuer[3],
            deployment=deployment,
        )

    def artifact(self, name: str) -> str:
        if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,95}", name, flags=re.ASCII):
            raise DisclosureError("artifact name must be a single bounded filename")
        return str(self.artifacts / name)


def build_disclosure_server(settings: DisclosureSettings | None = None) -> MCPServer:
    """No wallet, prover, Starknet configuration, or private file reads in Python."""
    settings = settings or DisclosureSettings.from_env()
    try:
        seam = DisclosureSeam(binary=settings.binary)
        capabilities = seam.call("version")
    except DisclosureError:
        raise ConfigError("configured disclosure binary is unavailable or incompatible") from None
    if type(capabilities.get("protocol")) is not int or capabilities["protocol"] != 1:
        raise ConfigError("disclosure binary protocol mismatch")
    server = MCPServer(name="erebus-disclosure", instructions=(
        "Verify or export one deal disclosure using a local Rust binary. "
        "Tools do not return plaintext terms or keys, do not make payments, and never prove delivery. "
        "Agreement verification is offline; payment verification needs trusted deployment configuration. "
        "Missing evidence or failed verification does not establish non-payment."
    ))

    async def call(method: str, **parameters: Any) -> dict[str, Any]:
        try:
            response = await asyncio.to_thread(seam.call, method, **parameters)
            return {"ok": True, "result": response}
        except DisclosureError as error:
            return {"ok": False, "error": {"code": "DISCLOSURE_UNAVAILABLE", "message": str(error)}}

    @server.tool()
    async def create_disclosure_key() -> dict[str, Any]:
        """Create the configured auditor key without overwriting a key; return its public key only."""
        return await call("keygen", key_file=str(settings.key_file))

    @server.tool()
    async def disclosure_key_info() -> dict[str, Any]:
        """Recover the public auditor key from its configured local file without regenerating it."""
        return await call("key_info", key_file=str(settings.key_file))

    @server.tool()
    async def verify_disclosed_agreement(grant_name: str, expected_issuer: str) -> dict[str, Any]:
        """Verify a grant offline against the participant identity obtained independently."""
        try:
            grant = settings.artifact(grant_name)
        except DisclosureError as error:
            return {"ok": False, "error": {"code": "INVALID_ARTIFACT", "message": str(error)}}
        return await call("verify_agreement", grant_file=grant,
                          key_file=str(settings.key_file), expected_issuer=expected_issuer)

    if settings.deployment is not None:
        @server.tool()
        async def verify_disclosed_payment(grant_name: str, expected_issuer: str) -> dict[str, Any]:
            """Verify finalized payment using only the operator's fixed deployment configuration."""
            try:
                grant = settings.artifact(grant_name)
            except DisclosureError as error:
                return {"ok": False, "error": {"code": "INVALID_ARTIFACT", "message": str(error)}}
            return await call("verify_payment", grant_file=grant,
                              key_file=str(settings.key_file), expected_issuer=expected_issuer,
                              deployment=settings.deployment)

    if settings.issuer_key_file is not None:
        @server.tool()
        async def select_deal_disclosure(operation_ref: str, evidence_name: str) -> dict[str, Any]:
            """Rebuild one verified deal from configured durable state and save private evidence locally."""
            try:
                evidence = settings.artifact(evidence_name)
            except DisclosureError as error:
                return {"ok": False, "error": {"code": "INVALID_ARTIFACT", "message": str(error)}}
            return await call("select", state_root=settings.state_root,
                              operation_ref=operation_ref, store_root=settings.store_root,
                              namespace=settings.namespace, transcript_hash_version=1,
                              evidence_file=evidence)

        @server.tool()
        async def export_deal_disclosure(
            evidence_name: str, recipient_public_key: str, grant_name: str, expires_at: int,
        ) -> dict[str, Any]:
            """Encrypt selected evidence to an auditor; create a new grant without exposing terms or keys."""
            try:
                evidence, grant = settings.artifact(evidence_name), settings.artifact(grant_name)
            except DisclosureError as error:
                return {"ok": False, "error": {"code": "INVALID_ARTIFACT", "message": str(error)}}
            return await call("export", evidence_file=evidence, issuer_key_file=settings.issuer_key_file,
                              recipient_public_key=recipient_public_key, grant_file=grant,
                              expires_at=expires_at)
    return server
