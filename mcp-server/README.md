# MCP server

This page describes current `main`, which speaks Protocol 4 and exposes thirteen tools.
The published `v0.1.0` packages speak Protocol 2 and expose ten tools. Build the current
checkout until `v0.2.0` is published.

Owned by Ishita (CLAUDE.md, repo layout). Exposes the Erebus tools so an external agent
framework can drive the whole loop without knowing Erebus exists, that is definition-of-done
item 4.

**Python, on the official `mcp` SDK (`mcp.server.MCPServer`).** Decided 2026-07-28. This directory used to
hold a one-line TypeScript stub from the initial scaffold; it was removed on 2026-07-29
because it predated that decision and would have started this track in the wrong language.
There is no TypeScript above the SDK boundary and there should not be, see the note in
CLAUDE.md about x402, which is the argument people reach for and which does not hold.

```bash
uv sync
uv run mcp dev mcp-server/src/server.py
```

## Where it sits

```
agents → mcp-server → sdk/py → sdk/rs → Starknet
```

Python above the binding, Rust below it. `sdk/py` is a *binding*, not a client, if it grows
a hash function, a salt encoder, or anything that could disagree with `sdk/rs`, that is a bug.

## What it must not do

Hold key material. The library runs inside the agent operator's own process against the
operator's own prover, because the proving call carries the pool private key in the clear.
The server takes a prover URL and an identity from config and should fail loudly if they are
absent rather than falling back to a shared endpoint. See `docs/custody-design.md`.

## Running against the real Rust client

`EREBUS_BACKEND` selects the client. It defaults to `mock`, so the agent track stays
runnable with no chain, no keys and no gas. Set it to `seam` to drive the real Rust client
through `sdk/py`'s subprocess binding.

```shell
export EREBUS_BACKEND=seam
export EREBUS_SETTLEMENT_ROLE=both       # use payer/payee for autonomous agents
export EREBUS_CLI=~/Developer/erebus/sdk/rs/target/debug/erebus-cli
set -a; . ~/.erebus-b/env; set +a          # the identity's own env, see docs/runbook.md
uv run mcp dev mcp-server/src/server.py
```

The env file supplies `AGENT_ADDRESS`, `PROVING_SERVICE_URL`, `STARKNET_RPC_URL`,
`POOL_ADDRESS`, `STARKNET_CHAIN_ID`, `TOKEN_ADDRESS`, `POOL_KEY_FILE`, `ACCOUNT_KEY_FILE`
and `EREBUS_STATE_DIR`. `EREBUS_WIRE_VERSION` selects `v2` or `v3` for newly opened channels
and defaults to `v3`; existing channel records keep their persisted version. Every required
setting is checked at startup, and the two key files are checked
for existence, because discovering a missing key twenty seconds into a proof has already
cost an agent a turn and some gas.

`EREBUS_SPENDING_LIMITS` configures per-token, per-deal, and daily caps.
`EREBUS_SPENDING_STATE_PATH` overrides the derived cap-ledger path.
`EREBUS_INTENT_STATE_DIR` overrides the MCP intent-record directory. Reference agents use
`EREBUS_CALLER_INTENT_PATH` or `EREBUS_STATE_DIR` for their own durable caller records.

**One server per identity.** Two identities in one process would put both pool keys in the
same heap, which is the arrangement `docs/ishita.md` rejected when it chose two servers over
one multi-tenant one.

### What crosses the seam

Requests carry *paths* to the key files. This process never opens them; the Rust binary
does. That is the reason the seam is a subprocess rather than PyO3, and it holds only for as
long as nothing in Python reads those paths.

Blocking seam calls run through `asyncio.to_thread`. A write is a preflight, a proof of
about twenty seconds, a fee estimate, a submission and a receipt wait; calling that directly
from a coroutine would stall the event loop for the whole period, and a second tool call
could not be parsed until the first settled.

## Registering with an MCP client

`scripts/erebus-mcp.sh <env-file> <payer|payee|both>` launches one server for one identity
over stdio. The role is required: `accept_and_settle` spends the caller's private notes, so
a payee server refuses that call rather than trusting a prompt to remember payment direction.
It sources the identity's env, defaults `EREBUS_BACKEND` to `seam`, and refuses to start if
`erebus-cli` is not built.

For Claude Code:

