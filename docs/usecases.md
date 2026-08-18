# Where Erebus fits

This document helps a team decide whether the current implementation fits a use case.
It separates implemented behavior, future use cases, and cases outside the present protocol.

> **Partly stale as of 2026-08-18.** Fit-test row 6 says no live wire-v2 negotiation has
> completed. One did, on 2026-08-07. This document also predates F38, which found the
> counterparty's address is public at channel-open. Check [`status.md`](./status.md) before
> relying on any claim here; it is the tiebreaker when documents disagree.

For the full technical explanation, read [`tech.md`](../tech.md).

## What Erebus is

Erebus is a two-party negotiation and shielded-payment protocol over the STRK20 privacy
pool. It is not a separate token, privacy pool, or Cairo application contract.

The Rust client writes offers, counters, and acceptances as encrypted data notes. One
message uses five zero-amount notes in a token subchannel (`sdk/rs/src/wire.rs:1-45`).

The payer accepts a counterparty offer and pays it in one action set. This operation spends
the payer's notes and creates a payment note for the offer author
(`sdk/rs/src/client.rs:789-875`).

Either party can export a bearer viewing grant. The grant reveals both directions of one
relationship and one token, but it does not grant spending authority
(`sdk/rs/src/disclosure.rs:24-36`, `sdk/rs/src/disclosure.rs:45-74`).

The high-level API has seven negotiation operations. It is not a general Rust replacement
for the upstream TypeScript privacy SDK (`sdk/rs/src/client.rs:538-573`,
`sdk/rs/src/lib.rs:3-20`).

## Current readiness

| Area | Current state | Meaning for a use case |
|---|---|---|
| Rust protocol path | Implemented with offline known-answer and integration tests | The implementation supports technical evaluation and controlled demonstrations (`sdk/rs/tests/cairo_conformance.rs:1-13`, `sdk/rs/tests/execution_pipeline.rs:1-5`). |
| Wire v2 | Encrypted and authenticated in Rust (`sdk/rs/src/wire.rs:1-35`) | The repository has not completed a live wire-v2 negotiation or an independent cryptographic review (`docs/friction.md:1117-1124`). |
| Relationship privacy | Not demonstrated | The fixed fifth-salt shape identifies current wire-v2 messages (`sdk/rs/tests/wire_v2_fingerprint.rs:31-75`). |
| MCP backend | `mock` by default, `seam` by explicit configuration | An agent demonstration does not prove that Rust, the prover, RPC, or Starknet ran (`mcp-server/src/erebus_mcp/config.py:10-13`, `mcp-server/src/erebus_mcp/config.py:72-113`). |
| Settlement | One private payment in one terminal deal | A settled channel rejects another deal (`sdk/rs/src/client.rs:797-801`, `sdk/rs/src/channel.rs:613-623`). |
| Disclosure | Self-contained bearer grant | The `grantee` field is metadata. Possession of the grant controls access (`sdk/rs/src/disclosure.rs:45-74`). |
| Production | Not ready | Key custody, trusted endpoints, traffic privacy, recovery, costs, and independent review remain open (`sdk/rs/README.md:86-119`). |

## The current fit test

A current use case fits only when all these conditions are true:

1. The flow has two known parties.
2. The operator controls both the account key and the STRK20 pool key for its identity.
3. The operator also controls or trusts the prover and write RPC endpoint.
4. One party makes one on-chain payment. The counter-value is off-chain.
5. One fixed offer record is sufficient. The wire does not carry free-form contract text.
6. The payer holds an exact subset of notes for the final amount.
7. The workflow tolerates a proof and a chain wait for each negotiation write.
8. A full-channel bearer disclosure is acceptable if disclosure is required.
9. Public transaction timing and the current wire-v2 traffic fingerprint are acceptable.
10. The same pair needs only one deal through the current high-level client.

The current client verifies registration before it opens a channel
(`sdk/rs/src/client.rs:575-583`). It also selects notes that sum exactly to the offer
(`sdk/rs/src/client.rs:819-831`). The client does not create a change note
(`sdk/rs/src/client.rs:1469-1487`).

