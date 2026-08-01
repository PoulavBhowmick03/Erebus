#!/usr/bin/env bash
# Wait until a Starknet transaction is accepted and at least N blocks deep.

set -euo pipefail

TX_HASH="${1:-}"
REQUIRED_DEPTH="${2:-10}"
RPC_URL="${RPC:-${STARKNET_RPC_URL:-}}"

if [[ -z "$TX_HASH" || -z "$RPC_URL" ]]; then
    echo "usage: RPC=<url> $0 <transaction-hash> [required-depth]" >&2
    exit 2
fi
if [[ ! "$REQUIRED_DEPTH" =~ ^[0-9]+$ ]]; then
    echo "required depth must be a non-negative integer" >&2
    exit 2
fi

rpc() {
    curl -fsS -m 15 -X POST "$RPC_URL" \
        -H 'content-type: application/json' \
        -d "$1"
}

transaction_block=""
while [[ -z "$transaction_block" ]]; do
    receipt=$(rpc "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"starknet_getTransactionReceipt\",\"params\":[\"$TX_HASH\"]}")
    transaction_block=$(python3 -c '
import json, sys
result = json.load(sys.stdin).get("result", {})
if result.get("execution_status") == "REVERTED":
    raise SystemExit("transaction reverted: " + result.get("revert_reason", "unknown reason"))
print(result.get("block_number", ""))
' <<<"$receipt")
    if [[ -z "$transaction_block" ]]; then
        echo "  waiting for transaction acceptance"
        sleep 10
    fi
done

while :; do
    head=$(rpc '{"jsonrpc":"2.0","id":1,"method":"starknet_blockNumber","params":[]}' \
        | python3 -c 'import json,sys; print(json.load(sys.stdin)["result"])')
    depth=$((head - transaction_block))
    echo "  depth $depth/$REQUIRED_DEPTH (tx block $transaction_block, head $head)"
    ((depth >= REQUIRED_DEPTH)) && break
    sleep 10
done
