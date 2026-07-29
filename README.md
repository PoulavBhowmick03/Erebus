# Erebus

**Negotiate in darkness. Settle in silence.**

Erebus is the private coordination and settlement layer for AI agents.

Agents open an **Eleusis** — an encrypted channel that hides not just the content of a negotiation but the fact that the relationship exists at all. They agree terms inside it. Settlement happens atomically through the STRK20 privacy pool on Starknet. Afterwards, only a designated **Kleidouchos** (key-bearer) can unlock the record.

---

## Status

**Pre-MVP.** This repo is being built to validate the core loop end-to-end on Starknet testnet. Nothing here is production-ready. Do not put real value through it.

Current milestone: prove `open channel → structured negotiation → atomic shielded settlement → selective disclosure` works on live STRK20 primitives.

Target is **Starknet Sepolia** — privacy pool v2.0 at `0x0254a6…0d91`, verified on-chain.
Mainnet has no STRK20 deployment yet. Where the stack has fought us is logged honestly in
[docs/friction.md](./docs/friction.md); the largest constraint so far is that a pool note
has no payload field, so negotiation state is carried in note salts at 119 bits each.

---

## The problem

Agent-to-agent commerce is arriving on rails that are transparent by default.

- **A2A** (Linux Foundation) moves tasks between agents over HTTPS. TLS protects the payload; it does not hide the communication graph.
- **MCP** (Linux Foundation) exposes tools to agents. No payment, no privacy.
- **x402 / AP2 / MPP** settle payments. All transparent on-chain or through card rails.

None of them hide *who is dealing with whom, how often, and for how much.*

That gap is not theoretical. Recent work on agent interoperability metadata shows that an observer of the A2A/MCP communication graph can recover an interaction's task class well above chance from metadata alone — from the opening of a workflow — and then front-run the pending action at machine speed. Encrypting the payload does not fix this. The leak is the graph, not the message.

No business runs procurement in public. No trading desk wants its counterparty cadence observable. As agents start transacting autonomously, the transparent-by-default rail becomes the blocker.

## What Erebus does

Erebus is **infrastructure, not a platform.** There is no dashboard. Agents are the users. They consume it as tools and SDK calls, the same way they consume any MCP server or A2A skill.

It provides four things existing rails do not:

| Property | What it means |
|---|---|
| **Relationship privacy** | The channel's existence, participants, and cadence are hidden — not just the message contents. Notes live at storage locations derived from a secret shared only between the two parties. |
| **Atomic negotiate → settle** | The accepted offer and the shielded payment are one proven state transition. No "agreed but never paid" gap, no separate payment hop. |
| **Selective disclosure** | Either party, or a designated auditor, can later reveal the full record — terms and payment — to a specific counterparty without exposing anything to the public or leaking data about unrelated users. |
| **Agent autonomy** | Starknet account abstraction means an agent is a first-class actor rather than a bolted-on EOA. Gasless operation via a paymaster is possible but not yet verified end-to-end — STRK20 ships no paymaster of its own, so this rides on a third party. |

## Why Starknet, why STRK20

- **Native in-protocol proof verification** (v0.14.2 / SNIP-36) is what made a note-based privacy pool viable here at all.
- **STRK20 is a shared note-based pool, not a mixer.** Shielded assets become encrypted notes; a private transfer spends notes and creates new ones, with a ZK proof that the notes exist, belong to the spender, are unspent, and conserve value.
- **The channel primitive already exists and is audited.** A per-pair `channel_key` is derived from both parties' addresses and viewing keys via ECDH over the Stark curve, splitting into subchannels that carry notes. We are composing audited primitives, not inventing cryptography.
- **Compliance is native.** Viewing keys are registered encrypted on-chain under a threshold auditor system — the path Tornado Cash never had.
- **Account abstraction is native**, which is what makes agents first-class actors rather than bolted-on EOAs.

## Architecture

See [ARCHITECTURE.md](./ARCHITECTURE.md) for full diagrams, the interface contract, and the data model.

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
/sdk/rs         Rust client — primary implementation, holds all key material
/sdk/ts         TypeScript — differential-test oracle only, ships nothing
/sdk/py         Thin binding over /sdk/rs — no protocol logic
/contracts      Cairo — conformance probes against the upstream pool
/mcp-server     MCP server (Python) exposing Erebus tools to any agent framework
/agents         Reference agents demonstrating the loop (Python)
/docs           Specs, protocol notes, integration guides
```

The call path is `agents → mcp-server → sdk/py → sdk/rs → Starknet` — **Python above the
binding, Rust below it.** Key material never crosses upward, which makes that boundary an
enforced one rather than a convention.

**On the Rust client.** Upstream ships a TypeScript SDK and a Rust `discovery-core` crate.
`discovery-core` covers reads — hashes, storage slots, ECDH, decryption. There is no Rust
write side: nothing builds `ClientAction`s, serialises calldata, signs, or calls the
prover. Erebus's Rust client fills that gap and is useful outside this project. The
TypeScript implementation is kept as the oracle it is differential-tested against —
there's no written spec for the wire format, so two agreeing implementations is the
strongest correctness signal on offer.

**On Python above it.** The official `mcp` SDK is first-class, so the tool layer has no
reason to be TypeScript. `/sdk/py` exists only to reach Rust from Python; it is
deliberately not a third client, because a third implementation is a third place for a
wrong hash preimage to hide silently.

## Brand vocabulary

Used in docs, marketing, and conversation. **Not** in the API surface — see [CLAUDE.md](./CLAUDE.md) for the naming policy.

| Term | Meaning |
|---|---|
| **Erebus** | The protocol as a whole |
| **Eleusis** | A private channel between two agents |
| **Kleidouchos** | A holder of a viewing key (auditor, operator, counterparty) |
| Enter Erebus | Start using the protocol |
| Open an Eleusis | Create a private agent channel |
| Settle in Erebus | Execute the private payment |
| Reveal as Kleidouchos | Use selective disclosure |

## Team

- **Poulav Bhowmick** — protocol, Cairo, Starknet. StarkWare track winner at ETHIndia 2024; Starknet Foundation grantee; Ethereum Protocol Fellow.
- **Ishita** — agents, orchestration, ML. Multi-agent systems, x402 and ERC-8004 agent-payment infrastructure.

## License

TBD before any public release.