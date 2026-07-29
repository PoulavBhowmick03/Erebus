# Tasks — Poulav (Rust client / protocol / on-chain)

You own everything from the SDK boundary down: the Rust client, the channel layer,
settlement, disclosure, and the proof pipeline.

Read [ARCHITECTURE.md](./ARCHITECTURE.md) §4 (the interface you implement behind), §5 (hard
constraints), §8 (open questions) before starting. Read [CLAUDE.md](./CLAUDE.md)
§"Non-negotiable technical constraints" twice.

**How to read this file.** Tasks are grouped by phase, not by day — the weekend framing is
gone. Task IDs are stable and cross-referenced from `friction.md`, `ishita.md`, and the
briefing, so they do not get renumbered. A ticked box means *the Rust client does this*.

**On TypeScript.** `sdk/ts` is not a track and has no tasks here. It exists only as the
differential-test oracle: where Cairo emits no reference vector, the Rust is pinned against
the TS byte-for-byte. That is why it is still referenced below — as the thing you check
*against*, never as work in progress.

---

## Status — 2026-07-28

**Answered, with evidence in [friction.md](./friction.md):**

| | Answer | Consequence |
|---|---|---|
| **P0.1** network | **Sepolia.** Pool v2.0 at `0x0254a6…0d91`, verified on-chain. Mainnet has no deployment at all. | Not a preference — the only option. |
| **P0.2** payload | **No payload field.** A note is `(packed_value: felt252, token: ContractAddress)`. The salt is client-chosen and round-trips, so payloads fit by fragmentation — 119 bits per note, one permanently-burned storage slot each. | P1.3 is the salt lane: 4 notes per message. |

**Decided:**

- **2026-07-25 — the salt lane.** Negotiation payload rides in the salts of zero-amount
  data notes, on-chain, in the counterparty's subchannel. Rejected `InvokeExternal` (public
  calldata makes every Erebus tx self-identifying) and off-chain transport (moves the
  negotiation graph to the transport — the exact leak we exist to fix — and breaks
  `reveal`). Costs accepted: an SDK bypass, ~4 notes per message, unspendable data notes.
- **2026-07-26 — the client is Rust** (`sdk/rs`). No Rust write side exists anywhere;
  `discovery-core` covers reads only. Nothing builds `ClientAction`s, serialises calldata,
  signs, or calls the prover. Confirmed with the Starknet group that nobody is building one.
- **2026-07-28 — the agent layer is Python.** MCP server and agents both. This is what puts
  the Rust client on the demo's critical path rather than beside it, and it adds P0.4 to
  your track.

**Blocking, and neither is yours to fix alone:**

1. **No prover endpoint — answered 2026-07-28, and the answer is "self-host".** Akash has
   asked his team; no ETA. His recommendation meanwhile is to run our own prover. So Phase 2
   is no longer waiting on anyone — it is Pathfinder v0.22.7 + `transaction-prover` on our
   own hardware, and the work can start now.
   **But it does not unblock the shield.** Self-hosting gets private transfers, channel
   writes, settlement and reveal. It does not get deposits: `proof-interceptor` holds no
   screener key, and the live pool's `screener_public_key` is StarkWare's. See blocker 3.
2. **Which prover / discovery tags match the deployed class hash.** The README matrix pins
   RC.0 → `0x52107f…633`, which is not what is live.
3. **How we get a screening attestation.** Either StarkWare give us screening access, or we
   deploy our own pool instance with a screener key we hold. This is now the only hard
   dependency on someone else, and it gates one leg — the shield — not the whole demo.
   Mechanism and both options: friction.md F6.

---

## Phase 0 — unblock

Ordered. Each one de-risks the next. None of them require the prover.

### P0.1 — Target network — **ANSWERED: Sepolia**

Read off the chain via two independent public RPCs, not assumed:

```
pool         0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91
class hash   0x67dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d
get_version()               → '2.0'
get_proof_validity_blocks() → 450
get_fee_amount()            → 0          (no STRK fee per apply_actions)
get_auditor_public_key()    → non-zero   (disclosure configured)
get_screener_public_key()   → non-zero   (screening ON — see P1.1)
```

