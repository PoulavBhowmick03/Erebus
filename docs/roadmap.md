# Roadmap — from the Sepolia MVP to a usable v1

Written 2026-08-06. **Status: draft — the three decisions in §3 are unfilled and the
sequencing in §6 is a proposal until they are.**

What this doc is: the sequencing layer on top of [production-gaps.md](./production-gaps.md)
(which lists the gaps and deliberately refuses to order them) and
[usecases.md](./usecases.md) (which lists what each fix unlocks). Facts are cited to those
docs, to [friction.md](./friction.md) entries, or to task files. Where something is
inference rather than a repo fact, it says so.

What this doc is not: a commitment. "v1" here means *usable by an external operator —
agent, company, or developer — through the MCP server or the Rust SDK, against real value,
without talking to us first.* Whether that is the right definition is itself decision D1.

---

## 0. Where the build stands (against the MVP Definition of Done)

| # | DoD item | Status |
|---|---|---|
| 1 | Two agents negotiate over a private channel on testnet | **Open.** The loop ran live on wire v1, which *disproved* the privacy claim — the transcript reconstructs from public calldata (F30). Wire v2 encrypts it and is green offline; it has not run live (poulav.md status). |
| 2 | Atomic settlement through the pool with a valid proof | **Done live.** Tx `0x44289c…84bb7`, acceptance record + payment note + spent nullifier in one proof. |
| 3 | Third party with a viewing key reconstructs the record | **Done live.** A fresh client with no local state reconstructed the full record from the bearer grant. |
| 4 | External framework drives the loop through MCP | **Nearly.** Verified against a real `mcp` client over stdio; one fresh live autonomous settlement is owed since the payer/payee guard landed (ishita.md I2.1, F33). |
| 5 | Honest friction log | **Done.** 33 entries. |

---

## 1. Now — close the evidence gaps (M0)

Cheap, unblocked, and everything downstream argues from them.

- **E1 — Fresh live wire-v2 run with an observer check.** A full negotiation + settlement
  on Sepolia under wire v2, then read the calldata as a public observer and show the five
  salts do not yield the transcript without the channel key. Closes the mechanical half of
  DoD #1. (poulav.md P1.2 Phase-2 evidence, F30/F31; usecases.md priority 1.)
- **E2 — Fresh live autonomous MCP settlement** with the payer/payee role guard in place.
  Closes DoD #4. (ishita.md I2.1, F33.)
- **E3 — Record the 2–3 minute demo** (ishita.md I2.2). Depends on E1/E2 so the recording
  shows the real wire and the real roles.
- **Hygiene, all small:**
  - Tell Akash the P0.2 result changed — open since the v1 privacy failure (poulav.md P0.2).
  - The P0.3 interface-freeze walkthrough sign-off — both open boxes are decided; this is
    confirmation, not negotiation (poulav.md P0.3).
  - `docs/one-pager.md` — ishita.md I2.3 marks it done at that path but the file does not
    exist in the repo. Recover or rewrite it; the packet in M1 needs it.
  - Measure proof time on our own hardware and record it (poulav.md P1.4) — the ~29 s figure
    is StarkWare's box, and the demo edit decision hangs on the real number.
  - **Reconcile the status docs — they currently contradict each other on the load-bearing
    claim.** production-gaps.md ("verified against a live transaction", §4) vs
    README/poulav.md/usecases.md/F31 (live v2 run open); ishita.md still describes a
    four-note grid in its intro; CLAUDE.md still says the Python seam is on protocol 1.
    For an external operator, stale custody/setup docs are a security problem, not
    cosmetic debt. One current-state doc generated from the E1/E2 evidence; mark the rest
    stale. *(Codex catch.)*

---

## 2. The fork points — external questions that reshape everything downstream

These belong to StarkWare or upstream, not to us. The M1 packet exists to force answers.

