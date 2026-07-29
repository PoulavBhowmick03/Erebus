# Friction log

Where the STRK20 stack fought us. Logged as we hit it, not batched at the end.

Source under investigation: `starkware-libs/starknet-privacy` @ `3dfe66f`
(2026-07-22, `feat(client): client.submit over the PrivacyWallet seam (#914)`).
Cloned to `../starknet-privacy` (sibling of this repo, not vendored in).

---

## F1 — A note has no payload field (P0.2)

> **Revised 2026-07-26.** This entry originally concluded "notes cannot carry a
> structured payload." That overstates it, and we disproved it ourselves by shipping a
> codec that does. There is no payload *field* — that part stands. But the salt is
> client-chosen, round-trips verbatim, and notes are unbounded in count, so arbitrary
> payloads are carryable by fragmentation at 119 bits each. The constraint is cost, not
> possibility. Read the sections below with that correction in mind.

**Status:** answered from source. Probe written, not yet run (no local scarb/snforge).

### What we were trying to do

Write the ARCHITECTURE.md §4 `OfferTerms` struct into a subchannel as a negotiation
state transition — `proposeOffer` / `counterOffer` writing an offer note the
counterparty reads and decrypts.

### What the stack does instead

**No. The write path is shaped around payment amounts only.** There is no payload
field anywhere on it — not in the Cairo note, not in the client action, not in the
SDK's public surface. Three independent layers say the same thing.

**1. The note has two fields, and one of them is unused for encrypted notes.**

`packages/privacy/src/objects.cairo:89-100`:

```cairo
pub struct Note {
    /// The packed value of the note `(salt, amount)`
    pub packed_value: felt252,
    /// The token address of the note (zero for encrypted notes).
    pub token: ContractAddress,
}
```

`packed_value = pack(salt, enc_amount)` — 120 bits of salt, 128 bits of encrypted
amount (`utils.cairo:288-301`, `utils.cairo:336-350`). And `create_enc_note` writes
only that one felt: *"Only `packed_value` needs to be written to storage, `token` is
initialized to zero"* (`privacy.cairo:664-666`). A note is one field element.

**2. `ClientAction` is a closed enum. There is no "write data" action.**

`packages/privacy/src/actions.cairo:246-273` — ten variants: `SetViewingKey`,
`OpenChannel`, `OpenSubchannel`, `CreateEncNote`, `CreateOpenNote`, `Deposit`,
`UseNote`, `Withdraw`, `InvokeExternal`, `ComputeAndInvoke`. The two that create
notes take `amount: u128` and nothing else:

```cairo
pub struct CreateEncNoteInput {
    pub recipient_addr: ContractAddress,
    pub recipient_public_key: felt252,
    pub token: ContractAddress,
    pub amount: u128,
    pub index: usize,
    pub salt: u128,      // must be > 1 and < 2^120
}
```

This is not bypassable by hand-rolling server actions. `apply_actions` recomputes the
message hash over the exact `Span<ServerAction>` submitted and requires it to appear
in the transaction's `proof_facts.message_to_l1_hashes` (`privacy.cairo:804-839`).
The server actions must be the ones the proven client simulation emitted, and the
client simulation only emits what the enum above produces. The generic
`ServerAction::WriteOnce { storage_address, value: Span<felt252> }` looks like an
escape hatch but its `storage_address` is always derived by the contract, never
supplied by the caller.

**3. A "subchannel" is a token, not a topic.**

`subchannel_marker = h(TAG, channel_key, recipient_addr, recipient_public_key, token)`
(`hashes.cairo:185-198`), and `open_subchannel` writes that marker through `WriteOnce`
(`privacy.cairo:473-478`), which asserts the slot is currently zero
(`privacy.cairo:932-947`). So: **one subchannel per (channel, token) pair, enforced.**
Opening a second subchannel for the same token reverts with `NON_ZERO_VALUE`. The
SDK's own type says it plainly (`sdk/src/internal/channel.ts:19-22`):

```typescript
export type TokenChannel = {
  tokenIndex: number;   // identifies which subchannel
  noteNonce: number;    // next note index
};
```

You cannot allocate subchannels as message lanes. Extra lanes cost extra ERC-20
addresses.

**4. The SDK narrows it further.**

`sdk/src/interfaces.ts:199-202` — the entire public note-creation surface:

```typescript
export type CreateNoteAction = {
  recipient: StarknetAddressBigint;
  token: StarknetAddressBigint;
} & ({ amount: Amount } | { amount: Open });
```

No salt, no payload. The salt is generated internally and unconditionally —
`salt: generateRandom120()` at `sdk/src/internal/compiler.ts:424`, with no override
hook. And the SDK's `Note` type (`sdk/src/interfaces.ts:77-85`) has no salt field:
`contract-discovery.ts:193` reads `packedValue >> 128n` and then throws it away.
So even the 120 bits that *are* writable are inaccessible through the public SDK in
both directions.

### The one thing that is writable

The note salt is a real 120-bit lane. The sender chooses it, the contract writes it
verbatim into the high 120 bits of `packed_value`, and the recipient recovers it with
`get_note(note_id)` — a keyed read (`note_id = h(TAG, channel_key, token, index, 0)`),
not a scan, so it is compatible with the no-chain-scanning constraint.

Two properties that make it usable, both confirmed in source:

- **Zero-amount notes are legal.** `CreateEncNoteInputValid` explicitly permits it:
  *"Zero `amount` is allowed to enable note creation on reverted transaction indexes"*
  (`actions.cairo:108-109`). The balance ledger nets to zero, so a data-only note
  needs no deposit and no funds.
- **They are permanently unspendable.** `use_note` rejects a zero-amount note with
  `ZERO_NOTE_AMOUNT_USAGE` (`privacy.cairo:614`). A data note consumes its subchannel
  index forever and can never be nullified.

Capacity, measured:

| Lane | Bits | Frequency | Reader |
|---|---|---|---|
| `EncSubchannelInfo.salt` (felt252, only non-zero enforced) | ~252 | once per (channel, token) | counterparty — `subchannel_id` is derivable from `channel_key` |
| `Note` salt (u128, `1 < salt < 2^120`) | 120 | once per note, unbounded notes | counterparty — `note_id` is derivable |
| `EncOutgoingChannelInfo.salt` (felt252) | ~252 | once per channel | **sender only** — id derives from `sender_private_key`, useless as a channel |