Mainnet has no published deployment: upstream's `demo/.env.mainnet.example` is entirely
`TODO_MAINNET_*` placeholders and the docs site publishes no mainnet address.

- [x] Sepolia or mainnet? Confirmed on-chain.
- [x] Post the answer to Ishita — chain id `SN_SEPOLIA`, pool address above.
- [x] **Get a Sepolia proving-service endpoint from StarkWare** — asked, and the answer is
      that none exists yet. Team notified, no ETA. **Self-host instead**, which is Akash's
      own recommendation: `transaction-prover` + Pathfinder v0.22.7
      (`PATHFINDER_STORAGE_STATE_TRIES=10000`). Now a Phase-2 task, not a Phase-2 blocker.
- [ ] **Resolve screening.** Self-hosting does not cover the deposit leg. Ask Akash for
      screening access, or deploy our own pool instance with our own screener key
      (constructor is unpermissioned, class already declared). *Gates the shield only.*
- [ ] Confirm which prover / discovery tags match the **deployed** class hash.
- [ ] Pick the settlement ERC-20 on Sepolia and confirm you can obtain it.
- [ ] Discovery service: not published either, but **optional** — `ContractDiscoveryProvider`
      reads the pool over plain RPC. Use that for the MVP; revisit only if too slow.
- [ ] Paymaster: STRK20 ships none. The demo wires third-party **AVNU**, optional and unset
      by default. ARCHITECTURE §8 Q4 rests on AVNU, not on anything STRK20 provides. Pool
      fee is 0, so this is ordinary tx gas, not a pool fee.

**Acceptance:** you can shield a test amount and see the note appear via discovery.
*(Blocked on the prover endpoint.)*

### P0.2 — Can a note carry a structured payload? — **ANSWERED: no payload field**

Payment-shaped envelope. Confirmed at three independent levels — the Cairo source, the
official docs (*"a note is an immutable record of three things"*: owner, token, amount), and
the deployed Sepolia contract's own ABI:

```
ClientAction -> [SetViewingKey, OpenChannel, OpenSubchannel, CreateEncNote,
                 CreateOpenNote, Deposit, UseNote, Withdraw, InvokeExternal, ComputeAndInvoke]
Note         -> [(packed_value, core::felt252), (token, ContractAddress)]
```

The only client-chosen bits that reach storage and come back are the note's **120-bit
salt**. A subchannel is a *token*, not a topic — one per (channel, token), enforced. Full
trace, capacity table, and three costed workarounds: [friction.md](./friction.md) F1.

- [x] Read the channel/subchannel write path in `starkware-libs/starknet-privacy`
- [x] Smallest possible test — `contracts/probes/p0_2_subchannel_payload.cairo`,
      passing 3/3 under snforge (friction.md F3)
- [x] Determine the workarounds and their costs — friction.md F1
- [x] Decide — the salt lane. See Status above and friction.md F1 "Resolution".
- [ ] **Tell Akash the answer changed.** The message sent 2026-07-25 described the
      off-chain-transport option; we picked the salt lane instead. Short correction — it is
      the better story anyway, since relationship privacy and `reveal` both survive.

**Acceptance:** met — written answer in `docs/friction.md`.

### P0.3 — Freeze the interface with Ishita

30 minutes together, ARCHITECTURE §4 line by line. The file exists; the *agreement* does
not, and that is the deliverable.

- [ ] **Walk her through P0.2 first.** The §4 method signatures all survive — the salt lane
      forces no signature change, so her mock is not invalidated. What changes is what
      `readChannelState` reads from and what `reveal` can reconstruct. Do not let that
      become a silent divergence.
- [ ] Confirm `memoHash` as a `felt252` hash works for both sides
- [ ] Agree the `SettlementErrorCode` set — the draft is a guess, not a freeze. Includes
      `SCREENING_REJECTED` / `SCREENING_UNAVAILABLE` on the deposit leg (see P1.1)
- [ ] Freeze it

Already true, so not a task: `OfferTerms` serialises to 5 felts against a 120-bit on-chain
lane — encodable in Cairo, just does not fit in one note.

**Acceptance:** both of you have signed off on the same interface.

### P0.4 — The Python ↔ Rust seam *(shared with Ishita)*

