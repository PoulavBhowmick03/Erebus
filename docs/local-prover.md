# Running the transaction prover locally

A local prover is enough to **generate proofs and read error reasons**. It is not enough to
transact on mainnet: see [Limits](#limits).

Verified on 2026-08-28 against the mainnet pool `0x040337b1…812a`.

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

## Configuration

The upstream image is `ghcr.io/starkware-libs/starknet-privacy/transaction-prover`, published
`linux/amd64` only; the run below used a locally built `arm64` variant of tag
`PRIVACY-0.14.3-RC.2`. Record your build steps here when you rebuild it.

```
RUST_LOG=info
RPC_URL=https://starknet-mainnet.g.alchemy.com/starknet/version/rpc/v0_10/YOUR_KEY
CHAIN_ID=SN_MAIN
MAX_CONCURRENT_REQUESTS=1
RAYON_NUM_THREADS=6
```

Bind the container to loopback only. The invocation it receives carries the pool private key
in plaintext calldata (`prover.rs` module docs, friction.md F14).

### Health check

```sh
curl -sS -X POST http://127.0.0.1:3000/ \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"starknet_specVersion","params":[]}'
# {"jsonrpc":"2.0","id":1,"result":"0.10.3-rc.2"}
```

`starknet_chainId` is not implemented; the prover exposes only `starknet_specVersion` and
`starknet_proveTransaction`. Empty params to the latter return `-32602`, which is how you
distinguish "the service rejected the request shape" from "the request executed and failed".

---

## Memory: proof generation was OOM-killed at 19 GiB

A registration proof reached the STWO stage and died:

```
Exited (137)   OOMKilled=true   container memory limit: 0 (unlimited)
VM: 19 GiB total, 16 used, 3 available, swap 0
last log: prove_cairo: Witness trace cells: [10161776, 165038688, 146595200]
```

Reaching `Generate the cairo proof` and then dying is the signature. Budget more than 20 GiB,
or add swap. `ops/juno/README.md` specifies a 32 GiB host for the remote stack.

**Check you are reading the live container.** A killed prover is often replaced by a restart
policy, and `docker logs` against the dead id shows a stale tail that looks like a hang:

```sh
docker ps -a --format '{{.ID}}  {{.Status}}'
```

---

## Limits

A local prover does not make mainnet reachable.

- **Deposit screening is protocol-enforced.** `apply_actions` requires an attestation signed
  by the screener key stored in the pool contract (`privacy.cairo:791-793`). Self-hosting
  does not produce one, so notes cannot be funded. Roadmap Q2, unanswered.
- **`compile_actions` sends the pool private key to Erebus's RPC.** Self-hosting the prover
  moves one endpoint inside the trust boundary, not both. Roadmap Q1.
- **Registration is the exception.** `SetViewingKey` moves no tokens, so it takes the branch
  requiring screening to be absent (`privacy.cairo:795`). It needs a *deployed* account: the
  pool calls `supports_interface` on the address, and an undeployed felt reverts with
  `is not deployed`.

See [runbook.md](./runbook.md) for the Sepolia path and [ops/juno/README.md](../ops/juno/README.md)
for the operator-hosted stack.
