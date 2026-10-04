"""One participant's native negotiation, payment, and optional access MCP tools."""

from __future__ import annotations

import asyncio
import json
import os
import stat
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from erebus import MetropolisError, MetropolisSeam
from mcp.server import MCPServer

from erebus_mcp.config import ConfigError


def _operator_file(value: str | None) -> tuple[str, dict[str, Any]]:
    if not value or not Path(value).is_absolute():
        raise ConfigError("configure absolute Metropolis operator files")
    path = Path(value)
    try:
        info = path.lstat()
        if not stat.S_ISREG(info.st_mode) or not 1 <= info.st_size <= 32_768 or (os.name == "posix" and info.st_mode & 0o077):
            raise ConfigError("Metropolis configuration must be an owner-only regular file")
        data = json.loads(path.read_text())
    except (OSError, ValueError):
        raise ConfigError("Metropolis configuration is unavailable or invalid") from None
    if not isinstance(data, dict):
        raise ConfigError("Metropolis configuration must be a JSON object")
    return str(path), data


@dataclass(frozen=True)
class MetropolisSettings:
    negotiation_config: str
    state_root: str
    role: str
    payment_config: str | None = None
    mode: str | None = None
    negotiation_binary: str | None = None
    payment_binary: str | None = None
    timeout: int = 600

    @classmethod
    def from_env(cls) -> MetropolisSettings:
        negotiation, data = _operator_file(os.environ.get("EREBUS_NEGOTIATION_CONFIG"))
        role, root = data.get("role"), data.get("state_root")
        if role not in {"buyer", "seller"} or not isinstance(root, str) or not Path(root).is_absolute():
            raise ConfigError("Metropolis needs a fixed buyer or seller role and absolute state root")
        payment, mode = None, None
        if role == "buyer" and os.environ.get("EREBUS_ACCESS_PAYMENT_RAIL") == "x402-exact":
            if os.environ.get("EREBUS_PAYMENT_CONFIG") or not os.environ.get("EREBUS_ACCESS_SERVICE_URL"):
                raise ConfigError("x402 requires an access service and no ordinary payment configuration")
            mode = "x402-exact"
        elif role == "buyer":
            payment, selected = _operator_file(os.environ.get("EREBUS_PAYMENT_CONFIG"))
            mode = selected.get("mode", "public-bound")
            if mode not in {"public-bound", "shielded"} or selected.get("state_root") != root:
                raise ConfigError("payment mode must be explicit and share this participant's state root")
        elif os.environ.get("EREBUS_PAYMENT_CONFIG"):
            raise ConfigError("a seller server cannot have payment configuration")
        timeout = os.environ.get("EREBUS_NATIVE_TIMEOUT_SECONDS", "600")
        if not timeout.isascii() or not timeout.isdecimal() or not 1 <= int(timeout) <= 900:
            raise ConfigError("native timeout must be between 1 and 900 seconds")
        return cls(negotiation, root, role, payment, mode, os.environ.get("EREBUS_NEGOTIATION_CLI"),
                   os.environ.get("EREBUS_PAYMENT_CLI"), int(timeout))


def build_metropolis_server(settings: MetropolisSettings | None = None) -> MCPServer:
    settings = settings or MetropolisSettings.from_env()
    if settings.mode == "x402-exact" and os.environ.get("EREBUS_ACCESS_PAYMENT_RAIL") != "x402-exact":
        raise ConfigError("x402 access rail must be explicitly selected by the operator")
    try:
        seam = MetropolisSeam(**vars(settings))
        seam.version()
    except MetropolisError:
        raise ConfigError("Metropolis native binaries are unavailable or incompatible") from None
    server = MCPServer(name="erebus-metropolis", instructions=(
        "Private negotiation with operator-fixed service policy, participant keys, and settlement mode. "
        f"This participant is the {settings.role}. "
        "Reuse the same operation ID after interruption. Negotiation cannot submit a payment. "
        "Only the buyer can settle; recovery observes without submitting. "
        "Pending or failed access is not a reason to pay again. "
        "Public-bound settlement exposes the accepted terms and parties. "
        "Shielded proving uses local witnesses and authenticated public artifacts. "
        "Payment verification does not prove delivery. "
        + ("x402 exact mode exposes negotiation and access only. First retrieval signs one local permit; the seller submits payment. Retries reuse that permit; independent disclosure verifies payment."
           if settings.mode == "x402-exact" else "")
    ))

    def failure() -> dict[str, Any]:
        return {"ok": False, "error": {"code": "METROPOLIS_UNAVAILABLE",
                "message": "retain the operation ID and recover by observation", "retry_without_new_payment": True}}

    @server.tool()
    async def negotiate_deal(operation_ref: str, freeze_only: bool = False) -> dict[str, Any]:
        """Negotiate and authorize one fixed-policy deal; never submits payment."""
        try:
            return {"ok": True, "result": await asyncio.to_thread(seam.negotiate, operation_ref, freeze_only=freeze_only)}
        except MetropolisError:
            return failure()

    if settings.role == "buyer" and settings.mode != "x402-exact":
        async def payment(operation_ref: str, method: str) -> dict[str, Any]:
            try:
                response = await asyncio.to_thread(seam.payment, operation_ref, method=method)
                return {"ok": response["status"] in {"ok", "ready"}, "result": response}
            except MetropolisError:
                return failure()

        @server.tool()
        async def check_settlement_funding(operation_ref: str) -> dict[str, Any]:
            """Check funding for an authorized deal without signing or submitting."""
            return await payment(operation_ref, "funding")

        @server.tool()
        async def settle_deal(operation_ref: str) -> dict[str, Any]:
            """Prepare, prove if shielded, and submit at most one automatic broadcast attempt."""
            return await payment(operation_ref, "settle")

        @server.tool()
        async def recover_deal(operation_ref: str) -> dict[str, Any]:
            """Observe and reconcile this operation; cannot sign, prove, or submit a payment."""
            return await payment(operation_ref, "observe")

    if settings.role == "buyer":
        if os.environ.get("EREBUS_ACCESS_SERVICE_URL"):
            from erebus_mcp.access import AccessSettings, register_access_tools

            if settings.mode != "x402-exact" and os.environ.get("EREBUS_ACCESS_PAYMENT_RAIL", "observe") != "observe":
                raise ConfigError("x402 authorization cannot be combined with ordinary settlement tools; use the dedicated access server")

            # Negotiation writes buyer evidence only under <state_root>/agent, so that is the
            # evidence directory; a separately configured one could only disagree with it.
            evidence = Path(settings.state_root) / "agent"
            configured = os.environ.get("EREBUS_ACCESS_EVIDENCE_DIR")
            if configured and Path(configured).resolve() != evidence.resolve():
                raise ConfigError("Metropolis access evidence is this participant's <state_root>/agent")
            try:
                evidence.mkdir(mode=0o700, parents=True, exist_ok=True)
            except OSError:
                raise ConfigError("cannot create the participant evidence directory") from None
            os.environ["EREBUS_ACCESS_EVIDENCE_DIR"] = str(evidence)
            register_access_tools(server, AccessSettings.from_env())
    return server
