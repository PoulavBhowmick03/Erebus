# What is missing before v1

Rewritten 2026-08-07 in plain English, for humans and for agents doing the work.
The previous version is in git at `a8fe7b9`, same content, far more labels.

**v1 means:** an outside operator, a person, a company, or an agent, installs this, runs
it against real value, and it keeps working without anyone from our team in the room.
Whether that is the right definition is Decision D1 below.

**How to read this file**

| Section | What it holds |
|---|---|
| 1 | What works today, with evidence |
| 2 | What does not work today |
| 3 | Questions only StarkWare can answer (Q1–Q4) |
| 4 | Decisions only the two owners can make (D1–D3) |
| 5 | The work, as one flat list (T1–T20) |
| 6 | The v1 finish line, as a checklist |
| 7 | What order to do it in |

There is one ID scheme: `T` for work, `Q` for questions, `D` for decisions. `F##` points at
an entry in [friction.md](./friction.md). Everything else is a plain sentence.

---

## 1. What works today

All of this has run for real. It is not a plan.

1. **A payment settles atomically on Sepolia.** Acceptance record, payment note, and spent
   nullifier all landed in one proof. Transaction
   `0x44289c4cacce0d07f45a6a788313ad341f44f40fd905c181a1e525050384bb7`, `SUCCEEDED /
   ACCEPTED_ON_L2`.
2. **Selective disclosure works.** A fresh client with no local state and no keys on disk
   rebuilt the entire negotiation and payment record from a bearer viewing grant.
3. **Shielding works, including screening.** A 1 STRK deposit went through
   preflight → prove → estimate → signed `apply_actions` → receipt. Screening happened
   invisibly, because the prover we use runs a `proof-interceptor` sidecar with a screener
   key. Fee was 3.04 STRK, all of it Starknet gas, the pool itself charges zero.
4. **The Rust client is complete offline.** 190 tests pass, 2 are intentionally skipped live
   prover probes.
5. **The MCP server reaches the chain.** Verified against a real `mcp` client over stdio.
6. **The friction log is honest.** 33 entries.

---

## 2. What does not work today

### 2a. The privacy claim is not proven yet

- **Wire v2 has never run on-chain.** The live run used wire v1, and wire v1 turned out to
  be publicly readable — every salt from the disclosed plaintext appears verbatim in the
  transaction calldata. Wire v2 encrypts the message (AES-256-GCM-SIV) and passes all its
  offline tests, but nobody has yet done a live run and then checked the calldata as an
  outsider. Until that happens we cannot say the negotiation is private. (F30)
- **Every message is recognisable as an Erebus message.** The wire uses 536 bits of 595
  bits of space, and the leftover 59 bits are filled with zeros. That gives the fifth salt
  of every single message an identical shape. Anyone reading calldata can pick out Erebus
  traffic, count the rounds, and see who talked to whom and when. Encrypting the contents
  does not hide any of that. (F31)
- **Wire v2 has no second implementation.** `sdk/ts` is still on wire v1 — there is no GCM
  code in it at all. The repo's own rule is that two agreeing implementations is the
  strongest correctness signal available, because there is no written spec. Right now wire
  v2 has one implementation and no spec.

### 2b. Two agents can only ever trade once

- The chain allows **one channel per pair of addresses**, permanently. `compute_channel_key`
  takes no index and its marker is written once. A second `open_channel` between the same
  two addresses reverts. We cannot change this locally. (F29)
- On top of that, **we allow one deal per channel** — our own `settled` check. That part is
  ours and we can change it.
- Together: two agents transact exactly once, ever. Removing our own check is not enough,
  because a settlement leaves the note cursor out of step with the five-notes-per-message
  grid. Fixing it properly means changing the wire format.

### 2c. Payments are rigid

Settlement picks notes that add up to **exactly** the offer amount. There is no change note.
If you hold a 5 STRK note and owe 3 STRK, the payment fails.

### 2d. The pool private key leaves the machine

The `compile_actions` preflight and the prove call both carry the pool private key in the
clear. So the prover operator and the write RPC operator can both decrypt every note that
identity will ever hold. Every other item in this area is a workaround for that one fact.
(F14)

