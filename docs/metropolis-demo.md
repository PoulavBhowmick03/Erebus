# Metropolis Demo Script (≤ 3 minutes)

Recording is **pending**: this file is the recording-ready script with real captured output.
A human operator records it; nothing here is fabricated, and no key material, grant, bearer
token, or private transcript appears on screen. Every output below was captured from a
published Metropolis environment (dev4) on 2026-10-05/06 and is labelled **CURRENT** (a command run
during recording), **HISTORICAL** (a finalized transaction read back read-only), or **CACHED**
(a local record from an earlier run).

Target length: 2:30–3:00. Terminal font large enough to read; no browser needed.

## Preconditions

- A fresh install from the public index (the dev5 environment used here is
  `/tmp/erebus-m8-acceptance.bwHulH/dev5-install/bin`) or any install from the
  [install guide](metropolis-install.md).
- `cast` (Foundry) for read-only chain reads.
- The live public-bound run record at
  `/tmp/erebus-m8-acceptance.bwHulH/public-bound-replacement-2/run-record.json` (or a fresh run).
- Never show: `*.key` files, `payment.json`/`access.json` key paths, grants, transcripts, or
  bearer tokens.

## Scene 1 — Installed product, offline (≈ 20 s) — CURRENT

```sh
export B=/tmp/erebus-m8-acceptance.bwHulH/dev5-install/bin
printf '%s' '{"method":"version"}' | "$B/erebus-negotiate"
printf '%s' '{"method":"version"}' | "$B/erebus-payment"
```

Narration: these are installed console commands from the public index, not a source checkout.
`erebus-payment` reports `"automatic_rebroadcast": false` — a durable broadcast attempt disables
resubmission forever.

## Scene 2 — Live Monad read-only check (≈ 30 s) — CURRENT

```sh
printf '%s' '{"chain_id":10143,"rpc_url":"https://testnet-rpc.monad.xyz","timeout_ms":15000}' \
  | "$B/erebus-network-check" | jq '{passed, chain_id, finalized: .finalized.number, head: .head.number}'
```

Captured output:

```json
{"passed": true, "chain_id": 10143, "finalized": 68502539, "head": 68502542}
```

Narration: 16 read-only checks passed against the live public RPC — chain id, explicit
`finalized`, hash-pinned reads, rejection of an unknown block hash, and the BN254 precompiles a
Groth16 verifier needs. No transaction is sent.

## Scene 3 — The finalized public-bound payment, read back read-only (≈ 60 s) — HISTORICAL

```sh
RPC=https://testnet-rpc.monad.xyz
TX=0x1ba9ddb363a0c916f8e41ca8eca7d4c524d1ba4722bf4ea37ed423547700ff73
cast receipt "$TX" --rpc-url "$RPC" | grep -E "blockNumber|status|from"
cast call 0xa5f0c864f434331bef9a7fc5e05450d598d24da4 \
  "consumedDeals(bytes32)" 0xa6d31c388b4c13f8fdedf3ec81ac91e1a300a05bc576c56f134a4a8aadfaeeaa --rpc-url "$RPC"
cast call 0x902f79145059910ef875aecf4187c771b204ea14 \
  "balanceOf(address)" 0x9f8326551aa3cf057e486fe180c299c858b6a1c7 --rpc-url "$RPC"
```

Captured output:

```
blockNumber          68484769
from                 0x4a5D6D11765373Ba4e40734f8Ad552E465f85696
status               1 (success)
consumedDeals: 0x…01
seller balance: 0x…46   (70 base units)
```

Narration: this is a **historical** transaction observed read-only, not a new payment. The
receipt contains the ERC-20 `Transfer` of 70 base units to the seller and the `DealSettled` event
carrying the deal commitment `0x477f9ad5…8f33` and nullifier `0xa6d31c38…eeaa`. The nullifier is
consumed once. On this public-bound rail the amount and parties are public; only the offchain
negotiation history is private.

## Scene 4 — The run record from the live workflow (≈ 40 s) — CACHED

```sh
jq '{settle, access, auditor, stages_ms}' \
  /tmp/erebus-m8-acceptance.bwHulH/public-bound-replacement-2/run-record.json
```

Captured output (abridged):

```json
{
  "settle": {"submitted_this_call": true, "transaction_hash": "0x1ba9ddb3…ff73"},
  "access": {"resource_verified": true, "seller_reported_payment_finalized": true,
             "payment_verified": false},
  "auditor": {"agreement_verified": true, "payment_verified": true, "delivery_verified": false}
}
```

Narration: this is a **cached local record** from the team-operated run, not a live command.
It shows one submission, seller access recovery after a restart, and an independent auditor
verifying the payment from the encrypted grant and public chain data alone. `payment_verified:
false` in the access block is deliberate: the seller's claim is not an independent audit; the
auditor block is the independent one.

## Scene 5 — Limits (≈ 20 s) — narration

- Shielded settlement is a local prototype with test-only artifacts and has **not** run on Monad.
- The demo shows public-bound settlement; accepted terms, blinding, signatures, amount, parties,
  and timing are public on chain.
- This is team-operated evidence; independent external acceptance and hosted services remain
  open gates. The reproducible handoff is [External Acceptance](metropolis-external-acceptance.md).

## Recording notes

- Record the terminal only; 1920×1080 or 1280×720; keep the whole session under three minutes.
- Run Scenes 1–3 live during recording (they are read-only); paste Scenes 4–5 as captured or
  re-run the `jq` locally.
- Do not speed up or splice command output. If a read is slow, cut the wait, not the output.
- After recording, place the file where the team hosts demo media and link it from the README
  and submission materials. The link is intentionally absent until the recording exists.
