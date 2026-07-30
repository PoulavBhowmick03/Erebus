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

## Status — 2026-07-30

**Rust protocol 2 is complete as an implementation, not yet as live evidence.** The crate
now has the high-level `ErebusClient` trait and all seven methods, Rust-owned opaque-handle
state, keyed RPC discovery, the full preflight → prove → estimate → `apply_actions` →
receipt path, exact-note atomic settlement, and self-contained disclosure grants. The full
offline suite is green: **172 passed, 2 deliberately ignored live-prover probes**.

What that sentence does **not** mean:

- no successful proof-carrying transaction has landed on Sepolia from this path yet;
- the shield still needs a real screening attestation;
- writes require an operator-controlled RPC/Pathfinder as well as prover, because the
  `compile_actions` preflight also carries the pool key;
- the Python/TypeScript protocol-1 mirrors were not changed during the Rust-only pass, so
  integration with Ishita is deliberately still P2.3;
- settlement currently selects notes whose values sum exactly to the offer. It refuses to
  burn surplus; general change-note construction remains post-MVP.
- protocol-2 calls have no idempotency token. Chain-derived cursor recovery prevents index
  reuse, but a crash after inclusion and before the response can orphan an open handle or
  cause a retried proposal to become a second proposal.

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

**Blocking. Corrected 2026-07-30 — the prover is no longer one of these:**

1. ~~**No prover endpoint.**~~ **Resolved.** Akash supplied a Sepolia
   `transaction-prover` endpoint after the 2026-07-28 "self-host" answer was written, and
   F20 is the receipt: a live `starknet_proveTransaction` from this crate reached it and
   failed at *execution* (`-32603`), not at parsing, which means the service accepted our
   `INVOKE_TXN_V3` whole. Keep the URL in the gitignored `.env` — he asked that it not be
   shared.

   **So self-hosting is a product decision, not an MVP unblocker,** and the earlier text
   here had that backwards. It is what makes poc.md's "the operator runs their own prover"
   claim literally true, and it removes the fact that the `compile_actions` preflight and
   the prove call both carry the pool private key in the clear to a third party. On testnet
   that is a demo convenience worth *stating* rather than hiding. Doing it is Pathfinder
   v0.22.7 (`PATHFINDER_STORAGE_STATE_TRIES=10000`) + `transaction-prover` on our own box;
   the Pathfinder sync is the long pole, so start it early if we want it.
2. **Which prover / discovery tags match the deployed class hash.** The README matrix pins
   RC.0 → `0x52107f…633`, which is not what is live. Still open, and now testable directly
   against Akash's endpoint rather than by reading the matrix.
3. **How we get a screening attestation.** Still the only hard dependency on someone else,
   and it gates one leg — the shield — not the whole demo.

   **Correcting the framing, because it changes what to do next:** the attestation is not a
   credential we obtain and then present. The prover's `proof-interceptor` sidecar produces
   it — one `starknet_checkTransaction` per client `starknet_proveTransaction`, screened via
   HMAC-signed `POST /screen` to elliptic-proxy, whose allow response *is* the STARK-curve
   signature over the depositor; the prover attaches it under `additional_data.signature`
   for us to pack into the deposit's `apply_actions` calldata (proof-interceptor README).

   That inverts blocker 1's old conclusion. If Akash's prover has `SCREENING_URL`
   configured, deposits already work and we need nothing; self-hosting would *lose* us the
   shield, since our own interceptor has no Elliptic partner secret and with `SCREENING_URL`
   unset degrades to a pass-through that returns `allowed: true` **with no signature**.
   **One test deposit against his endpoint settles it** — an attestation comes back, or
   `10000` does. Fallback if it does not: deploy our own pool instance and hold the screener
   key ourselves (constructor is unpermissioned, class already declared; friction.md F6).

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
- [x] **Get a Sepolia proving-service endpoint from StarkWare — GOT ONE 2026-07-30.** The
      first answer was "none exists, self-host"; Akash then supplied one, and F20 shows it
      parsing our `INVOKE_TXN_V3`. It lives in the gitignored `.env` and is not to be shared.
      Self-hosting (`transaction-prover` + Pathfinder v0.22.7,
      `PATHFINDER_STORAGE_STATE_TRIES=10000`) is now a *product* task — it is what makes the
      no-third-party-sees-your-key claim true — not a Phase-2 blocker.