Her MCP server is Python; your client is Rust. Something has to cross. Half yours and half
hers, which is exactly why it will otherwise be nobody's until integration day breaks on it.

- [ ] **Pick the mechanism.** Subprocess (`erebus-cli`, JSON over stdio) or PyO3/maturin.
      Costs are in ARCHITECTURE §3 — the tradeoff is yours to make. Bias worth naming:
      subprocess has no build matrix and puts an OS boundary around key material, which
      turns CLAUDE.md constraint 6 from a rule into a property.
- [ ] **Land one method end-to-end before the rest exist.** `openChannel`, stubbed in Rust,
      called from Python, asserted in a test. The point is to prove the marshalling and the
      error mapping, not the protocol.
- [ ] Decide how a `SettlementError` crosses — an error that loses its `SettlementErrorCode`
      on the way up makes her failure handling untestable.
- [ ] Keep `/sdk/py` free of protocol logic. A hash, a felt conversion, or a salt encoder
      there is the bug this architecture is arranged to prevent.

**Acceptance:** Python calls one real Rust method and gets a typed result and a typed error.

**Why now, not later:** everything else on both tracks is mocked. This is the only place two
people's code physically meets, and the plan currently has it meeting for the first time
where there is no schedule left to absorb it.

---

## Phase 1 — the Rust client, offline

No prover needed for any of this. Push as much here as possible, because Phase 2 has no
start date.

**The rule for this whole phase** (CLAUDE.md): nothing lands unpinned. Every derivation is
fixed by a known-answer test before it is trusted — against the Cairo reference vectors in
`sdk/rs/tests/fixtures/cairo-reference-data.json`, or where Cairo emits no vector, against
the TS oracle byte-for-byte. Every failure mode in this protocol is silent; a wrong preimage
derives a slot nobody wrote to and the note is simply "not found", with no error anywhere.

### P1.0 — Client foundation

- [x] Conformance harness against Cairo reference vectors — **12/12 green**
- [x] Domain-separated hashes, all KAT'd *(first bug of the port lived here — tags truncated
      into a `u128`, caught in 30 seconds instead of a day. friction.md F12.)*
- [x] **Cairo Serde for `ClientAction`** — all 10 variants diffed byte-for-byte against the
      TS oracle, 8 KATs green *(`sdk/rs/src/actions.rs`, vectors in
      `tests/fixtures/ts-clientaction-serde.json`). Includes a `NoteSalt` newtype for the
      `(1, 2^120)` bound and a `phase()` mapping — phase order is deliberately not variant
      order, see friction.md F15.* **Unreviewed — written by Claude, needs your pass.**
- [x] **`INVOKE_TXN_V3` construction** — tx hash KAT'd against starknet.js, 5 tests green
      *(`sdk/rs/src/tx.rs`, vectors in `tests/fixtures/starknetjs-invoke-v3-txhash.json`).
      Covers the `proof_facts` branch both ways — it is a privacy-specific extension to the
      v3 preimage, so no off-the-shelf Starknet crate hashes this correctly. friction.md F16.*
      **Unreviewed — written by Claude, needs your pass.**
- [x] **Signing** — Stark ECDSA, 7 KATs *(`sdk/rs/src/signing.rs`)*. `starknet-crypto`'s
      RFC-6979 derivation matches `@scure/starknet` exactly, so signatures are byte-identical
      to starknet.js, not merely valid. Both directions pinned: ours verify under their keys,
      theirs verify under our code.
- [x] **End-to-end composition pinned** — `tests/proof_invocation.rs` rebuilds a proof
      invocation captured from upstream's own `ProofInvocationFactory` and **reproduces its
      signature byte-for-byte**. Actions → `compile_actions` calldata → `__execute__` wrapper
      → v3 hash → signature, all at once. This is the test that catches pieces being correct
      but wired together wrongly. 36/36 across the crate. **Unreviewed.**
- [ ] **Which `assert_valid_signature` route the pool actually takes** — `utils.cairo:383`
      tries three (custom validation, tx hash, SNIP-12 `CallSet` hash) and which one succeeds
      depends on the agent's account contract, not on us. Untestable offline; needs a real
      account on Sepolia.
