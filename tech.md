# Erebus Rust SDK: source-grounded technical explanation

This document describes the repository as checked on 2026-08-05. A citation such as
`sdk/rs/src/client.rs:575-646` means the claim is visible at those source lines. “I’m
inferring…” marks reasoning that the code does not state directly. Operational test results
are labelled as execution evidence rather than disguised as source facts.

## Orientation: what Erebus is and how one deal works

### The shortest useful explanation

Erebus is a two-party negotiation and shielded-payment protocol implemented as a client
layer over the STRK20 privacy pool. Two agents write offers and counters into pool notes;
the payer later writes an acceptance and transfers the agreed private value in one action
set; either party can subsequently export a secret that lets a third party reconstruct that
one relationship without gaining spending authority. Those are the seven operations exposed
by the Rust API: open, propose, counter, read, accept-and-settle, grant, and reveal
(`sdk/rs/src/client.rs:538-573`).

It is not a separate payment token and the implemented write path does not submit an Erebus
contract call. It builds the existing STRK20 `ClientAction` variants and ultimately submits
the prover-produced server actions to the pool's `apply_actions` entrypoint
(`sdk/rs/src/actions.rs:288-313`, `sdk/rs/src/execution.rs:171-195`). Erebus's contribution is
the meaning and lifecycle imposed on those actions: which notes represent negotiation data,
which note represents payment, how the two are made atomic, how a transcript is reconstructed,
and how access to that transcript can be delegated (`sdk/rs/src/wire.rs:1-45`,
`sdk/rs/src/channel.rs:515-610`, `sdk/rs/src/disclosure.rs:1-36`).

### The three layers to keep separate

1. **STRK20 is the private state-transition layer.** It supplies channels, token-specific
   subchannels, encrypted notes, nullifiers, action compilation, proof verification, and
   `apply_actions`. The Rust client models the pool's ten action variants and their Cairo
   serialization (`sdk/rs/src/actions.rs:288-434`).
2. **Erebus is the application protocol over those primitives.** It gives five consecutive
   zero-value notes the meaning “one offer, counter, or acceptance,” defines reply and expiry
   rules, and combines the final acceptance with the payment spend/create actions
   (`sdk/rs/src/wire.rs:21-35`, `sdk/rs/src/negotiation.rs:163-193`,
   `sdk/rs/src/channel.rs:515-610`).
3. **The agent-facing stack is transport and policy.** MCP exposes operations such as
   `open_channel`, `propose_offer`, and `accept_and_settle`; Python adapts those calls to a
   one-request Rust subprocess; Rust alone performs the protocol derivations and network
   execution (`mcp-server/src/erebus_mcp/tools.py:89-141`,
   `mcp-server/src/erebus_mcp/seam_client.py:1-17`, `sdk/py/src/erebus/_seam.py:95-165`).

The distinction matters when explaining security. STRK20 proves validity of the underlying
private-note transition. Erebus's Rust client checks the application meaning—for example,
that the payment amount equals the accepted amount—before asking for that proof
(`sdk/rs/src/channel.rs:545-555`). The pool does not independently understand that a group of
zero-value notes means “Alice accepted Bob's offer”; that interpretation lives in the Erebus
wire decoder and negotiation state machine (`sdk/rs/src/read.rs:175-230`,
`sdk/rs/src/negotiation.rs:163-193`).

### The core mental model: one pool, two kinds of notes

A normal STRK20 value note carries private value. Erebus additionally writes **data notes**:
zero-amount encrypted-note actions whose salts contain fragments of a negotiation record.
The pool note has no application payload field, so wire v2 first packs the fixed fields
`type | replyTo | createdAt | amount | deadline | memoHash`, encrypts and authenticates the
50-byte plaintext, and splits the result across five salts (`sdk/rs/src/wire.rs:7-35`). The
`memoHash` is only a 128-bit commitment to detail held elsewhere; free-form prose and the
preimage of that commitment are not stored by this wire (`sdk/rs/src/client.rs:938-948`).

Data notes and payment notes use the same token subchannel and contiguous note-index space,
but the Rust constructors keep their salt rules separate. A data note has zero amount and a
structured salt; a value note requires fresh random salt because structured or reused salt
on differing amounts would leak their difference (`sdk/rs/src/wire.rs:37-45`,
`sdk/rs/src/channel.rs:502-512`). During settlement, the five acceptance notes remain on the
fixed message grid and the payment note is placed immediately after them
(`sdk/rs/src/channel.rs:613-620`).

Channels are directional. Alice-to-Bob and Bob-to-Alice have different channel keys and
therefore different note locations. Each party derives its outgoing key and learns the
reverse key from the counterparty's encrypted channel information; a full conversation reader
therefore needs both keys (`sdk/rs/src/disclosure.rs:24-30`). Inside each direction, a
subchannel is selected by token rather than by conversational topic
(`sdk/rs/src/channel.rs:282-295`).

### One complete deal, step by step

1. **Fund the payer.** Before negotiation can settle, the payer needs shielded notes whose
   denominations contain an exact subset equal to the price. The MVP shielding helper
   registers when necessary, opens a self-channel and token subchannel, deposits public value,
   and creates one encrypted value note in one action set (`sdk/rs/src/channel.rs:329-364`).
   Settlement currently constructs no change note, so total balance alone is insufficient;
   the client explicitly runs exact note selection (`sdk/rs/src/client.rs:819-831`).

2. **Open both directions.** `open_channel(counterparty)` verifies the caller's registration,
   looks up the counterparty's registered public key, derives the directional channel, and
   submits registration-if-needed plus `OpenChannel` and `OpenSubchannel`
   (`sdk/rs/src/client.rs:575-626`). The method then stores an opaque local handle containing
   the channel metadata and key needed by later calls (`sdk/rs/src/client.rs:627-645`). Because
   the conversation is bidirectional, the other party opens its reverse direction before the
   client can reconstruct both sides (`sdk/rs/src/client.rs:766-781`).

3. **Write an offer.** The caller supplies amount, token, deadline, and `memo_hash`. Rust
   validates those terms, synchronizes the next contiguous note index, constructs an `Offer`
   wire message, encrypts it into data notes, executes the action set, and commits the advanced
   cursor only after the transaction is accepted (`sdk/rs/src/client.rs:648-695`).

4. **Read and counter.** A reader derives exact note IDs from channel key, token, and index;
   it does not scan events or enumerate the pool (`sdk/rs/src/read.rs:7-25`,
   `sdk/rs/src/read.rs:149-172`). `counter_offer` first proves the referenced item is a
   counterparty offer or counter, then writes a new message containing its index in `replyTo`;
   it does not mutate the earlier record (`sdk/rs/src/client.rs:698-763`).

5. **Accept as the payer.** The caller can accept only a known, live counterparty offer. The
   client discovers its spendable private notes at a proof-compatible block and selects an
   exact subset for the offered amount (`sdk/rs/src/client.rs:789-831`). It then builds one
   ordered action set containing the input-note spends, five acceptance data notes, and the
   recipient's value note; the amount-equality check prevents an atomic but semantically
   inconsistent underpayment (`sdk/rs/src/client.rs:845-864`,
   `sdk/rs/src/channel.rs:545-610`). After chain acceptance, the local channel becomes terminal
   and repeated settlement is rejected (`sdk/rs/src/client.rs:865-875`,
   `sdk/rs/src/client.rs:797-801`).

6. **Simulate, prove, and submit.** Every write is first compiled against a historical proving
   block. Rust builds the virtual proof invocation, asks the proving service for a proof,
   rejects any mismatch between locally simulated and prover-returned server actions, checks
   proof freshness, estimates resources, signs the account transaction, submits
   `apply_actions`, and waits for an accepted receipt (`sdk/rs/src/execution.rs:132-238`).

7. **Disclose if required.** Either party can export a bearer viewing grant containing both
   directional channel keys for one token. A holder can use it to locate, decrypt, and
   reconstruct that channel's offers, counters, acceptance, and settlement, but the grant
   carries no pool private key and therefore cannot produce nullifiers or spend notes
   (`sdk/rs/src/disclosure.rs:24-36`, `sdk/rs/src/disclosure.rs:45-74`,
   `sdk/rs/src/client.rs:918-934`).

### What crosses each software boundary

An agent calls MCP tools using public terms and opaque identifiers. The real MCP backend runs
the blocking Python seam away from the event loop, and the seam starts `erebus-cli` once per
request with JSON on standard input and expects one JSON envelope on standard output
(`mcp-server/src/erebus_mcp/seam_client.py:1-17`, `sdk/py/src/erebus/_seam.py:120-165`,
`sdk/rs/src/bin/erebus_cli.rs:429-450`). Python receives paths to the pool and account key
files, not their contents; the CLI request type likewise accepts file paths
(`mcp-server/src/erebus_mcp/config.py:41-57`, `sdk/rs/src/bin/erebus_cli.rs:84-95`). Persistent
channel keys remain in Rust-owned, per-handle state protected by exclusive locks and atomic
replacement (`sdk/rs/src/state.rs:192-225`, `sdk/rs/src/state.rs:230-248`,
`sdk/rs/src/state.rs:400-446`).

This is the intended live path, not the default behavior of every checkout. The MCP server
defaults to its mock backend; selecting `EREBUS_BACKEND=seam` enables and validates the real
Rust configuration (`mcp-server/src/erebus_mcp/config.py:10-13`,
`mcp-server/src/erebus_mcp/config.py:72-113`). Therefore a successful agent demo is not by
itself evidence that the Rust, prover, RPC, or pool path ran.

### What “private” means here—and what it does not mean

Wire v2 encrypts and authenticates negotiation contents before placing ciphertext fragments
in public salts (`sdk/rs/src/wire.rs:3-17`, `sdk/rs/src/wire.rs:29-35`). Note discovery is
keyed: someone without the channel key cannot directly compute the locations the reader asks
for (`sdk/rs/src/read.rs:7-19`). **I'm inferring the observer consequence from those two
mechanisms:** an observer without the channel key cannot decode the fixed offer fields or use
the Erebus reader to locate the transcript; verify this claim against a transaction trace and
an independent wire-v2 review, neither of which the code itself supplies.

Wire v2 does not make the pool interaction invisible. An observer still sees the submitting
account, transaction timing and action shape, and the five public salt values; the current
fifth-chunk shape is distinguishable from uniformly random salts
(`sdk/rs/tests/wire_v2_fingerprint.rs:31-75`). Consequently, encrypted terms are implemented,
but relationship-graph and cadence privacy are **not yet demonstrated** by this repository.

Atomicity is narrower than semantic proof. The final acceptance and payment share one action
set, so the client does not intentionally submit one without the other
(`sdk/rs/src/channel.rs:515-523`). The equality between accepted amount and payment amount is
a Rust-side validation, however, not a statement that the STRK20 circuit understands the
negotiation record (`sdk/rs/src/channel.rs:545-555`). A disclosed record can reconstruct and
locally check the encoded history; the current implementation does not produce a separate ZK
receipt proving the business meaning, participant claims, disclosure policy, and settlement
consistency to an external verifier.

The viewing grant is also deliberately a bearer secret, not recipient-encrypted capability.
Its `grantee` value is metadata at the outer API, while possession of the serialized grant is
what permits reading (`sdk/rs/src/client.rs:878-915`, `sdk/rs/src/disclosure.rs:45-74`). Its
checksum detects incompatible or edited grant data, but it is not a signature that
authenticates who issued the grant (`sdk/rs/src/disclosure.rs:106-146`).

### What Erebus is not

- It is not a free-form encrypted-chat protocol: the on-chain wire contains six fixed fields
  and only a hash of any external memo (`sdk/rs/src/wire.rs:21-27`,
  `sdk/rs/src/client.rs:938-948`).
- It is not a general Rust interface for every STRK20 operation: the high-level trait is the
  seven-method negotiation surface (`sdk/rs/src/client.rs:538-573`).
- It is not a production security claim: wire v2 still needs live on-chain exercise,
  independent review, and stronger traffic-shape privacy
  (`sdk/rs/tests/wire_v2_fingerprint.rs:31-75`).
- It is not cryptographic proof that two businesses meant the same thing by `memoHash`; that
  field commits to off-chain detail whose preimage and semantics are outside this wire
  (`sdk/rs/src/client.rs:938-948`).

### Why it is designed this way: choices and tradeoffs

This table records the reasons stated by the source and architecture notes. “Cost” means a
mechanical consequence of the choice, not a recommendation that the choice was right or wrong.