- [ ] **Resolve screening — one experiment, do this before anything else in Phase 2.** Send a
      real deposit prove request to Akash's endpoint and look at the response: an
      `additional_data.signature` means screening is configured and the shield is already
      unblocked; JSON-RPC `10000` means it rejected us. Only if it fails do we need the
      fallback — our own pool instance with our own screener key (constructor is
      unpermissioned, class already declared). *Gates the shield only.* Cheapest possible
      way to close the last open blocker, so it should not wait behind the ERC-20.
- [ ] **Ask Akash directly, in parallel with the above:** is screening enabled on the prover
      you gave me — will a deposit come back with an attestation, or do I need something from
      your side? One question, saves a day whichever way it goes.
- [ ] Confirm which prover / discovery tags match the **deployed** class hash.
- [x] **Settlement ERC-20 — DECIDED 2026-07-30: deploy our own mintable test ERC-20.**
      The pool has **no token allowlist** — `deposit` calls `TransferFrom` on whatever address
      it is handed (`privacy.cairo:488-499`), so nothing about the architecture depends on
      which token this is. That makes the choice purely about certainty, and a token we mint
      cannot be rate-limited or drained by someone else on demo day. It is also what
      StarkWare's own demo does: `demo/.env.example` ships placeholder addresses with
      `mintEntrypoint` / `permissionedMint` rather than canonical tokens, which suggests they
      hit the same problem.
      *Product note:* because there is no allowlist, pointing this at USDC or STRK later is a
      config change with zero code impact. Confirm with `balanceOf` non-zero over RPC and one
      `approve` that lands.
- [ ] Discovery service: not published either, but **optional** — `ContractDiscoveryProvider`
      reads the pool over plain RPC. Use that for the MVP; revisit only if too slow.
- [x] **Paymaster — DECIDED 2026-07-30: out of scope, post-MVP.** `get_fee_amount()` is 0,
      so the pool charges nothing; `apply_actions` is an ordinary Starknet transaction and
      funded demo accounts cover its gas. STRK20 ships no paymaster and the upstream demo
      wires third-party **AVNU**, optional and unset by default.
      *Why it matters later, not now:* "agents should not have to hold a gas token" is a real
      product requirement and ARCHITECTURE §8 Q4 rests on AVNU to meet it. It changes nothing
      about the protocol, so it is additive after the green light.

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
- [x] **`memoHash` — RESOLVED 2026-07-30: it is 128-bit, not a `felt252`.** §4 declared a
      felt; the wire has only ever carried 128 bits (`sdk/rs/src/wire.rs:156`). F19 was
      entirely that gap — a SHA-256 digest is rejected by the Rust and silently truncated by
      the TS, so the two clients disagree about what the same memo commits to. Declaring the
      real width deletes the truncation step they could disagree about, and makes the 2^64
      collision resistance a visible caller choice. **No Rust change needed; the code was
      already right and the interface was wrong.** ARCHITECTURE §4 updated.
