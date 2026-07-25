# Tasks — Poulav (protocol / Cairo / on-chain)

You own everything from the SDK boundary down: the channel layer, settlement, disclosure, and the proof pipeline.

Read [ARCHITECTURE.md](./ARCHITECTURE.md) §4 (interface), §5 (hard constraints), §8 (open questions) before starting. Read [CLAUDE.md](./CLAUDE.md) §"Non-negotiable technical constraints" twice.

---

## Status — 2026-07-25

P0.1 and P0.2 are answered. Details and evidence in [friction.md](./friction.md).

| | Answer | Consequence |
|---|---|---|
| **P0.1** network | **Sepolia.** Pool v2.0 live at `0x0254a6…0d91`, verified on-chain. Mainnet has no deployment at all. | Not a preference — the only option. |
| **P0.2** payload | **No.** A note is `(packed_value: felt252, token: ContractAddress)`. No payload field at any layer. | **P1.3 is not implementable as written.** |

**Built and green as of 2026-07-25** (all offline, nothing touched a live network):

- P0.2 probe passes — 3/3 under `snforge`. F1 is measured now, not argued.
- Repo scaffolded: `contracts`, `sdk/ts`, `sdk/py`, `mcp-server`, `agents`.
- Baseline STRK20 flow works against upstream mocks — register → channel → subchannel
  → deposit → private transfer → recipient discovers the note. 3 tests.
- Static-static ECDH for the off-chain transport, 13 tests including 2 known-answer
  tests against Cairo reference vectors.
- `sdk/ts/src/interface.ts` — ARCHITECTURE §4 as committed TypeScript, for Ishita's mock.

`pnpm -r typecheck` clean, 16/16 tests green.

Two things are blocking and neither is ours to fix alone:

1. **No prover endpoint.** The pool is on-chain; the proving service is not published
   anywhere. Without a proof, `apply_actions` reverts. Ask StarkWare, or stand up
   Pathfinder + the prover container ourselves. Blocks P1.1 and everything after it.
2. **Where the negotiation record lives.** Three costed options in friction.md F1.
   This is a decision, not a task — and it is constrained by DoD #3 (a viewing key must
   reconstruct the full record), so it needs Ishita and probably StarkWare in the room.

---

## Day 0 — unblock everything else

These are ordered. Do them in order; each one de-risks the next.

### P0.1 — Verify the target network *(blocking for everyone)* — **PARTLY ANSWERED**

**Sepolia.** Not assumed — read off the chain via two independent public RPCs:

```
pool         0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91
class hash   0x67dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d
get_version()               → '2.0'
get_proof_validity_blocks() → 450
get_fee_amount()            → 0          (no STRK fee per apply_actions)
get_auditor_public_key()    → non-zero   (disclosure configured)
get_screener_public_key()   → non-zero   (screening ON — see P1.1)
```

Mainnet has no published deployment: `demo/.env.mainnet.example` upstream is entirely
`TODO_MAINNET_*` placeholders, and the docs site publishes no mainnet address.

- [x] Sepolia or mainnet? Confirmed on-chain.
- [x] Post the answer to Ishita — chain id `SN_SEPOLIA`, pool address above.
- [ ] **Get a Sepolia proving-service endpoint from StarkWare.** No public endpoint
      exists. Fallback is self-hosting `transaction-prover` + Pathfinder v0.22.7
      (`PATHFINDER_STORAGE_STATE_TRIES=10000`). *Blocks P1.1.*
- [ ] Confirm which prover / discovery tags match the **deployed** class hash. The
      README matrix pins RC.0 → `0x52107f…633`, which is not what is live.
      The matrix says use matching revisions.
- [ ] Pick the settlement ERC-20 on Sepolia and confirm we can obtain it.
- [ ] Discovery service: not published either, but **optional** — `ContractDiscoveryProvider`
      reads the pool over plain RPC. Use that for the MVP; revisit only if it's too slow.
- [ ] Paymaster: STRK20 ships none. The demo wires third-party **AVNU**, optional and
      unset by default. ARCHITECTURE §8 Q4 rests on AVNU, not on anything STRK20 gives us.
      Pool fee is 0, so this is about ordinary tx gas, not a pool fee.

**Acceptance:** you can shield a test amount and see the note appear via discovery.
*(Still blocked on the prover endpoint.)*

### P0.2 — Answer the highest-uncertainty question — **ANSWERED: no**

**Can subchannel writes carry arbitrary structured payloads, or does the SDK force a payment-shaped envelope?**

Payment-shaped envelope. Confirmed at three independent levels — the Cairo source, the
official docs (*"a note is an immutable record of three things"*: owner, token, amount),
and the deployed Sepolia contract's own ABI:

```
ClientAction -> [SetViewingKey, OpenChannel, OpenSubchannel, CreateEncNote,
                 CreateOpenNote, Deposit, UseNote, Withdraw, InvokeExternal, ComputeAndInvoke]
Note         -> [(packed_value, core::felt252), (token, ContractAddress)]
```

