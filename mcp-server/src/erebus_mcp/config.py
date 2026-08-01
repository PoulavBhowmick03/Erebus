"""Server configuration, loaded from the environment.

``docs/ishita.md``: "It needs a prover URL and identity in its config, and it should fail
loudly rather than fall back to a shared endpoint." Erebus holds no keys, so this process
runs against the operator's own prover (``docs/custody-design.md``) — ``prover_url`` is
required here even though the mock backend never calls it, because the failure mode this
guards against is a config that silently reaches for someone else's endpoint, and that
check should exist regardless of which client is behind it.
"""

from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path


class ConfigError(RuntimeError):
    """A required setting was missing. Fails loudly at startup, not on first use."""


@dataclass(frozen=True)
class ServerConfig:
    address: str
    prover_url: str
    mock_store_path: Path
    mock_latency_seconds: float

    @classmethod
    def from_env(cls) -> ServerConfig:
        # No counterparty here: `open_channel(counterparty)` takes it per call (§4), so it's
        # a tool argument, not server config — baking one in would make this server only
        # able to talk to a single fixed counterparty, which isn't what the interface says.
        address = _require("AGENT_ADDRESS")
        prover_url = _require("PROVING_SERVICE_URL")
        store_path = os.environ.get("EREBUS_MOCK_STORE_PATH", "/tmp/erebus-mock-store.json")
        latency = os.environ.get("EREBUS_MOCK_LATENCY_SECONDS", "0.2")
        try:
            latency_seconds = float(latency)
        except ValueError as exc:
            raise ConfigError(f"EREBUS_MOCK_LATENCY_SECONDS is not a number: {latency!r}") from exc

        return cls(
            address=address,
            prover_url=prover_url,
            mock_store_path=Path(store_path),
            mock_latency_seconds=latency_seconds,
        )


def _require(name: str) -> str:
    value = os.environ.get(name)
    if not value:
        raise ConfigError(f"{name} is required and not set")
    return value