| Design choice | Why the repository chose it | What the choice provides | Concrete cost or limit |
|---|---|---|---|
| Reuse STRK20 notes and actions instead of deploying an Erebus application contract | A pool note has no payload field, but its salt is client-writable. `InvokeExternal` would publish a distinct target contract and off-chain transport would move the negotiation graph outside the reconstructable pool record (`docs/friction.md:207-223`). | Negotiation and payment can share the pool's action compiler, proof, and `apply_actions` transition (`sdk/rs/src/actions.rs:288-434`, `sdk/rs/src/execution.rs:171-195`). | The pool proves note-state validity, not the business interpretation of an offer. Erebus must validate amount agreement and interpret the transcript in client code (`sdk/rs/src/channel.rs:545-555`, `sdk/rs/src/read.rs:175-230`). |
| Encode one message as five zero-amount data notes | Salt is the note's only application-writable lane. Five 119-bit payload chunks fit the 400-bit message, version byte, ciphertext, and 128-bit authentication tag (`sdk/rs/src/wire.rs:7-35`, `sdk/rs/src/wire.rs:56-95`). | The record stays on-chain, fixed-width, directly seekable, and reconstructable by a channel-key holder (`sdk/rs/src/read.rs:149-184`). | Every message permanently consumes five sequential note slots; its regular shape remains fingerprintable and each write pays pool execution/proving costs (`sdk/rs/src/wire.rs:34-45`, `sdk/rs/tests/wire_v2_fingerprint.rs:31-75`). |
| Encrypt with AES-256-GCM-SIV and derive context from chain, pool, channel, token, and index | A failed attempt may reuse the same note index with different terms. The architecture selected a nonce-misuse-resistant construction so that this retry case is not catastrophic (`ARCHITECTURE.md:381-384`). | Terms are authenticated as well as encrypted, and ciphertext is bound to its intended protocol context (`sdk/rs/src/wire.rs:383-476`). | The five-note shape, submitting account, timing, and ciphertext-bearing public salts remain observable; encryption does not supply traffic-analysis resistance (`sdk/rs/tests/wire_v2_fingerprint.rs:31-75`). |
| Keep the wire fixed-width rather than store prose | Offers require six bounded fields, and fixed v2 stride lets a reader calculate `5k..5k+4` without scanning for message boundaries (`sdk/rs/src/wire.rs:21-35`). | Deterministic framing, bounded decoding, and direct keyed reads (`sdk/rs/src/read.rs:149-184`). | Arbitrary terms must live elsewhere and be represented only by the 128-bit `memo_hash`; adding fields is a wire-version change (`sdk/rs/src/client.rs:938-948`). |
| Use directional channel keys and token-specific subchannels | This follows the pool's channel construction: the sender derives a key using its private key, while the recipient learns it through encrypted channel information (`sdk/rs/src/channel.rs:19-24`). Token is already part of the subchannel, so it need not occupy message bits (`sdk/rs/src/channel.rs:282-295`). | A reader holding one directional key learns only that direction and token-scoped note locations (`sdk/rs/src/read.rs:99-110`). | A complete negotiation requires two channel keys and both parties to establish reverse directions; a disclosure grant must carry both (`sdk/rs/src/disclosure.rs:24-30`). |
| Use keyed discovery with contiguous indices instead of event scanning | The note ID is derived from the channel key, token, and index, so an authorized reader can request exact slots without enumerating everyone else's notes (`sdk/rs/src/read.rs:7-19`, `sdk/rs/src/read.rs:149-172`). Contiguity makes the first absent slot a sound end marker (`sdk/rs/src/read.rs:21-25`). | Discovery does not require publishing an index that maps all pool activity into relationships (`sdk/rs/src/channel.rs:26-30`). | Every writer must serialize allocation and never create gaps; a gap makes later notes undiscoverable, while direct RPC discovery costs repeated reads (`sdk/rs/src/channel.rs:265-269`, `sdk/rs/src/read.rs:149-172`). |
| Put acceptance and payment in one action set | The design wants no committed acceptance without its corresponding payment. One action set shares one proof and one `apply_actions` transition (`sdk/rs/src/channel.rs:515-523`). | Both state changes land or neither does, and the Rust builder enforces the pool's spend-before-create phase ordering before proving (`sdk/rs/src/channel.rs:521-523`, `sdk/rs/src/action_set.rs:121-177`). | Atomicity alone does not show that the terms and payment agree, so Rust must perform a separate equality check (`sdk/rs/src/channel.rs:545-555`). Settlement also leaves the message cursor off-grid, making the current channel a one-deal channel (`sdk/rs/src/channel.rs:613-623`). |
| Require an exact subset of existing notes and create no change | This is the behavior implemented by the MVP selector and settlement construction: it selects notes totalling exactly the offer and creates only the recipient payment note (`sdk/rs/src/client.rs:819-860`). **I could not find a source comment establishing that no-change was chosen as a cryptographic necessity; treat it as an MVP implementation limit, not an STRK20 requirement.** | The settlement builder avoids introducing a second owner change note and another allocation path (`sdk/rs/src/client.rs:819-860`). | A payer can own enough total value and still be unable to settle—for example, a single larger note cannot pay a smaller price (`sdk/rs/src/client.rs:819-829`). |
| Implement the write path in Rust while retaining TypeScript as an oracle | The crate states that upstream Rust covers discovery but not action building, Cairo serialization, signing, or proving; it also states that silent cryptographic divergence requires known-answer tests (`sdk/rs/src/lib.rs:3-20`). | The agent's critical protocol and key-handling path runs in Rust, while fixtures pin agreement with independent Cairo, TypeScript, and starknet.js behavior (`sdk/rs/tests/cairo_conformance.rs:1-13`, `sdk/rs/tests/wire_codec.rs:1-10`, `sdk/rs/tests/clientaction_serde.rs:1-11`). | **I'm inferring the maintenance cost from the duplicated implementations:** every upstream format change must be detected and reconciled in Rust and the fixtures; verify this by updating the pinned sibling revision and rerunning the differential tests. |
| Encode invariants in Rust types and builders | The code separates `RandomSalt` from `NoteSalt`, makes `ActionSet` constructible only through its validating builder, and keeps the pool secret behind `PoolIdentity` without an accessor (`sdk/rs/src/actions.rs:25-104`, `sdk/rs/src/action_set.rs:97-177`, `sdk/rs/src/channel.rs:98-123`). | Invalid salt use, phase regression, duplicate invoke phases, and key leakage through the ordinary API become harder or impossible to express (`sdk/rs/src/action_set.rs:131-177`, `sdk/rs/src/channel.rs:7-17`). | Low-level action structs remain public, so a caller bypassing the high-level constructors can still assemble some semantically dangerous values; the type protection is strongest on the intended `Channel`/`Client` path (`sdk/rs/src/actions.rs:164-286`, `sdk/rs/src/channel.rs:502-512`). |
| Put a one-shot subprocess between Python agents and Rust | Key-file values need not enter Python, Rust can own its Tokio runtime, and the seam can return an explicit JSON error envelope instead of maintaining a PyO3 ABI (`sdk/py/src/erebus/_seam.py:59-92`, `sdk/py/src/erebus/_seam.py:95-165`, `sdk/rs/src/bin/erebus_cli.rs:429-450`; rationale recorded at `ARCHITECTURE.md:184-221`). | The agent layer passes public data, paths, and opaque handles while protocol derivations and keys remain below the process boundary (`mcp-server/src/erebus_mcp/config.py:41-57`, `sdk/rs/src/channel.rs:7-17`). | Each call pays process startup and JSON serialization; state that an in-process library would retain in memory instead requires a protected filesystem store (`sdk/py/src/erebus/_seam.py:95-165`, `sdk/rs/src/state.rs:192-248`). |
| Persist opaque handles with an exclusive lease and commit after chain success | A one-shot CLI must recover channel keys and the next note index across processes, while concurrent calls must not allocate the same slot (`sdk/rs/src/state.rs:192-248`, `sdk/rs/src/state.rs:425-446`). | Agent-visible handles reveal no channel key, per-handle locking serializes cursor changes, and atomic replacement avoids partial state files (`sdk/rs/src/state.rs:192-225`, `sdk/rs/src/state.rs:400-446`). | The local OS account and state directory become trust and availability boundaries. A crash after chain inclusion but before `commit` can leave local state stale; the current test suite does not exercise that recovery case, as recorded later in Part 5. |
| Make disclosure a self-contained bearer grant | `reveal` is meant to work from the grant and chain data without the grantor's state directory or pool private key. Both directional keys are necessary to reconstruct both halves of the conversation (`sdk/rs/src/disclosure.rs:24-36`, `sdk/rs/src/client.rs:918-934`). | The holder gets channel-scoped read capability but cannot compute spend nullifiers without the owner's pool private key (`sdk/rs/src/disclosure.rs:32-36`). | Possession, not the `grantee` metadata, controls access; copying or leaking the grant discloses the record, and the checksum is integrity checking rather than issuer authentication (`sdk/rs/src/disclosure.rs:45-74`, `sdk/rs/src/disclosure.rs:106-146`). |
| Trust the prover and write RPC with the pool private key | This is inherited from the upstream proving interface: the virtual invocation contains the pool key, and the `compile_actions` preflight sends the same secret to its RPC endpoint (`sdk/rs/src/prover.rs:3-14`). | Those services can compile and prove the private transition without receiving the separate Starknet account signing key (`sdk/rs/src/execution.rs:132-174`, `sdk/rs/src/execution.rs:222-231`). | Both endpoints can decrypt what the pool key protects and therefore sit inside the confidentiality trust boundary, even though the account signature remains separately necessary to submit the transaction (`sdk/rs/src/prover.rs:3-14`, `sdk/rs/src/execution.rs:222-231`). |
| Accept STRK20's pool-wide auditor escrow and add a narrower channel grant | Registration writes the pool private key encrypted to the configured auditor; Erebus cannot opt out of that pool rule (`sdk/rs/src/channel.rs:126-140`). The application grant instead releases only two directional channel keys for one token (`sdk/rs/src/disclosure.rs:7-22`). | The application can disclose one relationship without handing the recipient the pool-wide spending/decryption root (`sdk/rs/src/disclosure.rs:15-36`). | The pool auditor retains broader visibility across the registered identity's pool history, while the application grant adds another secret that must be delivered and protected (`sdk/rs/src/channel.rs:126-136`, `sdk/rs/src/disclosure.rs:45-74`). |

## 0. Scope boundary: what this Rust SDK is, and is not

### The one-sentence framing to use

**Say this:** “The Rust crate is an Erebus-specific STRK20 client: it independently implements
the selected privacy-pool write, read, proving, signing, and RPC primitives needed for the
Erebus flow, pins those primitives against Cairo/TypeScript/starknet.js oracles, and adds an
original two-party negotiation, persistence, and selective-disclosure protocol; it is not a
full port or drop-in replacement for StarkWare’s TypeScript SDK.” The crate’s own module
documentation says that upstream `discovery-core` covers reads while no upstream Rust write
side builds `ClientAction`s, serializes calldata, signs, or calls the prover
(`sdk/rs/src/lib.rs:3-20`); the high-level Rust surface is seven negotiation methods, not the
upstream general-purpose transfer API (`sdk/rs/src/client.rs:538-573`).

Do **not** call it “our Rust rewrite of the Starknet privacy SDK.” Upstream exports a broad
`createPrivateTransfers` API, discovery/indexer providers, history, OHTTP, and classifiers
(`../starknet-privacy/sdk/src/index.ts:1-52`), whereas this crate exposes only its nineteen
modules and one CLI (`sdk/rs/src/lib.rs:22-42`, `sdk/rs/Cargo.toml:38-40`). “Rewrite” therefore
overstates compatibility and understates the original protocol layered above the pool.

### What is a port or compatibility reimplementation

- The Poseidon domain tags and every hash preimage in `hashes.rs` are direct ports of
  `packages/privacy/src/hashes.cairo` (`sdk/rs/src/hashes.rs:1-16`;
  `../starknet-privacy/packages/privacy/src/hashes.cairo:9-39`). The same subset already has
  an upstream Rust implementation in `discovery-core`, so this is not the first Rust
  expression of those formulas (`../starknet-privacy/crates/discovery-core/src/privacy_pool/hashes.rs:64-223`).
- Additive note, subchannel, outgoing-channel, and ECDH channel-info decryption reproduce the
  Cairo/read-side behavior (`sdk/rs/src/decrypt.rs:21-35`, `sdk/rs/src/decrypt.rs:103-200`).
  They were reimplemented rather than imported because upstream `discovery-core` pins a
  `starknet-rust` fork and pulls in the provider stack that this crate deliberately avoided
  (`sdk/rs/src/decrypt.rs:6-19`, `sdk/rs/Cargo.toml:8-36`).
- The ten `ClientAction` variants, their enum indices, field order, span encoding, and phase
  mapping mirror Cairo and upstream TypeScript serialization (`sdk/rs/src/actions.rs:288-434`;
  `../starknet-privacy/packages/privacy/src/actions.cairo:245-315`;
  `../starknet-privacy/sdk/src/internal/serialization.ts:9-28`).
- `compile_actions` calldata, the virtual pool-account `__execute__` wrapper, proof invocation,
  Stark ECDSA, v3 transaction hashing, proving RPC, screening suffix, and final
  `apply_actions` call independently reproduce the upstream execution path
  (`sdk/rs/src/calldata.rs:25-82`, `sdk/rs/src/execution.rs:132-239`;
  `../starknet-privacy/sdk/src/internal/proof-invocation-factory.ts:88-195`;
  `../starknet-privacy/sdk/src/internal/private-transfers.ts:94-136`).
- Wire v1 is a port of this repository’s TypeScript salt codec, not an upstream STRK20
  primitive (`sdk/rs/src/wire.rs:1-5`, `sdk/ts/src/channel/wire.ts:1-46`).

### What is original Erebus protocol work

The 400-bit offer/counter/accept schema, its five-note AES-256-GCM-SIV wire v2, the fixed
message grid, and the use of zero-value note salts as a payload lane are Erebus-specific;
the module explicitly says a pool note has no payload field and describes the five-note
envelope (`sdk/rs/src/wire.rs:7-45`). `OfferBook` supplies deadlines, reply semantics,
direction-aware IDs, and terminal settlement that the pool does not know about
(`sdk/rs/src/negotiation.rs:163-193`, `sdk/rs/src/negotiation.rs:231-272`).

The following are also original client/protocol machinery: `ActionSetBuilder`’s early mirror
of pool phase/replay constraints (`sdk/rs/src/action_set.rs:1-28`), `SubchannelCursor`’s
contiguous allocator (`sdk/rs/src/subchannel.rs:1-32`), atomic accept-plus-payment composition
(`sdk/rs/src/channel.rs:515-610`), opaque-handle state and lease/commit persistence
(`sdk/rs/src/state.rs:174-228`, `sdk/rs/src/state.rs:425-446`), a two-direction bearer viewing
grant (`sdk/rs/src/disclosure.rs:45-88`), the high-level `Client` workflow
(`sdk/rs/src/client.rs:575-935`), and the protocol-2 one-shot CLI
(`sdk/rs/src/bin/erebus_cli.rs:27-82`, `sdk/rs/src/bin/erebus_cli.rs:429-450`).

### What upstream functionality was deliberately not ported

The Rust high-level client does not expose upstream’s general action compiler, arbitrary open
notes, withdrawals, private swaps/DeFi invokes, compute-and-invoke flow, discovery service,
history/indexing, OHTTP, paymasters, or general change construction. Although the low-level
Rust enum can serialize all ten Cairo variants, the frozen high-level trait exposes only
channel negotiation, settlement, and disclosure (`sdk/rs/src/actions.rs:288-313`;
`sdk/rs/src/client.rs:538-573`). The configured client is also single-token
(`sdk/rs/src/client.rs:37-59`). The repository calls these MVP limits and explicitly excludes
general note selection/change, multi-token negotiation, paymasters, and production custody
(`sdk/rs/README.md:103-121`, `CLAUDE.md:113-132`).

The TypeScript static-static ECDH helper in this repository was not ported into the active
Rust path. It describes a planned off-chain shared secret (`sdk/ts/src/crypto/channel-secret.ts:1-29`),
while the Rust protocol uses the directional channel key that the Cairo pool derives from the
sender’s private key and sends to the recipient via ephemeral-static ECDH
(`sdk/rs/src/hashes.rs:74-93`, `sdk/rs/src/decrypt.rs:152-175`).

