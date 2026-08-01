#!/usr/bin/env bash
# Launches one Erebus MCP server for one identity, over stdio.
#
#   scripts/erebus-mcp.sh <env-file>
#
# An MCP client spawns this and speaks JSON-RPC on stdin/stdout, so nothing may be printed
# to stdout that is not a protocol message. Diagnostics go to stderr.
#
# The env file is the identity's own (docs/runbook.md §1): AGENT_ADDRESS,
# PROVING_SERVICE_URL, STARKNET_RPC_URL, POOL_ADDRESS, STARKNET_CHAIN_ID, TOKEN_ADDRESS,
# POOL_KEY_FILE, ACCOUNT_KEY_FILE, EREBUS_STATE_DIR. Config validates all of them at startup
# rather than on the first tool call.
#
# One server per identity. Two identities in one process would hold both pool keys in the
# same heap, which is what the two-server decision in docs/ishita.md exists to avoid.

set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENV_FILE="${1:?usage: erebus-mcp.sh <env-file>}"

[ -r "$ENV_FILE" ] || { echo "cannot read env file: $ENV_FILE" >&2; exit 1; }

set -a
# shellcheck disable=SC1090
. "$ENV_FILE"
set +a

export EREBUS_BACKEND="${EREBUS_BACKEND:-seam}"
export EREBUS_CLI="${EREBUS_CLI:-$REPO/sdk/rs/target/debug/erebus-cli}"
export PYTHONPATH="$REPO/mcp-server/src${PYTHONPATH:+:$PYTHONPATH}"

if [ ! -x "$EREBUS_CLI" ]; then
    echo "erebus-cli not built at $EREBUS_CLI" >&2
    echo "run: cd $REPO/sdk/rs && cargo build --bin erebus-cli" >&2
    exit 1
fi

exec uv run --directory "$REPO" python "$REPO/mcp-server/src/server.py"
