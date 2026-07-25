# Tasks — Poulav (protocol / Cairo / on-chain)

You own everything from the SDK boundary down: the channel layer, settlement, disclosure, and the proof pipeline.

Read [ARCHITECTURE.md](./ARCHITECTURE.md) §4 (interface), §5 (hard constraints), §8 (open questions) before starting. Read [CLAUDE.md](./CLAUDE.md) §"Non-negotiable technical constraints" twice.

---

## Day 0 — unblock everything else

These are ordered. Do them in order; each one de-risks the next.

### P0.1 — Verify the target network *(blocking for everyone)*
Confirm which network has the full stack live and stable: privacy pool, discovery service, paymaster, and USDC (or whichever ERC-20 you'll settle in).

- [ ] Sepolia or mainnet? Confirm, don't assume.
- [ ] Discovery service endpoint reachable and returning data
- [ ] Paymaster available on that network
- [ ] Post the answer to Ishita immediately — her mock config depends on it

**Acceptance:** you can shield a test amount and see the note appear via the discovery service.

### P0.2 — Answer the highest-uncertainty question
**Can subchannel writes carry arbitrary structured payloads, or does the SDK force a payment-shaped envelope?**

This is the single question that determines whether the MVP is a two-day build or a rewrite. Find out before anything else is built on the assumption.

- [ ] Read the channel/subchannel write path in `starkware-libs/starknet-privacy`
- [ ] Write the smallest possible test that puts a non-payment struct into a subchannel
- [ ] If it doesn't work cleanly, determine the workaround and its cost

**Acceptance:** a written answer in `docs/friction.md` — yes it works, or here's the constraint and here's the workaround.

### P0.3 — Agree the interface with Ishita
Sit down together. 30 minutes. Walk ARCHITECTURE.md §4 line by line.

- [ ] Confirm the `OfferTerms` fields are sufficient and encodable in Cairo
- [ ] Confirm `memoHash` as a `felt252` hash works for both sides
- [ ] Agree the error shape — what does a failed settlement return?
- [ ] Freeze it

**Acceptance:** both of you have the same interface file committed.

---

## Day 1 — the settlement leg

### P1.1 — Shield → private transfer working end-to-end
Get the baseline STRK20 flow working before layering channels on it.

- [ ] Shield an ERC-20 into the pool
- [ ] Private transfer between two test accounts
- [ ] Confirm via discovery service that the recipient can find and decrypt the note
- [ ] Follow simulate → prove → `apply_actions` strictly

**Acceptance:** a script that runs the full shield-and-transfer and prints the receipt.

### P1.2 — Channel establishment
- [ ] Derive `channel_key` from both parties' addresses and viewing keys via ECDH over the Stark curve
- [ ] Register the channel
- [ ] Verify a third party observing the chain cannot detect the channel exists

**Acceptance:** two accounts share a channel; a third account scanning sees nothing.

### P1.3 — Offer state in subchannels
- [ ] Encode `Offer` / `Counter` / `Accept` as Cairo structs
- [ ] Write offer state into subchannels with contiguous indexing
- [ ] Counterparty can read and decrypt the state
- [ ] Enforce the state machine (ARCHITECTURE.md §4) — no accepting an expired or withdrawn offer

**Acceptance:** A writes an offer, B reads it, B counters, A reads the counter.

### P1.4 — Measure proof time *(do this today, not later)*
- [ ] Time client-side proof generation per action
- [ ] Record it in `docs/friction.md`

**Why now:** if proving takes 30 seconds per offer, the multi-round negotiation demo needs rethinking, and you want to know that on Day 1, not Sunday afternoon.

---

## Day 2 — atomicity and disclosure

### P2.1 — Atomic accept + settle
The core novelty. Acceptance and shielded transfer must be one proven state transition.

- [ ] Bind the accepted offer to the private transfer
- [ ] If the proof fails, the acceptance must not have happened
- [ ] Return a `SettlementReceipt` matching the agreed interface

**Acceptance:** accept-and-settle succeeds atomically; a deliberately invalid proof leaves state untouched.

### P2.2 — Viewing key disclosure
- [ ] Grant a viewing key to a third party
- [ ] Reconstruct the full record: participants, all offers, settlement
- [ ] Verify no leakage about unrelated users or channels

**Acceptance:** a Kleidouchos account reveals the complete negotiation and payment; a different account with a different key sees nothing.

### P2.3 — Integrate with Ishita's agents
- [ ] Swap her mock for the real implementation
- [ ] Run the full loop with live agents
- [ ] Fix the inevitable interface mismatches together

**Acceptance:** one green end-to-end run, agent-driven.

---

## Ongoing

- [ ] Log every piece of friction in `docs/friction.md` as you hit it — do not batch this at the end, you will forget the details
- [ ] Review all Cairo before it lands, including anything LLM-generated

---

## Guardrails

- Do not build a frontend.
- Do not implement free-text messaging (ARCHITECTURE.md §7).
- Do not add multi-party channels.
- Do not optimize anything before the loop is green.
- Do not change the interface without Ishita.
- If you find yourself refactoring the STRK20 primitives themselves, stop — we compose them, we don't fork them.

## Reading

1. **STRK20 by Example** — https://strk20-by-example.org (browser required, it's a JS app). Primary hands-on reference.
2. **OpenZeppelin audit** — https://www.openzeppelin.com/news/privacy-contracts-audit. Closest thing to an architecture spec that exists publicly. Read the findings too, not just the description.
3. **`starkware-libs/starknet-privacy`** — the source. Go here once the above give you the model.
4. Starknet v0.14.2 / SNIP-36 release notes — context on native proof verification.