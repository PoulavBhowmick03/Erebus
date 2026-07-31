# Where the keys live — design options

**Written 2026-07-28, for Poulav to decide.** Prompted by the decision that *Erebus should
not hold agent keys*, and by Akash's suggestion to use the Starknet Wallet API (Xverse,
Ready) instead of a proving URL.

The short version: the wallet-API route is blocked by something more fundamental than
headless agents, and the word "hold" is doing two different jobs in "Erebus holds agent
keys". Separating those two meanings resolves most of the question.

---

## 1. Two keys, and only one of them is contested

| | What it is | Who must have it |
|---|---|---|
| **Account signing key** | Ordinary Starknet account key. Signs the proof invocation and final `apply_actions` transaction; `assert_valid_signature` (`utils.cairo:383`) calls `is_valid_signature` on the agent's own account contract. | The operator's local Rust process for the MVP, supplied by file path. It never enters proof calldata and is never persisted in channel state. A wallet/session signer should replace the raw file in production. |
| **Pool identity key** | `user_private_key`. Derives the pool identity, decrypts notes, produces nullifiers, derives channel keys. | Whoever builds the action set. This is the contested one. |

The account key and pool key are distinct, but the complete Rust MVP handles both locally:
the proof invocation needs an account-valid signature, and the final invoke must also be
signed. “Does not appear in prover calldata” means the prover cannot steal it; it does not
mean the client can execute without a signer.

The Rust seam now provisions the pool key with `generate_pool_key`: OS entropy goes directly
to a new mode-`0600` file and only its path and public key are returned. The account key is
not generated there because a usable Starknet account also needs deployment metadata and
funding; `sncast` owns that lifecycle, while Erebus consumes a raw key file for the MVP.

## 2. Why the pool key cannot simply stay in the agent's wallet

Because the channel key derivation needs it:

```
channel_key = h(TAG, sender_addr, sender_privkey, recipient_addr, recipient_pubkey)
```

Anything that computes where a note lives needs the sender's pool private key. So "Erebus
computes the actions, the wallet signs them" does not decompose — Erebus cannot compute a
channel key, a note id, or a storage slot without the key.

## 3. The wallet API cannot carry a salt — read, not inferred

`STRK20_ACTION` is a union of exactly four variants. Full definitions, from
`@starknet-io/starknet-types-0103/dist/types/wallet-api/components.d.ts:187-227` (the
package is 404 on npmjs but installs transitively with `starknet@10.5.3`):

```ts
STRK20_DEPOSIT_ACTION  = { type: 'deposit',  token, amount }
STRK20_WITHDRAW_ACTION = { type: 'withdraw', token, amount, recipient }
STRK20_TRANSFER_ACTION = { type: 'transfer', token, amount: FELT | 'OPEN', recipient }
STRK20_INVOKE_ACTION   = { type: 'invoke',   contract, calldata: STRK20_CALLDATA_ITEM[] }
```

**No salt field. No index field. No note-level control of any kind.** `ClientAction` has
ten variants; the six absent here include every one that touches a note or channel directly
— `SetViewingKey`, `OpenChannel`, `OpenSubchannel`, `CreateEncNote`, `CreateOpenNote`,
`UseNote`.

`CreateEncNote` is the one that matters: it is the only action carrying a client-chosen
salt, and the negotiation payload rides in exactly that field. A wallet exposing
`transfer(recipient, token, amount)` picks the salt itself. **The salt lane is not
expressible through the wallet API, and no amount of cleverness changes that.**

### 3a. The invoke action looks like a way out. It is not.

`STRK20_INVOKE_ACTION` takes arbitrary calldata, and its placeholders
(`${openNoteIds[N]}`, `${poolAddress}`) let it reference open notes created in the same
transaction. That is the "invoke anonymizer" pattern — the obvious place to try putting an
`OfferTerms`.

