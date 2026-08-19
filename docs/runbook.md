# Runbook: reproducing the on-chain demonstration

What this gets you: two registered identities on Sepolia, each holding a shielded note, a
directional channel pair between them, negotiation state written into note salts and read
back with every field intact, an atomic settlement, and an independently reconstructed
disclosure record.

**Evidence boundary. Updated 2026-08-18:** wire v2 completed this live run on 2026-08-07,
settling as `0x14b38e9dbc65f0749be6da2fa05dd2713f8c4c893bac707961c73e616b34cb3`, and an
observer with no key recovered nothing from it. The note here previously said wire v2 was
verified offline only; that stopped being true on 2026-08-07.

Two limits remain. There has been no independent cryptographic review. And content
confidentiality is not relationship confidentiality: the counterparty's address is written
into public calldata at channel-open (F38), so an observer learns who dealt with whom
without decrypting anything. See friction.md F30/F31/F38 and
[privacy-model.md](./privacy-model.md).

First run end to end: 2026-07-31. Roughly 20 minutes, most of it waiting on blocks.

---

## Local wire-v2 verification (no network, no keys)

Run the focused codec and end-to-end Rust paths first:

```shell
cd ~/Developer/erebus/sdk/rs
cargo test --test wire_codec
cargo test --test channel_ops --test read_path --test index_contiguity \
  --test settlement --test disclosure
```

Then run the complete quality gate:

```shell
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --no-deps
```

Expected as of 2026-07-31: 190 passed and two intentionally ignored live-prover probes.
The focused codec suite has 17 tests: the pinned five-salt known answer, round trips,
single-bit tamper rejection, chain/pool/channel/token/index binding, canonical padding,
same-index retry safety, and wire-v1 compatibility.

---

## 0. Prerequisites

```shell
export REPO=~/Developer/erebus
cd "$REPO/sdk/rs" && cargo build --bin erebus-cli
export CLI="$REPO/sdk/rs/target/debug/erebus-cli"
export REQ="$REPO/scripts/erebus-request.py"
export RPC=https://starknet-sepolia-rpc.publicnode.com
export STRK=0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d
export POOL=0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91
```

`.env` must already hold `PROVING_SERVICE_URL` (StarkWare's endpoint, not in the repo, not
to be shared) and the pool/chain/RPC values. `.env.example` has the shape.

The request builder now lives in the repository; no generated Python file or heredoc is
needed. Confirm the helper can read the env file:

```shell
python3 "$REQ" "$REPO/.env" read_channel_state \
  '{"handle":"ch_0000000000000000000000000000000000000000000000000000000000000000"}' \
  >/dev/null
```

The block-depth gate also lives in the repository. **This is not optional.** The client proves against `head - 10`,
so an `approve` newer than that is invisible to the simulation and the shield fails with a
bare `-32603` carrying no reason (F20). Waiting a fixed "five minutes" is guesswork; poll:

```shell
bash "$REPO/scripts/wait-for-depth.sh" 0x_TRANSACTION_HASH
```

---

## 1. Create an identity

> **Sections 1 and 2 are automated.** `scripts/new-identity.sh bootstrap <name> <dir>
> <funder-account>` runs create → fund → deploy → key files → env → fee-aware approve →
> depth wait → shield, and finishes with `doctor`. The faucet variant
> (`create`, fund by hand, `activate`) is in the script header. The manual steps below
> remain the reference for what the script does and for debugging a step that fails.

Repeat this whole section once per agent. Agent A uses `~/.erebus` and the repo `.env`;
agent B uses `~/.erebus-b` and `~/.erebus-b/env`. Substitute `NAME` and `DIR` accordingly.

```bash
NAME=erebus-agent      # or erebus-agent-b
DIR=~/.erebus          # or ~/.erebus-b

sncast account create --url $RPC --name $NAME
```

**Pause: fund the printed address** at https://starknet-faucet.vercel.app. Budget ~10 STRK: a
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

Then extract the account key to its own file. This is a repository helper rather than a
heredoc: a heredoc terminator must start at column one, so indented copy/paste gets stuck at
the `heredoc>` prompt. The helper never prints the key, creates the file mode `0600`, and
refuses to overwrite an existing file:

```bash
python3 "$REPO/scripts/extract-sncast-account-key.py" "$NAME" "$DIR"
```

For every additional identity (B, C, and later), derive a dedicated env from A's without
pasting an address or using another identity's key paths:

```bash
AGENT_ADDR=$(python3 -c 'import json,os,sys; print(json.load(open(os.path.expanduser("~/.starknet_accounts/starknet_open_zeppelin_accounts.json")))["alpha-sepolia"][sys.argv[1]]["address"])' "$NAME")
ENV="$DIR/env"
sed -e "s|^AGENT_ADDRESS=.*|AGENT_ADDRESS=$AGENT_ADDR|" \
    -e "s|^POOL_KEY_FILE=.*|POOL_KEY_FILE=$DIR/pool.key|" \
    -e "s|^ACCOUNT_KEY_FILE=.*|ACCOUNT_KEY_FILE=$DIR/account.key|" \
    -e "s|^EREBUS_STATE_DIR=.*|EREBUS_STATE_DIR=$DIR/state|" \
    "$REPO/.env" > "$ENV"
chmod 600 "$ENV"
echo "agent env: $ENV"
```

**The two key files are not two accounts.** `account.key` signs Starknet transactions and is
custody; `pool.key` is the STRK20 identity and is confidentiality. Only the account key can
authorise a spend. `__execute__` calls `assert_valid_signature` against your account
contract (`utils.cairo:390`). See F26.

---

## 2. Shield, which also registers