- [x] **`SettlementErrorCode` — FROZEN 2026-07-30.** Grounded in the errors that now exist
      rather than guessed, and grouped by what the caller should *do*, because an agent cannot
      act on twelve distinct codes: do-not-retry (`OFFER_EXPIRED`, `OFFER_UNKNOWN`,
      `ALREADY_SETTLED`, `NOT_YOUR_OFFER`, `AMOUNT_MISMATCH`, `INSUFFICIENT_NOTES`,
      `INDEX_CONFLICT`), retry-may-work (`SCREENING_UNAVAILABLE`, `PROVER_UNAVAILABLE`,
      `PROOF_EXPIRED`, `SUBMIT_FAILED`), terminal (`SCREENING_REJECTED`), and opaque
      (`PROOF_FAILED` — the prover's `-32603` carries no reason, F20). Full list in
      ARCHITECTURE §4.
- [ ] **Walk it with Ishita and freeze.** The two open items above are now decided, so this is
      confirmation rather than negotiation.
- [ ] **Protocol-2 disclosure correction needs shared sign-off.** Rust now returns a
      self-contained `ViewingKeyGrant` and `reveal` consumes it without a local handle.
      The previous `Promise<void>`/`reveal(handle, key)` pair could not deliver a key or work
      on an auditor's machine. ARCHITECTURE §4 records the correction; Ishita's files were
      deliberately not edited in the Rust-only pass.

Already true, so not a task: `OfferTerms` serialises to 5 felts against a 120-bit on-chain
lane — encodable in Cairo, just does not fit in one note.

**Acceptance:** both of you have signed off on the same interface.

### P0.4 — The Python ↔ Rust seam *(shared with Ishita)*

Her MCP server is Python; your client is Rust. Something has to cross. Half yours and half
hers, which is exactly why it will otherwise be nobody's until integration day breaks on it.

- [x] **Mechanism — DECIDED 2026-07-30: subprocess.** `erebus-cli`, JSON on stdin, JSON on
      stdout. Reasoning in ARCHITECTURE §3; the deciding one is that the OS process boundary
      keeps constraint 6 *structural*. In-process, "key material never leaves the SDK
      boundary" degrades to "shares a heap with whatever agent code is loaded", and the
      custody claim made to StarkWare acquires a footnote. Async staying inside Rust and
      encoding failures being loud parse errors rather than silent felt corruption both point
      the same way. Distribution is a wash — PyO3 needed per-platform wheels anyway.
      Costs are in ARCHITECTURE §3 — the tradeoff is yours to make. Bias worth naming:
      subprocess has no build matrix and puts an OS boundary around key material, which
      turns CLAUDE.md constraint 6 from a rule into a property.
- [x] **Rust protocol-2 seam landed 2026-07-30.** `erebus-cli` exposes all seven methods plus
      the administrative `shield` helper. `open_channel` now submits the real setup and
      returns an opaque random handle rather than leaking the channel key. The handle maps
      to a locked, atomic, mode-`0600` Rust state record containing directional keys and the
      note cursor (`sdk/rs/src/state.rs`).

      Both pool and account keys are supplied as file paths and read only inside Rust.
      Entropy is generated inside the binary. The sole intentional secret export is
      `grant_viewing_key`, which returns a self-contained bearer grant for an auditor.

      **Integration is intentionally not ticked:** `sdk/py` still speaks protocol 1. It and
      Ishita's callers were left untouched in this Rust-only pass.
- [x] **`SettlementError` crossing — DONE.** A JSON envelope carries `code`, `message` and
      `retryable`; `ErebusError` on the Python side is a frozen dataclass with those fields.
      `retryable` is the only field agent logic should branch on — an agent cannot act on
      twelve distinct codes but can always act on "is another attempt worth making".
      A broken install raises `SeamUnavailable` instead, deliberately a *different* exception
      type: agent code that treated "wrong binary" as an ordinary protocol failure would
      retry it forever.
      Two codes were added to §4 rather than fudged into existing ones: `INVALID_REQUEST`
      and `IDENTITY_UNAVAILABLE`, both seam-level and both non-retryable.
- [ ] Keep `/sdk/py` free of protocol logic. A hash, a felt conversion, or a salt encoder
      there is the bug this architecture is arranged to prevent.

**Acceptance:** protocol 1 met this once; protocol 2 intentionally reopens the shared half.
Rust returns typed results/errors for the complete surface, but Python does not yet marshal
the new config, opaque state, or self-contained viewing grant.

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
- [x] **End-to-end composition pinned** — the upstream fixture still pins signature bytes,
      and `calldata.rs` / `execution.rs` now start from an actual `ActionSet`. This closes
      the old fixture gap where the test accepted prebuilt calldata and therefore did not
      exercise `ActionSet → compile_actions → __execute__`.
- [ ] **Which `assert_valid_signature` route the pool actually takes** — `utils.cairo:383`
      tries three (custom validation, tx hash, SNIP-12 `CallSet` hash) and which one succeeds
      depends on the agent's account contract, not on us. Untestable offline; needs a real
      account on Sepolia.
- [x] **`starknet_proveTransaction` client** — async on reqwest/rustls, retries only on
      transport failures, HTTP 503 and the service's `-32005` busy response
      *(`sdk/rs/src/prover.rs`)*. **Verified live against Sepolia:**
      `spec_version` returns `0.10.3-rc.2`, and a `prove_transaction` call reaches execution
      (`-32603`) rather than being rejected as malformed (`-32602`), which live-validates the
      whole `INVOKE_TXN_V3` serialization. Error shape is opaque though — friction.md **F20**.
- [x] **Ordering and fee invariants as types** — `ActionSet` enforces phase order, the
      one-invoke rule and replay protection; `PoolInvocation` enforces zero tip and zero
      resource prices. 14 tests. Each corresponds to a revert that would otherwise cost a
      proof (~29 s) to discover. *Notable: a deposit alone reverts with `NO_REPLAY_PROTECTION`
      because `Deposit` emits no `WriteOnce` — you cannot shield without also creating a note.*
- [x] **`apply_actions` submission implemented** — proof output extraction, screening
      `Option` suffix, account `Array<Call>`, query-version fee estimation with a 50% bound
      buffer, proof/proof-facts wire fields, custom facts-aware signature, submission and
      accepted/reverted receipt polling. `tests/execution_pipeline.rs` pins the complete
      request sequence against local JSON-RPC fixtures. **Still unverified live.**
- [x] Newtypes for the invariants that must be unrepresentable rather than remembered:
      structured salts only on zero-amount notes (`RandomSalt` vs the wire salt), phase
      ordering (`ActionSet`), `tip == 0` (`PoolInvocation`). Plus `FeltEntropy` for
      constraint 5 and `PoolIdentity` with no key accessor for constraint 6.

**Acceptance:** the client can build, serialise, and sign a complete action set that the TS
oracle agrees with byte-for-byte.

### P1.1 — Shield → private transfer

- [x] **Shield action construction** — `Channel::shield` creates register (when needed) +
      self-channel + token subchannel + deposit + exact-value encrypted note in one balanced,
      replay-protected `ActionSet`. `Client::shield` runs it through the common executor.
      Live success remains screening-blocked.
- [ ] **Standalone private transfer between test accounts.** The settlement value leg is
      implemented: it consumes discovered notes and creates the counterparty payment note.
      There is no separate transfer helper/script, and no live transfer has landed. The MVP
      selector finds an exact subset and refuses surplus rather than destroying it.
- [x] **Recipient keyed discovery** — `client.rs` reads `get_num_of_channels`,
      `get_channel_info`, `get_subchannel_info`, exact computed note ids, and nullifier
      existence. No event/world scan.
- [x] **One mandatory executor** — same historical block for `compile_actions` preflight and
      proof, then `apply_actions`; no public path submits `__execute__`.

*Phase 2 bullets, blocked on the prover:*

- [ ] **Screening bites here.** Any action set containing a `TransferFrom` needs a
      screener-signed attestation fresh within 300 s or reverts with `SCREENING_REQUIRED`.
      Rides along in the proving response's `additionalData`; self-hosting means the
      proof-interceptor sidecar too. Deposit leg only — note-to-note transfers are not gated.
- [x] Set `provingBlockId = currentBlock - 10` (notes mature 10 blocks; head-based proofs die
      to reorgs). Proofs stay valid 450 blocks on this pool. Each channel record persists the
      block of its last accepted write; dependent writes wait until that block is visible at
      `head - 10`, and settlement discovers spendable notes at that historical anchor rather
      than selecting fresh notes the proof cannot see.

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
- [x] **Read side implemented** — `sdk/rs/src/decrypt.rs` (5 decrypt functions, 12 KATs in
      `tests/decrypt_conformance.rs`) and `sdk/rs/src/read.rs` (`ChannelReader`, transcript
      walk, both-direction `reconstruct`, 9 tests in `tests/read_path.rs`). 4 mutations
      checked. **Unreviewed — written by Claude.**

      *Decision recorded: implemented rather than importing `discovery-core`.* It pins
      `starknet-core`/`crypto`/`providers` to a `software-mansion/starknet-rust` fork by git
      rev (`7caedfe`) and pulls in `futures`, `async-trait`, `url`. What it would have given
      us is field subtraction plus one ECDH — the five Poseidon masks were already in our
      `hashes.rs`, already pinned. Cairo is the source of truth for both, so this is not a
      second authority.

      *Found a real bug.* See **F22**: note indices are per-direction, so a message has no
      identifier. The state machine was keyed on the bare index and passed every test until
      both directions were reconstructed into one book. Would have settled against terms the
      counterparty never sent, with a valid proof and no revert. Now keyed on
      `OfferId { author, index }`.

- [x] Counterparty read path is wired to pool views — incoming `EncChannelInfo` is decrypted,
      the token subchannel is verified, and notes are fetched only at derived ids through
      `get_note`. This is implemented in `client.rs`; a live two-account read remains part
      of Phase 2 evidence.
- [x] Reverse-direction pairing is explicit local state. The Rust store excludes incoming
      keys already claimed by other handles and accepts exactly one remaining channel for
      the same counterparty/token. Zero means `ChannelNotReady`; more than one fails
      ambiguous instead of guessing. Concurrent first-time pairing across two handles is
      not yet serialized by a global identity lock.
- [ ] ~~Counterparty reads it back — needs the `EncChannelInfo` decrypt path, which is~~
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

- [x] **DECIDED 2026-07-30: accept it — one channel is one deal, and it costs nothing.**
      A second deal opens a new channel, and `OpenChannel`/`OpenSubchannel` are phases 1 and 2
      while note creation is phase 5 — so **the setup rides in the same action set as the next
      deal's first offer.** Zero extra proofs, zero extra latency. The alternatives were both
      worse: padding the payment to a full 4-slot burns three permanently unspendable notes
      per settlement forever, and dropping the fixed stride forces a framing search on every
      read. Original analysis kept below.

- [ ] ~~**DECISION NEEDED — one subchannel is currently one deal.**~~ A message is 4 notes on a
      `4k..4k+3` grid; a settlement's payment note is 1 note. So settling leaves the cursor
      at `4k+1`, off-grid, and nothing further can be written to that subchannel. Fine for
      the MVP. The alternatives if agent pairs are long-lived: pad the payment to a full
      4-slot (3 filler notes, permanently unspendable, indices burned forever), or drop the
      fixed stride and give the reader a framing search. Both cost something real.

- [x] Enforce the state machine (ARCHITECTURE §4) — `OfferBook`
      *(`sdk/rs/src/negotiation.rs`, 10 tests in `tests/negotiation_state.rs`, 3 mutations
      checked)*. Enforces: expiry by deadline, settle-once, you cannot accept your own offer,
      an `Accept` is not itself acceptable, and a `reply_to` pointing at a message that was
      never seen is refused at record time. **Unreviewed — written by Claude.**

      **These rules have no backstop.** Everywhere else in the SDK a mistake reverts on-chain
      — a bad index, a malformed set. Here the pool has no `status`, `deadline` or `replyTo`,
      so a settlement against a week-old offer proves and applies exactly as cleanly as a live
      one. A rule not in that file is not a rule. Worth weighting the review accordingly.

      *Boundary chosen: `now > deadline` expires, not `>=`.* An offer good "until 12:00" is
      good at 12:00. Pinned by a test because it is the kind of thing that silently flips.

- [x] **DECIDED 2026-07-30: `withdrawn` is cut from the interface, on product grounds.**
      Not because it was unimplemented — because it cannot be made to work. Notes are
      write-once, so an offer cannot be deleted; a retraction has to be a *new* message, and
      that races with no ordering guarantee. A writes the retraction, B has already built a
      settlement against the original, B's proof applies, and A is bound to terms it tried to
      withdraw. Withdrawal is therefore advisory — it works only where the counterparty
      voluntarily checks first, and not depending on that is the whole point of atomic
      settlement. A short `deadline` gives the same capability with no race, because the expiry
      travels inside the offer. Shipping `withdrawn` would advertise a guarantee the
      settlement layer cannot keep. ARCHITECTURE §4 and the state machine updated.

- [ ] ~~**DECISION NEEDED (P0.3) — `withdrawn` is unreachable.**~~ ARCHITECTURE §4 lists it as an
      `OfferStatus` with a `proposed --> withdrawn` transition, but `ErebusClient` has no
      `withdrawOffer` and `MessageType` is `Offer | Counter | Accept`. So it cannot happen in
      wire v1. Either the status comes out of §4, or a fourth message type goes in — and the
      second breaks Ishita's mock, which is exactly what the interface freeze exists to
      prevent. `OfferStatus` in the Rust deliberately omits it rather than carrying a variant
      nothing can construct. Ishita has been told not to mock it (`ishita.md`).

- [x] **High-level methods now use the state machine** — `propose_offer`, `counter_offer`
      and `read_channel_state` are implemented on `Client`, exposed by the Rust trait and
      protocol-2 CLI, and recover their outgoing cursor from chain state before writing.
      Offer ids include handle + direction + index, so an id from another handle or the
      wrong direction fails before proving.

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

**Not gated on anyone else, and — corrected 2026-07-30 — not gated on standing up our own
stack either.** The 2026-07-28 version of this section said there was no hosted prover, so
the entry cost was a Pathfinder sync. Akash then gave us an endpoint (blocker 1), which
means Phase 2 starts with an HTTP call, not a week of syncing.

The ordering that falls out, cheapest-first:

- [ ] One deposit prove request against Akash's endpoint — settles screening (blocker 3) and
      exercises the RC-version question (blocker 2) in the same round trip
- [ ] Fund a Sepolia account with STRK for gas
- [ ] Deploy the mintable test ERC-20 (P0.1)
- [ ] First genuine end-to-end `simulate → prove → apply_actions`

Self-hosting stays on the list, but as its own track rather than a prerequisite, and its
justification is custody rather than availability — the preflight and the prove call both
hand the pool private key to whoever runs the prover:

- [ ] Pathfinder v0.22.7 synced on Sepolia, `PATHFINDER_STORAGE_STATE_TRIES=10000`
- [ ] `transaction-prover:PRIVACY-0.14.3-RC.2` pointed at it — confirm the tag against the
      deployed class hash first (blocker 2)

The sync is the long pole and nothing about it is intellectually interesting, so if we want
it for the demo, start it early and in the background — it is the one cost here that cannot
be compressed by working harder. **But note it does not get us the shield**: our own
interceptor has no screener key (blocker 3), so a self-hosted prover is strictly worse for
deposits than Akash's. *Pushing work into Phase 1 is still right, and the reason is now
simply that on-chain iteration is slow at ~29 s a proof.*

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
- [x] Return a `SettlementReceipt` with offer id, transaction hash, spent nullifiers and
      proving block. State is committed only after an accepted successful receipt; a failed
      preflight/proof/submission does not advance the local cursor or mark the handle settled.
      The accepted block is committed too, so the next dependent proof waits for maturity.

**Acceptance:** accept-and-settle succeeds atomically; a deliberately invalid proof leaves
state untouched.

### P2.2 — Viewing key disclosure
- [x] Grant a viewing key to a third party — `Channel::grant_viewing_key` produces a
      `ViewingGrant` carrying the **two channel keys**, never a pool private key. Serializable
      because granting means handing it over; redacting `Debug` so it cannot land in a log.
      The protocol-2 return is self-contained (`channel_id`, grantee metadata, versioned
      checksummed bearer key), so the auditor needs no grantor-local state directory.
- [x] Reconstruct the full record: participants, all offers, settlement —
      `disclosure::reveal` *(`sdk/rs/src/disclosure.rs`, 12 tests in `tests/disclosure.rs`)*.
      Attributes every message to an address, and keeps `agreed_amount` and `paid_amount`
      separate so an auditor can check they match. **Unreviewed — written by Claude.**
- [x] Verify no leakage about unrelated users or channels — pinned three ways: a grant for
      A↔B reveals nothing of A↔C, nothing of a second token between the same parties, and
      confers no spending authority (a nullifier needs the pool key, which no grant carries).

      **The scoping is real and it is better than the pool's own.** STRK20's `SetViewingKey`
      escrows your *pool private key* to a single pool-wide auditor at registration —
      all-or-nothing, every channel, forever (`privacy.cairo:329-334`). An Erebus grant is a
      channel key: one relationship, one token. Worth correcting in `poc.md`, whose
      Disclosure paragraph currently claims "nobody else learns anything", which is not true
      of the pool auditor.

- [x] **A half grant is rejected, not partially disclosed.** Granting only the direction you
      derived yourself leaves the acceptance replying to an invisible counter, and
      `reveal` errors rather than returning a plausible-looking partial record. Recorded as a
      deliberate choice: for a compliance path, a record that quietly omits what the
      counterparty said is worse than no record. Serialized-field corruption is also caught
      by the grant checksum before reading. Revisit if an auditor ever legitimately holds one
      direction only.

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