- [x] **`starknet_proveTransaction` client** — async on reqwest/rustls, retries only on
      transport failures and 503 *(`sdk/rs/src/prover.rs`)*. **Verified live against Sepolia:**
      `spec_version` returns `0.10.3-rc.2`, and a `prove_transaction` call reaches execution
      (`-32603`) rather than being rejected as malformed (`-32602`), which live-validates the
      whole `INVOKE_TXN_V3` serialization. Error shape is opaque though — friction.md **F20**.
- [x] **Ordering and fee invariants as types** — `ActionSet` enforces phase order, the
      one-invoke rule and replay protection; `PoolInvocation` enforces zero tip and zero
      resource prices. 14 tests. Each corresponds to a revert that would otherwise cost a
      proof (~29 s) to discover. *Notable: a deposit alone reverts with `NO_REPLAY_PROTECTION`
      because `Deposit` emits no `WriteOnce` — you cannot shield without also creating a note.*
- [ ] **`apply_actions` submission** — needs a funded Sepolia account and the screening
      decision, so blocked rather than unstarted
- [x] Newtypes for the invariants that must be unrepresentable rather than remembered:
      structured salts only on zero-amount notes (`RandomSalt` vs the wire salt), phase
      ordering (`ActionSet`), `tip == 0` (`PoolInvocation`). Plus `FeltEntropy` for
      constraint 5 and `PoolIdentity` with no key accessor for constraint 6.

**Acceptance:** the client can build, serialise, and sign a complete action set that the TS
oracle agrees with byte-for-byte.

### P1.1 — Shield → private transfer

- [ ] Shield an ERC-20 into the pool
- [ ] Private transfer between two test accounts
- [ ] Recipient finds and decrypts the note — keyed read via `ContractDiscoveryProvider`,
      never a scan
- [ ] Follow simulate → prove → `apply_actions` strictly

*Phase 2 bullets, blocked on the prover:*

- [ ] **Screening bites here.** Any action set containing a `TransferFrom` needs a
      screener-signed attestation fresh within 300 s or reverts with `SCREENING_REQUIRED`.
      Rides along in the proving response's `additionalData`; self-hosting means the
      proof-interceptor sidecar too. Deposit leg only — note-to-note transfers are not gated.
- [ ] Set `provingBlockId = currentBlock - 10` (notes mature 10 blocks; head-based proofs die
      to reorgs). Proofs stay valid 450 blocks on this pool.

**Acceptance:** a script that runs the full shield-and-transfer and prints the receipt.

### P1.2 — Channel establishment

> ~~Derive `channel_key` from both parties' addresses and viewing keys via ECDH over the
> Stark curve~~

Struck because it describes something the pool does not do. `compute_channel_key` is
`h(TAG, sender_addr, sender_private_key, recipient_addr, recipient_public_key)` — a hash
over the *sender's private key*, not a symmetric ECDH secret. The recipient cannot derive
it; they receive it, encrypted under a separate ephemeral ECDH, via
`EncChannelInfo.enc_channel_key` in the on-chain `Append`. That is why channels are
directional.

- [x] Open a channel and its token subchannel — `Channel::open_channel`,
      `Channel::open_subchannel`, and `Channel::setup` which folds registration + channel +
      subchannel into **one action set, one proof** *(`sdk/rs/src/channel.rs`)*. 9 tests.
      `setup` skips registration for a returning identity, since the viewing key is
      immutable and a second `SetViewingKey` reverts on the WriteOnce.
      **Unreviewed — written by Claude.**
- [x] **`FeltEntropy` distinct from `NoteSalt`** — constraint 5 (non-uniform salt types)
      made a compile error rather than a comment. Channel-level `random`/`salt` are
      `felt252` with a non-zero requirement; note salts are 120-bit `u128`. Mixing them was
      the audit's flagged footgun and is now unrepresentable.
- [ ] Counterparty reads it back — needs the `EncChannelInfo` decrypt path, which is
      `discovery-core`'s side of the fence; deferred rather than done
- [ ] Static-static ECDH over the registered Stark-curve viewing keys, for the **off-chain
      transport**. A different secret from the pool's `channel_key` — do not conflate them.
      Now off the critical path, since the salt lane keeps negotiation on-chain.

*Phase 2, blocked on the prover:*

