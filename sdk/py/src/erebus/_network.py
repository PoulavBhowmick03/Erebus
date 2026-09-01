"""Named canonical Starknet networks for Erebus configuration.

These are configuration presets, not protocol logic. They keep SDK callers and the MCP
server from independently copying chain IDs and pool addresses.
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum

__all__ = [
    "NETWORK_NAMES",
    "Network",
    "NetworkPreset",
    "identify_network",
    "network_preset",
]


class Network(str, Enum):
    """Canonical public networks supported by the packaged onboarding flow."""

    SEPOLIA = "sepolia"
    MAINNET = "mainnet"


@dataclass(frozen=True)
class NetworkPreset:
    """Canonical chain and STRK20 pool for one public network."""

    chain_id: str
    pool_address: str


_NETWORK_PRESETS = {
    Network.SEPOLIA: NetworkPreset(
        chain_id="0x534e5f5345504f4c4941",
        pool_address="0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91",
    ),
    Network.MAINNET: NetworkPreset(
        chain_id="0x534e5f4d41494e",
        pool_address="0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a",
    ),
}

NETWORK_NAMES = tuple(network.value for network in Network)


def network_preset(network: Network | str) -> NetworkPreset:
    """Return the canonical preset or raise ``ValueError`` for an unknown name."""

    selected = network if isinstance(network, Network) else Network(network.strip().lower())
    return _NETWORK_PRESETS[selected]


def identify_network(chain_id: str, pool_address: str) -> Network | None:
    """Identify an exact canonical chain/pool pair, ignoring felt zero padding."""

    for network, preset in _NETWORK_PRESETS.items():
        if _same_felt(chain_id, preset.chain_id) and _same_felt(
            pool_address, preset.pool_address
        ):
            return network
    return None


def _same_felt(left: str, right: str) -> bool:
    try:
        return int(left, 0) == int(right, 0)
    except ValueError:
        return False