Registration only happens folded into an action set, so each identity has to do something
before it can be a channel counterparty. A 1 STRK shield is the cheapest. Skip it for B and
A's `open_channel` fails with `CounterpartyUnregistered`.

```bash
ENV=~/Developer/erebus/.env        # or ~/.erebus-b/env

echo "approving token: $STRK"
echo "for pool:        $POOL"
sncast --account "$NAME" invoke --url "$RPC" \
  --contract-address "$STRK" --function approve \
  --calldata "$POOL" 0xde0b6b3a7640000 0x0

# Set this only after sncast prints "Success: Invoke completed".
TX=0x_PASTE_THE_APPROVE_TRANSACTION_HASH

bash "$REPO/scripts/wait-for-depth.sh" "$TX"
python3 "$REQ" "$ENV" shield '{"amount":"1000000000000000000"}' | "$CLI"
```

`--contract-address` is the ERC-20 token (`$STRK`), never `AGENT_ADDRESS`. Calling
`approve` on an account contract fails with `ENTRYPOINT_NOT_FOUND`; if approve fails, do
not run the wait or shield commands.

**Warning: registration is irreversible and writes your pool private key encrypted to the pool's
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

Use a fresh A/B pair and fresh state directories. A settled pair is terminal because the
pool permits only one directional channel per sender/recipient pair. The script captures
every handle and offer id, keeps the bearer grant in a mode-`0600` temporary file, and runs
offer → counter → atomic settlement → reveal:

```bash
cd ~/Developer/erebus
./scripts/demo.sh 1000000000000000000
```

The amount must exactly match A's unspent private notes; the command above matches the
runbook's 1 STRK shield. It spends that private note and several STRK of Sepolia gas. Do not
rerun it against the already-settled wire-v1 identities from the reference run.

The commands below show the first offer/read portion manually for debugging:

```bash
B=$(grep '^AGENT_ADDRESS=' ~/.erebus-b/env | cut -d= -f2)

# A opens a channel to B
python3 "$REQ" "$REPO/.env" open_channel "{\"counterparty\":\"$B\"}" | "$CLI"
# -> {"channel_handle":"ch_..."}

H=ch_...   # paste the handle

# A writes an offer into the salt lane
DEADLINE=$(python3 -c "import time;print(int(time.time())+86400)")
python3 "$REQ" "$REPO/.env" propose_offer \
  "{\"handle\":\"$H\",\"terms\":{\"amount\":\"500000000000000000\",\"token\":\"$STRK\",\"deadline\":$DEADLINE,\"memo_hash\":\"0x1234\"}}" | "$CLI"

# read it back off-chain
python3 "$REQ" "$REPO/.env" read_channel_state "{\"handle\":\"$H\"}" | "$CLI" | python3 -m json.tool
```

**What success looks like.** The script first returns an offer whose `amount`, `deadline`,
`memo_hash` and `token` match exactly what was sent. Its final reveal contains three records
(offer, counter, acceptance), and `agreed_amount == paid_amount` for the atomic settlement.

**What wire v2 claims.** The five salts remain public inside `packed_value`, but they now
contain authenticated ciphertext rather than plaintext terms. Round-trip success proves the
storage/read path and v2 authentication agree. It does not hide the five-note traffic shape,
prove relationship anonymity, replace independent cryptographic review, or constitute live
v2 evidence until this exact run succeeds with fresh v2 state.

**What failure looks like, and why it is worth naming.** A wrong derivation does not raise.
`open_channel` still succeeds, `propose_offer` still succeeds, and `read_channel_state`
returns `"offers": []`. **An empty transcript after a successful write is the failure
signal.** There is no error anywhere, because a misderived note id simply addresses a
storage slot nobody wrote to.

---

## 4. Autonomous agents through MCP

Register separate role-bound servers. The role is required because
`accept_and_settle` spends the caller's notes: the buyer is the payer and the seller is the
payee.

```bash
claude mcp add erebus-seller -- "$REPO/scripts/erebus-mcp.sh" ~/.erebus-d/env payee
claude mcp add erebus-buyer  -- "$REPO/scripts/erebus-mcp.sh" ~/.erebus-e/env payer
```

Restart the agent clients after adding them. Before negotiating, inspect the buyer's exact
denominations:

```bash
scripts/agent.sh ~/.erebus-e/env balance
```

Inside MCP, call `get_note_balance` with each candidate price. Only a result with
`can_pay_exactly: true` may be proposed, countered, or accepted. The payer server enforces
that rule even if the prompt omits it. A payee server structurally refuses settlement and
must counter at the agreed price, leaving a payee-authored offer for the buyer to accept.

For prompts and the full two-agent sequence, use [agent-brief.md](./agent-brief.md).

---

## Reference: first successful run

| step | tx | result |
|---|---|---|
| A shield | `0x5f57eb...b9e2` | block 12715064, fee 3.04 STRK |
| B shield | `0x10c3376c...77e0` | registered |
| A `open_channel` → B |: | `ch_a8f81fdc...2d59` |
| A `propose_offer` |: | `ch_a8f81fdc...2d59:us:0` |
| `read_channel_state` |: | offer returned, all fields exact |
| B `open_channel` → A |: | `ch_c4d09ef1...5aab` |
| B `counter_offer` |: | 1 STRK counter returned and A read it |
| A `accept_and_settle` | `0x44289c...84bb7` | accepted on L2; nullifier exists |
| independent `reveal` | read-only | full offers, acceptance and exact payment reconstructed |

Screening never came up. StarkWare's prover mints the attestation via its
`proof-interceptor` sidecar and packs it automatically, see F6, which was open for six days
predicting the opposite.
