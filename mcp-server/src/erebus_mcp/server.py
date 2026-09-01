"""Erebus MCP server.

Owned by Ishita. The production backend reaches the protocol-4 Rust client through the
Python binding. The mock backend supports deterministic tests.

Run the installed entry point:

    erebus-mcp-server

or, from a checkout, either of:

    uv run mcp dev mcp-server/src/server.py
    uv run python -m erebus_mcp.server

Construction lives in `build_server()` rather than at import time so that importing this
module never requires environment configuration; only running it does. The `server.py`
shim beside the package builds at import for `mcp dev`, which inspects a file for a
module-level server object.

The official `mcp` Python SDK removed `FastMCP` in 2.0. Use
`mcp.server.MCPServer`. Documents written before 2026-07-29 can still refer to FastMCP,
which exists only on `mcp<2`. This workspace uses 2.0.0.

This server holds no key material. It takes a prover URL and an identity from config and
fails at startup when either is absent. The proving call sends the plaintext pool private
key, so the key owner must control the prover. See docs/custody-design.md.
"""

import argparse
import logging
import sys
from collections.abc import Sequence
from pathlib import Path

from mcp.server import MCPServer

from erebus_mcp.config import ConfigError, ServerConfig
from erebus_mcp.intent import IntentStore
from erebus_mcp.onboarding import (
    OnboardingError,
    default_config_path,
    environment_is_configured,
    init_main,
    load_config_file,
    resolve_config_path,
    schema_main,
)
from erebus_mcp.mock_client import MockErebusClient
from erebus_mcp.seam_client import SeamErebusClient
from erebus_mcp.spending import SpendGuard
from erebus_mcp.tools import register_tools

logger = logging.getLogger("erebus_mcp")


def build_server() -> MCPServer:
    """Read configuration from the environment and assemble the server."""
    config = ServerConfig.from_env()

    server = MCPServer(
        name="erebus",
        instructions=(
            "Structured negotiation and settlement between two agents on Starknet. "
            "Negotiation terms and settlement amounts are confidential against a chain "
            "reader; they are not confidential against the prover or write RPC, which see "
            "the pool key (docs/threat-model.md). Opening a channel is never confidential: "
            "it writes the counterparty's address to public calldata (F38). "
            "Open a channel with a counterparty, exchange structured offers, and settle "
            "atomically. accept_and_settle always spends this identity's private notes: "
            "only the payer calls it; a payee leaves its final offer for the payer. "
            f"This server is configured as {config.settlement_role.value}. "
            "Every write costs a proof, so negotiate in as few rounds as the policy allows."
        ),
    )

    # EREBUS_BACKEND selects the client. `mock` needs no chain, keys, or gas. `seam` calls
    # the Rust client through the subprocess binding. This layer contains no hashing, salt
    # encoding, or felt arithmetic. See erebus_mcp/{mock_client,seam_client,interface}.py.
    client: MockErebusClient | SeamErebusClient
    if config.backend == "seam":
        from erebus import PROTOCOL, Seam, SeamConfig

        assert config.seam is not None  # from_env guarantees this for the seam backend
        settings = config.seam
        seam = Seam(
            config=SeamConfig(
                rpc_url=settings.rpc_url,
                prover_url=config.prover_url,
                pool_address=settings.pool_address,
                chain_id=settings.chain_id,
                account_address=config.address,
                pool_key_file=settings.pool_key_file,
                account_key_file=settings.account_key_file,
                state_dir=settings.state_dir,
                token=settings.token,
                wire_version=settings.wire_version,
            ),
            binary=settings.binary,
        )

        # Startup handshake: one cheap `version` call proves the binary runs and speaks
        # this binding's contract, before any tool call depends on it. A long-running
        # server outliving a binary upgrade is the case this exists for (2026-08-19).
        reported = seam.version()
        spoken = reported.get("protocol")
        if spoken != PROTOCOL:
            raise ConfigError(
                f"{settings.binary} speaks seam protocol {spoken}, this server expects "
                f"{PROTOCOL}. Install erebus-cli and erebus-mcp-server from the same "
                "release."
            )

        # Pre-flight inspection, read-only, logged rather than fatal: an operator may
        # start the server first and repair second, and `doctor` names each repair.
        if config.startup_doctor:
            report = seam.doctor()
            if report.get("ready"):
                logger.info("doctor: ready, all checks passed")
            else:
                for check in report.get("checks", []):
                    if check.get("status") != "pass":
                        logger.warning(
                            "doctor: %s %s — %s (repair: %s)",
                            check.get("name"),
                            check.get("status"),
                            check.get("detail"),
                            check.get("repair") or "none given",
                        )
                logger.warning(
                    "doctor: not ready — writes will fail until the checks above pass"
                )

        client = SeamErebusClient(seam)
    else:
        client = MockErebusClient(
            identity=config.address,
            store_path=config.mock_store_path,
            latency_seconds=config.mock_latency_seconds,
            spendable_notes=list(config.mock_spendable_notes),
            pending_notes=list(config.mock_pending_notes),
        )

    spend_guard = SpendGuard(config.spending_limits, config.spending_state_path)
    # Persists an operation id and its canonical parameters before a write reaches the
    # seam, so a crashed process leaves a record instead of silence (plan.md, Ishita
    # task 1). Not yet consulted by Rust — see erebus_mcp/intent.py for the scope boundary.
    intent_store = IntentStore(config.intent_state_dir)
    # Stamped onto every tool result (roadmap 9.2) so a transcript alone tells a model
    # whether it is talking to a real chain, and which one. "mock" has no chain to report;
    # canonical seam configurations use a friendly name, and unknown deployments say custom.
    network = config.seam.network if config.seam is not None else "mock"
    register_tools(
        server, client, config.settlement_role, spend_guard, intent_store, config.backend, network
    )
    return server


def main(argv: Sequence[str] | None = None) -> None:
    """Load protected configuration, run first-use setup, or start MCP over stdio."""

    arguments = list(sys.argv[1:] if argv is None else argv)
    if arguments and arguments[0] == "init":
        raise SystemExit(init_main(arguments[1:]))
    if arguments and arguments[0] == "config-schema":
        if len(arguments) != 1:
            raise SystemExit("usage: erebus-mcp-server config-schema")
        raise SystemExit(schema_main())

    parser = argparse.ArgumentParser(prog="erebus-mcp-server")
    parser.add_argument("--config", type=Path)
    options = parser.parse_args(arguments)
    try:
        config_path = resolve_config_path(options.config)
        if config_path is not None:
            load_config_file(config_path)
        elif not environment_is_configured():
            if sys.stdin.isatty():
                created = default_config_path()
                print(
                    "Erebus is not configured. Starting one-time setup.", file=sys.stderr
                )
                raise SystemExit(init_main(["--config", str(created)]))
            raise OnboardingError(
                "Erebus is not configured. A marketplace must inject the fields from "
                "`erebus-mcp-server config-schema`; locally run `erebus-init` first."
            )
        build_server().run()
    except (ConfigError, OnboardingError) as exc:
        print(f"erebus-mcp-server: {exc}", file=sys.stderr)
        raise SystemExit(2) from exc


if __name__ == "__main__":
    main()