## Information and trust boundaries

"Private" has a different meaning for each observer.

| Observer | What the observer receives | Important limit |
|---|---|---|
| Public chain observer | The submitting account, pool interaction, timing, action shape, and public salt values | Wire v2 encrypts terms, but the current five-note shape identifies negotiation traffic (`sdk/rs/tests/wire_v2_fingerprint.rs:31-75`). |
| Prover operator | The virtual invocation, including the pool private key | This operator can decrypt the history protected by that key (`sdk/rs/src/prover.rs:3-14`). |
| Write RPC operator | The `compile_actions` preflight, including the same pool private key | A public RPC endpoint is not a suitable write endpoint for a production identity (`sdk/rs/src/calldata.rs:25-36`). |
| STRK20 pool auditor | The pool private key encrypted under the pool auditor key at registration | This access covers the identity across its pool history, not one Erebus deal (`sdk/rs/src/channel.rs:126-140`). |
| Viewing-grant holder | Both directional channel keys, the token, and participant metadata | The holder can read the channel but cannot spend its notes (`sdk/rs/src/disclosure.rs:24-36`). |
| Python agent layer | Public terms, opaque handles, key-file paths, and exported grants | Ordinary calls do not receive pool-key or account-key values (`sdk/py/src/erebus/_seam.py:59-92`). |

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

- Wire v2 encrypts the structured price and memo commitment.
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

Either party can give an auditor or arbitrator a grant for one relationship and token. The
holder reconstructs offers, counters, acceptance, and the payment amount from chain data
(`sdk/rs/src/disclosure.rs:234-270`, `sdk/rs/src/disclosure.rs:309-336`).

The reconstructed record keeps the agreed and paid amounts separate. This lets the holder
detect a mismatch written by another client (`sdk/rs/src/disclosure.rs:195-211`).

The record has three important limits:

- The grant is a bearer secret and is not encrypted to the named grantee.
- Participant addresses come from grant fields during reconstruction
  (`sdk/rs/src/disclosure.rs:249-268`).
- The result is a locally reconstructed record, not an outcome-only ZK receipt.

Therefore, this feature fits a trusted recipient who can read the full transcript. It does
not fit a verifier who must learn only the final result.

## Plausible use cases that need protocol work

### Recurring B2B invoices, contractor payments, and payroll

These cases match the bilateral payment direction and disclosure model. They do not match the
current one-deal lifecycle.

Supporting them requires repeat-deal framing after settlement, cursor recovery, and per-deal
disclosure. Removing only the `settled` check is insufficient because settlement leaves the
cursor outside the five-note message grid (`sdk/rs/src/channel.rs:613-623`).

General change-note construction is also important. A recurring payer cannot rely on exact
note denominations for every invoice (`sdk/rs/src/client.rs:1469-1487`).

### Sealed-bid auctions

One bilateral channel per bidder can hide each bid's content from other bidders. This does
not prove that the selected bid won under the auction rules.

A grant for the winning channel reveals only that channel. It says nothing about losing bids.
An outcome verifier needs every relevant bid, authenticated participant binding, a closing
rule, and proof of winner selection.

Therefore, bilateral channels are a transport building block for an auction. They are not a
complete sealed-bid auction protocol.

### Platform outcome verification