It leaks. A client `InvokeExternal` compiles to `ServerAction::Invoke`
(`actions.cairo:392`), carrying `InvokeInput { contract_address, calldata: Span<felt252> }`.
ServerActions are the *public* half of the design — they are the argument to
`apply_actions`, in an ordinary Starknet transaction, in the clear. Anything put in that
calldata is world-readable on Sepolia.

That distinction was wrong. Invoke calldata is public, but so is the salt: it is stored
verbatim in the high bits of `packed_value`, which appears in `apply_actions` calldata and
events. The keyed channel location is not a confidentiality boundary for a global observer.
See friction.md F30. A replacement wire must encrypt/authenticate before using the salt lane.

The invoke path is built for AMM swaps, where the swap parameters being public is fine and
only the *identity* needs hiding. Erebus needs the opposite.

The anonymizer route — the documented escape hatch for anything beyond the four actions —
does not change this. Its own summary: *"What this hides: the user's address behind the
DeFi action. What may stay public: the amounts and the app activity itself."* Identity
hiding, not data transport.

### 3b. The documentation says this outright

Both statements below are first-party or near-first-party, and neither is a reading of
type definitions:

> *"No note or proof management. The wallet discovers notes, builds the transaction,
> generates the proof, and submits it."*
> — strk20-by-example.org, Starknet Wallet API overview

> *"What the dapp **cannot** do: hold the viewing key, manage notes, or generate proofs."*
> — `starkience/strk20-agent-skills`, wallet-api route

Abstracting notes away is not an omission in the wallet API. It is the product.

### 3c. And the official split puts Erebus on the SDK side

The same skill states the intended division:

> *"in dev the **team** controls the account and keys, so the SDK is appropriate there.
> Production user flows still go through the Privacy Wallet API — end users never expose
> viewing keys."*

The rule is: **SDK when you control the keys, Wallet API when an end user does.** Erebus's
end user is an agent, and no agent wallet exists — wallet support is Ready plus an
in-progress Xverse, both browser extensions (F18). So Erebus falls on the SDK side by their
own framing, not by working around it. That is Option A, and it is the documented position
for this case rather than a workaround.

## 4. "Erebus holds agent keys" means two different things

Worth separating before choosing, because the decision reads differently under each:

- **Custody** — Erebus-the-project operates a service that holds many agents' keys. One
  breach, everyone's funds. This is what should be refused, and every option below refuses it.
- **Handling** — key material passes through Erebus *code* running inside the agent's own
  process, under the agent operator's control. This is what any wallet library does, and
  CLAUDE.md constraint 6 already assumes it: *"key material never leaves the SDK boundary"*
  presumes the SDK has a boundary that keys are inside.

The decision "Erebus should not hold agent keys" is unambiguous about custody. It does not
by itself rule out handling.

---

## The options

### Option A — Erebus ships a library, operates nothing

`erebus-cli` runs as a subprocess of the agent stack. The operator supplies paths to its pool
and account keys; only the Rust subprocess opens them. Erebus-the-project runs no custody
service and never sees either key.

- Erebus is a protocol + library + MCP server, all running agent-side.
- Full `ClientAction` control, so the salt lane works as designed.
- Nothing built so far is wasted; the interface in §4 survives if the wallet is a
  constructor dependency rather than a per-call argument, which also keeps Ishita's mock intact.
- **Cost:** each agent operator needs prover access. A shared prover sees pool keys (F14), so
  either they run their own (Pathfinder + prover) or accept that exposure.
- **Cost:** the MVP uses a local account-key file because no wallet/session signer is wired
  yet. “Agents use whatever Starknet wallet they have” is post-MVP work, not a current
  property. The pool key remains separate and Erebus's local state manages channel secrets
  derived from it.

### Option B — Push for a wallet-API extension

Get a salt-carrying note action into `STRK20_ACTION`, then agents genuinely delegate
everything to a wallet.

- Cleanest end state: Erebus touches no key of any kind.
- **Cost:** it is StarkWare's roadmap, not ours, on nobody's committed timeline.
- **Cost:** browser wallets still do not serve headless agents, so a headless
  `PrivacyWallet` implementation is needed regardless — which is Option A's library wearing
  a different name.