- [ ] Register the channel on Sepolia
- [ ] Verify a third party observing the chain cannot detect the channel exists. **Untestable
      offline** — needs a real chain and a real observer.

**Acceptance:** two accounts share a channel; a third account scanning sees nothing.

### P1.3 — Negotiation state via the salt lane

The original premise was wrong — there is no payload field, so `Offer`/`Counter`/`Accept`
cannot be Cairo structs written into a note. Struck, so the change stays visible:

> ~~Encode `Offer` / `Counter` / `Accept` as Cairo structs~~

The payload rides in note **salts**. Each zero-amount data note carries 119 usable bits; a
message is a fixed run of notes at consecutive indices in the counterparty's subchannel. No
Cairo is written for this — SDK-side encoding plus direct `ClientAction` construction.

**Wire format (v1)**

- Framed message is **400 bits**: `type` 8 + `replyTo` 32 + `createdAt` 40 + `amount` 128 +
  `deadline` 64 + `memoHash` 128. `token` is dropped — the subchannel *is* the token.
  `nonce` is dropped — the note index already orders and uniquely identifies.
- **119 bits per note, not 120.** The contract requires `2 ≤ salt < 2^120`, so a chunk
  landing on 0 or 1 would be rejected. Bit 119 is pinned to 1; payload occupies bits 0–118.
  Salt is then always in `[2^119, 2^120)` and always valid.
- Fixed width: **4 notes per message**. Message *k* is indices `4k .. 4k+3`, so the reader
  needs no framing search.
- All 4 notes go in **one action set** → one proof (~29 s) per negotiation round.

**Rules**

- Data notes carry structured salts. **Value-bearing notes keep random salts** — the salt is
  the one-time-pad nonce for the amount, and reuse across differing amounts leaks their
  difference. Zero-amount notes are immune (no amount variance).
- The accept-commitment is its own zero-amount note in the **same action set** as the
  settlement note. Same proof, so atomicity holds and the payment note keeps its random salt.

**Tasks**

- [x] Encoder/decoder for the wire format, ported to Rust — 11 differential tests against
      the TS oracle *(`sdk/rs/src/wire.rs`, vectors in `tests/fixtures/ts-wire-salts.json`,
      regenerate with `cd sdk/ts && pnpm vitest run tests/gen-wire-vectors.test.ts`)*.
      Mutation-tested: reversing chunk order and dropping the pinned flag both fail loudly.
      Caught a real disagreement on the first run — friction.md **F19**, the TS accepts memo
      hashes above the STARK prime and the Rust rejects them, which bites anyone who passes
      a SHA-256 or Keccak digest. **Needs an interface decision, not a codec fix.**
      **Unreviewed — written by Claude.**
      *Also: the ASCII layout table in `wire.ts` module docs is wrong — it puts the header
      in note 0, but the packing puts it in note 3. The Rust module docs carry the corrected
      table and a test pins it.*
- [x] Direct `ClientAction[]` construction — `Channel::write_message` emits the four
      zero-amount notes at `4k..4k+3` as a validated `ActionSet` *(`sdk/rs/src/channel.rs`)*
- [x] Counterparty read via keyed lookup — `Channel::note_ids_for_message`, computing
      `h(NOTE_ID_TAG, channel_key, token, index, 0)` directly. No scan anywhere in the crate.
      A test asserts the writer's slots and the reader's slots are the same felts, because a
      divergence there is found only as "note missing, no error".
- [x] **`PoolIdentity` holds the pool key with no accessor and a redacting `Debug`** — 11
      tests *(`tests/channel_ops.rs`)*. Constraint 6 becomes structural rather than
      remembered: the policy layer cannot reach key material because nothing returns it.
      Also pins that channels are directional (A→B ≠ B→A) — sharing a key between the two
      directions would put both parties' messages at the same slots.
      **Unreviewed — written by Claude.**