The only client-chosen bits that reach storage and come back are the note's **120-bit
salt**. A subchannel is a *token*, not a topic — one per (channel, token), enforced.
Full trace, capacity table, and three costed workarounds: [friction.md](./friction.md) F1.

- [x] Read the channel/subchannel write path in `starkware-libs/starknet-privacy`
- [x] Write the smallest possible test — `contracts/probes/p0_2_subchannel_payload.cairo`
      *(passing — 3/3 under snforge, see friction.md F3)*
- [x] Determine the workarounds and their costs — friction.md F1
- [ ] **Decide where the negotiation record lives.** Salt lane / own contract via
      `InvokeExternal` / off-chain behind a commitment. Constrained by DoD #3: a viewing
      key is *pool* key material, so only the salt lane keeps `reveal` working as §4
      specifies. Needs Ishita, and the framing change needs StarkWare (ARCHITECTURE §7
      already says to raise this early).

**Acceptance:** met — written answer in `docs/friction.md`.

### P0.3 — Agree the interface with Ishita
Sit down together. 30 minutes. Walk ARCHITECTURE.md §4 line by line.

- [ ] **Walk her through P0.2 first.** The §4 method *signatures* all survive — none of
      the three workarounds forces a signature change, so her mock is not invalidated.
      What changes is what `readChannelState` reads from and what `reveal` can
      reconstruct. Do not let this become a silent divergence.
- [x] Draft the interface file — `sdk/ts/src/interface.ts`, ARCHITECTURE §4 as committed
      TypeScript with per-method notes on what the P0.2 decision changes
- [x] Draft the error shape — `SettlementError` + `SettlementErrorCode`, including
      `SCREENING_REJECTED` / `SCREENING_UNAVAILABLE` on the deposit leg (see P1.1)
- [x] Confirm `OfferTerms` is encodable — it serialises to 5 felts, against a 120-bit
      on-chain lane. Encodable in Cairo; it just does not fit in a note.
- [ ] Confirm `memoHash` as a `felt252` hash works for both sides
- [ ] Agree the `SettlementErrorCode` set with her — the draft is a guess, not a freeze
- [ ] Freeze it

**Acceptance:** both of you have the same interface file committed.
*(File exists and is committed-ready. The agreement is still outstanding — this needs
the 30 minutes with Ishita, not more code.)*

---

## Day 1 — the settlement leg

### P1.1 — Shield → private transfer working end-to-end
Get the baseline STRK20 flow working before layering channels on it.

**Green offline. The on-chain half is blocked until P0.1's prover endpoint exists.**

The whole flow runs against upstream's `Mocknet` — mock pool, mock proving,
contract-backed discovery. `sdk/ts/tests/pool-flow.test.ts`, 3 tests.

- [x] Shield an ERC-20 into the pool *(mocks)*
- [x] Private transfer between two test accounts *(mocks — 500 shielded, 300 to Bob,
      200 change)*
- [x] Confirm the recipient can find and decrypt the note — keyed read via
      `ContractDiscoveryProvider`, no scanning
- [x] Establish that a note cannot exist without a subchannel — negative test
- [ ] Re-run all of the above against Sepolia. **Screening bites here, not on mocks**:
      any action set containing a `TransferFrom` needs a screener-signed attestation
      fresh within 300s or reverts with `SCREENING_REQUIRED`. Rides along in the proving
      response's `additionalData`; self-hosting means the proof-interceptor sidecar too.
      Only the deposit leg is gated — note-to-note transfers are not.
- [ ] Follow simulate → prove → `apply_actions` strictly *(the mock skips the prove leg,
      so this is genuinely untested)*
- [ ] Set `provingBlockId = currentBlock - 10` (notes mature 10 blocks; head-based
      proofs die to reorgs). Proofs stay valid 450 blocks on this pool.

**Acceptance:** a script that runs the full shield-and-transfer and prints the receipt.
*(Offline equivalent passes. The receipt needs a real proof.)*

### P1.2 — Channel establishment

> ~~Derive `channel_key` from both parties' addresses and viewing keys via ECDH over the
> Stark curve~~

Struck because it describes something the pool does not do. `compute_channel_key` is
`h(TAG, sender_addr, sender_private_key, recipient_addr, recipient_public_key)` — a hash
over the *sender's private key*, not a symmetric ECDH secret. The recipient cannot
derive it; they receive it, encrypted under a separate ephemeral ECDH, via
`EncChannelInfo.enc_channel_key` in the on-chain `Append`. That is why channels are
directional. ARCHITECTURE §2 merges the two mechanisms and should be corrected.

- [x] Open a channel and its token subchannel, and have the counterparty read it back
      *(mocks — `sdk/ts/tests/pool-flow.test.ts`)*
- [x] Static-static ECDH over the registered Stark-curve viewing keys, for the
      **off-chain transport** — `sdk/ts/src/crypto/channel-secret.ts`, 13 tests
      including 2 known-answer tests against Cairo reference vectors. This is a
      different secret from the pool's `channel_key`; do not conflate them.
