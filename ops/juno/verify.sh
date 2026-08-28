#!/usr/bin/env bash
set -euo pipefail

readonly RPC_URL="http://127.0.0.1:${JUNO_HTTP_PORT:-6060}/v0_10"

rpc() {
  local id="$1"
  local method="$2"
  curl --fail-with-body --silent --show-error "${RPC_URL}" \
    -H 'Content-Type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":${id},\"method\":\"${method}\",\"params\":[]}"
  printf '\n'
}

rpc 1 starknet_chainId
rpc 2 starknet_specVersion
rpc 3 starknet_syncing

echo "RPC checks completed; starknet_syncing must be false before starting the prover"