### Documentation disagreements

- `CLAUDE.md` and `sdk/rs/README.md` still call the Python seam “protocol 1”
  (`CLAUDE.md:93-97`, `sdk/rs/README.md:117-121`), but the current Python and Rust code both
  say and return protocol 2 (`sdk/py/src/erebus/_seam.py:15-18`,
  `sdk/rs/src/bin/erebus_cli.rs:202-210`). The running code wins.
- `sdk/ts/src/interface.ts` is an older, non-shipping interface with a string memo, nonce,
  `withdrawn` status, and a grant method returning `void`
  (`sdk/ts/src/interface.ts:39-57`, `sdk/ts/src/interface.ts:151-172`). Current Rust carries a
  128-bit `memo_hash`, no offer nonce/withdrawal, and returns a bearer grant
  (`sdk/rs/src/client.rs:938-1079`). Each language compiler obeys its source, but the repo says
  `/sdk/ts` “ships nothing,” so it is not the current product contract (`README.md:84-90`).
- `ARCHITECTURE.md` says the system hides existence, participants, and cadence
  (`ARCHITECTURE.md:466-476`), while the later README calls relationship privacy a target and
  the fingerprint test proves the fifth salt is distinguishable (`README.md:51-58`,
  `sdk/rs/tests/wire_v2_fingerprint.rs:31-58`). The later test-backed statement is the honest
  one.

### Four source comments worth quoting verbatim

> “`discovery-core` covers the *read* side — hashes, storage slots, decryption, note discovery
> — but there is no Rust write side: nothing builds `ClientAction`s, serialises Cairo calldata,
> signs the invoke, or calls the proving service. This crate is that gap.”

That is the crate’s own scope claim (`sdk/rs/src/lib.rs:5-9`).

> “The invocation handed to `starknet_proveTransaction` carries the pool private key in
> plaintext at `calldata[5]` — verified, not assumed.”

That is the custody boundary (`sdk/rs/src/prover.rs:3-11`).

> “Atomicity puts the acceptance and the payment in one proof, so both land or neither does.
> It says nothing about them *agreeing*.”

That is why the amount comparison is separate from atomic composition
(`sdk/rs/src/channel.rs:545-555`).

> “Keep the returned lease alive through any async operation that uses or advances its
> cursor.”

That is the ownership/concurrency rule for persistent channel state
(`sdk/rs/src/state.rs:230-232`).

## 1. Module map and reading order

Read these in the order below. “Depends on” names the important protocol dependency, not
every imported standard-library item.

| Order | File and layer | Responsibility; public surface; key dependencies |
|---:|---|---|
| 1 | `lib.rs` — crate boundary | Declares the crate’s purpose, forbids unsafe code, and exports all nineteen library modules (`sdk/rs/src/lib.rs:1-42`). |
| 2 | `hashes.rs` — cryptographic primitives | Exposes Poseidon `hash` plus fifteen Cairo-compatible derivations; depends only on Starknet felt/Poseidon primitives (`sdk/rs/src/hashes.rs:18-20`, `sdk/rs/src/hashes.rs:69-263`). |
| 3 | `actions.rs` — Cairo wire model | Defines salt/entropy newtypes, ten action-input structs, `ClientAction`, phase lookup, and Cairo serialization; depends on felts and the Cairo enum/field order (`sdk/rs/src/actions.rs:25-162`, `sdk/rs/src/actions.rs:164-434`). |
| 4 | `action_set.rs` — local protocol invariants | Exposes `ActionSet`, `ActionSetBuilder`, and `ActionSetError`; depends on action phase and replay-protection classification (`sdk/rs/src/action_set.rs:30-178`). |
| 5 | `subchannel.rs` — index allocation | Exposes `SubchannelCursor` and `IndexError`; depends on wire message width and mirrors contiguity/write-once rules (`sdk/rs/src/subchannel.rs:34-164`). |
| 6 | `wire.rs` — negotiation codec | Exposes wire versions, message types, contexts, constants, v1 compatibility functions, and v2 authenticated codec; depends on `NoteSalt`, HKDF-SHA-256, and AES-GCM-SIV (`sdk/rs/src/wire.rs:47-105`, `sdk/rs/src/wire.rs:118-229`, `sdk/rs/src/wire.rs:383-534`). |
| 7 | `decrypt.rs` — STRK20 read crypto | Exposes note unpack/decrypt and channel/subchannel/outgoing-info recovery; depends on `hashes` and Stark-curve point operations (`sdk/rs/src/decrypt.rs:37-40`, `sdk/rs/src/decrypt.rs:42-200`). |
| 8 | `channel.rs` — action composition | Exposes identities, counterparties, channels, owned notes, setup/payment/acceptance inputs, and constructors for setup, shielding, messages, settlement, and grants; depends on actions, builder, hashes, wire, cursor, and disclosure (`sdk/rs/src/channel.rs:32-45`, `sdk/rs/src/channel.rs:98-709`). |
| 9 | `read.rs` — keyed transcript reconstruction | Exposes `NoteSource`, `ChannelReader`, read errors, and two-direction `reconstruct`; depends on hashes, decryption, wire, and negotiation (`sdk/rs/src/read.rs:28-35`, `sdk/rs/src/read.rs:38-321`). |
| 10 | `negotiation.rs` — client-only state machine | Exposes direction-aware `OfferId`, statuses, errors, and `OfferBook`; depends on decoded `WireMessage`s and contains no chain code (`sdk/rs/src/negotiation.rs:25-42`, `sdk/rs/src/negotiation.rs:95-302`). |
| 11 | `disclosure.rs` — scoped read capability | Exposes secret-bearing grant and disclosed-record types plus `reveal`; depends on both directional readers and `OfferBook` (`sdk/rs/src/disclosure.rs:40-88`, `sdk/rs/src/disclosure.rs:175-337`). |
| 12 | `calldata.rs` — ABI assembly | Exposes selectors and exact `compile_actions`, single-call, proof-`__execute__`, screening, and `apply_actions` layouts; depends on `ActionSet` and prover additional data (`sdk/rs/src/calldata.rs:12-17`, `sdk/rs/src/calldata.rs:18-102`). |
| 13 | `tx.rs` — Starknet transaction model | Exposes v3 invoke/resource types, the privacy-specific proof-facts-aware hash, `PoolInvocation`, and signed RPC wire types; depends on Poseidon and Stark signatures (`sdk/rs/src/tx.rs:23-25`, `sdk/rs/src/tx.rs:50-350`). |
| 14 | `signing.rs` — account signatures | Exposes public-key derivation, sign, verify, and `SigningError`; depends on `starknet_crypto` (`sdk/rs/src/signing.rs:1-12`, `sdk/rs/src/signing.rs:22-84`). |
| 15 | `prover.rs` — proving transport | Exposes block IDs, proof/result/screening types, `ProvingService`, and retry-classified errors; depends on async HTTP and signed invoke wire data (`sdk/rs/src/prover.rs:23-28`, `sdk/rs/src/prover.rs:30-220`). |
| 16 | `rpc.rs` — Starknet transport | Exposes the minimal JSON-RPC calls, receipt model, and `RpcError`; depends on block IDs and signed transaction wire data (`sdk/rs/src/rpc.rs:1-15`, `sdk/rs/src/rpc.rs:17-239`). |
| 17 | `execution.rs` — write pipeline | Exposes execution config/receipt/error, `Executor`, maturity wait, and proof-invocation builder; depends on calldata, RPC, prover, signing, and tx modules (`sdk/rs/src/execution.rs:26-35`, `sdk/rs/src/execution.rs:39-359`). |
| 18 | `state.rs` — local secret state | Exposes opaque handles, stored channel records, filesystem store, lease, and errors; depends on wire version and OS entropy/file locks (`sdk/rs/src/state.rs:12-20`, `sdk/rs/src/state.rs:26-497`). |
| 19 | `keys.rs` — key provisioning | Exposes non-overwriting pool-key creation and metadata/errors; depends on OS entropy and Stark public-key derivation (`sdk/rs/src/keys.rs:1-13`, `sdk/rs/src/keys.rs:15-109`). |
| 20 | `client.rs` — application facade | Exposes configuration, `Client`, the seven-method trait, API records, and aggregate `ClientError`; composes every lower layer (`sdk/rs/src/client.rs:19-31`, `sdk/rs/src/client.rs:37-82`, `sdk/rs/src/client.rs:538-573`, `sdk/rs/src/client.rs:938-1079`). |
| 21 | `bin/erebus_cli.rs` — process boundary | Defines the protocol-2 request enum, response envelope, dispatch/error mapping, and one-request Tokio main; depends on the high-level client and key generator (`sdk/rs/src/bin/erebus_cli.rs:10-24`, `sdk/rs/src/bin/erebus_cli.rs:27-174`, `sdk/rs/src/bin/erebus_cli.rs:202-306`, `sdk/rs/src/bin/erebus_cli.rs:429-450`). |

The conceptual dependency order is therefore: **Cairo-compatible primitives → valid action
sets and index allocation → Erebus wire/channel/read/state machine → ABI/transaction/prover/RPC
execution → persistence and high-level client → CLI/process adapters.** This follows the actual
composition imports and the fact that `Client` owns `Executor` and `StateStore`
(`sdk/rs/src/client.rs:19-31`, `sdk/rs/src/client.rs:63-82`).

## 2. Cryptographic derivations, salts, and negotiation wire

### 2.1 Exact Poseidon derivations

Every row below is `poseidon_hash_many` over the listed felt sequence
(`sdk/rs/src/hashes.rs:69-72`). Tags are ASCII short strings right-aligned as big-endian bytes
in a felt (`sdk/rs/src/hashes.rs:21-49`). “Observer” statements are explicit inferences from
the listed inputs, not claims made by a security proof.

