# Tasks — Ishita (agents / orchestration / MCP)

You own everything above the SDK boundary: the agents, the negotiation policy, the MCP server, and the demo.

**You are not blocked on Poulav.** You build against a mock of the interface from hour one and integrate on Day 2. That parallelism is the whole plan — protect it.

Start with [ARCHITECTURE.md](./ARCHITECTURE.md) §4 (the interface you're building against) and §7 (the messaging nuance — important for how we talk about this externally).

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

- [ ] Implement `ErebusClient` as an in-memory mock — same signatures, fake state
- [ ] Realistic latency on `acceptAndSettle` (assume proof generation is slow; Poulav will give you a real number Day 1)
- [ ] Mock should be able to simulate failure: expired offer, failed proof, unreachable discovery service

**Acceptance:** you can call every method on the mock and get sane responses.

**Why the failure cases matter:** your agents need to handle a settlement that doesn't go through. If you only build the happy path, integration day breaks badly.

---

## Day 1 — agents and policy

### I1.1 — Negotiation policy engine
Keep this simple. A threshold rule is enough for the MVP — do not build a sophisticated bargaining strategy.

- [ ] Agent A (buyer): has a budget and a task; proposes, evaluates counters, accepts if within budget, walks away if not
- [ ] Agent B (seller): has a reserve price; evaluates offers, counters once, accepts or declines
- [ ] Enforce a maximum round count so the demo can't loop forever
- [ ] Unit tests on the accept/reject decision

**Acceptance:** given a set of terms, each agent makes a deterministic, testable decision.

### I1.2 — Two reference agents running the loop
- [ ] Two agent instances driving the negotiation against the mock
- [ ] They open a channel, negotiate to agreement, trigger settlement, and one grants a viewing key
- [ ] Structured logging of every state transition — you'll need this for the demo

**Acceptance:** `uv run python agents/demo.py` completes a full negotiation against the mock.

### I1.3 — MCP server
This is what makes it *infrastructure* rather than a demo. It's the difference between "we built a thing" and "any agent framework can use this."

- [ ] MCP server exposing: `open_channel`, `propose_offer`, `counter_offer`, `read_channel_state`, `accept_and_settle`, `grant_viewing_key`, `reveal`
- [ ] Clear tool descriptions — an agent should understand when to call each without reading source
- [ ] Verify from a real MCP client, not just your own agents

**Acceptance:** an external agent framework can drive the loop through the MCP server with no knowledge of Erebus internals.

**Why this matters more than it looks:** StarkWare is evaluating whether this is reusable infrastructure. An MCP server that any framework can call *is* the proof. Prioritize this over polishing the agents.

---

## Day 2 — integrate and demo

### I2.1 — Swap mock for real
- [ ] Point the SDK at Poulav's implementation
- [ ] Fix interface mismatches together — expect some
- [ ] Handle real proof latency in the agent loop

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

---

## Reading

You're coming to Starknet fresh. This ramps you without requiring Cairo.

1. **[ARCHITECTURE.md](./ARCHITECTURE.md) §1, §2, §4, §7** — start here. Fastest path to being on the same page as Poulav.
2. **"The Controversy of On-Chain Privacy: Monero, Tornado Cash, and STRK20"** — https://dev.to/okolievans/the-controversy-of-on-chain-privacy-monero-tornado-cash-and-strk20-2i7n. Plain language, no Cairo. Read this for the vocabulary: notes, nullifiers, channels, subchannels, viewing keys.
3. **STRK20 launch post** — https://www.starknet.io/blog/make-all-erc-20-tokens-private-with-strk20/. Product framing and use cases.
4. **STRK20 by Example** — https://strk20-by-example.org. Skim the Wallet API and SDK sections. You need the shape of the interface, not the contract internals.
5. **Starknet account abstraction docs** — https://docs.starknet.io. Specifically session keys and paymasters. This is *why* agents work well here: no gas balance, no seed phrase management.

**What already transfers:** the offer/counter/accept state machine is an agent decision loop, which is your existing work. The x402 and ERC-8004 patterns you've built are the same shape — autonomous agents transacting under rules. The only genuinely new thing is the settlement rail.