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
security review, relationship privacy at channel-open ([F38](./docs/friction.md)), and
traffic-shape privacy ([F31](./docs/friction.md)).

Target is **Starknet Sepolia**, privacy pool v2.0 at `0x0254a6...0d91`, verified on-chain.
A mainnet pool went live at `0x040337b1...812a` with an identical class hash, so porting is a
configuration change rather than a re-derivation. Both charge a protocol fee per
`apply_actions`, pulled with `transfer_from` against the caller: 2 STRK on Sepolia, 6 on
mainnet. A standing allowance is therefore a precondition for every write on either network.
Nothing has run on mainnet yet.

**`v0.1.0` is released** — installable from a public index, with wheels for Linux x86-64
and macOS arm64, checksums, and an SBOM. See [Install](#install).

Where the stack has fought us is logged in [docs/friction.md](./docs/friction.md). The
largest constraint so far is that a pool note has no payload field, so negotiation state is
carried in note salts at 119 bits each.

[docs/status.md](./docs/status.md) is the one-page current state, and the tiebreaker when
any two documents here disagree.

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
| **Message privacy: live. Relationship privacy: partial** | Wire v2 encrypts the negotiation payload under AES-256-GCM-SIV, so an observer with no key recovers no terms. Two things still leak: opening a channel writes the counterparty's address to public calldata ([F38](./docs/friction.md)), and the fifth salt has a fixed shape that fingerprints Erebus traffic, so an observer can count and time deals without reading them ([F31](./docs/friction.md)). See [privacy-model.md](./docs/privacy-model.md) for the full boundary. |
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

---

# Documentation

## Install

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

See [The CLI protocol](#the-cli-protocol) below.
</details>

Every release carries `SHA256SUMS` and a CycloneDX `sbom.json`. Verify before running:

```bash
curl -LO https://github.com/PoulavBhowmick03/Erebus/releases/latest/download/SHA256SUMS
sha256sum -c SHA256SUMS --ignore-missing
```

> If a download times out, the release assets are served by a CDN that can be slow from
> some networks. `UV_HTTP_TIMEOUT=600` raises uv's default 30-second limit.

## Set up an identity

An Erebus identity is a Starknet account plus two key files, registered with the pool and
holding shielded notes. Getting there takes six on-chain steps, and one script does all of
them:

```bash
scripts/new-identity.sh bootstrap erebus-a ~/.erebus-a <funder-account>
```

That runs: create the account → fund it → deploy → generate the pool key and extract the
account key → write an env file → approve the pool for the live per-write fee → wait for
the approval to reach proving depth → shield 1 STRK (which also registers the identity) →
`doctor`. It exits non-zero if `doctor` is not ready.

Without a funded account to pay from, use the faucet flow instead — `create`, fund the
printed address by hand, then `activate`. Both are documented in the script's header and in
[docs/runbook.md](./docs/runbook.md).

> **Registration is irreversible and writes the identity's pool private key encrypted to
> the pool's auditor on-chain.** From that moment the auditor can decrypt everything that
> identity ever does. Use testnet keys only.

**Three keys, and conflating them is the usual mistake:**

| Key | Purpose | Who sees it |
|---|---|---|
| Starknet account key | Signs transactions. Custody | Never leaves the Rust process |
| Pool private key | The STRK20 identity. Confidentiality | Sent in `compile_actions` calldata to your prover and preflight RPC — both must be operator-controlled |
| Pool auditor key | Pool-wide, set once at registration | StarkWare's, no rotation |

Python never sees key material, only file paths. See
[docs/custody-design.md](./docs/custody-design.md).

## Configure

The server reads its configuration from the environment and fails at startup naming
whatever is missing.

**Always required:**

| Variable | Meaning |
|---|---|
| `AGENT_ADDRESS` | This identity's Starknet account address |
| `PROVING_SERVICE_URL` | Your prover. It receives the pool private key, so it must be one you control |
| `EREBUS_SETTLEMENT_ROLE` | `payer`, `payee`, or `both`. A payee server structurally refuses `accept_and_settle` |

**Backend selection:** `EREBUS_BACKEND` is `mock` (default — no chain, no keys, no gas) or
`seam` (the real Rust client). `seam` additionally requires:

| Variable | Meaning |
|---|---|
| `STARKNET_RPC_URL` | Preflight RPC. Also receives the pool key — operator-controlled |
| `POOL_ADDRESS` | The STRK20 privacy pool |
| `STARKNET_CHAIN_ID` | e.g. `0x534e5f5345504f4c4941` for Sepolia. Part of every channel-key preimage, so a mismatch reads as "not found" everywhere |
| `TOKEN_ADDRESS` | The ERC-20 being settled |
| `POOL_KEY_FILE`, `ACCOUNT_KEY_FILE` | Paths, mode `0600`. Never read by Python |
| `EREBUS_STATE_DIR` | Channel state, mode `0700` |

**Optional:** `EREBUS_CLI` (explicit binary path; defaults to the packaged one),
`EREBUS_SKIP_STARTUP_DOCTOR=1` (skip the boot-time inspection when starting offline), and
the `EREBUS_MOCK_*` knobs for mock runs.

`scripts/new-identity.sh` writes a complete env file. Start the server with:

```bash
set -a && . ~/.erebus-a/env && set +a
EREBUS_BACKEND=seam EREBUS_SETTLEMENT_ROLE=payer erebus-mcp-server
```

## Check before you spend

Every setup fault in this stack surfaces the same way: `apply_actions` reverts with a bare
`Contract error` naming nothing, **after** a proof has been generated and paid for. `doctor`
answers those questions first, read-only, in one pass:

```bash
erebus-cli doctor   # or the `doctor` MCP tool, or automatically at server startup
```

Ten checks in dependency order — both key files and their modes, the state directory, RPC,
prover, chain id read from the node and compared with config, pool identity and version,
registration against the key file, allowance against the live fee, and public balance.
Every non-passing check carries a `repair` string naming one direct action.

`ready: false` means a write will fail right now. A `skipped` check is not a pass — it
means the thing it would have verified is unverified.

## Quickstart: two agents, one deal

Two servers, one per identity, one configured `payer` and one `payee`. The payer spends its
own notes; the payee never calls `accept_and_settle`.

```python
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
        opened = await payer.call_tool("open_channel", {"counterparty": seller_address})
        handle = opened.structured_content["result"]["channel_handle"]

        # 4. Wait for the seller's ask. One tool call, not a poll loop.
        waited = await payer.call_tool("wait_for_offers",
                                       {"channel_handle": handle, "expected_count": 1})
        offer = waited.structured_content["result"]["offers"][0]

        # 5. Settle atomically: the accepted offer and the shielded payment
        #    are one proven state transition. Takes 1-4 minutes; do not abort it.
        receipt = await payer.call_tool(
            "accept_and_settle",
            {"channel_handle": handle, "offer_id": offer["offer_id"]})

        # 6. Let a third party reconstruct the record. The returned viewing_key
        #    is a bearer secret — deliver it out of band, never log it.
        await payer.call_tool("grant_viewing_key",
                              {"channel_handle": handle, "grantee": auditor_address})
```

A runnable two-sided version is in
[`agents/src/erebus_agents/mcp_loop.py`](./agents/src/erebus_agents/mcp_loop.py); the
deterministic mock rehearsal is `uv run python agents/src/erebus_agents/demo.py`.

A worked example against Sepolia, with real transaction hashes, timings and the failures
along the way, is in [docs/runs/](./docs/runs/).

## The MCP tool surface

Ten tools. Amounts are decimal strings and `memo_hash` is a hex string — a JSON number
loses precision above 2^53, and 1 STRK is 1e18.

| Tool | Signature | Notes |
|---|---|---|
| `doctor` | `()` | Read-only. Always safe to call |
| `get_note_balance` | `()` | Payer must call before naming a price |
| `open_channel` | `(counterparty)` | Returns `channel_handle`. Not private — see F38 |
| `propose_offer` | `(channel_handle, amount, token, deadline, memo_hash)` | Payee asks; payer offers |
| `counter_offer` | `(channel_handle, reply_to, amount, token, deadline, memo_hash)` | Does not withdraw the offer it replies to |
| `read_channel_state` | `(channel_handle)` | Every visible offer plus the settlement |
| `wait_for_offers` | `(channel_handle, expected_count, timeout_seconds=300)` | One tool call instead of a poll loop. A timeout is not an error |
| `accept_and_settle` | `(channel_handle, offer_id)` | **Payer only.** Spends the caller's notes. Closes the channel |
| `grant_viewing_key` | `(channel_handle, grantee)` | Returns a bearer secret. Deliver out of band |
| `reveal` | `(channel_id, grantee, viewing_key)` | Reconstructs from chain data. Needs no local state |

Every result is an envelope: `{"ok": true, "result": {...}}` or
`{"ok": false, "error": {"code", "message", "retryable"}}`.

**Two protocol rules that surprise people.** One channel per pair of addresses, and one deal
per channel — a settled channel is terminal. And an offer has no `withdrawn` state; it is
accepted or it expires, so a short deadline is the only way to bound how long a stale price
stays acceptable.

## Errors and retries

Branch on the group, not the individual code. Every error also carries `retryable` — trust
it over guessing from the name.

| Group | Codes | What to do |
|---|---|---|
| The offer is wrong | `OFFER_EXPIRED`, `OFFER_UNKNOWN`, `ALREADY_SETTLED`, `NOT_YOUR_OFFER`, `AMOUNT_MISMATCH`, `INSUFFICIENT_NOTES`, `INDEX_CONFLICT` | Build a different offer. Retrying verbatim will not help |
| Transient | `SCREENING_UNAVAILABLE`, `PROVER_UNAVAILABLE`, `PROOF_EXPIRED`, `SUBMIT_FAILED` | Retry with backoff. `PROOF_EXPIRED` needs a fresh proof, not a resend |
| Terminal | `SCREENING_REJECTED` | Stop. Not transient |
| Opaque | `PROOF_FAILED` | The prover refused and gave no reason. Report it as unexplained |
| Before any protocol code ran | `INVALID_REQUEST`, `IDENTITY_UNAVAILABLE` | Fix the request or the key path. Never a chain-state problem |

A write takes 1–4 minutes: simulate, prove, estimate, submit. The binary prints stage names
to stderr as it goes. **Do not abort and retry a write that appears stuck** — abandoning it
does not cancel a transaction it may already have submitted.

## The CLI protocol

`erebus-cli` reads one JSON request on stdin and writes one envelope on stdout. Key *paths*
cross the boundary; key values never do.

```bash
echo '{"method":"doctor","params":{"config":{...}}}' | erebus-cli
```

```json
{"ok": true, "protocol": 2, "result": {"ready": true, "checks": [...]}}
```

`protocol` is the contract version. A consumer should refuse a mismatch by name rather than
failing on a changed shape later — `erebus-sdk` does this on every call, and the MCP server
handshakes at startup.

Methods: `version`, `generate_pool_key`, `doctor`, `balance`, `allowance`, `approve`,
`shield`, `open_channel`, `propose_offer`, `counter_offer`, `read_channel_state`,
`accept_and_settle`, `grant_viewing_key`, `reveal`. All except `version` and
`generate_pool_key` take a `config` object.

From Python, `erebus-sdk` wraps this:

```python
from erebus import Seam, SeamConfig
seam = Seam(config=SeamConfig(rpc_url=..., prover_url=..., ...))
report = seam.doctor()
```

`/sdk/py` is a binding, not a client: it marshals arguments and returns results, and
contains no hashing, salt encoding, or felt arithmetic. A second implementation would be a
second place for a wrong preimage to hide silently.

## Operating it with an agent

[`skills/erebus/`](./skills/erebus/) is an agent skill covering install, plan, operate, and
diagnose modes, with the safety rules that matter: never read a key file, a payee never
settles, never report a mock result as on-chain evidence, and never claim privacy without
naming both halves of it. Its
[unsafe-behavior evals](./skills/erebus/evals/unsafe-behavior.md) are the fixtures it must
pass before it is trusted.

## Building from source

```bash
git clone https://github.com/PoulavBhowmick03/Erebus && cd Erebus
cd sdk/rs && cargo test && cd ../..     # 216 tests
uv sync --all-packages && uv run pytest # 70 tests
```

`uv sync` without `--all-packages` skips the workspace members' editable installs and the
`erebus-*` packages will not be importable.

The TypeScript SDK is a differential-test oracle and ships nothing; it needs a sibling
checkout of `starkware-libs/starknet-privacy` (see [docs/friction.md](./docs/friction.md)
F8). Toolchain: scarb 2.17.0 / starknet-foundry 0.59.0, Node 20+, Rust stable.

---
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