| Derivation | Exact preimage and upstream Cairo | Derived from / explicitly not derived from | Who can compute; wrong-preimage symptom |
|---|---|---|---|
| Channel key | `H('CHANNEL_KEY_TAG:V1', sender_addr, sender_private_key, recipient_addr, recipient_public_key)` (`sdk/rs/src/hashes.rs:74-93`; `../starknet-privacy/packages/privacy/src/hashes.cairo:114-132`) | Includes the sender’s pool secret and both endpoint identities; **not** an ECDH result, token, pool address, chain ID, or channel index (`sdk/rs/src/hashes.rs:74-93`). | The sender can derive it; the recipient learns it from encrypted channel info (`sdk/rs/src/decrypt.rs:152-175`). I’m inferring that an observer without the sender secret cannot compute it—verify the cryptographic assumption against Poseidon preimage resistance. A wrong value makes channel markers/subchannels/note IDs disagree; preflight may reject `INVALID_CHANNEL`, or the recipient may silently search empty note IDs (`../starknet-privacy/packages/privacy/src/privacy.cairo:441-445`; `sdk/rs/tests/read_path.rs:245-275`). |
| Channel marker | `H('CHANNEL_MARKER_TAG:V1', channel_key, sender_addr, recipient_addr, recipient_public_key)` (`sdk/rs/src/hashes.rs:95-110`; `../starknet-privacy/packages/privacy/src/hashes.cairo:150-168`) | Not token-, pool-, chain-, or index-scoped. | Anyone with the channel key and public identities can compute it. A wrong marker is loud when `open_subchannel` reads `channel_exists` and raises `INVALID_CHANNEL` (`../starknet-privacy/packages/privacy/src/privacy.cairo:441-445`). |
| Subchannel ID | `H('SUBCHANNEL_ID_TAG:V1', channel_key, index, 0)`; the trailing zero is mandatory (`sdk/rs/src/hashes.rs:112-123`; `../starknet-privacy/packages/privacy/src/hashes.cairo:170-178`) | Not derived from token or recipient; token is encrypted in the record stored at this ID. | A channel-key holder can enumerate indices. A wrong ID makes `get_subchannel_info` look empty and discovery stop or skip the token (`sdk/rs/src/client.rs:428-442`, `sdk/rs/src/client.rs:471-486`). |
| Subchannel marker | `H('SUBCHANNEL_MARKER_TAG:V1', channel_key, recipient_addr, recipient_public_key, token)` (`sdk/rs/src/hashes.rs:125-140`; `../starknet-privacy/packages/privacy/src/hashes.cairo:180-198`) | Token- and recipient-bound; not index-, chain-, or pool-bound. | A channel-key holder with public metadata can compute it. A wrong marker makes note creation or spending fail `SUBCHANNEL_NOT_FOUND` (`../starknet-privacy/packages/privacy/src/privacy.cairo:595-604`, `../starknet-privacy/packages/privacy/src/privacy.cairo:730-734`). |
| Note ID | `H('NOTE_ID_TAG:V1', channel_key, token, index, 0)` (`sdk/rs/src/hashes.rs:142-151`; `../starknet-privacy/packages/privacy/src/hashes.cairo:200-210`) | Not amount-, salt-, sender-, recipient-, chain-, or pool-derived. | A channel-key holder can seek exact slots; an outsider cannot efficiently derive them without the key. A wrong read-side preimage is the canonical silent “not found” failure because `get_note` receives the wrong ID (`sdk/rs/src/client.rs:355-372`); a wrong write action can instead fail a subchannel/contiguity check during compile (`../starknet-privacy/packages/privacy/src/privacy.cairo:605-617`, `../starknet-privacy/packages/privacy/src/privacy.cairo:736-751`). |
| Nullifier | `H('NULLIFIER_TAG:V1', channel_key, token, index, 0, owner_private_key)` (`sdk/rs/src/hashes.rs:153-168`; `../starknet-privacy/packages/privacy/src/hashes.cairo:224-236`) | Adds spending authority to the note locator; not amount- or salt-derived. | Only a holder of both channel key and owner pool secret can compute it. This is why a viewing grant cannot spend (`sdk/rs/src/channel.rs:462-471`). A locally wrong nullifier falsely classifies spentness; actual `UseNote` compilation recomputes the Cairo nullifier from the owner secret, so the eventual symptom can be a double-spend `NON_ZERO_VALUE`, not necessarily silence (`sdk/rs/src/client.rs:501-517`; `../starknet-privacy/packages/privacy/src/privacy.cairo:616-628`, `../starknet-privacy/packages/privacy/src/privacy.cairo:932-946`). |
| Outgoing channel ID | `H('OUTGOING_CHANNEL_ID_TAG:V1', sender_addr, sender_private_key, index, 0)` (`sdk/rs/src/hashes.rs:170-184`; `../starknet-privacy/packages/privacy/src/hashes.cairo:134-148`) | Sender-secret and index scoped; not recipient- or channel-key-derived. | The sender can enumerate its own outgoing records; a public observer cannot without the secret. A wrong ID makes outgoing-channel counting stop early or recovery read the wrong slot (`sdk/rs/src/client.rs:292-310`; `sdk/rs/src/decrypt.rs:186-200`). |
| Encrypted-amount mask | `H('ENC_AMOUNT_TAG:V1', channel_key, token, index, 0, felt(salt_u128))` (`sdk/rs/src/hashes.rs:186-199`; `../starknet-privacy/packages/privacy/src/hashes.cairo:212-222`) | Includes a bounded note salt; not owner secret or amount. Only the low 128 hash bits mask the amount with wrapping arithmetic (`sdk/rs/src/decrypt.rs:115-137`). | A channel-key holder decrypts; public salt alone is insufficient. Wrong key/preimage returns plausible garbage without authentication (`sdk/rs/src/decrypt.rs:21-27`; `sdk/rs/tests/decrypt_conformance.rs:104-125`). |
| Encrypted-token mask | `H('ENC_TOKEN_TAG:V1', channel_key, index, 0, salt_felt)` (`sdk/rs/src/hashes.rs:201-212`; `../starknet-privacy/packages/privacy/src/hashes.cairo:77-82`) | Uses a full felt salt and no recipient/token in the mask. | A channel-key holder decrypts the stored token by field subtraction (`sdk/rs/src/decrypt.rs:178-184`). Wrong derivation produces another felt with no authentication and can make discovery miss the configured token (`sdk/rs/src/client.rs:428-442`). |
| Outgoing-recipient mask | `H('ENC_RECIPIENT_ADDR_TAG:V1', sender_addr, sender_private_key, index, 0, salt_felt)` (`sdk/rs/src/hashes.rs:214-229`; `../starknet-privacy/packages/privacy/src/hashes.cairo:99-112`) | Sender-secret scoped; not channel key or recipient-derived. | The sender can recover the recipient; a wrong mask silently produces another felt (`sdk/rs/src/decrypt.rs:186-200`). |
| ECDH channel-key mask | `H('ENC_CHANNEL_KEY_TAG:V1', shared_x)` (`sdk/rs/src/hashes.rs:231-234`; `../starknet-privacy/packages/privacy/src/hashes.cairo:85-90`) | Only the ECDH shared x-coordinate; no addresses or context. | Sender and recipient obtain the same x-coordinate through ephemeral-static ECDH (`../starknet-privacy/packages/privacy/src/utils.cairo:123-144`, `sdk/rs/src/decrypt.rs:152-175`). A non-curve ephemeral x is loud; a wrong private key returns a wrong channel key without error (`sdk/rs/src/decrypt.rs:48-57`; `sdk/rs/tests/decrypt_conformance.rs:160-204`). |
| ECDH sender-address mask | `H('ENC_SENDER_ADDR_TAG:V1', shared_x)` (`sdk/rs/src/hashes.rs:236-239`; `../starknet-privacy/packages/privacy/src/hashes.cairo:92-97`) | Same shared x only, but separate tag from channel-key mask. | Same ECDH boundary and unauthenticated failure as above (`sdk/rs/src/decrypt.rs:162-175`). |
| Auditor private-key mask | `H('ENC_PRIVATE_KEY_TAG:V1', shared_x)` (`sdk/rs/src/hashes.rs:241-244`; `../starknet-privacy/packages/privacy/src/hashes.cairo:63-68`) | Auditor ECDH shared x only. | Used by Cairo registration to escrow the whole pool private key to the configured auditor (`../starknet-privacy/packages/privacy/src/privacy.cairo:317-354`; `../starknet-privacy/packages/privacy/src/utils.cairo:201-227`). Erebus Rust defines the hash for conformance but does not expose auditor decryption (`sdk/rs/src/hashes.rs:241-244`). |
| Auditor user-address mask | `H('ENC_USER_ADDR_TAG:V1', shared_x)` (`sdk/rs/src/hashes.rs:246-249`; `../starknet-privacy/packages/privacy/src/hashes.cairo:70-74`) | Auditor ECDH shared x only. | Used upstream for encrypted withdrawal identity, not by the high-level Erebus flow (`../starknet-privacy/packages/privacy/src/privacy.cairo:505-523`). |
| Identity key | `H('IDENTITY_KEY_TAG:V1', user_addr, user_private_key, contract_address)` (`sdk/rs/src/hashes.rs:251-263`; `../starknet-privacy/packages/privacy/src/hashes.cairo:48-60`) | Pool-contract scoped; not channel/recipient/token-derived. | A pool-secret holder can compute it. The Rust crate pins it for conformance but its high-level client does not call it; verify any future use against the upstream call site rather than inferring one (`sdk/rs/src/hashes.rs:251-263`). |

The repository’s statement that “every failure mode … is silent” is too broad
(`CLAUDE.md:159-163`). Wrong locator hashes and unauthenticated additive masks are silent, but
wire-v2 context/key/tag mistakes raise `Authentication`, invalid ephemeral points raise
`InvalidEphemeralPubkey`, phase mistakes fail the builder, and several bad markers revert in
Cairo (`sdk/rs/src/wire.rs:455-498`, `sdk/rs/src/decrypt.rs:48-57`,
`sdk/rs/src/action_set.rs:121-178`). The useful precise claim is: **KATs are essential because
the highest-risk locator and additive-decryption errors can return absence or plausible data
instead of a type/crypto error.**

### 2.2 Salt lanes and the confidentiality invariant

There are three Rust types because “salt” names different protocol roles:

| Type | Valid range and permitted uses | Invariant it enforces |
|---|---|---|
| `FeltEntropy` | Any non-zero felt for `SetViewingKey`, `OpenChannel`, `OpenSubchannel`, and `CreateOpenNote` entropy/salt fields (`sdk/rs/src/actions.rs:91-120`). | Prevents accidentally feeding a 120-bit note salt into a full-felt Cairo field; constraint #5 and F2 document the upstream mismatch (`CLAUDE.md:28-30`, `docs/friction.md:258-276`). |
| `NoteSalt` | Strictly `1 < salt < 2^120`; `0` means absent and `1` is reserved for open notes (`sdk/rs/src/actions.rs:58-89`). | Makes contract range validity a constructor property. Structured wire chunks are `NoteSalt`s with bit 119 pinned (`sdk/rs/src/wire.rs:9-17`). |
| `RandomSalt` | Wraps a valid `NoteSalt` derived from caller-supplied CSPRNG bytes; it is accepted only by value-note constructors (`sdk/rs/src/actions.rs:122-162`, `sdk/rs/src/channel.rs:502-512`). | Makes it impossible to pass a structured wire salt to a value-bearing note without deliberately breaking the type boundary. |

The bug prevented by the split is mask reuse/predictability. The amount cipher is additive and
its mask depends on `(channel_key, token, index, salt)`; the code’s own comment warns that
using a structured/predictable salt on value notes can let an observer compare ciphertexts,
whereas structured salts are confined to zero-amount notes (`sdk/rs/src/actions.rs:122-132`,
`sdk/rs/src/channel.rs:414-421`, `sdk/rs/src/decrypt.rs:115-137`). `Channel::data_note` hardcodes
amount zero, while `Channel::value_note` requires `RandomSalt`
(`sdk/rs/src/channel.rs:490-512`). This is a type-level confidentiality boundary, not merely a
range check.

### 2.3 Negotiation wire: exact layout and what v2 hides

The canonical plaintext is exactly:

```text
MSB                                                                 LSB
type:8 | reply_to:32 | created_at:40 | amount:128 | deadline:64 | memo_hash:128
                              400 bits / 50 bytes
```

Fields are pushed most-significant-first; `None` uses `u32::MAX`, which is therefore forbidden
as a real `reply_to` (`sdk/rs/src/wire.rs:63-88`, `sdk/rs/src/wire.rs:327-370`). `created_at`
must fit 40 bits; amount and memo already occupy full `u128`; deadline occupies 64 bits
(`sdk/rs/src/wire.rs:320-345`). The Rust API accepts only a 128-bit memo hash, while its helper
for a felt intentionally keeps the low 128 bits (`sdk/rs/src/wire.rs:231-242`). This differs
from the repository TypeScript v1 helper, which silently masks any larger bigint
(`sdk/ts/src/channel/wire.ts:119-155`); Rust’s public CLI parses a `u128`, so oversized input is
rejected rather than silently truncated (`sdk/rs/src/bin/erebus_cli.rs:324-334`). That is the
F19 deliberate hardening (`docs/friction.md:706-732`).

Wire v1 places the 400 plaintext bits directly into four 119-bit payload chunks, pins bit 119
of every salt, and uses notes `4k..4k+3`; it remains readable but new writes return
`LegacyReadOnly` (`sdk/rs/src/wire.rs:501-534`, `sdk/rs/src/channel.rs:422-438`). Wire v2 first
encrypts the 50 plaintext bytes with AES-256-GCM-SIV and appends a 16-byte tag; an unencrypted
one-byte marker makes a 67-byte/536-bit envelope, placed least-significant chunk first across
five 119-bit chunks with 59 zero padding bits (`sdk/rs/src/wire.rs:29-35`,
`sdk/rs/src/wire.rs:78-95`, `sdk/rs/src/wire.rs:418-453`).

V2 derives a 32-byte key and 12-byte nonce with HKDF-SHA-256 from the directional channel key.
The HKDF salt is `EREBUS_WIRE_V2_HKDF_SHA256`; key info is
`EREBUS_WIRE_V2_KEY || chain_id || pool_address || token`; nonce info is
`EREBUS_WIRE_V2_NONCE || same_scope || message_index_be` (`sdk/rs/src/wire.rs:383-407`). AAD is
`EREBUS_WIRE_V2_AAD || chain_id || pool_address || token || message_index_be`
(`sdk/rs/src/wire.rs:409-415`). Thus content is encrypted and authenticated, and copying it
across any authenticated context fails (`sdk/rs/tests/wire_codec.rs:151-225`). AES-GCM-SIV was
chosen because a failed, not-yet-included write may retry different terms at the same free
index; ordinary nonce-sensitive AEAD would make that operational retry catastrophic
(`docs/friction.md:1086-1108`).

V2 does **not** hide that five note creations occurred, their transaction sender, their time,
or their five-note cadence. Worse, required-zero padding gives the fifth salt a 59-bit fixed
shape that the non-ignored fingerprint test detects (`sdk/rs/tests/wire_v2_fingerprint.rs:31-58`;
`docs/friction.md:990-1015`). It also does not hide the salt values themselves: salt is the
public high 120 bits of every stored packed note and appears in client action calldata
(`sdk/rs/src/decrypt.rs:103-112`, `sdk/rs/src/actions.rs:203-218`). So v2 provides content
confidentiality/authentication, not traffic-flow or sender-account privacy.

## 3. Port ledger: what was rewritten and why

| Upstream TS/Cairo function or primitive | Rust equivalent | Why rewritten rather than called; parity/divergence |
|---|---|---|
| Cairo `compute_*` functions in `hashes.cairo` | `hashes::{compute_channel_key, …}` (`sdk/rs/src/hashes.rs:74-263`) | Rust needs them in-process for construction and keyed discovery. Cairo is the KAT oracle; upstream `discovery-core` duplicates many but brings a conflicting git-fork dependency graph (`sdk/rs/src/decrypt.rs:6-19`). Translation, except Rust preserves exact heterogeneous salt types. |
| Cairo additive encryption/decryption formulas and upstream `discovery-core` decryption | `decrypt::{unpack_note,note_amount,packed_value,channel_info,subchannel_token,outgoing_recipient_addr}` (`sdk/rs/src/decrypt.rs:103-200`) | Needed in-process without importing the forked provider stack. Same Cairo fixture is the oracle (`sdk/rs/tests/decrypt_conformance.rs:1-11`). |
| Cairo `ClientAction` enum and TS `serializeClientActions` | Rust input structs, `ClientAction`, `serialize_actions` (`sdk/rs/src/actions.rs:164-434`) | Required for a Node-free Rust write path. The TS SDK supplies byte-for-byte Serde fixtures because Cairo emits no direct vector (`sdk/rs/tests/clientaction_serde.rs:1-11`). |
| Cairo phase/replay checks in `main`/`assert_and_advance_phase` | `ActionSetBuilder` (`sdk/rs/src/action_set.rs:121-178`) | Deliberate type/construction hardening: fail before proving rather than after. Token balance is intentionally still left to Cairo because the builder lacks consumed amounts (`sdk/rs/src/action_set.rs:24-28`). |
| Cairo contiguous/write-once note rules | `SubchannelCursor` (`sdk/rs/src/subchannel.rs:82-164`) | Erebus-specific allocator absent upstream; makes caller-side gap/reuse errors unrepresentable during a single process. It remains only a local belief and must be reseated from chain (`sdk/rs/src/subchannel.rs:27-32`). |
| Upstream `ProofInvocationFactory.create` and `compileExecuteCalldata` | `calldata::compile_actions`, `proof_execute`, `execution::build_proof_invocation` (`sdk/rs/src/calldata.rs:25-53`, `sdk/rs/src/execution.rs:268-299`) | Required in-process with no Node runtime and needed to start KAT composition from `ActionSet`. The end-to-end fixture is captured from upstream factory (`sdk/rs/tests/proof_invocation.rs:1-13`). |
| starknet.js invoke-v3 hash/signature | `InvokeV3::transaction_hash`, `signing` (`sdk/rs/src/tx.rs:156-193`, `sdk/rs/src/signing.rs:47-84`) | Rust signs locally and cannot call JS. It also must support the privacy-specific non-empty `proof_facts` hash term that a generic transaction model may omit (`sdk/rs/src/tx.rs:16-21`). Fixtures pin both libraries (`sdk/rs/tests/invoke_v3_txhash.rs:1-11`, `sdk/rs/tests/ecdsa.rs:1-9`). |
| Upstream `ProvingService.proveTransaction` | `ProvingService::prove_transaction` (`sdk/rs/src/prover.rs:142-220`) | Node-free async HTTP, typed response, and bounded retry policy. It preserves the upstream JSON-RPC method/shape, not protocol behavior invented by Erebus (`../starknet-privacy/sdk/src/internal/proving-service.ts:120-290`). |
| Upstream `PrivateTransfers.buildExecuteResult` screening suffix and output slicing | `calldata::screening_suffix`, `execution::server_actions` (`sdk/rs/src/calldata.rs:55-82`, `sdk/rs/src/execution.rs:323-343`) | Needed for direct Rust submission. It mirrors stripping the class-hash prefix and appending `Option<ScreeningAttestation>` (`../starknet-privacy/sdk/src/internal/private-transfers.ts:102-136`). |
| Starknet provider/account submission | Minimal `StarknetRpc` plus Rust `SignedInvokeV3` wire (`sdk/rs/src/rpc.rs:1-8`, `sdk/rs/src/rpc.rs:24-165`) | A full account SDK would not remove the custom proof-facts hash and would introduce a second transaction model (`sdk/rs/src/rpc.rs:1-5`). This is a narrow partial client, not a provider replacement. |
| Local TS wire-v1 pack/unpack | Rust `encode_legacy_message`/`decode_legacy_message` (`sdk/rs/src/wire.rs:501-534`) | Differential oracle for Erebus’s original format, retained read-only. Constants/salts/note indices match TS fixtures (`sdk/rs/tests/wire_codec.rs:1-10`, `sdk/rs/tests/wire_codec.rs:102-134`). |
| No upstream counterpart | Wire v2 (`sdk/rs/src/wire.rs:383-499`) | Erebus-specific authenticated encryption and migration behavior. It currently has only a Rust KAT/round-trip/tamper suite—not a second implementation (`sdk/rs/tests/wire_codec.rs:6-10`). |
| No upstream counterpart | `Channel`, `OfferBook`, `ViewingGrant`, `StateStore`, high-level `Client` | These define negotiation semantics, atomic composition, selective disclosure, persistence, and the application API (`sdk/rs/src/channel.rs:164-253`, `sdk/rs/src/negotiation.rs:147-302`, `sdk/rs/src/disclosure.rs:45-270`, `sdk/rs/src/state.rs:174-446`, `sdk/rs/src/client.rs:538-935`). Category: original Erebus protocol behavior. |
| Local TS static-static ECDH | No active Rust equivalent | It is planned off-chain transport crypto, while current v2 derives from the on-chain directional channel key (`sdk/ts/src/crypto/channel-secret.ts:1-29`, `sdk/rs/src/wire.rs:383-407`). This is an unported/unused path, not missing parity in the active protocol. |

