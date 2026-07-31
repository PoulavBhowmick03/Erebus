# Runbook — reproducing the on-chain demonstration

What this gets you: two registered identities on Sepolia, each holding a shielded note, a
private channel between them, and one negotiation offer written into note salts and read
back with every field intact.

**What it does not get you yet:** counter-offer, settlement, or viewing-key disclosure.
Those methods exist and pass offline tests; they have never run against a chain. See
`poulav.md` for the current line between proven and implemented.

First run end to end: 2026-07-31. Roughly 20 minutes, most of it waiting on blocks.

---

## 0. Prerequisites

```bash
cd ~/Developer/erebus/sdk/rs && cargo build --bin erebus-cli
export CLI=~/Developer/erebus/sdk/rs/target/debug/erebus-cli
export RPC=https://starknet-sepolia-rpc.publicnode.com
export STRK=0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d
export POOL=0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91
```

`.env` must already hold `PROVING_SERVICE_URL` (StarkWare's endpoint — not in the repo, not
to be shared) and the pool/chain/RPC values. `.env.example` has the shape.

**Two helpers.** The request builder, because assembling nine config fields by hand is how
you get a `INVALID_REQUEST` that tells you nothing:

```bash
mkdir -p ~/.erebus && chmod 700 ~/.erebus
cat > ~/.erebus/req.py <<'PY'
import json, sys
env = {}
for line in open(sys.argv[1]):
    line = line.strip()
    if line and not line.startswith('#') and '=' in line:
        k, v = line.split('=', 1); env[k] = v
cfg = {"rpc_url": env["STARKNET_RPC_URL"], "prover_url": env["PROVING_SERVICE_URL"],
       "pool_address": env["POOL_ADDRESS"], "chain_id": env["STARKNET_CHAIN_ID"],
       "account_address": env["AGENT_ADDRESS"], "pool_key_file": env["POOL_KEY_FILE"],
       "account_key_file": env["ACCOUNT_KEY_FILE"], "state_dir": env["EREBUS_STATE_DIR"],
       "token": env["TOKEN_ADDRESS"]}
params = json.loads(sys.argv[3]) if len(sys.argv) > 3 else {}
params["config"] = cfg
print(json.dumps({"method": sys.argv[2], "params": params}))
PY
```

And a block-depth gate. **This one is not optional.** The client proves against `head - 10`,
so an `approve` newer than that is invisible to the simulation and the shield fails with a
bare `-32603` carrying no reason (F20). Waiting a fixed "five minutes" is guesswork; poll:

```bash
cat > ~/.erebus/wait.sh <<'SH'
#!/bin/bash
# wait.sh <tx_hash> — blocks until the tx is at least 10 blocks deep
req() { curl -s -m 15 -X POST "$RPC" -H 'content-type: application/json' -d "$1"; }
B=$(req "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"starknet_getTransactionReceipt\",\"params\":[\"$1\"]}" \
    | python3 -c 'import sys,json;print(json.load(sys.stdin)["result"]["block_number"])')
while :; do
  H=$(req '{"jsonrpc":"2.0","id":1,"method":"starknet_blockNumber","params":[]}' \
      | python3 -c 'import sys,json;print(json.load(sys.stdin)["result"])')
  D=$((H - B))
  echo "  depth $D/10 (tx block $B, head $H)"
  [ "$D" -ge 10 ] && break
  sleep 20
done
SH
chmod +x ~/.erebus/wait.sh
```

---

## 1. Create an identity

Repeat this whole section once per agent. Agent A uses `~/.erebus` and the repo `.env`;
agent B uses `~/.erebus-b` and `~/.erebus-b/env`. Substitute `NAME` and `DIR` accordingly.

```bash
NAME=erebus-agent      # or erebus-agent-b
DIR=~/.erebus          # or ~/.erebus-b

sncast account create --url $RPC --name $NAME
```

⏸ **Fund the printed address** at https://starknet-faucet.vercel.app. Budget ~10 STRK: a
proof-carrying transaction costs ~3 STRK in gas (F27), and each agent does at least one.

```bash
sncast account deploy --url $RPC --name $NAME     # answer "No" to making it default
```

Answer **No** to the default prompt. It writes a machine-wide setting in `~/.config`, and in
a system where the identity determines which channel your notes live in, an implicit default
is the wrong ergonomic.

```bash
mkdir -p $DIR/state && chmod 700 $DIR $DIR/state
$CLI <<< "{\"method\":\"generate_pool_key\",\"params\":{\"path\":\"$DIR/pool.key\"}}"
```

Then extract the account key to its own file and build the agent's env. Note it reads the
address out of the JSON rather than asking you to paste it — pasting it by hand is how the
first attempt produced `field account_address is invalid: <paste B address from above>`:

```bash
NAME=$NAME DIR=$DIR python3 - <<'PY'
import json, os
name, d = os.environ['NAME'], os.path.expanduser(os.environ['DIR'])
a = json.load(open(os.path.expanduser(
    '~/.starknet_accounts/starknet_open_zeppelin_accounts.json')))['alpha-sepolia'][name]
dst = os.path.join(d, 'account.key')
fd = os.open(dst, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
os.fdopen(fd, 'w').write(a['private_key'] + '\n')
print('address:', a['address'])
PY
```