A serialized `OfferTerms` is 5 field elements (~760 bits of real content). That is
7 notes at 120 bits each, or 1 note if you carry a truncated commitment instead of
the terms.

### Workarounds, and what each costs

Not picking one — these are the options with their measured costs.

**A. Smuggle the payload through note salts.**
Zero-amount data notes, 120 bits each, contiguous indices in the token subchannel.

- Costs: forking or bypassing `compiler.ts` (the salt is hardcoded to
  `generateRandom120()`) and reading `packed_value` off the pool directly, since the
  SDK's `Note` type drops the salt. We stop being a plain SDK consumer.
- Costs: the salt's stated purpose is one-time key usage — it is the encryption nonce
  for the amount, guarding against index reuse after a revert. A structured salt
  weakens exactly that: retry the same index with a structured salt and a different
  amount, and the difference of the two `packed_value`s leaks the amount delta.
  This is a real privacy regression, not a theoretical one.
- Costs: ~7 notes per full offer, or 1 note per round if we carry a commitment and
  keep the terms off-chain. Each note is one WriteOnce + one storage felt + its share
  of proof time. Every data note is permanently unspendable dead weight in the
  subchannel index space.
- Buys: nothing leaves the pool. The channel-privacy story is unchanged.

**B. `InvokeExternal` / `ComputeAndInvoke` into an Erebus channel-state contract.**
Arbitrary calldata, arbitrary struct, atomic with the settlement in one proof. This is
how the Ekubo/Vesu/sub-account anonymizers compose (`packages/*_anonymizer`).

- Costs: **the calldata is public.** Upstream says so directly at
  `privacy.cairo:970-975` — *"calldata is intentionally not emitted, as it is already
  visible in the public call trace"* — and `ExternalContractInvoked` emits
  `contract_address` and `selector` as indexed keys (`events.cairo:81-91`). We would
  have to encrypt the payload client-side under `channel_key` and publish ciphertext.
- Costs: even with ciphertext, the invoked contract address is public. The claim
  degrades from "the channel's existence is hidden" to "a transaction with the Erebus
  contract happened; contents hidden". Every Erebus tx becomes self-identifying and
  the anonymity set collapses to Erebus users, not pool users. README's
  relationship-privacy table would need rewording.
- Costs: **at most one invoke-phase action per transaction, and it must be last**
  (`actions.cairo:306-315`: `INVOKE_PHASE` advances `curr_phase` past every other
  phase, so a second one hits `ACTIONS_OUT_OF_ORDER`).
- Costs: **an invoke alone is not a valid transaction.** `main` asserts
  `has_replay_protection` (`privacy.cairo:307`), which only a `WriteOnce` sets. Every
  offer round must also open a subchannel, create a note, or spend one.
- Costs: the target must implement `privacy_invoke` returning `Span<OpenNoteDeposit>`,
  and we now own and must review a second Cairo contract.
- Buys: unbounded structured payload; atomicity with settlement in one proof;
  a real place to enforce the offer state machine on-chain.

**C. Off-chain payload, on-chain commitment.**
Negotiation rounds move over an off-chain transport keyed off `channel_key`
(ARCHITECTURE §7 already calls this out as further work); only a commitment to the
accepted offer is bound on-chain at settlement, via A's salt lane or B's calldata.

- Costs: negotiation stops being on-chain state transitions. ARCHITECTURE §7's claim —
  *"negotiation means structured state transitions written into subchannels"* — becomes
  false as written, and the demo's novelty shifts from "negotiate privately on-chain"
  to "settle privately against an off-chain agreement". That framing needs to go to
  StarkWare before the demo, not during it (§7 already flags this exact risk).
- Costs: via the salt lane, the commitment is truncated to 120 bits — 60-bit birthday
  resistance. Fine or not fine is a judgment call about what the commitment is
  defending against.
- Costs: we build the transport. It is currently out of scope and unbuilt.
- Buys: the payload constraint stops mattering; offer structure becomes free.

### Resolution — decided 2026-07-25: the salt lane

We took workaround A. Negotiation payload rides in the salts of zero-amount data notes,
written into the counterparty's subchannel on-chain.

**Why not the other two.** `InvokeExternal` publishes its calldata and emits the target
contract address as an indexed event key, so every Erebus transaction becomes
self-identifying and the anonymity set collapses from "pool users" to "Erebus users".
Off-chain transport moves the negotiation graph to whoever runs the transport — which is
precisely the leak the README says the project exists to fix — and it breaks `reveal`,
because a viewing key is pool key material and cannot reconstruct off-chain offers.
The salt lane keeps both properties.

**One earlier objection of ours turned out to be wrong.** We flagged salt-reuse as a
reason not to touch the salt at all. It only bites on notes whose amount varies:
`enc_amount = (H(…, salt) + amount) mod 2^128` is an additive one-time pad, so reusing a
mask across two *different* amounts lets an observer subtract the ciphertexts and recover
the difference. Data notes are always `amount = 0`, so there is no variance and nothing to
learn. The rule is narrow: **structured salts on data notes, random salts on value notes.**

**Wire format v1.** 320 bits per message after compression (`token` dropped — the
subchannel is the token; `nonce` dropped — the note index orders and uniquely
identifies). 119 usable bits per note, not 120: the contract requires
`2 ≤ salt < 2^120`, so bit 119 is pinned to 1 and payload occupies bits 0–118, keeping
every salt in `[2^119, 2^120)`. Fixed width at 4 notes per message (1 header + 3
payload), all in one action set, one proof per round.

**Costs accepted.** ~4 notes per message. Every data note is permanently unspendable and
burns a subchannel index forever. We bypass the SDK builder and construct
`ClientAction[]` directly, because the salt is hardcoded on write and discarded on read.

### What would have made it easier

An optional `payload: felt252` (or `Span<felt252>`) on `CreateEncNoteInput`, written
alongside `packed_value` and returned by `get_note`. The storage and proof machinery
already handles multi-felt `WriteOnce` values — `_apply_write_once` loops over
`Span<felt252>` (`privacy.cairo:936-946`) — so the primitive exists; it is just not
exposed on any note-creating action. Failing that: an SDK hook to supply the note salt
and to surface it on the returned `Note`, which would make workaround A a supported
path instead of a fork.

