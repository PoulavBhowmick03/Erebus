"""Server configuration, loaded from the environment.

``docs/ishita.md``: "It needs a prover URL and identity in its config, and it should fail
loudly rather than fall back to a shared endpoint." Erebus holds no keys, so this process
runs against the operator's own prover (``docs/custody-design.md``) — ``prover_url`` is
required here even when the mock backend never calls it, because the failure this guards
against is a config that silently reaches for someone else's endpoint, and that check
should exist regardless of which client sits behind it.

Two backends. ``mock`` is the default and needs nothing beyond an address and a prover URL.
``seam`` drives the real Rust client and needs the full protocol-2 configuration, which is
validated at startup rather than on the first tool call: an agent discovering a missing
``POOL_KEY_FILE`` twenty seconds into a proof has already wasted a turn and some gas.
"""

from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path


class ConfigError(RuntimeError):
    """A required setting was missing. Fails loudly at startup, not on first use."""


@dataclass(frozen=True)
class SeamSettings:
    """Everything ``erebus-cli`` needs, plus where to find it.

    The key files are paths and stay paths. This process never reads them; the Rust binary
    does. That is the whole custody argument for the subprocess seam, and it holds only for
    as long as nothing here opens them.
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
    mock_store_path: Path
    mock_latency_seconds: float
    seam: SeamSettings | None = None

    @classmethod
    def from_env(cls) -> ServerConfig:
        # No counterparty here: `open_channel(counterparty)` takes it per call (§4), so it's
        # a tool argument, not server config — baking one in would make this server only
        # able to talk to a single fixed counterparty, which isn't what the interface says.
        address = _require("AGENT_ADDRESS")
        prover_url = _require("PROVING_SERVICE_URL")
        backend = os.environ.get("EREBUS_BACKEND", "mock").strip().lower()
        if backend not in {"mock", "seam"}:
            raise ConfigError(f"EREBUS_BACKEND must be 'mock' or 'seam', got {backend!r}")

        store_path = os.environ.get("EREBUS_MOCK_STORE_PATH", "/tmp/erebus-mock-store.json")
        latency = os.environ.get("EREBUS_MOCK_LATENCY_SECONDS", "0.2")
        try:
            latency_seconds = float(latency)
        except ValueError as exc:
            raise ConfigError(f"EREBUS_MOCK_LATENCY_SECONDS is not a number: {latency!r}") from exc

        seam = _seam_settings() if backend == "seam" else None

        return cls(
            address=address,
            prover_url=prover_url,
            backend=backend,
            mock_store_path=Path(store_path),
            mock_latency_seconds=latency_seconds,
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