- **FK1 — Does the pool key have to reach the prover and write RPC?**
  `compile_actions` and the prove call both carry it in the clear (F14,
  production-gaps.md §1). Every item in production-gaps §1 is a workaround for this.
  Three outcomes: operator self-hosts (see FK1b — this is *not* complete "today");
  StarkWare runs a public prover (moves the exposure, doesn't remove it); upstream lets the
  client pass derived values only (the real fix, never asked). production-gaps.md calls
  asking this the single highest-leverage item on the list. It is a question, not work.
  Nuance (Codex): the *mechanism* is theirs, but the endpoint trust boundary, its
  disclosure to operators, and the reproducible deployment stay ours whichever way they
  answer.
- **FK1b — Screening authority is a distinct dependency, not a footnote of FK1.**
  A self-hosted prover has no screener key, so a fresh identity cannot shield through it —
  poulav.md Phase 2 says this explicitly ("it does not get us the shield") and
  production-gaps §1's "works today" elides it. Custody-by-self-hosting therefore needs one
  of: authorized screening access from StarkWare, a trusted third-party prover for the
  funding leg only (exposure disclosed), or our own pool with our own screener key (F6).
  *(Codex catch — neither headline doc models this as its own dependency.)*
- **FK2 — Where does real value live?** Mainnet has no STRK20 deployment (F4). v1-with-real-
  money is gated on StarkWare deploying, or on us deploying our own pool instance
  (constructor unpermissioned, class declared — F6) — which puts the screener *and* the
  pool-wide auditor key in our hands. That is a different trust product, and production
  would want the auditor under threshold control (production-gaps.md §4). Not our call to
  make unilaterally; it is the second question in the packet.
- **FK3 — Will the Discovery Service be published?** Unpublished today; our client polls
  keyed reads instead. Determines whether "an agent learns a message arrived" is
  subscription or polling forever (production-gaps.md §2).
- **FK4 — Paymaster.** "Agents shouldn't hold a gas token" rides on third-party AVNU
  (poulav.md P0.1, decided post-MVP). Additive; does not block v1 unless D1 makes gasless
  operation part of the pitch.

---

## 3. Three decisions that define v1 — deliberately unfilled

These are product judgments, not engineering facts. They are the owners' to write, and the
sequencing in §6 changes with them.

### D1 — What v1 *is* and who the first user is
"Usable via MCP, skills, or the SDK by users, agents, companies" is three products with
three critical paths. usecases.md gives the menu: the **one-off off-chain service purchase /
bilateral RFQ** is the shape the current mechanism already fits; **recurring B2B
(invoices, payroll, repeat procurement)** is the shape it explicitly disqualifies today
(one deal per pair, ever — production-gaps.md §3). Pick the anchor use case and the
first-user profile; A-track priorities reorder accordingly.

One sub-decision Codex is right to force into the open: **what privacy claim v1 makes.**
Random padding closes the fifth-salt fingerprint (F31) — it does not hide the submitting
account, the pool interaction, timing, or action shape (usecases.md trust-boundary table).
"Confidential terms + shielded value" is achievable with Track A as written; "relationship-
graph privacy" needs submission unlinkability and traffic-shape work — a materially larger
track that nothing below budgets for.

> **Decision box (owners):**
> v1 anchor use case: ____
> First external user we build for: ____
> Deliverable surface for v1 (MCP server / Rust SDK / both): ____
> v1 privacy claim (confidential terms / relationship privacy): ____

### D2 — Repeat deals: is wire v3 in v1?
Two agents currently transact exactly once, ever — the pool's one-channel-per-pair rule
(F29) composed with our one-deal-per-channel framing. The fix is a wire-format change
(variable-width framing) and it should land **together** with the traffic-fingerprint fix
(random-fill the 59 spare bits and the marker — F31), because both move the wire, the TS
oracle, and the fixtures, and doing that twice is strictly worse (production-gaps.md,
closing note). If D1 picks a recurring use case, this is v1-blocking; if one-off, it can
follow.

> **Decision box (owners):** wire v3 in v1: yes / no. If yes, it is the single biggest
> protocol work item in §4 Track A.

### D3 — Where the review gate lands
poulav.md is candid that large protocol surfaces are marked **"Unreviewed — written by
Claude"** (actions, tx, channel setup, read path, wire codec, negotiation state machine,
settlement, disclosure), and wire v2's cryptography (AES-256-GCM-SIV + HKDF construction)
has no independent review. Any v1 touching real value has both reviews on the critical
path. The judgment call is *when*: reviewing before wire v3 means reviewing a wire that
will change; after means feature work rides unreviewed for longer.

> **Decision box (owners):** internal review pass scheduled: ____ ;
> external crypto review scoped to: wire v2 / wire v3 / both, engaged when: ____

---

## 4. The gap skeleton — known work, sorted into tracks

Sequencing within tracks is proposal; the items and their citations are fact.

### Track A — protocol work (ours; Rust + wire + fixtures)