---

## F2 — Salt types are genuinely not uniform (confirms the audit finding)

CLAUDE.md constraint #5, verified against source. The two salts differ in type *and*
in validation, and they sit two lines apart in the same file:

| Function | Salt type | Validation |
|---|---|---|
| `compute_enc_token_hash` (subchannel) — `hashes.cairo:80` | `felt252` | non-zero only |
| `compute_enc_recipient_addr_hash` (outgoing channel) — `hashes.cairo:102` | `felt252` | non-zero only |
| `compute_enc_amount_hash` (note) — `hashes.cairo:216` | `u128` | non-zero, `> 1`, `< 2^120` |

Anything off-chain that models "salt" as one type will produce a mismatched hash at
one of these call sites and the note will silently fail to locate or decrypt. The
120-bit bound is enforced at two independent places — `assert_valid`
(`actions.cairo:114-115`) and `unpack`'s sanity check (`utils.cairo:348`) — so getting
it wrong on the note path at least fails loudly. The felt252 sites have no such guard.

**Impact on us:** any Erebus-side salt derivation must be per-call-site, not shared.
No `deriveSalt()` helper used everywhere.

---

## F3 — Toolchain (resolved) and the probe result

`asdf` was already present, so `starkup` (which rewrites shell rc files) was avoided:

```sh
asdf install scarb 2.17.0
asdf install starknet-foundry 0.59.0     # pulls universal-sierra-compiler 2.9.1
```

Note the machine already had scarb 2.20.0 / snforge 0.62.1 globally. The upstream
`.tool-versions` pins 2.17.0 / 0.59.0 and asdf honours it inside that checkout, so the
newer globals are harmless — but anything we build in `/contracts` should pin the same
versions or we will hit skew.

**The F1 probe passes.** Copied into `packages/privacy/src/tests/`, registered in
`tests.cairo`, `snforge test p0_2`:

```
[PASS] probe_offer_terms_does_not_fit_in_a_note
[PASS] probe_note_salt_rejects_a_full_felt_payload
[PASS] probe_note_salt_is_a_120_bit_payload_lane
Tests: 3 passed, 0 failed, 0 ignored, 343 filtered out
```

So F1 is no longer a source reading. Empirically, against the audited contract:
a zero-amount encrypted note is accepted with no deposit and no balance; its salt
round-trips verbatim through storage; and one bit over 2^120 is rejected with
`SALT_EXCEEDS_120_BITS`. The 120-bit lane is real and it is the only one.

Minor friction: the upstream test harness (`Test`, `User`, `PrivacyCfg`) is
`#[cfg(test)]`-gated inside `packages/privacy`, so a probe like this cannot live in our
own repo and run against theirs — it has to be copied into their tree. Fine for a
one-off; it would be annoying if we needed a standing conformance suite.

---

## F8 — The SDK is not on npm; we depend on a sibling checkout (P1.1)

`@starkware-libs/starknet-privacy-sdk` is published to GitHub Packages, not npmjs, and
the docs note npmjs access is "temporarily unavailable". Options were a `.npmrc` with a
GitHub token, or a path dependency. We took the path dependency:

```json
"@starkware-libs/starknet-privacy-sdk": "file:../../../starknet-privacy/sdk"
```

which means **the build only works if `starkware-libs/starknet-privacy` is cloned as a
sibling of this repo and built once** (`cd sdk && npm ci && npm run build`). That is
fragile and will bite anyone cloning Erebus fresh. Fix before handoff: either vendor a
tarball, or add the `.npmrc` + token path and document it.

Upstream builds clean, at least — `npm ci && npm run build` worked first try on Node 22.

## F9 — Builder API is not what the docs imply (P1.1)

The fluent builder reads like `deposit(500n).to(addr)`, and the strk20-by-example
snippets encourage that reading. It isn't. Both `deposit` and `transfer` are variadic
over *objects*:

```typescript
.with(token).deposit({ recipient: addr, amount: 500n })
.with(token).transfer({ recipient: bob, amount: 300n })
```

Getting it wrong fails at runtime, not compile time — `transfer(10n)` typechecks and
then dies inside `builders.ts` with `Cannot convert undefined to a BigInt`, which points
at the SDK's internals rather than at your call site. Cost us two iterations. The real
shapes are `DepositInput`/`TransferOutput` in `sdk/src/interfaces.ts:569-574`.

## F10 — Channel setup is manual unless you find the auto flags (P1.1)

Done by hand, getting to the point where a note can be written to a counterparty is
five steps: recipient `register()`, sender `setup(recipient)`, `discoverChannels` to
seed a registry, `.with(token).setup(recipient)` for the subchannel, `discoverChannels`
again for the token info. The re-discovery is the irritating part — the registry does
not self-populate from actions you just submitted.

**But it collapses.** With `autoRegister` + `autoSetup` + `autoDiscover` set, a *cold*
deposit (unregistered user, no channel, no subchannel, no notes) is **one action set**,
and a transfer to a brand-new counterparty is **one action set**. Measured:

```
DEPOSIT action-sets:  1
TRANSFER action-sets: 1
bob received: 300n
```

This works because phase ordering lets `SetViewingKey`(0), `OpenChannel`(1),
`OpenSubchannel`(2), `Deposit`(3), `UseNote`(4) and `CreateEncNote`(5) share a single
action set, and the whole thing rides one proof.

Consequence for the demo: cold shield + settle is **2 proofs, ~58 s** (F7), not five
sequential transactions. Worth knowing before we design the recording around it.

Friction is that this is discoverable only by reading `ExecuteOptions` — the by-example
docs show the manual sequence, so the obvious path is the slow one. Both are in
`sdk/ts/tests/pool-flow.test.ts`: the explicit version documents which pool actions are
really involved, the collapsed one is what we would ship.

## F11 — `execute()` does not execute, and `salt` is called `random` (P1.3)

Two naming traps hit while building the salt-lane encoder. Both cost a debug cycle.