### Option C — Off-chain negotiation, on-chain commitment *(the wallet-API-compatible one)*

This is the design Poulav originally proposed to Akash, before the salt lane was found. It
is worth reconsidering — not because the salt lane failed, but because this is the only
option that works through the wallet API as it exists.

- Agents negotiate over an encrypted off-chain transport keyed by the same ECDH-derived
  channel secret. No notes, no proofs, no 29 s per round — negotiation becomes fast.
- The agreed terms are bound on-chain by a **commitment** — `STRK20_INVOKE_ACTION` to a
  small Erebus contract, calldata = `h(terms, nonce)`.
- Settlement is `STRK20_TRANSFER_ACTION` in the same action set, so acceptance and payment
  stay atomic.

**Why the public-calldata problem does not apply here:** a commitment is a hash. Publishing
it reveals nothing about the terms given sufficient entropy in the nonce. Section 3a kills
putting *terms* in invoke calldata; it does not touch putting a *commitment* there.

- Agents genuinely bring their own wallet. Erebus holds no key of any kind.
- Negotiation rounds cost nothing and are not rate-limited by proving.
- **Cost:** the record is no longer reconstructible from chain state alone. A viewing-key
  holder gets the settlement and the commitment on-chain, but needs the off-chain transcript
  to verify what was agreed. Auditable, but not self-contained — and that difference is
  exactly what ARCHITECTURE §7 weighed.
- **Cost:** needs an Erebus contract, contradicting the current "`/contracts` is nearly
  empty and that is correct" note in CLAUDE.md. A commitment store is small, but it is a
  contract to write, audit and deploy.
- **Cost:** off-chain transport is now ours to build and operate — availability, delivery,
  replay. The salt lane got that from the chain for free.

### Option D — Ask for a salt-carrying wallet action

Upstream's own `client/src/interfaces.ts:19` already carries a local
`STRK20_COMPUTE_AND_INVOKE_ACTION` shim, annotated "a real strk20 wallet gains it
upstream" — so the union is known-incomplete and actively being extended. A note action
with a caller-supplied salt is a plausible addition.

- Best end state: the salt lane *and* zero key handling.
- **Cost:** StarkWare's roadmap, no timeline, and browser wallets still do not serve
  headless agents — so a headless `PrivacyWallet` is needed regardless, which is Option A's
  library under another name.

---

---

## The shape of the choice

The four options are not four points on one axis. They trade two different things:

| | Erebus touches a key? | Payload on-chain? | Works today? |
|---|---|---|---|
| **A** — embedded library | yes, in the agent's own process | yes, salt lane | yes |
| **C** — off-chain + commitment | no | no, only a commitment | yes |
| **D** — extended wallet API | no | yes, salt lane | no |
| **B** — wait for wallet API | no | n/a | no |

**A and C are the only two that ship.** A keeps the on-chain payload and pays for it by
having Erebus code handle a key inside the agent's process. C keeps Erebus completely out
of key material and pays for it by moving the negotiation off-chain, which is the property
§7 spent the most effort defending.

D is what you want; it is not yours to schedule.

## What I would want decided

1. **Custody vs handling.** Is A's "library in the agent's own process" acceptable, or does
   "Erebus doesn't hold keys" mean Erebus code must never touch one? If handling is fine, A
   ships today and nothing built is wasted. If it is not, C is the only remaining option
   that ships, and §7 needs rewriting.
2. **How much the self-contained on-chain record is worth.** It is the concrete difference
   between A and C, and it is the thing the compliance story leans on.
3. **For Akash:** is a salt-carrying note action on the wallet-API roadmap? That single
   answer collapses D into "wait" or "never".

**None of this blocks the MVP.** The demo provisions both agents, so the keys are ours under
every option, and every client primitive built so far — hashes, action encoding, transaction
hashing, signing — is identical under all four.
