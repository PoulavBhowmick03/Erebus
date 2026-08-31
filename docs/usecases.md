# Where Erebus fits

This document helps a team decide whether the current implementation fits a use case.
It separates implemented behavior, future use cases, and cases outside the present protocol.

> **Updated 2026-08-31.** This document describes wire v3, repeat deals, scoped disclosure,
> change-note settlement, and Protocol 4 recovery. [`status.md`](./status.md) remains the
> tiebreaker when documents disagree.

For the full technical explanation, read [`tech.md`](../tech.md).

## What Erebus is

Erebus is a two-party negotiation and shielded-payment protocol over the STRK20 privacy
pool. It is not a separate token, privacy pool, or Cairo application contract.

The Rust client writes offers, counters, and acceptances as encrypted framed records. Wire
v3 binds each record to a deal ID and uses five zero-amount notes for each data frame.

The payer accepts a counterparty offer and pays it in one action set. This operation spends
the payer's notes and creates a payment note for the offer author
(`sdk/rs/src/client.rs:789-875`).

Either party can export one deal to a registered recipient. The encrypted grant contains
only the capabilities for that deal. It has an expiry and grants no spending authority.

The high-level API has seven negotiation operations. It is not a general Rust replacement
for the upstream TypeScript privacy SDK (`sdk/rs/src/client.rs:538-573`,
`sdk/rs/src/lib.rs:3-20`).

## Current readiness

| Area | Current state | Meaning for a use case |
|---|---|---|
| Rust protocol path | Implemented with offline known-answer and integration tests | The implementation supports technical evaluation and controlled demonstrations (`sdk/rs/tests/cairo_conformance.rs:1-13`, `sdk/rs/tests/execution_pipeline.rs:1-5`). |
| Wire v3 | Encrypted, authenticated, and exercised live on Sepolia and mainnet | Repeat deals and per-deal disclosure work. Independent cryptographic review remains open. |
| Relationship privacy | Partial | The counterparty address is public at channel-open. Timing, sender, action shape, and note count also remain public. |
| MCP backend | `mock` by default, `seam` by explicit configuration | An agent demonstration does not prove that Rust, the prover, RPC, or Starknet ran (`mcp-server/src/erebus_mcp/config.py:10-13`, `mcp-server/src/erebus_mcp/config.py:72-113`). |
| Settlement | Multiple deals per channel pair, with payer-owned change | Each settlement record separates agreed and paid amounts. |
| Disclosure | Recipient-bound, time-limited, and scoped to one deal | The recipient needs its registered pool key. Expiry cannot revoke facts already opened. |
| Recovery | Protocol 4 journal and explicit resume | Local fault tests and the packaged-source Sepolia recovery canary pass. |
| Production | Not ready | One bounded mainnet canary passed. Monitoring, backup tooling, external operation, and independent review remain open. |

## The current fit test

A current use case fits only when all these conditions are true:

1. The flow has two known parties.
2. The operator controls both the account key and the STRK20 pool key for its identity.
3. The operator also controls or trusts the prover and write RPC endpoint.
4. One party makes each on-chain payment. The counter-value is off-chain.
5. One fixed offer record is sufficient. The wire does not carry free-form contract text.
6. The payer holds notes with a total that covers the final amount.
7. The workflow tolerates a proof and a chain wait for each negotiation write.
8. Recipient-bound disclosure of one complete deal is acceptable when disclosure is required.
9. Public relationship, transaction timing, sender, action shape, and note count are acceptable.
10. The operator accepts the current unaudited, bounded-canary boundary.

The client verifies registration before it opens a channel. Settlement selects notes that
cover the offer and returns the excess as a payer-owned change note.

## Information and trust boundaries

"Private" has a different meaning for each observer.

| Observer | What the observer receives | Important limit |
|---|---|---|
| Public chain observer | The submitting account, pool interaction, timing, action shape, note count, and public salt values | Wire v3 encrypts terms and removes the fixed v2 spare-bit classifier. It does not hide the relationship or traffic shape. |
| Prover operator | The virtual invocation, including the pool private key | This operator can decrypt the history protected by that key (`sdk/rs/src/prover.rs:3-14`). |
| Write RPC operator | The `compile_actions` preflight, including the same pool private key | A public RPC endpoint is not a suitable write endpoint for a production identity (`sdk/rs/src/calldata.rs:25-36`). |
| STRK20 pool auditor | The pool private key encrypted under the pool auditor key at registration | This access covers the identity across its pool history, not one Erebus deal (`sdk/rs/src/channel.rs:126-140`). |
| Viewing-grant holder | Encrypted capabilities for one deal, plus participant and token scope | The registered recipient can read that deal but cannot spend notes or derive the parent channel key. |
| Python agent layer | Public terms, opaque handles, key-file paths, and grant-file metadata | Tool results do not contain pool keys, account keys, parent-channel keys, or encrypted grant contents. |