- [ ] Register the channel on Sepolia *(blocked on the prover)*
- [ ] Verify a third party observing the chain cannot detect the channel exists.
      **Untestable on mocks** — needs a real chain and a real observer.

**Acceptance:** two accounts share a channel; a third account scanning sees nothing.
*(First half done offline; the privacy claim itself is still unverified.)*

### P1.3 — Offer state in subchannels — **NOT IMPLEMENTABLE AS WRITTEN**

P0.2 killed this task's premise. There is nowhere to put `Offer` / `Counter` / `Accept`
structs: a note is one felt of encrypted amount plus a 120-bit salt, and `ClientAction`
has no data variant. The old checklist is kept below, struck through, so the change is
visible rather than quietly rewritten.

> ~~Encode `Offer` / `Counter` / `Accept` as Cairo structs~~
> ~~Write offer state into subchannels with contiguous indexing~~

What survives regardless of which workaround wins:

- [ ] Counterparty can read and decrypt whatever we *do* write, via a keyed read
      (`note_id = h(NOTE_ID_TAG, channel_key, token, index, 0)`), never a scan
- [ ] Contiguous indexing — the pool enforces it (`INDEX_NOT_SEQUENTIAL`), so any
      offer encoding must never skip or reorder an index
- [ ] Enforce the state machine (ARCHITECTURE.md §4) — no accepting an expired or
      withdrawn offer. **The pool cannot do this for us**; it has no field for
      `status`, `deadline`, or `replyTo`. Enforcement lives wherever the record lives.

Rewrite the rest of this section once P0.2's open decision lands. Sketch per branch:

| Branch | What P1.3 becomes |
|---|---|
| Salt lane | Zero-amount data notes, ~7 per offer or 1 per commitment. Needs an SDK fork — the salt is hardcoded to `generateRandom120()` and dropped on read. |
| `InvokeExternal` | Write and review a second Cairo contract holding offer state; payload public unless we encrypt under `channel_key`. One invoke per tx, and it can't stand alone (`NO_REPLAY_PROTECTION`). |
| Off-chain + commitment | No Cairo work here at all; task moves to the SDK/transport track and ARCHITECTURE §7 needs rewording. |

**Acceptance:** A writes an offer, B reads it, B counters, A reads the counter — by
whichever mechanism the decision picks.

### P1.4 — Measure proof time *(do this today, not later)* — **VENDOR FIGURE IN HAND**

StarkWare publish **~29 s (12-core / 46 GiB; machine-dependent)** for Stwo proof
generation. The "if proving takes 30 seconds" guess below was almost exactly right.

The unit is the **transaction**, not the action — so batching notes into one action set
amortises, but rounds do not. Each offer and each counter is its own tx and its own
proof. Three rounds ≈ 90 s of proving before settlement even starts.

- [x] Ballpark from the vendor — friction.md F7
- [ ] Measure it ourselves on our hardware. Theirs is a 12-core/46 GiB box; ours is not.
- [ ] Record the real number in `docs/friction.md` and revise the demo script around it

**Why now:** confirmed — the multi-round negotiation demo needs rethinking. A 2–3 minute
recording cannot show several rounds in real time at ~29 s per proof. Decide whether the
demo shows fewer rounds, is time-compressed in the edit, or is honest about the wait.

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
  - **Tension to resolve, not ignore:** P0.2's salt-lane workaround needs an SDK fork
    (the note salt is hardcoded to `generateRandom120()` and dropped on read), and the
    `InvokeExternal` workaround needs a second Cairo contract of our own. Neither forks
    the *pool*, so both arguably stay inside this guardrail — but say so out loud when
    the decision lands rather than discovering the tension mid-build.

## Reading

1. **STRK20 by Example** — https://strk20-by-example.org. Primary hands-on reference.
   **No browser needed** — every route mirrors as raw Markdown. Append `.md` to any
   path, or start from `/llms.txt` (page index) or `/llms-full.txt` (whole site, one
   file). The Sepolia pool address and the ~29 s proving figure both come from here.
2. **OpenZeppelin audit** — https://www.openzeppelin.com/news/privacy-contracts-audit. Closest thing to an architecture spec that exists publicly. Read the findings too, not just the description.
3. **`starkware-libs/starknet-privacy`** — the source. Go here once the above give you the model.
   Cloned to `../starknet-privacy` @ `3dfe66f`. Highest-value files for our purposes:
   `packages/privacy/src/{objects,actions,privacy,hashes,utils}.cairo`,
   `sdk/src/interfaces.ts`, `sdk/src/internal/compiler.ts`. The upstream README's
   compatibility matrix and `demo/.env*.example` are where the deployment facts live.
4. Starknet v0.14.2 / SNIP-36 release notes — context on native proof verification.
5. **Agent skill** — `npx skills add starkience/strk20-agent-skills`
   ([repo](https://github.com/starkience/strk20-agent-skills), Apache 2.0). Plans an
   STRK20 integration for a repo. Not installed — it authors its own
   `STRK20_INTEGRATION_PLAN.md`, which would overlap ARCHITECTURE.md and this file.