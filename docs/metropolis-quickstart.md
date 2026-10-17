# Metropolis Testnet Quickstart

> **Status: on `main`, not in a published release.** The published package is still
> `0.3.0.dev2026101307`, which has none of the commands on this page: no `quickstart`, no
> fresh-wallet checkpoint, no `fund_wallet`, and the older error codes. Until the next release,
> run it from a build of this repository (below). Everything here runs on **Monad testnet**
> (chain 10143); it is test-only, single-operator and unaudited, like the deployment it uses.

One command sets up a buyer, a seller and an auditor on your machine, already connected to each
other, and writes MCP configs and prompts for two agents. You fund one address; the agents do the
rest: fund a private note, negotiate, settle, verify the payment and retrieve what they paid for.

## What you need

- macOS arm64 or Linux x86_64, `uv`, and about 2.15 testnet MON for the buyer. The amount is printed
  for you: Monad charges the full gas limit, measured at about 0.31 MON per transaction, and
  each send needs 0.9 MON free up front. The seller needs none for this flow.
- Testnet MON from the faucet, <https://faucet.monad.xyz>. Nothing here automates the faucet.
- Two agents that speak MCP: Claude Code, Codex or Cursor.

## 1. Install

From the next published release (not yet available):

```sh
uv venv --python 3.11 erebus-metropolis
uv pip install --python erebus-metropolis/bin/python --no-config \
  --index https://poulavbhowmick03.github.io/erebus-metropolis/simple/ \
  --default-index https://pypi.org/simple --index-strategy first-index "erebus-mcp-server==<next version>"
export PATH="$PWD/erebus-metropolis/bin:$PATH"
erebus-init --metropolis pins --version <next version> --directory ./pins
```

Until then, from a checkout of `main`: build an unpublished candidate, install it outside the
checkout, and pin it against the published signed pool manifests. These pins are labelled a
rehearsal and are never publication evidence.

```sh
uv run python scripts/build-metropolis-registry.py --version 0.3.0.dev<date>99 --output ./registry \
  --profile release --build --require-deployment contracts/evm/deployments/monad-testnet-shielded-v2.json \
  --proving-manifest <published-pins>/manifest-wmon.json --proving-manifest-sha256 <its sha256> \
  --proving-signature <published-pins>/manifest-wmon.signature.json \
  --proving-manifest <published-pins>/manifest-usdc.json --proving-manifest-sha256 <its sha256> \
  --proving-signature <published-pins>/manifest-usdc.signature.json
# install erebus-mcp-server==0.3.0.dev<date>99 from ./registry into a fresh venv, then:
erebus-init --metropolis pins --directory ./pins --rehearsal-release ./registry/release.json \
  --rehearsal-release-sha256 <sha256 of release.json> --authorized-pins <published-pins>
```

`<published-pins>` is a directory made by `erebus-init --metropolis pins --version 0.3.0.dev2026101307`.

## 2. Up

```sh
erebus-init --metropolis quickstart up --directory ./erebus --pins ./pins --allow-agent-funding
```

It creates two gas keys with the installed `erebus-settle` (owner-only files; Python never reads
them), runs the ordinary `init`, `connect`, `wallet-policy` and `auditor` steps for both roles with
their shares wired together, sets conservative RPC rate limits, and prints:

```json
"funding": {
  "faucet": "https://faucet.monad.xyz",
  "buyer":  {"address": "0x…", "send_at_least_mon": "2.143", "why": "5 transactions at about 0.310 MON each …"},
  "seller": {"address": "0x…", "send_at_least_mon": "0", "why": "the seller sends nothing in this flow …"}
}
```

Send the buyer at least that much. `up` waits for the balance (`--funding-timeout`, default 30
minutes; `--no-wait` to skip), then runs `start`:

1. takes each fresh wallet's paired finalized **history checkpoint**, so the wallets start at the
   current pool state instead of replaying a million blocks;
2. runs `doctor` for both roles;
3. starts the seller's access service in the background. The service keeps its history current
   between requests, so the first delivery does not wait for a catch-up.

Options:

- `--rpc-url`/`--peer-rpc-url` or `--rpc-url-file`/`--peer-rpc-url-file` replace the default public
  pair, `testnet-rpc.monad.xyz` and `rpc.ankr.com/monad_testnet`, verified live on 2026-10-16. The
  two must report the same finalized block: `monad-testnet.drpc.org` ran about two blocks ahead,
  and paired checkpoint acquisition then fails. Use the `-file` forms for keyed URLs: only the
  host is ever printed.
- `--buyer-key-file`/`--seller-key-file` use existing owner-only gas keys instead of creating them.
- `--price`, `--fund-amount`, `--resource`, `--payload-file`, `--port`, and `--pool usdc` (send
  testnet USDC to the buyer as well).
- Without `--allow-agent-funding` the buyer agent can only read its wallet; fund it yourself.

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

The buyer's flow is: `fund_wallet` → `negotiate_deal` → `check_settlement_funding` →
`settle_deal` → `recover_deal` → `retrieve_service_access(operation_ref=…)`. The seller calls
`negotiate_deal(wait_seconds=600)` and waits for the buyer.

What the agents can rely on:

