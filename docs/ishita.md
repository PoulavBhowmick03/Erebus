# Tasks — Ishita (agents / orchestration / MCP)

You own everything above the SDK boundary: the agents, the negotiation policy, the MCP server, and the demo.

**Your track is Python** — decided 2026-07-28. Agents, policy engine, and MCP server all
in Python on the official `mcp` SDK. Below you sits `sdk/py`, a thin binding over Poulav's
Rust client; below that, Starknet. You never see Rust.

**You are not blocked on Poulav.** You build against a mock of the interface from hour one and integrate on Day 2. That parallelism is the whole plan — protect it.

Start with [ARCHITECTURE.md](./ARCHITECTURE.md) §4 (the interface you're building against) and §7 (the messaging nuance — important for how we talk about this externally).

---

## What changed under you — read before building the mock

The SDK moved a long way while you were on the agent side. **None of the §4 method
signatures changed**, so your mock is still the right thing to build. What changed is
behaviour the mock has to imitate, and two things that affect what your agents can do.

**Negotiation is on-chain, not off-chain.** The earlier plan sent offers over an encrypted
side channel and only committed the result. That reversed: the payload now rides in the
*salts* of zero-amount notes in the counterparty's subchannel. One message is 4 notes and
one proof. Practical effect on you: **every negotiation round costs ~29 s**, not just the
settlement. A three-round negotiation is ~90 s before settling. Build the mock's latency in
from the start or the agent loop will feel wrong on demo day.

**One subchannel is currently one deal.** Found 2026-07-29. Messages sit on a fixed
4-note grid; a settlement's payment note is 1 note wide, which knocks the grid out of
alignment. So after `acceptAndSettle`, **no further message can be written to that
channel**. Your reference agents must not try to renegotiate or start a second deal in a
settled channel — they open a new one. Mock this: a write after settle should raise, not
succeed. It is an open decision (`poulav.md` P1.3) and may change, so keep it behind one
helper rather than sprinkled through the policy engine.

**`withdrawn` is cut — decided 2026-07-30, do not mock it.** It is out of §4 now. Not
because it was unbuilt, but because it cannot work: notes are write-once, so a retraction has
to be a new message, and that races. You write "I withdraw offer 3"; the counterparty has
already built a settlement against offer 3; their proof applies and you are bound to terms
you tried to pull. Withdrawal only ever works if the counterparty voluntarily checks first,
and not needing them to be well-behaved is the entire point of atomic settlement.

**If your policy engine wants to retract, use a short `deadline` instead.** It gives you the
same capability with no race, because the expiry travels inside the offer and the
counterparty knows it the moment they read it. An agent that would have withdrawn after 30
seconds should offer with a 30-second deadline.

`expired` **is** real and is enforced client-side — the pool has no `deadline` field, so
nothing on-chain stops an agent accepting a stale offer. Your policy engine checking the
deadline is not belt-and-braces, it is the enforcement.

**`memoHash` is 128-bit — resolved 2026-07-30, and simpler than it was.** §4 used to declare
it a `felt252`, which was wrong: the wire has only ever carried 128 bits. So if your agents
hash something into `memoHash`, take the low 128 bits and pass that. A raw SHA-256 or
Keccak digest is 256 bits and does not fit.

Worth knowing *why* you are truncating rather than just doing it: 128 bits is 2^64 collision
resistance. Fine for a commitment to a memo neither side is trying to forge, and the reason
the field is deliberately narrow rather than accidentally so.

**Two MCP servers, not one — decided 2026-07-30.** Each agent runs its own instance with its
own identity and its own prover URL. Not a multi-tenant server with two identities
configured, because that process would hold both agents' pool keys and contradict the custody
claim we have already made to StarkWare. The demo then shows the real topology, which is the
one an actual deployment has.

**Erebus holds no keys, and that shapes your MCP server.** The library runs inside the agent
operator's process, against the operator's own prover — because the proving call carries the
pool private key in the clear. So the MCP server is something an operator runs, not a
service we host. It needs a prover URL and identity in its config, and it should fail loudly
rather than fall back to a shared endpoint. See `docs/custody-design.md`.

**`SettlementErrorCode` is frozen — 2026-07-30.** Full list in ARCHITECTURE §4. It is
grouped by *what you should do*, because an agent cannot sensibly branch on twelve codes:

- **Do not retry, the offer is wrong:** `OFFER_EXPIRED`, `OFFER_UNKNOWN`, `ALREADY_SETTLED`,
  `NOT_YOUR_OFFER`, `AMOUNT_MISMATCH`, `INSUFFICIENT_NOTES`, `INDEX_CONFLICT`
- **Retry may succeed:** `SCREENING_UNAVAILABLE`, `PROVER_UNAVAILABLE`, `PROOF_EXPIRED`,
  `SUBMIT_FAILED`
- **Terminal:** `SCREENING_REJECTED`
- **Opaque:** `PROOF_FAILED` — genuinely so. The prover answers a failed execution with a bare
  JSON-RPC `-32603` carrying no reason at all (F20), so this is not a lazy mapping on our side.

Mock at least one from each group. The retry/no-retry split is the only distinction your
agent logic actually needs to branch on.

**One more that affects your agents directly.** `AMOUNT_MISMATCH` exists because the SDK now
refuses to write a settlement whose acceptance record and payment note disagree. Atomicity
guarantees both legs land, not that they describe the same trade (friction F23). If your
policy engine ever computes the payment separately from the accepted terms, that is where it
will show up.

---

## Day 0 — mock and unblock

### I0.1 — Agree the interface with Poulav *(do this first)*
30 minutes together. Walk ARCHITECTURE.md §4 line by line.

- [ ] Confirm `OfferTerms` has everything your policy engine needs to decide
- [ ] Agree what a failed settlement returns
- [ ] Freeze it

**Acceptance:** you both have the same interface file committed.

### I0.2 — Build the mock
This is the most important thing you do on Day 0. Everything else depends on it.

- [ ] Implement `ErebusClient` as an in-memory mock in Python — same signatures, fake state
- [ ] **`acceptAndSettle` latency: ~30 s.** That is the real number now, not a guess —
      StarkWare publish ~29 s per transaction for Stwo proof generation. Per *transaction*,
      so a message's 4 notes are one proof, but every negotiation round is its own.
- [ ] Mock should be able to simulate failure: expired offer, failed proof, unreachable
      discovery service, and **`SCREENING_REJECTED` / `SCREENING_UNAVAILABLE`** — shielding
      funds needs a screener-signed attestation fresh within 300 s. Deposit leg only.
- [ ] Mock the *binding* surface, not a hypothetical Python client. Whatever shape P0.4
      settles on is what you swap out on integration day.

**Acceptance:** you can call every method on the mock and get sane responses.

**Why the failure cases matter:** your agents need to handle a settlement that doesn't go through. If you only build the happy path, integration day breaks badly.

---

## Day 1 — agents and policy

### I1.1 — Negotiation policy engine
Keep this simple. A threshold rule is enough for the MVP — do not build a sophisticated bargaining strategy.

- [ ] Agent A (buyer): has a budget and a task; proposes, evaluates counters, accepts if within budget, walks away if not
- [ ] Agent B (seller): has a reserve price; evaluates offers, counters once, accepts or declines
- [ ] Enforce a maximum round count so the demo can't loop forever. **This now has a price
      attached:** ~29 s of proving per round, so three rounds is ~90 s of dead air before
      settlement even begins. Decide with Poulav whether the demo runs fewer rounds, is
      time-compressed in the edit, or is honest about the wait.
- [ ] Unit tests on the accept/reject decision

**Acceptance:** given a set of terms, each agent makes a deterministic, testable decision.

### I1.2 — Two reference agents running the loop
- [ ] Two agent instances driving the negotiation against the mock
- [ ] They open a channel, negotiate to agreement, trigger settlement, and one grants a viewing key
- [ ] Structured logging of every state transition — you'll need this for the demo

**Acceptance:** `uv run python agents/demo.py` completes a full negotiation against the mock.

### I1.3 — MCP server *(Python)*
This is what makes it *infrastructure* rather than a demo. It's the difference between "we built a thing" and "any agent framework can use this."

Built on the official `mcp` Python SDK — `MCPServer`, decorator-defined tools, stdio transport
is enough for the demo.

- [ ] MCP server exposing: `open_channel`, `propose_offer`, `counter_offer`, `read_channel_state`, `accept_and_settle`, `grant_viewing_key`, `reveal`
- [ ] Clear tool descriptions — an agent should understand when to call each without reading source
- [ ] Verify from a real MCP client, not just your own agents
- [ ] Tool errors must carry the `SettlementErrorCode` through — a failure that arrives as
      an opaque string makes the whole failure-handling path untestable from the agent side

**Acceptance:** an external agent framework can drive the loop through the MCP server with no knowledge of Erebus internals.

**Why this matters more than it looks:** StarkWare is evaluating whether this is reusable infrastructure. An MCP server that any framework can call *is* the proof. Prioritize this over polishing the agents.

---

## The wire your tools actually call — protocol 2, added 2026-07-31

**Read this before writing a tool handler.** The Rust CLI moved to protocol 2 when the chain
path landed, and `sdk/py/src/erebus/_seam.py` still sends the protocol-1 shapes. They are
incompatible — not subtly, but immediately, on the first call. Build against what is below,
not against what `_seam.py` currently does. Poulav broke this; it is on his side to say so,
which is what this section is.

The source of truth is `sdk/rs/src/bin/erebus_cli.rs:33` and nothing else. If this section
and that enum disagree, the enum is right and this section is stale — tell Poulav.

One JSON object in on stdin, one out on stdout:

```json
{ "method": "propose_offer", "params": { "config": { … }, "handle": "…", "terms": { … } } }
```

**Every method except `version` carries the same `config` block.** The CLI is one-shot and
holds nothing between calls, so operator configuration is re-supplied each time and channel
state is recovered from `state_dir` by handle. All nine fields are required:

```json
"config": {
  "rpc_url":           "…",   "prover_url":     "…",
  "pool_address":      "0x…", "chain_id":       "0x…",
  "account_address":   "0x…", "token":          "0x…",
  "pool_key_file":     "/path/to/pool.key",
  "account_key_file":  "/path/to/account.key",
  "state_dir":         "/path/to/state"
}
```

The two `*_key_file` fields are **paths**. That is the whole custody argument for this seam:
Rust opens them, and no pool or account private key ever enters your Python heap, a request
body, or argv. If you ever find yourself reading one of those files in Python to pass its
contents, stop — that is the architecture violation the guardrails mean.

| method | params beyond `config` |
|---|---|
| `version` | *(none, and no `config` — safe startup probe)* |
| `open_channel` | `counterparty` |
| `propose_offer` | `handle`, `terms` |
| `counter_offer` | `handle`, `reply_to`, `terms` |
| `read_channel_state` | `handle` |
| `accept_and_settle` | `handle`, `offer_id` |
| `grant_viewing_key` | `handle`, `grantee` |
| `reveal` | `viewing_key` *(no handle — see below)* |
| `shield` | `amount` — administrative funding, not part of the negotiation loop |

`terms` is `{ "amount": "…", "token": "0x…", "deadline": <number>, "memo_hash": "…" }`.
`amount` and `memo_hash` take decimal or `0x` hex; `token` is hex only. `deadline` is a JSON
number, not a string.

**Four things that will bite if nobody says them out loud:**

1. **Unknown fields are a hard error, not ignored** (`deny_unknown_fields`). A leftover
   protocol-1 field like `counterparty_public_key` or `register` fails the whole call with
   `INVALID_REQUEST`. This is deliberate and it is in your favour — a shape mismatch is the
   one class of bug in this stack that gets to be loud, so let it be loud rather than
   filtering fields to make it pass.
2. **Handles are opaque and mean nothing.** A handle is a random value naming a record under
   `state_dir`; it is not a key, an address, or anything derivable. Store it, pass it back,
   never parse it. Two processes sharing a `state_dir` share channels — that is how a
   one-shot CLI keeps a multi-step negotiation.
3. **`reveal` takes no handle.** It takes the entire object `grant_viewing_key` returned, and
   that object *is* the secret — anyone holding it can read that one relationship. Pass it
   through verbatim; do not construct one, do not log it, do not put it in a tool result an
   agent will echo into a model context. This is what lets an auditor reveal on a different
   machine with no access to our `state_dir`, which is the whole point of the compliance
   story in the demo.
4. **`open_channel` no longer takes a counterparty public key or channel indices.** Just the
   counterparty. If your mock has those parameters, drop them.

Errors arrive as `{"ok": false, "error": {"code", "message", "retryable"}}` exactly as
before — that part of the contract did not move, and `retryable` is still the only field
worth branching on.

---

## Day 2 — integrate and demo

### I2.1 — Swap mock for real
- [ ] Bring `sdk/py/src/erebus/_seam.py` up to protocol 2 — it still sends the protocol-1
      `open_channel` and knows none of the other six methods. **Shared file: agree the change
      with Poulav first** (CLAUDE.md), and keep it transport-only — new methods are new
      argument-forwarding functions, never new logic.
- [ ] Point the agents at the real seam instead of the mock
- [ ] Handle real proof latency in the agent loop — ~29 s per proof, and settlement is more
      than one

**Do not let this be the first time the two languages meet.** P0.4 already got one method
across with a stub underneath, so the *mechanism* is proven — but protocol 2 then changed the
shapes underneath it, so the surface is not. The gap is exactly the wire contract above:
mechanical, but it is nobody's by default, which is how it survives to demo day.

**Acceptance:** one green end-to-end run on testnet.

### I2.2 — Record the demo
2–3 minutes. Not longer. StarkWare asked for validation, not a product launch.

- [ ] Show the two agents negotiating autonomously
- [ ] Show that an observer sees nothing on-chain
- [ ] Show the atomic settlement
- [ ] End on the viewing-key reveal — this is the compliance story and it's what makes them care
- [ ] No music, no logo animation, no polish. Screen recording with narration is fine.

**Acceptance:** a link you can send.

### I2.3 — Write the one-pager
- [ ] What we built
- [ ] What fought us (pull from `docs/friction.md`)
- [ ] One sentence on what we'd ship next

**Acceptance:** fits on one page. Resist expanding it.

---

## Guardrails

- Do not build a frontend or dashboard. The MCP server is the interface.
- Do not build a sophisticated negotiation strategy. Threshold rules only.
- Do not let agent-layer code touch key material — that's an architecture violation (CLAUDE.md).
- Do not add a third agent or multi-party negotiation.
- Do not change the interface without Poulav.
- Do not wait on Poulav for anything. If you're blocked, extend the mock.
- **Do not put protocol logic in `/sdk/py`.** It is a binding, not a client. If you find
  yourself writing a hash, a felt conversion, or a salt encoder there, stop — that
  belongs in Rust, and a second copy of it is a second place for a silent mismatch.

---

## Reading

You're coming to Starknet fresh. This ramps you without requiring Cairo.

1. **[ARCHITECTURE.md](./ARCHITECTURE.md) §1, §2, §4, §7** — start here. Fastest path to being on the same page as Poulav.
2. **"The Controversy of On-Chain Privacy: Monero, Tornado Cash, and STRK20"** — https://dev.to/okolievans/the-controversy-of-on-chain-privacy-monero-tornado-cash-and-strk20-2i7n. Plain language, no Cairo. Read this for the vocabulary: notes, nullifiers, channels, subchannels, viewing keys.
3. **STRK20 launch post** — https://www.starknet.io/blog/make-all-erc-20-tokens-private-with-strk20/. Product framing and use cases.
4. **STRK20 by Example** — https://strk20-by-example.org. Skim the Wallet API and SDK sections. You need the shape of the interface, not the contract internals.
5. **Starknet account abstraction docs** — https://docs.starknet.io. Specifically session keys and paymasters. This is *why* agents work well here: no gas balance, no seed phrase management.
6. **MCP Python SDK** — https://pypi.org/project/mcp/ (`pip install "mcp[cli]"`, or `uv add "mcp[cli]"`). **`FastMCP` was removed in `mcp` 2.0** — the class is `mcp.server.MCPServer` and it keeps the same `@server.tool()` decorator style. Verified against 2.0.0; anything you read online about FastMCP is `mcp<2`. Stdio transport is sufficient for the demo.

**What already transfers:** the offer/counter/accept state machine is an agent decision loop, which is your existing work. The x402 and ERC-8004 patterns you've built are the same shape — autonomous agents transacting under rules. The only genuinely new thing is the settlement rail.

**What does not transfer — checked 2026-07-28, so nobody plans around it:** the *code*.
ERC-8004 is Draft and EVM-only (`eip155` namespace), with no Starknet form at all. x402 has
an official Python SDK, but its mechanisms are EVM/Solana/TON — x402-on-Starknet exists
only as `NethermindEth/x402-starknet`, which is TypeScript. So there is no Python x402 path
to Starknet, and the TypeScript one solves paying for HTTP APIs rather than private
settlement. Pattern transfers; libraries don't. *(If any of your prior x402 work is
actually on Starknet, say so — that would be TypeScript and it changes this.)*