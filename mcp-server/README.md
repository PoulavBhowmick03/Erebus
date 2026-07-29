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