**`MockPoolContract.execute()` simulates.** It calls `compile_actions`, which snapshots
state, applies the server actions so later actions in the same set can see them, then
restores the snapshot in a `finally` — because it models a view function. Nothing lands
until you call `apply_actions([...serverActionTypes, "0x1"])` separately.

This is *correct* — it mirrors the real simulate → prove → submit split faithfully, and
the gap between the two calls is exactly where proof generation goes on Sepolia. But a
method called `execute` that leaves no trace produces a baffling first failure: the write
appears to succeed and the read finds nothing.

(The trailing `"0x1"` is Serde for `Option::None` — no screening attestation. Correct for
data notes, which contain no `TransferFrom`.)

**The salt is called `random` inside the mock.** The client action field is `salt`, Cairo
calls it `salt`, but `MockPoolContract.createEncNote` takes it as `random: bigint` and
forwards it to `encryptNoteAmount(channelKey, token, index, random, amount)`. Meanwhile
`CreateOpenNoteInput` has a genuinely different field that is *actually* called `random`.
So the same name means two different things depending on which note type you are looking
at, and the same concept has two names.

This is the same class of problem as F2 (salt types not uniform). Anyone reading the mock
to understand the write path will mis-map the fields.

## F12 — Domain tags do not fit in a u128, and nothing tells you (Rust port)

First bug of the Rust port, and a clean specimen of the failure mode this protocol has
everywhere.

Cairo short-string literals like `'CHANNEL_KEY_TAG:V1'` are felts — up to 31 bytes. The
first draft of `hashes.rs` accumulated them into a `u128`:

```rust
acc = (acc << 8) | bytes[i] as u128;   // silently drops the high bytes
```

Rust does not panic here. `<<` only panics when the *shift amount* exceeds the width, not
when bits fall off the top. So every tag of 16 bytes or fewer encoded correctly and every
longer one was quietly truncated.

The split was exact: `NOTE_ID_TAG:V1` (14), `NULLIFIER_TAG:V1` (16) and
`ENC_TOKEN_TAG:V1` (16) passed. `CHANNEL_KEY_TAG:V1` (18), `SUBCHANNEL_MARKER_TAG:V1`
(24), `OUTGOING_CHANNEL_ID_TAG:V1` (26) and the rest failed. 4 passed, 8 failed.

Had this shipped without KATs, the symptom on Sepolia would have been: notes write
successfully, and the counterparty finds nothing at the note id it derives. No error, no
revert — a storage slot nobody wrote to. Debugging that against a ~29 s proof loop with
no local prover would have cost a day, easily.

**This is the argument for wiring the conformance harness before writing anything else.**
It caught the bug on the first `cargo test`, before a single line of the port had run
against a network. Fix is to right-align the bytes in a 32-byte buffer and use
`Felt::from_bytes_be`.

## F13 — There is no Python path to Starknet privacy, and the adjacent ecosystem doesn't provide one either (2026-07-28)

Not the STRK20 stack fighting us directly, but a gap in the surrounding ecosystem that
shaped our architecture, so it belongs here.

**What we were trying to do.** Put the agent layer in Python — the natural language for
agent frameworks, and the language our agent-side developer works in. Then reach the
privacy pool from it.

**What the ecosystem offers.** Nothing that connects those two ends:

| | Python? | Starknet? | Both? |
|---|---|---|---|
| `starkware-libs/starknet-privacy` SDK | no — TypeScript | yes | — |
| `discovery-core` (same repo) | no — Rust, and read-side only | yes | — |
| MCP (`mcp`, official) | yes, first-class | n/a | — |
| x402 (`pip install x402`, v2.16.0) | yes, official | **no** — EVM / Solana / TON | — |
| `NethermindEth/x402-starknet` | **no** — TypeScript only | yes | — |
| ERC-8004 | client side is trivial via web3.py | **no** — Draft, `eip155`-only | — |

Every row is missing exactly one of the two properties. There is no cell where a Python
program talks to Starknet privacy primitives, and the agent-payments ecosystem next door
has the same hole: x402 has an official Python SDK but no Starknet mechanism in it, and
x402 *does* run on Starknet — in TypeScript, in a different library, maintained elsewhere.

**How we worked around it.** We are building the Rust write side anyway (see F12 and
ARCHITECTURE §3), so the Python layer binds down into that rather than reimplementing.
`/sdk/py` is deliberately a marshalling shim with no protocol logic — a third
implementation would be a third place for a wrong hash preimage to hide, and F12 is the
demonstration of how quietly that fails.

**What would have made it easier.** Any one of: Python bindings over `discovery-core`; a
`starknet` mechanism in the official x402 Python SDK; or — most useful of all — a
documented, language-neutral wire spec for the pool's action encoding, so a client in any
language could be written against a document instead of against another client. The
absence of that spec is the root cause of both this entry and F12.

## F14 — The prover's key exposure is real, but the SDK's rename hides it (2026-07-28)

*The exposure itself is already recorded in ARCHITECTURE §5 trust assumptions, with OHTTP
as the mitigation. What belongs here is only why it is easy to miss, and the source trail
to verify it — §5 asserts the fact without citing where it lives.*

**Verified empirically 2026-07-28**, not just read off the source. Building a
`ProofInvocation` through the SDK's own `ProofInvocationFactory` with a distinctive
throwaway key and printing the calldata that would go to `starknet_proveTransaction`:

```
[0] 0x1          array_len = 1 call
[1] 0x254a6b…d91 to = pool address
[2] 0x360f87…192 selector = compile_actions
[3] 0x3          inner_calldata_len
[4] 0xdeadbeef   user_addr        (the address passed in)
[5] 0xcafebabe   user_private_key (the pool key passed in, verbatim)
[6] 0x0          client_actions span length
```

The key appears at index 5 and at no other index. The account signature is a separate
field on the invocation, derived from a different key that is never placed in the calldata
— which is the concrete demonstration that confidentiality and custody separate here
exactly as §5 claims.

The pool's own interface (`packages/privacy/src/interface.cairo:370-375`):

```cairo
fn compile_actions(
    self: @T,
    user_addr: ContractAddress,
    user_private_key: felt252,
    client_actions: Span<ClientAction>,
) -> Span<ServerAction>;
```

