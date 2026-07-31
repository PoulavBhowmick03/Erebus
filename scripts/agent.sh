#!/usr/bin/env bash
# Verb-level wrapper for driving one Erebus identity from an autonomous agent.
#
# An LLM agent should not be assembling nine-field config JSON, and it should not be shown
# more output than it needs to decide. Every verb here prints a few lines. Errors print the
# code and message and exit non-zero.
#
#   agent.sh <env> open      <counterparty_addr>          -> handle
#   agent.sh <env> offer     <handle> <amount> [memo_hex]  -> offer_id
#   agent.sh <env> counter   <handle> <reply_to> <amount> [memo_hex]
#   agent.sh <env> accept    <handle> <offer_id>           -> tx hash
#   agent.sh <env> status    <handle>                      -> compact transcript
#   agent.sh <env> wait      <handle> <count> [timeout_s]  -> blocks until N offers exist
#   agent.sh <env> whoami                                  -> this identity's address
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
    local method="$1" params="${2:-{\}}" out
    out="$(mktemp)"
    python3 "$REQ" "$ENV_FILE" "$method" "$params" | "$CLI" > "$out"
    if ! python3 -c 'import sys,json;sys.exit(0 if json.load(sys.stdin).get("ok") else 1)' < "$out"; then
        python3 -c 'import sys,json; e=json.load(sys.stdin)["error"]; print(e["code"] + ": " + e["message"], file=sys.stderr)' < "$out"
        rm -f "$out"; return 1
    fi
    python3 -c 'import sys,json;print(json.dumps(json.load(sys.stdin)["result"]))' < "$out"
    rm -f "$out"
}

# Deadline is 24h out. Agents negotiating over minutes have no reason to think about it, and
# an expired offer is rejected client-side with a distinct error rather than silently.
deadline() { python3 -c 'import time;print(int(time.time())+86400)'; }

# One line per offer: who proposed it, what it asks, what became of it. Enough to apply a
# threshold rule, small enough not to flood a model's context.
summarise() { python3 "$REPO/scripts/summarise.py"; }

# Pull one field out of a result object. Callers assign `call` output to a variable first
# and feed it here, rather than piping: a failing `call` in a pipeline sends empty stdin
# downstream, and the resulting JSON traceback buries the error that actually mattered.
field() { python3 -c 'import sys,json; print(json.load(sys.stdin)[sys.argv[1]])' "$1"; }

# An offer id is <handle>:<author>:<index>, but `status` prints only the tail because the
# handle is 67 identical characters on every line. Accept either form here so an agent can
# copy what it just read instead of reassembling it.
offer_ref() { case "$1" in ch_*) printf '%s' "$1" ;; *) printf '%s:%s' "$2" "$1" ;; esac; }

case "$VERB" in
whoami)
    grep '^AGENT_ADDRESS=' "$ENV_FILE" | cut -d= -f2
    ;;
open)
    out=$(call open_channel "{\"counterparty\":\"${1:?counterparty address}\"}")
    field channel_handle <<<"$out"
    ;;
offer)
    handle="${1:?handle}"; amount="${2:?amount in wei}"; memo="${3:-0x1234}"
    token=$(grep '^TOKEN_ADDRESS=' "$ENV_FILE" | cut -d= -f2)
    out=$(call propose_offer "$(printf '{"handle":"%s","terms":{"amount":"%s","token":"%s","deadline":%s,"memo_hash":"%s"}}' \
        "$handle" "$amount" "$token" "$(deadline)" "$memo")")
    field offer_id <<<"$out"
    ;;
counter)
    handle="${1:?handle}"; reply=$(offer_ref "${2:?offer_id being countered}" "${1}"); amount="${3:?amount in wei}"; memo="${4:-0x5678}"
    token=$(grep '^TOKEN_ADDRESS=' "$ENV_FILE" | cut -d= -f2)
    out=$(call counter_offer "$(printf '{"handle":"%s","reply_to":"%s","terms":{"amount":"%s","token":"%s","deadline":%s,"memo_hash":"%s"}}' \
        "$handle" "$reply" "$amount" "$token" "$(deadline)" "$memo")")
    field offer_id <<<"$out"
    ;;
accept)
    out=$(call accept_and_settle "{\"handle\":\"${1:?handle}\",\"offer_id\":\"$(offer_ref "${2:?offer_id}" "${1}")\"}")
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
*)
    echo "unknown verb: $VERB" >&2; exit 2
    ;;
esac