Self-hosting the prover fixes the custody problem but **breaks shielding**: our own
interceptor has no screener key, so a fresh identity cannot deposit through it. Custody and
screening are two separate problems, and solving one currently un-solves the other.

### 2e. It cannot survive a crash

There are no idempotency tokens. If the process dies after the transaction is included but
before the response comes back, you can orphan a channel handle, or a retried proposal
becomes a second proposal. Losing the state directory loses the handles, the channel key is
derivable and on-chain recovery is possible in principle, but nobody has implemented it.

### 2f. Reading does not scale

`fetch_notes` walks `get_note` one index at a time, starting from zero, every single time.
A six-round negotiation is about thirty round trips per direction, and each poll repeats all
of it from scratch. Two agents polling every ten seconds make a few hundred RPC calls a
minute to read a twelve-message conversation.

Nothing pushes. There is no notification that a message arrived, only polling.

### 2g. Nobody can install it

Python runs a compiled Rust binary. Today that means building it yourself. There are no
platform wheels, so there is no one-command install.

### 2h. Large parts have never been reviewed

`poulav.md` marks these surfaces **"Unreviewed, written by Claude"**: actions, transactions,
channel setup, the read path, the wire codec, the negotiation state machine, settlement, and
disclosure. Wire v2's cryptography, the AES-256-GCM-SIV plus HKDF construction, has had no
independent review either.

### 2i. Other open items

- No STRK20 deployment on mainnet. There is nowhere to put real value. (F4)
- Each message costs about 3 STRK in gas, measured, not guessed. A six-round negotiation
  costs roughly 18 STRK before any value moves. (F27)
- A viewing grant is a bearer secret. It is not encrypted to the named recipient, and it has
  no expiry and no revocation. Whoever holds it can read the whole channel.
- One token per client instance. `ClientConfig.token` is fixed at construction.
- `memo_hash` can bind an offer to an external document, but we never published the format,
  so nobody else can produce or check one.
- The license says TBD. v1 is the public release.
- **The docs contradict each other.** `production-gaps.md` says wire v2 was "verified against
  a live transaction"; the README, `poulav.md`, `usecases.md`, and F31 all say the live v2 run
  is still open. `ishita.md` still describes a four-note grid. `CLAUDE.md` still says the
  Python seam is on protocol 1. `docs/one-pager.md` is marked done but the file does not
  exist. For an outside operator, wrong setup and custody docs are a security bug, not tidying.

### 2j. What "autonomous" specifically demands

The word "autonomous" in "v1" is doing real work. An agent running unattended cannot:

- **be handed exact note denominations**, so §2c blocks it,
- **be restarted by hand after a crash**, so §2e blocks it,
- **poll forever at a few hundred RPC calls a minute**, so §2f blocks it,
- **have a human read a stack trace**, so it needs real error reporting,
- **be installed by us on the operator's box**, so §2g blocks it,
- **transact with a counterparty exactly once in its life**, so §2b is a problem the moment
  the same two agents want to deal twice.

Whether §2b is v1-blocking is Decision D2. The rest are not judgment calls: they are the
difference between a demo and something that runs on its own.

---

## 3. Questions only StarkWare can answer

These are not our work. They are questions, and the answers change what we build. The point
of the demo packet is to force them.

**Q1 — Does the pool key really have to reach the prover and the write RPC?**
The key is needed inside the virtual execution to derive channel keys and nullifiers. Could
the client derive those locally and pass only the derived values? We have never asked. This
is the highest-leverage question on the list, because everything in §2d is a workaround for
it. There are three possible worlds: the operator self-hosts (see Q2), StarkWare runs a
public prover (moves the exposure, does not remove it), or upstream accepts derived values
(the actual fix).

**Q2. Can a self-hosted prover get screening access?**
A self-hosted prover has no screener key, so a fresh identity cannot shield through it. So
"just self-host" is not a complete custody answer. We need one of: authorised screening
access from StarkWare, a trusted third-party prover for the funding step only with the
exposure written down, or our own pool with our own screener key (F6).

**Q3. Where will real value live?**
Mainnet has no STRK20 deployment (F4). Either StarkWare deploys, or we deploy our own pool
instance, the constructor is unpermissioned and the class is already declared. Deploying our
own puts both the screener key and the pool-wide auditor key in our hands. That is a
different trust product and not a decision to make quietly.

