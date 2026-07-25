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

3. **Never write chain-scanning code for note retrieval.** Use the Discovery Service. Notes live at locations derived from a per-pair shared secret; the recipient already knows where to look. Scanning defeats the design and does not work.

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
/contracts      Cairo — channel logic, offer state, settlement, disclosure
/sdk/ts         TypeScript client
/sdk/py         Python client (agent-framework facing)
/mcp-server     MCP server exposing Erebus tools
/agents         Reference agents demonstrating the loop
/docs           Specs and integration guides
```

Ownership: `/contracts` is Poulav's. `/agents`, `/mcp-server`, `/sdk/py` are Ishita's. `/sdk/ts` is shared — coordinate before changing the interface.

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

- **Cairo:** follow the conventions in `starkware-libs/starknet-privacy`. Match their patterns rather than inventing new ones — this codebase composes their primitives.
- **TypeScript:** strict mode. No `any` in the SDK's public surface.
- **Python:** type hints on all public functions. `uv` for dependency management.
- **Commits:** conventional commits. Keep contract changes and agent changes in separate commits.
- **Tests:** every contract entry point needs at least a happy-path test before it is considered done. Agent policy logic needs unit tests for the accept/reject decision.
- **No LLM-generated Cairo without review.** Poulav reviews every line of contract code regardless of who or what wrote it.

## Commands

```bash
# Contracts
scarb build
snforge test

# TypeScript SDK
pnpm install && pnpm build && pnpm test

# Python SDK / agents
uv sync
uv run pytest

# MCP server
pnpm --filter mcp-server dev
```

*(Update these once the repo is actually scaffolded — they are the intended shape, not verified working commands.)*

---

## Definition of done for the MVP

All five must be true:

1. Two agents autonomously negotiate and reach agreement over a private channel on testnet.
2. Settlement executes atomically through the STRK20 pool with a valid proof.
3. A third party with a granted viewing key can reconstruct the full record.
4. An external agent framework can drive the whole loop through the MCP server without touching Erebus internals.
5. `docs/friction.md` has a real, honest list of what fought us.

Anything beyond this is post-green-light work.