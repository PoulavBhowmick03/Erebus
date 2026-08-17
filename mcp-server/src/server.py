"""Erebus MCP server entry point.

Owned by Ishita. The production backend reaches the protocol-2 Rust client through the
Python binding. The mock backend supports deterministic tests.

Run with:

    uv run mcp dev mcp-server/src/server.py

The official `mcp` Python SDK removed `FastMCP` in 2.0. Use
`mcp.server.MCPServer`. Documents written before 2026-07-29 can still refer to FastMCP,
which exists only on `mcp<2`. This workspace uses 2.0.0.

This server holds no key material. It takes a prover URL and an identity from config and
fails at startup when either is absent. The proving call sends the plaintext pool private
key, so the key owner must control the prover. See docs/custody-design.md.
"""

from mcp.server import MCPServer

from erebus_mcp.config import ServerConfig
from erebus_mcp.mock_client import MockErebusClient
from erebus_mcp.seam_client import SeamErebusClient
from erebus_mcp.tools import register_tools

_config = ServerConfig.from_env()

server = MCPServer(
    name="erebus",
    instructions=(
        "Structured negotiation and settlement between two agents on Starknet. "
        "Negotiation terms and settlement amounts stay private; opening a channel does "
        "not, it writes the counterparty's address to public calldata (F38). "
        "Open a channel with a counterparty, exchange structured offers, and settle "
        "atomically. accept_and_settle always spends this identity's private notes: "
        "only the payer calls it; a payee leaves its final offer for the payer. "
        f"This server is configured as {_config.settlement_role.value}. "
        "Every write costs a proof, so negotiate in as few rounds as the policy allows."
    ),
)

# EREBUS_BACKEND selects the client. `mock` needs no chain, keys, or gas. `seam` calls the
# Rust client through the subprocess binding. This layer contains no hashing, salt encoding,
# or felt arithmetic. See erebus_mcp/{mock_client,seam_client,interface}.py.
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
        spendable_notes=list(_config.mock_spendable_notes),
        pending_notes=list(_config.mock_pending_notes),
    )

register_tools(server, _client, _config.settlement_role)


if __name__ == "__main__":
    server.run()
