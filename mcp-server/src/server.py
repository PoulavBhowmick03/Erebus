"""Erebus MCP server — entry point.

Owned by Ishita. This is scaffolding only: it establishes the language, the SDK API and
the run command so the track starts in the right place. The tools themselves are I1.3.

Run with:

    uv run mcp dev mcp-server/src/server.py

Note on the SDK API: the official `mcp` Python SDK reached 2.0, which **removed
`FastMCP`** — the class is now `mcp.server.MCPServer`. Docs written before 2026-07-29
(including CLAUDE.md and docs/ishita.md at the time) say FastMCP, which only exists on
`mcp<2`. Verified against the installed 2.0.0 in this workspace.

This server holds no key material. It takes a prover URL and an identity from config and
must fail loudly when they are absent rather than reaching for a shared endpoint — the
proving call carries the pool private key in the clear, so the prover belongs to whoever
owns that key. See docs/custody-design.md.
"""

from mcp.server import MCPServer

from erebus_mcp.config import ServerConfig
from erebus_mcp.mock_client import MockErebusClient
from erebus_mcp.seam_client import SeamErebusClient
from erebus_mcp.tools import register_tools

server = MCPServer(
    name="erebus",
    instructions=(
        "Private coordination and settlement between two agents on Starknet. "
        "Open a channel with a counterparty, exchange structured offers, and settle "
        "atomically. Every write costs a proof (~29 s), so negotiate in as few rounds "
        "as the policy allows."
    ),
)

# EREBUS_BACKEND picks the client. `mock` is the default so the agent track stays runnable
# with no chain, no keys and no gas; `seam` drives the real Rust client through the
# subprocess binding. No protocol logic at this layer either way: no hashing, no salt
# encoding, no felt arithmetic — see erebus_mcp/{mock_client,seam_client,interface}.py.
_config = ServerConfig.from_env()

if _config.backend == "seam":
    from erebus import Seam, SeamConfig

    assert _config.seam is not None  # from_env guarantees this for the seam backend
    _settings = _config.seam
    _client = SeamErebusClient(
        Seam(
            config=SeamConfig(
                rpc_url=_settings.rpc_url,
                prover_url=_config.prover_url,
                pool_address=_settings.pool_address,
                chain_id=_settings.chain_id,
                account_address=_config.address,
                pool_key_file=_settings.pool_key_file,
                account_key_file=_settings.account_key_file,
                state_dir=_settings.state_dir,
                token=_settings.token,
            ),
            binary=_settings.binary,
        )
    )
else:
    _client = MockErebusClient(
        identity=_config.address,
        store_path=_config.mock_store_path,
        latency_seconds=_config.mock_latency_seconds,
    )

register_tools(server, _client)


if __name__ == "__main__":
    server.run()
