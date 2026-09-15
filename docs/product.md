# First product: agent-to-agent paid work

Updated September 12, 2026. This replaces an earlier draft that proposed a standalone
private-transfer application. That premise no longer holds: STRK20 provides private transfers
and private swaps natively, so building them would duplicate a platform feature.

No customer demand, price, or willingness to pay is verified. Everything below is a
hypothesis to test, except where it cites the repository or a dated run record.

## What the product is

An agent pays another agent for a service. The two negotiate — offer, counter-offer,
acceptance — and the moment they agree, the payment executes in the same atomic action set.
The amounts and terms are confidential against a chain reader. Either party can later hand a
third party a receipt scoped to exactly that one deal.

This already works. The 2026-09-07 run did it across two different agent frameworks
([run record](./runs/2026-09-07-mainnet-2strk-agents.md)). What is missing is not the
mechanism, it is everything that makes the mechanism legible to somebody who did not build it.

## Why this and not private payments

STRK20 lets two parties *pay* privately. It has no concept of an *agreement*. There is no
"we agreed and then they didn't pay" gap in Erebus, because acceptance and payment share one
proof (`sdk/rs/src/channel.rs`). That binding — not confidentiality — is what Erebus adds.

The honest limit, from [privacy-model.md](./privacy-model.md): the pool does not understand
the agreement. The amount-equality check between what was accepted and what was paid is
Rust-side validation, not a circuit guarantee, and a hostile client can write a semantically
inconsistent record that still satisfies pool rules.

## The four gaps

| # | Gap | What exists today | What is missing |
| --- | --- | --- | --- |
| 1 | **A settlement has no subject** | `memo_hash`, a 128-bit field in `OfferTerms` that round-trips through the wire | No defined preimage, canonicalization, or signature convention. The record says what was paid, never what was bought |
| 2 | **Policy covers amount, not counterparty** | `EREBUS_SPENDING_LIMITS` enforces per-token, per-deal and daily caps below the agent (`mcp-server/src/erebus_mcp/spending.py`) | No allowlist. An agent within its budget may pay anyone |
| 3 | **The first run fails on funding** | `doctor` reports allowance against the live fee | Nothing tells an operator what allowance a *planned* run needs. A wrong value costs a failed write and 6 STRK (F42), and an `approve` must be ten blocks deep before the next write simulates against it (`sdk/rs/src/execution.rs:40`) |
| 4 | **No way to find a counterparty** | Channels open against a known address; the client verifies registration first | Two agents transact only if each already knows the other. There is no entry point for a third party |

Gap 1 is the one that matters most and is cheapest to close. The field already exists in the
wire, so defining its contents needs no protocol change — and a defined receipt format is the
only thing on this list that a competitor cannot trivially reproduce, because its value comes
from other people adopting it.

## Sequence

| Milestone | Deliverable | Acceptance |
| --- | --- | --- |
| 1 | `memo_hash` preimage spec: canonical JSON service record, hashing rules, versioning | Known-answer vectors pinned against Cairo reference data or the TypeScript oracle, per `CLAUDE.md`. Round-trips through a live settlement |
| 2 | Counterparty allowlist in the policy layer | Accept and reject cases in `mcp-server/tests/test_spending.py`. Enforced below the agent, like the existing caps |
| 3 | `funding_plan` read-only MCP tool | Returns the exact allowance per identity for a planned run shape. Exercised against `EREBUS_BACKEND=mock` before any live use |
| 4 | Minimal counterparty discovery | An agent that knows no addresses can find one and open a channel |

Out of scope, and stated so people stop asking: escrow, refunds, and delivery proof
(settlement is atomic and the pool has no timelock, so this cannot be added client-side —
see [status.md](./status.md)); any frontend; protocol logic in `/sdk/py`.

## Who uses it and who pays

These are different groups and conflating them is the usual mistake.

- **Agent developers** use it. They need payment tools that an agent can call, and they find
  those through MCP registries. This is the group the four gaps above serve.
- **Agent platforms and marketplaces** pay for it. They have traffic and a real problem:
  their customers' prices are public. One embedded integration is worth more than many
  individual users.

Effort should go to making the first group successful, because that is the evidence the
second group needs.

## What the economics rule out

From the 2026-08-31 canary ([run record](./runs/2026-08-31-mainnet-060-040-canary.md)): a
three-write negotiated deal cost **26.38 STRK**, of which **18 was the pool fee**, which
StarkWare collects and Erebus does not.

For infrastructure to stay under 1% of the payment, a deal must exceed roughly 2,638 STRK.

**A per-transaction take rate is therefore not viable.** Any fee sits on top of a cost that
already dominates. What remains plausible: an embedded platform contract, and paid
integration and support. Neither is evidenced. Ecosystem grants are runway, not revenue.

## Open decisions

- Which settlement asset after the STRK baseline, and validating one full deposit → settle →
  exit path for it.
- Whether the `memo_hash` record should be signed, and by whom.
- Whether discovery is a registry Erebus operates or a convention others implement.
- Custody: who holds the pool key, and whether a hosted option is offered at all.
- Pricing, support commitments, and maximum exposure for any first integration.

## Stop conditions

If agent developers do not adopt the receipt format, gap 1 has failed and the moat argument
with it. If nobody needs confidential *agreements* — only confidential payments — then STRK20
already serves them and Erebus is a library, not a product. If a platform integration cannot
be closed, the honest read is supported services, not recurring software.

Material real value requires the remaining [production work](./production-gaps.md).
