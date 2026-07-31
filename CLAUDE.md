# CLAUDE.md

Guidance for Claude Code working in this repository.

## What this project is

Erebus is private coordination and settlement infrastructure for AI agents, built on Starknet's STRK20 privacy framework. Two agents open an encrypted channel, negotiate as structured state transitions, and settle atomically through the shielded privacy pool, with viewing-key selective disclosure afterwards.

It is **infrastructure, not an application.** Agents are the users. There is no dashboard, no consumer UI. If you find yourself building a frontend, stop and re-read the scope section.

Read [ARCHITECTURE.md](./ARCHITECTURE.md) before writing any code that touches the chain layer.

---

## Non-negotiable technical constraints

These come from the OpenZeppelin audit of `starkware-libs/starknet-privacy`. Getting them wrong means either a broken build or a security hole. Do not "simplify" past them.

1. **NEVER call `__execute__` on-chain.** The privacy pool is a Starknet account contract that exposes `__validate__` and `__execute__` **for local simulation only** — the private key is embedded in the calldata. State changes go through `apply_actions` with a proof.

2. **Always: simulate locally → generate proof → submit via `apply_actions`.** There is no fast path. If a code path skips proof generation, it is wrong.

3. **Never write world/event-scanning code for note retrieval.** Use keyed discovery:
   either the Discovery Service or, for the MVP, upstream's contract provider pattern that
   queries only secret-derived channel/subchannel/note ids over RPC. Scanning public events
   or global storage defeats the design and does not work.

4. **Sequential indexing has no gaps.** Note indices within a channel/subchannel must be contiguous. Do not write code that skips or reorders indices.

5. **Salt types are not uniform across encryption hash functions.** The audit flagged this and StarkWare acknowledged without resolving. Do not assume a uniform salt type across call sites — verify each against the source repo. Symptom of getting it wrong: notes silently fail to decrypt or cannot be located.

6. **Key material never leaves the SDK boundary.** The negotiation policy engine decides *what* to do. It never handles keys. If agent-layer code imports anything key-related, that is an architecture violation.

7. **Never commit key material, seed phrases, or `.env` files.** Testnet keys included.

---

## Naming policy — important

The brand uses Greek mythology: **Erebus** (protocol), **Eleusis** (private channel), **Kleidouchos** (viewing-key holder).

**This vocabulary belongs in README, docs, marketing, and conversation. It does NOT belong in the API surface, function names, variable names, or type names.**

```typescript
// Correct
openChannel()
grantViewingKey()
interface ChannelState {}

// Wrong — do not do this
openEleusis()
becomeKleidouchos()
interface EleusisState {}
```

Rationale: agents and developers read function signatures, not brand guidelines. Obscure terms in the API surface tax every integration. The brand is a marketing layer; the code stays boring and greppable.

The one exception: the package name and top-level namespace may be `erebus` (e.g. `@erebus/sdk`, `import erebus`). That is a product name, which is fine.

---

## Repo layout

```
/sdk/rs         Rust client — the primary implementation, sole holder of key material
/sdk/ts         TypeScript — differential-test oracle only, ships nothing
/sdk/py         Thin binding over /sdk/rs — no protocol logic
/contracts      Cairo — probes only; the MVP needs no contract of our own
/mcp-server     MCP server (Python) exposing Erebus tools
/agents         Reference agents demonstrating the loop (Python)
/docs           Specs and integration guides
```

The call path is `agents → mcp-server → sdk/py → sdk/rs → Starknet`, Python above the
binding and Rust below it. `/sdk/ts` is not in it.

Ownership: `/sdk/rs` and `/contracts` are Poulav's. `/agents`, `/mcp-server` are Ishita's.
`/sdk/py` and `/sdk/ts` are shared — coordinate before changing the interface.

**Two things shifted after the P0.2 and Rust decisions, and the layout above reflects them:**

- **`/contracts` is nearly empty and that is correct.** The salt-lane decision means the negotiation payload rides in note salts and settlement uses the pool's own actions, so no Erebus contract is needed. What lives there is `probes/` — throwaway conformance tests run inside a checkout of `starkware-libs/starknet-privacy`, because their test harness is `#[cfg(test)]`-gated. Poulav's work has moved into `/sdk/rs`.
- **`/sdk/ts` is not dead weight.** It stays alive as the oracle the Rust port is differential-tested against. Do not delete it to "clean up" — two implementations agreeing on the same Cairo vectors is the strongest correctness signal available here, and there is no written spec to fall back on.
**A third shift, decided 2026-07-28: everything above the SDK is Python.**

- **`/sdk/py` is required, but as a binding — not an implementation.** The earlier note here
  said it might be unnecessary. That was resolved the other way: the MCP server is Python
  (the official `mcp` SDK is first-class), so something has to reach Rust from Python.
  What it must *not* become is a third client. **If `/sdk/py` grows a hash function, a
  salt encoder, or anything that could disagree with `/sdk/rs`, that is a bug** — every
  failure mode in this protocol is silent, and a third implementation is a third place for
  a wrong preimage to hide. It marshals arguments and returns results. Nothing else.
- **The seam is a subprocess** — `erebus-cli`, one JSON request on stdin and one envelope on
  stdout. Async stays in Rust. Key *paths* cross the request; key values do not. Protocol 2
  uses opaque random channel handles backed by locked, mode-`0600` Rust state files. The
  Python binding remains on protocol 1 until the shared integration pass; do not add
  protocol logic there to bridge the gap.
- **This is not for x402/ERC-8004 reuse.** That was checked and it does not transfer as
  code: ERC-8004 is EVM-only, and while x402 has an official Python SDK, x402-on-Starknet
  exists only in TypeScript (`NethermindEth/x402-starknet`). Do not reintroduce a
  TypeScript agent layer on the theory that it buys x402 compatibility. It does not.