### Known behavioral differences, not translations

1. Rust rejects oversized `memo_hash` at its typed/CLI boundary, while TypeScript v1 masks to
   the low 128 bits; this is deliberate hardening, but it means callers must truncate before
   crossing the Rust API (`sdk/rs/src/bin/erebus_cli.rs:324-334`,
   `sdk/ts/src/channel/wire.ts:119-155`, `docs/friction.md:706-732`).
2. Rust refuses new wire-v1 writes, while TypeScript still exposes its v1 encoder; this is a
   deliberate confidentiality migration (`sdk/rs/src/channel.rs:428-430`,
   `sdk/ts/src/channel/wire.ts:196-219`).
3. Rust adds `ActionSetBuilder`, `RandomSalt`, cursor, exact-amount check, token checks, and
   pool-invocation newtypes beyond TS/Cairo serialization. These are client hardenings, not
   alternate Cairo semantics (`sdk/rs/src/action_set.rs:1-28`, `sdk/rs/src/actions.rs:122-162`,
   `sdk/rs/src/channel.rs:545-555`, `sdk/rs/src/client.rs:1306-1336`, `sdk/rs/src/tx.rs:221-250`).
4. Rust tx hashing appends a hash of `proof_facts` only when non-empty. The fixture covers both
   branches; this is a privacy-stack extension that must agree with the deployed RPC/prover,
   not standard starknet.js behavior to assume universally (`sdk/rs/src/tx.rs:172-193`,
   `sdk/rs/tests/invoke_v3_txhash.rs:129-161`).
5. The Rust grant returns a self-contained bearer package, correcting the older TS interface’s
   `void` return and local-handle-dependent reveal shape (`sdk/rs/src/client.rs:565-572`,
   `sdk/ts/src/interface.ts:151-172`, `docs/friction.md:922-936`).

### Why `/sdk/ts` still exists

It is a private, non-shipping oracle package (`sdk/ts/package.json:1-5`, `README.md:84-90`). It
generates frozen wire-v1 salts and exercises upstream Mocknet behavior
(`sdk/ts/tests/gen-wire-vectors.test.ts:1-25`, `sdk/ts/tests/pool-flow.test.ts:1-12`). Rust then
compares the frozen `ts-wire-salts.json` and `ts-clientaction-serde.json` byte-for-byte
(`sdk/rs/tests/wire_codec.rs:1-10`, `sdk/rs/tests/clientaction_serde.rs:1-11`). Agreement proves
that the tested inputs have identical serialization/legacy-wire outputs; it does **not** prove
general semantic equivalence, live-network compatibility, or wire-v2 interoperability.

## 4. Communication end to end

### 4A. Process and transport path

```text
agent policy
    │ typed MCP tool arguments/results; no keys
    ▼
Python MCP server ── async adapter ── sdk/py Seam
    │ one JSON request on child stdin; key *paths*, URLs, handle, method data
    ▼
erebus-cli (one process/request, protocol 2)
    │ opens 0600 key/state files; Rust values and opaque handle stay below seam
    ▼
Rust Client → RPC preflight/read + proving JSON-RPC + signed Starknet submission
```

**Agent → MCP server.** The agent supplies flat tool arguments such as counterparty, opaque
channel handle, amount, token, deadline, memo hash, or viewing-grant fields; the tool layer
converts these to the `ErebusClient` interface and returns a JSON-serializable `{ok,result}` or
`{ok:false,error}` payload (`mcp-server/src/erebus_mcp/tools.py:89-168`,
`mcp-server/src/erebus_mcp/tools.py:170-273`). The production MCP server constructs one
identity-bound seam from environment configuration; the default backend is actually `mock`,
not chain, unless `EREBUS_BACKEND=seam` selects Rust (`mcp-server/src/server.py:42-76`). No key
value is passed to a tool; only configured file paths reach the seam
(`mcp-server/src/server.py:46-66`).

The repository’s reference `agents/` demo does **not** traverse MCP or Rust: it directly uses
`MockErebusClient` and says so (`agents/src/erebus_agents/agent.py:1-7`,
`agents/src/erebus_agents/agent.py:27-45`). That distinction matters when answering “what did
the agent demo validate?” It validates policy/mock behavior, not this transport chain.

**MCP server → `sdk/py`.** `SeamErebusClient` reshapes Python dataclasses into seam dictionaries
and offloads each blocking child process with `asyncio.to_thread`, keeping the MCP event loop
responsive (`mcp-server/src/erebus_mcp/seam_client.py:1-17`,
`mcp-server/src/erebus_mcp/seam_client.py:94-109`). Therefore the premise “async is confined to
Rust” is false: Python uses async for server concurrency, while Rust owns asynchronous protocol
I/O and receipt/prover waiting. Python performs no hashes, felt arithmetic, salt encoding, or
proof logic (`mcp-server/src/erebus_mcp/seam_client.py:1-12`).

**`sdk/py` → CLI.** The seam builds exactly one JSON object, runs `erebus-cli`, writes the JSON
to stdin, captures stdout, and requires one JSON response envelope
(`sdk/py/src/erebus/_seam.py:120-165`). Every configured call sends nine fields: RPC URL,
prover URL, pool/chain/account, two key-file paths, state directory, and token
(`sdk/py/src/erebus/_seam.py:59-92`, `sdk/py/src/erebus/_seam.py:167-173`). Private key values do
not enter Python; the CLI opens their paths (`sdk/py/src/erebus/_seam.py:10-18`).

Protocol 1 was the earlier seam documented in stale prose. Current protocol 2 adds one-shot
configuration on every call, opaque state handles, `balance`, key generation, and structured
responses; `version` returns `protocol: 2` (`sdk/rs/src/bin/erebus_cli.rs:27-82`,
`sdk/rs/src/bin/erebus_cli.rs:202-306`). The CLI’s Tokio main reads stdin to EOF, deserializes
one request, awaits one dispatch, prints one envelope, and exits nonzero on failure
(`sdk/rs/src/bin/erebus_cli.rs:429-450`).

**CLI → Rust state/key boundary.** `generate_pool_key` creates an absolute-path, non-overwriting
0600 file from OS entropy and returns only its path and public key (`sdk/rs/src/keys.rs:24-80`).
Channel handles are `ch_` plus 64 lowercase hex characters and are validated before becoming
path components (`sdk/rs/src/state.rs:26-66`). The state directory is mode 0700 and lock,
temporary, and record files are mode 0600 on Unix (`sdk/rs/src/state.rs:180-189`,
`sdk/rs/src/state.rs:217-245`, `sdk/rs/src/state.rs:380-413`, `sdk/rs/src/state.rs:499-528`).
The non-Unix mode helpers are no-ops, so the 0600/0700 statement is Unix-specific
(`sdk/rs/src/state.rs:499-528`, `sdk/rs/src/keys.rs:83-90`).

**Rust → RPC/prover/Starknet.** Read/discovery calls send only public entrypoint arguments and
secret-derived IDs, but `starknet_call(compile_actions)` sends the pool private key in calldata
to the RPC (`sdk/rs/src/rpc.rs:1-8`, `sdk/rs/src/calldata.rs:25-36`). The virtual proof
invocation also sends that key in clear at `calldata[5]` to the prover
(`sdk/rs/src/prover.rs:3-14`, `sdk/rs/tests/proof_invocation.rs:129-151`). The final network
transaction is signed by the Starknet account key and calls `apply_actions`; the pool private
key is not in that final call, but proof facts and the proof blob are
(`sdk/rs/src/execution.rs:192-231`). Thus both preflight RPC and prover are inside the pool-key
trust boundary; the public chain sees the final account, action-derived state changes/events,
and transaction timing.

### 4B. On-chain protocol path

```text
open_channel: [SetViewingKey? → OpenChannel → OpenSubchannel]
      │ simulate compile_actions → prove virtual pool __execute__ → apply_actions
      ▼
offer/counter: five CreateEncNote(amount=0, encrypted wire salts)
      │ same execution pipeline per message
      ▼
accept_and_settle:
  UseNote(input 1..n) → five CreateEncNote(amount=0, acceptance)
                      → one CreateEncNote(amount=payment, random salt)
      │ one action set / one proof / one apply_actions transaction
      ▼
grant_viewing_key: local bearer export, no transaction
reveal: keyed RPC reads + local decrypt/reconstruct, no transaction
```

**1. `open_channel`.** The client reads both pool registrations, returns an existing local
handle if the pair/token already exists, derives the directional channel key, and builds one
setup action set (`sdk/rs/src/client.rs:575-622`). Setup optionally contains `SetViewingKey`,
then `OpenChannel`, then `OpenSubchannel`; that is account→channel→subchannel phase order
(`sdk/rs/src/channel.rs:298-326`). Cairo registration publishes the pool public key and
encrypts the pool private key to the configured auditor; opening a channel writes encrypted
channel info, a channel marker, and an encrypted outgoing record; opening a subchannel writes
encrypted token info and its marker (`../starknet-privacy/packages/privacy/src/privacy.cairo:317-354`,
`../starknet-privacy/packages/privacy/src/privacy.cairo:357-428`,
`../starknet-privacy/packages/privacy/src/privacy.cairo:431-470`). After an accepted receipt,
the client creates the opaque local record; a crash after inclusion but before `state.create`
can therefore orphan the local handle (`sdk/rs/src/client.rs:623-644`, `sdk/rs/README.md:113-116`).

**2. Offer.** The client validates token/terms/state, holds the state lease, waits until the
last write is visible to the proving anchor, reconstructs the chain transcript, reseats a
cursor at the first empty outgoing note, and builds an `Offer` message
(`sdk/rs/src/client.rs:648-687`). `Channel::write_message` encrypts it into five salts and
creates five consecutive `CreateEncNote` actions with amount zero
(`sdk/rs/src/channel.rs:414-459`, `sdk/rs/src/channel.rs:490-500`). Only after receipt does it
advance/persist the cursor and block (`sdk/rs/src/client.rs:688-695`).

**3. Counter.** The client attaches the reverse directional channel, verifies that `reply_to`
names a counterparty offer/counter, writes a `Counter` whose `reply_to` is the opposite
direction’s note-grid message index, then executes/commits exactly as for an offer
(`sdk/rs/src/client.rs:698-764`). Direction is part of `OfferId` because note indices collide
across the two independent subchannels (`sdk/rs/src/negotiation.rs:95-139`,
`sdk/rs/src/negotiation.rs:163-187`). Deadlines and reply validity are client semantics; Cairo
has no offer/deadline/status concept (`sdk/rs/tests/negotiation_state.rs:1-6`).

**4. `accept_and_settle`.** The payer checks the counterparty offer is live, discovers all
unspent value notes at the same mature block, and selects an exact subset; there is no change
note (`sdk/rs/src/client.rs:789-831`, `sdk/rs/src/client.rs:1088-1117`). It constructs an
`Accept` copying amount/deadline/memo and calls `settle_next`
(`sdk/rs/src/client.rs:833-860`). The action set places every `UseNote` in phase 4, then the
five zero-value acceptance notes and one random-salted payment note in phase 5, sorted by note
index (`sdk/rs/src/channel.rs:577-610`). The record occupies `5k..5k+4`; payment is `5k+5`,
leaving the cursor off-grid and making the current subchannel one-deal-only
(`sdk/rs/src/channel.rs:613-653`). The client checks recorded and paid amounts match before
construction (`sdk/rs/src/channel.rs:545-555`).

