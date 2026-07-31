#!/usr/bin/env bash
# Runs the on-chain negotiation demonstration end to end: open a channel, write an offer
# into the salt lane, read it back.
#
# There is no step here that asks you to copy a value from one command into the next. That
# is deliberate — the handle is captured, not pasted. Both times this was written as a
# copy-paste block, the placeholder went in literally and the run failed with
# "invalid channel handle: ch_...".
#
# Usage:  scripts/demo.sh [amount-wei]
#
# Requires: .env populated for agent A, ~/.erebus-b/env for agent B, both identities
# already registered (see docs/runbook.md §1-2), and erebus-cli built.

set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CLI="${CLI:-$REPO/sdk/rs/target/debug/erebus-cli}"
REQ="${REQ:-$HOME/.erebus/req.py}"
ENV_A="${ENV_A:-$REPO/.env}"
ENV_B="${ENV_B:-$HOME/.erebus-b/env}"
AMOUNT="${1:-500000000000000000}"

for f in "$CLI" "$REQ" "$ENV_A" "$ENV_B"; do
    [ -e "$f" ] || { echo "missing: $f" >&2; exit 1; }
done

STRK=$(grep '^TOKEN_ADDRESS=' "$ENV_A" | cut -d= -f2)
B=$(grep '^AGENT_ADDRESS=' "$ENV_B" | cut -d= -f2)

# Fails the script on an {"ok":false} envelope rather than letting the next step run with a
# nonsense value — which is how a readable error becomes an unreadable one two steps later.
#
# The output goes to a file rather than stdout because `exit 1` inside `$(...)` only kills
# the subshell, and in `X=$(call ... | python3 ...)` the pipeline's status is python's. The
# first version of this swallowed a SUBMIT_FAILED and reported a JSONDecodeError instead.
call() {
    local env="$1" method="$2" params="${3:-{\}}" out
    out="$(mktemp)"
    python3 "$REQ" "$env" "$method" "$params" | "$CLI" > "$out"
    if ! python3 -c 'import sys,json; sys.exit(0 if json.load(sys.stdin).get("ok") else 1)' < "$out"; then
        echo "$method failed:" >&2
        python3 -m json.tool < "$out" >&2
        rm -f "$out"
        return 1
    fi
    cat "$out"
    rm -f "$out"
}

echo "counterparty: $B"
echo
echo "==> open_channel  (~20s: preflight, proof, estimate, submit, receipt)"
# Idempotent: the pool's channel key takes no index (hashes.cairo:119) and its marker is
# WriteOnce, so there is exactly one channel per pair, ever. A second open returns the
# existing handle instead of spending a proof to discover a revert. See F29.
HANDLE=$(call "$ENV_A" open_channel "{\"counterparty\":\"$B\"}" \
    | python3 -c 'import sys,json;print(json.load(sys.stdin)["result"]["channel_handle"])')
echo "    handle: $HANDLE"

# And because one channel per pair meets our one-deal-per-channel rule, a settled pair can
# never trade again with this client. Say so here rather than failing at propose_offer with
# ALREADY_SETTLED, which reads like a bug in the script.
if call "$ENV_A" read_channel_state "{\"handle\":\"$HANDLE\"}" \
    | python3 -c 'import sys,json;sys.exit(0 if json.load(sys.stdin)["result"]["settled"] else 1)'; then
    cat >&2 <<MSG

This pair has already settled, and the channel is terminal.

There is exactly one channel per (sender, recipient) pair — the pool derives the channel
key without an index and writes its marker WriteOnce — so this is not a state you can
clear locally. To run the demo again, create a third identity (docs/runbook.md §1) and
point ENV_B at it.
MSG
    exit 1
fi

echo
echo "==> propose_offer  (4 zero-amount notes, one action set, one proof)"
DEADLINE=$(python3 -c 'import time;print(int(time.time())+86400)')
TERMS=$(printf '{"handle":"%s","terms":{"amount":"%s","token":"%s","deadline":%s,"memo_hash":"0x1234"}}' \
    "$HANDLE" "$AMOUNT" "$STRK" "$DEADLINE")
OFFER=$(call "$ENV_A" propose_offer "$TERMS" \
    | python3 -c 'import sys,json;print(json.load(sys.stdin)["result"]["offer_id"])')
echo "    offer:  $OFFER"

echo
echo "==> read_channel_state  (reassembled from note salts, nothing in calldata)"
STATE=$(call "$ENV_A" read_channel_state "{\"handle\":\"$HANDLE\"}")
python3 -m json.tool <<<"$STATE"

# A wrong derivation does not raise: the writes succeed and the read comes back empty,
# because a misderived note id addresses a slot nobody wrote to. So an empty transcript
# after a successful write is the failure, and it needs to be asserted rather than eyeballed.
python3 - "$AMOUNT" "$DEADLINE" <<'PY' <<<"$STATE"
import json, sys
amount, deadline = int(sys.argv[1]), int(sys.argv[2])
offers = json.load(sys.stdin)["result"]["offers"]
if not offers:
    sys.exit("FAIL: transcript empty after a successful write — the note ids are misderived")
t = offers[-1]["terms"]
assert t["amount"] == amount, f'amount {t["amount"]} != {amount}'
assert t["deadline"] == deadline, f'deadline {t["deadline"]} != {deadline}'
assert t["memo_hash"] == 0x1234, f'memo_hash {t["memo_hash"]:#x} != 0x1234'
print("\nOK: every field round-tripped through the salt lane intact.")
PY
