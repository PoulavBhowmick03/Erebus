# Status

**As of 2026-08-18.** One page, current, and the tiebreaker: where any other document in
this repository disagrees with this one, this one is right and the other is stale.

Nine documents describe this system and they were written across three weeks in which the
privacy claim changed twice. That is why this page exists.

---

## In one line

Erebus negotiates and settles privately between two agents on Starknet Sepolia. The terms
are confidential and demonstrated to be so. The relationship is not.

---

## What is true right now

| | |
|---|---|
| Network | Sepolia only. Never run on mainnet |
| Wire | v2 — AES-256-GCM-SIV, 128-bit tag, five note salts |
| Live evidence | one full negotiation and atomic settlement, `0x14b38e9d…4cb3` |
| Version | `0.0.1` across every package |
| Tests | 213 Rust, 70 Python, 38 TypeScript |
| CI | green on every push: Rust, Python, secret scan, dependency hashes |
| Install | wheels build and verify, published nowhere |

## What Erebus does

Two agents open a channel, exchange offers and counters as encrypted state, and settle
atomically in one action set. Either side can later hand a scoped viewing grant to a third
party, who reconstructs the whole exchange from chain data without gaining the ability to
spend.

Agents drive it through MCP, so any framework in any language can use it. No contract of our
own is deployed: the negotiation rides in note salts the pool already provides.

## What Erebus does not do

- **Hide who you are dealing with.** The counterparty's address is written in public
  calldata at channel-open. This is upstream of our encryption and no wire change fixes it.
  See F38 and [privacy-model.md](./privacy-model.md).
- **Hide that a negotiation happened.** Wire v2 leaves 59 bits zero-filled, giving every
  message a constant shape a reader can fingerprint.
- **Run on mainnet.** No published mainnet prover exists, and self-hosting needs a synced
  Pathfinder node.
- **Support more than one deal per pair of addresses.** One channel per pair is a protocol
  constraint; one deal per channel is our own rule and is the first thing to revisit.
- **Escrow, or deferred delivery.** Settlement is atomic, so there is no "agree now, deliver
  later". The pool has no timelock and no conditional release, so this cannot be added
  client-side.

## The honest privacy claim

> Erebus hides the terms, not the relationship.

Content confidentiality is demonstrated: an observer script with no key recovers the full
terms from wire v1 and nothing from wire v2. Traffic confidentiality is not, and the four
known leaks are listed in severity order in [privacy-model.md](./privacy-model.md).

Never describe this as private in an absolute sense.

---

## Which document to trust for what

| Question | Read | Confidence |
|---|---|---|
| What leaks, and what does not | [privacy-model.md](./privacy-model.md) | current, canonical |
| What fought us, and how | [friction.md](./friction.md) | current, 38 entries |
| What to do next | [roadmap.md](./roadmap.md) | current |
| How to reproduce a run | [runbook.md](./runbook.md) | mostly current, see below |
| How the code works | [tech.md](../tech.md) | current, source-cited |
| Does this fit my use case | [usecases.md](./usecases.md) | **stale**, see below |
| What is missing for production | [production-gaps.md](./production-gaps.md) | **stale**, see below |
| Key custody reasoning | [custody-design.md](./custody-design.md) | current as a decision record |
| The pitch | [poc.md](./poc.md) | current |

### Known stale claims, not yet corrected

These are wrong in the source documents and listed here so nobody quotes them:

- **`usecases.md` fit-test row 6** says the repository "has not completed a live wire-v2
  negotiation or an independent cryptographic review". Half wrong since 2026-08-07: the live
  wire-v2 negotiation completed. The review half is still true.
- **`production-gaps.md` §4** predates F38 and treats relationship exposure as an inference
  from timing rather than an address written in the clear.
- **`runbook.md`** carries an evidence boundary dated 2026-07-31 saying wire v2 "has not yet
  completed this live run". It has.
- **`scripts/observer.py`** classifies wire-v1 traffic as wire v2. The recovery results are
  unaffected — v1 content is recovered, v2 is not — but the version label it prints is not
  trustworthy.

---

## What ships, and what does not

**Ships:** `/sdk/rs` is the implementation and the only holder of key material. `/sdk/py` is
a binding with no protocol logic. `/mcp-server` exposes the tools. `/agents` are reference
agents.

**Does not ship:** `/sdk/ts` is the differential-test oracle and exists so two independent
implementations can be checked against the same Cairo vectors. It is still on wire v1.
`/contracts` holds throwaway probes and is nearly empty, which is correct.

## Where the keys go

Three distinct keys, and conflating them is the usual mistake:

- **Starknet account key** — signs transactions. Custody. Never leaves the Rust process.
- **Pool private key** — the STRK20 identity. Confidentiality. Sent in `compile_actions`
  calldata to two operator-chosen endpoints: the prover and the preflight RPC. Both can
  reconstruct that identity's full history. The submitted transaction does not carry it.
- **Pool auditor key** — pool-wide, StarkWare's, set once at registration, no rotation.

Python never sees key material, only file paths. It does handle bearer viewing grants, so
MCP transcripts sit inside the disclosure trust boundary.

---

## The next three things

1. **Grant the Sepolia allowance, then do one full run on merged code.** `doctor` reports
   the allowance as the only failing check. The live evidence above predates change notes,
   the allowance path, string amounts, and the MCP agents, so nothing on `main` has been
   proven end to end. That run also produces the receipt, observer output, and disclosure
   that three other tasks need.
2. **Decide where wheels are published**, and move `server.py` into the package with an
   entry point. Until then `uvx erebus-mcp-server` resolves and does nothing.
3. **Fill the spare wire bits with random padding.** It closes the only privacy leak that is
   actually within reach — F38 is upstream, and the public funding leg has no fix.
