#!/usr/bin/env bash
# Run the local prover and its private screening sidecar without printing secrets.

set -euo pipefail

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
export PROVER_ENV_FILE="${PROVER_ENV_FILE:-$HOME/.erebus/prover.env}"
export SCREENING_ENV_FILE="${SCREENING_ENV_FILE:-$HOME/.erebus/screening.env}"

[ -r "$PROVER_ENV_FILE" ] || {
    echo "prover env is not readable: $PROVER_ENV_FILE" >&2
    exit 1
}
[ -r "$SCREENING_ENV_FILE" ] || {
    echo "screening env is not readable: $SCREENING_ENV_FILE" >&2
    echo "run scripts/write-screening-env.sh after the operator supplies access" >&2
    exit 1
}

exec docker compose -f "$SCRIPT_DIR/compose.yaml" "$@"
