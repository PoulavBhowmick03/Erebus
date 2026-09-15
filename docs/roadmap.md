# Erebus roadmap

Updated September 12, 2026. This roadmap starts after the published `v0.2.0` release.
Versions below are planning targets, not release dates or shipped capabilities.
Completed work and dated measurements belong in [status](./status.md) and [run records](./runs/).

## Baseline: v0.2.0

Protocol 4 provides encrypted negotiation, payer-side settlement, change, repeat deals,
recovery, and recipient-bound disclosure. The public release contains thirteen MCP tools.
Recorded mainnet settlements use STRK; stablecoin settlement is unverified in this repository.

Erebus hides the terms, not the relationship. The [privacy model](./privacy-model.md)
remains canonical. A release or a new application does not imply production readiness.

## 1. What Erebus is for, after STRK20 shipped privacy

STRK20 now provides shielded balances, **private transfers, and private swaps** natively, and
StarkWare has said it will release its own SDK and open-source wallet API. Those are platform
capabilities available to every Starknet team.

Erebus is therefore **not** a privacy product. STRK20 lets two parties *pay* privately; it has
no concept of an *agreement*. Erebus adds the agreement: offer, counter-offer and acceptance
bound to the payment in one atomic action set, plus a receipt scoped to one deal.

This shapes the roadmap below. Work that duplicates a platform feature is out; work that makes
the agreement layer usable and legible is in.

**Removed from this roadmap, deliberately:** standalone private transfers, private swaps, and
generic token-selection APIs. Earlier drafts scheduled them. STRK20 provides them, and
rebuilding them would compete with the platform Erebus depends on. If a paying customer ever
requires an Erebus-native transfer, reopen it as a design question, not a scheduled feature.

## 2. Next target: make a settlement mean something

All four items run on the published `v0.2.0` wire. None needs a protocol change.

| # | Work | Why it is next |
| --- | --- | --- |
| 1 | **`memo_hash` preimage convention.** A canonical record of what was bought, hashed into the existing 128-bit field, published as a spec with known-answer vectors | A settlement currently records "0.8 STRK agreed, 0.8 STRK paid" and nothing about the subject of the deal. [usecases.md](./usecases.md) has listed this as open work. It is the precondition for receipts, disputes and third-party integration |
| 2 | **Counterparty allowlist**, beside the existing spending limits in `mcp-server/src/erebus_mcp/spending.py` | The policy layer caps *how much* an agent may spend and nothing caps *to whom*. Half a policy |
| 3 | **`funding_plan` read-only tool** — takes an intended run shape, returns the exact allowance each identity needs | [friction.md](./friction.md) F42. A wrong allowance costs a failed write and 6 STRK, and it lands on a new operator's first attempt. The 2026-09-11 run added a second cause: an `approve` must be ten blocks deep before the next write simulates against it |
| 4 | **Minimal counterparty discovery** | Two agents can transact only if each already knows the other's address and both are registered. Without discovery there is no loop an outside party can enter |

Acceptance for each: known-answer vectors where a value is derived, tests where a decision is
made, and no protocol-critical Rust merged without review.

## 3. Distribution

Not engineering work, but it belongs on the roadmap because it gates every product question.

- List the MCP server in the public registries — mcp.so, smithery.ai, glama.ai, the official
  registry, `awesome-mcp-servers`. This is where agent developers look and Erebus is in none
  of them.
- Apply to the Starknet Foundation Proof of Privacy incubator and the Seed Grant programme.
- Replace the site's primary call to action. It currently offers a demo, an explanation and
  source. None of those is "start using it."

Reasoning and sources are in `claudesplan.md` at the repository root.

## 4. Privacy and operational work

Open work carried forward from the release:

- Durable authorization covers all write paths, fees, and uncertain operations.
- Encrypted backup and restore preserve the operation journal and keys consistently.
- Provider failure drills cover lost proof results, expired proofs, RPC drift, and restarts.
- Monitoring reports operation status without exposing terms, credentials, or grants.
- An independent review covers protocol, custody, disclosure, and recovery changes.

Standing constraints that no item above removes: the counterparty address is public at
channel open (F38); each write costs a proof and a pool fee (F27, F7); the prover and the
preflight RPC both receive the pool private key (F14).

The [targeted review](./v0.2-targeted-review.md) retains the hosted prover's one-time result
delivery risk. It does not substitute for independent security review.

## 5. Release and product gates

Each feature release needs compatible artifacts, relevant tests, current documentation,
and explicit owner publication approval. The release workflow remains the executable source
for artifact checks. [Production gaps](./production-gaps.md) remain open after publication.

Product progress uses external evidence: agents completing paid work through Erebus, repeat
deals between the same parties, paying integrations, support hours, intervention rate, and
cost per completed deal. Maintainer-funded runs, grants, and demonstrations are counted
separately from customer demand.

Per-transaction pricing is ruled out by measurement, not by preference: a three-write
negotiated deal cost 26.38 STRK in the 2026-08-31 canary, of which 18 was the pool fee, which
Erebus does not collect. Expansion to escrow, bridges, outcome-only proofs, or additional
chains requires a separate customer requirement and design decision. No dates or revenue
forecasts are committed here.
