# Metropolis Testnet Quickstart

> **Release `0.4.0.dev2026101801`** (Monad testnet, chain 10143). Test-only, single-operator and unaudited, like
> the deployment it uses. Releases before `0.4.0` have no `quickstart`, fresh-wallet checkpoint or
> `fund_wallet`.

One command sets up a buyer, a seller and an auditor on your machine, already connected to each
other, and writes MCP configs and prompts for two agents. You fund one address; the agents do the
rest: fund a private note, negotiate, settle, verify the payment and retrieve what they paid for.

## What you need

- macOS arm64 or Linux x86_64, `uv`, and about 1.44 testnet MON for the buyer (1.43 MON plus 0.3
  testnet USDC with `--pool usdc`, see below). The amount is printed for you as measured-price
  guidance: Monad charges the full gas limit, so each send is sized by its own limit (wrap and
  approve 65,000, deposit 2,100,000, settle 3,010,000) at the measured 102 gwei, and a settlement
  needs its limit times the 300 gwei fee cap (0.903 MON) free before it is signed.
  The seller needs none for this flow.
- Testnet MON from the faucet, <https://faucet.monad.xyz>. Nothing here automates the faucet.
- Two agents that speak MCP: Claude Code, Codex or Cursor.

## 1. Install

```sh
uv tool install --python 3.11 --no-config \
  --index https://poulavbhowmick03.github.io/erebus-metropolis/simple/ \
  --default-index https://pypi.org/simple --index-strategy first-index \
  "erebus-mcp-server==0.4.0.dev2026101801"
```

`uv tool install` puts `erebus-init` and `erebus-mcp-server` on your PATH (run `uv tool update-shell`
once if it is not) and keeps the native commands inside the tool environment, where quickstart
finds them. The wheels download from GitHub release assets; if uv reports `operation timed out`,
run it again with `UV_HTTP_TIMEOUT=300` (uv waits 30 seconds by default). To build an unreleased candidate from a checkout instead, see the install guide's
"Candidate onboarding before publication".

## 2. Up

```sh
erebus-init --metropolis quickstart up --directory ./erebus --allow-agent-funding
```

It pins this release's signed manifests (or pass `--version V` / `--pins DIR`), creates two gas keys with the installed `erebus-settle` (owner-only files; Python never reads
them), runs the ordinary `init`, `connect`, `wallet-policy` and `auditor` steps for both roles with
their shares wired together, sets conservative RPC rate limits, and prints:

```json
"funding": {
  "faucet": "https://faucet.monad.xyz",
  "buyer":  {"address": "0x…", "send_at_least_mon": "1.440", "why": "measured-price guidance: gas limits wrap 65,000, …"},
  "seller": {"address": "0x…", "send_at_least_mon": "0", "why": "the seller sends nothing in this flow …"}
}
```

Send the buyer at least that much. `up` waits for the balance (`--funding-timeout`, default 30
minutes; `--no-wait` to skip), then runs `start`:

1. downloads and hash-checks the proving artifacts (`quickstart prefetch` does it on its own, and
   continues after a timeout);
2. takes the auditor's paired finalized **history checkpoint** first, then each fresh wallet's, so
   the auditor and both wallets start at the current pool state instead of replaying a million
   blocks, and every deal the two agents make settles after the auditor's checkpoint;
3. runs `doctor` for both roles;
4. starts the seller's access service in the background. The service keeps its history current
   between requests, so the first delivery does not wait for a catch-up.

`quickstart status` shows the auditor's `checkpoint_block` and `history_from_block`.

Options:

- `--rpc-url`/`--peer-rpc-url` or `--rpc-url-file`/`--peer-rpc-url-file` replace the default public
  pair, `testnet-rpc.monad.xyz` and `rpc.ankr.com/monad_testnet`, verified live on 2026-10-16. The
  two must report the same finalized block: `monad-testnet.drpc.org` ran about two blocks ahead,
  and paired checkpoint acquisition then fails. Use the `-file` forms for keyed URLs: only the
  host is ever printed.