The SDK fills `user_private_key` with `user.viewingKey`
(`sdk/src/internal/proof-invocation-factory.ts:132`), wraps it in `__execute__` calldata,
and passes it verbatim as the invocation to the proving service
(`sdk/src/internal/proving-service-provider.ts:121`). The prover therefore sees the
private key and every `ClientAction` in the clear — amounts, counterparties, and in our
design the `OfferTerms` riding in the note salts.

**Why it is easy to miss.** Two renames stand between the honest name and the wire. Cairo
calls the parameter `user_private_key`. The SDK fills it from `user.viewingKey`
(`proof-invocation-factory.ts:132`) and the type is `ViewingKey`, which reads as a
read-only capability rather than a secret. Separately, the repo warns that
`__validate__`/`__execute__` are simulation-only *because* the key is in the calldata —
our own CLAUDE.md constraint 1 — which reads as "so keep it local", without ever saying
that the same calldata is the payload of the remote proving call.

**Bearing on P0.1.** This is the argument that self-hosting the prover is not a stopgap
until StarkWare publish a URL. A hosted prover is a confidentiality dependency on its
operator; §5 already says self-hosting removes the assumption. Worth stating explicitly in
the write-up, because "we ran our own prover" otherwise reads as a workaround rather than
the correct configuration.

**What would have made it easier.** One sentence in the proving-service docs: the invocation
carries `user_private_key`, so the prover belongs inside your trust boundary. The Cairo name
is honest; the SDK rename is enough to lose it on the way past.

---

## F15 — Three ways to get `ClientAction` Serde wrong, none of which error (P1.0)

Porting the action encoding to Rust. All three of these were caught by generating vectors
from the TS SDK *before* writing the Rust, rather than by reasoning about the Cairo — which
is the whole argument for the oracle.

**1. Phase order is not variant order, and the enum invites the mistake.** `UseNote` is
variant 6, `CreateEncNote` is variant 3 — but `UseNote` runs in phase 4 and `CreateEncNote`
in phase 5 (`actions.cairo:287-299`). So an action set assembled in enum order is rejected
with `ACTIONS_OUT_OF_ORDER`. The enum declaration order is the *wire* format and the phase
mapping is a separate, deliberately different order; nothing in the type system says so.
Spend-then-create is the correct sequence and the enum lists it backwards.

**2. `u128` is one felt, not two.** `u256` in Cairo Serde is two limbs, and `amount: u128`
sitting next to `index: usize` looks exactly like a place where someone would reach for a
two-limb encoding. Get it wrong and every subsequent field shifts by one — the salt lands
in the index slot, the note is written somewhere nobody reads, and nothing anywhere errors.

**3. The TS `ClientAction` types are generated, not authored.**
`sdk/src/internal/client-actions.ts` is marked `AUTO-GENERATED … Generated from
sdk/src/internal/abi.ts`. So the oracle's authority derives from the ABI, and a fixture
generated today silently stops describing the deployed pool if upstream regenerates against
a newer ABI. The vectors need regenerating whenever the sibling checkout moves — that is a
maintenance obligation the KAT itself cannot detect.

Also worth noting for whoever writes `/sdk/py`: the TS `ClientAction` is a *type*, not a
runtime value, so there is no constructor to call across a binding. That is one more reason
the Python layer marshals rather than builds.

**What would have made it easier.** The language-neutral wire spec named in F13 — again.
Every one of these is a fact about the encoding that exists only as a property of two
implementations agreeing.

---

## F16 — `proof_facts` is a non-standard extension to the v3 transaction hash (P1.0)

The privacy pool's transactions are ordinary Starknet `INVOKE_TXN_V3` — except the hash
preimage has one extra term. `calculateInvokeTransactionHash` in starknet.js v10 appends
`poseidon_hash_many(proof_facts)` after the calldata hash, **and only when `proof_facts` is
non-empty**:

```js
const proofFactsAdditionalData = proofFacts?.length ? [poseidonHashMany(AToBI(proofFacts))] : [];
```

Two consequences for a Rust client:

- **No off-the-shelf Starknet crate hashes this correctly.** `starknet-rs` implements the
  standard v3 preimage, which is right for every transaction except the one that matters —
  the `apply_actions` submission that carries the proof. Using it would produce a correct
  hash for the proof invocation and a silently wrong one for the submission, which is the
  worse of the two failure orderings because the first half of the flow would appear to work.
- **The conditional is load-bearing.** Appending `poseidon_hash_many(&[])` unconditionally
  changes the hash of every *ordinary* transaction, so "always append, it's empty anyway" is
  wrong in the other direction. Both mistakes are pinned by KAT here; the fixture carries a
  vector each way precisely because a single-branch fixture would pass while the other stayed
  broken.

We only found it because the argument type had an optional `proofFacts` field. It is not in
any spec we could find, and the starknet.js version carrying it is `10.0.0-beta.6` — a
prerelease. **If that extension is still unreleased upstream, pinning the exact starknet.js
version is not optional**, because the oracle moving silently re-defines what a correct
signature is.

**What would have made it easier.** Naming it in the pool's docs as a transaction-format
requirement rather than leaving it to be inferred from a `?:` in a TypeScript type.

---

## F17 — The Wallet API can move value but cannot carry a payload (2026-07-28)

StarkWare's suggested integration path for a team that does not want to hold keys is the
Starknet Wallet API — the dapp hands `STRK20_ACTION[]` to a wallet, the wallet holds the
key, proves, and submits. Structurally exactly right for us.

It does not fit, and the reason is worth recording because it is a statement about what the
privacy stack currently considers a use case.

`STRK20_ACTION` in starknet.js 10.5.3 is four variants
(`@starknet-io/starknet-types-0103/…/wallet-api/components.d.ts:187-227` — 404 on npmjs,
installs transitively with `starknet`):

```ts
deposit  { token, amount }
withdraw { token, amount, recipient }
transfer { token, amount: FELT | 'OPEN', recipient }
invoke   { contract, calldata }
```

No salt field, no index field, no note-level control. `ClientAction` has ten variants; the
six absent include every one that touches a note or channel directly, and critically
**`CreateEncNote`** — the only action carrying a client-chosen salt.