**5. Every write’s execution pipeline.** `Executor::execute` selects an older proving block,
calls the pool view `compile_actions`, builds/signs the pool-as-account virtual invocation,
asks `starknet_proveTransaction`, extracts the unique pool L2→L1 message, and rejects it if
its serialized server actions differ from the preflight (`sdk/rs/src/execution.rs:143-182`). It
then checks proof age, appends screening data, wraps `apply_actions` in the operator account’s
single-call calldata, estimates the proof-carrying transaction, signs/submits it, and waits for
an accepted or reverted receipt (`sdk/rs/src/execution.rs:184-264`). The prover receives the
pool virtual invoke and returns proof, proof facts, L2→L1 messages, and optional screening
signature (`sdk/rs/src/prover.rs:92-140`, `sdk/rs/src/prover.rs:185-220`). The final
`apply_actions` calldata is serialized server actions followed by Cairo `Option` screening;
the proof blob/proof facts live on the v3 transaction envelope (`sdk/rs/src/calldata.rs:55-82`,
`sdk/rs/src/tx.rs:264-350`).

**Why this SDK never submits `__execute__` to the chain.** It only builds `proof_execute` for
the transaction sent to the proving service (`sdk/rs/src/calldata.rs:50-53`,
`sdk/rs/src/execution.rs:268-299`); chain submission always wraps `apply_actions`
(`sdk/rs/src/execution.rs:192-231`). The Cairo `__execute__` compiles actions and sends server
actions as an L2→L1 message but does not call the server-side storage application path
(`../starknet-privacy/packages/privacy/src/privacy.cairo:193-212`). A normal contract call would
fail `assert_valid_os_call` because caller must be zero and tx version v3; more importantly,
submitting the virtual account transaction would publish the pool secret and would not execute
the proof-validated `apply_actions` transition (`../starknet-privacy/packages/privacy/src/utils.cairo:561-576`,
`../starknet-privacy/packages/privacy/src/privacy.cairo:782-839`). Calling it “local simulation
only” is shorthand: it is executed by the prover’s virtual Starknet OS, not by this SDK as the
real state-changing transaction.

**Phase order.** Cairo maps deposit to phase 3, note use to 4, note creation to 5, withdrawal
to 6, and invoke to 7; it rejects decreasing phases and a second/post-invoke action
(`../starknet-privacy/packages/privacy/src/actions.cairo:275-315`). `ActionSetBuilder::push`
rejects phase regression and multiple invokes, while `build` rejects empty or
non-replay-protected sets (`sdk/rs/src/action_set.rs:121-178`). It does not model per-token
balance; Cairo remains the authority for that runtime invariant
(`sdk/rs/src/action_set.rs:24-28`).

**6. Grant/reveal.** Granting is local: after attaching the reverse channel, it exports both
directional keys plus chain, pool, wire version, token, participants, and a checksum; no
transaction is sent (`sdk/rs/src/client.rs:878-916`, `sdk/rs/src/disclosure.rs:45-88`). Reveal
validates config scope, derives readers from the grant, fetches only computed note IDs, and
reconstructs locally (`sdk/rs/src/client.rs:918-935`, `sdk/rs/src/disclosure.rs:234-270`).

### How notes are found and why indices cannot have gaps

Discovery is deterministic nested enumeration: recipient channel count → decrypt each channel
info → derive sequential subchannel IDs until the first empty → decrypt token → derive
sequential note IDs until the first empty → decrypt amount → derive nullifier and query
spentness (`sdk/rs/src/client.rs:445-521`). For known negotiation channels, `fetch_notes` starts
from the channel key and stops at the first zero `get_note` result
(`sdk/rs/src/client.rs:355-372`). Events cannot substitute for this because note addresses are
secret-derived and the intended interface is exact keyed lookup; the repository explicitly
forbids world scanning (`CLAUDE.md:23-26`).

Cairo enforces that note `n-1` exists before creating note `n`, and `WriteOnce` prevents reuse
(`../starknet-privacy/packages/privacy/src/privacy.cairo:736-751`,
`../starknet-privacy/packages/privacy/src/privacy.cairo:932-946`). Therefore the first empty
slot is both the end of discovery and the only next legal write. `next_free_note_index` and
`SubchannelCursor` encode exactly that rule (`sdk/rs/src/client.rs:269-290`,
`sdk/rs/src/subchannel.rs:97-162`). A gap is not just inefficient: readers stop there and
everything beyond it becomes unreachable through this discovery algorithm.

## 5. How correctness is established

### Known-answer fixtures

| Fixture | Oracle and what agreement proves |
|---|---|
| `cairo-reference-data.json` | Inputs/outputs emitted from upstream Cairo derivations; pins every Poseidon preimage and read-side encrypted value (`sdk/rs/tests/fixtures/cairo-reference-data.json:2-26`, `sdk/rs/tests/cairo_conformance.rs:1-13`, `sdk/rs/tests/decrypt_conformance.rs:1-11`). |
| `ts-wire-salts.json` | Generated by the independent local TypeScript v1 codec; pins constants, note indices, and exact four salts for representative messages (`sdk/rs/tests/fixtures/ts-wire-salts.json:1-42`, `sdk/rs/tests/wire_codec.rs:1-10`). It proves v1 compatibility only. |
| `ts-clientaction-serde.json` | Generated through upstream `serializeClientActions` plus Starknet `CallData.compile`; pins all ten variant indices/field orders/felts (`sdk/rs/tests/fixtures/ts-clientaction-serde.json:1-70`, `sdk/rs/tests/clientaction_serde.rs:1-11`). |
| `starknetjs-ecdsa.json` | starknet.js keys/messages/signatures; pins public keys, cross-verification, and deterministic byte equality (`sdk/rs/tests/fixtures/starknetjs-ecdsa.json:1-29`, `sdk/rs/tests/ecdsa.rs:1-9`). |
| `starknetjs-invoke-v3-txhash.json` | `hash.calculateInvokeTransactionHash` vectors with and without proof facts and nontrivial bounds; pins transaction-hash composition (`sdk/rs/tests/fixtures/starknetjs-invoke-v3-txhash.json:1-44`, `sdk/rs/tests/invoke_v3_txhash.rs:1-11`). |
| `sdk-proof-invocation.json` | Captured upstream `ProofInvocationFactory` result; pins composition from `ActionSet` through `__execute__` calldata, v3 hash, signature, and wire transaction (`sdk/rs/tests/fixtures/sdk-proof-invocation.json:1-38`, `sdk/rs/tests/proof_invocation.rs:1-13`). |

### Integration-test map

| Test file | What it would catch |
|---|---|
| `action_set.rs` | Phase regression, multiple invoke, missing replay protection, wrong span shape, or nonzero proof-invocation prices/tip (`sdk/rs/tests/action_set.rs:1-15`, `sdk/rs/tests/action_set.rs:81-259`). |
| `cairo_conformance.rs` | Any tag/preimage/order/salt-type divergence from Cairo (`sdk/rs/tests/cairo_conformance.rs:1-13`, `sdk/rs/tests/cairo_conformance.rs:69-226`). |
| `decrypt_conformance.rs` | Incorrect pack halves, wrapping subtraction, ECDH recovery, or unauthenticated-wrong-key assumptions (`sdk/rs/tests/decrypt_conformance.rs:1-11`, `sdk/rs/tests/decrypt_conformance.rs:62-234`). |
| `clientaction_serde.rs` | Wrong Cairo enum index, field order, span prefix, phase map, or salt bounds (`sdk/rs/tests/clientaction_serde.rs:1-11`, `sdk/rs/tests/clientaction_serde.rs:104-207`). |
| `channel_ops.rs` | Correct primitives wired to wrong key, party, token, index, salt, or zero amount (`sdk/rs/tests/channel_ops.rs:1-6`, `sdk/rs/tests/channel_ops.rs:55-235`). |
| `setup.rs` | Incorrect register/channel/subchannel composition, shield balance/replay structure, or top-up reopening (`sdk/rs/tests/setup.rs:1-6`, `sdk/rs/tests/setup.rs:59-310`). |
| `settlement.rs` | Acceptance/payment split, create-before-spend, missing inputs, amount mismatch, salt-lane mix-up, or index collision (`sdk/rs/tests/settlement.rs:1-6`, `sdk/rs/tests/settlement.rs:90-354`). |
| `index_contiguity.rs` | Gaps, overwrite, failed-reservation cursor burns, off-grid messages, or illegal post-settlement message (`sdk/rs/tests/index_contiguity.rs:1-10`, `sdk/rs/tests/index_contiguity.rs:85-261`). |
| `wire_codec.rs` | V1 compatibility drift and v2 round-trip, KAT, tamper, context, padding, retry, flag, range, or redaction failure (`sdk/rs/tests/wire_codec.rs:1-10`, `sdk/rs/tests/wire_codec.rs:102-367`). |
| `wire_v2_fingerprint.rs` | Detects today’s fifth-salt traffic fingerprint; the desired indistinguishability property remains ignored (`sdk/rs/tests/wire_v2_fingerprint.rs:1-5`, `sdk/rs/tests/wire_v2_fingerprint.rs:31-75`). |
| `read_path.rs` | Writer/reader slot drift, torn messages, v1 migration loss, wrong-key behavior, settlement-note placement, or direction/author reconstruction errors (`sdk/rs/tests/read_path.rs:1-7`, `sdk/rs/tests/read_path.rs:122-407`). |
| `negotiation_state.rs` | Expiry boundary, own/unknown/non-offer acceptance, dangling replies, countered-offer semantics, and repeat settlement (`sdk/rs/tests/negotiation_state.rs:1-6`, `sdk/rs/tests/negotiation_state.rs:61-191`). |
| `disclosure.rs` | Incomplete reconstruction, wrong attribution/payment comparison, cross-token/counterparty leakage, half grant, serialization corruption, or spending-key leakage (`sdk/rs/tests/disclosure.rs:1-9`, `sdk/rs/tests/disclosure.rs:181-495`). |
| `invoke_v3_txhash.rs` | Divergence from starknet.js, especially conditional proof-facts and resource-bound packing (`sdk/rs/tests/invoke_v3_txhash.rs:1-11`, `sdk/rs/tests/invoke_v3_txhash.rs:114-175`). |
| `ecdsa.rs` | Public-key/signature incompatibility or nondeterminism (`sdk/rs/tests/ecdsa.rs:1-9`, `sdk/rs/tests/ecdsa.rs:38-125`). |
| `proof_invocation.rs` | Locally correct components composed into an upstream-incompatible proof request, and accidental denial of the clear-key exposure (`sdk/rs/tests/proof_invocation.rs:1-13`, `sdk/rs/tests/proof_invocation.rs:97-151`). |
| `execution_pipeline.rs` | One in-process preflight→prove→compare→estimate→sign→submit→receipt transport path; it explicitly uses deterministic local JSON-RPC servers, not Sepolia (`sdk/rs/tests/execution_pipeline.rs:1-5`, `sdk/rs/tests/execution_pipeline.rs:76-204`). |
| `cli_seam.rs` | Broken one-envelope contract, protocol version, structured failures, handle validation, key-value smuggling, key overwrite, or path-only boundary (`sdk/rs/tests/cli_seam.rs:1-5`, `sdk/rs/tests/cli_seam.rs:50-231`). |
| `prover_live.rs` | Manually probes shared Sepolia prover reachability/error shape; both tests are intentionally ignored from normal CI (`sdk/rs/tests/prover_live.rs:1-16`, `sdk/rs/tests/prover_live.rs:31-66`). |

**Fresh execution evidence, 2026-08-05:** `cargo test --all-targets` in `sdk/rs` completed
with 194 passed and 3 ignored; `pnpm vitest run` in `sdk/ts` completed with 38 passed. The
ignored Rust cases are the two live shared-prover probes and the not-yet-achieved uniform-salt
fingerprint target, as declared in their source (`sdk/rs/tests/prover_live.rs:1-16`,
`sdk/rs/tests/wire_v2_fingerprint.rs:60-75`).

### The u128 domain-tag bug

The first implementation accumulated Cairo short-string bytes into `u128`; tags longer than
16 bytes silently lost high bytes. Short tags passed while channel/subchannel/outgoing tags
failed, producing a deceptive partial success (`docs/friction.md:406-424`). The Cairo KAT
failed immediately, before a network run; the fix right-aligns up to 31 bytes in a 32-byte
buffer and calls `Felt::from_bytes_be` (`docs/friction.md:426-434`,
`sdk/rs/src/hashes.rs:25-40`). Without the KAT, read/write parties using different tags would
derive different secret slots and report absence, which is exactly why “it compiles” has almost
no evidentiary value for these formulas.

### What is not covered

Wire v2 has not been exercised in a fresh live offer/counter/settlement/reveal, implemented by
a second language, independently reviewed, or fee-measured (`README.md:14-24`,
`docs/friction.md:1115-1122`). The local execution-pipeline test does not claim Sepolia
compatibility (`sdk/rs/tests/execution_pipeline.rs:1-5`), and normal CI does not reach the real
prover (`sdk/rs/tests/prover_live.rs:1-16`). The repository also has no test proving grantee
cryptographic authorization, because `grantee` is metadata and the grant is bearer
(`docs/friction.md:928-936`).

I could not find a test for a crash after chain inclusion but before `lease.commit`, a fully
successful screening-attested deposit against live infrastructure, non-Unix permission
enforcement, large/reorging discovery, or a malicious grant holder recomputing the unkeyed
checksum after editing participant metadata. I’m inferring these gaps from the relevant code
paths—verify by adding fault-injection/live tests around `Client` receipt→commit boundaries,
screening responses, and `grant_checksum_v2` (`sdk/rs/src/client.rs:688-695`,
`sdk/rs/src/disclosure.rs:290-307`).

## 6. Rust-specific engineering decisions

### Unsafe, panics, and FFI boundaries

Both library and CLI use `#![forbid(unsafe_code)]` (`sdk/rs/src/lib.rs:22`,
`sdk/rs/src/bin/erebus_cli.rs:8`). That makes accidental in-crate unsafe memory/FFI escape
impossible, but dependencies can still contain unsafe internally; the attribute is not a
whole-supply-chain proof. There is no PyO3/FFI boundary: Python starts an ordinary process and
exchanges JSON (`sdk/py/src/erebus/_seam.py:1-18`, `sdk/py/src/erebus/_seam.py:120-165`). A
subprocess was chosen specifically so Python never owns the key value.

The convention is no `unwrap`/`expect` outside tests and construction-proven constants
(`CLAUDE.md:149-152`). Production `expect`s are confined to invariants such as fixed-width
HKDF output, AES key size, checked vector-to-array width, writing into a `String`, and an
internally-created transparent ID (`sdk/rs/src/wire.rs:383-426`, `sdk/rs/src/read.rs:235-245`,
`sdk/rs/src/state.rs:52-57`, `sdk/rs/src/bin/erebus_cli.rs:312-315`). If those assumptions drift,
the one-shot CLI panics and the seam receives non-JSON stdout or a failed child rather than a
structured protocol error (`sdk/py/src/erebus/_seam.py:146-165`); that is why invariant
locality matters here.

