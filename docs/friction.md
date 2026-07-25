# Friction log

Where the STRK20 stack fought us. Logged as we hit it, not batched at the end.

Source under investigation: `starkware-libs/starknet-privacy` @ `3dfe66f`
(2026-07-22, `feat(client): client.submit over the PrivacyWallet seam (#914)`).
Cloned to `../starknet-privacy` (sibling of this repo, not vendored in).

---

## F1 — Subchannel notes cannot carry a structured payload (P0.2)

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

**Action:** ask StarkWare for a Sepolia prover endpoint before standing up Pathfinder
+ prover ourselves. Self-hosting means running a full Starknet node for a weekend MVP.

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