**The invoke action is the near-miss.** It takes arbitrary calldata and can reference open
notes via `${openNoteIds[N]}` placeholders, which looks like a payload channel. It is not:
a client `InvokeExternal` compiles to `ServerAction::Invoke` (`actions.cairo:392`), and
ServerActions are the *public* argument to `apply_actions`. Anything in that calldata is
world-readable. The invoke lane exists for AMM swaps, where public swap parameters are fine
and only identity needs hiding — the opposite of what Erebus needs.

That contrast is the sharpest statement of why the salt lane matters: a salt is consumed by
hash derivations and recoverable only through the pair's shared secret, so it is private at
rest; invoke calldata is published verbatim. Two payload-shaped fields, one private, one
not, and nothing labels which is which.

That salt is our entire payload mechanism (F1: 119 bits per note, four notes per message).
A wallet exposing `transfer(recipient, token, amount)` picks the salt itself; there is no
seam to hand it an `OfferTerms`.

**The generalisable point:** the wallet API models the privacy pool as a *private payments*
product. Erebus is structured data transport that settles. Every internal StarkWare project
on the list — Beam, Payroll, Offmarket, the Privacy Bridge — moves value, so four actions
covers all of them. The first use case that needs a note to *say* something rather than be
worth something falls outside the abstraction.

Not a defect; a scope boundary that nothing states. It is only visible by diffing the two
action enums, which nothing invites you to do.

**Consequence:** "the agent brings its own wallet and Erebus holds no keys" is not
achievable through the wallet API as it stands, independently of the headless-agent problem.
Options in [custody-design.md](./custody-design.md).

**What would have made it easier.** One line in the wallet-API overview saying which
`ClientAction`s it does not cover, and why.

---

## F18 — Operational constraints that only exist in a third-party skill (2026-07-28)

Four facts that change demo choreography, none of which are in the contract, the SDK
README, or the by-example site. All from `starkience/strk20-agent-skills`, a community
skill — *not* a StarkWare source, so each is flagged with how far we verified it.

**Only two wallets support STRK20 at all.** Ready, plus Xverse "in progress". The skill is
explicit that *"Braavos, Privy, and other wallets or embedded-wallet providers are not
prepared for STRK20 — treat them as unsupported."* This matters well beyond us: "the user
brings their own wallet" currently means one working browser extension. **Unverified** —
we have no way to check from here.

**A deposit is two transactions.** The ERC-20 `approve` must be visible on-chain before the
private deposit can be proven, so the wallet prompts twice. **Consistent with the SDK**,
whose `preCalls` mechanism exists to carry exactly that approve (`client/src/interfaces.ts`).

**Notes mature roughly 10 blocks after creation** — freshly shielded funds are not
immediately spendable, so shield-then-transfer fails if run back to back. **Partly
verified**: there is no maturity constant in the pool contract (the similarly named
`open_note_depositor_blocked` is an admin blocklist, unrelated). The plausible mechanism is
`validate_proof`'s `base_block_number < current_block_number` — a note is not in the
prover's base-block view until its block settles. The *number* is unconfirmed. **Check
empirically before choreographing the demo around it.**

**The pool charges a flat fee per private operation** — 4 STRK on mainnet per the skill.
We measured `get_fee_amount() = 0` on the Sepolia pool (F4), so the demo is unaffected, but
any mainnet costing needs the real figure and it is large enough to drive UX.

**Why this is friction rather than trivia:** three of the four change how the demo has to be
sequenced, and all four were found in a community skill rather than in any first-party
document. The by-example site covers concepts and API surface; the operational edges — what
prompts twice, what is not spendable yet, what a wallet actually supports — are not written
down by StarkWare anywhere we found.

---

## F19 — The TS accepts memo hashes that are not valid felts; the Rust does not (P1.3)

Found by the differential test on the first run, which is the argument for keeping the
TypeScript oracle alive in one entry.

`truncateMemoHash` (`sdk/ts/src/channel/wire.ts:120`) takes a string and does
`BigInt(memoHash) & ((1n << 128n) - 1n)`. `BigInt` range-checks nothing, so a value above
the STARK prime is accepted and silently truncated. `Felt::from_hex` in Rust rejects it
with `RepresentativeOutOfRange`.

**This is not a hypothetical.** `memoHash` is the commitment to detail held off-chain, and
the obvious way to produce one is a general-purpose hash. **SHA-256 and Keccak-256 both
emit 256 bits, which is larger than the felt252 prime (~2^252).** So the natural
implementation — hash the memo, pass the digest — produces a value the TypeScript accepts
and the Rust refuses. Whichever side is believed, the two clients disagree about what the
same memo commits to.

Both behaviours are arguably defensible in isolation, which is what makes it dangerous:
neither implementation is obviously wrong when read on its own.

**Resolution for the MVP:** the Rust is right — a `felt252` field should hold a `felt252`.
The TS should validate rather than truncate silently, or `memoHash` should be documented as
"a felt, not a digest" and the caller told to reduce mod p. That is a call for whoever owns
the interface (ARCHITECTURE §4), not one to make inside the codec.

**What this cost:** nothing, because the fixture caught it before either client wrote a
note. That is the whole point of the second implementation and it earned its keep here.

---

## F20 — The prover answers a failed execution with a bare `-32603 Internal error` (P1.0)

First live `starknet_proveTransaction` from the Rust client, against a deliberately
non-existent identity to learn the error shape before there is real state to prove.

```
prove_transaction error: proving service returned error -32603: Internal error
```

**The good half.** The request was *accepted*. An earlier probe with empty params returned
`-32602 Invalid params`, so the two are distinguishable, and reaching `-32603` means the
service parsed our `INVOKE_TXN_V3` — signature, resource bounds, DA modes, the
`__execute__` calldata wrapper — and failed only in execution. That is a live validation of
the wire serialization that no offline fixture can give.

**The bad half.** `-32603` is JSON-RPC's generic internal error and it carries nothing else.
The virtual execution panicked with a Cairo reason — `ZERO_USER_ADDR`, or a storage read on
an unregistered identity, or something else entirely — and none of it survives to the
client. So these three are indistinguishable from the outside:

- the action encoding is wrong,
- the encoding is right but the on-chain state does not support it,
- the proving service is unwell.