### Newtypes as protocol invariants

- `NoteSalt`, `FeltEntropy`, and `RandomSalt` separate bounded note storage, full-felt entropy,
  and unpredictable value-note nonces (`sdk/rs/src/actions.rs:58-162`). Runtime checks at every
  call site would be easy to omit; distinct parameter types make the wrong lane fail to compile.
- `ActionSet` cannot be constructed except through a builder that checks ordering, invokes,
  nonempty, and replay protection (`sdk/rs/src/action_set.rs:97-178`). A raw `Vec` would defer
  errors until a paid proof/revert.
- `SubchannelCursor` allocates exact contiguous ranges and refuses off-grid starts
  (`sdk/rs/src/subchannel.rs:82-162`). A bare `u32` would require every write path to remember
  gap, reuse, and five-note framing rules.
- `PoolInvocation` can exist only after zero-tip/zero-resource-price checks
  (`sdk/rs/src/tx.rs:205-250`). Without it, every proof call could produce a valid-looking v3
  object that Cairo `__validate__` rejects.
- `ChannelHandle` validates a narrow grammar before filesystem use
  (`sdk/rs/src/state.rs:26-66`). This turns path traversal/malformed handle rejection into the
  boundary constructor rather than repeated sanitization.

The structured-salt-on-zero-amount invariant is not wholly encoded in a single public type:
`CreateEncNoteInput` still publicly accepts a `NoteSalt` with any amount
(`sdk/rs/src/actions.rs:203-218`). It becomes unrepresentable only through the private
`Channel::data_note`/`value_note` constructors (`sdk/rs/src/channel.rs:490-512`). A caller using
the public low-level action structs can bypass that policy. This is a real limit of the
newtype claim.

### Async, ownership, and lifetimes in this code

Network waits live in Rust’s `reqwest`/Tokio clients and the high-level async trait: RPC,
proving, maturity, fee, submission, and receipt polling all borrow `&self` across `.await`
(`sdk/rs/src/prover.rs:177-220`, `sdk/rs/src/rpc.rs:33-165`,
`sdk/rs/src/execution.rs:105-264`, `sdk/rs/src/client.rs:538-573`). `Client` owns cloned,
internally reference-counted HTTP clients through `Executor`; requests can borrow configuration
and action data without moving the client (`sdk/rs/src/client.rs:63-82`,
`sdk/rs/src/execution.rs:84-103`). The public trait uses native `async fn` and explicitly
allows the `async_fn_in_trait` lint, which means it is intended for concrete/static use rather
than promising a boxed object-safe future surface (`sdk/rs/src/client.rs:538-573`).

The important ownership choice is `ChannelLease`: it owns the lock file and mutable state, so
the OS lock remains held while client methods await maturity, reads, proving, submission, and
receipt (`sdk/rs/src/state.rs:230-280`, `sdk/rs/src/state.rs:425-446`;
`sdk/rs/src/client.rs:648-695`). No borrowed `&mut StoredChannel` escapes its owning lease, and
commit consumes the lease, so code cannot accidentally commit twice. The cost is head-of-line
blocking: one slow proof serializes every operation on that handle. I’m inferring that this is
intentional retry/cursor safety from the “keep lease alive through any async operation” doc
comment—verify with maintainers if concurrent read-only access is desired
(`sdk/rs/src/state.rs:230-232`).

`NoteSource` is synchronous and generic over `&impl NoteSource`, allowing pure read/decode code
to borrow a map/closure without async lifetimes (`sdk/rs/src/read.rs:72-88`,
`sdk/rs/src/read.rs:163-180`). `Client` first performs async RPC into an owned `HashMap`, then
passes a short-lived borrowing closure to reconstruction (`sdk/rs/src/client.rs:923-934`). That
split avoids an async trait object and lets crypto/state-machine tests run with ordinary maps.

Python async is orchestration rather than protocol ownership: `asyncio.to_thread` prevents the
blocking subprocess from freezing MCP’s event loop (`mcp-server/src/erebus_mcp/seam_client.py:8-17`,
`mcp-server/src/erebus_mcp/seam_client.py:105-109`). Thus “async confined to Rust” should be
rephrased as “protocol I/O, cryptography, state mutation, and transaction lifecycle are
implemented once in Rust; Python only schedules a blocking process adapter.”

### Error taxonomy

`ChannelError` represents pure action-composition violations—wire, phase/index, zero payment,
missing inputs, amount disagreement, or record/payment collision—before transport
(`sdk/rs/src/channel.rs:48-96`). `ReadError` represents corrupted grants, partial/foreign data
notes, wire authentication/decoding, or invalid reconstructed negotiation
(`sdk/rs/src/read.rs:38-70`). `ClientError` is the application boundary: it adds request,
identity, token, state, discovery, RPC/prover/execution, and transparently wraps the narrower
errors (`sdk/rs/src/client.rs:1406-1521`). Keeping them separate lets pure channel/read code
remain independent of filesystem/network policy while the CLI maps only the aggregate error
into stable agent-facing codes (`sdk/rs/src/bin/erebus_cli.rs:336-427`).

### State lease/commit and crashes

State writes serialize to a new 0600 temporary file, flush and `sync_all`, then atomically
rename over the record (`sdk/rs/src/state.rs:380-414`). `lock` holds an exclusive sibling lock
file; `commit(self)` writes and then drops the lock (`sdk/rs/src/state.rs:230-280`,
`sdk/rs/src/state.rs:425-446`). If a process dies before an on-chain write, the original record
survives. If it dies after chain inclusion but before commit, the record is structurally intact
but logically stale; subsequent `sync_book` can recover note cursor/acceptance from keyed chain
reads in many operations (`sdk/rs/src/client.rs:312-353`, `sdk/rs/src/client.rs:766-785`). It is
not a transactional two-phase commit with Starknet, and opening can be orphaned because state
creation occurs after receipt (`sdk/rs/src/client.rs:623-644`, `sdk/rs/README.md:113-116`).

## 7. Where the upstream stack fought us

This section separates upstream feedback from Erebus’s own design mistakes. `docs/friction.md`
contains two entries numbered F31—traffic fingerprinting and AEAD choice—which is an editorial
collision, not one issue (`docs/friction.md:990-1020`, `docs/friction.md:1086-1122`).

### Genuine upstream bugs or capability gaps

- A private note has no application payload and the Wallet API does not expose controlled
  note/salt construction. That forced the 119-bit salt lane and a lower-level client instead
  of a supported metadata extension (`docs/friction.md:11-254`, `docs/friction.md:612-666`). A
  payload/commitment field or safe opaque metadata hook would remove the workaround.
- Salt types are genuinely inconsistent: amount encryption takes bounded `u128`; token and
  outgoing-recipient masks take full felts (`docs/friction.md:258-276`;
  `../starknet-privacy/packages/privacy/src/hashes.cairo:77-112`,
  `../starknet-privacy/packages/privacy/src/hashes.cairo:212-222`). Named newtypes in the
  upstream ABI/spec would make the distinction visible.
- Upstream’s encrypted-note view returns token zero by design/storage layout, while a naïve
  client expects the requested token echoed; the live path initially treated this as a bug
  (`docs/friction.md:1126-1184`, `sdk/rs/src/client.rs:1306-1336`). A typed `EncryptedNote` view
  with “token implied by subchannel” documentation would prevent the misread.
- `proof_facts` extends v3 transaction hashing but is absent from ordinary account models, and
  prover failures can collapse to bare `-32603`/contract errors (`docs/friction.md:577-608`,
  `docs/friction.md:736-774`). A versioned public transaction schema and structured prover
  error data would eliminate local transaction-model code and blind debugging.
- One channel key has no channel index, so a sender/recipient pair has one WriteOnce channel
  forever; reopening can spend a proof before failing (`docs/friction.md:940-986`,
  `sdk/rs/src/hashes.rs:74-93`). An indexed/session channel derivation or explicit upstream
  idempotent lookup would support repeated relationships.

### Documentation failures

The work repeatedly required source archaeology for installability of the workspace package,
builder-required fields, automatic action insertion, mock semantics, Serde enum/span layout,
key exposure, sequential index scope, and the distinction between pool identity and Starknet
signing keys (`docs/friction.md:316-404`, `docs/friction.md:436-573`,
`docs/friction.md:778-861`, `docs/friction.md:1225-1252`). A language-neutral protocol
document containing exact preimages, storage/discovery loops, action phases, calldata, proof
transaction extensions, key roles, and failure codes would have turned most of the Rust work
from reverse engineering into implementation.

The most serious missing warning was custody: both `compile_actions` RPC and prover payloads
receive the pool private key (`docs/friction.md:473-538`). That should be stated beside endpoint
configuration, not only inferable from calldata. The Rust modules now put the warning at the
URL boundary (`sdk/rs/src/rpc.rs:1-8`, `sdk/rs/src/prover.rs:3-14`).

### Defensible but surprising design decisions

The pool is intentionally keyed-discovery rather than event scanning, action compilation is a
virtual-account execution followed by proof-bound `apply_actions`, and client actions must
contain WriteOnce replay protection (`CLAUDE.md:19-30`, `sdk/rs/src/action_set.rs:1-28`). These
choices are internally coherent, but each violates a normal Starknet client intuition. A
single official sequence diagram plus executable reference vectors would make the design
legible without weakening it.

Auditor registration escrows the entire pool private key, not a relationship-scoped read key
(`../starknet-privacy/packages/privacy/src/privacy.cairo:317-354`). This is a defensible
compliance model but materially stronger disclosure than Erebus’s bearer grant. The API name
`SetViewingKey` does not communicate “encrypt my spending/decryption root key to the pool
auditor”; explicit custody language would.

### Operational blockers

The shared prover is slow enough that errors cost a full proving round, has private/community
operational knowledge, and sometimes returns opaque failures; proof state also lags head and
expires after a validity window (`docs/friction.md:670-774`, `docs/friction.md:1407-1427`,
`sdk/rs/src/execution.rs:105-130`, `sdk/rs/src/execution.rs:184-190`). Public dev endpoints,
health/version compatibility, queue status, structured errors, and a local deterministic prover
would dramatically shorten third-party iteration.

Deposits additionally require an external screening attestation, with freshness/signature
rules enforced by the pool (`../starknet-privacy/packages/privacy/src/privacy.cairo:907-929`;
`docs/friction.md:1346-1403`). The screening signer, prover/interceptor, pool key, RPC, token
approval, gas, maturity, and proof validity form one operational chain; a maintained testnet
runbook and disposable funded fixture identity would make end-to-end validation reproducible.

Gas and latency multiply with the protocol’s fixed shapes: every offer/counter is five note
creations and settlement adds spends plus six creations (`sdk/rs/src/channel.rs:414-438`,
`sdk/rs/src/channel.rs:577-610`). Current gas evidence and proof timing are snapshot-specific,
not production guarantees (`docs/friction.md:1188-1221`, `docs/friction.md:1407-1427`).

## 8. Security properties and limits

### What is actually guaranteed by the implemented path

- The final `apply_actions` contract validates recent proof facts and binds their message hash
  to the exact server actions before applying them atomically
  (`../starknet-privacy/packages/privacy/src/privacy.cairo:782-839`).
- The Rust settlement constructor puts note spends, acceptance-data notes, and the payment note
  in one `ActionSet`; one proof and one transaction therefore apply all or none
  (`sdk/rs/src/channel.rs:515-610`, `sdk/rs/src/execution.rs:132-239`). It also locally rejects
  disagreement between acceptance and payment amount (`sdk/rs/src/channel.rs:545-555`).
- Wire v2 encrypts and authenticates canonical terms under a channel/context-derived key
  (`sdk/rs/src/wire.rs:383-499`). The channel key permits note location/decryption but the
  owner pool private key is additionally required for nullifiers/spending
  (`sdk/rs/src/hashes.rs:142-168`).
- A grant contains two channel keys for one token/chain/pool relationship and no pool private
  key, allowing full two-direction reconstruction without unrelated channel keys
  (`sdk/rs/src/disclosure.rs:45-88`, `sdk/rs/src/disclosure.rs:149-171`).

### What those guarantees do not mean

Atomicity is scheduling, not semantic truth. A hostile client can create zero-value notes whose
salts decode as an acceptance and a separate value note with another amount; the pool sees
notes, not offers (`docs/friction.md:865-896`). Erebus rejects this on write and compares again
on disclosure, but settlement consistency is **not** a Cairo/ZK-enforced negotiation rule
(`sdk/rs/src/channel.rs:545-555`, `sdk/rs/src/disclosure.rs:309-337`). Closing that gap requires
the proof program/contract to understand and bind the acceptance schema to the payment amount,
or a separate verifiable receipt circuit.

The viewing grant is a bearer secret. `grantee` is metadata, not encryption or authorization;
any holder can read the scoped relationship (`docs/friction.md:922-936`). Its Poseidon checksum
covers scope and keys, but it is unkeyed and recomputable by a holder
(`sdk/rs/src/disclosure.rs:290-307`). I’m inferring that it detects accidental corruption but
does not authenticate the grantor or prevent deliberate metadata edits—verify with a test that
edits fields and recomputes the checksum. Cryptographic grantee binding needs encryption to
the grantee public key or a signed/attested capability.

The grant differs from STRK20 `SetViewingKey` in authority and scope. `SetViewingKey` encrypts
the identity’s **pool private key** to the configured auditor, which can consequently derive
all channels/nullifiers protected by that identity; the Erebus grant shares only two channel
keys and cannot derive owner-secret nullifiers (`../starknet-privacy/packages/privacy/src/privacy.cairo:317-354`,
`sdk/rs/src/channel.rs:462-471`). The former is pool-wide auditor escrow; the latter is
application-level relationship disclosure.

An on-chain observer still learns the submitting Starknet account, transaction timing, action
and calldata sizes, five-note message cadence, and the current fifth-salt fingerprint; v2 only
hides/authenticates the 400-bit content (`docs/friction.md:990-1015`,
`sdk/rs/tests/wire_v2_fingerprint.rs:31-58`). Counterparty linking may still be inferred from
timing and participating accounts. Removing zero padding/marker fingerprint is necessary but
not sufficient; traffic padding, relaying/account unlinkability, and timing analysis defenses
would be needed for relationship privacy.

