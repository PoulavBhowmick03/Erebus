#!/usr/bin/env bash
# Creates and activates one Erebus identity in phases around funding.
#
#   scripts/new-identity.sh create    <name> <dir>              -> prints an address to fund
#   scripts/new-identity.sh fund      <funder-account> <addr> [strk]
#   scripts/new-identity.sh activate  <name> <dir>              -> deploy, keygen, env, shield
#   scripts/new-identity.sh bootstrap <name> <dir> <funder-account> [strk]
#
# `bootstrap` runs all three phases without a human step. It needs a funded account for gas
# because the public faucet uses a captcha.
#
# The deployed account pays for deployment. Fund its counterfactual address before deploy.
#
# Example, unattended:
#   scripts/new-identity.sh bootstrap erebus-d ~/.erebus-d erebus-agent 15
#
# Example, faucet:
#   scripts/new-identity.sh create   erebus-d ~/.erebus-d
#   # fund the printed address at https://starknet-faucet.vercel.app
#   scripts/new-identity.sh activate erebus-d ~/.erebus-d
#
# WARNING: Activation registers the identity permanently and writes its pool private key
# encrypted to the pool auditor (privacy.cairo:329-334). Use testnet keys only.

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

# "none" when the address holds no contract yet, which is how a counterfactual account looks
# before its deploy is accepted.
class_hash_at() {
    curl -fsS -m 15 -X POST "$RPC" -H 'content-type: application/json' \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"starknet_getClassHashAt\",\"params\":[\"latest\",\"$1\"]}" \
        | python3 -c 'import sys,json; print(json.load(sys.stdin).get("result","none"))' 2>/dev/null || echo none
}

address_of() {
    python3 -c '
import json, os, sys
data = json.load(open(os.path.expanduser("~/.starknet_accounts/starknet_open_zeppelin_accounts.json")))
print(data["alpha-sepolia"][sys.argv[1]]["address"])' "$1"
}

# STRK, as the u256 low/high pair every ERC-20 entrypoint wants.
strk_u256() {
    python3 -c '
import sys
from decimal import Decimal
wei = int(Decimal(sys.argv[1]) * Decimal(10) ** 18)
print(hex(wei & ((1 << 128) - 1)), hex(wei >> 128))' "$1"
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

    ADDR=$(address_of "$NAME")
    if [ "$(class_hash_at "$ADDR")" = "none" ]; then
        echo "==> deploying account"
        DEPLOY_TX=$(sncast account deploy --url "$RPC" --name "$NAME" \
                    | grep -oE '0x[0-9a-f]{6,}' | head -1)
        echo "    $DEPLOY_TX"
        # `account deploy` returns at submission. Wait for acceptance before the next call or
        # approve fails with "Account ... not found on network".
        bash "$REPO/scripts/wait-for-depth.sh" "$DEPLOY_TX" 1
    else
        echo "==> account already deployed"
    fi

    echo "==> key files"
    mkdir -p "$DIR/state"
    chmod 700 "$DIR" "$DIR/state"
    # Key commands refuse to overwrite. Skip existing files so reruns can resume.
    if [ -e "$DIR/pool.key" ]; then
        echo "    pool key already present"
    else
        "$CLI" <<<"{\"method\":\"generate_pool_key\",\"params\":{\"path\":\"$DIR/pool.key\"}}"
    fi
    if [ -e "$DIR/account.key" ]; then
        echo "    account key already present"
    else
        python3 "$REPO/scripts/extract-sncast-account-key.py" "$NAME" "$DIR"
    fi

    echo "==> env"
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

fund)
    # Reuses the positional slots: <funder-account> <recipient-address> [strk].
    FUNDER="$NAME"
    RECIPIENT="$DIR"
    AMOUNT="${4:-15}"
    ENV_ANY="${EREBUS_ENV:-$REPO/.env}"
    STRK_TOKEN=$(grep '^TOKEN_ADDRESS=' "$ENV_ANY" | cut -d= -f2)

    read -r LOW HIGH <<<"$(strk_u256 "$AMOUNT")"
    echo "==> sending $AMOUNT STRK from $FUNDER to $RECIPIENT"
    TX=$(sncast --account "$FUNDER" invoke --url "$RPC" \
            --contract-address "$STRK_TOKEN" --function transfer \
            --calldata "$RECIPIENT" "$LOW" "$HIGH" \
         | grep -oE '0x[0-9a-f]{6,}' | tail -1)
    echo "    $TX"
    # One block is enough here. The recipient only needs a balance before it deploys, and
    # nothing proves against this transfer, so the head - 10 rule does not apply.
    bash "$REPO/scripts/wait-for-depth.sh" "$TX" 1
    ;;

bootstrap)
    FUNDER="${4:?funder sncast account, e.g. erebus-agent}"
    AMOUNT="${5:-15}"
    "$0" create "$NAME" "$DIR"
    "$0" fund "$FUNDER" "$(address_of "$NAME")" "$AMOUNT"
    "$0" activate "$NAME" "$DIR"
    ;;

*)
    echo "unknown phase: $PHASE (expected create, fund, activate or bootstrap)" >&2
    exit 2
    ;;
esac