For a stack whose failures are already silent (F12, F19), losing the panic reason at the
last hop is expensive. During development it turns every failed proof into a bisection.

**Contrast with screening**, which *is* distinguishable: the interceptor surfaces a blocked
deposit as JSON-RPC `10000` (proof-interceptor README), which is why `ProverError` carries
`is_screening_rejection()`. So the service can pass specific codes through when someone
decides to. Execution failures just have not been given one.

**Workaround:** simulate locally first and trust the local Cairo panic, treating the prover
purely as a proof generator rather than as a validator. That is the pipeline anyway
(CLAUDE.md constraint 2), which softens this — but it does mean a local simulator is not
optional for development, it is the only place errors are legible.

**What would have made it easier.** Relaying the Cairo panic reason in the JSON-RPC error
`data` field. The prover already has it; it is discarded at the boundary.

---

## F21 — Note indices are an allocator, and nothing tells you that (P1.3)

Two separate contract rules govern every note index, and neither is documented as being
about the other:

- **Contiguity.** `privacy.cairo:737-746` asserts `index == 0 || note[index - 1] exists`,
  reverting `INDEX_NOT_SEQUENTIAL`. Same shape appears at `:385` for channels and `:448`
  for subchannels.
- **Write-once.** `_apply_write_once` (`privacy.cairo:932-946`) reads each target slot and
  asserts it is zero, reverting `NON_ZERO_VALUE`.

Read separately they look like two ordinary validations. Together they mean the index space
is an **allocator** — hand out contiguously, never twice — and a client that treats
`index` as a caller-supplied parameter has pushed both invariants onto every call site.
Ours had. Both `write_message` and `accept_and_settle` took an index and trusted it.

**The part that actually cost time: emission order inside a single action set.**

`__execute__` compiles via a `call_contract_syscall` to `compile_and_panic`, and
`_client_apply_actions` (`privacy.cairo:755-777`) applies each `WriteOnce` *as it walks the
list*. The whole sub-call then panics so the state is discarded and the `ServerAction`s are
recovered from the panic data. The consequence is not obvious from any single line: **the
contiguity check on note `n` can see notes that the same action set created earlier.**

So a set is not an unordered bag of actions. Creating note 8 before notes 4–7 in the same
set fails `INDEX_NOT_SEQUENTIAL` against slots that set was about to fill. Our
`accept_and_settle` emitted the payment note first regardless of index, which is correct
only when the payment happens to be the lowest index. The tests passed because the fixture
put the payment at 4 and the record at 8–11 — the one ordering that hides the bug.

*Inferred from the source, not yet observed on-chain.* Nothing has run against Sepolia, so
this is a reading of `compile_and_panic` rather than a reproduction. It is recorded because
the fix (sort creates ascending) is free and correct either way. Verify at P2.0.

**Worked around:** a `SubchannelCursor` (`sdk/rs/src/subchannel.rs`) is now the single
allocator per subchannel, and `accept_and_settle` sorts its creations by index before
emitting. Nine tests in `tests/index_contiguity.rs`, three mutations checked.

**A design consequence, not a bug.** A negotiation message occupies four notes on a
`4k..4k+3` grid so the reader needs no framing search. A settlement's payment note is one
note wide. So settling leaves the cursor at `4k+1`, off the grid, and no further message can
be written to that subchannel. One subchannel is currently one deal. Fine for the MVP;
a real constraint on long-lived agent pairs, and there is no cheap fix — padding to the
grid means writing filler notes that are permanently unspendable and burn indices forever.

**What would have made it easier.** Saying in the `apply_actions` docs that actions are
applied sequentially and observe each other's writes. It changes an action set from a set
into a program, and that is worth one sentence.

---

## F4 — Target network: Sepolia. Confirmed, not assumed. (P0.1)

**Sepolia. The pool is live and callable; mainnet has no published deployment.**

Verified against the chain, not the docs:

```
address     0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91
class hash  0x67dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d
get_version()                → '2.0'
get_proof_validity_blocks()  → 450
get_fee_amount()             → 0          (no STRK fee per apply_actions)
get_auditor_public_key()     → 0x1d17f9…bb2   (non-zero: disclosure configured)
get_screener_public_key()    → 0x62f1e7…552   (non-zero: screening configured — see F6)
```

Class hash agreed by two independent public RPCs (`api.cartridge.gg`, `drpc.org`).
The live `ClientAction` enum and `Note` struct read off the deployed ABI match the
source trace in F1 exactly — that is the third independent confirmation of F1.

**Mainnet is not an option.** `demo/.env.mainnet.example` in the upstream repo is
100% `TODO_MAINNET_*` placeholders — pool address, class hash, indexer, prover,
feeder gateway, tokens, all unfilled. `strk20-by-example.org` publishes a Sepolia
address and no mainnet one. So Sepolia isn't a preference, it's the only deployment.

**Open, needs resolving before P1.1:** the README compatibility matrix lists the
Privacy Pool at tag `PRIVACY-0.14.3-RC.0` with class hash
`0x52107fadffab71bdcbb6b2ccb68ba3e1b5558d94036538053e159d3076ad633`. The live Sepolia
pool is `0x67dddd…554d` — **a different build**. The matrix says "All components in a
row are tested together. Use matching revisions when deploying." We need to know which
prover and discovery-service tags pair with the deployed pool, not with RC.0.
`get_version()` returns `'2.0'` for both, so the version string does not disambiguate.

**Also:** `docs/poulav.md` reading #1 says strk20-by-example.org is "browser required,
it's a JS app." That is no longer true — every route mirrors as raw Markdown, and
`/llms.txt` / `/llms-full.txt` index the whole site. Worth correcting in the task list.

---

## F5 — No hosted off-chain services are published. The prover is the real Day-0 blocker. (P0.1)

The pool is on-chain and verifiable. The two services it depends on are not.

Every reference to the discovery service and the proving service — on the docs site,
in the SDK, in the demo — is an **env-var placeholder**: `INDEXER_URL`,
`PROVING_SERVICE_URL`. No StarkWare-operated endpoint is published anywhere I could
find. The repo's own configs point at `http://localhost:8080` / `http://localhost:3000`
against a local devnet on `SN_INTEGRATION_SEPOLIA` (`0x534e5f494e544547524154494f4e5f5345504f4c4941`),
which is a StarkWare staging chain, not public Sepolia.