### What a disclosed record proves versus asserts

Chain proof facts prove that the pool’s virtual execution produced the exact server actions
accepted by `apply_actions` (`../starknet-privacy/packages/privacy/src/privacy.cairo:804-839`).
The grant holder can decrypt on-chain notes into messages and a payment amount, and can check
the latter equals the acceptance’s claimed amount (`sdk/rs/src/disclosure.rs:234-270`,
`sdk/rs/src/disclosure.rs:309-337`).

The meanings “offer,” “counter,” “deadline,” “participants,” and “accepted offer” are local
interpretations of encrypted salt bytes and grant metadata; the pool circuit does not assert
them (`sdk/rs/src/wire.rs:19-35`, `sdk/rs/src/negotiation.rs:163-272`). No grantor signature or
ZK receipt binds the disclosed participant metadata and negotiation policy to the settlement.
So a “verified bound outcome covering membership, disclosure policy, and settlement
consistency” is **not yet** exposed. It would require a signed or ZK-verifiable record that
commits to participant identities, canonical terms/policy, the exact proven settlement actions,
and the disclosure authorization.

## 9. Hostile Q&A

1. **Why did you not just use our TypeScript SDK?** The active application is Python above a
   Rust key boundary, and running Node would add another key-holding runtime. The upstream Rust
   crate covered reads but not action construction/proving/submission, while this crate needed
   a Node-free write path (`sdk/rs/src/lib.rs:3-20`, `sdk/py/src/erebus/_seam.py:10-18`).

2. **Is this a replacement for our SDK?** No. It implements the narrow negotiation/payment
   path and omits the general transfer, discovery-service, history, OHTTP, DeFi, and paymaster
   surface (`sdk/rs/src/client.rs:538-573`, `../starknet-privacy/sdk/src/index.ts:1-52`).

3. **Why duplicate `discovery-core` hashes/decryption?** Upstream already has those formulas,
   but importing it would force a git-pinned `starknet-rust` fork/provider graph rejected by
   this write-side crate (`sdk/rs/src/decrypt.rs:6-19`). Cairo KATs remain the common oracle.

4. **How do you know the hashes match Cairo?** Every derivation is compared against
   Cairo-emitted known answers, including the heterogeneous salt paths
   (`sdk/rs/tests/cairo_conformance.rs:1-13`, `sdk/rs/tests/cairo_conformance.rs:69-226`). The
   u128 tag bug demonstrates that these tests catch real, compiling divergence.

5. **How do you know action calldata matches our SDK?** All ten variants are compared
   byte-for-byte with an upstream TypeScript `serializeClientActions`/`CallData.compile` fixture
   (`sdk/rs/tests/clientaction_serde.rs:1-11`, `sdk/rs/tests/clientaction_serde.rs:104-153`).

6. **How do you know the composed proof invocation matches?** A captured upstream
   `ProofInvocationFactory` vector pins calldata, v3 hash, signature, and wire envelope together
   (`sdk/rs/tests/proof_invocation.rs:1-13`, `sdk/rs/tests/proof_invocation.rs:97-151`).

7. **What is actually live versus mocked? [WEAKNESS]** The README reports a complete wire-v1
   Sepolia flow, while the reference agent demo and current default MCP backend are mock
   (`README.md:14-24`, `agents/src/erebus_agents/agent.py:1-7`, `mcp-server/src/server.py:42-74`).
   Wire v2 still lacks a fresh live run and independent implementation/review.

8. **Does wire v2 hide the relationship? [WEAKNESS]** No. It hides terms but the fifth salt
   fingerprints each five-note message, and account/timing/cadence remain public
   (`docs/friction.md:990-1015`, `sdk/rs/tests/wire_v2_fingerprint.rs:31-75`).

9. **Why five zero-value notes for one message?** The pool has no payload field; each valid
   encrypted note exposes 119 usable salt bits, while authenticated v2 needs 536 bits
   (`sdk/rs/src/wire.rs:7-17`, `sdk/rs/src/wire.rs:29-35`). Five notes provide 595 bits.

10. **Can a structured salt leak money?** The high-level constructors permit structured salts
    only on zero-value data notes and require `RandomSalt` for value notes
    (`sdk/rs/src/channel.rs:490-512`). **[WEAKNESS]** Low-level public action structs can bypass
    that policy (`sdk/rs/src/actions.rs:203-218`).

11. **What does atomic settlement guarantee?** One proof/transaction applies the spends,
    acceptance record, and payment together, and Rust checks equal amounts
    (`sdk/rs/src/channel.rs:515-610`). It does not make the pool understand offer semantics.

12. **Can the grant holder spend?** Not from the grant: it has channel keys, while the
    nullifier also requires the owner pool private key (`sdk/rs/src/disclosure.rs:45-88`,
    `sdk/rs/src/hashes.rs:153-168`).

13. **Is the grant bound to the named grantee? [WEAKNESS]** No; it is bearer and `grantee` is
    metadata (`docs/friction.md:928-936`). The checksum is integrity formatting, not
    authorization (`sdk/rs/src/disclosure.rs:290-307`).

14. **Why does the prover/RPC see the pool secret? [WEAKNESS]** Upstream `compile_actions`
    requires it in calldata, and the virtual invocation includes the same input
    (`sdk/rs/src/calldata.rs:25-53`). Both endpoints must therefore be operator-trusted
    (`sdk/rs/src/prover.rs:3-14`).

15. **Why not call `__execute__` on-chain?** It is the virtual account path that emits server
    actions for proving; the real state transition is proof-validated `apply_actions`
    (`../starknet-privacy/packages/privacy/src/privacy.cairo:193-212`,
    `../starknet-privacy/packages/privacy/src/privacy.cairo:782-839`). Rust never submits the
    proof invocation to Starknet (`sdk/rs/src/execution.rs:192-231`).

16. **What prevents index races? [WEAKNESS]** A per-handle exclusive filesystem lease and
    chain reseating serialize one local installation (`sdk/rs/src/state.rs:230-280`,
    `sdk/rs/src/client.rs:312-353`). Another machine/process with separate state can race; Cairo
    remains the final WriteOnce/contiguity authority.

17. **What happens after a crash? [WEAKNESS]** Atomic file rename prevents torn local JSON,
    and later chain reads can recover much stale cursor state (`sdk/rs/src/state.rs:380-446`,
    `sdk/rs/src/client.rs:312-353`). A crash after channel inclusion but before local creation
    can orphan the handle (`sdk/rs/src/client.rs:623-644`).

18. **Why exact-note payment with no change? [WEAKNESS]** `select_exact_notes` returns only a
    subset summing exactly to the offer, and settlement creates only the recipient payment note
    (`sdk/rs/src/client.rs:1088-1117`, `sdk/rs/src/channel.rs:596-608`). General change output is
    outside the MVP.

19. **What would you change for production?** Complete a live/cross-language v2 run and review,
    remove the salt fingerprint, cryptographically bind grants, harden crash/idempotency and
    multi-process coordination, use operator-owned RPC/prover infrastructure, and validate
    screening/gas/reorg behavior (`README.md:14-24`, `docs/friction.md:990-1015`,
    `sdk/rs/README.md:103-121`).

20. **What did this validate for Starknet Foundation?** It validated that a third party can
    independently build a Rust action/proof/submission/read path and obtain byte-level agreement
    with upstream oracles (`sdk/rs/src/lib.rs:3-20`, `sdk/rs/tests/proof_invocation.rs:1-13`).
    **[WEAKNESS]** It also validated that documentation, custody, prover access, screening,
    latency, and traffic privacy remain material adoption barriers (`docs/friction.md:436-538`,
    `docs/friction.md:1295-1427`).

## 10. Explainer layer

### Glossary

- **Note:** a pool record at `H(channel_key, token, index, 0)` containing packed salt and
  encrypted amount; spending writes a nullifier rather than erasing the note
  (`sdk/rs/src/hashes.rs:142-168`, `sdk/rs/src/decrypt.rs:103-149`).
- **Channel:** one directional sender→recipient secret and encrypted discovery record; a full
  conversation needs two opposing channels (`sdk/rs/src/channel.rs:164-253`,
  `sdk/rs/src/read.rs:295-321`).
- **Subchannel:** a token-specific indexed record inside a channel; note indices are scoped to
  `(channel_key, token)` (`sdk/rs/src/channel.rs:282-295`, `sdk/rs/src/subchannel.rs:24-25`).
- **Nullifier:** the owner-secret hash that marks a note spent without deleting/revealing its
  note slot (`sdk/rs/src/hashes.rs:153-168`,
  `../starknet-privacy/packages/privacy/src/privacy.cairo:616-628`).
- **Action set:** an ordered, replay-protected batch of client intentions compiled privately
  into server actions (`sdk/rs/src/action_set.rs:1-28`).
- **Salt lane:** the public 119 usable bits in each encrypted-note salt used here to carry a
  fragmented zero-value negotiation message (`sdk/rs/src/wire.rs:7-17`).
- **Shielding:** depositing a public token amount and creating a private encrypted self-note in
  one balanced/replay-protected action set (`sdk/rs/src/channel.rs:329-365`).
- **Viewing grant:** a bearer package with both directional channel keys and one token’s scope;
  it reads one relationship but carries no owner pool key (`sdk/rs/src/disclosure.rs:45-88`).
- **Proving block:** the historical block against which virtual execution is proved; recent
  writes must mature into that view and the result must be submitted before expiry
  (`sdk/rs/src/execution.rs:105-130`, `sdk/rs/src/execution.rs:143-190`).

### Process diagram

```text
Agent decision
  → MCP JSON tool call
  → identity-bound Python MCP adapter
  → sdk/py blocking subprocess request (key paths only)
  → erebus-cli / Rust Client (opens keys + locked state)
  → trusted RPC preflight + trusted prover
  → signed account invoke containing proof/proof_facts
  → Starknet privacy pool apply_actions
```

The boundaries and payloads are implemented at `mcp-server/src/server.py:42-76`,
`sdk/py/src/erebus/_seam.py:120-173`, `sdk/rs/src/bin/erebus_cli.rs:202-306`, and
`sdk/rs/src/execution.rs:132-239`.

### Settlement diagram

```text
two directional channel keys
       │
       ├─ offer:   [data note ×5]
       ├─ counter: [data note ×5]
       └─ accept + settle (one ActionSet)
            ├─ UseNote(input A)
            ├─ UseNote(input B) ...        phase 4
            ├─ acceptance data notes ×5    phase 5, zero value
            └─ payment value note ×1       phase 5, random salt
                    │
           compile → prove → compare → apply_actions
                    │
           all accepted or all reverted
```

The exact construction and phase sort are at `sdk/rs/src/channel.rs:527-610`; execution is at
`sdk/rs/src/execution.rs:132-239`.

### Five-minute spoken version

“This is not a Rust rewrite of the whole StarkWare SDK. It is a narrow Rust client for one
application flow. It reproduces the privacy-pool hashes, note decryption, Cairo action
serialization, proof invocation, transaction hash, signing, prover RPC, and final submission
that this flow needs. Above those pieces it adds its own offer, counter, acceptance,
persistence, and disclosure protocol. The source itself defines that boundary
(`sdk/rs/src/lib.rs:3-20`, `sdk/rs/src/client.rs:538-573`).

We wrote the write path in Rust because the upstream Rust crate covered discovery but not
action construction and proving, and because the Python agent layer must not hold pool or
account keys. Python sends file paths through a one-request subprocess seam. Rust opens the
keys, owns the state, and performs the network lifecycle (`sdk/py/src/erebus/_seam.py:1-18`,
`sdk/rs/src/execution.rs:132-239`).

The underlying pool is note based. A note’s location is a Poseidon hash of a secret channel
key, token, and index. Spending does not erase it; it writes an owner-secret nullifier. Notes
must be created at contiguous indices, so clients find them by deriving index zero upward and
stopping at the first empty slot (`sdk/rs/src/hashes.rs:142-168`,
`sdk/rs/src/client.rs:445-521`).

The pool has no application payload field. This protocol therefore puts a fixed 400-bit
message into note salts on zero-value notes. Version 2 encrypts and authenticates those bytes
and needs five notes per message. Value notes use random salts, because mixing structured
salts with the amount mask is a confidentiality error (`sdk/rs/src/wire.rs:7-45`,
`sdk/rs/src/channel.rs:490-512`).

For settlement, the payer consumes an exact subset of its notes, writes five zero-value notes
containing the acceptance, and creates one value note for the payee. Those actions are one
ordered action set, one proof, and one final `apply_actions` transaction. That gives all-or-none
application. The Rust constructor separately checks that the acceptance amount and payment
amount agree, because the pool itself does not know what an offer means
(`sdk/rs/src/channel.rs:515-610`, `docs/friction.md:865-896`).

Correctness comes from differential evidence rather than confidence in a second
implementation. Hashes and decryption match Cairo vectors; action serialization and the full
proof invocation match the upstream TypeScript SDK; transaction hashes and signatures match
starknet.js (`sdk/rs/tests/cairo_conformance.rs:1-13`,
`sdk/rs/tests/clientaction_serde.rs:1-11`, `sdk/rs/tests/proof_invocation.rs:1-13`,
`sdk/rs/tests/invoke_v3_txhash.rs:1-11`). The first Rust bug truncated felt domain tags into
128 bits, and those KATs caught it immediately (`docs/friction.md:406-434`).

The honest limits are important. The prover and preflight RPC see the pool private key. Wire
version 2 hides terms but still fingerprints traffic and exposes account timing. The viewing
grant is a bearer channel secret, not cryptographically bound to its named grantee. The
disclosed record reconstructs and checks what the notes say, but there is not yet a ZK receipt
binding participant metadata and negotiation policy to settlement. And version 2 still needs
a fresh live run, a second implementation, and independent review
(`sdk/rs/src/prover.rs:3-14`, `docs/friction.md:990-1015`, `docs/friction.md:922-936`,
`README.md:14-24`).

What this proves is narrower and useful: a third party can implement the client-critical path
in Rust, match upstream byte-for-byte oracles, and run the full offline execution composition.
It also gives concrete feedback on what makes the privacy stack hard to adopt: missing
language-neutral specifications, strong endpoint custody assumptions, external prover and
screening operations, proving latency, and metadata leakage (`sdk/rs/src/lib.rs:11-20`,
`docs/friction.md:436-538`, `docs/friction.md:1295-1427`).”
