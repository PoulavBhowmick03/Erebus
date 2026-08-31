# Erebus

**Negotiate in darkness, settle in silence.**

Erebus is experimental coordination and shielded-settlement infrastructure for AI agents.

[Open the public demo](https://erebus-private-agents.vercel.app). It runs the reference-agent
flow in the browser and links to the available on-chain evidence. The browser run is a
simulation and does not ask for a wallet. The
[three-minute evidence video](https://erebus-private-agents.vercel.app/erebus-private-sprint.mp4)
records the sprint state before the later full mainnet canary.

Two agents open an **Eleusis**, an encrypted channel carried in privacy-pool note salts,
exchange structured offers over it, and settle atomically through the shielded pool.
Historical wire-v2 channels can export a whole-channel viewing key. Wire v3 replaces that
with a grant scoped to one deal and one named recipient.

---

## Status

**Unaudited and experimental.** The full loop — two agents negotiating, settling atomically,
and disclosing to a third party — runs on Starknet Sepolia at wire v3. One bounded mainnet
canary also completed a Starkscan-screened 1 STRK shield, MCP proposal and counter, atomic
0.8 STRK settlement with 0.2 STRK change, reconciliation, observer test, and scoped
disclosure. See the [mainnet run](./docs/runs/2026-08-31-mainnet-starkscan-workflow.md).
It has had no external security review. Do not put value you care about through it.

**Erebus hides the terms, not the relationship.** Negotiation content and settlement amounts
are confidential, and that is demonstrated rather than asserted: an observer with no key
recovers nothing from a settlement. New source-built channels default to wire v3, which
removes v2's fixed salt-shape classifier: against live wire-v3 transactions that
classifier scores 0.5000 balanced accuracy, which is chance. That a channel was opened, and
with whom, is still public.
[privacy-model.md](./docs/privacy-model.md) is the full boundary and the only
source to quote for privacy claims.

`v0.1.0` is released and installable. It speaks Protocol 2 and exposes ten MCP tools.
Current `main` speaks Protocol 4 and exposes thirteen tools. Protocol 4 will ship in
`v0.2.0` after the operator-alpha gates pass.
[docs/status.md](./docs/status.md) is the current state in one page, and the tiebreaker when
any two documents here disagree.

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
| **Message privacy: live. Relationship privacy: partial** | Wire v3 encrypts authenticated deal records under AES-256-GCM-SIV. Measured against live Sepolia transactions, it removes wire v2's fixed fifth-salt classifier. Opening a channel still writes the counterparty address to public calldata ([F38](./docs/friction.md)). See [privacy-model.md](./docs/privacy-model.md). |
| **Atomic negotiate to settle** | The accepted offer and the shielded payment are one proven state transition. There is no "agreed but never paid" gap and no separate payment hop. |
| **Selective disclosure** | Wire v3 encrypts one deal's directional subkeys and exact note capabilities to a registered recipient, with an explicit expiry. It exports no parent channel key and grants no spending authority. Historical wire-v1/v2 grants remain broader bearer secrets. |
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

![Erebus system overview, the stack from agent to pool, the three layers, a deal end to
end, how an offer becomes five notes, the note frames, who sees what, and what the system does
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

---

## Install

> **Release boundary:** The command below installs `v0.1.0`. Build the current source when
> you need Protocol 4 operation IDs, reconciliation, resume, or state rebuild. Do not use
> the Protocol 4 quickstart below with the published Protocol 2 binary.

**Requirements.** Linux x86-64 or macOS arm64, Python 3.11+. Intel macOS is not built — its
CI runner is no longer available, and a cross-build would ship a binary that was never
executed on its own architecture.

Releases live on GitHub with a static [PEP 503 index](https://poulavbhowmick03.github.io/Erebus/simple/)
on GitHub Pages. GitHub has no Python registry, so the index is what makes the wheels
resolvable rather than merely downloadable.

```bash
uv tool install \
  --extra-index-url https://poulavbhowmick03.github.io/Erebus/simple \
  erebus-mcp-server
```

`--extra-index-url` rather than `--index-url`: PyPI still serves `mcp` and its dependencies,
and only the three `erebus-*` packages come from this index.

That pulls in three packages — `erebus-mcp-server` (the tool layer), `erebus-sdk` (the
Python binding), and `erebus-cli` (the Rust binary, shipped as a platform wheel). No Rust
toolchain is needed.

<details>
<summary>Installing into a virtualenv instead</summary>

```bash
uv venv && uv pip install \
  --extra-index-url https://poulavbhowmick03.github.io/Erebus/simple \
  erebus-mcp-server
```

This puts both `erebus-mcp-server` and `erebus-cli` on the environment's `PATH`.
`uv tool install` exposes only `erebus-mcp-server`; the server locates the binary inside
its own environment either way.
</details>

<details>
<summary>Driving the binary from another language</summary>

`erebus-cli` speaks one JSON request on stdin and one envelope on stdout, so anything that
can spawn a process can drive it. Take it from the release directly rather than installing
a Python package to extract it:

```bash
curl -LO https://github.com/PoulavBhowmick03/Erebus/releases/latest/download/erebus-cli-x86_64-unknown-linux-gnu
chmod +x erebus-cli-x86_64-unknown-linux-gnu
echo '{"method":"version"}' | ./erebus-cli-x86_64-unknown-linux-gnu
```

See [the CLI protocol](./docs/reference.md#the-cli-protocol).
</details>

Every release carries `SHA256SUMS` and a CycloneDX `sbom.json`. Verify before running:

```bash
curl -LO https://github.com/PoulavBhowmick03/Erebus/releases/latest/download/SHA256SUMS
sha256sum -c SHA256SUMS --ignore-missing
```

> If a download times out, the release assets are served by a CDN that can be slow from
> some networks. `UV_HTTP_TIMEOUT=600` raises uv's default 30-second limit.

### Next: an identity

Installing gets you the binary and the server. Running against a chain also needs a Starknet
account, two key files, a pool allowance, and shielded notes. One script does all of it:

```bash
scripts/new-identity.sh bootstrap erebus-a ~/.erebus-a <funder-account>
```

It ends by running `erebus-cli doctor` and exits non-zero if anything is unready. The
faucet flow, the three keys and who sees them, and every environment variable are in the
[reference](./docs/reference.md#set-up-an-identity).

To try the tools without a chain, keys, or gas, set `EREBUS_BACKEND=mock` and skip all of
that.

## Current-source quickstart: Protocol 4

Build and install the current checkout before you use this example. The public `v0.1.0`
wheel does not support these operation-ID and recovery calls.

Two servers, one per identity, one configured `payer` and one `payee`. The payer spends its
own notes; the payee never calls `accept_and_settle`. `base_env` below is the identity's env
file — the one `new-identity.sh` writes — loaded into the environment.

```python
import secrets

from mcp import ClientSession
from mcp.client.stdio import StdioServerParameters, stdio_client

params = StdioServerParameters(command="erebus-mcp-server", args=[], env={
    **base_env, "EREBUS_BACKEND": "seam", "EREBUS_SETTLEMENT_ROLE": "payer",
})

async with stdio_client(params) as (read, write):
    async with ClientSession(read, write) as payer:
        await payer.initialize()

        # 1. Health first. Every write costs a proof; find faults before paying for one.
        await payer.call_tool("doctor", {})

        # 2. Know what you can pay. Any 0 < amount <= total is payable —
        #    settlement returns the excess as a change note.
        await payer.call_tool("get_note_balance", {})

        # 3. Open the channel. NOT private: this writes the counterparty's
        #    address to public calldata (F38).
        # Persist each ID with its canonical intent before the call. Reuse the same ID
        # after a restart. This abbreviated example only shows ID generation.
        open_id = "op_" + secrets.token_hex(32)
        opened = await payer.call_tool(
            "open_channel", {"operation_id": open_id, "counterparty": seller_address})
        handle = opened.structured_content["result"]["channel_handle"]

        # 4. Wait for the seller's ask. One tool call, not a poll loop.
        waited = await payer.call_tool("wait_for_offers",
                                       {"channel_handle": handle, "expected_count": 1})
        offer = waited.structured_content["result"]["offers"][0]

        # 5. Settle atomically: the accepted offer and the shielded payment
        #    are one proven state transition. Takes 1-4 minutes; do not abort it.
        receipt = await payer.call_tool(
            "accept_and_settle",
            {"operation_id": "op_" + secrets.token_hex(32),
             "channel_handle": handle, "offer_id": offer["offer_id"]})

        # Optional operator step: export this deal to a registered auditor. The MCP tool
        # writes the encrypted capsule to a new mode-0600 file and returns only its path.
        await payer.call_tool(
            "grant_viewing_key",
            {"operation_id": "op_" + secrets.token_hex(32),
             "channel_handle": handle, "deal_id": offer["deal_id"],
             "grantee": auditor_address, "expires_at": grant_expiry,
             "output_path": "/secure/path/deal.grant.json"})
```

A runnable two-sided version is in
[`agents/src/erebus_agents/mcp_loop.py`](./agents/src/erebus_agents/mcp_loop.py); the
deterministic mock rehearsal is `uv run python agents/src/erebus_agents/demo.py`.

For driving the same loop from a mainstream agent framework instead of `erebus_agents`, see
[`agents/examples/openai-agents-quickstart/`](./agents/examples/openai-agents-quickstart/):
two GPT-backed agents negotiating and settling entirely through the OpenAI Agents SDK's own
MCP tool-calling loop, installed from published wheels rather than this checkout.

Worked examples against mainnet and Sepolia, with real transaction hashes, timings, and
failure and recovery evidence, are in [docs/runs/](./docs/runs/).

Configuration, the full tool surface, error handling, and the raw CLI protocol are in the
[reference](./docs/reference.md).

## Documentation

| | |
|---|---|
| [Reference](./docs/reference.md) | Identity setup, configuration, the thirteen MCP tools, recovery, and CLI protocol 4 |
| [Runbook](./docs/runbook.md) | Reproduce the on-chain demonstration step by step |
| [Architecture](./ARCHITECTURE.md) | Component boundaries, the interface contract, the data model |
| [Privacy model](./docs/privacy-model.md) | What leaks and what does not. The only source for privacy claims |
| [Status](./docs/status.md) | Current state in one page; the tiebreaker between documents |
| [Friction log](./docs/friction.md) | Where the stack fought us, and how we worked around it |
| [Agent skill](./skills/erebus/) | Operating Erebus from an agent, with unsafe-behavior evals |
| [Run evidence](./docs/runs/) | Real mainnet and Sepolia runs with transaction hashes and timings |

## Repo layout

```
/sdk/rs         Rust client: primary implementation, holds all key material
/sdk/ts         TypeScript: differential-test oracle only, ships nothing
/sdk/py         Thin binding over /sdk/rs: no protocol logic
/contracts      Cairo: conformance probes against the upstream pool
/mcp-server     MCP server (Python) exposing Erebus tools to any agent framework
/agents         Reference agents demonstrating the loop (Python)
/skills         Agent skill for operating Erebus, with unsafe-behavior evals
/packaging      Platform wheel that ships the erebus-cli binary
/docs           Specs, protocol notes, integration guides, friction log
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
| **Eleusis** | The private channel between two agents; opening one still exposes the counterparty's address on-chain (F38) |
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
