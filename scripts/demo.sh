#!/usr/bin/env bash
# Runs the on-chain negotiation demo: open both directional channels,
# offer, counter, atomically accept/pay, then reconstruct through a bearer viewing grant.
#
# The script captures the handle instead of asking for copy and paste. Earlier versions used
# a literal placeholder and failed with
# "invalid channel handle: ch_...".
#
# Usage:  scripts/demo.sh [amount-wei]
#
# The amount must exactly match one or more unspent private notes owned by agent A. With the
# runbook's fresh setup, both agents shield 1 STRK and the default below settles exactly.
#
# Requires: .env populated for agent A, ~/.erebus-b/env for agent B, both identities
# already registered (see docs/runbook.md §1-2), and erebus-cli built.

set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CLI="${CLI:-$REPO/sdk/rs/target/debug/erebus-cli}"
REQ="${REQ:-$HOME/.erebus/req.py}"
ENV_A="${ENV_A:-$REPO/.env}"
ENV_B="${ENV_B:-$HOME/.erebus-b/env}"
AMOUNT="${1:-1000000000000000000}"

for f in "$CLI" "$REQ" "$ENV_A" "$ENV_B"; do
    [ -e "$f" ] || { echo "missing: $f" >&2; exit 1; }
done

STRK=$(grep '^TOKEN_ADDRESS=' "$ENV_A" | cut -d= -f2)
A=$(grep '^AGENT_ADDRESS=' "$ENV_A" | cut -d= -f2)
B=$(grep '^AGENT_ADDRESS=' "$ENV_B" | cut -d= -f2)

# Stop on an {"ok":false} envelope before the next step receives an invalid value.
#
# Save output to a file because `exit 1` inside `$(...)` only stops the subshell. In
# `X=$(call ... | python3 ...)`, Python supplies the pipeline status. The first version hid
# SUBMIT_FAILED behind JSONDecodeError.
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
# The channel key has no index (hashes.cairo:119), and its marker is WriteOnce. A pair has
# one channel. A second open returns the existing handle without spending a proof. See F29.
HANDLE=$(call "$ENV_A" open_channel "{\"counterparty\":\"$B\"}" \
    | python3 -c 'import sys,json;print(json.load(sys.stdin)["result"]["channel_handle"])')
echo "    handle: $HANDLE"

# One channel supports one deal, so a settled pair cannot trade again with this client.
# Report that condition before propose_offer returns ALREADY_SETTLED.
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
echo "==> propose_offer  (5 encrypted zero-amount notes, one action set, one proof)"
DEADLINE=$(python3 -c 'import time;print(int(time.time())+86400)')
TERMS=$(printf '{"handle":"%s","terms":{"amount":"%s","token":"%s","deadline":%s,"memo_hash":"0x1234"}}' \
    "$HANDLE" "$AMOUNT" "$STRK" "$DEADLINE")
OFFER=$(call "$ENV_A" propose_offer "$TERMS" \
    | python3 -c 'import sys,json;print(json.load(sys.stdin)["result"]["offer_id"])')
echo "    offer:  $OFFER"

echo
echo "==> read_channel_state  (authenticated and decrypted from five public ciphertext salts)"
STATE=$(call "$ENV_A" read_channel_state "{\"handle\":\"$HANDLE\"}")
python3 -m json.tool <<<"$STATE"

# A wrong derivation writes to a slot that the reader never checks. The read returns an empty
# result without an error. Reject an empty transcript after a successful write.
python3 - "$AMOUNT" "$DEADLINE" "$STATE" <<'PY'
import json, sys
amount, deadline = int(sys.argv[1]), int(sys.argv[2])
offers = json.loads(sys.argv[3])["result"]["offers"]
if not offers:
    sys.exit("FAIL: transcript empty after a successful write — the note ids are misderived")
t = offers[-1]["terms"]
assert t["amount"] == amount, f'amount {t["amount"]} != {amount}'
assert t["deadline"] == deadline, f'deadline {t["deadline"]} != {deadline}'
assert t["memo_hash"] == 0x1234, f'memo_hash {t["memo_hash"]:#x} != 0x1234'
print("\nOK: every field round-tripped through the salt lane intact.")
PY

echo
echo "==> open_channel B -> A"
HANDLE_B=$(call "$ENV_B" open_channel "{\"counterparty\":\"$A\"}" \
    | python3 -c 'import sys,json;print(json.load(sys.stdin)["result"]["channel_handle"])')
echo "    B handle: $HANDLE_B"

echo
echo "==> read A's offer from B's direction"
STATE_B=$(call "$ENV_B" read_channel_state "{\"handle\":\"$HANDLE_B\"}")
REPLY_FOR_B=$(python3 - "$STATE_B" "$A" "$AMOUNT" <<'PY'
import json, sys
state, author, amount = json.loads(sys.argv[1])["result"], int(sys.argv[2], 16), int(sys.argv[3])
matches = [o for o in state["offers"]
           if int(o["proposer"], 16) == author and int(o["terms"]["amount"]) == amount]
if len(matches) != 1:
    raise SystemExit(f"FAIL: expected one matching A offer in B's view, found {len(matches)}")
print(matches[0]["offer_id"])
PY
)

