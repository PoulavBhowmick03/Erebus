#!/usr/bin/env bash
# Creates and activates one Erebus identity, in two phases around the faucet.
#
#   scripts/new-identity.sh create   <name> <dir>   -> prints an address to fund
#   scripts/new-identity.sh activate <name> <dir>   -> deploy, keygen, env, approve, shield
#
# Two phases because funding is manual and everything after it depends on gas being there.
# Splitting at that boundary means a failed faucet does not leave half an identity behind.
#
# Example:
#   scripts/new-identity.sh create   erebus-d ~/.erebus-d
#   # fund the printed address at https://starknet-faucet.vercel.app
#   scripts/new-identity.sh activate erebus-d ~/.erebus-d
#
# Activation registers the identity with the pool, which is irreversible and writes the pool
# private key encrypted to the pool's auditor (privacy.cairo:329-334). Testnet keys only.

set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CLI="${CLI:-$REPO/sdk/rs/target/debug/erebus-cli}"
REQ="${REQ:-$REPO/scripts/erebus-request.py}"
# Exported because wait-for-depth.sh reads it from the environment.
export RPC="${RPC:-https://starknet-sepolia-rpc.publicnode.com}"

PHASE="${1:?usage: new-identity.sh create|activate <name> <dir>}"
NAME="${2:?identity name, e.g. erebus-d}"
DIR="${3:?directory, e.g. ~/.erebus-d}"
DIR="${DIR/#\~/$HOME}"

ACCOUNTS=~/.starknet_accounts/starknet_open_zeppelin_accounts.json

address_of() {
    python3 -c '
import json, os, sys
data = json.load(open(os.path.expanduser("~/.starknet_accounts/starknet_open_zeppelin_accounts.json")))
print(data["alpha-sepolia"][sys.argv[1]]["address"])' "$1"
}

case "$PHASE" in
create)
    sncast account create --url "$RPC" --name "$NAME"
    echo
    echo "Fund this address, then run:"
    echo "  $0 activate $NAME $DIR"
    echo
    echo "  https://starknet-faucet.vercel.app"
    echo "  address: $(address_of "$NAME")"
    echo
    echo "Budget ~15 STRK. Deployment is ~0.09, and every proof-carrying write is ~3 (F27)."
    ;;

activate)
    [ -r "$ACCOUNTS" ] || { echo "no accounts file; run '$0 create $NAME $DIR' first" >&2; exit 1; }

    echo "==> deploying account"
    sncast account deploy --url "$RPC" --name "$NAME"

    echo "==> key files"
    mkdir -p "$DIR/state"
    chmod 700 "$DIR" "$DIR/state"
    # Both refuse to overwrite, so re-running after a partial failure is safe.
    "$CLI" <<<"{\"method\":\"generate_pool_key\",\"params\":{\"path\":\"$DIR/pool.key\"}}"
    python3 "$REPO/scripts/extract-sncast-account-key.py" "$NAME" "$DIR"

    echo "==> env"
    ADDR=$(address_of "$NAME")
    sed -e "s|^AGENT_ADDRESS=.*|AGENT_ADDRESS=$ADDR|" \
        -e "s|^POOL_KEY_FILE=.*|POOL_KEY_FILE=$DIR/pool.key|" \
        -e "s|^ACCOUNT_KEY_FILE=.*|ACCOUNT_KEY_FILE=$DIR/account.key|" \
        -e "s|^EREBUS_STATE_DIR=.*|EREBUS_STATE_DIR=$DIR/state|" \
        "$REPO/.env" > "$DIR/env"
    chmod 600 "$DIR/env"

    STRK=$(grep '^TOKEN_ADDRESS=' "$DIR/env" | cut -d= -f2)
    POOL=$(grep '^POOL_ADDRESS=' "$DIR/env" | cut -d= -f2)

    echo "==> approving the pool for 1 STRK"
    TX=$(sncast --account "$NAME" invoke --url "$RPC" \
            --contract-address "$STRK" --function approve \
            --calldata "$POOL" 0xde0b6b3a7640000 0x0 \
         | grep -oE '0x[0-9a-f]{6,}' | tail -1)
    echo "    $TX"

    # The client proves against head - 10, so an approve newer than that is invisible to
    # the simulation and the shield fails with a bare -32603 carrying no reason (F20).
    echo "==> waiting for the approve to mature"
    bash "$REPO/scripts/wait-for-depth.sh" "$TX"

    echo "==> shielding 1 STRK, which also registers the identity"
    python3 "$REQ" "$DIR/env" shield '{"amount":"1000000000000000000"}' | "$CLI"

    echo
    echo "ready: $ADDR"
    echo "env:   $DIR/env"
    ;;

*)
    echo "unknown phase: $PHASE (expected create or activate)" >&2
    exit 2
    ;;
esac
