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

**`withdrawn` does not exist — do not mock it.** §4's data model lists `withdrawn` as an
`OfferStatus` and draws a `proposed --> withdrawn` transition, but nothing can reach it:
`ErebusClient` has no `withdrawOffer` and the wire format has only `Offer | Counter |
Accept`. Leave it out of your `OfferStatus` enum until P0.3 settles it. If your policy
engine currently withdraws offers, that path has no implementation waiting for it.

`expired` **is** real and is enforced client-side — the pool has no `deadline` field, so
nothing on-chain stops an agent accepting a stale offer. Your policy engine checking the
deadline is not belt-and-braces, it is the enforcement.

**`memoHash` has a range constraint.** It must be a valid `felt252`, i.e. below the STARK
prime. A raw SHA-256 or Keccak digest is 256 bits and **will be rejected** — the TS oracle
accepted these silently and the Rust does not (`friction.md` F19). If your agents hash
anything into `memoHash`, truncate to 128 bits. This is a live P0.3 item; flag it if your
side wants different behaviour.

**Erebus holds no keys, and that shapes your MCP server.** The library runs inside the agent
operator's process, against the operator's own prover — because the proving call carries the
pool private key in the clear. So the MCP server is something an operator runs, not a
service we host. It needs a prover URL and identity in its config, and it should fail loudly
rather than fall back to a shared endpoint. See `docs/custody-design.md`.

**Real error surface for the mock.** These now exist as concrete Rust types rather than
guesses, so mock these rather than inventing your own: `NotAnAcceptance`, `ZeroPayment`,
`NothingToSpend`, `IndexCollision`, `NotSequential`, `AlreadyWritten`, `Misaligned`, and
from the prover, screening rejection (JSON-RPC `10000`) distinct from a generic failure
(`-32603`, which carries no reason at all — see F20). The `SettlementErrorCode` mapping is
still a **guess and a P0.3 item**; do not treat the draft list as frozen.

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

## Day 2 — integrate and demo

### I2.1 — Swap mock for real
- [ ] Point `sdk/py` at Poulav's Rust client instead of the mock
- [ ] Fix interface mismatches together — expect some
- [ ] Handle real proof latency in the agent loop

**Do not let this be the first time the two languages meet.** P0.4 on Poulav's list exists
to get one method across the Python↔Rust seam early, with a stub underneath. Push for that
to happen in Phase 1, not here — this is the step with no schedule left behind it.

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