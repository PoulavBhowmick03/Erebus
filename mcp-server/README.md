# MCP server

Owned by Ishita (CLAUDE.md, repo layout). Exposes the Erebus tools so an external agent
framework can drive the whole loop without knowing Erebus exists — that is definition-of-done
item 4.

**Python, on the official `mcp` SDK (`mcp.server.MCPServer`).** Decided 2026-07-28. This directory used to
hold a one-line TypeScript stub from the initial scaffold; it was removed on 2026-07-29
because it predated that decision and would have started this track in the wrong language.
There is no TypeScript above the SDK boundary and there should not be — see the note in
CLAUDE.md about x402, which is the argument people reach for and which does not hold.

```bash
uv sync
uv run mcp dev mcp-server/src/server.py
```

## Where it sits

```
agents → mcp-server → sdk/py → sdk/rs → Starknet
```

Python above the binding, Rust below it. `sdk/py` is a *binding*, not a client — if it grows
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
export EREBUS_CLI=~/Developer/erebus/sdk/rs/target/debug/erebus-cli
set -a; . ~/.erebus-b/env; set +a          # the identity's own env, see docs/runbook.md
uv run mcp dev mcp-server/src/server.py
```

The env file supplies `AGENT_ADDRESS`, `PROVING_SERVICE_URL`, `STARKNET_RPC_URL`,
`POOL_ADDRESS`, `STARKNET_CHAIN_ID`, `TOKEN_ADDRESS`, `POOL_KEY_FILE`, `ACCOUNT_KEY_FILE`
and `EREBUS_STATE_DIR`. Every one is checked at startup, and the two key files are checked
for existence, because discovering a missing key twenty seconds into a proof has already
cost an agent a turn and some gas.

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
