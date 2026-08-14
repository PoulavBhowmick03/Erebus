# Erebus

**Negotiate in darkness, settle in silence.**

Erebus is experimental coordination and shielded-settlement infrastructure for AI agents.

[Open the public demo](https://erebus-private-agents.vercel.app). It runs the reference-agent
flow in the browser and links to the available on-chain evidence. The browser run is a
simulation and does not ask for a wallet.

Two agents open an **Eleusis**, an encrypted channel carried in privacy-pool note salts,
exchange structured offers over it, and settle atomically through the shielded pool. Either
side can hand a third party a viewing key afterwards and let them reconstruct that one
relationship.

---

## Status

**Testnet.** The full loop runs on Sepolia: two-sided negotiation, atomic settlement and
bearer-grant disclosure, driven autonomously by two agents through MCP. It is not
production-hardened and has had no external review. Do not put real value through it.

The first live transaction exposed wire-v1 terms because salts are public. Wire v2 replaces
that format with AES-256-GCM-SIV ciphertext and a 128-bit authentication tag, fragmented
across five salt chunks.

Wire v2 has since run end to end on Sepolia. Two fresh identities negotiated and settled
autonomously through two role-bound MCP servers, transaction
`0x14b38e9dbc65f0749be6da2fa05dd2713f8c4c893bac707961c73e616b34cb3`, block 13095252.
`scripts/observer.py` was then pointed at that transaction's public calldata with no channel
key and recovered nothing. The same tool run against the wire-v1 settlement
`0x44289c...84bb7` recovers the full acceptance, which is what makes the negative result
worth anything. Details in [F30](./docs/friction.md).

Still outstanding: a second implementation of wire v2 (`sdk/ts` is on v1), an independent
security review, and traffic-shape privacy ([F31](./docs/friction.md)).

Target is **Starknet Sepolia**, privacy pool v2.0 at `0x0254a6...0d91`, verified on-chain.
A mainnet pool went live at `0x040337b1...812a` with an identical class hash, so porting is a
configuration change rather than a re-derivation, but it charges a 6 STRK protocol fee per
`apply_actions` where Sepolia charges nothing. Nothing has run on mainnet yet.

Where the stack has fought us is logged in [docs/friction.md](./docs/friction.md). The
largest constraint so far is that a pool note has no payload field, so negotiation state is
carried in note salts at 119 bits each.

---

## The problem

Agent-to-agent commerce is arriving on rails that are transparent by default.

- **A2A** (Linux Foundation) moves tasks between agents over HTTPS. TLS protects the payload; it does not hide the communication graph.
- **MCP** (Linux Foundation) exposes tools to agents. No payment, no privacy.
- **x402 / AP2 / MPP** settle payments. All transparent on-chain or through card rails.

All three leave the relationship itself in the open: who is dealing with whom, how often, at what size.

An observer who can see the A2A or MCP communication graph learns a lot from metadata alone. Message timing, message sizes and the identity of the endpoints are enough to classify what kind of workflow is running and to act on a pending transaction before it lands. Encrypting the payload does not close that, because the leak is in the graph rather than in the message.

Procurement teams and trading desks both treat counterparty cadence as confidential, and for the same reason. As agents start transacting autonomously without a human approving each step, a transparent-by-default rail stops being an inconvenience and starts being the blocker.

## What Erebus does

Erebus is **infrastructure, not a platform.** There is no dashboard. Agents are the users. They consume it as tools and SDK calls, the same way they consume any MCP server or A2A skill.

It provides four things existing rails do not:

| Property | What it means |
|---|---|
| **Message privacy: live. Traffic privacy: partial** | Wire v2 encrypts the negotiation payload under AES-256-GCM-SIV, so an observer with no key recovers no terms. The fifth salt still has a fixed 59-bit shape that fingerprints Erebus traffic, so an observer can count and time deals without reading them. See [F31](./docs/friction.md). |
| **Atomic negotiate to settle** | The accepted offer and the shielded payment are one proven state transition. There is no "agreed but never paid" gap and no separate payment hop. |
| **Selective disclosure** | Either party, or a designated auditor, can later reveal the full record (terms and payment) to a specific counterparty, without exposing anything to the public or leaking data about unrelated users. |
| **Agent autonomy** | Starknet account abstraction means an agent is a first-class actor rather than a bolted-on EOA. Gasless operation via a paymaster is possible but not yet verified end-to-end: STRK20 ships no paymaster of its own, so this rides on a third party. |

## Why Starknet, why STRK20

- **Native in-protocol proof verification** (v0.14.2 / SNIP-36) is what made a note-based privacy pool viable here at all.
- **STRK20 is a shared note-based pool, not a mixer.** Shielded assets become encrypted notes; a private transfer spends notes and creates new ones, with a ZK proof that the notes exist, belong to the spender, are unspent, and conserve value.
- **The channel primitive already exists and is audited.** A directional `channel_key` is
  derived from the sender's pool key and both addresses/public identity data, then encrypted
  to the recipient through the pool's channel-info flow. Encrypting Erebus's salt payload is
  still our own wire-design responsibility.
- **Compliance is native.** Viewing keys are registered encrypted on-chain under a threshold auditor system, the path Tornado Cash never had.
- **Account abstraction is native.** An agent can hold and use an account without a human signing each transaction.

## Architecture

See [ARCHITECTURE.md](./ARCHITECTURE.md) for full diagrams, the interface contract, and the data model.

![Erebus system overview, the stack from agent to pool, the three layers, one deal end to
end, how an offer becomes five notes, the note grid, who sees what, and what the system does
and does not claim](./docs/assets/erebus-overview.excalidraw.svg)

*One-page map of the whole system. Open the image full size to read the panels, or
[open it in Excalidraw](https://excalidraw.com/#json=Tgsd4uNGRqrH3wp-E1sfu,AZOvD7vZCyhVtwX6fmibRQ)
to pan and zoom.*

```
Agent A ─┐
         ├─ MCP / A2A tools ─→ Erebus SDK ─→ Channel Layer (Eleusis) ─→ STRK20 Privacy Pool
Agent B ─┘                                          │                          │
                                                    └──→ Discovery Service ←────┘
                                                                 │
                                                    Viewing Key Disclosure (Kleidouchos)
```

## Repo layout

```
/sdk/rs         Rust client: primary implementation, holds all key material
/sdk/ts         TypeScript: differential-test oracle only, ships nothing
/sdk/py         Thin binding over /sdk/rs: no protocol logic
/contracts      Cairo: conformance probes against the upstream pool
/mcp-server     MCP server (Python) exposing Erebus tools to any agent framework
/agents         Reference agents demonstrating the loop (Python)
/docs           Specs, protocol notes, integration guides
```

The call path is `agents → mcp-server → sdk/py → sdk/rs → Starknet`. **Python above the
binding, Rust below it.** Key material never crosses upward, which makes that boundary an
enforced one rather than a convention.

**On the Rust client.** Upstream ships a TypeScript SDK and a Rust `discovery-core` crate.
`discovery-core` covers reads, hashes, storage slots, ECDH, decryption. There is no Rust
write side: nothing builds `ClientAction`s, serialises calldata, signs, or calls the
prover. Erebus's Rust client fills that gap and is useful outside this project. The
TypeScript implementation is kept as the oracle it is differential-tested against. There is
no written spec for the wire format, so two agreeing implementations is the strongest
correctness signal on offer.

**On Python above it.** The official `mcp` SDK is first-class, so the tool layer has no
reason to be TypeScript. `/sdk/py` exists only to reach Rust from Python; it is
not a third client, because a third implementation is a third place for a
wrong hash preimage to hide silently.

## Brand vocabulary

Used in docs, marketing, and conversation. **Not** in the API surface, see [CLAUDE.md](./CLAUDE.md) for the naming policy.

| Term | Meaning |
|---|---|
| **Erebus** | The protocol as a whole |
| **Eleusis** | The intended private channel between two agents; current wire still leaks its payload |
| **Kleidouchos** | A holder of a viewing key (auditor, operator, counterparty) |
| Enter Erebus | Start using the protocol |
| Open an Eleusis | Create a private agent channel |
| Settle in Erebus | Execute the private payment |
| Reveal as Kleidouchos | Use selective disclosure |

## Team

- **Poulav Bhowmick**, protocol, Cairo, Starknet. StarkWare track winner at ETHIndia 2024; Starknet Foundation grantee; Ethereum Protocol Fellow.
- **Ishita**, agents, orchestration, ML. Multi-agent systems, x402 and ERC-8004 agent-payment infrastructure.

## License

Apache-2.0. See [LICENSE](./LICENSE). Matches `starkware-libs/starknet-privacy`, whose
primitives this composes.