**Q4. Will the Discovery Service be published?**
It is unpublished today, and our client polls keyed reads instead. The answer decides whether
"an agent learns a message arrived" is a subscription or polling forever.

*(A fifth, smaller one: a paymaster so agents need not hold a gas token currently rides on
third-party AVNU. It is additive and does not block v1 unless D1 makes gasless part of the
pitch.)*

---

## 4. Decisions only the owners can make

These are product judgments, not engineering facts. Leave them blank until you decide.
Section 7's ordering changes depending on the answers.

### D1 — What is v1, and who is the first user?

"Usable by users, agents, and companies" is three products with three critical paths. The
shape the current mechanism actually fits is a **one-off purchase of an off-chain service or
a one-shot bilateral RFQ**. The shape it explicitly does not fit is **recurring B2B** —
invoices, payroll, repeat procurement — because of §2b.

There is a second half to this: **what privacy claim does v1 make?** These are not the same
product:

- *"Confidential terms and shielded value"* — achievable with the work listed below.
- *"Nobody can see who is dealing with whom"* — needs submission unlinkability and traffic
  shaping. That is a much larger track and nothing in §5 budgets for it.

> **Fill in:**
> Anchor use case: ______
> First external user: ______
> What we ship (MCP server / Rust SDK / both): ______
> Privacy claim (confidential terms / relationship privacy): ______

### D2 — Do repeat deals ship in v1?

This is the wire-format change in §2b. If D1 picks anything recurring, it is v1-blocking. If
D1 picks one-off deals, it can wait.

One thing is not optional either way: **if you change the wire, change it once.** The repeat-
deal fix and the fingerprint fix (§2a) touch the same files — `wire.rs`, the `sdk/ts` oracle,
and the fixtures. Doing that migration twice is strictly worse than doing it once.

> **Fill in:** repeat deals in v1: yes / no

### D3 — When does the review happen?

Everything in §2h has to be reviewed before v1 touches real value. The judgment is *when*.
Review before the wire changes and you pay for the review twice. Review after and feature
work rides unreviewed for longer.

> **Fill in:** internal review pass: ______
> external crypto review, scoped to (wire v2 / wire v3 / both), starting: ______

---

## 5. The work

One flat list. Each item says what to do, why it is needed, and how you know it is finished.
"Done when" is meant to be literally checkable, by a person or by an agent.

### Group 1: Evidence. Do this first; it is cheap and everything else argues from it.

**T1. Run a full negotiation and settlement live on wire v2, then check it as an outsider.**
- *Why:* Nobody has proven the privacy claim. Wire v2 is green offline and untested on-chain.
- *Done when:* A Sepolia transaction exists with a wire-v2 offer, counter, acceptance, and
  payment; and someone has read that transaction's calldata with no channel key and shown
  the five salts do not yield the transcript.
- *Source:* §2a, F30/F31, poulav.md P1.2 Phase 2.

**T2 — Run one live autonomous settlement through the MCP server with the payer/payee role
guard active.**
- *Why:* The guard landed after the last live MCP run, so the current code path has never
  settled for real.
- *Done when:* Two agents drove a full deal to settlement through MCP with `EREBUS_BACKEND=seam`
  and neither one was nudged by hand.
- *Source:* ishita.md I2.1, F33.

**T3 — Record the 2–3 minute demo.**
- *Done when:* The recording shows the real wire and the real roles — so it depends on T1 and T2.

**T4 — Make one status document true and mark the rest stale.**
- *Why:* Five docs currently disagree about whether wire v2 ran live. An operator reading the
  wrong one gets their custody model wrong.
- *Done when:* One document describes current state, generated from the T1/T2 evidence; the
  contradictions listed in §2i are each either fixed or marked stale; `docs/one-pager.md`
  exists again; and `F31` stops being two different entries (it is currently used for both
  the traffic fingerprint and the nonce-misuse note, so every citation of it is ambiguous —
  34 headings, 33 unique IDs).
- *Source:* §2i.

**T5 — Measure proof time on our own hardware.**
- *Why:* The ~29 s number is StarkWare's machine, and the demo editing decision depends on ours.
- *Done when:* A measured number is written into friction.md.

**T6 — Small open loops.** Tell Akash the P0.2 result changed. Get the P0.3 interface-freeze
sign-off — both open boxes are already decided, so this is confirmation, not negotiation.