- [x] Contiguous indexing, with a negative test — `SubchannelCursor` is now the single index
      allocator per subchannel *(`sdk/rs/src/subchannel.rs`, 9 tests in
      `tests/index_contiguity.rs`, 3 mutations checked)*. It mirrors **both** contract rules,
      which turn out to be one rule: `INDEX_NOT_SEQUENTIAL` (`privacy.cairo:737-746`) plus
      write-once `NON_ZERO_VALUE` (`:932-946`) means the index space is an allocator, not a
      parameter. **Unreviewed — written by Claude.**

      *Found a real bug doing this.* `_client_apply_actions` (`:755-777`) applies each
      `WriteOnce` as it walks, so the contiguity check on a note **sees notes the same action
      set created earlier**. Emission order inside a set is therefore load-bearing, and
      `accept_and_settle` was pushing the payment note first regardless of its index. The
      old tests passed only because the fixture put the payment below the record. Fixed by
      sorting creations ascending. Inferred from source, not yet observed on-chain — verify
      at P2.0. Written up as **F21**.

- [ ] **DECISION NEEDED — one subchannel is currently one deal.** A message is 4 notes on a
      `4k..4k+3` grid; a settlement's payment note is 1 note. So settling leaves the cursor
      at `4k+1`, off-grid, and nothing further can be written to that subchannel. Fine for
      the MVP. The alternatives if agent pairs are long-lived: pad the payment to a full
      4-slot (3 filler notes, permanently unspendable, indices burned forever), or drop the
      fixed stride and give the reader a framing search. Both cost something real.

- [ ] Enforce the state machine (ARCHITECTURE §4) — no accepting an expired offer.
      **The pool cannot do this for us**; it has no `status`, `deadline` or `replyTo` field.
      Enforcement is client-side, over the decoded notes.

      **Blocked in part, and it is an interface question, not a Rust one.** ARCHITECTURE §4
      lists `withdrawn` as an `OfferStatus` with a `proposed --> withdrawn` transition, but
      nothing can reach it: `ErebusClient` has no `withdrawOffer` method and `MessageType` is
      `Offer | Counter | Accept` with no Withdraw variant. So withdrawal is unreachable in
      wire v1. Either the status comes out of §4, or a fourth message type goes in — and the
      second breaks Ishita's mock. **Take it to P0.3 rather than deciding it in the SDK.**
      Expiry and accept-once are unblocked and land independently.

**Known costs accepted**

- Every data note is permanently unspendable (`use_note` rejects zero amounts) and burns a
  subchannel index forever.
- Channels are directional, so B countering back needs its own channel and subchannel.

**Acceptance:** A writes an offer, B reads it, B counters, A reads the counter.

### P1.4 — Measure proof time

StarkWare publish **~29 s** (12-core / 46 GiB; machine-dependent) for Stwo proof generation.

The unit is the **transaction**, not the action — batching notes into one action set
amortises, but rounds do not. Each offer and each counter is its own tx and its own proof.
Three rounds ≈ 90 s of proving before settlement even starts.

- [x] Ballpark from the vendor — friction.md F7
- [ ] Measure it on our hardware. Theirs is a 12-core/46 GiB box; ours is not.
- [ ] Measure whether it scales with action count — matters for the 4-note message
- [ ] Record the real number in `friction.md` and revise the demo script around it

**Why it matters:** a 2–3 minute recording cannot show several rounds in real time at ~29 s
per proof. Decide with Ishita whether the demo shows fewer rounds, is time-compressed in the
edit, or is honest about the wait.

---

## Phase 2 — on-chain

**No longer gated on anyone else — as of 2026-07-28 this phase is ours to start.** There is
no hosted prover and there may never be one in our timeframe, so the entry cost is standing
up the stack:

- [ ] Pathfinder v0.22.7 synced on Sepolia, `PATHFINDER_STORAGE_STATE_TRIES=10000`
- [ ] `transaction-prover:PRIVACY-0.14.3-RC.2` pointed at it
- [ ] Confirm the RC.2 prover actually matches the deployed pool class (blocker 2)
- [ ] Then resolve screening one way or the other (blocker 3) before the shield can run

The sync is the long pole and nothing about it is intellectually interesting, so start it
early and in the background — it is the one cost here that cannot be compressed by working
harder. *Pushing work into Phase 1 is still right, but the reason has changed: not "Phase 2
may never start" but "Phase 2 now has a setup cost in front of it".*

Beyond that, this phase is the Phase-1 tasks re-run for real: the bullets marked *Phase 2*
under P1.1 and P1.2, plus the first genuine end-to-end `simulate → prove → apply_actions`.
Nothing there is new functionality — it is the same functionality, first contact with a real
chain, real screening, and real proof latency.