echo
echo "==> counter_offer B -> A  (five encrypted notes, one proof)"
COUNTER_TERMS=$(printf '{"handle":"%s","reply_to":"%s","terms":{"amount":"%s","token":"%s","deadline":%s,"memo_hash":"0x5678"}}' \
    "$HANDLE_B" "$REPLY_FOR_B" "$AMOUNT" "$STRK" "$DEADLINE")
COUNTER=$(call "$ENV_B" counter_offer "$COUNTER_TERMS" \
    | python3 -c 'import sys,json;print(json.load(sys.stdin)["result"]["offer_id"])')
echo "    B-local counter: $COUNTER"

echo
echo "==> read B's counter from A's direction"
STATE_A=$(call "$ENV_A" read_channel_state "{\"handle\":\"$HANDLE\"}")
TARGET_FOR_A=$(python3 - "$STATE_A" "$B" "$AMOUNT" <<'PY'
import json, sys
state, author, amount = json.loads(sys.argv[1])["result"], int(sys.argv[2], 16), int(sys.argv[3])
matches = [o for o in state["offers"]
           if int(o["proposer"], 16) == author
           and o["reply_to"] is not None
           and int(o["terms"]["amount"]) == amount]
if len(matches) != 1:
    raise SystemExit(f"FAIL: expected one matching B counter in A's view, found {len(matches)}")
print(matches[0]["offer_id"])
PY
)

echo
echo "==> accept_and_settle A -> B  (five-note acceptance + payment, one proof)"
SETTLEMENT=$(call "$ENV_A" accept_and_settle \
    "{\"handle\":\"$HANDLE\",\"offer_id\":\"$TARGET_FOR_A\"}")
python3 -m json.tool <<<"$SETTLEMENT"

echo
echo "==> grant_viewing_key and reveal"
GRANT_FILE=$(mktemp)
REVEAL_OUT=$(mktemp)
trap 'rm -f "$GRANT_FILE" "$REVEAL_OUT"' EXIT
chmod 600 "$GRANT_FILE" "$REVEAL_OUT"
call "$ENV_A" grant_viewing_key "{\"handle\":\"$HANDLE\",\"grantee\":\"$B\"}" \
    | python3 -c 'import sys,json;json.dump(json.load(sys.stdin)["result"],sys.stdout)' \
    > "$GRANT_FILE"

# The grant is an intentional bearer secret. Feed it from a mode-0600 file, never argv or
# an environment variable. A's config supplies the matching chain/pool RPC for this local
# demonstration; an independent auditor can use its own config for the same chain/pool.
python3 - "$ENV_A" "$GRANT_FILE" <<'PY' | "$CLI" > "$REVEAL_OUT"
import json, sys
env = {}
for line in open(sys.argv[1]):
    line = line.strip()
    if line and not line.startswith('#') and '=' in line:
        key, value = line.split('=', 1); env[key] = value
config = {
    "rpc_url": env["STARKNET_RPC_URL"], "prover_url": env["PROVING_SERVICE_URL"],
    "pool_address": env["POOL_ADDRESS"], "chain_id": env["STARKNET_CHAIN_ID"],
    "account_address": env["AGENT_ADDRESS"], "pool_key_file": env["POOL_KEY_FILE"],
    "account_key_file": env["ACCOUNT_KEY_FILE"], "state_dir": env["EREBUS_STATE_DIR"],
    "token": env["TOKEN_ADDRESS"],
}
print(json.dumps({"method": "reveal", "params": {
    "config": config, "viewing_key": json.load(open(sys.argv[2]))
}}))
PY

if ! python3 -c 'import sys,json;sys.exit(0 if json.load(sys.stdin).get("ok") else 1)' \
    < "$REVEAL_OUT"; then
    echo "reveal failed:" >&2
    python3 -m json.tool < "$REVEAL_OUT" >&2
    exit 1
fi
python3 -m json.tool < "$REVEAL_OUT"
python3 - "$REVEAL_OUT" "$AMOUNT" <<'PY'
import json, sys
record, amount = json.load(open(sys.argv[1]))["result"], int(sys.argv[2])
settlement = record.get("settlement")
if settlement is None:
    raise SystemExit("FAIL: disclosed record has no settlement")
assert int(settlement["agreed_amount"]) == amount
assert int(settlement["paid_amount"]) == amount
assert len(record["offers"]) == 3, f'expected offer/counter/accept, got {len(record["offers"])}'
print("\nOK: five-note offer/counter/accept, atomic payment, and scoped reveal all agree.")
PY