Both are self-hostable. Upstream README compatibility matrix:

| Component | Image |
|---|---|
| Transaction Prover | `ghcr.io/starkware-libs/starknet-privacy/transaction-prover:PRIVACY-0.14.3-RC.2` |
| Discovery Service | `ghcr.io/starkware-libs/starknet-privacy/discovery-service:PRIVACY-0.14.3-RC.2` |
| Proof Interceptor (screening sidecar) | `ghcr.io/starkware-libs/starknet-privacy/proof-interceptor:PRIVACY-0.14.3-RC.2` |
| Pathfinder | `eqlabs/pathfinder:v0.22.7`, with `PATHFINDER_STORAGE_STATE_TRIES=10000` |

Asymmetry that matters for scoping:

- **Discovery is optional for the MVP.** `ContractDiscoveryProvider` queries the pool
  over plain RPC instead — upstream calls it the "development, testing, no-extra-infra"
  path. Costs a burst of RPC calls per scan, scaling with pool history. Still a keyed
  read, so it does not violate the no-chain-scanning constraint.
- **The prover is not optional and has no fallback.** Without it there is no proof;
  without proof facts `apply_actions` reverts on `EMPTY_PROOF_FACTS`
  (`privacy.cairo:808`). The SDK's mock proving path is simulation-only and cannot
  produce a proof a real chain will accept.

**Answered 2026-07-28 (Akash, StarkWare), in two stages.** First: no endpoint exists, team
asked, no ETA, and his interim recommendation was to run our own prover. Then, later the
same day, he provided a Sepolia prover and discovery endpoint directly, asking that we not
share them. They are in `.env` as `PROVING_SERVICE_URL` / `INDEXER_URL` and are deliberately
not written down here.

**Verified live the same day:** the prover answers `starknet_specVersion` with
`0.10.3-rc.2`, and `starknet_proveTransaction` exists (an empty-params call returns
`-32602 Invalid params`, not method-not-found). The `rc.2` matches the
`transaction-prover:PRIVACY-0.14.3-RC.2` row of the compatibility matrix, which is partial
evidence on the open "which tags match the deployed class hash" question — necessary, not
sufficient, since it does not prove agreement with the *pool's* deployed class.

**What this does and does not change.** It unblocks Phase 2 without a Pathfinder sync, so
the MVP can run on StarkWare's prover. It does not change the production answer: a hosted
prover sees the pool key in the clear (F14), so self-hosting remains correct for anything
real. And it still does not buy the deposit leg — see F6.

---

## F6 — The live pool is screening-enabled: shielding needs an attestation (P0.1 → P1.1)

`get_screener_public_key()` on the Sepolia pool is non-zero, and the code path is
unconditional: if an action set contains a `TransferFrom` — i.e. any deposit —
`apply_actions` requires a `ScreeningAttestation` or reverts with `SCREENING_REQUIRED`
(`privacy.cairo:791-797`). The attestation must be signed by the configured screener
and be **fresh within 300 seconds** (`DEPOSITOR_VALIDATION_MAX_AGE`,
`utils.cairo:96`), with 60s of tolerated clock skew.

So P1.1's acceptance criterion — "shield a test amount" — is gated on the screening
service, not just the prover. The SDK surfaces this as `ScreeningRejected` /
`ScreeningUnavailable` and carries the signature in the proving response's
`additionalData`, so in practice it rides along with whatever prover endpoint we get.
If we self-host, it means the proof-interceptor sidecar too.

Transfers between already-shielded notes are unaffected — no `TransferFrom`, no
screening. Only the deposit leg is gated.

**Amended 2026-07-28 — self-hosting the prover does not solve this, and that is the
sharp edge.** Once StarkWare's answer on F5 became "run your own prover", the obvious
reading was that the screening sidecar comes with it. It does not. `proof-interceptor`
holds no screener key: the STARK-curve signature it relays comes from elliptic-proxy's
`/screen` response (`screening-interceptor.ts:220,293`), which needs partner HMAC
credentials. Run it without `SCREENING_URL` and it degrades to a no-op that returns
`allowed: true` **with no signature** — `/health` still reports OK, so nothing looks
wrong until `apply_actions` reverts on-chain.

That leaves two ways to shield, and they are different in kind:

| | |
|---|---|
| Get screening access from StarkWare | an endpoint, partner credentials, or a signed attestation for our depositor. Note the 300 s freshness window — a pre-signed attestation has to be timed to the transaction. |
| Deploy our own pool instance | the class is already declared on Sepolia. The constructor (`privacy.cairo:150-163`) takes `governance_admin, auditor_public_key, screener_public_key, proof_validity_blocks` and is unpermissioned, so we can set a screener key we hold and sign our own SNIP-12 `DepositorValidation` attestations. |

**The generalisable friction:** the deployment is self-hostable component by component,
but one link in the chain — the screener's private key — is not a component, and nothing
in the compatibility matrix says so. The matrix lists proof-interceptor as an optional
sidecar "for screening-enabled pools", which reads as *you may add screening*, when for
a pool that already has a screener key configured it actually means *you cannot deposit
without the key holder's cooperation*. The failure mode is the usual one for this stack:
silent until it isn't.

---

## F7 — Proof generation is ~29 s per transaction (P1.4, partial)

Published figure from strk20-by-example.org: **"~29 s (12-core / 46 GiB;
machine-dependent)"** for Stwo proof generation. Not our measurement — this is the
vendor's number and should be re-measured on our own hardware, but it is the right
order of magnitude to plan against.

The unit is the *transaction*, not the note, so batching many notes into one action
set amortises. What does not amortise is rounds: each offer or counter is its own
transaction and its own proof. A three-round negotiation is ~90 s of proving before
any settlement.

This directly answers ARCHITECTURE §8 open question 2, and it constrains the demo —
a live 2–3 minute recording cannot show many negotiation rounds in real time.

Also relevant: `provingBlockId` must be `currentBlock - 10`, because notes mature 10
blocks after creation and a head-based proof can be invalidated by an L2 reorg. Proofs
stay valid for 450 blocks on this pool.

---