### Group 2 — Work that does not depend on any decision. Start it in parallel.

**T7 — Ship platform wheels that carry `erebus-cli`.**
- *Why:* No install story today. This is the pattern ruff uses; there is no design content in it.
- *Done when:* `uvx erebus-mcp` works on a clean machine, from CI-built wheels for macOS arm64,
  macOS x86, and Linux.

**T8 — Cache reads and read forward from the stored cursor.**
- *Why:* Reads are O(notes) from zero on every poll. Notes are write-once, so an index that
  resolved once can never change.
- *Done when:* A second poll of an unchanged channel makes a constant number of RPC calls,
  not one per note.

**T9 — Add a long-poll MCP tool so agents stop spinning.**
- *Why:* There is no arrival notification. A server-side blocking tool keeps the agent's turn
  count down without changing the transport. If Q4 resolves, wire in the Discovery Service
  instead or as well.
- *Done when:* An agent can wait for a counterparty message without issuing repeated polls.

**T10 — Make setup work without us in the room.**
- *Why:* The two-keys guidance gap (F26), `shield`'s once-per-identity behaviour and its
  useless error message (F32), and a runbook that reads like notes rather than a quickstart.
- *Done when:* A `doctor` command checks and reports on: key file permissions, state
  directory, pool registration, RPC head, prover compatibility, screening availability,
  balance and gas, and exactly how much is privately payable. And a new operator gets through
  the quickstart without asking us anything.

**T11. Do the internal review pass** over every surface marked "Unreviewed, written by
Claude" in §2h.
- *Done when:* Every listed surface has been read line by line by Poulav and the marker is gone.

**T12. Script and document the self-hosted prover.**
- *What:* Pathfinder v0.22.7 with `PATHFINDER_STORAGE_STATE_TRIES=10000`, plus
  `transaction-prover`. **The sync is the long pole, start it in the background the day this
  group opens.**
- *Caveat:* This alone does not make the custody claim true, because of Q2. A fresh identity
  still cannot shield through it.
- *Done when:* A fresh identity can register, fund, negotiate, and settle without ever
  touching Akash's endpoint. Until then, mark it partial and say so out loud.

### Group 3: Protocol work. Blocked on D1 and D2.

**T13. The wire change, landed once.**
- *What:* Variable-width framing (so the note grid survives a settlement), randomised spare
  bits and marker (so the fingerprint goes away), and deal identifiers.
- *Plus, and this is not optional for an SDK anyone else builds on:* port the `sdk/ts` oracle
  to the final wire, publish conformance vectors, and write a normative byte-level spec. An
  audit of one implementation does not produce interoperability.
- *Done when:* Two agents complete two separate deals; an observer cannot distinguish Erebus
  calldata by shape; `sdk/ts` and `sdk/rs` agree byte-for-byte on published vectors; the spec
  is written down.
- *Source:* §2a, §2b, F29/F31.

**T14 — General note selection with change notes.**
- *Done when:* A payer holding a single 5 STRK note can pay 3 STRK and keep 2.

**T15 — Idempotency tokens, crash recovery, and on-chain state recovery.**
- *What:* A durable journal of request → proof → submitted hash → receipt. On restart,
  reconcile against chain state before retrying anything.
- *Done when:* You can kill either MCP process at every boundary — preflight, prove, submit,
  inclusion, receipt, persist — and resume without double-proposing and without paying the
  wrong direction. There is a fault-injection test for each boundary.
- *Source:* §2e, F33.

**T16 — One client instance handles multiple tokens.**
- *Why:* `ClientConfig.token` is fixed at construction. The pool supports a subchannel per
  token, so this is our limitation, not the protocol's.

**T17 — External cryptographic review**, run against the wire that actually ships (see D3).

### Group 4 — Needed for v1, but the shape depends on D1 and Q1–Q3.

**T18 — Recipient-bound viewing grants:** encrypted to the grantee, with expiry, revocation,
and per-deal rather than per-channel scope. Needed if D1 involves disclosing to anyone you do
not already trust completely.

**T19 — Publish the `memo_hash` preimage and signature convention.** Needed the moment
anything external has to produce or verify a commitment.

