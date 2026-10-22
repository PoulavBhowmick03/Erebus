# Metropolis Tester Brief

> Release `0.4.0.dev2026101801`, Monad testnet (chain 10143). Test-only, single-operator and
> unaudited. Use testnet MON only. Never put value you care about through it.

## Who this is for

You have never seen this repository. You will install the published release, run two AI agents
on your own machine, and let them make one deal: the buyer funds a private note, both agents
negotiate, the buyer pays, and the buyer retrieves what it paid for.

It should take about 20 to 30 minutes. Our own runs took 21 minutes for one deal from the published package and 24 minutes for two deals
from the release candidate ([roadmap](metropolis-roadmap.md), WS4). Both agents, both
wallets and the seller's service run on your machine; they talk to public Monad RPCs.

We want to learn where the public docs fail you. If you need help, ask, and note in the results
what you asked and why.

## What you need

- macOS on Apple silicon (arm64) or Linux x86_64.
- [uv](https://docs.astral.sh/uv/).
- About 2.15 testnet MON for the buyer, from <https://faucet.monad.xyz>. `quickstart up` prints
  the exact amount. The seller needs none.
- An MCP client for each agent: Claude Code, Codex or Cursor.

## Steps

Follow the [quickstart](metropolis-quickstart.md). In short:

1. Install. This is the one line from the quickstart; keep the index order exactly as written.

   ```sh
   uv tool install --python 3.11 --no-config \
     --index https://poulavbhowmick03.github.io/erebus-metropolis/simple/ \
     --default-index https://pypi.org/simple --index-strategy first-index \
     "erebus-mcp-server==0.4.0.dev2026101801"
   ```

2. Set up and fund.

   ```sh
   erebus-init --metropolis quickstart up --directory ./erebus --allow-agent-funding
   ```

   Send the buyer address it prints at least `send_at_least_mon`. `up` waits for the balance,
   then prepares both wallets and starts the seller's access service.

3. Run the two agents. `up` wrote configs and prompts under `erebus/agents/seller/` and
   `erebus/agents/buyer/`. Start the seller first. Give each agent only its own role's config and
   its own `prompt.md`.

4. Check progress at any time.

   ```sh
   erebus-init --metropolis quickstart status --directory ./erebus
   ```

5. Stop when you are done. Keys and funds are kept.

   ```sh
   erebus-init --metropolis quickstart down --directory ./erebus
   ```

## Known traps

- **uv times out on GitHub downloads.** The wheels come from GitHub release assets. If uv reports
  `operation timed out`, run the install again with `UV_HTTP_TIMEOUT=300`.
- **Laptop sleep.** The seller's access service runs in the background. If your machine sleeps,
  the service is suspended with it. Keep the machine awake for the whole run
  (the [demo notes](../demo.md) say the same).
- **Wait for the MCP server.** In interactive Claude Code, wait until `/mcp` lists the server
  (`erebus-monad-buyer` or `erebus-monad-seller`; releases up to `0.4.0.dev2026101801` say `erebus-buyer` or `erebus-seller`) as connected before pasting the prompt.
- **ankr returns HTTP 429.** The default peer RPC is ankr's free endpoint, which throttles under
  load. Tools back off and retry, and a persistent 429 shows as `RATE_LIMITED`. Wait and retry.
- **Descriptors expire in 30 days.** If you come back later and see `DESCRIPTOR_EXPIRED`, run
  `quickstart renew`.
- **A `pending` result is not an error.** Repeat the same call with the same `operation_ref`.
  Never start a new payment to "fix" a pending one.

Every error has a `code` and a `next_action`. The quickstart's Troubleshooting table lists them.

## What not to share

Do not send us, paste into an issue, or upload any of these:

- anything under `erebus/keys/` or any role's `keys/` directory;
- `wallet.json`, `payment.json`, `access.json`;
- disclosure grants an agent or the auditor exported;
- keyed RPC URLs, or the files that hold them;
- full agent transcripts, unless you have checked them for the above.

Addresses and transaction hashes are public on chain and are fine to share.

## What to send back

Fill in [the results template](metropolis-tester-results.md) and send it to the person who
invited you. Partial results are useful. A run that failed halfway is the most useful kind.
