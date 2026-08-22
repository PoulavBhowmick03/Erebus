#!/usr/bin/env bash
# Launches one Erebus MCP server for one identity, over stdio.
#
#   scripts/erebus-mcp.sh <env-file> <payer|payee|both>
#
# An MCP client uses stdin and stdout for JSON-RPC. Send all diagnostics to stderr.
#
# The env file is the identity's own (docs/runbook.md §1): AGENT_ADDRESS,
# PROVING_SERVICE_URL, STARKNET_RPC_URL, POOL_ADDRESS, STARKNET_CHAIN_ID, TOKEN_ADDRESS,
# POOL_KEY_FILE, ACCOUNT_KEY_FILE, EREBUS_STATE_DIR, and optional EREBUS_WIRE_VERSION
# (v3 by default). Config validates them at startup
# rather than on the first tool call.
#
# Run one server per identity. Two identities in one process put both pool keys in the same
# heap. See the two-server decision in docs/ishita.md.

set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENV_FILE="${1:?usage: erebus-mcp.sh <env-file>}"
SETTLEMENT_ROLE="${2:?usage: erebus-mcp.sh <env-file> <payer|payee|both>}"
case "$SETTLEMENT_ROLE" in
    payer|payee|both) ;;
    *) echo "settlement role must be payer, payee, or both" >&2; exit 2 ;;
esac

# Optional first-start provisioning runs before the server binds its identity at import.
# This lets one MCP configuration create a funded and registered identity.
#
#   EREBUS_PROVISION_FROM=<funder sncast account>   seeds gas from an existing identity
#   EREBUS_PROVISION_STRK=<amount>                  defaults to 15
if [ ! -r "$ENV_FILE" ] && [ -n "${EREBUS_PROVISION_FROM:-}" ]; then
    DIR="$(dirname "$ENV_FILE")"
    NAME="$(basename "$DIR")"
    echo "no identity at $ENV_FILE; provisioning $NAME from $EREBUS_PROVISION_FROM" >&2
    # stdout belongs to the MCP protocol, so the whole noisy setup goes to stderr.
    "$REPO/scripts/new-identity.sh" bootstrap "$NAME" "$DIR" \
        "$EREBUS_PROVISION_FROM" "${EREBUS_PROVISION_STRK:-15}" >&2
fi

[ -r "$ENV_FILE" ] || { echo "cannot read env file: $ENV_FILE" >&2; exit 1; }

set -a
# shellcheck disable=SC1090
. "$ENV_FILE"
set +a

export EREBUS_BACKEND="${EREBUS_BACKEND:-seam}"
export EREBUS_SETTLEMENT_ROLE="$SETTLEMENT_ROLE"
export EREBUS_CLI="${EREBUS_CLI:-$REPO/sdk/rs/target/debug/erebus-cli}"
export PYTHONPATH="$REPO/mcp-server/src${PYTHONPATH:+:$PYTHONPATH}"

if [ ! -x "$EREBUS_CLI" ]; then
    echo "erebus-cli not built at $EREBUS_CLI" >&2
    echo "run: cd $REPO/sdk/rs && cargo build --bin erebus-cli" >&2
    exit 1
fi

exec uv run --directory "$REPO" python "$REPO/mcp-server/src/server.py"