- **Operation IDs are optional.** Omit `operation_ref` and the tool creates one, saves it before
  anything is sent, and returns it. Retrying without one resumes the same unfinished action instead
  of sending another; a different amount while one is unfinished is refused.
- **`fund_wallet(amount)`** wraps (WMON) and approves only if needed, deposits, and waits until the
  note is spendable. If its wait runs out, call it again with the returned `operation_ref`;
  finished steps never repeat.
- **The seller waits.** `negotiate_deal(wait_seconds=N)` keeps listening up to N seconds (0–900).
- **Settlement waits for the wallet.** `settle_deal` syncs first, so a deposit that is not yet
  final no longer causes `local signing failed`.
- **Retrieval by deal.** `retrieve_service_access(operation_ref=…)`; `negotiate_deal` also returns
  `evidence_name`.
- **Errors say what to do.** Every failure has a stable `code`, the native `native_error`, a
  `next_action`, and `retry_same_operation_ref` (see Troubleshooting).

## 4. Status, renew, down

```sh
erebus-init --metropolis quickstart status --directory ./erebus   # balances, checkpoint lag, service, next step
erebus-init --metropolis quickstart renew  --directory ./erebus   # re-sign descriptors before they expire
erebus-init --metropolis quickstart down   --directory ./erebus   # stop the access service; keys and funds kept
erebus-init --metropolis quickstart start  --directory ./erebus   # resume after down
```

Descriptors and identity bindings last 30 days. `status` warns a day before they expire, and
`renew` re-signs both descriptors with their existing keys (the wallets are untouched), re-issues
the bindings and service terms, and restarts the service.

## Troubleshooting

Agents see these in `error.code`. The old blanket value (`METROPOLIS_UNAVAILABLE`,
`WALLET_UNAVAILABLE`) is kept in `error.category`; an unrecognized failure keeps that blanket code
and omits the native text (only the listed messages are forwarded, so private details never reach an agent).

| Code | Native message(s) | What to do | Same `operation_ref`? |
|---|---|---|---|
| `PEER_UNAVAILABLE` | `negotiation connection deadline exceeded`; `negotiation endpoint unavailable`; `peer connection unavailable or timed out`; `offer delivery uncertain; retain this operation and reconnect` | Start the counterparty's `negotiate_deal`, then retry; or pass a larger `wait_seconds`. | yes |
| `LISTENER_UNAVAILABLE` | `negotiation listener unavailable` | Another process holds the negotiation port; stop it. | yes |
| `DESCRIPTOR_EXPIRED` | `local discovery descriptor is invalid or expired`; `peer discovery descriptor is invalid or expired` | `quickstart renew` (keys and funds kept); on separate machines re-run `init` and `connect` and share the new descriptor. | no |
| `OPERATOR_CONFIG_INVALID` | `invalid operator configuration` | The operator must fix the config (for example `timeout_seconds` above 300). | no |
| `INVALID_OPERATION_REF` | `invalid operation reference`; `operation ID must be 64 nonzero lowercase hex digits` | Pass 64 lowercase hex digits, or omit it. | no |
| `SIGNER_CLAIM_HELD` | `local signing failed; retain nonce and reservations` | A wallet transaction is not final yet: `sync_wallet` until nothing is pending, then retry. | yes |
| `RATE_LIMITED` | `RPC provider rate limited; …` | A provider kept answering HTTP 429 after backoff. Wait and retry; the operator can lower `rpc_requests_per_second`. | yes |
| `PROVIDER_UNAVAILABLE` | `paired deployment authentication failed`; `paired pool evidence unavailable; …`; `paired … wallet recovery failed …` | The two providers could not give agreeing evidence. Retry later, or change endpoints with `erebus-init --metropolis endpoints`. | yes |
| `POLICY_REFUSED` | `wallet amount exceeds operator policy`; `wallet action is not authorized by operator policy` | The operator's policy refuses it; use a smaller amount or ask the operator. | no |
| `ALLOWANCE_REQUIRED` | `deposit allowance insufficient; run approve first` | Approve, wait until it completes, then deposit (`fund_wallet` does this). | yes |
| `OPERATION_IN_PROGRESS` | `another deposit is retained; …`, or an omitted reference with a different amount | Finish the returned `operation_ref` first. | yes |
| `WALLET_OPERATION_PENDING` | (settlement waited 5 minutes for the wallet) | `sync_wallet` until nothing is pending, then retry `settle_deal`. | yes |
| `FUNDS_REQUIRED` | (`fund_wallet` on the USDC pool) | Send testnet USDC to the buyer address. | yes |

A `pending` result is not an error: history or settlement is still catching up. Repeat the same
call with the same `operation_ref`, never a new payment.

## Known limits

- The history cache is one file capped at 128 MiB, about 290 bytes per block. With the access
  service syncing continuously, that is about two days of Monad history; past it the cache refuses
  to grow and verification stops. Segmented storage is not built yet, so a quickstart is for runs
  of hours to a day or two, not a long-lived seller.
- The auditor's payment verification still replays from the deployment block.
- Public RPCs throttle. ankr's free endpoint returns HTTP 429 under load; reads back off for
  seconds and retry, quickstart paces each provider at 20 requests per second, and a keyed
  endpoint (`--peer-rpc-url-file`) is the reliable fix.
- One seller serves the one buyer it was connected to. Many-buyer sellers are later work.