The deposit that funds a payer is a public token leg. A later private transfer does not erase
that fact. A separate funding transaction reduces direct correlation with a later payment,
but the pool interaction and timing remain visible (`sdk/rs/src/channel.rs:329-364`).

## Use cases that match the current mechanism

These are technical MVP fits. They are not production deployment recommendations.

### 1. One-off purchase of an off-chain service

One agent can buy a compute result, API response, dataset, report, or other off-chain
deliverable. In the role-bound flow, the seller writes the final offer. The buyer accepts
that offer and pays (`docs/friction.md:1502-1510`).

The MCP server makes this direction explicit. A payee cannot call `accept_and_settle`, because
that method always spends the caller's notes (`mcp-server/src/erebus_mcp/tools.py:170-205`).

This flow provides two useful properties:

- Wire v3 encrypts the structured price, memo commitment, and authenticated deal ID.
- Acceptance and payment enter one action set, so they land together
  (`sdk/rs/src/channel.rs:515-555`).

This flow does not prove delivery. The proof says nothing about the quality, availability,
or meaning of the off-chain result. A marketplace still needs delivery, refund, and dispute
rules.

Each interactive write needs a separate proof and chain update
(`sdk/rs/src/execution.rs:132-238`). A deployment must measure whether its value and latency
budgets can absorb those operations.

### 2. One-off bilateral request for quote

A buyer and a supplier can exchange an offer and a counter-offer for an off-chain obligation.
The supplier writes the final price. The buyer accepts and pays that price.

The fixed wire carries amount, deadline, and `memo_hash`. The token comes from the subchannel
(`sdk/rs/src/wire.rs:21-35`, `sdk/rs/src/client.rs:938-948`).

The `memo_hash` can bind the offer to an external specification. The repository does not
define the preimage format, canonicalization rules, or signature policy for that specification.

An offer cannot be deleted after publication. A short deadline limits its validity, and the
client rejects an expired offer before settlement (`sdk/rs/src/negotiation.rs:231-272`).

This is not delivery-versus-payment for two on-chain assets. Only the accepting identity
spends private notes in the current proof invocation (`sdk/rs/src/client.rs:789-864`).

### 3. One-off machine procurement

A service or device can buy bandwidth, storage, compute, or energy from another operator.
The protocol shape is identical to the one-off service purchase.

This fit assumes that each machine operates through a managed Starknet identity. The current
repository is not a consumer-wallet integration and stores secret channel state locally
(`sdk/rs/src/state.rs:1-10`).

### 4. Full-record audit or dispute reconstruction

Either party can give a registered auditor or arbitrator a grant for one deal. The recipient
reconstructs offers, counters, acceptance, and the payment amount from chain data.

The reconstructed record keeps the agreed and paid amounts separate. This lets the holder
detect a mismatch written by another client (`sdk/rs/src/disclosure.rs:195-211`).

The record has three important limits:

- The grant is encrypted to the registered recipient. The recipient must protect its pool key.
- Participant addresses remain part of the disclosed record.
- The result is a locally reconstructed record, not an outcome-only ZK receipt.

Therefore, this feature fits a trusted recipient who can read the full transcript. It does
not fit a verifier who must learn only the final result.

## Plausible use cases that need protocol work

### Recurring B2B invoices, contractor payments, and payroll

These cases match the bilateral payment direction, repeat-deal framing, change-note
settlement, and per-deal disclosure. The remaining blockers are operational. The operator
alpha still needs monitoring, backup tooling, independent review, and repeated external
operation before recurring real-value use. One bounded mainnet workflow does not close that
gap.

### Sealed-bid auctions

One bilateral channel per bidder can hide each bid's content from other bidders. This does
not prove that the selected bid won under the auction rules.