**T20 — An integration surface for agent frameworks:** an Erebus skill and an integration
guide, so an outside agent can drive the loop with no Erebus knowledge. Plus enough error
reporting and logging that an operator can diagnose a failed settlement on their own. And
settle the license.

### Explicitly not in v1

Outcome-only ZK receipts. Reducing the number of on-chain rounds. Multi-party channels.
Delivery-versus-payment. Sealed-bid auctions. Order books or anything high-frequency. A
frontend or dashboard. Free-text messaging. A token or economic layer. Cross-chain anything.
A hosted multi-tenant Erebus — that last one now for custody reasons, not just scope.

---

## 6. The v1 finish line

This is a checklist, not prose. Every line has to be true, whatever D1–D3 decide.

- [ ] **Reviewed.** Internal pass done (T11) and external crypto review done (T17), both
      against the wire that ships.
- [ ] **Custody is honest.** No default path hands the pool key to a third party. At minimum
      the self-host recipe ships and is documented (T12); ideally Q1 gets answered properly.
- [ ] **Screening works for a stranger.** A fresh identity can fund itself under whatever
      model Q2 resolves to, and the exposure is written down where an operator will read it.
- [ ] **There is somewhere to put real value.** Q3 resolved, either direction.
- [ ] **It survives a crash.** T15 done, with the kill test passing at every boundary.
- [ ] **One command installs it** (T7) and **a stranger can onboard alone** (T10).
- [ ] **The privacy claim is proven, not asserted.** Live wire-v2 evidence (T1), no traffic
      fingerprint (T13), and the claim worded to match what D1 actually chose.
- [ ] **Someone else can build against it.** A normative wire spec, published vectors, and
      `sdk/ts` green against them (T13).
- [ ] **The license is settled** and dependency licences reviewed.
- [ ] **Costs are published, not hidden.** ~3 STRK per message today (F27), stated as a
      measurement and not a promise.

Not a gate, but say it anyway: a six-round negotiation costs roughly 18 STRK in gas before
any value moves.

---

## 7. Order

```
Step 1   Evidence            T1 live wire-v2 + observer check
         (this week)         T2 live MCP settlement
                             T3 demo, T4 doc reconciliation, T5 proof timing, T6 loose ends
                                 |
Step 2   The packet          demo + friction.md + one-pager, with Q1 and Q3 asked in writing
         (right after)       This is not engineering. It exists to force the answers.
                                 |
Step 3   Decision-free work  T7 wheels    T8 reads     T9 long-poll
         (runs during        T10 setup    T11 internal review
          step 2's wait)     T12 prover recipe  <- start the Pathfinder sync on day one
                                 |
Step 4   Protocol            D1 and D2 answered, then:
         (after decisions)   T13 the wire change, landed once
                             T14 change notes -> T15 crash recovery -> T16 multi-token
                             then T17 external review, against the shipping wire
                                 |
Step 5   Assembly            Q1: upstream fix, or T12 as the documented default
                             Q2: a screening path a stranger can actually use
                             Q3: mainnet, or our own pool with a threshold auditor plan
                             T18 grants (if D1 needs them), T19 memo, T20 integration + license
                                 |
Step 6   Canary, then tag    One low-value run on the Q3 chain that proves all of it at once:
                             fresh registration and funding under the chosen screening model,
                             two deals between the same pair,
                             a payment needing change,
                             kill-and-recover mid-settlement,
                             per-deal reveal,
                             auditor access behaving exactly as documented,
                             measured proof latency, gas, and pool fee on that chain.
```

**Four ordering facts:**

1. **Step 2 does not block step 3.** Everything in step 3 is true regardless of what
   StarkWare answers. That is precisely the work to do while waiting.
2. **The wire change comes before the external review.** Reviewing wire v2 and then shipping
   wire v3 means paying for the review twice. This is the tension in D3, stated as a default
   rather than a decision.
3. **The Pathfinder sync is the one cost that cannot be compressed** by working harder. Start
   it in the background the moment step 3 opens.
4. **The wire is touched once.** Framing and fingerprint move the same files. Two migrations
   is strictly worse than one.

---

*Sources: production-gaps.md, usecases.md, poulav.md, ishita.md, friction.md, README.md, and
the repo itself. The decision boxes in §4 are intentionally blank, they belong to the owners.*
