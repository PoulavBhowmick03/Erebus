#!/usr/bin/env bash
# Command wrapper for one Erebus identity.
#
# The wrapper builds the versioned configuration and limits output. Each command prints a
# few lines. Errors print a code and message and return a non-zero status.
#
#   agent.sh <env> open      <counterparty_addr>          -> handle
#   agent.sh <env> offer     <handle> <amount> [memo_hex]  -> offer_id
#   agent.sh <env> counter   <handle> <reply_to> <amount> [memo_hex]
#   agent.sh <env> accept    <handle> <offer_id>           -> tx hash
#   agent.sh <env> status    <handle>                      -> compact transcript
#   agent.sh <env> wait      <handle> <count> [timeout_s]  -> blocks until N offers exist
#   agent.sh <env> balance                                 -> spendable note denominations
#   agent.sh <env> doctor                                  -> read-only readiness report
#   agent.sh <env> fund      <amount>                      -> approve + shield one note
#   agent.sh <env> whoami                                  -> this identity's address
#
# Check `balance` before naming a price. Settlement selects inputs that cover the price and
# returns the remainder as payer-owned change, so any amount up to the spendable total is
# payable. A standing pool allowance is a precondition for every charged write: see the
# `approve` and `allowance` methods on erebus-cli.
#
# <env> is a file of KEY=VALUE lines: the repo .env for agent A, ~/.erebus-b/env for B, and
# so on. It names the key files by path. Key values never appear in any argument here.
#
# Each write costs about 3 STRK of gas and about 20 seconds of proving (F27), so an agent
# loop should not poll faster than a few seconds.

set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CLI="${CLI:-$REPO/sdk/rs/target/debug/erebus-cli}"
REQ="${REQ:-$REPO/scripts/erebus-request.py}"

ENV_FILE="${1:?usage: agent.sh <env> <verb> [args]}"
VERB="${2:?usage: agent.sh <env> <verb> [args]}"
shift 2

[ -e "$ENV_FILE" ] || { echo "no such env file: $ENV_FILE" >&2; exit 1; }

call() {
    local method="$1" params="${2:-{\}}" out status
    out="$(mktemp)"
    set +e
    python3 "$REQ" "$ENV_FILE" "$method" "$params" | "$CLI" > "$out"
    status=$?
    set -e
    if [ "$status" -ne 0 ] && [ ! -s "$out" ]; then
        echo "erebus-cli exited with status $status and no response" >&2
        rm -f "$out"; return "$status"
    fi
    if ! python3 -c 'import sys,json;sys.exit(0 if json.load(sys.stdin).get("ok") else 1)' < "$out"; then
        python3 -c 'import sys,json; e=json.load(sys.stdin)["error"]; print(e["code"] + ": " + e["message"], file=sys.stderr)' < "$out"
        rm -f "$out"; return 1
    fi
    python3 -c 'import sys,json;print(json.dumps(json.load(sys.stdin)["result"]))' < "$out"
    rm -f "$out"
}

write_call() {
    local method="$1" params="$2" operation_id state_dir intent_file bound
    operation_id="${EREBUS_OPERATION_ID:-$(python3 -c 'import secrets; print("op_" + secrets.token_hex(32))')}"
    state_dir=$(grep '^EREBUS_STATE_DIR=' "$ENV_FILE" | cut -d= -f2-)
    intent_file="$state_dir/caller-intents.jsonl"
    mkdir -p "$state_dir"; chmod 700 "$state_dir"
    bound=$(python3 -c 'import json,sys; value=json.loads(sys.argv[1]); value["operation_id"]=sys.argv[2]; print(json.dumps(value,separators=(",",":"),sort_keys=True))' "$params" "$operation_id")
    python3 -c 'import json,os,sys; path=sys.argv[1]; row={"operation_id":sys.argv[2],"method":sys.argv[3],"params":json.loads(sys.argv[4])}; fd=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_APPEND,0o600); os.write(fd,(json.dumps(row,separators=(",",":"),sort_keys=True)+"\n").encode()); os.fsync(fd); os.close(fd)' "$intent_file" "$operation_id" "$method" "$bound"
    call "$method" "$bound"
}

# Use a 24-hour deadline for negotiations that take minutes. The client returns a distinct
# error for an expired offer.
deadline() { python3 -c 'import time;print(int(time.time())+86400)'; }

# Print one line per offer for threshold decisions.
summarise() { python3 "$REPO/scripts/summarise.py"; }

# Read one field from a result object. Callers save `call` output before using this function.
# A pipeline can replace the original error with a JSON traceback on empty input.
field() { python3 -c 'import sys,json; print(json.load(sys.stdin)[sys.argv[1]])' "$1"; }

# `status` omits the repeated 67-character handle from each offer id. Accept full and short
# forms so callers can reuse its output.
offer_ref() { case "$1" in ch_*) printf '%s' "$1" ;; *) printf '%s:%s' "$2" "$1" ;; esac; }

case "$VERB" in
whoami)
    grep '^AGENT_ADDRESS=' "$ENV_FILE" | cut -d= -f2
    ;;
open)
    out=$(write_call open_channel "{\"counterparty\":\"${1:?counterparty address}\"}")
    field channel_handle <<<"$out"
    ;;
offer)
    handle="${1:?handle}"; amount="${2:?amount in wei}"; memo="${3:-0x1234}"
    token=$(grep '^TOKEN_ADDRESS=' "$ENV_FILE" | cut -d= -f2)
    out=$(write_call propose_offer "$(printf '{"handle":"%s","terms":{"amount":"%s","token":"%s","deadline":%s,"memo_hash":"%s"}}' \
        "$handle" "$amount" "$token" "$(deadline)" "$memo")")
    field offer_id <<<"$out"
    ;;
