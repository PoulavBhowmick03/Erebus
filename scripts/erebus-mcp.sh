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

# Provision on first start when asked to. The server binds one identity at import, so an
# agent cannot ask it to create a wallet and then use it — the identity is already fixed by
# then. Doing it here means one line of MCP config gives an agent a funded, registered
# identity on first use, with no human step after the config.
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
export EREBUS_CLI="${EREBUS_CLI:-$REPO/sdk/rs/target/debug/erebus-cli}"
export PYTHONPATH="$REPO/mcp-server/src${PYTHONPATH:+:$PYTHONPATH}"

if [ ! -x "$EREBUS_CLI" ]; then
    echo "erebus-cli not built at $EREBUS_CLI" >&2
    echo "run: cd $REPO/sdk/rs && cargo build --bin erebus-cli" >&2
    exit 1
fi

exec uv run --directory "$REPO" python "$REPO/mcp-server/src/server.py"
