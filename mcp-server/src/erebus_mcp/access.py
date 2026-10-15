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
    x402_exact: bool = False
    profile: str = "snapshot"

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
        profile = os.environ.get("EREBUS_ACCESS_PROFILE", "snapshot")
        if profile not in {"snapshot", "metered"}:
            raise ConfigError("access profile must be snapshot or metered")
        rail = os.environ.get("EREBUS_ACCESS_PAYMENT_RAIL", "observe")
        if rail not in {"observe", "x402-exact"}:
            raise ConfigError("access payment rail must be observe or x402-exact")
        if profile == "metered" and rail == "x402-exact":
            raise ConfigError("metered access cannot use the x402-exact rail")
        return cls(directory.resolve(), key_file, url, service_id, Path(cache).expanduser(),
                   os.environ.get("EREBUS_ACCESS_CLI"), development == "1", rail == "x402-exact", profile)

    def evidence(self, name: str) -> str:
        if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,95}", name, flags=re.ASCII):
            raise AccessError("evidence name must be a single bounded filename")
        return str(self.evidence_root / name)


def build_access_server(settings: AccessSettings | None = None) -> MCPServer:
    """Rust handles key access, signatures, payload verification, and durable storage."""
    settings = settings or AccessSettings.from_env()
    if settings.profile == "metered":
        instructions = (
            "This server consumes prepaid metered calls and cannot authorize payment. "
            "Rust keeps the durable cursor, signs each call with the existing buyer agreement key, "
            "and retries the same in-flight index after a timeout, pending payment, or lost reply. "
            "The endpoint, key path, and private evidence directory are fixed by the operator. "
            "Private keys and response bodies stay in local files. "
            "The seller's payment claim is not independent chain verification or a delivery audit. "
            "Pending or failed calls must not trigger another payment rail."
        )
    else:
        instructions = (
            ("The operator selected x402-exact: the first retrieval locally authorizes payment and asks the seller to submit once. Retries reuse the durable permit. " if settings.x402_exact else "This server retrieves already paid resources and cannot authorize payment. ") +
            "Retrieve one immutable snapshot using the existing buyer agreement key. "
            "The endpoint, key path, and private evidence directory are fixed by the operator. "
            "Private keys and payloads stay in local files. Rust verifies content against the signed agreement. "
            "The seller's payment claim is not independent chain verification or a delivery audit. "
            "Pending or failed access must not trigger another payment rail."
        )
    server = MCPServer(name="erebus-access", instructions=instructions)
    register_access_tools(server, settings)
    return server


def register_access_tools(server: MCPServer, settings: AccessSettings) -> None:
    """Reuse the same path-only access boundary in a combined Metropolis server."""
    try:
        seam = AccessSeam(binary=settings.binary)
        seam.version()
    except AccessError:
        raise ConfigError("configured access binary is unavailable or incompatible") from None

    if settings.profile == "metered":
        _register_metered_tools(server, settings, seam)
    else:
        _register_retrieval_tool(server, settings, seam)


def _access_unavailable(error: AccessError) -> dict[str, Any]:
    failure = {"code": "ACCESS_UNAVAILABLE", **error.public_failure(), "retry_without_payment": True}
    return {"ok": False, "error": failure}


def _register_retrieval_tool(server: MCPServer, settings: AccessSettings, seam: AccessSeam) -> None:
    @server.tool()
    async def retrieve_service_access(evidence_name: str | None = None,
                                      operation_ref: str | None = None) -> dict[str, Any]:
        """Retrieve a resource; operator-selected x402 mode authorizes its first payment.

        Pass the deal's operation_ref, or the evidence_name negotiate_deal returned (exactly one).
        Retries reuse the persisted permit and never authorize a second payment.
        """
        if (evidence_name is None) == (operation_ref is None):
            return _access_unavailable(AccessError("pass exactly one of operation_ref or evidence_name"))
        if operation_ref is not None:
            if not re.fullmatch(r"[a-f0-9]{64}", operation_ref, flags=re.ASCII):
                return _access_unavailable(AccessError("operation_ref must be 64 lowercase hex digits"))
            # Negotiation writes a deal's buyer evidence as <operation_ref>.0.tx.
            evidence_name = f"{operation_ref}.0.tx"
        try:
            options = {"x402_exact": True} if settings.x402_exact else {}
            response = await asyncio.to_thread(
                seam.retrieve, evidence_file=settings.evidence(evidence_name), buyer_key_file=str(settings.buyer_key_file),
                service_url=settings.service_url, service_id=settings.service_id, cache_root=str(settings.cache_root),
                allow_loopback_http=settings.allow_loopback_http,
                **options,
            )
            payment_source = ("independent disclosure verification (erebus-shielded-disclosure verify_payment); this x402 server has no payment-verification tool"
                              if settings.x402_exact else "recover_deal")
            return {"ok": response["status"] == "retrieved", "result": response,
                    "verification_meaning": "resource_verified checks the resource hash; seller_reported_payment_finalized is the seller's claim; "
                    f"payment_verified comes only from {payment_source}; delivery_verified requires a separate delivery audit, not a successful download."}
        except AccessError as error:
            return _access_unavailable(error)


def _register_metered_tools(server: MCPServer, settings: AccessSettings, seam: AccessSeam) -> None:
    @server.tool()
    async def call_metered_service(evidence_name: str, call_index: int | None = None) -> dict[str, Any]:
        """Consume one prepaid call, finish the call in flight, or return an earlier index.

        A timeout, pending payment, or lost reply keeps the same index in flight; retry it,
        never a new payment.
        """
        try:
            response = await asyncio.to_thread(
                seam.metered_call, evidence_file=settings.evidence(evidence_name), buyer_key_file=str(settings.buyer_key_file),
                service_url=settings.service_url, service_id=settings.service_id, cache_root=str(settings.cache_root),
                allow_loopback_http=settings.allow_loopback_http, call_index=call_index,
            )
            return {"ok": response["status"] == "issued", "result": response,
                    "verification_meaning": "response_hash_consistent checks the bytes against the seller-reported hash; "
                    "payment_verified is false: this server does not observe the chain; "
                    "delivery_verified requires a separate delivery audit, not a successful download."}
        except AccessError as error:
            return _access_unavailable(error)

    @server.tool()
    async def metered_service_status(evidence_name: str) -> dict[str, Any]:
        """Report the seller's allowance and reconcile the local cursor. Consumes nothing."""
        try:
            response = await asyncio.to_thread(
                seam.metered_status, evidence_file=settings.evidence(evidence_name), buyer_key_file=str(settings.buyer_key_file),
                service_url=settings.service_url, service_id=settings.service_id, cache_root=str(settings.cache_root),
                allow_loopback_http=settings.allow_loopback_http,
            )
            return {"ok": response["status"] == "allowance", "result": response,
                    "verification_meaning": "seller_next_call_index is the seller's report and next_call_index is the local cursor; "
                    "payment_verified is false: this server does not observe the chain."}
        except AccessError as error:
            return _access_unavailable(error)