A grant for the winning channel reveals only that channel. It says nothing about losing bids.
An outcome verifier needs every relevant bid, authenticated participant binding, a closing
rule, and proof of winner selection.

Therefore, bilateral channels are a transport building block for an auction. They are not a
complete sealed-bid auction protocol.

### Platform outcome verification

A platform can receive a recipient-bound grant and reconstruct one full deal. This model
gives the platform every negotiated field in that deal.

Many integrations need a narrower result. They need proof that named parties agreed, policy
was satisfied, and settlement matched the agreement.

That machine-readable outcome does not exist yet. It needs a signed or ZK-verifiable receipt
that binds these items:

- the two participants
- the canonical terms commitment
- the accepted offer
- the disclosure policy
- the payment note or settlement transaction
- the chain and pool scope

The current checksum protects grant serialization. It does not authenticate the grantor or
prove those application claims (`sdk/rs/src/disclosure.rs:106-146`).

### High-frequency agent payments

The privacy motivation is strong for repeated agent payments. The current execution model
does not fit this frequency.

Each dependent offer or counter requires the previous state, so interactive rounds cannot be
combined in advance. Batching is useful only for actions known at the same time.

A high-frequency design needs fewer on-chain negotiation writes. Options include an off-chain
encrypted negotiation with one final commitment, proof aggregation, or a payment-channel
design. Each option changes the evidence and metadata model.

### General confidential messaging

The salt lane can carry arbitrary bits, but that does not make it a practical chat channel.
Wire v3 spends five permanent note slots for one structured data frame.

Free-form messages belong in an off-chain encrypted transport. Erebus can commit to that
conversation, but the current `memo_hash` field needs a published convention first.

### Private delivery-versus-payment

The current proof invocation represents one pool user and one account signature. It cannot
make two independent pool identities spend notes in one action set
(`sdk/rs/src/calldata.rs:25-36`, `sdk/rs/src/execution.rs:162-174`).

Two-asset delivery-versus-payment needs a multi-party authorization design or an application
contract. Any application-contract design must state which amounts, calls, and identities
remain public.

## Cases outside the current fit

The current implementation does not fit these cases:

- order books, market making, and high-frequency trading
- multi-party negotiation or consensus
- repeated transactions between the same pair and token
- low-value payments that cannot absorb one proof per write
- consumer dapps that must not hold user privacy keys
- deployments that use an untrusted prover or public write RPC
- workflows that require the negotiation event itself to be hidden
- fair exchange that must prove delivery of an off-chain result
- atomic exchange where two independent parties spend on-chain assets
- disclosure where the verifier must not receive the transcript

This repository does not implement a wallet-facing dapp flow. A separate dapp design must
keep user viewing keys outside application code.

## Why the design uses these choices

| Choice | Reason | Benefit | Cost |
|---|---|---|---|
| STRK20 note salts as the data lane | A pool note has no application payload field. | Negotiation and payment remain in the same pool action model. | Each data frame uses five permanent notes. The note count remains visible. |
| Framed structured wire | A reader derives record boundaries and deal IDs from authenticated frames. | One channel pair can carry repeat deals and settlement records. | The wire cannot carry full contract text. |
| On-chain negotiation records | A grant holder can reconstruct the record from chain data. | No separate transcript log is necessary for reconstruction. The bearer grant still needs delivery. | Each write needs a proof, chain inclusion, storage, and a sequential note allocation. |
| Atomic acceptance and payment | The design removes an accepted-but-unpaid on-chain state. | Both changes enter one proof (`sdk/rs/src/channel.rs:515-523`). | The pool does not understand agreement semantics. Rust must compare the amounts (`sdk/rs/src/channel.rs:545-555`). |
| Recipient-bound viewing grant | A registered recipient can reveal without the grantor's local state. | Disclosure is portable and scoped to one deal. | Expiry stops later opening but cannot erase an earlier disclosure. |
| Rust subprocess boundary | Python passes paths, operation IDs, and opaque handles instead of key values. | Protocol logic, key operations, and chain outcomes remain in Rust. | The operator must preserve the Rust journal and agent intent records. |
| Client-side offer state machine | STRK20 remains a general note pool with no offer semantics. | Erebus can evolve without a new pool contract. | A hostile or different client can write semantically inconsistent records that still satisfy pool rules (`sdk/rs/src/channel.rs:545-555`). |

## Integration levels

