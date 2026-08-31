# Running the transaction prover locally

A local prover plus a proof-fact-compatible RPC is enough to register on mainnet. It is not
enough to fund notes without the screening path: see [Limits](#limits). For the approved
hosted mainnet path, see [Starkscan hosted proving](#starkscan-hosted-proving).

Verified on 2026-08-28 against the mainnet pool `0x040337b1…812a`.

## Starkscan hosted proving

Starkscan can replace both the local transaction prover and proof interceptor for mainnet.
It requires an operator-issued API key with `prove` scope:

```text
PROVING_SERVICE_URL=https://api.starkscan.co/v1/SN_MAIN/prove
STARKSCAN_API_KEY=<secret>
```

The relay accepts an explicit block and Invoke transaction, returns a job ID, and is polled
until terminal. A successful deposit proof includes `additional_data.signature`. Erebus
stores the job ID and complete one-time result under the identity's mode-`0700`
`EREBUS_STATE_DIR/prover-jobs`; the files are mode `0600`. Erebus derives the Starkscan
idempotency key from the durable operation ID and exact proof-request hash. An identical
rebuild coalesces to the original job, while a new pinned block creates a new logical proof.

The hosted prover receives the proof invocation, including the pool private key. The
Starknet RPC still receives that key during `compile_actions` and is still required for
preflight, fee estimation, submission, and receipt reads. Hosted proving removes the local
prover's compute and storage requirements; it does not remove either trust boundary.

The API key must stay in a protected environment or secret manager. It must not enter an
identity request JSON, command argument, log, screenshot, or repository file. The hosted
relay is mainnet-only and does not expose `starknet_specVersion`; Erebus `doctor` checks the
authenticated capability document and reports `starkscan-async/prove` instead.

Official relay contract: https://starkscan.co/docs/api/strk20-prover

---

## What a local prover buys you

**The Cairo panic reason.** F20 records that the prover answers a failed execution with a
bare `-32603 Internal error` carrying nothing else. That is still true at the JSON-RPC
boundary, but the container log holds the full revert:

```
prove_transaction failed: RunnerError(VirtualBlockExecutor(TransactionReverted(…,
  "… 0x4e4f5f5245504c41595f50524f54454354494f4e ('NO_REPLAY_PROTECTION')")))
```

Running the prover yourself is currently the only way to see that string. During
development it turns a bisection into a one-line answer.

---

## Use Alchemy `v0_10` for both endpoints

Two endpoints are involved and they have different requirements, so the safe default is one
provider that satisfies both:

```
RPC_URL=https://starknet-mainnet.g.alchemy.com/starknet/version/rpc/v0_10/YOUR_KEY
CHAIN_ID=SN_MAIN
```

- **The prover's state RPC** reads state to run a virtual block. It never submits, so
  `proof_facts` support is not strictly required here.
- **Erebus's own RPC** calls `estimateFee` and `addInvokeTransaction`, which must carry
  `proof_facts`. Not every provider forwards the field.

Because the second requirement is strict and the failure is silent, point both at the same
`v0_10` endpoint rather than tracking which provider is acceptable where.

Measured on 2026-08-28 with identical requests:

| Endpoint | `proof_facts` |
| --- | --- |
| `api.cartridge.gg/x/starknet/mainnet` (spec 0.10.2) | **Silently dropped.** Identical errors with and without the field — do not use |
| `starknet-mainnet.g.alchemy.com/…/rpc/v0_10/…` (spec 0.10.3-rc.0) | **Forwarded.** Parses the field and validates its contents |

Alchemy versions by URL segment, and the spread is wide: `v0_7`→0.7.1, `v0_8`→0.8.1,
`v0_9`→0.9.0, `v0_10`→0.10.3-rc.0. Use `v0_10`; the shared `demo` key is rate-limited, so
get your own. A valid `proof_facts[0]` is the ASCII felt `PROOF0` (`0x50524f4f4630`) or `PROOF1`.

---

## Setup

The upstream image is `ghcr.io/starkware-libs/starknet-privacy/transaction-prover`, published
`linux/amd64` only; the runs below used a locally built `arm64` variant of tag
`PRIVACY-0.14.3-RC.2`. On x86_64 use the pinned digest in `ops/juno/compose.yaml`.

### 1. Get an RPC endpoint that can read Starknet mainnet

Alchemy versions by URL segment, and the spread is wide: `v0_7`→0.7.1, `v0_8`→0.8.1,
`v0_9`→0.9.0, `v0_10`→0.10.3-rc.0. **Use `v0_10`.**

Your Alchemy *app* must have Starknet Mainnet enabled. A perfectly valid key on an
Ethereum-only app returns HTTP 403:

```
STARKNET_MAINNET is not enabled for this app.
Visit this page to enable the network: https://dashboard.alchemy.com/apps/<app-id>/networks
```

Re-entering the key, rewriting the env file, and recreating the container cannot fix that;
it is a dashboard toggle. If the app's network list does not offer Starknet, create a new app
and select Starknet Mainnet at creation. You will know it worked because the app id in the
error changes, or the error stops.

Verify the endpoint **with a state read**, not with `starknet_specVersion` — see
[Verify](#3-verify) for why.

### 2. Write the env file and start the container

Keep the key out of shell history and out of the repository. `~/.erebus/prover.env` is
outside the working tree, so it cannot be committed by accident.

```sh
mkdir -p ~/.erebus
printf 'Alchemy key: '; read -rs ALCHEMY_KEY; echo
printf '%s\n' \
  'RUST_LOG=info' \
  "RPC_URL=https://starknet-mainnet.g.alchemy.com/starknet/version/rpc/v0_10/$ALCHEMY_KEY" \
  'CHAIN_ID=SN_MAIN' \
  'MAX_CONCURRENT_REQUESTS=1' \
  'RAYON_NUM_THREADS=2' \
  > ~/.erebus/prover.env
chmod 600 ~/.erebus/prover.env
unset ALCHEMY_KEY
```

**`read -rs -p "prompt"` is a bash-ism.** In zsh it fails with `read: -p: no coprocess`,
leaving `ALCHEMY_KEY` unset and writing a URL that ends in a bare `/v0_10/`. The `printf`
prompt above works in both shells. Prefer the `printf '%s\n'` form over a heredoc: a URL
pasted from a chat client can arrive as a Markdown link, `[url](url)`, and a heredoc will
happily write that.

Check the key actually landed. A byte count will not tell you — a complete file with an empty
key is still 152 bytes:

```sh
grep -c 'v0_10/.\+' ~/.erebus/prover.env    # must print 1
```

Then start it. Container environment is immutable, so changing `RPC_URL` later means
recreating, never editing:

```sh
docker rm -f strk20-prover 2>/dev/null
docker run -d --name strk20-prover \
  -p 127.0.0.1:3000:3000 \
  --env-file ~/.erebus/prover.env \
  strk20-prover:privacy-0.14.3-rc.2-arm64
```

Keep `-p 127.0.0.1:` — binding `0.0.0.0` would publish an endpoint that receives the pool
private key in plaintext calldata (`prover.rs` module docs, friction.md F14).

If `docker run` reports `Conflict. The container name "/strk20-prover" is already in use`, a
stopped container still holds the name; `docker rm -f strk20-prover` clears it.

### 3. Verify

Allow a few seconds for precomputation, then check the prover answers:

```sh
curl -sS -X POST http://127.0.0.1:3000/ -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"starknet_specVersion","params":[]}'
# {"jsonrpc":"2.0","id":1,"result":"0.10.3-rc.2"}
```

**That check alone is not sufficient.** `starknet_specVersion` is answered by the prover
itself and can return `200` while its RPC is completely dead, so it passes with an
unauthenticated or network-disabled endpoint. Always follow it with a state read:

```sh
curl -sS -X POST "$(docker exec strk20-prover printenv RPC_URL)" \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"starknet_getClassHashAt",
       "params":["latest","0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a"]}'
# {"jsonrpc":"2.0","id":1,"result":"0x67dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d"}
```

That is the mainnet pool's class hash. It cannot come back from a broken endpoint.

Confirm the container took the URL you meant:

```sh
docker logs strk20-prover | head -3   # "rpc_node_url: <unset> -> ..." names the host
```

`starknet_chainId` is not implemented; the prover exposes only `starknet_specVersion` and
`starknet_proveTransaction`. Empty params to the latter return `-32602`, which distinguishes
"the service rejected the request shape" from "the request executed and failed".

### Troubleshooting

| Symptom | Cause |
| --- | --- |
| `read: -p: no coprocess` | zsh. Use the `printf` prompt form above |
| `RPC_URL` ends in `/v0_10/` | The `read` failed, so the key was empty |
| RPC returns `401 Must be authenticated!` | No key in the URL |
| RPC returns `403 … is not enabled for this app` | The Alchemy app lacks Starknet Mainnet |
| `specVersion` passes but proofs fail | Classic false green; run the state read |
| `Conflict … name is already in use` | A stopped container holds the name; `docker rm -f` |
| Container exits `137` mid-proof | OOM. See [Memory](#memory-proof-generation-was-oom-killed-at-19-gib) |
| Logs look frozen mid-proof | You are reading a dead container. Check `docker ps -a` |

A shared `demo` key exists in place of a real one and does read mainnet state, which is
useful for confirming the plumbing. It is rate-limited and shared, so do not prove against
it: being throttled partway through wastes the entire run.

---

## Memory: proof size can exhaust a 20 GiB VM

A registration proof reached the STWO stage and died:

```
Exited (137)   OOMKilled=true   container memory limit: 0 (unlimited)
VM: 19 GiB total, 16 used, 3 available, swap 0
last log: prove_cairo: Witness trace cells: [10161776, 165038688, 146595200]
```

Reaching `Generate the cairo proof` and then dying is the signature. On the recorded ARM64
machine, reducing `RAYON_NUM_THREADS` from 6 to 2 completed the registration proof in the
same 20 GiB VM. Treat this only as a local recovery setting; proving resource use is
machine-dependent. `ops/juno/README.md` specifies a 32 GiB host for the remote stack.

A larger deposit proof still OOM-killed the two-thread process. One thread survived, but
exceeded the SDK's former 180-second HTTP timeout while continuing to prove. The source
default is now 600 seconds so a healthy low-memory prover is not abandoned mid-request.

On 2026-08-30, the reverse mainnet channel proof also exhausted the 20 GiB VM because the
local env misspelled `RAYON_NUM_THREADS` and Rayon used the available CPUs. Reconciliation
proved that no transaction had been signed. Adding 8 GiB of VM swap, setting
`RAYON_NUM_THREADS=4`, and resuming the same durable operation ID completed the proof. The
swap activation is not persistent across a Colima VM restart; check `swapon --show` before a
deadline run.

**Check you are reading the live container.** A killed prover is often replaced by a restart
policy, and `docker logs` against the dead id shows a stale tail that looks like a hang:

```sh
docker ps -a --format '{{.ID}}  {{.Status}}'
```

---

## Limits

A local prover and Alchemy v0.10 completed the standalone mainnet registration recorded in
`docs/runs/2026-08-28-mainnet-registration.md`. They do not make every mainnet flow reachable.

- **Deposit screening is protocol-enforced.** `apply_actions` requires an attestation signed
  by the screener key stored in the pool contract (`privacy.cairo:791-793`). A live read on
  2026-08-28 returned the non-zero mainnet screener key `0x501cc4…fdb2`. Self-hosting the
  prover does not produce its signature, so notes cannot be funded.
- **The published proof interceptor is a relay, not the screening authority.** Upstream's
  service needs `SCREENING_URL`, `SCREENING_PARTNER_NAME`, and
  `SCREENING_PARTNER_SECRET`. Its `/screen` upstream returns the STARK signature. Running
  the interceptor without those values is a no-op and still cannot satisfy the pool.
- **`compile_actions` sends the pool private key to Erebus's RPC.** Self-hosting the prover
  moves one endpoint inside the trust boundary, not both. Roadmap Q1.
- **Registration is the exception.** `SetViewingKey` moves no tokens, so it takes the branch
  requiring screening to be absent (`privacy.cairo:795`). It needs a *deployed* account: the
  pool calls `supports_interface` on the address, and an undeployed felt reverts with
  `is not deployed`.

See [runbook.md](./runbook.md) for the Sepolia path and [ops/juno/README.md](../ops/juno/README.md)
for the operator-hosted stack.

The local screened-prover stack is prepared in
[`ops/screened-prover`](../ops/screened-prover/README.md). It pins the matching RC.2
interceptor, connects it through `BLOCKING_CHECK_URL`, and sets both layers to fail closed.
Run `scripts/write-screening-env.sh` only after the screening operator issues the proxy URL,
partner name, and partner secret.

## Write protected mainnet identity configurations

Keep one env and one state directory per account. The helper reads `RPC_URL` from the
protected prover env and writes a mode-`0600` identity env without printing the credential:

```sh
scripts/write-mainnet-identity-env.sh \
  "$ACCOUNT_ADDRESS" "$ACCOUNT_KEY_FILE" "$POOL_KEY_FILE" \
  "$STATE_DIR" "$IDENTITY_ENV"

scripts/agent.sh "$IDENTITY_ENV" doctor
```

The helper refuses relative key/state/env paths and refuses to overwrite an existing env.
The resulting env contains the RPC credential, so do not copy it into the repository.