| ID | Item | Why / source | Unlocks |
|---|---|---|---|
| A1 | **Wire v3: variable-width framing + randomized spare bits/marker + deal identifiers.** One change, lands once. Scope includes what interop actually requires (Codex): porting the `sdk/ts` oracle to the final wire, publishing conformance vectors, and a normative byte-level spec — an audit of one implementation does not create interoperability, and wire v2 currently has no cross-language peer (F31). | F29/F31, production-gaps §3+§4, usecases pri 2+5; CLAUDE.md's own two-implementations rule | Repeat deals per pair; removes the fingerprint; external SDK consumers can build against a spec |
| A2 | **General note selection + change notes.** Settlement currently requires an exact-sum subset and refuses surplus. | poulav.md status; usecases pri 6 | Arbitrary prices; recurring balances |
| A3 | **Idempotency tokens + crash recovery + on-chain state recovery.** A crash between inclusion and response can orphan a handle or double-propose; losing `state_dir` loses handles though keys are derivable. Concretely (Codex): a durable journal of request → proof → submitted hash → receipt, restart reconciliation against chain state before any retry, and fault-injection tests at every boundary (preflight / prove / submit / inclusion / receipt / persist). Exit test: kill either MCP process at every boundary and resume without double-proposing or paying the wrong direction. | production-gaps §4; F33 | Safe unattended operation |
| A4 | **Recipient-bound grants: encrypt to grantee, expiry, revocation; per-deal disclosure.** Today's grant is a bearer secret for a whole channel. | usecases Level 2, pri 4 | Safer third-party disclosure |
| A5 | **Multi-token per client instance.** `ClientConfig.token` fixed at construction; client limitation, not protocol. | production-gaps §4 | Multi-asset operators |
| A6 | **`memo_hash` preimage + signature convention, published.** | usecases pri 8 | Interoperable external commitments |
| A7 | **Outcome-only receipts** (participants + terms commitment + settlement bound, without the transcript). | usecases Level 3, pri 3 | Platform integrations — likely post-v1 unless D1 says otherwise |
| A8 | **Fewer on-chain rounds** (off-chain negotiation with one commitment, or aggregation). | usecases pri 7; F27 (~3 STRK per message, measured) | Lower-value / higher-frequency commerce — post-v1 |
| A9 | **The review debt** (D3): internal pass over every "Unreviewed" surface, then external crypto review. | poulav.md, CLAUDE.md review rule | Gate G1 |

### Track B — operator experience (ours; Python/MCP/distribution/docs)

| ID | Item | Why / source | Unlocks |
|---|---|---|---|
| B1 | **Platform wheels carrying `erebus-cli`; `uvx erebus-mcp` works.** The ruff pattern; CI cross-compiles macOS arm64/x86 + Linux. "Routine work with no design content." | production-gaps §1 | Anyone installs in one command |
| B2 | **Self-host prover recipe:** Pathfinder v0.22.7 (`PATHFINDER_STORAGE_STATE_TRIES=10000`) + `transaction-prover`, scripted and documented; sync is the long pole, start early. **Caveat (FK1b): this alone does not let a fresh identity shield — the funding leg needs a screening path.** Not an exit criterion until a fresh identity can register, fund, negotiate, and settle without falling back to Akash's endpoint. | poulav.md Phase 2; production-gaps §1 | The custody claim becomes true for everything *except* the funding leg, pending FK1b |
| B3 | **Read scaling: cache the write-once prefix, read forward from the stored cursor.** Today reads are O(notes) from zero on every poll. | production-gaps §2 | Polling stops being quadratic |
| B4 | **Arrival notification: MCP long-poll tool server-side, and/or wire in the Discovery Service if FK3 resolves.** | production-gaps §2 | Agents subscribe instead of poll |
| B5 | **Setup UX:** the two-keys guidance gap (F26), `shield`'s once-per-identity behavior and its useless error (F32), runbook → quickstart, plus a `doctor` preflight (Codex): key permissions, state dir, pool registration, RPC head, prover compatibility, screening availability, balance/gas, exact private payability. | F26, F32, runbook.md | An operator onboards without us in the room |
| B6 | **Integration surface for agent frameworks:** an Erebus skill + integration guide so external agents drive the loop with no Erebus knowledge — the "skills" leg of v1. | DoD #4 generalized | Adoption beyond our two reference agents |
| B7 | **Minimal operational story:** enough error reporting/logging for an operator to diagnose a failed settlement. MVP scope-out expires at v1. | CLAUDE.md scope (inference: v1 needs more than demo-grade) | Companies can run it |

### Track C — upstream-dependent (theirs; we ask, track, and adapt)

| ID | Item | Fork |
|---|---|---|
| C1 | Derived-values-only proving (key never leaves the client) | FK1 — the real fix; everything in §1 of production-gaps is its workaround |
| C2 | Mainnet deployment, or a blessed self-deploy with threshold auditor | FK2 |
| C3 | Published Discovery Service | FK3 |
| C4 | Screening/attestation path on whatever chain v1 targets | rides FK2 |
| C5 | Paymaster (AVNU or upstream) | FK4 |