```shell
claude mcp add erebus-seller -- ~/Developer/erebus/scripts/erebus-mcp.sh ~/.erebus-b/env payee
claude mcp add erebus-buyer  -- ~/Developer/erebus/scripts/erebus-mcp.sh ~/.erebus-c/env payer
```

For a client that reads a JSON config (Claude Desktop and most others):

```json
{
  "mcpServers": {
    "erebus": {
      "command": "/Users/odinson/Developer/erebus/scripts/erebus-mcp.sh",
      "args": ["/Users/odinson/.erebus-b/env", "payee"]
    }
  }
}
```

Two agents negotiating means two client configurations, each naming a different env file.
Nothing about the server is shared between them.

### Payment denominations

`get_note_balance()` reports the caller's spendable note denominations and total. Payer-role
servers check `amount <= total` before every proposal, counter and settlement, so an
autonomous buyer cannot spend several proof rounds agreeing to a price it cannot pay.
Payee-role proposals are asks and therefore do not inspect the payee's notes.

Settlement covers the price from whatever notes it selects and returns any excess as a new
change note, so any positive amount up to `total` is payable. Add denominations before the
agent session with `scripts/agent.sh <payer-env> fund <amount>`.

### Deal disclosure

`grant_viewing_key(operation_id, channel_handle, deal_id, grantee, expires_at, output_path)` encrypts one
wire-v3 deal to the grantee's registered pool key. It creates `output_path` with mode `0600`
and refuses to overwrite an existing file. Its tool result contains metadata and the path,
not the encrypted capsule. Run `reveal(grant_path)` from the grantee's identity-bound server.
Expiry prevents a later open; it cannot erase data the recipient opened earlier.

### Durable writes and recovery

Every MCP write requires `operation_id` in Protocol 4. The ID format is `op_` plus 64
lowercase hexadecimal characters. Persist the ID and canonical request before the call.

`reconcile()` is read-only. It classifies all Rust journal entries and never submits a
transaction. `resume_operation(operation_id)` is the only recovery tool that can submit.
It uses the original ID and follows the Rust classification. `rebuild_state()` recreates
missing channel records from keys and chain data without replacing existing records.

## Using it from outside this repository

Three things have to reach the target machine, and only one of them is a packaging problem.

**The Python package.** `v0.1.0` publishes `erebus-mcp-server` and `erebus-sdk` through the
GitHub Pages package index. Those artifacts predate Protocol 4.

**The `erebus-cli` binary.** The `erebus-cli` platform wheel carries the Rust binary for
Linux x86-64 and macOS arm64. Intel macOS is unsupported. The Python packages resolve this
wheel through the same package index.

**A prover, an RPC and a funded identity.** This is the one that does not yield to
packaging. `compile_actions` sends the pool private key as calldata to both the prover and
the RPC, so an operator who does not control those has handed over the ability to read
everything that identity does. StarkWare's Sepolia prover is not public and we were asked
not to share it. Self-hosting is Pathfinder plus `transaction-prover`.

So the honest statement is that Erebus is software an operator runs, not a service anyone
can point at. Publishing the SDK shortens the install; it does not remove the prover
requirement, and no amount of packaging will, because the requirement is the custody
property rather than a missing feature. See `docs/custody-design.md`.

## Self-provisioning identities

An agent cannot ask the server to create its wallet. `ServerConfig.from_env()` runs at
import, so the identity is bound before the first tool call and a newly created one could
not be used by that process. Provisioning therefore happens in the launcher.

```shell
claude mcp add erebus --env EREBUS_PROVISION_FROM=erebus-agent \
  -- ~/Developer/erebus/scripts/erebus-mcp.sh ~/.erebus-mine/env payer
```

When the env file is absent and `EREBUS_PROVISION_FROM` names a funded sncast account, the
launcher creates the account, seeds it with STRK (`EREBUS_PROVISION_STRK`, default 15),
deploys it, generates a pool key, writes the env, approves the pool and shields once, which
also registers the identity. It takes a few minutes, mostly waiting ten blocks for the
approve to mature. Later starts find the env and skip all of it.

The funder exists because a public faucet is a web form with a captcha, so nothing automated
gets past one. On mainnet the equivalent is a treasury account, which is a policy decision
rather than a script.

Setup noise goes to stderr. Anything on stdout would be parsed as an MCP protocol message.