A platform can receive a full bearer grant and reconstruct the full record. This model gives
the platform every negotiated field.

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
Wire v2 spends five permanent note slots for one 50-byte structured record
(`sdk/rs/src/wire.rs:21-35`).

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
| STRK20 note salts as the data lane | A pool note has no application payload field (`sdk/rs/src/wire.rs:7-12`). | Negotiation and payment remain in the same pool action model. | Five permanent notes carry each message, and the current shape is identifiable. |
| Fixed structured wire | A reader can derive exact note locations and message boundaries. | No event scan or framing search is necessary (`sdk/rs/src/read.rs:7-25`). | The wire cannot carry full contract text. |
| On-chain negotiation records | A grant holder can reconstruct the record from chain data. | No separate transcript log is necessary for reconstruction. The bearer grant still needs delivery. | Each write needs a proof, chain inclusion, storage, and a sequential note allocation. |
| Atomic acceptance and payment | The design removes an accepted-but-unpaid on-chain state. | Both changes enter one proof (`sdk/rs/src/channel.rs:515-523`). | The pool does not understand agreement semantics. Rust must compare the amounts (`sdk/rs/src/channel.rs:545-555`). |
| Bearer viewing grant | A remote holder can reveal without the grantor's local state or pool key. | Disclosure is portable and channel-scoped. | The grant has no recipient encryption, expiry, or revocation (`sdk/rs/src/disclosure.rs:45-74`). |
| Rust subprocess boundary | Python passes paths and opaque handles instead of key values. | Protocol logic and key operations remain in Rust (`sdk/py/src/erebus/_seam.py:59-92`). | One-shot processes require a local secret-state store and recovery logic. |
| Client-side offer state machine | STRK20 remains a general note pool with no offer semantics. | Erebus can evolve without a new pool contract. | A hostile or different client can write semantically inconsistent records that still satisfy pool rules (`sdk/rs/src/channel.rs:545-555`). |

## Integration levels

### Level 1: Full transcript disclosure

This level exists in the Rust implementation and offline tests. The partner receives a
bearer grant and reads the complete negotiation and settlement
(`sdk/rs/tests/disclosure.rs:179-250`).

This level requires trust in the partner. Wire v2 still needs live end-to-end evidence and
independent review before production use.

### Level 2: Recipient-bound disclosure

This level does not exist yet. It needs grant encryption or another access-control mechanism
for a named recipient, plus expiry and revocation semantics.

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
| 1 | Complete a live wire-v2 offer, counter, settlement, and reveal. Add independent review. | Defensible encrypted-term demonstrations. |
| 2 | Randomize the spare wire-v2 bits and remove the fixed marker fingerprint. | Better traffic-shape privacy for every case. |
| 3 | Bind participants, terms, policy, and settlement in an authenticated or ZK receipt. | Outcome-only verification and platform integrations. |
| 4 | Encrypt grants to recipients. Define expiry, revocation, and delivery. | Safer third-party disclosure. |
| 5 | Design repeat-deal framing and per-deal disclosure. Add state recovery. | Recurring B2B payments, payroll, and repeat procurement. |
| 6 | Add general note selection and change-note construction. | Flexible prices and recurring balances. |
| 7 | Reduce the number of on-chain interactive rounds. | Lower-value and higher-frequency commerce. |
| 8 | Define and publish the `memo_hash` preimage and signature convention. | Interoperable external contract and delivery commitments. |
| 9 | Harden key custody, trusted endpoint deployment, monitoring, and crash recovery. | Production operator deployments. |

The measured fee and proving-time entries in `docs/friction.md` are engineering snapshots.
They are not durable product prices or performance guarantees (`docs/friction.md:1190-1223`,
`docs/friction.md:1409-1429`).

## Accurate positioning

Use this description:

> Erebus is an experimental client protocol for encrypted bilateral negotiation and one-way
> shielded settlement over STRK20. It binds acceptance and payment in one action set and can
> disclose one full channel through a bearer viewing grant.

Do not say these things:

- "Erebus hides that a negotiation happened." The current wire shape is identifiable.
- "Erebus provides fair exchange." It does not prove off-chain delivery.
- "The viewing grant proves both parties and their policy." Participant metadata is not
  independently authenticated by the current reveal path.
- "The grant is access-controlled for the named grantee." It is a bearer secret.
- "This is a Rust rewrite of the Starknet privacy SDK." It is a selective client.
- "This is production-ready." Wire-v2 live evidence and security review remain open.
- "Self-hosting the prover removes every trust assumption." The write RPC and pool auditor
  remain in the confidentiality model.

## STRK20 references

- [STRK20 pool model](https://strk20-by-example.org/what-is-strk20)
- [Notes and nullifiers](https://strk20-by-example.org/notes-and-nullifiers)
- [Channels and subchannels](https://strk20-by-example.org/channels-and-subchannels)
- [Selective disclosure and screening](https://strk20-by-example.org/compliance)