---

## 5. v1 gates — true regardless of how D1–D3 resolve

Inference, but a confident one: no version of "usable by companies" skips these.

- **G1 — Reviewed.** Internal pass done, external crypto review done, on the wire actually
  shipping (A9, D3).
- **G2 — Custody story.** No default path hands the pool key to a third party: at minimum
  B2 (self-host recipe) shipped and documented; ideally C1. A hosted Erebus that holds
  everyone's keys is explicitly a worse trust position than the public chain
  (production-gaps §1).
- **G3 — A chain with real value** (FK2 resolved, either direction).
- **G4 — Crash-safe** (A3): idempotent writes, recoverable state.
- **G5 — Installable in one command** (B1) and **onboardable without us** (B5).
- **G6 — The privacy claim holds as pitched:** live v2 evidence (E1), no traffic
  fingerprint (A1's F31 half), and the claim scoped per the D1 privacy-claim box —
  "confidential terms" unless the larger unlinkability track is funded.
- **G7 — Interop evidence** (Codex): a normative wire spec + published vectors + the
  cross-language peer green against them, per the repo's own two-implementations rule.
- **G8 — A settled license** and dependency-license review. README says TBD before any
  public release; v1 *is* the public release.
- **Cost honesty, not a gate:** ~3 STRK gas per message, measured (F27). Publish it;
  usecases.md already warns these snapshots are not durable prices.

---

## 6. Proposed sequence

```
M0  Evidence         E1 live wire-v2 + observer ── E2 live MCP settlement ── E3 demo
    (this week)      + hygiene (Akash, P0.3 sign-off, one-pager, proof timing)
                          │
M1  The packet       demo + friction.md + one-pager + FK1/FK2 asked explicitly
    (immediately     ── forces the forks; nothing else here is work
     after M0)            │
M2  Fork-independent  A9-internal review pass ── B1 wheels ── B3 reads ── B4 long-poll
    (parallel with    ── B5 setup UX ── B2 prover recipe (start the sync early)
     M1's wait)           │
M3  Protocol          D1/D2 decided → A1 wire v3 (+fingerprint fix, one landing)
    (post-decisions)  → A2 change notes → A3 idempotency/recovery → A5 multi-token
                      → then A9-external review, against the wire that ships
                          │
M4  v1 assembly       FK1 branch: C1 upstream fix  — or —  B2 as the documented default
                      FK1b: a screening path a fresh identity can actually use
                      FK2 branch: mainnet — or — own pool w/ threshold auditor plan
                      + A4 grants (if D1 needs), A6 memo convention, B6 skill/guides,
                      B7 ops story, G8 license
                          │
M5  Canary → tag v1   minimal-value run on the FK2 chain proving, in one pass:
                      fresh registration + funding under the chosen screening model,
                      repeat deals same pair, a non-exact payment needing change,
                      kill-and-recover mid-settlement, per-deal reveal,
                      auditor access behaving exactly as disclosed,
                      measured mainnet proof latency / gas / pool fee
                          │
Post-v1 parking lot   A7 receipts, A8 round reduction, multi-party, DvP, auctions,
                      sealed bids (usecases.md "needs protocol work" section)
```

Dependency edges that matter:

- **M1 does not block M2.** Everything in M2 is fork-independent by construction; that is
  the work to do while StarkWare's answer is pending.
- **A1 before A9-external.** Reviewing wire v2 and then shipping wire v3 buys the review
  twice (this is the D3 tension, stated as a default, not a decision).
- **B2's Pathfinder sync is the one cost that cannot be compressed** (poulav.md Phase 2) —
  start it in the background the moment M2 opens.
- **A1 lands once.** Framing change and fingerprint fix move the same files
  (`wire.rs`, `sdk/ts` oracle, fixtures); production-gaps closing note says land together.

---

## 7. Explicit non-goals, carried forward from MVP scope

Unchanged by this roadmap (CLAUDE.md scope; usecases.md "outside the current fit"):
no frontend or dashboard, no free-text messaging lane, no multi-party channels, no token or
economic layer, no cross-chain, no order-book/HFT ambitions, no hosted multi-tenant Erebus —
that last one now for custody reasons, not just scope.

---

*Provenance: drafted by Claude from production-gaps.md / usecases.md / poulav.md /
ishita.md / friction.md, then synthesized with an independent Codex planning pass run blind
on the same sources (2026-08-06, Codex session `019fd6d9-1a7f-7411-863f-3a4e2fbdd3a5`).
Items marked "(Codex catch)" were missed by the first draft. The decision boxes in §3 are
deliberately unfilled — they are the owners', not either model's.*
