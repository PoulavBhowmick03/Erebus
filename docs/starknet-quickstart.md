# Starknet Mainnet Quickstart

> **Mainnet, real STRK, unaudited.** Every write below is an `apply_actions` transaction on
> Starknet mainnet that pays the live pool fee from public STRK. Do not put value you care
> about through it. Available from the first release built from a commit that contains
> `erebus-init --starknet`; the published `v0.3.0` and `0.4.0.dev2026101801` packages do
> not have it.

One command sets up a buyer (payer) and a seller (payee) identity on Starknet mainnet, with
one confirmation for both, and writes MCP configs and prompts for two agents. It drives the
same `erebus-init` setup described in [onboarding.md](./onboarding.md) once per role, so the
same resume rules apply: an interrupted write is finished under its original operation ID and
never sent again.

It never bridges, swaps or touches another chain, and it runs beside the
[Monad quickstart](./metropolis-quickstart.md) from the same install without sharing a
directory, a server name or an env file with it.

## What you need

- The `erebus-mcp-server` install (it ships `erebus-init` and `erebus-cli`).
- A Starknet **mainnet** RPC URL over https. Use one you control: the RPC receives each
  identity's pool key during every proof (`compile_actions` preflight).
- A Starkscan API key for the mainnet prover, `api.starkscan.co`, which also receives the pool
  key. Pass it as `STARKSCAN_API_KEY`, as an owner-only file with `--starkscan-key-file`, or at
  the hidden prompt. It is written only to each role's mode-0600 `mcp.env` and setup file.
- STRK on mainnet to fund two fresh addresses (amounts below, computed from the live fee).

## 1. Install

```sh
uv tool install --python 3.11 --no-config \
  --index https://poulavbhowmick03.github.io/erebus-metropolis/simple/ \
  --default-index https://pypi.org/simple --index-strategy first-index \
  "erebus-mcp-server==<a release with --starknet>"
```

This is the same install as the Monad quickstart. `erebus-init --starknet` finds `erebus-cli`
and `erebus-mcp-server` inside that tool environment and writes their absolute paths into the
generated configs, so the agents run exactly the install that set them up.

## 2. Up

```sh
erebus-init --starknet quickstart up --directory ./erebus-starknet \
  --rpc-url-file ~/.config/erebus/mainnet-rpc.url --max-strk 100
```

`--directory` must be new. `up`:

1. checks the RPC is https and serves `SN_MAIN`, the prover is `api.starkscan.co`, and the
   installed `erebus-cli` speaks this install's seam protocol; anything else is refused before
   a directory or key exists;
2. writes `starknet-quickstart.json` (backend, network, chain, pool, token, and the RPC and
   prover **hosts** only; a keyed RPC URL never lands there);
3. creates a new OpenZeppelin account per role with the packaged Rust onboarding (or imports
   one with `--buyer-account` / `--seller-account sncast:alpha-mainnet:<name>`, ids from
   `erebus-init --list-accounts`), and its pool key;
4. reads the **live** pool fee and gas reserve from Rust's `inspect`, sizes each role's plan,
   and prints both addresses and the exact STRK each must hold;
5. polls both balances (`--funding-timeout`, default 1800 s);
6. asks once (below), then per role, buyer first: deploy, approve the exact allowance, wait
   until the approval is 10 blocks deep, shield (this registers the pool identity), wait 10
   blocks, `doctor`;
7. runs `start`.

If it stops anywhere (funding, a timeout, a lost response), run the same `up` again: it
continues from the saved setup and never repeats a write. Once both roles are ready, `up`
refuses to run again; use `start`, `status` and `renew`.

### What to fund

Per role, from the live fee `F` (STRK per charged write) and gas reserve `G` (STRK per
transaction, an estimate) that `up` prints:

| | Buyer (payer) | Seller (payee) |
|---|---|---|
| Charged writes | open, propose, settle = 3 | open, counter = 2 |
| Shield (registers; a new address cannot use 0) | `--buyer-deposit`, default 0.5 | `--seller-deposit`, default 0.01 |
| Exact approval | `0.5 + 4F` | `0.01 + 3F` |
| Fund to | `0.5 + 4F + 6G` | `0.01 + 3F + 5G` |

The shield is itself a charged write, so it adds one `F`. Gas covers each charged write plus the
deployment and the approval. `--extra-rounds K` adds `K` writes to each role (one more counter
each). **Use the numbers `up` prints, not this table**: the fee is read from the pool when you
run it.

For illustration only, with fixture values `F = 6` and `G = 3` (the values the tests use, not a
quote): buyer 24.5 approval, fund to 42.5; seller 18.01 approval, fund to 33.01; 75.51 STRK in
total. `--max-strk` must be at least the printed total or `up` returns `plan_exceeds_max` and
sends nothing.

### The one confirmation

In a terminal you type `mainnet <total STRK>`, exactly as printed, for example
`mainnet 75.51`. Without a terminal, `up` returns `authorization_required` with the plan until
you rerun it with `--yes`. The notice states that the Starkscan prover and your RPC receive each
identity's pool key during every proof, and that registration encrypts the pool key to the pool
auditor. If the live fee rises after you confirm, so that an approval would be larger than what
you confirmed, setup stops with `authorization_required` and asks again.

## 3. Run two agents

`up` writes, for each role:

```text
erebus-starknet/agents/buyer/{claude-code.mcp.json, codex.config.toml, cursor.mcp.json, prompt.md}
erebus-starknet/agents/seller/{claude-code.mcp.json, codex.config.toml, cursor.mcp.json, prompt.md}
```

The servers are named `erebus-starknet-buyer` and `erebus-starknet-seller`. Each launches the
absolute `erebus-mcp-server` with `--config <directory>/<role>/mcp.env`, a complete env file:
role, network, the RPC and prover, the key **paths**, `EREBUS_CLI`, the guardrails below, and
state directories inside the quickstart directory. An explicit `--config` wins over anything the
MCP client inherits, so a stray `EREBUS_*` variable in your shell cannot change the identity.

```sh
erebus-init --starknet quickstart start --directory ./erebus-starknet
```

`start` is read-only. It checks both identities are registered (and so each one's
counterparty), the allowance and public STRK cover the remaining writes at the live fee, every
approval is 10 blocks deep, `reconcile` is clean with no unresolved intent, and `doctor` is
ready. Then it prints the registration commands, for example
`claude mcp add erebus-starknet-buyer -- /abs/erebus-mcp-server --config /abs/erebus-starknet/buyer/mcp.env`,
and both prompts.

The prompts embed the counterparty's address and this order:

1. both: `doctor`; the buyer also `get_note_balance`;
2. each opens its own channel to the other (`open_channel`);
3. the buyer waits for the counterparty's channel (the operator confirms the seller's open
   returned), then `propose_offer` at half the price cap; the seller `wait_for_offers`;
4. the seller makes at most one `counter_offer` (at three quarters of the cap), plus one per
   extra round;
5. the buyer settles the seller's offer with `accept_and_settle` if it is at or below the cap.
   The seller never calls `accept_and_settle`; its server refuses it.

Start the seller first. Each agent sees only its own server.

## Guardrails

The generated env files turn these on; any MCP identity can set them:

- `EREBUS_SPENDING_LIMITS`: per deal at most `--price-cap` (default 0.2 STRK), per day twice
  that, enforced below the agent before `accept_and_settle` reaches Rust.
- `EREBUS_MAX_WRITES`: a lifetime cap on the charged writes this identity may start (buyer 3,
  seller 2, plus extra rounds). It is checked before the caller intent is recorded, so a refused
  write leaves nothing behind and is never proved. Retrying the same `operation_id` never
  counts twice; a claimed write that then fails still counts, because the counter cannot know
  whether Rust reached the chain. The refusal is `SPENDING_LIMIT_EXCEEDED` naming
  `EREBUS_MAX_WRITES`. The counter lives in `write-budget.json` in the identity's state.
- `EREBUS_REQUIRE_CANONICAL_NETWORK=1`: the server refuses to start on a `custom` network or any
  chain or pool that differs from the canonical mainnet preset.

## 4. Status, renew, down

```sh
erebus-init --starknet quickstart status --directory ./erebus-starknet [--offline]
erebus-init --starknet quickstart renew  --directory ./erebus-starknet --deals 2 --max-strk 50
erebus-init --starknet quickstart down   --directory ./erebus-starknet [--offline]
```

- `status`: per role the address, deployed, registered, public STRK, allowance and the writes it
  covers at the live fee, notes, writes used against the cap, and unresolved operations.
  `--offline` reports only local files.
- `renew --deals N`: approves the buyer `2N × F` (propose and settle per deal) and the seller
  `N × F` (one counter), each an ERC-20 approve that **replaces** the previous allowance, waits
  until each approval is 10 blocks deep, and raises each `EREBUS_MAX_WRITES` to the writes used
  so far plus the new ones. It confirms the same way (`mainnet <total>` or `--yes`) and respects
  `--max-strk`. Restart both MCP servers afterwards. It does not shield again: the buyer's note
  must still cover each price.
- `down`: prints the commands that remove both servers from Claude Code, Codex and Cursor, the
  balances, and how to sweep. Nothing is deleted or sent. Public STRK can be sent from each
  account with a wallet or `sncast` holding its account key; this release has no unshield, so
  shielded notes stay spendable by that identity in later settlements. Keep the directory: an
  identity's pool key cannot be replaced.

## Running alongside the Monad quickstart

Use sibling directories:

```sh
erebus-init --metropolis quickstart up --directory ./erebus-monad --allow-agent-funding
erebus-init --starknet   quickstart up --directory ./erebus-starknet --rpc-url-file … --max-strk 100
```

Each refuses the other's directory: the Monad quickstart's `quickstart.json` and this one's
`starknet-quickstart.json` mark them. The Monad servers are `erebus-monad-buyer` and
`erebus-monad-seller` (renamed from `erebus-buyer` / `erebus-seller`, which clashed with the
single-identity Starknet registrations in [setup.md](./setup.md)); these are
`erebus-starknet-buyer` and `erebus-starknet-seller`. A buyer agent can hold both a Monad and a
Starknet server; the two never settle against each other.

## Mock

```sh
erebus-init --starknet quickstart up --mock --directory ./erebus-starknet-mock
```

The same layout, prompts and guardrails against the mock backend, with its shared store inside
the directory. No network, no keys, no funds. `status` and `renew` work; `renew` only raises the
write caps.

## What this page does not establish

The quickstart is exercised by offline tests with a fake chain and seam
(`mcp-server/tests/test_starknet_quickstart.py`). No live mainnet run of it is recorded yet. A run
is new evidence; record it the way `docs/runs/` does, with transaction hashes.
