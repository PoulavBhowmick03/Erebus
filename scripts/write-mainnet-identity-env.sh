#!/usr/bin/env bash
# Write one protected Erebus mainnet runtime configuration without printing secrets.
#
# Usage:
#   write-mainnet-identity-env.sh <address> <account-key> <pool-key> <state-dir> <output-env>
#
# RPC_URL is read from ~/.erebus/prover.env by default. Override PROVER_ENV or
# PROVING_SERVICE_URL when the prover runs elsewhere.

set -euo pipefail

ADDRESS="${1:?account address}"
ACCOUNT_KEY="${2:?account key file}"
POOL_KEY="${3:?pool key file}"
STATE_DIR="${4:?state directory}"
OUTPUT_ENV="${5:?output env file}"
PROVER_ENV="${PROVER_ENV:-$HOME/.erebus/prover.env}"
PROVING_SERVICE_URL="${PROVING_SERVICE_URL:-http://127.0.0.1:3000}"

case "$ADDRESS" in 0x[0-9a-fA-F]*) ;; *) echo "address must be a 0x-prefixed felt" >&2; exit 2 ;; esac
case "$ACCOUNT_KEY" in /*) ;; *) echo "account key path must be absolute" >&2; exit 2 ;; esac
case "$POOL_KEY" in /*) ;; *) echo "pool key path must be absolute" >&2; exit 2 ;; esac
case "$STATE_DIR" in /*) ;; *) echo "state directory must be absolute" >&2; exit 2 ;; esac
case "$OUTPUT_ENV" in /*) ;; *) echo "output env path must be absolute" >&2; exit 2 ;; esac

[ -r "$ACCOUNT_KEY" ] || { echo "account key is not readable: $ACCOUNT_KEY" >&2; exit 1; }
[ -r "$POOL_KEY" ] || { echo "pool key is not readable: $POOL_KEY" >&2; exit 1; }
[ -r "$PROVER_ENV" ] || { echo "prover env is not readable: $PROVER_ENV" >&2; exit 1; }

RPC_URL=$(sed -n 's/^RPC_URL=//p' "$PROVER_ENV")
[ -n "$RPC_URL" ] || { echo "RPC_URL is missing from $PROVER_ENV" >&2; exit 1; }

OUTPUT_DIR=$(dirname "$OUTPUT_ENV")
mkdir -p "$OUTPUT_DIR" "$STATE_DIR"
chmod 700 "$OUTPUT_DIR" "$STATE_DIR"
[ ! -e "$OUTPUT_ENV" ] || { echo "refusing to overwrite: $OUTPUT_ENV" >&2; exit 1; }

TEMP_ENV=$(mktemp "$OUTPUT_ENV.tmp.XXXXXX")
trap 'rm -f "$TEMP_ENV"' EXIT
chmod 600 "$TEMP_ENV"
printf '%s\n' \
    'EREBUS_NETWORK=mainnet' \
    'STARKNET_CHAIN_ID=0x534e5f4d41494e' \
    "STARKNET_RPC_URL=$RPC_URL" \
    'POOL_ADDRESS=0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a' \
    "PROVING_SERVICE_URL=$PROVING_SERVICE_URL" \
    'INDEXER_URL=' \
    "AGENT_ADDRESS=$ADDRESS" \
    "POOL_KEY_FILE=$POOL_KEY" \
    "ACCOUNT_KEY_FILE=$ACCOUNT_KEY" \
    "EREBUS_STATE_DIR=$STATE_DIR" \
    'TOKEN_ADDRESS=0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d' \
    'EREBUS_WIRE_VERSION=v3' \
    > "$TEMP_ENV"
mv "$TEMP_ENV" "$OUTPUT_ENV"
trap - EXIT

echo "mainnet identity env written: $OUTPUT_ENV"