For agent B, derive its env from A's:

```bash
B_ADDR=$(python3 -c "import json,os;print(json.load(open(os.path.expanduser(
  '~/.starknet_accounts/starknet_open_zeppelin_accounts.json')))['alpha-sepolia']['erebus-agent-b']['address'])")
sed -e "s|^AGENT_ADDRESS=.*|AGENT_ADDRESS=$B_ADDR|" \
    -e "s|/.erebus/|/.erebus-b/|g" ~/Developer/erebus/.env > ~/.erebus-b/env
chmod 600 ~/.erebus-b/env
```

**The two key files are not two accounts.** `account.key` signs Starknet transactions and is
custody; `pool.key` is the STRK20 identity and is confidentiality. Only the account key can
authorise a spend — `__execute__` calls `assert_valid_signature` against your account
contract (`utils.cairo:390`). See F26.

---

## 2. Shield, which also registers

Registration only happens folded into an action set, so each identity has to do something
before it can be a channel counterparty. A 1 STRK shield is the cheapest. Skip it for B and
A's `open_channel` fails with `CounterpartyUnregistered`.

```bash
ENV=~/Developer/erebus/.env        # or ~/.erebus-b/env

TX=$(sncast --account $NAME invoke --url $RPC \
  --contract-address $STRK --function approve \
  --calldata $POOL 0xde0b6b3a7640000 0x0 \
  | grep -o '0x[0-9a-f]*' | tail -1)

~/.erebus/wait.sh $TX
python3 ~/.erebus/req.py $ENV shield '{"amount":"1000000000000000000"}' | $CLI
```

⚠️ **Registration is irreversible and writes your pool private key encrypted to the pool's
auditor on-chain** (`channel.cairo:329-334`, and `channel.rs:123-129`). From that moment the
auditor can decrypt everything that identity ever does. Fine for throwaway testnet keys; it
is the strongest argument for deploying our own pool instance for the product, where we
would hold that auditor key.

Verify registration took, and that the pool agrees with what `generate_pool_key` produced:

```bash
SEL=$(python3 -c "from Crypto.Hash import keccak;h=keccak.new(digest_bits=256);h.update(b'get_public_key');print(hex(int.from_bytes(h.digest(),'big')&((1<<250)-1)))")
ADDR=$(grep '^AGENT_ADDRESS=' $ENV | cut -d= -f2)
curl -s -X POST $RPC -H 'content-type: application/json' \
  -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"starknet_call\",\"params\":[{\"contract_address\":\"$POOL\",\"entry_point_selector\":\"$SEL\",\"calldata\":[\"$ADDR\"]},\"latest\"]}"
```

---

## 3. The demonstration

```bash
B=$(grep '^AGENT_ADDRESS=' ~/.erebus-b/env | cut -d= -f2)

# A opens a channel to B
python3 ~/.erebus/req.py ~/Developer/erebus/.env open_channel "{\"counterparty\":\"$B\"}" | $CLI
# -> {"channel_handle":"ch_..."}

H=ch_...   # paste the handle

# A writes an offer into the salt lane
DEADLINE=$(python3 -c "import time;print(int(time.time())+86400)")
python3 ~/.erebus/req.py ~/Developer/erebus/.env propose_offer \
  "{\"handle\":\"$H\",\"terms\":{\"amount\":\"500000000000000000\",\"token\":\"$STRK\",\"deadline\":$DEADLINE,\"memo_hash\":\"0x1234\"}}" | $CLI

# read it back off-chain
python3 ~/.erebus/req.py ~/Developer/erebus/.env read_channel_state "{\"handle\":\"$H\"}" | $CLI | python3 -m json.tool
```

**What success looks like.** The final read returns one offer whose `amount`, `deadline`,
`memo_hash` and `token` match exactly what was sent. Those values were never in calldata —
they were reassembled from the salts of four zero-amount notes.

**What failure looks like, and why it is worth naming.** A wrong derivation does not raise.
`open_channel` still succeeds, `propose_offer` still succeeds, and `read_channel_state`
returns `"offers": []`. **An empty transcript after a successful write is the failure
signal.** There is no error anywhere, because a misderived note id simply addresses a
storage slot nobody wrote to.

---

## Reference: first successful run

| step | tx | result |
|---|---|---|
| A shield | `0x5f57eb…b9e2` | block 12715064, fee 3.04 STRK |
| B shield | `0x10c3376c…77e0` | registered |
| A `open_channel` → B | — | `ch_a8f81fdc…2d59` |
| A `propose_offer` | — | `ch_a8f81fdc…2d59:us:0` |
| `read_channel_state` | — | offer returned, all fields exact |

Screening never came up. StarkWare's prover mints the attestation via its
`proof-interceptor` sidecar and packs it automatically — see F6, which was open for six days
predicting the opposite.