counter)
    handle="${1:?handle}"; reply=$(offer_ref "${2:?offer_id being countered}" "${1}"); amount="${3:?amount in wei}"; memo="${4:-0x5678}"
    token=$(grep '^TOKEN_ADDRESS=' "$ENV_FILE" | cut -d= -f2)
    out=$(write_call counter_offer "$(printf '{"handle":"%s","reply_to":"%s","terms":{"amount":"%s","token":"%s","deadline":%s,"memo_hash":"%s"}}' \
        "$handle" "$reply" "$amount" "$token" "$(deadline)" "$memo")")
    field offer_id <<<"$out"
    ;;
accept)
    out=$(write_call accept_and_settle "{\"handle\":\"${1:?handle}\",\"offer_id\":\"$(offer_ref "${2:?offer_id}" "${1}")\"}")
    field tx_hash <<<"$out"
    ;;
status)
    # Assign before summarising. Piping a failing `call` feeds empty stdin to the
    # summariser, which then dies with a JSON traceback and buries the real error.
    state=$(call read_channel_state "{\"handle\":\"${1:?handle}\"}")
    summarise <<<"$state"
    ;;
wait)
    handle="${1:?handle}"; want="${2:?expected offer count}"; timeout="${3:-300}"
    start=$(date +%s)
    while :; do
        state=$(call read_channel_state "{\"handle\":\"$handle\"}")
        have=$(python3 -c 'import sys,json;print(len(json.load(sys.stdin)["offers"]))' <<<"$state")
        if [ "$have" -ge "$want" ]; then
            summarise <<<"$state"; exit 0
        fi
        if [ $(( $(date +%s) - start )) -ge "$timeout" ]; then
            echo "timed out after ${timeout}s with $have/$want offers" >&2; exit 1
        fi
        sleep 10
    done
    ;;
balance)
    out=$(call balance '{}')
    python3 "$REPO/scripts/balance.py" <<<"$out"
    ;;
doctor)
    call doctor '{}'
    ;;
reconcile)
    call reconcile '{}'
    ;;
resume)
    call resume_operation "{\"operation_id\":\"${1:?operation id}\"}"
    ;;
fund)
    # The documented two-transaction deposit (runbook §2): the ERC-20 approve must be on
    # chain and `proving_block_lag` deep before the shield simulates, or the shield fails
    # with a bare Contract error naming nothing (F20). Doing it in one verb is the only way
    # an autonomous agent gets past this without a human pasting a transaction hash.
    amount="${1:?amount in wei}"
    token=$(grep '^TOKEN_ADDRESS=' "$ENV_FILE" | cut -d= -f2)
    pool=$(grep '^POOL_ADDRESS=' "$ENV_FILE" | cut -d= -f2)
    rpc=$(grep '^STARKNET_RPC_URL=' "$ENV_FILE" | cut -d= -f2)
    me=$(grep '^AGENT_ADDRESS=' "$ENV_FILE" | cut -d= -f2)

    # Resolve the signer by address rather than taking a name on faith: approving from the
    # wrong account silently grants the wrong allowance and the shield still fails.
    account="${SNCAST_ACCOUNT:-$(python3 -c '
import json,sys,pathlib
want = int(sys.argv[1], 16)
path = pathlib.Path.home() / ".starknet_accounts/starknet_open_zeppelin_accounts.json"
if not path.exists():
    sys.exit("no sncast accounts file; set SNCAST_ACCOUNT")
for network, accounts in json.loads(path.read_text()).items():
    for name, body in accounts.items():
        if int(body.get("address", "0x0"), 16) == want:
            print(name); raise SystemExit(0)
sys.exit("no sncast account matches " + sys.argv[1] + "; set SNCAST_ACCOUNT")
' "$me")}"

    # The pool pulls twice from this account in the shield transaction: the deposit itself,
    # and `collect_fee` before it applies anything (privacy.cairo:790). One allowance covers
    # both, so approving only the deposit is short by exactly the fee. That was invisible
    # while Sepolia charged nothing; it charges 2 STRK now and mainnet charges 6, and the
    # shortfall surfaces as a bare Contract error naming nothing, which is F20 again.
    #
    # Read the fee rather than hard-coding it: it is pool storage that `set_fee_amount` can
    # change, and it already differs per network.
    fee=$(call allowance '{}' | field fee_per_write)
    approve_amount=$(python3 -c 'import sys;print(int(sys.argv[1])+int(sys.argv[2]))' "$amount" "$fee")

    low=$(python3 -c 'print(hex(int(__import__("sys").argv[1]) & ((1<<128)-1)))' "$approve_amount")
    high=$(python3 -c 'print(hex(int(__import__("sys").argv[1]) >> 128))' "$approve_amount")

    echo "approving $approve_amount ($amount deposit + $fee fee) for pool $pool as sncast account '$account'" >&2
    tx=$(sncast --account "$account" invoke --url "$rpc" \
        --contract-address "$token" --function approve \
        --calldata "$pool" "$low" "$high" \
        | awk '/Transaction Hash:/ {print $3}')
    [ -n "$tx" ] || { echo "approve did not report a transaction hash" >&2; exit 1; }
    echo "approve tx $tx" >&2

    RPC="$rpc" bash "$REPO/scripts/wait-for-depth.sh" "$tx" >&2
    out=$(write_call shield "$(printf '{"amount":"%s"}' "$amount")")
    shield_tx=$(field tx_hash <<<"$out")
    echo "shield tx $shield_tx; waiting until its note is spendable" >&2
    RPC="$rpc" bash "$REPO/scripts/wait-for-depth.sh" "$shield_tx" >&2
    echo "$shield_tx"
    ;;
*)
    echo "unknown verb: $VERB" >&2; exit 2
    ;;
esac