### Level 1: Historical full-channel disclosure

This level remains readable for historical wire-v1 and wire-v2 channels. The holder receives
a bearer grant and can read the complete channel.

This level requires trust in the holder. Do not create new bearer grants for wire v3.

### Level 2: Recipient-bound disclosure

This level exists for wire v3. The grant is encrypted to a registered recipient, has an
expiry, and opens one deal. It does not provide revocation after the recipient opens it.

### Level 3: Outcome-only verification

This level does not exist yet. It needs a receipt that proves selected claims without
revealing the full transcript.

### Level 4: Managed settlement service

This level does not exist yet. It needs authenticated tenancy, production key management,
prover and RPC isolation, recovery, monitoring, and service-level failure handling.

## Infrastructure value from this repository

### A selective Rust STRK20 client

`sdk/rs` fills the Rust write-path gap for this workflow. It builds and serializes actions,
preflights them, requests proofs, signs transactions, submits `apply_actions`, and reads notes
(`sdk/rs/src/lib.rs:3-20`, `sdk/rs/src/execution.rs:132-238`).

It is not a complete replacement for the upstream TypeScript SDK. General transfers, swaps,
withdrawals, paymasters, OHTTP, and full discovery providers remain outside its high-level API.

### Reusable conformance fixtures

The fixture set pins Rust against Cairo, TypeScript, and starknet.js behavior. It covers hash
derivations, action serialization, signatures, transaction hashes, and proof invocation.

These vectors are useful to another client implementation. They still require maintenance
when upstream formats or dependencies change (`sdk/rs/src/lib.rs:11-20`).

### A non-JavaScript process seam

The Python package sends one JSON request to `erebus-cli` and reads one response envelope
(`sdk/py/src/erebus/_seam.py:95-165`). The MCP adapter uses a Python worker thread so the
blocking child process does not stop the event loop
(`mcp-server/src/erebus_mcp/seam_client.py:1-17`).

This seam demonstrates one binding design. It is not evidence that every non-JavaScript
client must use a subprocess.

## Work that unlocks more use cases

| Priority | Missing work | Use cases unlocked |
|---:|---|---|
| 1 | Publish the current Protocol 4 wheel set as `v0.2.0` and rerun the installed-artifact canary. | A reproducible operator-alpha release. |
| 2 | Add a clean-install framework example from the published wheels. | External agent-framework integrations. |
| 3 | Add journal retention, backup/restore tooling, and secret-safe monitoring. | Long-running operator deployments. |
| 4 | Bind participants, terms, policy, and settlement in an authenticated or ZK receipt. | Outcome-only verification and platform integrations. |
| 5 | Reduce the number of on-chain interactive rounds. | Lower-value and higher-frequency commerce. |
| 6 | Define and publish the `memo_hash` preimage and signature convention. | Interoperable external contract and delivery commitments. |
| 7 | Complete independent review and a bounded mainnet canary. | Use with real value under a written trust model. |

The measured fee and proving-time entries in `docs/friction.md` are engineering snapshots.
They are not durable product prices or performance guarantees (`docs/friction.md:1190-1223`,
`docs/friction.md:1409-1429`).

## Accurate positioning

Use this description:

> Erebus is experimental operator-run infrastructure for encrypted bilateral negotiation
> and shielded settlement over STRK20. It binds acceptance and payment in one action set.
> Wire v3 supports repeat deals and recipient-bound disclosure for one deal.

Do not say these things:

- "Erebus hides that a negotiation happened." The relationship and traffic shape remain visible.
- "Erebus provides fair exchange." It does not prove off-chain delivery.
- "The viewing grant proves both parties and their policy." The grant reveals the recorded
  deal. It does not prove an off-chain policy or delivery.
- "This is a Rust rewrite of the Starknet privacy SDK." It is a selective client.
- "This is production-ready." Mainnet evidence, monitoring, operator drills, and
  independent review remain open.
- "Self-hosting the prover removes every trust assumption." The write RPC and pool auditor
  remain in the confidentiality model.

## STRK20 references

- [STRK20 pool model](https://strk20-by-example.org/what-is-strk20)
- [Notes and nullifiers](https://strk20-by-example.org/notes-and-nullifiers)
- [Channels and subchannels](https://strk20-by-example.org/channels-and-subchannels)
- [Selective disclosure and screening](https://strk20-by-example.org/compliance)
