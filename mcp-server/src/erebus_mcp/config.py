"""Server configuration from environment variables.

``docs/ishita.md``: "It needs a prover URL and identity in its config, and it should fail
loudly rather than fall back to a shared endpoint." Erebus holds no keys, so this process
runs against the operator's prover (``docs/custody-design.md``). ``prover_url`` remains
required for the mock backend so every configuration names its endpoint explicitly.

``mock`` is the default and needs an address and prover URL. ``seam`` drives the Rust client
and needs the full protocol-2 configuration. Startup validation catches a missing
``POOL_KEY_FILE`` before a tool call starts proving.
"""

from __future__ import annotations

import os
from dataclasses import dataclass
from enum import Enum
from pathlib import Path


class ConfigError(RuntimeError):
    """A required setting is missing at startup."""


class SettlementRole(str, Enum):
    """Which side of a payment this MCP identity is allowed to take.

    ``accept_and_settle`` spends the caller's notes. A configured role prevents a payee
    server from accepting. ``BOTH`` supports an identity that buys and sells.
    """

    PAYER = "payer"
    PAYEE = "payee"
    BOTH = "both"


@dataclass(frozen=True)
class SeamSettings:
    """Everything ``erebus-cli`` needs, plus where to find it.

    This process passes key-file paths to the Rust binary and never opens them.
    """

    binary: Path
    rpc_url: str
    pool_address: str
    chain_id: str
    pool_key_file: Path
    account_key_file: Path
    state_dir: Path
    token: str


@dataclass(frozen=True)
class ServerConfig:
    address: str
    prover_url: str
    backend: str
    settlement_role: SettlementRole
    mock_store_path: Path
    mock_latency_seconds: float
    mock_spendable_notes: tuple[int, ...]
    mock_pending_notes: tuple[int, ...]
    seam: SeamSettings | None = None

    @classmethod
    def from_env(cls) -> ServerConfig:
        # `open_channel(counterparty)` supplies the counterparty for each call (§4). Do not
        # bind the server to one counterparty in configuration.
        address = _require("AGENT_ADDRESS")
        prover_url = _require("PROVING_SERVICE_URL")
        backend = os.environ.get("EREBUS_BACKEND", "mock").strip().lower()
        if backend not in {"mock", "seam"}:
            raise ConfigError(f"EREBUS_BACKEND must be 'mock' or 'seam', got {backend!r}")

        role_raw = _require("EREBUS_SETTLEMENT_ROLE").strip().lower()
        try:
            settlement_role = SettlementRole(role_raw)
        except ValueError as exc:
            choices = ", ".join(role.value for role in SettlementRole)
            raise ConfigError(
                f"EREBUS_SETTLEMENT_ROLE must be one of {choices}, got {role_raw!r}"
            ) from exc

        store_path = os.environ.get("EREBUS_MOCK_STORE_PATH", "/tmp/erebus-mock-store.json")
        latency = os.environ.get("EREBUS_MOCK_LATENCY_SECONDS", "0.2")
        try:
            latency_seconds = float(latency)
        except ValueError as exc:
            raise ConfigError(f"EREBUS_MOCK_LATENCY_SECONDS is not a number: {latency!r}") from exc

        mock_spendable = _amounts("EREBUS_MOCK_SPENDABLE_NOTES", "1000000000000000000")
        mock_pending = _amounts("EREBUS_MOCK_PENDING_NOTES", "")

        seam = _seam_settings() if backend == "seam" else None

        return cls(
            address=address,
            prover_url=prover_url,
            backend=backend,
            settlement_role=settlement_role,
            mock_store_path=Path(store_path),
            mock_latency_seconds=latency_seconds,
            mock_spendable_notes=mock_spendable,
            mock_pending_notes=mock_pending,
            seam=seam,
        )


def _seam_settings() -> SeamSettings:
    binary = Path(_require("EREBUS_CLI"))
    if not binary.exists():
        raise ConfigError(
            f"EREBUS_CLI points at {binary}, which does not exist. Build it with "
            "`cargo build --release --bin erebus-cli` in sdk/rs."
        )

    pool_key = Path(_require("POOL_KEY_FILE"))
    account_key = Path(_require("ACCOUNT_KEY_FILE"))
    for label, path in (("POOL_KEY_FILE", pool_key), ("ACCOUNT_KEY_FILE", account_key)):
        if not path.exists():
            raise ConfigError(f"{label} points at {path}, which does not exist")

    return SeamSettings(
        binary=binary,
        rpc_url=_require("STARKNET_RPC_URL"),
        pool_address=_require("POOL_ADDRESS"),
        chain_id=_require("STARKNET_CHAIN_ID"),
        pool_key_file=pool_key,
        account_key_file=account_key,
        state_dir=Path(_require("EREBUS_STATE_DIR")),
        token=_require("TOKEN_ADDRESS"),
    )


def _require(name: str) -> str:
    value = os.environ.get(name)
    if not value:
        raise ConfigError(f"{name} is required and not set")
    return value


def _amounts(name: str, default: str) -> tuple[int, ...]:
    raw = os.environ.get(name, default).strip()
    if not raw:
        return ()
    try:
        values = tuple(int(part.strip()) for part in raw.split(","))
    except ValueError as exc:
        raise ConfigError(f"{name} must be comma-separated integers, got {raw!r}") from exc
    if any(value <= 0 for value in values):
        raise ConfigError(f"{name} values must all be positive")
    return values