- `--buyer-key-file`/`--seller-key-file` use existing owner-only gas keys instead of creating them.
- `--price`, `--fund-amount` (base units of the pool's asset; defaults per pool, below), `--resource`,
  `--payload-file`, `--port`, and `--pool usdc` (see "The USDC pool").
- Without `--allow-agent-funding` the buyer agent can only read its wallet; fund it yourself.

### The USDC pool

```sh
erebus-init --metropolis quickstart up --directory ./erebus-usdc --pool usdc --allow-agent-funding
```

The USDC pool (`0x863f5404…50FB`) settles in the deployed testnet USDC token
`0x534b2f3A21130d7a60830c2Df862319e593943A3`, which has 6 decimals and is issuer-controlled, not
Circle USDC. Amounts are base units of that token:

| | WMON pool (default) | USDC pool |
|---|---|---|
| `--price` default | `1000000000000000` (0.001 WMON) | `100000` (0.1 USDC) |
| `--fund-amount` default | `3000000000000000` (0.003 WMON) | `300000` (0.3 USDC) |
| Buyer transactions | 5: wrap, approve, deposit, two for settlement | 4: approve, deposit, two for settlement |
| Buyer MON | 5.24M gas × 102 gwei + 0.903 + 0.003 wrapped = 1.440 | 5.175M gas × 102 gwei + 0.903 = 1.431 |
| Buyer asset | none: `fund_wallet` wraps MON | at least `--fund-amount` USDC base units, sent to the buyer address |

There is no wrap on this pool, so the buyer must hold the USDC it deposits. This repository does not
document a faucet for this token: send testnet USDC to the buyer address yourself. `up` prints both
amounts:

```json
"buyer": {"address": "0x…", "send_at_least_mon": "1.431", "why": "measured-price guidance: gas limits approve 65,000, …",
          "usdc_token": "0x534b2f3a21130d7a60830c2df862319e593943a3",
          "send_at_least_usdc_base_units": "300000", "send_at_least_usdc": "0.300000", "also": "…"}
```

and waits until the buyer holds both the MON and the USDC (finalized `balanceOf`). `status` shows
`balance_usdc_base_units` for each role. With `--allow-agent-funding` the buyer agent gets approve
and deposit only; if the USDC is short, `fund_wallet` returns `FUNDS_REQUIRED` with the exact
`shortfall` in base units and retries with the same `operation_ref` once it arrives.

## 3. Run two agents

`up` wrote one MCP config per client and a prompt for each role:

```text
erebus/agents/buyer/{claude-code.mcp.json, codex.config.toml, cursor.mcp.json, prompt.md}
erebus/agents/seller/{claude-code.mcp.json, codex.config.toml, cursor.mcp.json, prompt.md}
```

Each agent sees only its own role's server. Start the seller first, and paste each agent its own
`prompt.md`.

- **Claude Code:** `claude --mcp-config erebus/agents/buyer/claude-code.mcp.json`. In interactive
  mode Claude Code may send your first prompt before the MCP server has connected. Wait until
  `/mcp` lists `erebus-buyer` as connected before pasting the prompt, or run non-interactively with
  `claude -p "$(cat erebus/agents/buyer/prompt.md)" --mcp-config erebus/agents/buyer/claude-code.mcp.json`.
- **Codex:** merge `codex.config.toml` into `~/.codex/config.toml` (one role per Codex profile).
- **Cursor:** copy `cursor.mcp.json` to the project's `.cursor/mcp.json`.

The buyer's flow is: `fund_wallet` → `negotiate_deal` → `settle_deal` → `recover_deal` →
`retrieve_service_access(operation_ref=…)`. `check_settlement_funding` stays available but is
optional: `settle_deal` checks the note and gas itself before it proves or signs. The seller loops:
`negotiate_deal(wait_seconds=600)`, report the returned `peer` and `deal_commitment`, and call it
again for the next buyer. `list_peers` shows which buyers the seller is configured for.

What the agents can rely on:

- **Operation IDs are optional.** Omit `operation_ref` and the tool creates one, saves it before
  anything is sent, and returns it. Retrying without one resumes the same unfinished action instead
  of sending another; a different amount while one is unfinished is refused.
- **`fund_wallet(amount)`** wraps (WMON pool only) and approves only if needed, deposits, and waits
  until the note is spendable. Each step starts once the previous one is submitted; a step that
  sees the previous one not yet final syncs once and retries under the same reference. If its wait runs out, call it again with the returned `operation_ref`;
  finished steps never repeat.
- **The seller waits.** `negotiate_deal(wait_seconds=N)` keeps listening up to N seconds (0–900).
- **Settlement checks the wallet itself.** `settle_deal` does not sync first. If a wallet
  transaction still holds the signer or the funded note, it syncs once and retries once, then
  returns a retryable `WALLET_OPERATION_PENDING` or `SIGNER_CLAIM_HELD`.
- **Retrieval by deal.** `retrieve_service_access(operation_ref=…)`; `negotiate_deal` also returns
  `evidence_name`.
- **Errors say what to do.** Every failure has a stable `code`, the native `native_error`, a
  `next_action`, and `retry_same_operation_ref` (see Troubleshooting).

### More buyers

One seller can sell the same resource to several buyers, each at its own price:

```sh
erebus-init --metropolis quickstart add-buyer --directory ./erebus --name b2 --price 900000000000000 \
  [--fund-amount 3000000000000000] [--buyer-key-file KEY] [--allow-agent-funding]
```

This creates a buyer operator under `erebus/buyers/b2/`, a seller peer `erebus/seller/peers/b2/`
with its handoff in `erebus/seller/outbox/b2/`, and agent files in `erebus/agents/b2/`. It prints
the new buyer's gas address and how much to fund. It never changes the seller's access service,
service identity or payload. Fund the address, run `quickstart start` (it checkpoints every funded
buyer), and give `agents/b2/` to the new buyer agent. The seller agent's loop serves buyers one
deal at a time, in the order they connect; a buyer that connects while another deal runs is told
the peer is unavailable and its `negotiate_deal` retries.

`--price` and `--fund-amount` default to the pool's own defaults (see "The USDC pool"). On the USDC
pool, send the new buyer the printed USDC as well as the MON; `start` checkpoints it only once it
holds both, and always takes the auditor's checkpoint first.

## 4. Status, renew, down

```sh
erebus-init --metropolis quickstart status   --directory ./erebus  # balances, checkpoint lag, artifacts, service
erebus-init --metropolis quickstart prefetch --directory ./erebus  # proving artifacts now, resumable
erebus-init --metropolis quickstart renew  --directory ./erebus   # re-sign descriptors before they expire
erebus-init --metropolis quickstart down   --directory ./erebus   # stop the access service; keys and funds kept
erebus-init --metropolis quickstart start  --directory ./erebus   # resume after down
```

Descriptors and identity bindings last 30 days. `status` lists every buyer and warns a day before
descriptors expire. `renew` re-signs every descriptor with its existing keys (the wallets are
untouched), re-issues the bindings, gives each buyer fresh service terms with a new delivery
deadline (the seller's and the buyer's copies match), and restarts the service once.

## Troubleshooting

Agents see these in `error.code`. The old blanket value (`METROPOLIS_UNAVAILABLE`,
`WALLET_UNAVAILABLE`) is kept in `error.category`; an unrecognized failure keeps that blanket code
and omits the native text (only the listed messages are forwarded, so private details never reach an agent).

| Code | Native message(s) | What to do | Same `operation_ref`? |
|---|---|---|---|
| `PEER_UNAVAILABLE` | `negotiation connection deadline exceeded`; `negotiation endpoint unavailable`; `peer connection unavailable or timed out`; `offer delivery uncertain; retain this operation and reconnect` | Start the counterparty's `negotiate_deal`, then retry; or pass a larger `wait_seconds`. | yes |
| `NO_PEER_CONNECTED` | `no configured peer connected before the deadline` | Seller: no configured buyer connected in time. Retry the same call once a buyer runs `negotiate_deal`. | yes |
| `PEER_NOT_CONFIGURED` | `negotiation peer is not configured` | Seller: that `peer` is not in the peer table. Call `list_peers`, or add it with `quickstart add-buyer`. | no |
| `PEER_EXPIRED` | `negotiation peer is expired or disabled` | Seller: that buyer's configuration expired or is disabled. `quickstart renew`, or re-add the buyer. | no |
| `OPERATION_BOUND_TO_OTHER_PEER` | `operation is bound to another negotiation peer` | Seller: the `operation_ref` belongs to another buyer. Omit it, or pass that buyer's `peer`. | no |
| `LISTENER_UNAVAILABLE` | `negotiation listener unavailable` | Another process holds the negotiation port; stop it. | yes |
| `DESCRIPTOR_EXPIRED` | `local discovery descriptor is invalid or expired`; `peer discovery descriptor is invalid or expired` | `quickstart renew` (keys and funds kept); on separate machines re-run `init` and `connect` and share the new descriptor. | no |
| `OPERATOR_CONFIG_INVALID` | `invalid operator configuration` | The operator must fix the config (for example `timeout_seconds` above 300). | no |
| `INVALID_OPERATION_REF` | `invalid operation reference`; `operation ID must be 64 nonzero lowercase hex digits` | Pass 64 lowercase hex digits, or omit it. | no |
| `SIGNER_CLAIM_HELD` | `local signing failed; retain nonce and reservations`; `signer lane is busy; sync the wallet and retry` | A wallet transaction is not final yet: `sync_wallet` until nothing is pending, then retry. | yes |
| `RATE_LIMITED` | `RPC provider rate limited; …` | A provider kept answering HTTP 429 after backoff. Wait and retry; the operator can lower `rpc_requests_per_second`. | yes |
| `PROVIDER_UNAVAILABLE` | `paired deployment authentication failed`; `paired pool evidence unavailable; …`; `paired … wallet recovery failed …` | The two providers could not give agreeing evidence. Retry later, or change endpoints with `erebus-init --metropolis endpoints`. | yes |
| `POLICY_REFUSED` | `wallet amount exceeds operator policy`; `wallet action is not authorized by operator policy` | The operator's policy refuses it; use a smaller amount or ask the operator. | no |
| `ALLOWANCE_REQUIRED` | `deposit allowance insufficient; run approve first` | Approve, wait until it completes, then deposit (`fund_wallet` does this). | yes |
| `BALANCE_REQUIRED` | `deposit asset balance insufficient; wrap or fund the asset first` | The wrap or transfer is not visible yet: `sync_wallet`, then deposit with the same reference (`fund_wallet` does this). | yes |
| `GAS_ESTIMATE_FAILED` | `gas estimate failed; the call would revert or the provider failed; nothing was signed` | The call would revert now (often an earlier step not visible yet) or a provider failed; nothing was signed. `sync_wallet`, then retry with the same reference. | yes |
| `SIGNER_PLAN_CHANGED` | `signer plan changed after the funding check; …` | Another process changed the signer's saved plan between the funding check and signing; nothing was signed. Retry with the same reference. | yes |
| `PLAN_ABOVE_GAS_CAP` | `retained plan exceeds the configured gas cap; …` | A transaction signed before an upgrade exceeds the configured `gas_limit`; it is kept and not sent. The operator decides. | no |
| `GAS_LIMIT_EXCEEDED` | `gas estimate exceeds the method gas limit; nothing was signed` | The call's estimate plus 5% exceeds the operator's per-method gas limit; nothing was signed. The operator reviews the limit. | no |
| `OPERATION_IN_PROGRESS` | `another deposit is retained; …`, or an omitted reference with a different amount | Finish the returned `operation_ref` first. | yes |
| `WALLET_OPERATION_PENDING` | (settlement synced once and retried once while a wallet transaction was pending) | `sync_wallet` until nothing is pending, then retry `settle_deal`. | yes |
| `FUNDS_REQUIRED` | (`fund_wallet` when the signer holds less than `amount`) | On the USDC pool, send the buyer address the error's `shortfall` (base units of `asset`); on WMON, send WMON or allow wrap. | yes |

A `pending` result is not an error: history or settlement is still catching up. Repeat the same
call with the same `operation_ref`, never a new payment.

## Known limits

- The history cache is a manifest plus segment files of 65,536 blocks each, with no size cap. A
  mostly empty Monad block takes about 40 bytes on disk; at 3.3 blocks a second, continuous sync adds
  about 11 MB a day, roughly 4 GB a year. A cache from an older release migrates on its first save;
  older binaries then refuse it. A reorg more than about 1,000 blocks deep into saved history is
  refused rather than rewritten, and a cache that fails verification must be deleted and rescanned.
  Memory grows with pool events, not with empty blocks.
- The quickstart auditor verifies only deals settled after its checkpoint block. A deal settled at
  or before it is reported as `settled_before_checkpoint` (never as paid or unpaid); verify that one
  with a standalone `erebus-init --metropolis auditor`, which replays from the pool deployment by
  default. A checkpoint at an earlier block is not offered yet.
- Public RPCs throttle. ankr's free endpoint returns HTTP 429 under load; reads back off for
  seconds and retry, quickstart paces each provider at 20 requests per second, and a keyed
  endpoint (`--peer-rpc-url-file`) is the reliable fix.
- A seller negotiates with one buyer at a time. Buyers can start together; the others wait and
  retry until the seller's agent calls `negotiate_deal` again. A concurrent negotiation daemon is
  later work.
- Buyers added with `add-buyer` buy the seller's existing resource. Serving several resources from
  one access service needs a hand-written `resources` list in `access.json` (operations guide §4).
- Buyers share the seller's negotiation endpoint and access service. A slow or unfinished
  handshake holds the seller for up to 5 seconds, and one buyer can observe the timing of another's
  requests there. Access answers are the same for an unknown and another buyer's agreement, and
  both spend the same signature checks. A hit still also reads, decodes and replays the evidence
  and hashes the snapshot payload, so a close timer can tell a hit from a miss (operations guide §4).