---

## The interface contract is frozen during MVP

The `ErebusClient` interface in ARCHITECTURE.md §4 is the seam between the two tracks. Ishita's agents build against a mock of it; Poulav implements behind it.

**Do not change this interface unilaterally.** If a change is genuinely needed, it must be agreed by both sides first, because changing it breaks the other track's mock and destroys the parallelism that makes the weekend work.

---

## Scope discipline

StarkWare asked for an MVP to validate the loop. Not a product. The failure mode here is over-building.

**In scope:**
- Two agents, one channel, offer/counter/accept, one atomic shielded settlement, one viewing-key reveal.
- MCP server exposing the tools.
- A 2–3 minute recorded demo of the happy path.
- An honest written list of where the stack fought us.

**Out of scope — do not build these:**
- Any frontend or dashboard
- Free-text encrypted messaging between agents (see ARCHITECTURE.md §7)
- Multi-party channels (more than two participants)
- Cross-chain anything
- Token, tokenomics, or any economic layer
- Multi-round complex negotiation strategies — a simple threshold rule is enough
- Production error handling, retry logic, or observability beyond what the demo needs

If a task is not on a track's task list, it is out of scope. Ask before adding.

---

## Friction is a deliverable

Where the SDK or the primitives fight us is not a failure to hide — it is the exact validation feedback StarkWare asked for. Log it as you hit it in `docs/friction.md`:

- What you were trying to do
- What the stack did instead
- Whether you worked around it and how
- What would have made it easier

Do not paper over rough edges silently.

---

## Conventions

- **Rust:** `#![forbid(unsafe_code)]`. No `unwrap`/`expect` outside tests and const-known values. Prefer newtypes for protocol invariants — the point of writing this in Rust is that things like "structured salts only on zero-amount notes", phase ordering, and `tip == 0` become unrepresentable rather than remembered.
- **Cairo:** follow the conventions in `starkware-libs/starknet-privacy`. Match their patterns rather than inventing new ones — this codebase composes their primitives.
- **TypeScript:** strict mode. No `any` in the SDK's public surface.
- **Python:** type hints on all public functions. `uv` for dependency management. MCP server uses the official `mcp` SDK — `mcp.server.MCPServer` on v2.x (**`FastMCP` was removed in `mcp` 2.0**; it only exists on `mcp<2`). `/sdk/py` stays a binding — no protocol logic, no crypto, no encoding. **Tripwire: `/sdk/py` should never need a known-answer test.** If a test there asserts a computed value, the package is computing something and has become a third implementation. Its tests assert that a call got through and came back, nothing else — that is mechanical enough to catch in review, unlike "no crypto".
- **Commits:** conventional commits. Keep contract changes and agent changes in separate commits.
- **Tests:** every contract entry point needs at least a happy-path test before it is considered done. Agent policy logic needs unit tests for the accept/reject decision.
- **No LLM-generated Cairo or Rust protocol code without review.** Poulav reviews every line, regardless of who or what wrote it. The Rust client is protocol-critical for the same reason the Cairo is: there is no written spec, and every mistake fails silently.

### Nothing lands in `/sdk/rs` unpinned

Every derivation must be pinned by a known-answer test before it is trusted — against the Cairo reference vectors (`sdk/rs/tests/fixtures/cairo-reference-data.json`, regenerate with `snforge test generate_reference_hashes --include-ignored`), or where Cairo emits no vector, against the TypeScript SDK byte-for-byte.

This is not ceremony. Every failure mode in this protocol is silent: a wrong hash preimage derives a storage slot nobody wrote to, and the note is simply "not found" with no error anywhere. The first bug of the Rust port was exactly this — domain tags truncated into a `u128` — and the KATs caught it in thirty seconds instead of a day. See `docs/friction.md` F12.

## Commands

```bash
# Rust SDK — verified working
cd sdk/rs && cargo test

# TypeScript SDK — verified working
pnpm install && pnpm -r typecheck && cd sdk/ts && pnpm vitest run

# Cairo probes — copy into a starknet-privacy checkout first, see contracts/README.md
cd ../starknet-privacy && snforge test p0_2

# Python workspace — verified working. Plain `uv sync` skips workspace members' editable
# installs, so use --all-packages or the 4 erebus-* packages won't be importable.
uv sync --all-packages && uv run pytest

# Reference agents — mock-backed (I2.1, the swap to the real sdk/py seam, hasn't landed)
uv run python agents/src/erebus_agents/demo.py

# MCP server (Python) — verified working, also mock-backed. Requires AGENT_ADDRESS and
# PROVING_SERVICE_URL in the environment; fails loudly without them.
AGENT_ADDRESS=0xyouraddress PROVING_SERVICE_URL=http://placeholder \
  uv run mcp dev mcp-server/src/server.py
```

Toolchain: scarb 2.17.0 / starknet-foundry 0.59.0 (pinned via asdf to match upstream's
`.tool-versions`), Node 20+, Rust stable.

The TS SDK depends on a **sibling checkout** of `starkware-libs/starknet-privacy` — it is
published to GitHub Packages, not npmjs. Clone it next to this repo and run
`cd sdk && npm ci && npm run build` there once. See `docs/friction.md` F8.

---

## Definition of done for the MVP

All five must be true:

1. Two agents autonomously negotiate and reach agreement over a private channel on testnet.
2. Settlement executes atomically through the STRK20 pool with a valid proof.
3. A third party with a granted viewing key can reconstruct the full record.
4. An external agent framework can drive the whole loop through the MCP server without touching Erebus internals.
5. `docs/friction.md` has a real, honest list of what fought us.

Anything beyond this is post-green-light work.