---

## Phase 3 — atomicity and disclosure

### P2.1 — Atomic accept + settle
The core novelty. Acceptance and shielded transfer must be one proven state transition.

- [x] Bind the accepted offer to the private transfer, same action set —
      `Channel::accept_and_settle` *(`sdk/rs/src/channel.rs`)*. Spends, then the payment
      note, then the four-note acceptance record, in one `ActionSet` → one proof. 11 tests.
      Rejects a non-`Accept` record, a zero payment, an unfunded settlement, and a payment
      index colliding with the record's range (they share one subchannel index space).
      **Unreviewed — written by Claude.**
- [x] **`RandomSalt` vs structured salt as distinct types** *(`sdk/rs/src/actions.rs`)*.
      Settlement is the first action set mixing value-bearing and data notes, so "structured
      salts only on zero-amount notes" had to stop being a rule and become a signature —
      `value_note` will not accept a wire salt. Mutation-tested.
- [ ] If the proof fails, the acceptance must not have happened — *holds by construction
      (one action set, one proof), but unverified on-chain until the shield works*
- [ ] Return a `SettlementReceipt` matching the frozen interface — waits on P0.3

**Acceptance:** accept-and-settle succeeds atomically; a deliberately invalid proof leaves
state untouched.

### P2.2 — Viewing key disclosure
- [ ] Grant a viewing key to a third party
- [ ] Reconstruct the full record: participants, all offers, settlement
- [ ] Verify no leakage about unrelated users or channels

**Acceptance:** a Kleidouchos account reveals the complete negotiation and payment; a
different account with a different key sees nothing. *(This is DoD #3.)*

---

## Phase 4 — integrate and ship

### P2.3 — Integrate with Ishita's agents
- [ ] Swap her mock for the real client through the P0.4 seam
- [ ] Run the full loop with live agents
- [ ] Fix the inevitable interface mismatches together

**Acceptance:** one green end-to-end run, agent-driven.

---

## Ongoing

- [ ] Log friction in `docs/friction.md` as you hit it — do not batch this at the end, you
      will forget the details
- [ ] Review every line of protocol code before it lands, including anything LLM-generated

---

## Guardrails

- Do not build a frontend.
- Do not implement free-text messaging (ARCHITECTURE §7). The mechanism *could* carry prose;
  at 119 bits and one burned slot per note, it shouldn't.
- Do not add multi-party channels.
- Do not optimize anything before the loop is green.
- Do not change the interface without Ishita.
- Do not let protocol logic leak into `/sdk/py`. It is a binding.
- If you find yourself refactoring the STRK20 primitives themselves, stop — we compose them,
  we don't fork them.
  - **Tension named, not ignored:** the salt lane needs an SDK bypass (upstream hardcodes the
    note salt to `generateRandom120()` and drops it on read). That does not fork the *pool*,
    so it stays inside this guardrail — but it is a bypass, and it should be said out loud
    rather than discovered mid-build.

## Reading

1. **STRK20 by Example** — https://strk20-by-example.org. Primary hands-on reference.
   **No browser needed** — every route mirrors as raw Markdown. Append `.md` to any path, or
   start from `/llms.txt` (page index) or `/llms-full.txt` (whole site, one file). The
   Sepolia pool address and the ~29 s proving figure both come from here.
2. **OpenZeppelin audit** — https://www.openzeppelin.com/news/privacy-contracts-audit. The
   closest thing to an architecture spec that exists publicly. Read the findings too, not
   just the description.
3. **`starkware-libs/starknet-privacy`** — the source. Go here once the above give you the
   model. Cloned to `../starknet-privacy` @ `3dfe66f`. Highest-value files:
   `packages/privacy/src/{objects,actions,privacy,hashes,utils}.cairo`,
   `sdk/src/interfaces.ts`, `sdk/src/internal/compiler.ts`. The README's compatibility matrix
   and `demo/.env*.example` are where the deployment facts live.
4. **`discovery-core`** — the one existing Rust piece. Read side only; it is what your write
   side is the missing half of.
5. Starknet v0.14.2 / SNIP-36 release notes — context on native proof verification.
