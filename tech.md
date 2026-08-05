# Erebus Rust SDK and MCP server: source-grounded technical explanation

This document describes the repository as checked on 2026-08-05. A citation such as
[`sdk/rs/src/client.rs:575-646`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L575-L646) means the claim is visible at those source lines. “I’m
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
([`sdk/rs/src/client.rs:538-573`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L538-L573)).

It is not a separate payment token and the implemented write path does not submit an Erebus
contract call. It builds the existing STRK20 `ClientAction` variants and ultimately submits
the prover-produced server actions to the pool's `apply_actions` entrypoint
([`sdk/rs/src/actions.rs:288-313`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/actions.rs#L288-L313), [`sdk/rs/src/execution.rs:171-195`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L171-L195)). Erebus's contribution is
the meaning and lifecycle imposed on those actions: which notes represent negotiation data,
which note represents payment, how the two are made atomic, how a transcript is reconstructed,
and how access to that transcript can be delegated ([`sdk/rs/src/wire.rs:1-45`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L1-L45),
[`sdk/rs/src/channel.rs:515-610`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L515-L610), [`sdk/rs/src/disclosure.rs:1-36`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L1-L36)).

### The three layers to keep separate

1. **STRK20 is the private state-transition layer.** It supplies channels, token-specific
   subchannels, encrypted notes, nullifiers, action compilation, proof verification, and
   `apply_actions`. The Rust client models the pool's ten action variants and their Cairo
   serialization ([`sdk/rs/src/actions.rs:288-434`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/actions.rs#L288-L434)).
2. **Erebus is the application protocol over those primitives.** It gives five consecutive
   zero-value notes the meaning “one offer, counter, or acceptance,” defines reply and expiry
   rules, and combines the final acceptance with the payment spend/create actions
   ([`sdk/rs/src/wire.rs:21-35`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L21-L35), [`sdk/rs/src/negotiation.rs:163-193`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/negotiation.rs#L163-L193),
   [`sdk/rs/src/channel.rs:515-610`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L515-L610)).
3. **The agent-facing stack is transport and policy.** MCP exposes operations such as
   `open_channel`, `propose_offer`, and `accept_and_settle`; Python adapts those calls to a
   one-request Rust subprocess; Rust alone performs the protocol derivations and network
   execution ([`mcp-server/src/erebus_mcp/tools.py:89-141`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L89-L141),
   [`mcp-server/src/erebus_mcp/seam_client.py:1-17`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L1-L17), [`sdk/py/src/erebus/_seam.py:95-165`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/py/src/erebus/_seam.py#L95-L165)).

The distinction matters when explaining security. STRK20 proves validity of the underlying
private-note transition. Erebus's Rust client checks the application meaning. For example,
it checks that the payment amount equals the accepted amount before asking for that proof
([`sdk/rs/src/channel.rs:545-555`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L545-L555)). The pool does not independently understand that a group of
zero-value notes means “Alice accepted Bob's offer”; that interpretation lives in the Erebus
wire decoder and negotiation state machine ([`sdk/rs/src/read.rs:175-230`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/read.rs#L175-L230),
[`sdk/rs/src/negotiation.rs:163-193`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/negotiation.rs#L163-L193)).

### The core mental model: one pool, two kinds of notes

A normal STRK20 value note carries private value. Erebus additionally writes **data notes**:
zero-amount encrypted-note actions whose salts contain fragments of a negotiation record.
The pool note has no application payload field, so wire v2 first packs the fixed fields
`type | replyTo | createdAt | amount | deadline | memoHash`, encrypts and authenticates the
50-byte plaintext, and splits the result across five salts ([`sdk/rs/src/wire.rs:7-35`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L7-L35)). The
`memoHash` is only a 128-bit commitment to detail held elsewhere; free-form prose and the
preimage of that commitment are not stored by this wire ([`sdk/rs/src/client.rs:938-948`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L938-L948)).

Data notes and payment notes use the same token subchannel and contiguous note-index space,
but the Rust constructors keep their salt rules separate. A data note has zero amount and a
structured salt; a value note requires fresh random salt because structured or reused salt
on differing amounts would leak their difference ([`sdk/rs/src/wire.rs:37-45`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L37-L45),
[`sdk/rs/src/channel.rs:502-512`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L502-L512)). During settlement, the five acceptance notes remain on the
fixed message grid and the payment note is placed immediately after them
([`sdk/rs/src/channel.rs:613-620`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L613-L620)).

Channels are directional. Alice-to-Bob and Bob-to-Alice have different channel keys and
therefore different note locations. Each party derives its outgoing key and learns the
reverse key from the counterparty's encrypted channel information; a full conversation reader
therefore needs both keys ([`sdk/rs/src/disclosure.rs:24-30`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L24-L30)). Inside each direction, a
subchannel is selected by token rather than by conversational topic
([`sdk/rs/src/channel.rs:282-295`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L282-L295)).

### One complete deal, step by step

1. **Fund the payer.** Before negotiation can settle, the payer needs shielded notes whose
   denominations contain an exact subset equal to the price. The MVP shielding helper
   registers when necessary, opens a self-channel and token subchannel, deposits public value,
   and creates one encrypted value note in one action set ([`sdk/rs/src/channel.rs:329-364`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L329-L364)).
   Settlement currently constructs no change note, so total balance alone is insufficient;
   the client explicitly runs exact note selection ([`sdk/rs/src/client.rs:819-831`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L819-L831)).

2. **Open both directions.** `open_channel(counterparty)` verifies the caller's registration,
   looks up the counterparty's registered public key, derives the directional channel, and
   submits registration-if-needed plus `OpenChannel` and `OpenSubchannel`
   ([`sdk/rs/src/client.rs:575-626`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L575-L626)). The method then stores an opaque local handle containing
   the channel metadata and key needed by later calls ([`sdk/rs/src/client.rs:627-645`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L627-L645)). Because
   the conversation is bidirectional, the other party opens its reverse direction before the
   client can reconstruct both sides ([`sdk/rs/src/client.rs:766-781`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L766-L781)).

3. **Write an offer.** The caller supplies amount, token, deadline, and `memo_hash`. Rust
   validates those terms, synchronizes the next contiguous note index, constructs an `Offer`
   wire message, encrypts it into data notes, executes the action set, and commits the advanced
   cursor only after the transaction is accepted ([`sdk/rs/src/client.rs:648-695`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L648-L695)).

4. **Read and counter.** A reader derives exact note IDs from channel key, token, and index;
   it does not scan events or enumerate the pool ([`sdk/rs/src/read.rs:7-25`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/read.rs#L7-L25),
   [`sdk/rs/src/read.rs:149-172`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/read.rs#L149-L172)). `counter_offer` first proves the referenced item is a
   counterparty offer or counter, then writes a new message containing its index in `replyTo`;
   it does not mutate the earlier record ([`sdk/rs/src/client.rs:698-763`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L698-L763)).

5. **Accept as the payer.** The caller can accept only a known, live counterparty offer. The
   client discovers its spendable private notes at a proof-compatible block and selects an
   exact subset for the offered amount ([`sdk/rs/src/client.rs:789-831`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L789-L831)). It then builds one
   ordered action set containing the input-note spends, five acceptance data notes, and the
   recipient's value note; the amount-equality check prevents an atomic but semantically
   inconsistent underpayment ([`sdk/rs/src/client.rs:845-864`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L845-L864),
   [`sdk/rs/src/channel.rs:545-610`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L545-L610)). After chain acceptance, the local channel becomes terminal
   and repeated settlement is rejected ([`sdk/rs/src/client.rs:865-875`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L865-L875),
   [`sdk/rs/src/client.rs:797-801`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L797-L801)).

6. **Simulate, prove, and submit.** Every write is first compiled against a historical proving
   block. Rust builds the virtual proof invocation, asks the proving service for a proof,
   rejects any mismatch between locally simulated and prover-returned server actions, checks
   proof freshness, estimates resources, signs the account transaction, submits
   `apply_actions`, and waits for an accepted receipt ([`sdk/rs/src/execution.rs:132-238`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L132-L238)).

7. **Disclose if required.** Either party can export a bearer viewing grant containing both
   directional channel keys for one token. A holder can use it to locate, decrypt, and
   reconstruct that channel's offers, counters, acceptance, and settlement, but the grant
   carries no pool private key and therefore cannot produce nullifiers or spend notes
   ([`sdk/rs/src/disclosure.rs:24-36`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L24-L36), [`sdk/rs/src/disclosure.rs:45-74`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L45-L74),
   [`sdk/rs/src/client.rs:918-934`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L918-L934)).

### What crosses each software boundary

An agent calls MCP tools using public terms and opaque identifiers. The real MCP backend runs
the blocking Python seam away from the event loop, and the seam starts `erebus-cli` once per
request with JSON on standard input and expects one JSON envelope on standard output
([`mcp-server/src/erebus_mcp/seam_client.py:1-17`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L1-L17), [`sdk/py/src/erebus/_seam.py:120-165`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/py/src/erebus/_seam.py#L120-L165),
[`sdk/rs/src/bin/erebus_cli.rs:429-450`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/bin/erebus_cli.rs#L429-L450)). Python receives paths to the pool and account key
files, not their contents; the CLI request type likewise accepts file paths
([`mcp-server/src/erebus_mcp/config.py:41-57`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L41-L57), [`sdk/rs/src/bin/erebus_cli.rs:84-95`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/bin/erebus_cli.rs#L84-L95)). Persistent
channel keys remain in Rust-owned, per-handle state protected by exclusive locks and atomic
replacement ([`sdk/rs/src/state.rs:192-225`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L192-L225), [`sdk/rs/src/state.rs:230-248`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L230-L248),
[`sdk/rs/src/state.rs:400-446`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L400-L446)).

This is the intended live path, not the default behavior of every checkout. The MCP server
defaults to its mock backend; selecting `EREBUS_BACKEND=seam` enables and validates the real
Rust configuration ([`mcp-server/src/erebus_mcp/config.py:10-13`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L10-L13),
[`mcp-server/src/erebus_mcp/config.py:72-113`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L72-L113)). Therefore a successful agent demo is not by
itself evidence that the Rust, prover, RPC, or pool path ran.

### Privacy scope and limits

Wire v2 encrypts and authenticates negotiation contents before placing ciphertext fragments
in public salts ([`sdk/rs/src/wire.rs:3-17`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L3-L17), [`sdk/rs/src/wire.rs:29-35`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L29-L35)). Note discovery is
keyed: someone without the channel key cannot directly compute the locations the reader asks
for ([`sdk/rs/src/read.rs:7-19`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/read.rs#L7-L19)). **I'm inferring the observer consequence from those two
mechanisms:** an observer without the channel key cannot decode the fixed offer fields or use
the Erebus reader to locate the transcript; verify this claim against a transaction trace and
an independent wire-v2 review, neither of which the code itself supplies.

Wire v2 does not make the pool interaction invisible. An observer still sees the submitting
account, transaction timing and action shape, and the five public salt values; the current
fifth-chunk shape is distinguishable from uniformly random salts
([`sdk/rs/tests/wire_v2_fingerprint.rs:31-75`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/wire_v2_fingerprint.rs#L31-L75)). Consequently, encrypted terms are implemented,
but relationship-graph and cadence privacy are **not yet demonstrated** by this repository.

Atomicity is narrower than semantic proof. The final acceptance and payment share one action
set, so the client does not intentionally submit one without the other
([`sdk/rs/src/channel.rs:515-523`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L515-L523)). The equality between accepted amount and payment amount is
a Rust-side validation, however, not a statement that the STRK20 circuit understands the
negotiation record ([`sdk/rs/src/channel.rs:545-555`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L545-L555)). A disclosed record can reconstruct and
locally check the encoded history; the current implementation does not produce a separate ZK
receipt proving the business meaning, participant claims, disclosure policy, and settlement
consistency to an external verifier.

The viewing grant is also intentionally a bearer secret, not recipient-encrypted capability.
Its `grantee` value is metadata at the outer API, while possession of the serialized grant is
what permits reading ([`sdk/rs/src/client.rs:878-915`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L878-L915), [`sdk/rs/src/disclosure.rs:45-74`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L45-L74)). Its
checksum detects incompatible or edited grant data, but it is not a signature that
authenticates who issued the grant ([`sdk/rs/src/disclosure.rs:106-146`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L106-L146)).

### What Erebus is not

- It is not a free-form encrypted-chat protocol: the on-chain wire contains six fixed fields
  and only a hash of any external memo ([`sdk/rs/src/wire.rs:21-27`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L21-L27),
  [`sdk/rs/src/client.rs:938-948`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L938-L948)).
- It is not a general Rust interface for every STRK20 operation: the high-level trait is the
  seven-method negotiation surface ([`sdk/rs/src/client.rs:538-573`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L538-L573)).
- It is not a production security claim: wire v2 still needs live on-chain exercise,
  independent review, and stronger traffic-shape privacy
  ([`sdk/rs/tests/wire_v2_fingerprint.rs:31-75`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/wire_v2_fingerprint.rs#L31-L75)).
- It is not cryptographic proof that two businesses meant the same thing by `memoHash`; that
  field commits to off-chain detail whose preimage and semantics are outside this wire
  ([`sdk/rs/src/client.rs:938-948`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L938-L948)).

### Why it is designed this way: choices and tradeoffs

This table records the reasons stated by the source and architecture notes. “Cost” means a
mechanical consequence of the choice, not a recommendation that the choice was right or wrong.

| Design choice | Why the repository chose it | What the choice provides | Concrete cost or limit |
|---|---|---|---|
| Reuse STRK20 notes and actions instead of deploying an Erebus application contract | A pool note has no payload field, but its salt is client-writable. `InvokeExternal` would publish a distinct target contract and off-chain transport would move the negotiation graph outside the reconstructable pool record ([`docs/friction.md:207-223`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L207-L223)). | Negotiation and payment can share the pool's action compiler, proof, and `apply_actions` transition ([`sdk/rs/src/actions.rs:288-434`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/actions.rs#L288-L434), [`sdk/rs/src/execution.rs:171-195`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L171-L195)). | The pool proves note-state validity, not the business interpretation of an offer. Erebus must validate amount agreement and interpret the transcript in client code ([`sdk/rs/src/channel.rs:545-555`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L545-L555), [`sdk/rs/src/read.rs:175-230`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/read.rs#L175-L230)). |
| Encode one message as five zero-amount data notes | Salt is the note's only application-writable lane. Five 119-bit payload chunks fit the 400-bit message, version byte, ciphertext, and 128-bit authentication tag ([`sdk/rs/src/wire.rs:7-35`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L7-L35), [`sdk/rs/src/wire.rs:56-95`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L56-L95)). | The record stays on-chain, fixed-width, directly seekable, and reconstructable by a channel-key holder ([`sdk/rs/src/read.rs:149-184`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/read.rs#L149-L184)). | Every message permanently consumes five sequential note slots; its regular shape remains fingerprintable and each write pays pool execution/proving costs ([`sdk/rs/src/wire.rs:34-45`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L34-L45), [`sdk/rs/tests/wire_v2_fingerprint.rs:31-75`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/wire_v2_fingerprint.rs#L31-L75)). |
| Encrypt with AES-256-GCM-SIV and derive context from chain, pool, channel, token, and index | A failed attempt may reuse the same note index with different terms. The architecture selected a nonce-misuse-resistant construction so that this retry case is not catastrophic ([`ARCHITECTURE.md:381-384`](https://github.com/PoulavBhowmick03/Erebus/blob/main/ARCHITECTURE.md#L381-L384)). | Terms are authenticated as well as encrypted, and ciphertext is bound to its intended protocol context ([`sdk/rs/src/wire.rs:383-476`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L383-L476)). | The five-note shape, submitting account, timing, and ciphertext-bearing public salts remain observable; encryption does not supply traffic-analysis resistance ([`sdk/rs/tests/wire_v2_fingerprint.rs:31-75`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/wire_v2_fingerprint.rs#L31-L75)). |
| Keep the wire fixed-width rather than store prose | Offers require six bounded fields, and fixed v2 stride lets a reader calculate `5k..5k+4` without scanning for message boundaries ([`sdk/rs/src/wire.rs:21-35`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L21-L35)). | Deterministic framing, bounded decoding, and direct keyed reads ([`sdk/rs/src/read.rs:149-184`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/read.rs#L149-L184)). | Arbitrary terms must live elsewhere and be represented only by the 128-bit `memo_hash`; adding fields is a wire-version change ([`sdk/rs/src/client.rs:938-948`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L938-L948)). |
| Use directional channel keys and token-specific subchannels | This follows the pool's channel construction: the sender derives a key using its private key, while the recipient learns it through encrypted channel information ([`sdk/rs/src/channel.rs:19-24`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L19-L24)). Token is already part of the subchannel, so it need not occupy message bits ([`sdk/rs/src/channel.rs:282-295`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L282-L295)). | A reader holding one directional key learns only that direction and token-scoped note locations ([`sdk/rs/src/read.rs:99-110`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/read.rs#L99-L110)). | A complete negotiation requires two channel keys and both parties to establish reverse directions; a disclosure grant must carry both ([`sdk/rs/src/disclosure.rs:24-30`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L24-L30)). |
| Use keyed discovery with contiguous indices instead of event scanning | The note ID is derived from the channel key, token, and index, so an authorized reader can request exact slots without enumerating everyone else's notes ([`sdk/rs/src/read.rs:7-19`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/read.rs#L7-L19), [`sdk/rs/src/read.rs:149-172`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/read.rs#L149-L172)). Contiguity makes the first absent slot a sound end marker ([`sdk/rs/src/read.rs:21-25`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/read.rs#L21-L25)). | Discovery does not require publishing an index that maps all pool activity into relationships ([`sdk/rs/src/channel.rs:26-30`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L26-L30)). | Every writer must serialize allocation and never create gaps; a gap makes later notes undiscoverable, while direct RPC discovery costs repeated reads ([`sdk/rs/src/channel.rs:265-269`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L265-L269), [`sdk/rs/src/read.rs:149-172`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/read.rs#L149-L172)). |
| Put acceptance and payment in one action set | The design wants no committed acceptance without its corresponding payment. One action set shares one proof and one `apply_actions` transition ([`sdk/rs/src/channel.rs:515-523`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L515-L523)). | Both state changes land or neither does, and the Rust builder enforces the pool's spend-before-create phase ordering before proving ([`sdk/rs/src/channel.rs:521-523`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L521-L523), [`sdk/rs/src/action_set.rs:121-177`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/action_set.rs#L121-L177)). | Atomicity alone does not show that the terms and payment agree, so Rust must perform a separate equality check ([`sdk/rs/src/channel.rs:545-555`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L545-L555)). Settlement also leaves the message cursor off-grid, making the current channel a one-deal channel ([`sdk/rs/src/channel.rs:613-623`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L613-L623)). |
| Require an exact subset of existing notes and create no change | This is the behavior implemented by the MVP selector and settlement construction: it selects notes totalling exactly the offer and creates only the recipient payment note ([`sdk/rs/src/client.rs:819-860`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L819-L860)). **I could not find a source comment establishing that no-change was chosen as a cryptographic necessity; treat it as an MVP implementation limit, not an STRK20 requirement.** | The settlement builder avoids introducing a second owner change note and another allocation path ([`sdk/rs/src/client.rs:819-860`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L819-L860)). | A payer can own enough total value and still be unable to settle. For example, a single larger note cannot pay a smaller price ([`sdk/rs/src/client.rs:819-829`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L819-L829)). |
| Implement the write path in Rust while retaining TypeScript as an oracle | The crate states that upstream Rust covers discovery but not action building, Cairo serialization, signing, or proving; it also states that silent cryptographic divergence requires known-answer tests ([`sdk/rs/src/lib.rs:3-20`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/lib.rs#L3-L20)). | The agent's critical protocol and key-handling path runs in Rust, while fixtures pin agreement with independent Cairo, TypeScript, and starknet.js behavior ([`sdk/rs/tests/cairo_conformance.rs:1-13`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/cairo_conformance.rs#L1-L13), [`sdk/rs/tests/wire_codec.rs:1-10`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/wire_codec.rs#L1-L10), [`sdk/rs/tests/clientaction_serde.rs:1-11`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/clientaction_serde.rs#L1-L11)). | **I'm inferring the maintenance cost from the duplicated implementations:** every upstream format change must be detected and reconciled in Rust and the fixtures; verify this by updating the pinned sibling revision and rerunning the differential tests. |
| Encode invariants in Rust types and builders | The code separates `RandomSalt` from `NoteSalt`, makes `ActionSet` constructible only through its validating builder, and keeps the pool secret behind `PoolIdentity` without an accessor ([`sdk/rs/src/actions.rs:25-104`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/actions.rs#L25-L104), [`sdk/rs/src/action_set.rs:97-177`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/action_set.rs#L97-L177), [`sdk/rs/src/channel.rs:98-123`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L98-L123)). | Invalid salt use, phase regression, duplicate invoke phases, and key leakage through the ordinary API become harder or impossible to express ([`sdk/rs/src/action_set.rs:131-177`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/action_set.rs#L131-L177), [`sdk/rs/src/channel.rs:7-17`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L7-L17)). | Low-level action structs remain public, so a caller bypassing the high-level constructors can still assemble some semantically dangerous values; the type protection is strongest on the intended `Channel`/`Client` path ([`sdk/rs/src/actions.rs:164-286`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/actions.rs#L164-L286), [`sdk/rs/src/channel.rs:502-512`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L502-L512)). |
| Put a one-shot subprocess between Python agents and Rust | Key-file values need not enter Python, Rust can own its Tokio runtime, and the seam can return an explicit JSON error envelope instead of maintaining a PyO3 ABI ([`sdk/py/src/erebus/_seam.py:59-92`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/py/src/erebus/_seam.py#L59-L92), [`sdk/py/src/erebus/_seam.py:95-165`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/py/src/erebus/_seam.py#L95-L165), [`sdk/rs/src/bin/erebus_cli.rs:429-450`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/bin/erebus_cli.rs#L429-L450); rationale recorded at [`ARCHITECTURE.md:184-221`](https://github.com/PoulavBhowmick03/Erebus/blob/main/ARCHITECTURE.md#L184-L221)). | The agent layer passes public data, paths, and opaque handles while protocol derivations and keys remain below the process boundary ([`mcp-server/src/erebus_mcp/config.py:41-57`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L41-L57), [`sdk/rs/src/channel.rs:7-17`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L7-L17)). | Each call pays process startup and JSON serialization; state that an in-process library would retain in memory instead requires a protected filesystem store ([`sdk/py/src/erebus/_seam.py:95-165`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/py/src/erebus/_seam.py#L95-L165), [`sdk/rs/src/state.rs:192-248`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L192-L248)). |
| Persist opaque handles with an exclusive lease and commit after chain success | A one-shot CLI must recover channel keys and the next note index across processes, while concurrent calls must not allocate the same slot ([`sdk/rs/src/state.rs:192-248`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L192-L248), [`sdk/rs/src/state.rs:425-446`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L425-L446)). | Agent-visible handles reveal no channel key, per-handle locking serializes cursor changes, and atomic replacement avoids partial state files ([`sdk/rs/src/state.rs:192-225`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L192-L225), [`sdk/rs/src/state.rs:400-446`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L400-L446)). | The local OS account and state directory become trust and availability boundaries. A crash after chain inclusion but before `commit` can leave local state stale; the current test suite does not exercise that recovery case, as recorded later in Part 5. |
| Make disclosure a self-contained bearer grant | `reveal` is meant to work from the grant and chain data without the grantor's state directory or pool private key. Both directional keys are necessary to reconstruct both halves of the conversation ([`sdk/rs/src/disclosure.rs:24-36`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L24-L36), [`sdk/rs/src/client.rs:918-934`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L918-L934)). | The holder gets channel-scoped read capability but cannot compute spend nullifiers without the owner's pool private key ([`sdk/rs/src/disclosure.rs:32-36`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L32-L36)). | Possession, not the `grantee` metadata, controls access; copying or leaking the grant discloses the record, and the checksum is integrity checking rather than issuer authentication ([`sdk/rs/src/disclosure.rs:45-74`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L45-L74), [`sdk/rs/src/disclosure.rs:106-146`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L106-L146)). |
| Trust the prover and write RPC with the pool private key | This is inherited from the upstream proving interface: the virtual invocation contains the pool key, and the `compile_actions` preflight sends the same secret to its RPC endpoint ([`sdk/rs/src/prover.rs:3-14`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/prover.rs#L3-L14)). | Those services can compile and prove the private transition without receiving the separate Starknet account signing key ([`sdk/rs/src/execution.rs:132-174`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L132-L174), [`sdk/rs/src/execution.rs:222-231`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L222-L231)). | Both endpoints can decrypt what the pool key protects and therefore sit inside the confidentiality trust boundary, even though the account signature remains separately necessary to submit the transaction ([`sdk/rs/src/prover.rs:3-14`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/prover.rs#L3-L14), [`sdk/rs/src/execution.rs:222-231`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L222-L231)). |
| Accept STRK20's pool-wide auditor escrow and add a narrower channel grant | Registration writes the pool private key encrypted to the configured auditor; Erebus cannot opt out of that pool rule ([`sdk/rs/src/channel.rs:126-140`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L126-L140)). The application grant instead releases only two directional channel keys for one token ([`sdk/rs/src/disclosure.rs:7-22`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L7-L22)). | The application can disclose one relationship without handing the recipient the pool-wide spending/decryption root ([`sdk/rs/src/disclosure.rs:15-36`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L15-L36)). | The pool auditor retains broader visibility across the registered identity's pool history, while the application grant adds another secret that must be delivered and protected ([`sdk/rs/src/channel.rs:126-136`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L126-L136), [`sdk/rs/src/disclosure.rs:45-74`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L45-L74)). |

## 0. Scope boundary: what this Rust SDK is, and is not

### The one-sentence framing to use

**Say this:** “The Rust crate is an Erebus-specific STRK20 client: it independently implements
the selected privacy-pool write, read, proving, signing, and RPC primitives needed for the
Erebus flow, pins those primitives against Cairo/TypeScript/starknet.js oracles, and adds an
original two-party negotiation, persistence, and selective-disclosure protocol; it is not a
full port or drop-in replacement for StarkWare’s TypeScript SDK.” The crate’s own module
documentation says that upstream `discovery-core` covers reads while no upstream Rust write
side builds `ClientAction`s, serializes calldata, signs, or calls the prover
([`sdk/rs/src/lib.rs:3-20`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/lib.rs#L3-L20)); the high-level Rust surface is seven negotiation methods, not the
upstream general-purpose transfer API ([`sdk/rs/src/client.rs:538-573`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L538-L573)).

Do **not** call it “our Rust rewrite of the Starknet privacy SDK.” Upstream exports a broad
`createPrivateTransfers` API, discovery/indexer providers, history, OHTTP, and classifiers
([`../starknet-privacy/sdk/src/index.ts:1-52`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/sdk/src/index.ts#L1-L52)), whereas this crate exposes only its nineteen
modules and one CLI ([`sdk/rs/src/lib.rs:22-42`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/lib.rs#L22-L42), [`sdk/rs/Cargo.toml:38-40`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/Cargo.toml#L38-L40)). “Rewrite” therefore
overstates compatibility and understates the original protocol layered above the pool.

### What is a port or compatibility reimplementation

- The Poseidon domain tags and every hash preimage in [`hashes.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs) are direct ports of
  [`packages/privacy/src/hashes.cairo`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/hashes.cairo) ([`sdk/rs/src/hashes.rs:1-16`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L1-L16);
  [`../starknet-privacy/packages/privacy/src/hashes.cairo:9-39`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/hashes.cairo#L9-L39)). The same subset already has
  an upstream Rust implementation in `discovery-core`, so this is not the first Rust
  expression of those formulas ([`../starknet-privacy/crates/discovery-core/src/privacy_pool/hashes.rs:64-223`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/crates/discovery-core/src/privacy_pool/hashes.rs#L64-L223)).
- Additive note, subchannel, outgoing-channel, and ECDH channel-info decryption reproduce the
  Cairo/read-side behavior ([`sdk/rs/src/decrypt.rs:21-35`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/decrypt.rs#L21-L35), [`sdk/rs/src/decrypt.rs:103-200`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/decrypt.rs#L103-L200)).
  They were reimplemented rather than imported because upstream `discovery-core` pins a
  `starknet-rust` fork and pulls in the provider stack that this crate intentionally avoided
  ([`sdk/rs/src/decrypt.rs:6-19`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/decrypt.rs#L6-L19), [`sdk/rs/Cargo.toml:8-36`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/Cargo.toml#L8-L36)).
- The ten `ClientAction` variants, their enum indices, field order, span encoding, and phase
  mapping mirror Cairo and upstream TypeScript serialization ([`sdk/rs/src/actions.rs:288-434`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/actions.rs#L288-L434);
  [`../starknet-privacy/packages/privacy/src/actions.cairo:245-315`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/actions.cairo#L245-L315);
  [`../starknet-privacy/sdk/src/internal/serialization.ts:9-28`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/sdk/src/internal/serialization.ts#L9-L28)).
- `compile_actions` calldata, the virtual pool-account `__execute__` wrapper, proof invocation,
  Stark ECDSA, v3 transaction hashing, proving RPC, screening suffix, and final
  `apply_actions` call independently reproduce the upstream execution path
  ([`sdk/rs/src/calldata.rs:25-82`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/calldata.rs#L25-L82), [`sdk/rs/src/execution.rs:132-239`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L132-L239);
  [`../starknet-privacy/sdk/src/internal/proof-invocation-factory.ts:88-195`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/sdk/src/internal/proof-invocation-factory.ts#L88-L195);
  [`../starknet-privacy/sdk/src/internal/private-transfers.ts:94-136`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/sdk/src/internal/private-transfers.ts#L94-L136)).
- Wire v1 is a port of this repository’s TypeScript salt codec, not an upstream STRK20
  primitive ([`sdk/rs/src/wire.rs:1-5`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L1-L5), [`sdk/ts/src/channel/wire.ts:1-46`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/ts/src/channel/wire.ts#L1-L46)).

### What is original Erebus protocol work

The 400-bit offer/counter/accept schema, its five-note AES-256-GCM-SIV wire v2, the fixed
message grid, and the use of zero-value note salts as a payload lane are Erebus-specific;
the module explicitly says a pool note has no payload field and describes the five-note
envelope ([`sdk/rs/src/wire.rs:7-45`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L7-L45)). `OfferBook` supplies deadlines, reply semantics,
direction-aware IDs, and terminal settlement that the pool does not know about
([`sdk/rs/src/negotiation.rs:163-193`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/negotiation.rs#L163-L193), [`sdk/rs/src/negotiation.rs:231-272`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/negotiation.rs#L231-L272)).

The following are also original client/protocol machinery: `ActionSetBuilder`’s early mirror
of pool phase/replay constraints ([`sdk/rs/src/action_set.rs:1-28`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/action_set.rs#L1-L28)), `SubchannelCursor`’s
contiguous allocator ([`sdk/rs/src/subchannel.rs:1-32`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/subchannel.rs#L1-L32)), atomic accept-plus-payment composition
([`sdk/rs/src/channel.rs:515-610`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L515-L610)), opaque-handle state and lease/commit persistence
([`sdk/rs/src/state.rs:174-228`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L174-L228), [`sdk/rs/src/state.rs:425-446`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L425-L446)), a two-direction bearer viewing
grant ([`sdk/rs/src/disclosure.rs:45-88`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L45-L88)), the high-level `Client` workflow
([`sdk/rs/src/client.rs:575-935`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L575-L935)), and the protocol-2 one-shot CLI
([`sdk/rs/src/bin/erebus_cli.rs:27-82`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/bin/erebus_cli.rs#L27-L82), [`sdk/rs/src/bin/erebus_cli.rs:429-450`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/bin/erebus_cli.rs#L429-L450)).

### What upstream functionality was intentionally not ported

The Rust high-level client does not expose upstream’s general action compiler, arbitrary open
notes, withdrawals, private swaps/DeFi invokes, compute-and-invoke flow, discovery service,
history/indexing, OHTTP, paymasters, or general change construction. Although the low-level
Rust enum can serialize all ten Cairo variants, the frozen high-level trait exposes only
channel negotiation, settlement, and disclosure ([`sdk/rs/src/actions.rs:288-313`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/actions.rs#L288-L313);
[`sdk/rs/src/client.rs:538-573`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L538-L573)). The configured client is also single-token
([`sdk/rs/src/client.rs:37-59`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L37-L59)). The repository calls these MVP limits and explicitly excludes
general note selection/change, multi-token negotiation, paymasters, and production custody
([`sdk/rs/README.md:103-121`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/README.md#L103-L121), [`CLAUDE.md:113-132`](https://github.com/PoulavBhowmick03/Erebus/blob/main/CLAUDE.md#L113-L132)).

The TypeScript static-static ECDH helper in this repository was not ported into the active
Rust path. It describes a planned off-chain shared secret ([`sdk/ts/src/crypto/channel-secret.ts:1-29`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/ts/src/crypto/channel-secret.ts#L1-L29)),
while the Rust protocol uses the directional channel key that the Cairo pool derives from the
sender’s private key and sends to the recipient via ephemeral-static ECDH
([`sdk/rs/src/hashes.rs:74-93`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L74-L93), [`sdk/rs/src/decrypt.rs:152-175`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/decrypt.rs#L152-L175)).

### Documentation disagreements

- [`CLAUDE.md`](https://github.com/PoulavBhowmick03/Erebus/blob/main/CLAUDE.md) and [`sdk/rs/README.md`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/README.md) still call the Python seam “protocol 1”
  ([`CLAUDE.md:93-97`](https://github.com/PoulavBhowmick03/Erebus/blob/main/CLAUDE.md#L93-L97), [`sdk/rs/README.md:117-121`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/README.md#L117-L121)), but the current Python and Rust code both
  say and return protocol 2 ([`sdk/py/src/erebus/_seam.py:15-18`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/py/src/erebus/_seam.py#L15-L18),
  [`sdk/rs/src/bin/erebus_cli.rs:202-210`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/bin/erebus_cli.rs#L202-L210)). The running code wins.
- [`sdk/ts/src/interface.ts`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/ts/src/interface.ts) is an older, non-shipping interface with a string memo, nonce,
  `withdrawn` status, and a grant method returning `void`
  ([`sdk/ts/src/interface.ts:39-57`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/ts/src/interface.ts#L39-L57), [`sdk/ts/src/interface.ts:151-172`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/ts/src/interface.ts#L151-L172)). Current Rust carries a
  128-bit `memo_hash`, no offer nonce/withdrawal, and returns a bearer grant
  ([`sdk/rs/src/client.rs:938-1079`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L938-L1079)). Each language compiler obeys its source, but the repo says
  `/sdk/ts` “ships nothing,” so it is not the current product contract ([`README.md:84-90`](https://github.com/PoulavBhowmick03/Erebus/blob/main/README.md#L84-L90)).
- [`ARCHITECTURE.md`](https://github.com/PoulavBhowmick03/Erebus/blob/main/ARCHITECTURE.md) says the system hides existence, participants, and cadence
  ([`ARCHITECTURE.md:466-476`](https://github.com/PoulavBhowmick03/Erebus/blob/main/ARCHITECTURE.md#L466-L476)), while the later README calls relationship privacy a target and
  the fingerprint test proves the fifth salt is distinguishable ([`README.md:51-58`](https://github.com/PoulavBhowmick03/Erebus/blob/main/README.md#L51-L58),
  [`sdk/rs/tests/wire_v2_fingerprint.rs:31-58`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/wire_v2_fingerprint.rs#L31-L58)). The later test-backed statement is the honest
  one.

### Four source comments worth quoting verbatim

> “`discovery-core` covers the *read* side, hashes, storage slots, decryption, note discovery
>, but there is no Rust write side: nothing builds `ClientAction`s, serialises Cairo calldata,
> signs the invoke, or calls the proving service. This crate is that gap.”

That is the crate’s own scope claim ([`sdk/rs/src/lib.rs:5-9`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/lib.rs#L5-L9)).

> “The invocation handed to `starknet_proveTransaction` carries the pool private key in
> plaintext at `calldata[5]`, verified, not assumed.”

That is the custody boundary ([`sdk/rs/src/prover.rs:3-11`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/prover.rs#L3-L11)).

> “Atomicity puts the acceptance and the payment in one proof, so both land or neither does.
> It says nothing about them *agreeing*.”

That is why the amount comparison is separate from atomic composition
([`sdk/rs/src/channel.rs:545-555`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L545-L555)).

> “Keep the returned lease alive through any async operation that uses or advances its
> cursor.”

That is the ownership/concurrency rule for persistent channel state
([`sdk/rs/src/state.rs:230-232`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L230-L232)).

## 1. Module map and reading order

Read these in the order below. “Depends on” names the important protocol dependency, not
every imported standard-library item.

| Order | File and layer | Responsibility; public surface; key dependencies |
|---:|---|---|
| 1 | [`lib.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/lib.rs): crate boundary | Declares the crate’s purpose, forbids unsafe code, and exports all nineteen library modules ([`sdk/rs/src/lib.rs:1-42`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/lib.rs#L1-L42)). |
| 2 | [`hashes.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs): cryptographic primitives | Exposes Poseidon `hash` plus fifteen Cairo-compatible derivations; depends only on Starknet felt/Poseidon primitives ([`sdk/rs/src/hashes.rs:18-20`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L18-L20), [`sdk/rs/src/hashes.rs:69-263`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L69-L263)). |
| 3 | [`actions.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/actions.rs): Cairo wire model | Defines salt/entropy newtypes, ten action-input structs, `ClientAction`, phase lookup, and Cairo serialization; depends on felts and the Cairo enum/field order ([`sdk/rs/src/actions.rs:25-162`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/actions.rs#L25-L162), [`sdk/rs/src/actions.rs:164-434`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/actions.rs#L164-L434)). |
| 4 | [`action_set.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/action_set.rs): local protocol invariants | Exposes `ActionSet`, `ActionSetBuilder`, and `ActionSetError`; depends on action phase and replay-protection classification ([`sdk/rs/src/action_set.rs:30-178`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/action_set.rs#L30-L178)). |
| 5 | [`subchannel.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/subchannel.rs): index allocation | Exposes `SubchannelCursor` and `IndexError`; depends on wire message width and mirrors contiguity/write-once rules ([`sdk/rs/src/subchannel.rs:34-164`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/subchannel.rs#L34-L164)). |
| 6 | [`wire.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs): negotiation codec | Exposes wire versions, message types, contexts, constants, v1 compatibility functions, and v2 authenticated codec; depends on `NoteSalt`, HKDF-SHA-256, and AES-GCM-SIV ([`sdk/rs/src/wire.rs:47-105`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L47-L105), [`sdk/rs/src/wire.rs:118-229`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L118-L229), [`sdk/rs/src/wire.rs:383-534`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L383-L534)). |
| 7 | [`decrypt.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/decrypt.rs): STRK20 read crypto | Exposes note unpack/decrypt and channel/subchannel/outgoing-info recovery; depends on `hashes` and Stark-curve point operations ([`sdk/rs/src/decrypt.rs:37-40`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/decrypt.rs#L37-L40), [`sdk/rs/src/decrypt.rs:42-200`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/decrypt.rs#L42-L200)). |
| 8 | [`channel.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs): action composition | Exposes identities, counterparties, channels, owned notes, setup/payment/acceptance inputs, and constructors for setup, shielding, messages, settlement, and grants; depends on actions, builder, hashes, wire, cursor, and disclosure ([`sdk/rs/src/channel.rs:32-45`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L32-L45), [`sdk/rs/src/channel.rs:98-709`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L98-L709)). |
| 9 | [`read.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/read.rs): keyed transcript reconstruction | Exposes `NoteSource`, `ChannelReader`, read errors, and two-direction `reconstruct`; depends on hashes, decryption, wire, and negotiation ([`sdk/rs/src/read.rs:28-35`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/read.rs#L28-L35), [`sdk/rs/src/read.rs:38-321`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/read.rs#L38-L321)). |
| 10 | [`negotiation.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/negotiation.rs): client-only state machine | Exposes direction-aware `OfferId`, statuses, errors, and `OfferBook`; depends on decoded `WireMessage`s and contains no chain code ([`sdk/rs/src/negotiation.rs:25-42`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/negotiation.rs#L25-L42), [`sdk/rs/src/negotiation.rs:95-302`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/negotiation.rs#L95-L302)). |
| 11 | [`disclosure.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs): scoped read capability | Exposes secret-bearing grant and disclosed-record types plus `reveal`; depends on both directional readers and `OfferBook` ([`sdk/rs/src/disclosure.rs:40-88`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L40-L88), [`sdk/rs/src/disclosure.rs:175-337`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L175-L337)). |
| 12 | [`calldata.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/calldata.rs): ABI assembly | Exposes selectors and exact `compile_actions`, single-call, proof-`__execute__`, screening, and `apply_actions` layouts; depends on `ActionSet` and prover additional data ([`sdk/rs/src/calldata.rs:12-17`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/calldata.rs#L12-L17), [`sdk/rs/src/calldata.rs:18-102`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/calldata.rs#L18-L102)). |
| 13 | [`tx.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/tx.rs): Starknet transaction model | Exposes v3 invoke/resource types, the privacy-specific proof-facts-aware hash, `PoolInvocation`, and signed RPC wire types; depends on Poseidon and Stark signatures ([`sdk/rs/src/tx.rs:23-25`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/tx.rs#L23-L25), [`sdk/rs/src/tx.rs:50-350`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/tx.rs#L50-L350)). |
| 14 | [`signing.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/signing.rs): account signatures | Exposes public-key derivation, sign, verify, and `SigningError`; depends on `starknet_crypto` ([`sdk/rs/src/signing.rs:1-12`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/signing.rs#L1-L12), [`sdk/rs/src/signing.rs:22-84`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/signing.rs#L22-L84)). |
| 15 | [`prover.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/prover.rs): proving transport | Exposes block IDs, proof/result/screening types, `ProvingService`, and retry-classified errors; depends on async HTTP and signed invoke wire data ([`sdk/rs/src/prover.rs:23-28`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/prover.rs#L23-L28), [`sdk/rs/src/prover.rs:30-220`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/prover.rs#L30-L220)). |
| 16 | [`rpc.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/rpc.rs): Starknet transport | Exposes the minimal JSON-RPC calls, receipt model, and `RpcError`; depends on block IDs and signed transaction wire data ([`sdk/rs/src/rpc.rs:1-15`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/rpc.rs#L1-L15), [`sdk/rs/src/rpc.rs:17-239`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/rpc.rs#L17-L239)). |
| 17 | [`execution.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs): write pipeline | Exposes execution config/receipt/error, `Executor`, maturity wait, and proof-invocation builder; depends on calldata, RPC, prover, signing, and tx modules ([`sdk/rs/src/execution.rs:26-35`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L26-L35), [`sdk/rs/src/execution.rs:39-359`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L39-L359)). |
| 18 | [`state.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs): local secret state | Exposes opaque handles, stored channel records, filesystem store, lease, and errors; depends on wire version and OS entropy/file locks ([`sdk/rs/src/state.rs:12-20`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L12-L20), [`sdk/rs/src/state.rs:26-497`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L26-L497)). |
| 19 | [`keys.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/keys.rs): key provisioning | Exposes non-overwriting pool-key creation and metadata/errors; depends on OS entropy and Stark public-key derivation ([`sdk/rs/src/keys.rs:1-13`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/keys.rs#L1-L13), [`sdk/rs/src/keys.rs:15-109`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/keys.rs#L15-L109)). |
| 20 | [`client.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs): application facade | Exposes configuration, `Client`, the seven-method trait, API records, and aggregate `ClientError`; composes every lower layer ([`sdk/rs/src/client.rs:19-31`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L19-L31), [`sdk/rs/src/client.rs:37-82`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L37-L82), [`sdk/rs/src/client.rs:538-573`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L538-L573), [`sdk/rs/src/client.rs:938-1079`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L938-L1079)). |
| 21 | [`bin/erebus_cli.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/bin/erebus_cli.rs): process boundary | Defines the protocol-2 request enum, response envelope, dispatch/error mapping, and one-request Tokio main; depends on the high-level client and key generator ([`sdk/rs/src/bin/erebus_cli.rs:10-24`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/bin/erebus_cli.rs#L10-L24), [`sdk/rs/src/bin/erebus_cli.rs:27-174`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/bin/erebus_cli.rs#L27-L174), [`sdk/rs/src/bin/erebus_cli.rs:202-306`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/bin/erebus_cli.rs#L202-L306), [`sdk/rs/src/bin/erebus_cli.rs:429-450`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/bin/erebus_cli.rs#L429-L450)). |

The conceptual dependency order is therefore: **Cairo-compatible primitives → valid action
sets and index allocation → Erebus wire/channel/read/state machine → ABI/transaction/prover/RPC
execution → persistence and high-level client → CLI/process adapters.** This follows the actual
composition imports and the fact that `Client` owns `Executor` and `StateStore`
([`sdk/rs/src/client.rs:19-31`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L19-L31), [`sdk/rs/src/client.rs:63-82`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L63-L82)).

## 2. Cryptographic derivations, salts, and negotiation wire

### 2.1 Exact Poseidon derivations

Every row below is `poseidon_hash_many` over the listed felt sequence
([`sdk/rs/src/hashes.rs:69-72`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L69-L72)). Tags are ASCII short strings right-aligned as big-endian bytes
in a felt ([`sdk/rs/src/hashes.rs:21-49`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L21-L49)). “Observer” statements are explicit inferences from
the listed inputs, not claims made by a security proof.

| Derivation | Exact preimage and upstream Cairo | Derived from / explicitly not derived from | Who can compute; wrong-preimage symptom |
|---|---|---|---|
| Channel key | `H('CHANNEL_KEY_TAG:V1', sender_addr, sender_private_key, recipient_addr, recipient_public_key)` ([`sdk/rs/src/hashes.rs:74-93`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L74-L93); [`../starknet-privacy/packages/privacy/src/hashes.cairo:114-132`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/hashes.cairo#L114-L132)) | Includes the sender’s pool secret and both endpoint identities; **not** an ECDH result, token, pool address, chain ID, or channel index ([`sdk/rs/src/hashes.rs:74-93`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L74-L93)). | The sender can derive it; the recipient learns it from encrypted channel info ([`sdk/rs/src/decrypt.rs:152-175`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/decrypt.rs#L152-L175)). I’m inferring that an observer without the sender secret cannot compute it. Verify the cryptographic assumption against Poseidon preimage resistance. A wrong value makes channel markers/subchannels/note IDs disagree; preflight may reject `INVALID_CHANNEL`, or the recipient may silently search empty note IDs ([`../starknet-privacy/packages/privacy/src/privacy.cairo:441-445`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/privacy.cairo#L441-L445); [`sdk/rs/tests/read_path.rs:245-275`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/read_path.rs#L245-L275)). |
| Channel marker | `H('CHANNEL_MARKER_TAG:V1', channel_key, sender_addr, recipient_addr, recipient_public_key)` ([`sdk/rs/src/hashes.rs:95-110`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L95-L110); [`../starknet-privacy/packages/privacy/src/hashes.cairo:150-168`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/hashes.cairo#L150-L168)) | Not token-, pool-, chain-, or index-scoped. | Anyone with the channel key and public identities can compute it. A wrong marker is loud when `open_subchannel` reads `channel_exists` and raises `INVALID_CHANNEL` ([`../starknet-privacy/packages/privacy/src/privacy.cairo:441-445`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/privacy.cairo#L441-L445)). |
| Subchannel ID | `H('SUBCHANNEL_ID_TAG:V1', channel_key, index, 0)`; the trailing zero is mandatory ([`sdk/rs/src/hashes.rs:112-123`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L112-L123); [`../starknet-privacy/packages/privacy/src/hashes.cairo:170-178`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/hashes.cairo#L170-L178)) | Not derived from token or recipient; token is encrypted in the record stored at this ID. | A channel-key holder can enumerate indices. A wrong ID makes `get_subchannel_info` look empty and discovery stop or skip the token ([`sdk/rs/src/client.rs:428-442`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L428-L442), [`sdk/rs/src/client.rs:471-486`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L471-L486)). |
| Subchannel marker | `H('SUBCHANNEL_MARKER_TAG:V1', channel_key, recipient_addr, recipient_public_key, token)` ([`sdk/rs/src/hashes.rs:125-140`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L125-L140); [`../starknet-privacy/packages/privacy/src/hashes.cairo:180-198`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/hashes.cairo#L180-L198)) | Token- and recipient-bound; not index-, chain-, or pool-bound. | A channel-key holder with public metadata can compute it. A wrong marker makes note creation or spending fail `SUBCHANNEL_NOT_FOUND` ([`../starknet-privacy/packages/privacy/src/privacy.cairo:595-604`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/privacy.cairo#L595-L604), [`../starknet-privacy/packages/privacy/src/privacy.cairo:730-734`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/privacy.cairo#L730-L734)). |
| Note ID | `H('NOTE_ID_TAG:V1', channel_key, token, index, 0)` ([`sdk/rs/src/hashes.rs:142-151`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L142-L151); [`../starknet-privacy/packages/privacy/src/hashes.cairo:200-210`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/hashes.cairo#L200-L210)) | Not amount-, salt-, sender-, recipient-, chain-, or pool-derived. | A channel-key holder can seek exact slots; an outsider cannot efficiently derive them without the key. A wrong read-side preimage is the canonical silent “not found” failure because `get_note` receives the wrong ID ([`sdk/rs/src/client.rs:355-372`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L355-L372)); a wrong write action can instead fail a subchannel/contiguity check during compile ([`../starknet-privacy/packages/privacy/src/privacy.cairo:605-617`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/privacy.cairo#L605-L617), [`../starknet-privacy/packages/privacy/src/privacy.cairo:736-751`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/privacy.cairo#L736-L751)). |
| Nullifier | `H('NULLIFIER_TAG:V1', channel_key, token, index, 0, owner_private_key)` ([`sdk/rs/src/hashes.rs:153-168`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L153-L168); [`../starknet-privacy/packages/privacy/src/hashes.cairo:224-236`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/hashes.cairo#L224-L236)) | Adds spending authority to the note locator; not amount- or salt-derived. | Only a holder of both channel key and owner pool secret can compute it. This is why a viewing grant cannot spend ([`sdk/rs/src/channel.rs:462-471`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L462-L471)). A locally wrong nullifier falsely classifies spentness; actual `UseNote` compilation recomputes the Cairo nullifier from the owner secret, so the eventual symptom can be a double-spend `NON_ZERO_VALUE`, not necessarily silence ([`sdk/rs/src/client.rs:501-517`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L501-L517); [`../starknet-privacy/packages/privacy/src/privacy.cairo:616-628`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/privacy.cairo#L616-L628), [`../starknet-privacy/packages/privacy/src/privacy.cairo:932-946`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/privacy.cairo#L932-L946)). |
| Outgoing channel ID | `H('OUTGOING_CHANNEL_ID_TAG:V1', sender_addr, sender_private_key, index, 0)` ([`sdk/rs/src/hashes.rs:170-184`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L170-L184); [`../starknet-privacy/packages/privacy/src/hashes.cairo:134-148`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/hashes.cairo#L134-L148)) | Sender-secret and index scoped; not recipient- or channel-key-derived. | The sender can enumerate its own outgoing records; a public observer cannot without the secret. A wrong ID makes outgoing-channel counting stop early or recovery read the wrong slot ([`sdk/rs/src/client.rs:292-310`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L292-L310); [`sdk/rs/src/decrypt.rs:186-200`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/decrypt.rs#L186-L200)). |
| Encrypted-amount mask | `H('ENC_AMOUNT_TAG:V1', channel_key, token, index, 0, felt(salt_u128))` ([`sdk/rs/src/hashes.rs:186-199`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L186-L199); [`../starknet-privacy/packages/privacy/src/hashes.cairo:212-222`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/hashes.cairo#L212-L222)) | Includes a bounded note salt; not owner secret or amount. Only the low 128 hash bits mask the amount with wrapping arithmetic ([`sdk/rs/src/decrypt.rs:115-137`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/decrypt.rs#L115-L137)). | A channel-key holder decrypts; public salt alone is insufficient. Wrong key/preimage returns plausible garbage without authentication ([`sdk/rs/src/decrypt.rs:21-27`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/decrypt.rs#L21-L27); [`sdk/rs/tests/decrypt_conformance.rs:104-125`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/decrypt_conformance.rs#L104-L125)). |
| Encrypted-token mask | `H('ENC_TOKEN_TAG:V1', channel_key, index, 0, salt_felt)` ([`sdk/rs/src/hashes.rs:201-212`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L201-L212); [`../starknet-privacy/packages/privacy/src/hashes.cairo:77-82`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/hashes.cairo#L77-L82)) | Uses a full felt salt and no recipient/token in the mask. | A channel-key holder decrypts the stored token by field subtraction ([`sdk/rs/src/decrypt.rs:178-184`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/decrypt.rs#L178-L184)). Wrong derivation produces another felt with no authentication and can make discovery miss the configured token ([`sdk/rs/src/client.rs:428-442`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L428-L442)). |
| Outgoing-recipient mask | `H('ENC_RECIPIENT_ADDR_TAG:V1', sender_addr, sender_private_key, index, 0, salt_felt)` ([`sdk/rs/src/hashes.rs:214-229`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L214-L229); [`../starknet-privacy/packages/privacy/src/hashes.cairo:99-112`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/hashes.cairo#L99-L112)) | Sender-secret scoped; not channel key or recipient-derived. | The sender can recover the recipient; a wrong mask silently produces another felt ([`sdk/rs/src/decrypt.rs:186-200`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/decrypt.rs#L186-L200)). |
| ECDH channel-key mask | `H('ENC_CHANNEL_KEY_TAG:V1', shared_x)` ([`sdk/rs/src/hashes.rs:231-234`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L231-L234); [`../starknet-privacy/packages/privacy/src/hashes.cairo:85-90`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/hashes.cairo#L85-L90)) | Only the ECDH shared x-coordinate; no addresses or context. | Sender and recipient obtain the same x-coordinate through ephemeral-static ECDH ([`../starknet-privacy/packages/privacy/src/utils.cairo:123-144`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/utils.cairo#L123-L144), [`sdk/rs/src/decrypt.rs:152-175`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/decrypt.rs#L152-L175)). A non-curve ephemeral x is loud; a wrong private key returns a wrong channel key without error ([`sdk/rs/src/decrypt.rs:48-57`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/decrypt.rs#L48-L57); [`sdk/rs/tests/decrypt_conformance.rs:160-204`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/decrypt_conformance.rs#L160-L204)). |
| ECDH sender-address mask | `H('ENC_SENDER_ADDR_TAG:V1', shared_x)` ([`sdk/rs/src/hashes.rs:236-239`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L236-L239); [`../starknet-privacy/packages/privacy/src/hashes.cairo:92-97`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/hashes.cairo#L92-L97)) | Same shared x only, but separate tag from channel-key mask. | Same ECDH boundary and unauthenticated failure as above ([`sdk/rs/src/decrypt.rs:162-175`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/decrypt.rs#L162-L175)). |
| Auditor private-key mask | `H('ENC_PRIVATE_KEY_TAG:V1', shared_x)` ([`sdk/rs/src/hashes.rs:241-244`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L241-L244); [`../starknet-privacy/packages/privacy/src/hashes.cairo:63-68`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/hashes.cairo#L63-L68)) | Auditor ECDH shared x only. | Used by Cairo registration to escrow the whole pool private key to the configured auditor ([`../starknet-privacy/packages/privacy/src/privacy.cairo:317-354`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/privacy.cairo#L317-L354); [`../starknet-privacy/packages/privacy/src/utils.cairo:201-227`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/utils.cairo#L201-L227)). Erebus Rust defines the hash for conformance but does not expose auditor decryption ([`sdk/rs/src/hashes.rs:241-244`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L241-L244)). |
| Auditor user-address mask | `H('ENC_USER_ADDR_TAG:V1', shared_x)` ([`sdk/rs/src/hashes.rs:246-249`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L246-L249); [`../starknet-privacy/packages/privacy/src/hashes.cairo:70-74`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/hashes.cairo#L70-L74)) | Auditor ECDH shared x only. | Used upstream for encrypted withdrawal identity, not by the high-level Erebus flow ([`../starknet-privacy/packages/privacy/src/privacy.cairo:505-523`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/privacy.cairo#L505-L523)). |
| Identity key | `H('IDENTITY_KEY_TAG:V1', user_addr, user_private_key, contract_address)` ([`sdk/rs/src/hashes.rs:251-263`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L251-L263); [`../starknet-privacy/packages/privacy/src/hashes.cairo:48-60`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/hashes.cairo#L48-L60)) | Pool-contract scoped; not channel/recipient/token-derived. | A pool-secret holder can compute it. The Rust crate pins it for conformance but its high-level client does not call it; verify any future use against the upstream call site rather than inferring one ([`sdk/rs/src/hashes.rs:251-263`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L251-L263)). |

The repository’s statement that “every failure mode … is silent” is too broad
([`CLAUDE.md:159-163`](https://github.com/PoulavBhowmick03/Erebus/blob/main/CLAUDE.md#L159-L163)). Wrong locator hashes and unauthenticated additive masks are silent, but
wire-v2 context/key/tag mistakes raise `Authentication`, invalid ephemeral points raise
`InvalidEphemeralPubkey`, phase mistakes fail the builder, and several bad markers revert in
Cairo ([`sdk/rs/src/wire.rs:455-498`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L455-L498), [`sdk/rs/src/decrypt.rs:48-57`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/decrypt.rs#L48-L57),
[`sdk/rs/src/action_set.rs:121-178`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/action_set.rs#L121-L178)). The useful precise claim is: **KATs are essential because
the highest-risk locator and additive-decryption errors can return absence or plausible data
instead of a type/crypto error.**

### 2.2 Salt lanes and the confidentiality invariant

There are three Rust types because “salt” names different protocol roles:

| Type | Valid range and permitted uses | Invariant it enforces |
|---|---|---|
| `FeltEntropy` | Any non-zero felt for `SetViewingKey`, `OpenChannel`, `OpenSubchannel`, and `CreateOpenNote` entropy/salt fields ([`sdk/rs/src/actions.rs:91-120`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/actions.rs#L91-L120)). | Prevents accidentally feeding a 120-bit note salt into a full-felt Cairo field; constraint #5 and F2 document the upstream mismatch ([`CLAUDE.md:28-30`](https://github.com/PoulavBhowmick03/Erebus/blob/main/CLAUDE.md#L28-L30), [`docs/friction.md:258-276`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L258-L276)). |
| `NoteSalt` | Strictly `1 < salt < 2^120`; `0` means absent and `1` is reserved for open notes ([`sdk/rs/src/actions.rs:58-89`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/actions.rs#L58-L89)). | Makes contract range validity a constructor property. Structured wire chunks are `NoteSalt`s with bit 119 pinned ([`sdk/rs/src/wire.rs:9-17`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L9-L17)). |
| `RandomSalt` | Wraps a valid `NoteSalt` derived from caller-supplied CSPRNG bytes; it is accepted only by value-note constructors ([`sdk/rs/src/actions.rs:122-162`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/actions.rs#L122-L162), [`sdk/rs/src/channel.rs:502-512`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L502-L512)). | Makes it impossible to pass a structured wire salt to a value-bearing note without breaking the type boundary. |

The bug prevented by the split is mask reuse/predictability. The amount cipher is additive and
its mask depends on `(channel_key, token, index, salt)`; the code’s own comment warns that
using a structured/predictable salt on value notes can let an observer compare ciphertexts,
whereas structured salts are confined to zero-amount notes ([`sdk/rs/src/actions.rs:122-132`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/actions.rs#L122-L132),
[`sdk/rs/src/channel.rs:414-421`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L414-L421), [`sdk/rs/src/decrypt.rs:115-137`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/decrypt.rs#L115-L137)). `Channel::data_note` hardcodes
amount zero, while `Channel::value_note` requires `RandomSalt`
([`sdk/rs/src/channel.rs:490-512`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L490-L512)). This is a type-level confidentiality boundary, not merely a
range check.

### 2.3 Negotiation wire: exact layout and what v2 hides

The canonical plaintext is exactly:

```text
MSB                                                                 LSB
type:8 | reply_to:32 | created_at:40 | amount:128 | deadline:64 | memo_hash:128
                              400 bits / 50 bytes
```

Fields are pushed most-significant-first; `None` uses `u32::MAX`, which is therefore forbidden
as a real `reply_to` ([`sdk/rs/src/wire.rs:63-88`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L63-L88), [`sdk/rs/src/wire.rs:327-370`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L327-L370)). `created_at`
must fit 40 bits; amount and memo already occupy full `u128`; deadline occupies 64 bits
([`sdk/rs/src/wire.rs:320-345`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L320-L345)). The Rust API accepts only a 128-bit memo hash, while its helper
for a felt intentionally keeps the low 128 bits ([`sdk/rs/src/wire.rs:231-242`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L231-L242)). This differs
from the repository TypeScript v1 helper, which silently masks any larger bigint
([`sdk/ts/src/channel/wire.ts:119-155`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/ts/src/channel/wire.ts#L119-L155)); Rust’s public CLI parses a `u128`, so oversized input is
rejected rather than silently truncated ([`sdk/rs/src/bin/erebus_cli.rs:324-334`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/bin/erebus_cli.rs#L324-L334)). That is the
F19 hardening ([`docs/friction.md:706-732`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L706-L732)).

Wire v1 places the 400 plaintext bits directly into four 119-bit payload chunks, pins bit 119
of every salt, and uses notes `4k..4k+3`; it remains readable but new writes return
`LegacyReadOnly` ([`sdk/rs/src/wire.rs:501-534`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L501-L534), [`sdk/rs/src/channel.rs:422-438`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L422-L438)). Wire v2 first
encrypts the 50 plaintext bytes with AES-256-GCM-SIV and appends a 16-byte tag; an unencrypted
one-byte marker makes a 67-byte/536-bit envelope, placed least-significant chunk first across
five 119-bit chunks with 59 zero padding bits ([`sdk/rs/src/wire.rs:29-35`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L29-L35),
[`sdk/rs/src/wire.rs:78-95`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L78-L95), [`sdk/rs/src/wire.rs:418-453`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L418-L453)).

V2 derives a 32-byte key and 12-byte nonce with HKDF-SHA-256 from the directional channel key.
The HKDF salt is `EREBUS_WIRE_V2_HKDF_SHA256`; key info is
`EREBUS_WIRE_V2_KEY || chain_id || pool_address || token`; nonce info is
`EREBUS_WIRE_V2_NONCE || same_scope || message_index_be` ([`sdk/rs/src/wire.rs:383-407`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L383-L407)). AAD is
`EREBUS_WIRE_V2_AAD || chain_id || pool_address || token || message_index_be`
([`sdk/rs/src/wire.rs:409-415`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L409-L415)). Thus content is encrypted and authenticated, and copying it
across any authenticated context fails ([`sdk/rs/tests/wire_codec.rs:151-225`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/wire_codec.rs#L151-L225)). AES-GCM-SIV was
chosen because a failed, not-yet-included write may retry different terms at the same free
index; ordinary nonce-sensitive AEAD would make that operational retry catastrophic
([`docs/friction.md:1086-1108`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L1086-L1108)).

V2 does **not** hide that five note creations occurred, their transaction sender, their time,
or their five-note cadence. Worse, required-zero padding gives the fifth salt a 59-bit fixed
shape that the non-ignored fingerprint test detects ([`sdk/rs/tests/wire_v2_fingerprint.rs:31-58`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/wire_v2_fingerprint.rs#L31-L58);
[`docs/friction.md:990-1015`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L990-L1015)). It also does not hide the salt values themselves: salt is the
public high 120 bits of every stored packed note and appears in client action calldata
([`sdk/rs/src/decrypt.rs:103-112`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/decrypt.rs#L103-L112), [`sdk/rs/src/actions.rs:203-218`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/actions.rs#L203-L218)). So v2 provides content
confidentiality/authentication, not traffic-flow or sender-account privacy.

## 3. Port ledger: what was rewritten and why

| Upstream TS/Cairo function or primitive | Rust equivalent | Why rewritten rather than called; parity/divergence |
|---|---|---|
| Cairo `compute_*` functions in [`hashes.cairo`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/hashes.cairo) | `hashes::{compute_channel_key, …}` ([`sdk/rs/src/hashes.rs:74-263`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L74-L263)) | Rust needs them in-process for construction and keyed discovery. Cairo is the KAT oracle; upstream `discovery-core` duplicates many but brings a conflicting git-fork dependency graph ([`sdk/rs/src/decrypt.rs:6-19`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/decrypt.rs#L6-L19)). Translation, except Rust preserves exact heterogeneous salt types. |
| Cairo additive encryption/decryption formulas and upstream `discovery-core` decryption | `decrypt::{unpack_note,note_amount,packed_value,channel_info,subchannel_token,outgoing_recipient_addr}` ([`sdk/rs/src/decrypt.rs:103-200`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/decrypt.rs#L103-L200)) | Needed in-process without importing the forked provider stack. Same Cairo fixture is the oracle ([`sdk/rs/tests/decrypt_conformance.rs:1-11`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/decrypt_conformance.rs#L1-L11)). |
| Cairo `ClientAction` enum and TS `serializeClientActions` | Rust input structs, `ClientAction`, `serialize_actions` ([`sdk/rs/src/actions.rs:164-434`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/actions.rs#L164-L434)) | Required for a Node-free Rust write path. The TS SDK supplies byte-for-byte Serde fixtures because Cairo emits no direct vector ([`sdk/rs/tests/clientaction_serde.rs:1-11`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/clientaction_serde.rs#L1-L11)). |
| Cairo phase/replay checks in `main`/`assert_and_advance_phase` | `ActionSetBuilder` ([`sdk/rs/src/action_set.rs:121-178`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/action_set.rs#L121-L178)) | Deliberate type/construction hardening: fail before proving rather than after. Token balance is intentionally still left to Cairo because the builder lacks consumed amounts ([`sdk/rs/src/action_set.rs:24-28`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/action_set.rs#L24-L28)). |
| Cairo contiguous/write-once note rules | `SubchannelCursor` ([`sdk/rs/src/subchannel.rs:82-164`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/subchannel.rs#L82-L164)) | Erebus-specific allocator absent upstream; makes caller-side gap/reuse errors unrepresentable during a single process. It remains only a local belief and must be reseated from chain ([`sdk/rs/src/subchannel.rs:27-32`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/subchannel.rs#L27-L32)). |
| Upstream `ProofInvocationFactory.create` and `compileExecuteCalldata` | `calldata::compile_actions`, `proof_execute`, `execution::build_proof_invocation` ([`sdk/rs/src/calldata.rs:25-53`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/calldata.rs#L25-L53), [`sdk/rs/src/execution.rs:268-299`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L268-L299)) | Required in-process with no Node runtime and needed to start KAT composition from `ActionSet`. The end-to-end fixture is captured from upstream factory ([`sdk/rs/tests/proof_invocation.rs:1-13`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/proof_invocation.rs#L1-L13)). |
| starknet.js invoke-v3 hash/signature | `InvokeV3::transaction_hash`, `signing` ([`sdk/rs/src/tx.rs:156-193`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/tx.rs#L156-L193), [`sdk/rs/src/signing.rs:47-84`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/signing.rs#L47-L84)) | Rust signs locally and cannot call JS. It also must support the privacy-specific non-empty `proof_facts` hash term that a generic transaction model may omit ([`sdk/rs/src/tx.rs:16-21`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/tx.rs#L16-L21)). Fixtures pin both libraries ([`sdk/rs/tests/invoke_v3_txhash.rs:1-11`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/invoke_v3_txhash.rs#L1-L11), [`sdk/rs/tests/ecdsa.rs:1-9`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/ecdsa.rs#L1-L9)). |
| Upstream `ProvingService.proveTransaction` | `ProvingService::prove_transaction` ([`sdk/rs/src/prover.rs:142-220`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/prover.rs#L142-L220)) | Node-free async HTTP, typed response, and bounded retry policy. It preserves the upstream JSON-RPC method/shape, not protocol behavior invented by Erebus ([`../starknet-privacy/sdk/src/internal/proving-service.ts:120-290`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/sdk/src/internal/proving-service.ts#L120-L290)). |
| Upstream `PrivateTransfers.buildExecuteResult` screening suffix and output slicing | `calldata::screening_suffix`, `execution::server_actions` ([`sdk/rs/src/calldata.rs:55-82`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/calldata.rs#L55-L82), [`sdk/rs/src/execution.rs:323-343`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L323-L343)) | Needed for direct Rust submission. It mirrors stripping the class-hash prefix and appending `Option<ScreeningAttestation>` ([`../starknet-privacy/sdk/src/internal/private-transfers.ts:102-136`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/sdk/src/internal/private-transfers.ts#L102-L136)). |
| Starknet provider/account submission | Minimal `StarknetRpc` plus Rust `SignedInvokeV3` wire ([`sdk/rs/src/rpc.rs:1-8`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/rpc.rs#L1-L8), [`sdk/rs/src/rpc.rs:24-165`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/rpc.rs#L24-L165)) | A full account SDK would not remove the custom proof-facts hash and would introduce a second transaction model ([`sdk/rs/src/rpc.rs:1-5`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/rpc.rs#L1-L5)). This is a narrow partial client, not a provider replacement. |
| Local TS wire-v1 pack/unpack | Rust `encode_legacy_message`/`decode_legacy_message` ([`sdk/rs/src/wire.rs:501-534`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L501-L534)) | Differential oracle for Erebus’s original format, retained read-only. Constants/salts/note indices match TS fixtures ([`sdk/rs/tests/wire_codec.rs:1-10`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/wire_codec.rs#L1-L10), [`sdk/rs/tests/wire_codec.rs:102-134`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/wire_codec.rs#L102-L134)). |
| No upstream counterpart | Wire v2 ([`sdk/rs/src/wire.rs:383-499`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L383-L499)) | Erebus-specific authenticated encryption and migration behavior. It currently has a Rust KAT/round-trip/tamper suite but no second implementation ([`sdk/rs/tests/wire_codec.rs:6-10`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/wire_codec.rs#L6-L10)). |
| No upstream counterpart | `Channel`, `OfferBook`, `ViewingGrant`, `StateStore`, high-level `Client` | These define negotiation semantics, atomic composition, selective disclosure, persistence, and the application API ([`sdk/rs/src/channel.rs:164-253`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L164-L253), [`sdk/rs/src/negotiation.rs:147-302`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/negotiation.rs#L147-L302), [`sdk/rs/src/disclosure.rs:45-270`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L45-L270), [`sdk/rs/src/state.rs:174-446`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L174-L446), [`sdk/rs/src/client.rs:538-935`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L538-L935)). Category: original Erebus protocol behavior. |
| Local TS static-static ECDH | No active Rust equivalent | It is planned off-chain transport crypto, while current v2 derives from the on-chain directional channel key ([`sdk/ts/src/crypto/channel-secret.ts:1-29`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/ts/src/crypto/channel-secret.ts#L1-L29), [`sdk/rs/src/wire.rs:383-407`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L383-L407)). This is an unported/unused path, not missing parity in the active protocol. |

### Known behavioral differences, not translations

1. Rust rejects oversized `memo_hash` at its typed/CLI boundary, while TypeScript v1 masks to
   the low 128 bits. This hardening means callers must truncate before
   crossing the Rust API ([`sdk/rs/src/bin/erebus_cli.rs:324-334`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/bin/erebus_cli.rs#L324-L334),
   [`sdk/ts/src/channel/wire.ts:119-155`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/ts/src/channel/wire.ts#L119-L155), [`docs/friction.md:706-732`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L706-L732)).
2. Rust refuses new wire-v1 writes, while TypeScript still exposes its v1 encoder; this is a
   confidentiality migration ([`sdk/rs/src/channel.rs:428-430`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L428-L430),
   [`sdk/ts/src/channel/wire.ts:196-219`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/ts/src/channel/wire.ts#L196-L219)).
3. Rust adds `ActionSetBuilder`, `RandomSalt`, cursor, exact-amount check, token checks, and
   pool-invocation newtypes beyond TS/Cairo serialization. These are client hardenings, not
   alternate Cairo semantics ([`sdk/rs/src/action_set.rs:1-28`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/action_set.rs#L1-L28), [`sdk/rs/src/actions.rs:122-162`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/actions.rs#L122-L162),
   [`sdk/rs/src/channel.rs:545-555`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L545-L555), [`sdk/rs/src/client.rs:1306-1336`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L1306-L1336), [`sdk/rs/src/tx.rs:221-250`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/tx.rs#L221-L250)).
4. Rust tx hashing appends a hash of `proof_facts` only when non-empty. The fixture covers both
   branches; this is a privacy-stack extension that must agree with the deployed RPC/prover,
   not standard starknet.js behavior to assume universally ([`sdk/rs/src/tx.rs:172-193`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/tx.rs#L172-L193),
   [`sdk/rs/tests/invoke_v3_txhash.rs:129-161`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/invoke_v3_txhash.rs#L129-L161)).
5. The Rust grant returns a self-contained bearer package, correcting the older TS interface’s
   `void` return and local-handle-dependent reveal shape ([`sdk/rs/src/client.rs:565-572`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L565-L572),
   [`sdk/ts/src/interface.ts:151-172`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/ts/src/interface.ts#L151-L172), [`docs/friction.md:922-936`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L922-L936)).

### Why `/sdk/ts` still exists

It is a private, non-shipping oracle package ([`sdk/ts/package.json:1-5`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/ts/package.json#L1-L5), [`README.md:84-90`](https://github.com/PoulavBhowmick03/Erebus/blob/main/README.md#L84-L90)). It
generates frozen wire-v1 salts and exercises upstream Mocknet behavior
([`sdk/ts/tests/gen-wire-vectors.test.ts:1-25`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/ts/tests/gen-wire-vectors.test.ts#L1-L25), [`sdk/ts/tests/pool-flow.test.ts:1-12`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/ts/tests/pool-flow.test.ts#L1-L12)). Rust then
compares the frozen [`ts-wire-salts.json`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/fixtures/ts-wire-salts.json) and [`ts-clientaction-serde.json`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/fixtures/ts-clientaction-serde.json) byte-for-byte
([`sdk/rs/tests/wire_codec.rs:1-10`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/wire_codec.rs#L1-L10), [`sdk/rs/tests/clientaction_serde.rs:1-11`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/clientaction_serde.rs#L1-L11)). Agreement proves
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
`{ok:false,error}` payload ([`mcp-server/src/erebus_mcp/tools.py:89-168`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L89-L168),
[`mcp-server/src/erebus_mcp/tools.py:170-273`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L170-L273)). The production MCP server constructs one
identity-bound seam from environment configuration. The default backend is `mock`,
not chain, unless `EREBUS_BACKEND=seam` selects Rust ([`mcp-server/src/server.py:42-76`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/server.py#L42-L76)). No key
value is passed to a tool; only configured file paths reach the seam
([`mcp-server/src/server.py:46-66`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/server.py#L46-L66)).

The repository’s reference [`agents/`](https://github.com/PoulavBhowmick03/Erebus/tree/main/agents) demo does **not** traverse MCP or Rust: it directly uses
`MockErebusClient` and says so ([`agents/src/erebus_agents/agent.py:1-7`](https://github.com/PoulavBhowmick03/Erebus/blob/main/agents/src/erebus_agents/agent.py#L1-L7),
[`agents/src/erebus_agents/agent.py:27-45`](https://github.com/PoulavBhowmick03/Erebus/blob/main/agents/src/erebus_agents/agent.py#L27-L45)). That distinction matters when answering “what did
the agent demo validate?” It validates policy/mock behavior, not this transport chain.

**MCP server → [`sdk/py`](https://github.com/PoulavBhowmick03/Erebus/tree/main/sdk/py).** `SeamErebusClient` reshapes Python dataclasses into seam dictionaries
and offloads each blocking child process with `asyncio.to_thread`, keeping the MCP event loop
responsive ([`mcp-server/src/erebus_mcp/seam_client.py:1-17`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L1-L17),
[`mcp-server/src/erebus_mcp/seam_client.py:94-109`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L94-L109)). Therefore the premise “async is confined to
Rust” is false: Python uses async for server concurrency, while Rust owns asynchronous protocol
I/O and receipt/prover waiting. Python performs no hashes, felt arithmetic, salt encoding, or
proof logic ([`mcp-server/src/erebus_mcp/seam_client.py:1-12`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L1-L12)).

**[`sdk/py`](https://github.com/PoulavBhowmick03/Erebus/tree/main/sdk/py) → CLI.** The seam builds exactly one JSON object, runs `erebus-cli`, writes the JSON
to stdin, captures stdout, and requires one JSON response envelope
([`sdk/py/src/erebus/_seam.py:120-165`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/py/src/erebus/_seam.py#L120-L165)). Every configured call sends nine fields: RPC URL,
prover URL, pool/chain/account, two key-file paths, state directory, and token
([`sdk/py/src/erebus/_seam.py:59-92`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/py/src/erebus/_seam.py#L59-L92), [`sdk/py/src/erebus/_seam.py:167-173`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/py/src/erebus/_seam.py#L167-L173)). Private key values do
not enter Python; the CLI opens their paths ([`sdk/py/src/erebus/_seam.py:10-18`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/py/src/erebus/_seam.py#L10-L18)).

Protocol 1 was the earlier seam documented in stale prose. Current protocol 2 adds one-shot
configuration on every call, opaque state handles, `balance`, key generation, and structured
responses; `version` returns `protocol: 2` ([`sdk/rs/src/bin/erebus_cli.rs:27-82`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/bin/erebus_cli.rs#L27-L82),
[`sdk/rs/src/bin/erebus_cli.rs:202-306`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/bin/erebus_cli.rs#L202-L306)). The CLI’s Tokio main reads stdin to EOF, deserializes
one request, awaits one dispatch, prints one envelope, and exits nonzero on failure
([`sdk/rs/src/bin/erebus_cli.rs:429-450`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/bin/erebus_cli.rs#L429-L450)).

**CLI → Rust state/key boundary.** `generate_pool_key` creates an absolute-path, non-overwriting
0600 file from OS entropy and returns only its path and public key ([`sdk/rs/src/keys.rs:24-80`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/keys.rs#L24-L80)).
Channel handles are `ch_` plus 64 lowercase hex characters and are validated before becoming
path components ([`sdk/rs/src/state.rs:26-66`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L26-L66)). The state directory is mode 0700 and lock,
temporary, and record files are mode 0600 on Unix ([`sdk/rs/src/state.rs:180-189`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L180-L189),
[`sdk/rs/src/state.rs:217-245`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L217-L245), [`sdk/rs/src/state.rs:380-413`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L380-L413), [`sdk/rs/src/state.rs:499-528`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L499-L528)).
The non-Unix mode helpers are no-ops, so the 0600/0700 statement is Unix-specific
([`sdk/rs/src/state.rs:499-528`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L499-L528), [`sdk/rs/src/keys.rs:83-90`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/keys.rs#L83-L90)).

**Rust → RPC/prover/Starknet.** Read/discovery calls send only public entrypoint arguments and
secret-derived IDs, but `starknet_call(compile_actions)` sends the pool private key in calldata
to the RPC ([`sdk/rs/src/rpc.rs:1-8`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/rpc.rs#L1-L8), [`sdk/rs/src/calldata.rs:25-36`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/calldata.rs#L25-L36)). The virtual proof
invocation also sends that key in clear at `calldata[5]` to the prover
([`sdk/rs/src/prover.rs:3-14`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/prover.rs#L3-L14), [`sdk/rs/tests/proof_invocation.rs:129-151`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/proof_invocation.rs#L129-L151)). The final network
transaction is signed by the Starknet account key and calls `apply_actions`; the pool private
key is not in that final call, but proof facts and the proof blob are
([`sdk/rs/src/execution.rs:192-231`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L192-L231)). Thus both preflight RPC and prover are inside the pool-key
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
setup action set ([`sdk/rs/src/client.rs:575-622`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L575-L622)). Setup optionally contains `SetViewingKey`,
then `OpenChannel`, then `OpenSubchannel`; that is account→channel→subchannel phase order
([`sdk/rs/src/channel.rs:298-326`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L298-L326)). Cairo registration publishes the pool public key and
encrypts the pool private key to the configured auditor; opening a channel writes encrypted
channel info, a channel marker, and an encrypted outgoing record; opening a subchannel writes
encrypted token info and its marker ([`../starknet-privacy/packages/privacy/src/privacy.cairo:317-354`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/privacy.cairo#L317-L354),
[`../starknet-privacy/packages/privacy/src/privacy.cairo:357-428`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/privacy.cairo#L357-L428),
[`../starknet-privacy/packages/privacy/src/privacy.cairo:431-470`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/privacy.cairo#L431-L470)). After an accepted receipt,
the client creates the opaque local record; a crash after inclusion but before `state.create`
can therefore orphan the local handle ([`sdk/rs/src/client.rs:623-644`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L623-L644), [`sdk/rs/README.md:113-116`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/README.md#L113-L116)).

**2. Offer.** The client validates token/terms/state, holds the state lease, waits until the
last write is visible to the proving anchor, reconstructs the chain transcript, reseats a
cursor at the first empty outgoing note, and builds an `Offer` message
([`sdk/rs/src/client.rs:648-687`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L648-L687)). `Channel::write_message` encrypts it into five salts and
creates five consecutive `CreateEncNote` actions with amount zero
([`sdk/rs/src/channel.rs:414-459`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L414-L459), [`sdk/rs/src/channel.rs:490-500`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L490-L500)). Only after receipt does it
advance/persist the cursor and block ([`sdk/rs/src/client.rs:688-695`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L688-L695)).

**3. Counter.** The client attaches the reverse directional channel, verifies that `reply_to`
names a counterparty offer/counter, writes a `Counter` whose `reply_to` is the opposite
direction’s note-grid message index, then executes/commits exactly as for an offer
([`sdk/rs/src/client.rs:698-764`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L698-L764)). Direction is part of `OfferId` because note indices collide
across the two independent subchannels ([`sdk/rs/src/negotiation.rs:95-139`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/negotiation.rs#L95-L139),
[`sdk/rs/src/negotiation.rs:163-187`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/negotiation.rs#L163-L187)). Deadlines and reply validity are client semantics; Cairo
has no offer/deadline/status concept ([`sdk/rs/tests/negotiation_state.rs:1-6`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/negotiation_state.rs#L1-L6)).

**4. `accept_and_settle`.** The payer checks the counterparty offer is live, discovers all
unspent value notes at the same mature block, and selects an exact subset; there is no change
note ([`sdk/rs/src/client.rs:789-831`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L789-L831), [`sdk/rs/src/client.rs:1088-1117`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L1088-L1117)). It constructs an
`Accept` copying amount/deadline/memo and calls `settle_next`
([`sdk/rs/src/client.rs:833-860`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L833-L860)). The action set places every `UseNote` in phase 4, then the
five zero-value acceptance notes and one random-salted payment note in phase 5, sorted by note
index ([`sdk/rs/src/channel.rs:577-610`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L577-L610)). The record occupies `5k..5k+4`; payment is `5k+5`,
leaving the cursor off-grid and making the current subchannel one-deal-only
([`sdk/rs/src/channel.rs:613-653`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L613-L653)). The client checks recorded and paid amounts match before
construction ([`sdk/rs/src/channel.rs:545-555`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L545-L555)).

**5. Every write’s execution pipeline.** `Executor::execute` selects an older proving block,
calls the pool view `compile_actions`, builds/signs the pool-as-account virtual invocation,
asks `starknet_proveTransaction`, extracts the unique pool L2→L1 message, and rejects it if
its serialized server actions differ from the preflight ([`sdk/rs/src/execution.rs:143-182`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L143-L182)). It
then checks proof age, appends screening data, wraps `apply_actions` in the operator account’s
single-call calldata, estimates the proof-carrying transaction, signs/submits it, and waits for
an accepted or reverted receipt ([`sdk/rs/src/execution.rs:184-264`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L184-L264)). The prover receives the
pool virtual invoke and returns proof, proof facts, L2→L1 messages, and optional screening
signature ([`sdk/rs/src/prover.rs:92-140`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/prover.rs#L92-L140), [`sdk/rs/src/prover.rs:185-220`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/prover.rs#L185-L220)). The final
`apply_actions` calldata is serialized server actions followed by Cairo `Option` screening;
the proof blob/proof facts live on the v3 transaction envelope ([`sdk/rs/src/calldata.rs:55-82`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/calldata.rs#L55-L82),
[`sdk/rs/src/tx.rs:264-350`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/tx.rs#L264-L350)).

**Why this SDK never submits `__execute__` to the chain.** It only builds `proof_execute` for
the transaction sent to the proving service ([`sdk/rs/src/calldata.rs:50-53`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/calldata.rs#L50-L53),
[`sdk/rs/src/execution.rs:268-299`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L268-L299)); chain submission always wraps `apply_actions`
([`sdk/rs/src/execution.rs:192-231`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L192-L231)). The Cairo `__execute__` compiles actions and sends server
actions as an L2→L1 message but does not call the server-side storage application path
([`../starknet-privacy/packages/privacy/src/privacy.cairo:193-212`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/privacy.cairo#L193-L212)). A normal contract call would
fail `assert_valid_os_call` because caller must be zero and tx version v3; more importantly,
submitting the virtual account transaction would publish the pool secret and would not execute
the proof-validated `apply_actions` transition ([`../starknet-privacy/packages/privacy/src/utils.cairo:561-576`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/utils.cairo#L561-L576),
[`../starknet-privacy/packages/privacy/src/privacy.cairo:782-839`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/privacy.cairo#L782-L839)). Calling it “local simulation
only” is shorthand: it is executed by the prover’s virtual Starknet OS, not by this SDK as the
real state-changing transaction.

**Phase order.** Cairo maps deposit to phase 3, note use to 4, note creation to 5, withdrawal
to 6, and invoke to 7; it rejects decreasing phases and a second/post-invoke action
([`../starknet-privacy/packages/privacy/src/actions.cairo:275-315`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/actions.cairo#L275-L315)). `ActionSetBuilder::push`
rejects phase regression and multiple invokes, while `build` rejects empty or
non-replay-protected sets ([`sdk/rs/src/action_set.rs:121-178`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/action_set.rs#L121-L178)). It does not model per-token
balance; Cairo remains the authority for that runtime invariant
([`sdk/rs/src/action_set.rs:24-28`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/action_set.rs#L24-L28)).

**6. Grant/reveal.** Granting is local: after attaching the reverse channel, it exports both
directional keys plus chain, pool, wire version, token, participants, and a checksum; no
transaction is sent ([`sdk/rs/src/client.rs:878-916`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L878-L916), [`sdk/rs/src/disclosure.rs:45-88`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L45-L88)). Reveal
validates config scope, derives readers from the grant, fetches only computed note IDs, and
reconstructs locally ([`sdk/rs/src/client.rs:918-935`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L918-L935), [`sdk/rs/src/disclosure.rs:234-270`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L234-L270)).

### How notes are found and why indices cannot have gaps

Discovery is deterministic nested enumeration: recipient channel count → decrypt each channel
info → derive sequential subchannel IDs until the first empty → decrypt token → derive
sequential note IDs until the first empty → decrypt amount → derive nullifier and query
spentness ([`sdk/rs/src/client.rs:445-521`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L445-L521)). For known negotiation channels, `fetch_notes` starts
from the channel key and stops at the first zero `get_note` result
([`sdk/rs/src/client.rs:355-372`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L355-L372)). Events cannot substitute for this because note addresses are
secret-derived and the intended interface is exact keyed lookup; the repository explicitly
forbids world scanning ([`CLAUDE.md:23-26`](https://github.com/PoulavBhowmick03/Erebus/blob/main/CLAUDE.md#L23-L26)).

Cairo enforces that note `n-1` exists before creating note `n`, and `WriteOnce` prevents reuse
([`../starknet-privacy/packages/privacy/src/privacy.cairo:736-751`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/privacy.cairo#L736-L751),
[`../starknet-privacy/packages/privacy/src/privacy.cairo:932-946`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/privacy.cairo#L932-L946)). Therefore the first empty
slot is both the end of discovery and the only next legal write. `next_free_note_index` and
`SubchannelCursor` encode exactly that rule ([`sdk/rs/src/client.rs:269-290`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L269-L290),
[`sdk/rs/src/subchannel.rs:97-162`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/subchannel.rs#L97-L162)). A gap is not just inefficient: readers stop there and
everything beyond it becomes unreachable through this discovery algorithm.

## 5. How correctness is established

### Known-answer fixtures

| Fixture | Oracle and what agreement proves |
|---|---|
| [`cairo-reference-data.json`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/fixtures/cairo-reference-data.json) | Inputs/outputs emitted from upstream Cairo derivations; pins every Poseidon preimage and read-side encrypted value ([`sdk/rs/tests/fixtures/cairo-reference-data.json:2-26`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/fixtures/cairo-reference-data.json#L2-L26), [`sdk/rs/tests/cairo_conformance.rs:1-13`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/cairo_conformance.rs#L1-L13), [`sdk/rs/tests/decrypt_conformance.rs:1-11`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/decrypt_conformance.rs#L1-L11)). |
| [`ts-wire-salts.json`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/fixtures/ts-wire-salts.json) | Generated by the independent local TypeScript v1 codec; pins constants, note indices, and exact four salts for representative messages ([`sdk/rs/tests/fixtures/ts-wire-salts.json:1-42`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/fixtures/ts-wire-salts.json#L1-L42), [`sdk/rs/tests/wire_codec.rs:1-10`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/wire_codec.rs#L1-L10)). It proves v1 compatibility only. |
| [`ts-clientaction-serde.json`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/fixtures/ts-clientaction-serde.json) | Generated through upstream `serializeClientActions` plus Starknet `CallData.compile`; pins all ten variant indices/field orders/felts ([`sdk/rs/tests/fixtures/ts-clientaction-serde.json:1-70`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/fixtures/ts-clientaction-serde.json#L1-L70), [`sdk/rs/tests/clientaction_serde.rs:1-11`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/clientaction_serde.rs#L1-L11)). |
| [`starknetjs-ecdsa.json`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/fixtures/starknetjs-ecdsa.json) | starknet.js keys/messages/signatures; pins public keys, cross-verification, and deterministic byte equality ([`sdk/rs/tests/fixtures/starknetjs-ecdsa.json:1-29`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/fixtures/starknetjs-ecdsa.json#L1-L29), [`sdk/rs/tests/ecdsa.rs:1-9`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/ecdsa.rs#L1-L9)). |
| [`starknetjs-invoke-v3-txhash.json`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/fixtures/starknetjs-invoke-v3-txhash.json) | `hash.calculateInvokeTransactionHash` vectors with and without proof facts and nontrivial bounds; pins transaction-hash composition ([`sdk/rs/tests/fixtures/starknetjs-invoke-v3-txhash.json:1-44`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/fixtures/starknetjs-invoke-v3-txhash.json#L1-L44), [`sdk/rs/tests/invoke_v3_txhash.rs:1-11`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/invoke_v3_txhash.rs#L1-L11)). |
| [`sdk-proof-invocation.json`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/fixtures/sdk-proof-invocation.json) | Captured upstream `ProofInvocationFactory` result; pins composition from `ActionSet` through `__execute__` calldata, v3 hash, signature, and wire transaction ([`sdk/rs/tests/fixtures/sdk-proof-invocation.json:1-38`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/fixtures/sdk-proof-invocation.json#L1-L38), [`sdk/rs/tests/proof_invocation.rs:1-13`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/proof_invocation.rs#L1-L13)). |

### Integration-test map

| Test file | What it would catch |
|---|---|
| [`action_set.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/action_set.rs) | Phase regression, multiple invoke, missing replay protection, wrong span shape, or nonzero proof-invocation prices/tip ([`sdk/rs/tests/action_set.rs:1-15`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/action_set.rs#L1-L15), [`sdk/rs/tests/action_set.rs:81-259`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/action_set.rs#L81-L259)). |
| [`cairo_conformance.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/cairo_conformance.rs) | Any tag/preimage/order/salt-type divergence from Cairo ([`sdk/rs/tests/cairo_conformance.rs:1-13`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/cairo_conformance.rs#L1-L13), [`sdk/rs/tests/cairo_conformance.rs:69-226`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/cairo_conformance.rs#L69-L226)). |
| [`decrypt_conformance.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/decrypt_conformance.rs) | Incorrect pack halves, wrapping subtraction, ECDH recovery, or unauthenticated-wrong-key assumptions ([`sdk/rs/tests/decrypt_conformance.rs:1-11`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/decrypt_conformance.rs#L1-L11), [`sdk/rs/tests/decrypt_conformance.rs:62-234`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/decrypt_conformance.rs#L62-L234)). |
| [`clientaction_serde.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/clientaction_serde.rs) | Wrong Cairo enum index, field order, span prefix, phase map, or salt bounds ([`sdk/rs/tests/clientaction_serde.rs:1-11`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/clientaction_serde.rs#L1-L11), [`sdk/rs/tests/clientaction_serde.rs:104-207`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/clientaction_serde.rs#L104-L207)). |
| [`channel_ops.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/channel_ops.rs) | Correct primitives wired to wrong key, party, token, index, salt, or zero amount ([`sdk/rs/tests/channel_ops.rs:1-6`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/channel_ops.rs#L1-L6), [`sdk/rs/tests/channel_ops.rs:55-235`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/channel_ops.rs#L55-L235)). |
| [`setup.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/setup.rs) | Incorrect register/channel/subchannel composition, shield balance/replay structure, or top-up reopening ([`sdk/rs/tests/setup.rs:1-6`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/setup.rs#L1-L6), [`sdk/rs/tests/setup.rs:59-310`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/setup.rs#L59-L310)). |
| [`settlement.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/settlement.rs) | Acceptance/payment split, create-before-spend, missing inputs, amount mismatch, salt-lane mix-up, or index collision ([`sdk/rs/tests/settlement.rs:1-6`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/settlement.rs#L1-L6), [`sdk/rs/tests/settlement.rs:90-354`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/settlement.rs#L90-L354)). |
| [`index_contiguity.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/index_contiguity.rs) | Gaps, overwrite, failed-reservation cursor burns, off-grid messages, or illegal post-settlement message ([`sdk/rs/tests/index_contiguity.rs:1-10`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/index_contiguity.rs#L1-L10), [`sdk/rs/tests/index_contiguity.rs:85-261`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/index_contiguity.rs#L85-L261)). |
| [`wire_codec.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/wire_codec.rs) | V1 compatibility drift and v2 round-trip, KAT, tamper, context, padding, retry, flag, range, or redaction failure ([`sdk/rs/tests/wire_codec.rs:1-10`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/wire_codec.rs#L1-L10), [`sdk/rs/tests/wire_codec.rs:102-367`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/wire_codec.rs#L102-L367)). |
| [`wire_v2_fingerprint.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/wire_v2_fingerprint.rs) | Detects today’s fifth-salt traffic fingerprint; the desired indistinguishability property remains ignored ([`sdk/rs/tests/wire_v2_fingerprint.rs:1-5`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/wire_v2_fingerprint.rs#L1-L5), [`sdk/rs/tests/wire_v2_fingerprint.rs:31-75`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/wire_v2_fingerprint.rs#L31-L75)). |
| [`read_path.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/read_path.rs) | Writer/reader slot drift, torn messages, v1 migration loss, wrong-key behavior, settlement-note placement, or direction/author reconstruction errors ([`sdk/rs/tests/read_path.rs:1-7`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/read_path.rs#L1-L7), [`sdk/rs/tests/read_path.rs:122-407`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/read_path.rs#L122-L407)). |
| [`negotiation_state.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/negotiation_state.rs) | Expiry boundary, own/unknown/non-offer acceptance, dangling replies, countered-offer semantics, and repeat settlement ([`sdk/rs/tests/negotiation_state.rs:1-6`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/negotiation_state.rs#L1-L6), [`sdk/rs/tests/negotiation_state.rs:61-191`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/negotiation_state.rs#L61-L191)). |
| [`disclosure.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/disclosure.rs) | Incomplete reconstruction, wrong attribution/payment comparison, cross-token/counterparty leakage, half grant, serialization corruption, or spending-key leakage ([`sdk/rs/tests/disclosure.rs:1-9`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/disclosure.rs#L1-L9), [`sdk/rs/tests/disclosure.rs:181-495`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/disclosure.rs#L181-L495)). |
| [`invoke_v3_txhash.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/invoke_v3_txhash.rs) | Divergence from starknet.js, especially conditional proof-facts and resource-bound packing ([`sdk/rs/tests/invoke_v3_txhash.rs:1-11`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/invoke_v3_txhash.rs#L1-L11), [`sdk/rs/tests/invoke_v3_txhash.rs:114-175`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/invoke_v3_txhash.rs#L114-L175)). |
| [`ecdsa.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/ecdsa.rs) | Public-key/signature incompatibility or nondeterminism ([`sdk/rs/tests/ecdsa.rs:1-9`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/ecdsa.rs#L1-L9), [`sdk/rs/tests/ecdsa.rs:38-125`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/ecdsa.rs#L38-L125)). |
| [`proof_invocation.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/proof_invocation.rs) | Locally correct components composed into an upstream-incompatible proof request, and accidental denial of the clear-key exposure ([`sdk/rs/tests/proof_invocation.rs:1-13`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/proof_invocation.rs#L1-L13), [`sdk/rs/tests/proof_invocation.rs:97-151`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/proof_invocation.rs#L97-L151)). |
| [`execution_pipeline.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/execution_pipeline.rs) | One in-process preflight→prove→compare→estimate→sign→submit→receipt transport path; it explicitly uses deterministic local JSON-RPC servers, not Sepolia ([`sdk/rs/tests/execution_pipeline.rs:1-5`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/execution_pipeline.rs#L1-L5), [`sdk/rs/tests/execution_pipeline.rs:76-204`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/execution_pipeline.rs#L76-L204)). |
| [`cli_seam.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/cli_seam.rs) | Broken one-envelope contract, protocol version, structured failures, handle validation, key-value smuggling, key overwrite, or path-only boundary ([`sdk/rs/tests/cli_seam.rs:1-5`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/cli_seam.rs#L1-L5), [`sdk/rs/tests/cli_seam.rs:50-231`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/cli_seam.rs#L50-L231)). |
| [`prover_live.rs`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/prover_live.rs) | Manually probes shared Sepolia prover reachability/error shape; both tests are intentionally ignored from normal CI ([`sdk/rs/tests/prover_live.rs:1-16`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/prover_live.rs#L1-L16), [`sdk/rs/tests/prover_live.rs:31-66`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/prover_live.rs#L31-L66)). |

**Fresh execution evidence, 2026-08-05:** `cargo test --all-targets` in [`sdk/rs`](https://github.com/PoulavBhowmick03/Erebus/tree/main/sdk/rs) completed
with 194 passed and 3 ignored; `pnpm vitest run` in [`sdk/ts`](https://github.com/PoulavBhowmick03/Erebus/tree/main/sdk/ts) completed with 38 passed. The
ignored Rust cases are the two live shared-prover probes and the not-yet-achieved uniform-salt
fingerprint target, as declared in their source ([`sdk/rs/tests/prover_live.rs:1-16`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/prover_live.rs#L1-L16),
[`sdk/rs/tests/wire_v2_fingerprint.rs:60-75`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/wire_v2_fingerprint.rs#L60-L75)).

### The u128 domain-tag bug

The first implementation accumulated Cairo short-string bytes into `u128`; tags longer than
16 bytes silently lost high bytes. Short tags passed while channel/subchannel/outgoing tags
failed, producing a deceptive partial success ([`docs/friction.md:406-424`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L406-L424)). The Cairo KAT
failed immediately, before a network run; the fix right-aligns up to 31 bytes in a 32-byte
buffer and calls `Felt::from_bytes_be` ([`docs/friction.md:426-434`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L426-L434),
[`sdk/rs/src/hashes.rs:25-40`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L25-L40)). Without the KAT, read/write parties using different tags would
derive different secret slots and report absence, which is exactly why “it compiles” has almost
no evidentiary value for these formulas.

### What is not covered

Wire v2 has not been exercised in a fresh live offer/counter/settlement/reveal, implemented by
a second language, independently reviewed, or fee-measured ([`README.md:14-24`](https://github.com/PoulavBhowmick03/Erebus/blob/main/README.md#L14-L24),
[`docs/friction.md:1115-1122`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L1115-L1122)). The local execution-pipeline test does not claim Sepolia
compatibility ([`sdk/rs/tests/execution_pipeline.rs:1-5`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/execution_pipeline.rs#L1-L5)), and normal CI does not reach the real
prover ([`sdk/rs/tests/prover_live.rs:1-16`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/prover_live.rs#L1-L16)). The repository also has no test proving grantee
cryptographic authorization, because `grantee` is metadata and the grant is bearer
([`docs/friction.md:928-936`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L928-L936)).

I could not find a test for a crash after chain inclusion but before `lease.commit`, a fully
successful screening-attested deposit against live infrastructure, non-Unix permission
enforcement, large/reorging discovery, or a malicious grant holder recomputing the unkeyed
checksum after editing participant metadata. I’m inferring these gaps from the relevant code
paths. Verify by adding fault-injection/live tests around `Client` receipt→commit boundaries,
screening responses, and `grant_checksum_v2` ([`sdk/rs/src/client.rs:688-695`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L688-L695),
[`sdk/rs/src/disclosure.rs:290-307`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L290-L307)).

## 6. Rust-specific engineering decisions

### Unsafe, panics, and FFI boundaries

Both library and CLI use `#![forbid(unsafe_code)]` ([`sdk/rs/src/lib.rs:22`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/lib.rs#L22),
[`sdk/rs/src/bin/erebus_cli.rs:8`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/bin/erebus_cli.rs#L8)). That makes accidental in-crate unsafe memory/FFI escape
impossible, but dependencies can still contain unsafe internally; the attribute is not a
whole-supply-chain proof. There is no PyO3/FFI boundary: Python starts an ordinary process and
exchanges JSON ([`sdk/py/src/erebus/_seam.py:1-18`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/py/src/erebus/_seam.py#L1-L18), [`sdk/py/src/erebus/_seam.py:120-165`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/py/src/erebus/_seam.py#L120-L165)). A
subprocess was chosen specifically so Python never owns the key value.

The convention is no `unwrap`/`expect` outside tests and construction-proven constants
([`CLAUDE.md:149-152`](https://github.com/PoulavBhowmick03/Erebus/blob/main/CLAUDE.md#L149-L152)). Production `expect`s are confined to invariants such as fixed-width
HKDF output, AES key size, checked vector-to-array width, writing into a `String`, and an
internally-created transparent ID ([`sdk/rs/src/wire.rs:383-426`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L383-L426), [`sdk/rs/src/read.rs:235-245`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/read.rs#L235-L245),
[`sdk/rs/src/state.rs:52-57`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L52-L57), [`sdk/rs/src/bin/erebus_cli.rs:312-315`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/bin/erebus_cli.rs#L312-L315)). If those assumptions drift,
the one-shot CLI panics and the seam receives non-JSON stdout or a failed child rather than a
structured protocol error ([`sdk/py/src/erebus/_seam.py:146-165`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/py/src/erebus/_seam.py#L146-L165)); that is why invariant
locality matters here.

### Newtypes as protocol invariants

- `NoteSalt`, `FeltEntropy`, and `RandomSalt` separate bounded note storage, full-felt entropy,
  and unpredictable value-note nonces ([`sdk/rs/src/actions.rs:58-162`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/actions.rs#L58-L162)). Runtime checks at every
  call site would be easy to omit; distinct parameter types make the wrong lane fail to compile.
- `ActionSet` cannot be constructed except through a builder that checks ordering, invokes,
  nonempty, and replay protection ([`sdk/rs/src/action_set.rs:97-178`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/action_set.rs#L97-L178)). A raw `Vec` would defer
  errors until a paid proof/revert.
- `SubchannelCursor` allocates exact contiguous ranges and refuses off-grid starts
  ([`sdk/rs/src/subchannel.rs:82-162`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/subchannel.rs#L82-L162)). A bare `u32` would require every write path to remember
  gap, reuse, and five-note framing rules.
- `PoolInvocation` can exist only after zero-tip/zero-resource-price checks
  ([`sdk/rs/src/tx.rs:205-250`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/tx.rs#L205-L250)). Without it, every proof call could produce a valid-looking v3
  object that Cairo `__validate__` rejects.
- `ChannelHandle` validates a narrow grammar before filesystem use
  ([`sdk/rs/src/state.rs:26-66`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L26-L66)). This turns path traversal/malformed handle rejection into the
  boundary constructor rather than repeated sanitization.

The structured-salt-on-zero-amount invariant is not wholly encoded in a single public type:
`CreateEncNoteInput` still publicly accepts a `NoteSalt` with any amount
([`sdk/rs/src/actions.rs:203-218`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/actions.rs#L203-L218)). It becomes unrepresentable only through the private
`Channel::data_note`/`value_note` constructors ([`sdk/rs/src/channel.rs:490-512`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L490-L512)). A caller using
the public low-level action structs can bypass that policy. This is a real limit of the
newtype claim.

### Async, ownership, and lifetimes in this code

Network waits live in Rust’s `reqwest`/Tokio clients and the high-level async trait: RPC,
proving, maturity, fee, submission, and receipt polling all borrow `&self` across `.await`
([`sdk/rs/src/prover.rs:177-220`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/prover.rs#L177-L220), [`sdk/rs/src/rpc.rs:33-165`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/rpc.rs#L33-L165),
[`sdk/rs/src/execution.rs:105-264`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L105-L264), [`sdk/rs/src/client.rs:538-573`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L538-L573)). `Client` owns cloned,
internally reference-counted HTTP clients through `Executor`; requests can borrow configuration
and action data without moving the client ([`sdk/rs/src/client.rs:63-82`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L63-L82),
[`sdk/rs/src/execution.rs:84-103`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L84-L103)). The public trait uses native `async fn` and explicitly
allows the `async_fn_in_trait` lint, which means it is intended for concrete/static use rather
than promising a boxed object-safe future surface ([`sdk/rs/src/client.rs:538-573`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L538-L573)).

The important ownership choice is `ChannelLease`: it owns the lock file and mutable state, so
the OS lock remains held while client methods await maturity, reads, proving, submission, and
receipt ([`sdk/rs/src/state.rs:230-280`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L230-L280), [`sdk/rs/src/state.rs:425-446`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L425-L446);
[`sdk/rs/src/client.rs:648-695`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L648-L695)). No borrowed `&mut StoredChannel` escapes its owning lease, and
commit consumes the lease, so code cannot accidentally commit twice. The cost is head-of-line
blocking: one slow proof serializes every operation on that handle. I’m inferring that this is
intentional retry/cursor safety from the “keep lease alive through any async operation” doc
comment. Verify with maintainers if concurrent read-only access is desired
([`sdk/rs/src/state.rs:230-232`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L230-L232)).

`NoteSource` is synchronous and generic over `&impl NoteSource`, allowing pure read/decode code
to borrow a map/closure without async lifetimes ([`sdk/rs/src/read.rs:72-88`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/read.rs#L72-L88),
[`sdk/rs/src/read.rs:163-180`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/read.rs#L163-L180)). `Client` first performs async RPC into an owned `HashMap`, then
passes a short-lived borrowing closure to reconstruction ([`sdk/rs/src/client.rs:923-934`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L923-L934)). That
split avoids an async trait object and lets crypto/state-machine tests run with ordinary maps.

Python async is orchestration rather than protocol ownership: `asyncio.to_thread` prevents the
blocking subprocess from freezing MCP’s event loop ([`mcp-server/src/erebus_mcp/seam_client.py:8-17`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L8-L17),
[`mcp-server/src/erebus_mcp/seam_client.py:105-109`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L105-L109)). Thus “async confined to Rust” should be
rephrased as “protocol I/O, cryptography, state mutation, and transaction lifecycle are
implemented once in Rust; Python only schedules a blocking process adapter.”

### Error taxonomy

`ChannelError` represents action-composition violations such as wire, phase/index, zero
payment, missing inputs, amount disagreement, or record/payment collision before transport
([`sdk/rs/src/channel.rs:48-96`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L48-L96)). `ReadError` represents corrupted grants, partial/foreign data
notes, wire authentication/decoding, or invalid reconstructed negotiation
([`sdk/rs/src/read.rs:38-70`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/read.rs#L38-L70)). `ClientError` is the application boundary: it adds request,
identity, token, state, discovery, RPC/prover/execution, and transparently wraps the narrower
errors ([`sdk/rs/src/client.rs:1406-1521`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L1406-L1521)). Keeping them separate lets pure channel/read code
remain independent of filesystem/network policy while the CLI maps only the aggregate error
into stable agent-facing codes ([`sdk/rs/src/bin/erebus_cli.rs:336-427`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/bin/erebus_cli.rs#L336-L427)).

### State lease/commit and crashes

State writes serialize to a new 0600 temporary file, flush and `sync_all`, then atomically
rename over the record ([`sdk/rs/src/state.rs:380-414`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L380-L414)). `lock` holds an exclusive sibling lock
file; `commit(self)` writes and then drops the lock ([`sdk/rs/src/state.rs:230-280`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L230-L280),
[`sdk/rs/src/state.rs:425-446`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L425-L446)). If a process dies before an on-chain write, the original record
survives. If it dies after chain inclusion but before commit, the record is structurally intact
but logically stale; subsequent `sync_book` can recover note cursor/acceptance from keyed chain
reads in many operations ([`sdk/rs/src/client.rs:312-353`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L312-L353), [`sdk/rs/src/client.rs:766-785`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L766-L785)). It is
not a transactional two-phase commit with Starknet, and opening can be orphaned because state
creation occurs after receipt ([`sdk/rs/src/client.rs:623-644`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L623-L644), [`sdk/rs/README.md:113-116`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/README.md#L113-L116)).

## 7. Where the upstream stack fought us

This section separates upstream feedback from Erebus’s own design mistakes. [`docs/friction.md`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md)
contains two entries numbered F31, traffic fingerprinting and AEAD choice, which is an editorial
collision, not one issue ([`docs/friction.md:990-1020`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L990-L1020), [`docs/friction.md:1086-1122`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L1086-L1122)).

### Genuine upstream bugs or capability gaps

- A private note has no application payload and the Wallet API does not expose controlled
  note/salt construction. That forced the 119-bit salt lane and a lower-level client instead
  of a supported metadata extension ([`docs/friction.md:11-254`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L11-L254), [`docs/friction.md:612-666`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L612-L666)). A
  payload/commitment field or safe opaque metadata hook would remove the workaround.
- Salt types differ: amount encryption takes bounded `u128`; token and
  outgoing-recipient masks take full felts ([`docs/friction.md:258-276`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L258-L276);
  [`../starknet-privacy/packages/privacy/src/hashes.cairo:77-112`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/hashes.cairo#L77-L112),
  [`../starknet-privacy/packages/privacy/src/hashes.cairo:212-222`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/hashes.cairo#L212-L222)). Named newtypes in the
  upstream ABI/spec would make the distinction visible.
- Upstream’s encrypted-note view returns token zero by design/storage layout, while a naïve
  client expects the requested token echoed; the live path initially treated this as a bug
  ([`docs/friction.md:1126-1184`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L1126-L1184), [`sdk/rs/src/client.rs:1306-1336`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L1306-L1336)). A typed `EncryptedNote` view
  with “token implied by subchannel” documentation would prevent the misread.
- `proof_facts` extends v3 transaction hashing but is absent from ordinary account models, and
  prover failures can collapse to bare `-32603`/contract errors ([`docs/friction.md:577-608`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L577-L608),
  [`docs/friction.md:736-774`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L736-L774)). A versioned public transaction schema and structured prover
  error data would eliminate local transaction-model code and blind debugging.
- One channel key has no channel index, so a sender/recipient pair has one WriteOnce channel
  forever; reopening can spend a proof before failing ([`docs/friction.md:940-986`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L940-L986),
  [`sdk/rs/src/hashes.rs:74-93`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L74-L93)). An indexed/session channel derivation or explicit upstream
  idempotent lookup would support repeated relationships.

### Documentation failures

The work repeatedly required source archaeology for installability of the workspace package,
builder-required fields, automatic action insertion, mock semantics, Serde enum/span layout,
key exposure, sequential index scope, and the distinction between pool identity and Starknet
signing keys ([`docs/friction.md:316-404`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L316-L404), [`docs/friction.md:436-573`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L436-L573),
[`docs/friction.md:778-861`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L778-L861), [`docs/friction.md:1225-1252`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L1225-L1252)). A language-neutral protocol
document containing exact preimages, storage/discovery loops, action phases, calldata, proof
transaction extensions, key roles, and failure codes would have turned most of the Rust work
from reverse engineering into implementation.

The most serious missing warning was custody: both `compile_actions` RPC and prover payloads
receive the pool private key ([`docs/friction.md:473-538`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L473-L538)). That should be stated beside endpoint
configuration, not only inferable from calldata. The Rust modules now put the warning at the
URL boundary ([`sdk/rs/src/rpc.rs:1-8`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/rpc.rs#L1-L8), [`sdk/rs/src/prover.rs:3-14`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/prover.rs#L3-L14)).

### Defensible but surprising design decisions

The pool is intentionally keyed-discovery rather than event scanning, action compilation is a
virtual-account execution followed by proof-bound `apply_actions`, and client actions must
contain WriteOnce replay protection ([`CLAUDE.md:19-30`](https://github.com/PoulavBhowmick03/Erebus/blob/main/CLAUDE.md#L19-L30), [`sdk/rs/src/action_set.rs:1-28`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/action_set.rs#L1-L28)). These
choices are internally coherent, but each violates a normal Starknet client intuition. A
single official sequence diagram plus executable reference vectors would make the design
legible without weakening it.

Auditor registration escrows the entire pool private key, not a relationship-scoped read key
([`../starknet-privacy/packages/privacy/src/privacy.cairo:317-354`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/privacy.cairo#L317-L354)). This is a defensible
compliance model but materially stronger disclosure than Erebus’s bearer grant. The API name
`SetViewingKey` does not communicate “encrypt my spending/decryption root key to the pool
auditor”; explicit custody language would.

### Operational blockers

The shared prover is slow enough that errors cost a full proving round, has private/community
operational knowledge, and sometimes returns opaque failures; proof state also lags head and
expires after a validity window ([`docs/friction.md:670-774`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L670-L774), [`docs/friction.md:1407-1427`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L1407-L1427),
[`sdk/rs/src/execution.rs:105-130`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L105-L130), [`sdk/rs/src/execution.rs:184-190`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L184-L190)). Public dev endpoints,
health/version compatibility, queue status, structured errors, and a local deterministic prover
would dramatically shorten third-party iteration.

Deposits additionally require an external screening attestation, with freshness/signature
rules enforced by the pool ([`../starknet-privacy/packages/privacy/src/privacy.cairo:907-929`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/privacy.cairo#L907-L929);
[`docs/friction.md:1346-1403`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L1346-L1403)). The screening signer, prover/interceptor, pool key, RPC, token
approval, gas, maturity, and proof validity form one operational chain; a maintained testnet
runbook and disposable funded fixture identity would make end-to-end validation reproducible.

Gas and latency multiply with the protocol’s fixed shapes: every offer/counter is five note
creations and settlement adds spends plus six creations ([`sdk/rs/src/channel.rs:414-438`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L414-L438),
[`sdk/rs/src/channel.rs:577-610`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L577-L610)). Current gas evidence and proof timing are snapshot-specific,
not production guarantees ([`docs/friction.md:1188-1221`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L1188-L1221), [`docs/friction.md:1407-1427`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L1407-L1427)).

## 8. Security properties and limits

### Guarantees of the implemented path

- The final `apply_actions` contract validates recent proof facts and binds their message hash
  to the exact server actions before applying them atomically
  ([`../starknet-privacy/packages/privacy/src/privacy.cairo:782-839`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/privacy.cairo#L782-L839)).
- The Rust settlement constructor puts note spends, acceptance-data notes, and the payment note
  in one `ActionSet`; one proof and one transaction therefore apply all or none
  ([`sdk/rs/src/channel.rs:515-610`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L515-L610), [`sdk/rs/src/execution.rs:132-239`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L132-L239)). It also locally rejects
  disagreement between acceptance and payment amount ([`sdk/rs/src/channel.rs:545-555`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L545-L555)).
- Wire v2 encrypts and authenticates canonical terms under a channel/context-derived key
  ([`sdk/rs/src/wire.rs:383-499`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L383-L499)). The channel key permits note location/decryption but the
  owner pool private key is additionally required for nullifiers/spending
  ([`sdk/rs/src/hashes.rs:142-168`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L142-L168)).
- A grant contains two channel keys for one token/chain/pool relationship and no pool private
  key, allowing full two-direction reconstruction without unrelated channel keys
  ([`sdk/rs/src/disclosure.rs:45-88`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L45-L88), [`sdk/rs/src/disclosure.rs:149-171`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L149-L171)).

### What those guarantees do not mean

Atomicity is scheduling, not semantic truth. A hostile client can create zero-value notes whose
salts decode as an acceptance and a separate value note with another amount; the pool sees
notes, not offers ([`docs/friction.md:865-896`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L865-L896)). Erebus rejects this on write and compares again
on disclosure, but settlement consistency is **not** a Cairo/ZK-enforced negotiation rule
([`sdk/rs/src/channel.rs:545-555`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L545-L555), [`sdk/rs/src/disclosure.rs:309-337`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L309-L337)). Closing that gap requires
the proof program/contract to understand and bind the acceptance schema to the payment amount,
or a separate verifiable receipt circuit.

The viewing grant is a bearer secret. `grantee` is metadata, not encryption or authorization;
any holder can read the scoped relationship ([`docs/friction.md:922-936`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L922-L936)). Its Poseidon checksum
covers scope and keys, but it is unkeyed and recomputable by a holder
([`sdk/rs/src/disclosure.rs:290-307`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L290-L307)). I’m inferring that it detects accidental corruption but
does not authenticate the grantor or prevent intentional metadata edits. Verify with a test that
edits fields and recomputes the checksum. Cryptographic grantee binding needs encryption to
the grantee public key or a signed/attested capability.

The grant differs from STRK20 `SetViewingKey` in authority and scope. `SetViewingKey` encrypts
the identity’s **pool private key** to the configured auditor, which can consequently derive
all channels/nullifiers protected by that identity; the Erebus grant shares only two channel
keys and cannot derive owner-secret nullifiers ([`../starknet-privacy/packages/privacy/src/privacy.cairo:317-354`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/privacy.cairo#L317-L354),
[`sdk/rs/src/channel.rs:462-471`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L462-L471)). The former is pool-wide auditor escrow; the latter is
application-level relationship disclosure.

An on-chain observer still learns the submitting Starknet account, transaction timing, action
and calldata sizes, five-note message cadence, and the current fifth-salt fingerprint; v2 only
hides/authenticates the 400-bit content ([`docs/friction.md:990-1015`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L990-L1015),
[`sdk/rs/tests/wire_v2_fingerprint.rs:31-58`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/wire_v2_fingerprint.rs#L31-L58)). Counterparty linking may still be inferred from
timing and participating accounts. Removing zero padding/marker fingerprint is necessary but
not sufficient; traffic padding, relaying/account unlinkability, and timing analysis defenses
would be needed for relationship privacy.

### What a disclosed record proves versus asserts

Chain proof facts prove that the pool’s virtual execution produced the exact server actions
accepted by `apply_actions` ([`../starknet-privacy/packages/privacy/src/privacy.cairo:804-839`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/privacy.cairo#L804-L839)).
The grant holder can decrypt on-chain notes into messages and a payment amount, and can check
the latter equals the acceptance’s claimed amount ([`sdk/rs/src/disclosure.rs:234-270`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L234-L270),
[`sdk/rs/src/disclosure.rs:309-337`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L309-L337)).

The meanings “offer,” “counter,” “deadline,” “participants,” and “accepted offer” are local
interpretations of encrypted salt bytes and grant metadata; the pool circuit does not assert
them ([`sdk/rs/src/wire.rs:19-35`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L19-L35), [`sdk/rs/src/negotiation.rs:163-272`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/negotiation.rs#L163-L272)). No grantor signature or
ZK receipt binds the disclosed participant metadata and negotiation policy to the settlement.
So a “verified bound outcome covering membership, disclosure policy, and settlement
consistency” is **not yet** exposed. It would require a signed or ZK-verifiable record that
commits to participant identities, canonical terms/policy, the exact proven settlement actions,
and the disclosure authorization.

## 9. Hostile Q&A

1. **Why did you not just use our TypeScript SDK?** The active application is Python above a
   Rust key boundary, and running Node would add another key-holding runtime. The upstream Rust
   crate covered reads but not action construction/proving/submission, while this crate needed
   a Node-free write path ([`sdk/rs/src/lib.rs:3-20`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/lib.rs#L3-L20), [`sdk/py/src/erebus/_seam.py:10-18`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/py/src/erebus/_seam.py#L10-L18)).

2. **Is this a replacement for our SDK?** No. It implements the narrow negotiation/payment
   path and omits the general transfer, discovery-service, history, OHTTP, DeFi, and paymaster
   surface ([`sdk/rs/src/client.rs:538-573`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L538-L573), [`../starknet-privacy/sdk/src/index.ts:1-52`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/sdk/src/index.ts#L1-L52)).

3. **Why duplicate `discovery-core` hashes/decryption?** Upstream already has those formulas,
   but importing it would force a git-pinned `starknet-rust` fork/provider graph rejected by
   this write-side crate ([`sdk/rs/src/decrypt.rs:6-19`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/decrypt.rs#L6-L19)). Cairo KATs remain the common oracle.

4. **How do you know the hashes match Cairo?** Every derivation is compared against
   Cairo-emitted known answers, including the heterogeneous salt paths
   ([`sdk/rs/tests/cairo_conformance.rs:1-13`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/cairo_conformance.rs#L1-L13), [`sdk/rs/tests/cairo_conformance.rs:69-226`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/cairo_conformance.rs#L69-L226)). The
   u128 tag bug demonstrates that these tests catch real, compiling divergence.

5. **How do you know action calldata matches our SDK?** All ten variants are compared
   byte-for-byte with an upstream TypeScript `serializeClientActions`/`CallData.compile` fixture
   ([`sdk/rs/tests/clientaction_serde.rs:1-11`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/clientaction_serde.rs#L1-L11), [`sdk/rs/tests/clientaction_serde.rs:104-153`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/clientaction_serde.rs#L104-L153)).

6. **How do you know the composed proof invocation matches?** A captured upstream
   `ProofInvocationFactory` vector pins calldata, v3 hash, signature, and wire envelope together
   ([`sdk/rs/tests/proof_invocation.rs:1-13`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/proof_invocation.rs#L1-L13), [`sdk/rs/tests/proof_invocation.rs:97-151`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/proof_invocation.rs#L97-L151)).

7. **Live and mocked paths [WEAKNESS].** The README reports a complete wire-v1
   Sepolia flow, while the reference agent demo and current default MCP backend are mock
   ([`README.md:14-24`](https://github.com/PoulavBhowmick03/Erebus/blob/main/README.md#L14-L24), [`agents/src/erebus_agents/agent.py:1-7`](https://github.com/PoulavBhowmick03/Erebus/blob/main/agents/src/erebus_agents/agent.py#L1-L7), [`mcp-server/src/server.py:42-74`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/server.py#L42-L74)).
   Wire v2 still lacks a fresh live run and independent implementation/review.

8. **Does wire v2 hide the relationship? [WEAKNESS]** No. It hides terms but the fifth salt
   fingerprints each five-note message, and account/timing/cadence remain public
   ([`docs/friction.md:990-1015`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L990-L1015), [`sdk/rs/tests/wire_v2_fingerprint.rs:31-75`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/wire_v2_fingerprint.rs#L31-L75)).

9. **Why five zero-value notes for one message?** The pool has no payload field; each valid
   encrypted note exposes 119 usable salt bits, while authenticated v2 needs 536 bits
   ([`sdk/rs/src/wire.rs:7-17`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L7-L17), [`sdk/rs/src/wire.rs:29-35`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L29-L35)). Five notes provide 595 bits.

10. **Can a structured salt leak money?** The high-level constructors permit structured salts
    only on zero-value data notes and require `RandomSalt` for value notes
    ([`sdk/rs/src/channel.rs:490-512`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L490-L512)). **[WEAKNESS]** Low-level public action structs can bypass
    that policy ([`sdk/rs/src/actions.rs:203-218`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/actions.rs#L203-L218)).

11. **What does atomic settlement guarantee?** One proof/transaction applies the spends,
    acceptance record, and payment together, and Rust checks equal amounts
    ([`sdk/rs/src/channel.rs:515-610`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L515-L610)). It does not make the pool understand offer semantics.

12. **Can the grant holder spend?** Not from the grant: it has channel keys, while the
    nullifier also requires the owner pool private key ([`sdk/rs/src/disclosure.rs:45-88`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L45-L88),
    [`sdk/rs/src/hashes.rs:153-168`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L153-L168)).

13. **Is the grant bound to the named grantee? [WEAKNESS]** No; it is bearer and `grantee` is
    metadata ([`docs/friction.md:928-936`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L928-L936)). The checksum is integrity formatting, not
    authorization ([`sdk/rs/src/disclosure.rs:290-307`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L290-L307)).

14. **Why does the prover/RPC see the pool secret? [WEAKNESS]** Upstream `compile_actions`
    requires it in calldata, and the virtual invocation includes the same input
    ([`sdk/rs/src/calldata.rs:25-53`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/calldata.rs#L25-L53)). Both endpoints must therefore be operator-trusted
    ([`sdk/rs/src/prover.rs:3-14`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/prover.rs#L3-L14)).

15. **Why not call `__execute__` on-chain?** It is the virtual account path that emits server
    actions for proving; the real state transition is proof-validated `apply_actions`
    ([`../starknet-privacy/packages/privacy/src/privacy.cairo:193-212`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/privacy.cairo#L193-L212),
    [`../starknet-privacy/packages/privacy/src/privacy.cairo:782-839`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/privacy.cairo#L782-L839)). Rust never submits the
    proof invocation to Starknet ([`sdk/rs/src/execution.rs:192-231`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L192-L231)).

16. **What prevents index races? [WEAKNESS]** A per-handle exclusive filesystem lease and
    chain reseating serialize one local installation ([`sdk/rs/src/state.rs:230-280`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L230-L280),
    [`sdk/rs/src/client.rs:312-353`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L312-L353)). Another machine/process with separate state can race; Cairo
    remains the final WriteOnce/contiguity authority.

17. **What happens after a crash? [WEAKNESS]** Atomic file rename prevents torn local JSON,
    and later chain reads can recover much stale cursor state ([`sdk/rs/src/state.rs:380-446`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/state.rs#L380-L446),
    [`sdk/rs/src/client.rs:312-353`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L312-L353)). A crash after channel inclusion but before local creation
    can orphan the handle ([`sdk/rs/src/client.rs:623-644`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L623-L644)).

18. **Why exact-note payment with no change? [WEAKNESS]** `select_exact_notes` returns only a
    subset summing exactly to the offer, and settlement creates only the recipient payment note
    ([`sdk/rs/src/client.rs:1088-1117`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L1088-L1117), [`sdk/rs/src/channel.rs:596-608`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L596-L608)). General change output is
    outside the MVP.

19. **What would you change for production?** Complete a live/cross-language v2 run and review,
    remove the salt fingerprint, cryptographically bind grants, harden crash/idempotency and
    multi-process coordination, use operator-owned RPC/prover infrastructure, and validate
    screening/gas/reorg behavior ([`README.md:14-24`](https://github.com/PoulavBhowmick03/Erebus/blob/main/README.md#L14-L24), [`docs/friction.md:990-1015`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L990-L1015),
    [`sdk/rs/README.md:103-121`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/README.md#L103-L121)).

20. **What did this validate for Starknet Foundation?** It validated that a third party can
    independently build a Rust action/proof/submission/read path and obtain byte-level agreement
    with upstream oracles ([`sdk/rs/src/lib.rs:3-20`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/lib.rs#L3-L20), [`sdk/rs/tests/proof_invocation.rs:1-13`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/proof_invocation.rs#L1-L13)).
    **[WEAKNESS]** It also validated that documentation, custody, prover access, screening,
    latency, and traffic privacy remain material adoption barriers ([`docs/friction.md:436-538`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L436-L538),
    [`docs/friction.md:1295-1427`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L1295-L1427)).

## 10. Explainer layer

### Glossary

- **Note:** a pool record at `H(channel_key, token, index, 0)` containing packed salt and
  encrypted amount; spending writes a nullifier rather than erasing the note
  ([`sdk/rs/src/hashes.rs:142-168`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L142-L168), [`sdk/rs/src/decrypt.rs:103-149`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/decrypt.rs#L103-L149)).
- **Channel:** one directional sender→recipient secret and encrypted discovery record; a full
  conversation needs two opposing channels ([`sdk/rs/src/channel.rs:164-253`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L164-L253),
  [`sdk/rs/src/read.rs:295-321`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/read.rs#L295-L321)).
- **Subchannel:** a token-specific indexed record inside a channel; note indices are scoped to
  `(channel_key, token)` ([`sdk/rs/src/channel.rs:282-295`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L282-L295), [`sdk/rs/src/subchannel.rs:24-25`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/subchannel.rs#L24-L25)).
- **Nullifier:** the owner-secret hash that marks a note spent without deleting/revealing its
  note slot ([`sdk/rs/src/hashes.rs:153-168`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L153-L168),
  [`../starknet-privacy/packages/privacy/src/privacy.cairo:616-628`](https://github.com/starkware-libs/starknet-privacy/blob/3dfe66fe2b59d7b95709ec719547fa88b8ef63f9/packages/privacy/src/privacy.cairo#L616-L628)).
- **Action set:** an ordered, replay-protected batch of client intentions compiled privately
  into server actions ([`sdk/rs/src/action_set.rs:1-28`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/action_set.rs#L1-L28)).
- **Salt lane:** the public 119 usable bits in each encrypted-note salt used here to carry a
  fragmented zero-value negotiation message ([`sdk/rs/src/wire.rs:7-17`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L7-L17)).
- **Shielding:** depositing a public token amount and creating a private encrypted self-note in
  one balanced/replay-protected action set ([`sdk/rs/src/channel.rs:329-365`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L329-L365)).
- **Viewing grant:** a bearer package with both directional channel keys and one token’s scope;
  it reads one relationship but carries no owner pool key ([`sdk/rs/src/disclosure.rs:45-88`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/disclosure.rs#L45-L88)).
- **Proving block:** the historical block against which virtual execution is proved; recent
  writes must mature into that view and the result must be submitted before expiry
  ([`sdk/rs/src/execution.rs:105-130`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L105-L130), [`sdk/rs/src/execution.rs:143-190`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L143-L190)).

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

The boundaries and payloads are implemented at [`mcp-server/src/server.py:42-76`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/server.py#L42-L76),
[`sdk/py/src/erebus/_seam.py:120-173`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/py/src/erebus/_seam.py#L120-L173), [`sdk/rs/src/bin/erebus_cli.rs:202-306`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/bin/erebus_cli.rs#L202-L306), and
[`sdk/rs/src/execution.rs:132-239`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L132-L239).

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

The exact construction and phase sort are at [`sdk/rs/src/channel.rs:527-610`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L527-L610); execution is at
[`sdk/rs/src/execution.rs:132-239`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L132-L239).

### Five-minute spoken version

“This is not a Rust rewrite of the whole StarkWare SDK. It is a narrow Rust client for one
application flow. It reproduces the privacy-pool hashes, note decryption, Cairo action
serialization, proof invocation, transaction hash, signing, prover RPC, and final submission
that this flow needs. Above those pieces it adds its own offer, counter, acceptance,
persistence, and disclosure protocol. The source itself defines that boundary
([`sdk/rs/src/lib.rs:3-20`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/lib.rs#L3-L20), [`sdk/rs/src/client.rs:538-573`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L538-L573)).

We wrote the write path in Rust because the upstream Rust crate covered discovery but not
action construction and proving, and because the Python agent layer must not hold pool or
account keys. Python sends file paths through a one-request subprocess seam. Rust opens the
keys, owns the state, and performs the network lifecycle ([`sdk/py/src/erebus/_seam.py:1-18`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/py/src/erebus/_seam.py#L1-L18),
[`sdk/rs/src/execution.rs:132-239`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/execution.rs#L132-L239)).

The underlying pool is note based. A note’s location is a Poseidon hash of a secret channel
key, token, and index. Spending does not erase it; it writes an owner-secret nullifier. Notes
must be created at contiguous indices, so clients find them by deriving index zero upward and
stopping at the first empty slot ([`sdk/rs/src/hashes.rs:142-168`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/hashes.rs#L142-L168),
[`sdk/rs/src/client.rs:445-521`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/client.rs#L445-L521)).

The pool has no application payload field. This protocol therefore puts a fixed 400-bit
message into note salts on zero-value notes. Version 2 encrypts and authenticates those bytes
and needs five notes per message. Value notes use random salts, because mixing structured
salts with the amount mask is a confidentiality error ([`sdk/rs/src/wire.rs:7-45`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/wire.rs#L7-L45),
[`sdk/rs/src/channel.rs:490-512`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L490-L512)).

For settlement, the payer consumes an exact subset of its notes, writes five zero-value notes
containing the acceptance, and creates one value note for the payee. Those actions are one
ordered action set, one proof, and one final `apply_actions` transaction. That gives all-or-none
application. The Rust constructor separately checks that the acceptance amount and payment
amount agree, because the pool itself does not know what an offer means
([`sdk/rs/src/channel.rs:515-610`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/channel.rs#L515-L610), [`docs/friction.md:865-896`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L865-L896)).

Correctness comes from differential evidence rather than confidence in a second
implementation. Hashes and decryption match Cairo vectors; action serialization and the full
proof invocation match the upstream TypeScript SDK; transaction hashes and signatures match
starknet.js ([`sdk/rs/tests/cairo_conformance.rs:1-13`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/cairo_conformance.rs#L1-L13),
[`sdk/rs/tests/clientaction_serde.rs:1-11`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/clientaction_serde.rs#L1-L11), [`sdk/rs/tests/proof_invocation.rs:1-13`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/proof_invocation.rs#L1-L13),
[`sdk/rs/tests/invoke_v3_txhash.rs:1-11`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/tests/invoke_v3_txhash.rs#L1-L11)). The first Rust bug truncated felt domain tags into
128 bits, and those KATs caught it immediately ([`docs/friction.md:406-434`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L406-L434)).

The honest limits are important. The prover and preflight RPC see the pool private key. Wire
version 2 hides terms but still fingerprints traffic and exposes account timing. The viewing
grant is a bearer channel secret, not cryptographically bound to its named grantee. The
disclosed record reconstructs and checks what the notes say, but there is not yet a ZK receipt
binding participant metadata and negotiation policy to settlement. And version 2 still needs
a fresh live run, a second implementation, and independent review
([`sdk/rs/src/prover.rs:3-14`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/prover.rs#L3-L14), [`docs/friction.md:990-1015`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L990-L1015), [`docs/friction.md:922-936`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L922-L936),
[`README.md:14-24`](https://github.com/PoulavBhowmick03/Erebus/blob/main/README.md#L14-L24)).

What this proves is narrower and useful: a third party can implement the client-critical path
in Rust, match upstream byte-for-byte oracles, and run the full offline execution composition.
It also gives concrete feedback on what makes the privacy stack hard to adopt: missing
language-neutral specifications, strong endpoint custody assumptions, external prover and
screening operations, proving latency, and metadata leakage ([`sdk/rs/src/lib.rs:11-20`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/rs/src/lib.rs#L11-L20),
[`docs/friction.md:436-538`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L436-L538), [`docs/friction.md:1295-1427`](https://github.com/PoulavBhowmick03/Erebus/blob/main/docs/friction.md#L1295-L1427)).”

## 11. MCP server: the agent-facing control and policy layer

### 11.0 Scope boundary and one-sentence framing

**Use this sentence:** “The MCP server is an identity-bound Python adapter that presents the
negotiation client as nine agent-callable tools, applies payer/payee policy and exact-payment
preflights, and delegates real protocol execution through the Python subprocess seam to Rust.”
The identity and role are fixed when the module loads, the nine tools are registered against one
client, and the seam backend forwards calls rather than implementing hashes, field arithmetic, or
salt encoding ([`mcp-server/src/server.py:28-40`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/server.py#L28-L40), [`mcp-server/src/server.py:42-76`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/server.py#L42-L76),
[`mcp-server/src/erebus_mcp/seam_client.py:1-17`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L1-L17)).

That framing has four important exclusions:

- It is **not another privacy-protocol implementation**. The real adapter only converts Python
  dataclasses and dictionaries and runs the blocking seam off the event loop; the cryptographic
  work remains in Rust ([`mcp-server/src/erebus_mcp/seam_client.py:1-17`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L1-L17),
  [`mcp-server/src/erebus_mcp/seam_client.py:105-109`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L105-L109)).
- It is **not a general wallet API**. The exposed interface is channel negotiation, note-balance
  planning, settlement, waiting, and disclosure; the concrete seam has a `shield` helper, but
  `register_tools` does not expose it as an MCP tool ([`mcp-server/src/erebus_mcp/interface.py:187-226`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/interface.py#L187-L226),
  [`mcp-server/src/erebus_mcp/seam_client.py:181-193`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L181-L193),
  [`mcp-server/src/erebus_mcp/tools.py:89-273`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L89-L273)).
- It is **not the enforcement authority for all deal semantics**. It blocks a payee-role process
  from settling and preflights exact denominations, but Rust and the pool remain the lower
  execution layers ([`mcp-server/src/erebus_mcp/tools.py:71-87`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L71-L87),
  [`mcp-server/src/erebus_mcp/tools.py:170-206`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L170-L206)).
- It is **not a keyless interface in the broad sense**. Python does not open the pool or account
  key files, but it does return and accept the bearer `viewing_key` secret through MCP tool
  payloads ([`mcp-server/src/erebus_mcp/config.py:41-57`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L41-L57),
  [`mcp-server/src/erebus_mcp/tools.py:242-273`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L242-L273)).

The package itself requires Python 3.11 or later and depends on the official MCP package and the
workspace `erebus-sdk` Python seam ([`mcp-server/pyproject.toml:1-9`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/pyproject.toml#L1-L9),
[`mcp-server/pyproject.toml:18-22`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/pyproject.toml#L18-L22)).

### 11.1 Where it sits in the system

```text
agent / MCP host
       │  MCP JSON-RPC over stdin/stdout
       │  public terms, opaque handles, receipts, and possibly a bearer viewing grant
       ▼
server.py: one configured identity + one configured settlement role
       │
       ├── mock backend → shared JSON test store (no chain, proof, encryption, or locking)
       │
       └── seam backend → SeamErebusClient → sdk/py Seam
                                      │  one JSON request / child process
                                      ▼
                                  erebus-cli → Rust Client → RPC / prover / Starknet
```

The launcher reserves stdout for MCP JSON-RPC and sends diagnostics to stderr
([`scripts/erebus-mcp.sh:2-15`](https://github.com/PoulavBhowmick03/Erebus/blob/main/scripts/erebus-mcp.sh#L2-L15), [`scripts/erebus-mcp.sh:34-40`](https://github.com/PoulavBhowmick03/Erebus/blob/main/scripts/erebus-mcp.sh#L34-L40)). The Python seam sends one JSON
request to one CLI child and parses one JSON envelope ([`sdk/py/src/erebus/_seam.py:1-18`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/py/src/erebus/_seam.py#L1-L18),
[`sdk/py/src/erebus/_seam.py:120-165`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/py/src/erebus/_seam.py#L120-L165)). [`server.py`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/server.py) selects either the
JSON-backed mock or a `SeamErebusClient` configured with the Rust binary, endpoints, key-file
paths, state directory, and token ([`mcp-server/src/server.py:42-74`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/server.py#L42-L74)). The mock explicitly omits
locking and cryptography, while the real branch crosses the subprocess seam
([`mcp-server/src/erebus_mcp/mock_client.py:7-14`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L7-L14),
[`mcp-server/src/erebus_mcp/seam_client.py:1-12`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L1-L12)).

### 11.2 Module map and reading order

Read the files in this order because each later layer implements or exposes the contract defined
above it:

| Order | File | Responsibility, public surface, and dependencies | Layer |
|---:|---|---|---|
| 1 | [`interface.py`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/interface.py) | Defines the type aliases, frozen value objects, error codes, mutable exception, and async `ErebusClient` protocol implemented by both backends; it depends only on Python dataclasses, enums, and typing ([`mcp-server/src/erebus_mcp/interface.py:1-20`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/interface.py#L1-L20), [`mcp-server/src/erebus_mcp/interface.py:23-184`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/interface.py#L23-L184), [`mcp-server/src/erebus_mcp/interface.py:187-226`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/interface.py#L187-L226)). | Backend contract |
| 2 | [`config.py`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py) | Parses and validates environment configuration, models the settlement role, and builds optional seam settings; it depends on `os`, `Path`, dataclasses, and enums ([`mcp-server/src/erebus_mcp/config.py:16-21`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L16-L21), [`mcp-server/src/erebus_mcp/config.py:24-70`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L24-L70), [`mcp-server/src/erebus_mcp/config.py:72-140`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L72-L140)). | Bootstrap and policy configuration |
| 3 | [`mock_client.py`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py) | Implements the client protocol against a shared JSON file, synthetic handles/receipts/grants, simulated latency, and test-only failure injection ([`mcp-server/src/erebus_mcp/mock_client.py:1-24`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L1-L24), [`mcp-server/src/erebus_mcp/mock_client.py:75-121`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L75-L121), [`mcp-server/src/erebus_mcp/mock_client.py:183-411`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L183-L411)). | Deterministic test double |
| 4 | [`seam_client.py`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py) | Adapts the blocking [`sdk/py`](https://github.com/PoulavBhowmick03/Erebus/tree/main/sdk/py) seam to the async interface, maps dictionaries into interface objects, and translates seam errors ([`mcp-server/src/erebus_mcp/seam_client.py:1-17`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L1-L17), [`mcp-server/src/erebus_mcp/seam_client.py:47-91`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L47-L91), [`mcp-server/src/erebus_mcp/seam_client.py:94-208`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L94-L208)). | Real backend adapter |
| 5 | [`tools.py`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py) | Registers nine flat-argument MCP tools, applies role and payability policy, polls for offers, and converts interface objects into structured JSON envelopes ([`mcp-server/src/erebus_mcp/tools.py:1-15`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L1-L15), [`mcp-server/src/erebus_mcp/tools.py:42-87`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L42-L87), [`mcp-server/src/erebus_mcp/tools.py:89-299`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L89-L299)). | Agent API and client policy |
| 6 | [`server.py`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/server.py) | Loads config at import, creates one MCP server, chooses one backend, registers tools, and starts the stdio runtime ([`mcp-server/src/server.py:21-40`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/server.py#L21-L40), [`mcp-server/src/server.py:42-80`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/server.py#L42-L80)). | Composition root |
| 7 | [`scripts/erebus-mcp.sh`](https://github.com/PoulavBhowmick03/Erebus/blob/main/scripts/erebus-mcp.sh) | Validates the role and env file, optionally provisions an identity, sets seam defaults, protects stdout, and `exec`s the Python server ([`scripts/erebus-mcp.sh:1-24`](https://github.com/PoulavBhowmick03/Erebus/blob/main/scripts/erebus-mcp.sh#L1-L24), [`scripts/erebus-mcp.sh:27-61`](https://github.com/PoulavBhowmick03/Erebus/blob/main/scripts/erebus-mcp.sh#L27-L61)). | Operator launcher |
| 8 | [`pyproject.toml`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/pyproject.toml) | Declares the Python/MCP/seam dependencies and packages `src/erebus_mcp` as a wheel ([`mcp-server/pyproject.toml:1-22`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/pyproject.toml#L1-L22)). | Packaging |
| 9 | [`test_mock_client.py`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_mock_client.py) | Calls the entire mock workflow directly, tests payer-side note consumption and offer-state failures, and injects representative error groups ([`mcp-server/tests/test_mock_client.py:1-10`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_mock_client.py#L1-L10), [`mcp-server/tests/test_mock_client.py:31-67`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_mock_client.py#L31-L67), [`mcp-server/tests/test_mock_client.py:70-192`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_mock_client.py#L70-L192)). | Backend unit/integration tests |
| 10 | [`test_seam_client.py`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_seam_client.py) | Tests real-adapter shape translation against recorded CLI-shaped payloads and a stub seam; it explicitly reaches no chain ([`mcp-server/tests/test_seam_client.py:1-12`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_seam_client.py#L1-L12), [`mcp-server/tests/test_seam_client.py:47-66`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_seam_client.py#L47-L66), [`mcp-server/tests/test_seam_client.py:69-195`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_seam_client.py#L69-L195)). | Adapter tests |
| 11 | [`test_tools.py`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_tools.py) | Spawns [`server.py`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/server.py) over stdio and uses the official MCP client SDK to test discovery, role policy, exact payability, two-server mock settlement, and structured failures ([`mcp-server/tests/test_tools.py:1-17`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_tools.py#L1-L17), [`mcp-server/tests/test_tools.py:20-41`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_tools.py#L20-L41), [`mcp-server/tests/test_tools.py:44-248`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_tools.py#L44-L248)). | Transport-level tests |

### 11.3 Startup, identity binding, and configuration

Configuration is evaluated as `_config = ServerConfig.from_env()` during module import, before
the server and backend are constructed ([`mcp-server/src/server.py:21-30`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/server.py#L21-L30)). A missing required
setting therefore prevents startup instead of failing on the first agent call
([`mcp-server/src/erebus_mcp/config.py:24-25`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L24-L25),
[`mcp-server/src/erebus_mcp/config.py:143-147`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L143-L147)).

| Configuration | Required when | Meaning and validation |
|---|---|---|
| `AGENT_ADDRESS` | Always | Identity bound to this server; only non-emptiness is checked ([`mcp-server/src/erebus_mcp/config.py:72-78`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L72-L78), [`mcp-server/src/erebus_mcp/config.py:143-147`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L143-L147)). |
| `PROVING_SERVICE_URL` | Always, including mock | Prover endpoint; required as an anti-fallback configuration rule even though mock does not use it ([`mcp-server/src/erebus_mcp/config.py:1-13`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L1-L13), [`mcp-server/src/erebus_mcp/config.py:77-78`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L77-L78)). |
| `EREBUS_SETTLEMENT_ROLE` | Always | Must be `payer`, `payee`, or `both`; this controls MCP-side offer preflights and settlement authorization ([`mcp-server/src/erebus_mcp/config.py:28-38`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L28-L38), [`mcp-server/src/erebus_mcp/config.py:83-90`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L83-L90)). |
| `EREBUS_BACKEND` | Optional | Defaults to `mock`; the only accepted values are `mock` and `seam` ([`mcp-server/src/erebus_mcp/config.py:79-81`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L79-L81)). |
| `EREBUS_MOCK_STORE_PATH` | Mock only | Shared JSON path; defaults to `/tmp/erebus-mock-store.json` ([`mcp-server/src/erebus_mcp/config.py:92-93`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L92-L93)). |
| `EREBUS_MOCK_LATENCY_SECONDS` | Mock only | Floating-point artificial delay; defaults to `0.2` ([`mcp-server/src/erebus_mcp/config.py:93-97`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L93-L97)). |
| `EREBUS_MOCK_SPENDABLE_NOTES`, `EREBUS_MOCK_PENDING_NOTES` | Mock only | Comma-separated positive integer denominations; spendable defaults to one `10^18` note and pending defaults empty ([`mcp-server/src/erebus_mcp/config.py:99-100`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L99-L100), [`mcp-server/src/erebus_mcp/config.py:150-160`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L150-L160)). |
| `EREBUS_CLI` | Seam | Path whose existence is checked before startup ([`mcp-server/src/erebus_mcp/config.py:117-123`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L117-L123)). |
| `POOL_KEY_FILE`, `ACCOUNT_KEY_FILE` | Seam | Paths whose existence is checked; Python stores and forwards them but does not open them ([`mcp-server/src/erebus_mcp/config.py:41-57`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L41-L57), [`mcp-server/src/erebus_mcp/config.py:125-139`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L125-L139)). |
| `STARKNET_RPC_URL`, `POOL_ADDRESS`, `STARKNET_CHAIN_ID`, `EREBUS_STATE_DIR`, `TOKEN_ADDRESS` | Seam | Required values used to build `SeamConfig` ([`mcp-server/src/erebus_mcp/config.py:131-140`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L131-L140), [`mcp-server/src/server.py:51-65`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/server.py#L51-L65)). |

The Python validator checks that the CLI and key paths exist, but it does not check that the CLI
is executable, key-file permissions are `0600`, the state directory exists, or the endpoints are
reachable ([`mcp-server/src/erebus_mcp/config.py:117-140`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L117-L140)). The launcher adds an executable check
for the CLI, while identity provisioning creates identity/state directories as `0700` and the env
file as `0600` ([`scripts/erebus-mcp.sh:55-59`](https://github.com/PoulavBhowmick03/Erebus/blob/main/scripts/erebus-mcp.sh#L55-L59), [`scripts/new-identity.sh:97-119`](https://github.com/PoulavBhowmick03/Erebus/blob/main/scripts/new-identity.sh#L97-L119)). **I could not
find an MCP-startup check that rejects overly broad key-file permissions.**

The launcher defaults the backend to `seam`, while direct [`server.py`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/server.py) startup defaults it to
`mock` ([`scripts/erebus-mcp.sh:50-53`](https://github.com/PoulavBhowmick03/Erebus/blob/main/scripts/erebus-mcp.sh#L50-L53), [`mcp-server/src/erebus_mcp/config.py:79-81`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L79-L81)). This is a
operational distinction in source: using the supplied launcher selects the real path,
but importing or running the Python entry point without `EREBUS_BACKEND` selects the test double.

### 11.4 The Python interface and its invariants

`ErebusClient` is an async structural protocol: a backend need not inherit from it, but it must
offer the listed method signatures to satisfy static typing ([`mcp-server/src/erebus_mcp/interface.py:15`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/interface.py#L15),
[`mcp-server/src/erebus_mcp/interface.py:187-226`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/interface.py#L187-L226)). The identity is absent from every method and is
instead bound when the concrete client is constructed ([`mcp-server/src/erebus_mcp/interface.py:1-8`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/interface.py#L1-L8),
[`mcp-server/src/erebus_mcp/mock_client.py:75-104`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L75-L104)).

The main data objects are immutable dataclasses: offer terms and offers, receipts, note balances,
viewing grants, channel state, and disclosed records ([`mcp-server/src/erebus_mcp/interface.py:30-132`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/interface.py#L30-L132)).
`ErebusError` is intentionally mutable because the exception protocol and `pytest.raises` attach
traceback state; the source records that freezing this exception caused `FrozenInstanceError`
([`mcp-server/src/erebus_mcp/interface.py:165-184`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/interface.py#L165-L184)).

`NoteBalance.can_pay` answers exact subset-sum, not total-balance sufficiency. It mirrors Rust’s
search bounds by considering at most 256 notes and returning false when reachable states reach
100,000 ([`mcp-server/src/erebus_mcp/interface.py:57-92`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/interface.py#L57-L92)). This is why one note worth 1 STRK does
not advertise a 0.7 STRK payment as possible when settlement creates no change
([`mcp-server/src/erebus_mcp/interface.py:59-64`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/interface.py#L59-L64)).

`SettlementErrorCode` separates non-retryable offer/state/payment errors, potentially retryable
screening/prover/submission errors, counterparty screening rejection, an opaque proof failure,
and seam-level invalid-request/identity errors ([`mcp-server/src/erebus_mcp/interface.py:135-162`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/interface.py#L135-L162)).
`ErebusError` carries code, message, and the backend’s retryability decision
([`mcp-server/src/erebus_mcp/interface.py:165-184`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/interface.py#L165-L184)).

### 11.5 Tool ledger: what an agent can call

Tool arguments are flat primitive fields rather than a nested `OfferTerms`, so the MCP-generated
schema exposes the meaning of amount, token, deadline, and memo hash directly
([`mcp-server/src/erebus_mcp/tools.py:1-5`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L1-L5),
[`mcp-server/src/erebus_mcp/tools.py:98-141`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L98-L141)). Every tool returns either
`{"ok": true, "result": ...}` or `{"ok": false, "error": {"code", "message",
"retryable"}}` ([`mcp-server/src/erebus_mcp/tools.py:42-58`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L42-L58)).

| Tool | Input and result | MCP policy before delegation | Real backend operation |
|---|---|---|---|
| `open_channel` | Takes a counterparty address and returns an opaque `channel_handle` ([`mcp-server/src/erebus_mcp/tools.py:89-96`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L89-L96)). | None beyond backend errors ([`mcp-server/src/erebus_mcp/tools.py:89-96`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L89-L96)). | Calls seam `open_channel`, which returns the CLI’s handle ([`mcp-server/src/erebus_mcp/seam_client.py:111-113`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L111-L113)). |
| `propose_offer` | Takes handle, base-unit amount, token address, Unix deadline, and 128-bit memo hash; returns `offer_id` ([`mcp-server/src/erebus_mcp/tools.py:98-117`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L98-L117)). | A server configured exactly as `payer` rejects an amount that is not payable by an exact note subset; `payee` and `both` skip this proposal preflight ([`mcp-server/src/erebus_mcp/tools.py:106-113`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L106-L113)). | Converts amount and memo hash to decimal strings, then invokes seam proposal ([`mcp-server/src/erebus_mcp/seam_client.py:115-117`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L115-L117), [`mcp-server/src/erebus_mcp/seam_client.py:196-208`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L196-L208)). |
| `counter_offer` | Adds `reply_to` to the offer fields and returns a new `offer_id` ([`mcp-server/src/erebus_mcp/tools.py:119-141`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L119-L141)). | A pure payer gets the same exact-payability preflight; the doc states that countering does not revoke the previous offer ([`mcp-server/src/erebus_mcp/tools.py:128-137`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L128-L137)). | Invokes seam counter with the reference and wire terms ([`mcp-server/src/erebus_mcp/seam_client.py:119-125`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L119-L125)). |
| `get_note_balance` | Optionally accepts a proposed amount and returns spendable denominations, total, pending denominations, and `can_pay_exactly` when requested ([`mcp-server/src/erebus_mcp/tools.py:60-69`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L60-L69), [`mcp-server/src/erebus_mcp/tools.py:143-154`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L143-L154)). | Read-only planning helper; pending notes are reported but not counted as spendable ([`mcp-server/src/erebus_mcp/interface.py:66-71`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/interface.py#L66-L71), [`mcp-server/src/erebus_mcp/tools.py:143-150`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L143-L150)). | Calls seam `balance` and converts amount strings to Python integers ([`mcp-server/src/erebus_mcp/seam_client.py:137-142`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L137-L142)). |
| `read_channel_state` | Takes a handle and returns offers plus an optional settlement ([`mcp-server/src/erebus_mcp/tools.py:156-168`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L156-L168)). | None beyond backend errors ([`mcp-server/src/erebus_mcp/tools.py:156-168`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L156-L168)). | Calls seam read; the adapter can map a `settlement` object if present, although the current CLI read result is documented as carrying settled status rather than reconstructed settlement details ([`mcp-server/src/erebus_mcp/seam_client.py:127-135`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L127-L135)). |
| `accept_and_settle` | Takes handle and offer id; returns offer id, transaction hash, nullifiers, and proving time ([`mcp-server/src/erebus_mcp/tools.py:170-206`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L170-L206)). | Rejects a `payee` process immediately. Otherwise it reads the offer, preflights its exact amount when found, and then delegates; unknown offers fall through so the backend returns its authoritative error ([`mcp-server/src/erebus_mcp/tools.py:179-197`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L179-L197)). | Calls seam atomic acceptance and maps the receipt ([`mcp-server/src/erebus_mcp/seam_client.py:144-153`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L144-L153)). |
| `wait_for_offers` | Takes handle, expected count, and a default 300-second timeout; returns state plus `timed_out` ([`mcp-server/src/erebus_mcp/tools.py:208-240`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L208-L240)). | Polls every five seconds using monotonic time; timeout is a successful result, not an error ([`mcp-server/src/erebus_mcp/tools.py:37-39`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L37-L39), [`mcp-server/src/erebus_mcp/tools.py:218-240`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L218-L240)). | Repeatedly invokes the same read method; there is no push-subscription backend ([`mcp-server/src/erebus_mcp/tools.py:212-240`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L212-L240)). |
| `grant_viewing_key` | Takes handle and grantee metadata; returns channel id, grantee, and bearer `viewing_key` ([`mcp-server/src/erebus_mcp/tools.py:242-256`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L242-L256)). | Warns through its tool description that delivery is the caller’s responsibility and possession grants read access ([`mcp-server/src/erebus_mcp/tools.py:243-247`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L243-L247)). | Passes through the Rust-owned grant fields without parsing the secret ([`mcp-server/src/erebus_mcp/seam_client.py:155-165`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L155-L165)). |
| `reveal` | Takes the three grant fields and returns channel, participants, offers, and optional settlement ([`mcp-server/src/erebus_mcp/tools.py:258-273`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L258-L273)). | Reconstructs a `ViewingKeyGrant`; no grantor-local channel state is required at the MCP layer ([`mcp-server/src/erebus_mcp/tools.py:258-264`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L258-L264)). | Passes the complete grant dictionary to the seam and maps the disclosed record ([`mcp-server/src/erebus_mcp/seam_client.py:167-179`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L167-L179)). |

There are therefore seven negotiation/disclosure methods from the frozen interface, plus the
`get_note_balance` planning helper and the `wait_for_offers` polling helper. The stdio test fixes
that exact nine-tool surface and fails if a protocol method disappears or an unexpected tool is
added ([`mcp-server/tests/test_tools.py:44-78`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_tools.py#L44-L78)).

### 11.6 Why payer and payee roles exist

The crucial semantic rule is stated in the source: `accept_and_settle` spends the **calling
identity’s** private notes ([`mcp-server/src/erebus_mcp/config.py:28-34`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L28-L34),
[`mcp-server/src/erebus_mcp/tools.py:170-178`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L170-L178)). Therefore a seller/payee does not “accept” the
buyer’s offer in the ordinary marketplace sense. The payee authors or counters with the final
offer, and the payer accepts that payee-authored offer so the payer’s notes fund settlement
([`mcp-server/src/erebus_mcp/tools.py:128-132`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L128-L132),
[`mcp-server/src/erebus_mcp/tools.py:179-184`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L179-L184)).

The role is defense in depth at the agent boundary:

1. Startup validates one of `payer`, `payee`, or `both` ([`mcp-server/src/erebus_mcp/config.py:83-90`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L83-L90)).
2. Server instructions tell the model which role it has and explicitly state that only the payer
   calls settlement ([`mcp-server/src/server.py:30-39`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/server.py#L30-L39)).
3. The tool implementation rejects settlement on a payee process even if the model ignores its
   instructions ([`mcp-server/src/erebus_mcp/tools.py:179-184`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L179-L184)).
4. A payer proposal/counter is preflighted against current note denominations, and any
   non-payee acceptance is preflighted against the selected offer ([`mcp-server/src/erebus_mcp/tools.py:98-141`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L98-L141),
   [`mcp-server/src/erebus_mcp/tools.py:186-197`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L186-L197)).

`both` is intentionally less restrictive: it can buy and sell, it skips offer-time payability
checks because an offer may be an ask, but it still checks payability before acceptance
([`mcp-server/src/erebus_mcp/config.py:31-38`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L31-L38),
[`mcp-server/src/erebus_mcp/tools.py:109-113`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L109-L113),
[`mcp-server/src/erebus_mcp/tools.py:179-197`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L179-L197)). This means the role is not an on-chain capability
or a cryptographic identity attribute; it is local MCP policy selected by the launcher or
environment ([`scripts/erebus-mcp.sh:20-25`](https://github.com/PoulavBhowmick03/Erebus/blob/main/scripts/erebus-mcp.sh#L20-L25), [`scripts/erebus-mcp.sh:50-53`](https://github.com/PoulavBhowmick03/Erebus/blob/main/scripts/erebus-mcp.sh#L50-L53)).

The preflight is also not a concurrency guarantee. It reads channel state, then note balance,
then invokes settlement as separate awaits ([`mcp-server/src/erebus_mcp/tools.py:186-197`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L186-L197)). **I’m
inferring from that sequence that another concurrent operation could change state between those
steps; verify the final safety behavior in Rust’s `Client::accept_and_settle` and state lease code.**
The MCP preflight improves agent feedback; it cannot replace the lower-layer validation.

### 11.7 End-to-end real calls

#### Opening and writing an offer

1. The MCP host sends flat JSON-compatible tool arguments to `open_channel`; the tool calls the
   identity-bound client and wraps the handle in the standard envelope
   ([`mcp-server/src/erebus_mcp/tools.py:89-96`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L89-L96)).
2. `SeamErebusClient` calls the blocking Python seam through `asyncio.to_thread` and extracts the
   `channel_handle` field ([`mcp-server/src/erebus_mcp/seam_client.py:105-113`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L105-L113)).
3. A later offer tool builds `OfferTerms`; for a pure payer it first calls balance and runs exact
   subset planning ([`mcp-server/src/erebus_mcp/tools.py:71-87`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L71-L87),
   [`mcp-server/src/erebus_mcp/tools.py:98-117`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L98-L117)).
4. The adapter serializes amount and memo hash as strings so wide integers do not cross a
   JavaScript-style double representation, while deadline remains numeric because the CLI expects
   a number ([`mcp-server/src/erebus_mcp/seam_client.py:196-208`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L196-L208)).
5. The Python seam starts one Rust CLI request below this layer; this adapter awaits its returned
   dictionary and exposes only the new offer id ([`mcp-server/src/erebus_mcp/seam_client.py:105-117`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L105-L117)).

#### Reading, deciding, and settling

1. `read_channel_state` returns all mapped offers. `_offer` requires the CLI payload’s offer id,
   channel id, proposer, terms, status, creation time, and optional reply id
   ([`mcp-server/src/erebus_mcp/seam_client.py:62-80`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L62-L80),
   [`mcp-server/src/erebus_mcp/seam_client.py:127-135`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L127-L135)).
2. The MCP serializer intentionally omits each offer’s `channel_id` because the surrounding tool
   call is already scoped by a channel handle; that omission is visible in `_offer_to_json`
   ([`mcp-server/src/erebus_mcp/tools.py:276-289`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L276-L289)). **This rationale is an inference from the
   output shape; I could not find a source comment stating why the field is dropped.**
3. `accept_and_settle` first enforces role, reads the channel, finds the requested offer, and checks
   exact payability when the offer exists ([`mcp-server/src/erebus_mcp/tools.py:170-197`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L170-L197)).
4. The real adapter calls the seam and returns the Rust receipt fields without interpreting the
   proof, nullifiers, or transaction hash ([`mcp-server/src/erebus_mcp/seam_client.py:144-153`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L144-L153),
   [`mcp-server/src/erebus_mcp/tools.py:197-206`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L197-L206)).

#### Grant and reveal

1. `grant_viewing_key` receives a channel handle and named grantee, delegates to Rust, and returns
   the grant’s channel id, grantee, and bearer secret ([`mcp-server/src/erebus_mcp/tools.py:242-256`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L242-L256)).
2. The adapter’s comment is explicit: “The grant is a bearer secret: whoever holds it can read
   this one relationship” ([`mcp-server/src/erebus_mcp/seam_client.py:155-165`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L155-L165)).
3. `reveal` accepts all three fields from any caller, rebuilds the grant object, and invokes the
   seam without needing the grantor’s local handle state ([`mcp-server/src/erebus_mcp/tools.py:258-273`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L258-L273)).
4. Python maps the disclosed participants, offers, and settlement but performs no cryptographic
   grant verification itself ([`mcp-server/src/erebus_mcp/seam_client.py:167-179`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L167-L179)).

### 11.8 Async, threads, subprocesses, and concurrency

The MCP functions are Python coroutines, so async is **not** confined to Rust
([`mcp-server/src/erebus_mcp/tools.py:89-273`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L89-L273)). The real seam itself is blocking, and a write can
occupy preflight, proof, fee estimation, submission, and receipt waiting. Calling it on the event
loop would prevent the server from parsing another tool call, so `_run` uses
`asyncio.to_thread` ([`mcp-server/src/erebus_mcp/seam_client.py:8-17`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L8-L17),
[`mcp-server/src/erebus_mcp/seam_client.py:105-109`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L105-L109)).

The important ownership consequence in Python is modest: the `SeamErebusClient` stores one `Seam`
object, and the worker thread receives a bound method plus ordinary arguments
([`mcp-server/src/erebus_mcp/seam_client.py:94-109`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L94-L109)). There is no Python FFI, borrowed Rust
reference, or unsafe shared buffer in this layer; the boundary is a blocking subprocess wrapper
imported from `erebus` ([`mcp-server/src/erebus_mcp/seam_client.py:20-26`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L20-L26),
[`mcp-server/src/erebus_mcp/seam_client.py:105-109`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L105-L109)).

`wait_for_offers` is different: it stays on the event loop, but each delay uses
`await asyncio.sleep`, so it yields while waiting ([`mcp-server/src/erebus_mcp/tools.py:208-240`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L208-L240)).
Each read on the seam is itself sent to a thread, so a long poll does not continuously occupy the
event loop ([`mcp-server/src/erebus_mcp/seam_client.py:105-109`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L105-L109),
[`mcp-server/src/erebus_mcp/seam_client.py:127-135`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L127-L135)).

The mock does **not** model concurrent writers. It performs read-modify-write with a fixed `.tmp`
path and atomic replacement, and its doc comment says it assumes sequential calls
([`mcp-server/src/erebus_mcp/mock_client.py:7-14`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L7-L14),
[`mcp-server/src/erebus_mcp/mock_client.py:123-131`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L123-L131)). Two simultaneous mock processes could race,
overwrite each other, or contend for the same temporary path. **I’m inferring those failure modes
from the unlocked read/modify/replace sequence; reproduce them with concurrent mock writes before
treating the exact symptom as verified.**

### 11.9 Error behavior across MCP

An application failure intentionally remains an MCP-protocol success. The source records that an
uncaught exception becomes an opaque `ToolError` string, so `_call` catches `ErebusError` and
returns its code, message, and retryability in a JSON object ([`mcp-server/src/erebus_mcp/tools.py:7-15`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L7-L15),
[`mcp-server/src/erebus_mcp/tools.py:47-58`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L47-L58)). The real stdio test confirms that an unknown offer
returns `ok: false` with `OFFER_UNKNOWN`, while the MCP result has `is_error == false`
([`mcp-server/tests/test_tools.py:222-248`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_tools.py#L222-L248)).

The seam adapter catches only the Python seam’s `ErebusError` in `_run`; another exception, such as
a missing response field or a programming error, escapes and becomes an MCP-level tool failure
([`mcp-server/src/erebus_mcp/seam_client.py:105-109`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L105-L109),
[`mcp-server/src/erebus_mcp/tools.py:47-52`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L47-L52)). This preserves structured expected failures without
silently classifying arbitrary bugs as protocol errors.

If Rust adds an error code that the MCP enum does not know, `_translate` maps its code to
`PROOF_FAILED` but preserves the message and `retryable` flag
([`mcp-server/src/erebus_mcp/seam_client.py:47-59`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L47-L59)). This keeps the agent from crashing, but loses
the new code’s identity. The adapter test fixes that current fallback behavior
([`mcp-server/tests/test_seam_client.py:157-169`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_seam_client.py#L157-L169)).

### 11.10 Mock backend: what it models and what it does not

The mock exists so two independent MCP processes can share a deterministic representation of one
pool-like channel without chain access ([`mcp-server/src/erebus_mcp/mock_client.py:1-14`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L1-L14)). It uses a
deterministic symmetric handle made from the two identity strings, per-author sequence numbers for
offer ids, stored JSON offers, synthetic random receipt fields, and a checksum-shaped synthetic
viewing key ([`mcp-server/src/erebus_mcp/mock_client.py:133-150`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L133-L150),
[`mcp-server/src/erebus_mcp/mock_client.py:183-225`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L183-L225),
[`mcp-server/src/erebus_mcp/mock_client.py:358-411`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L358-L411)).

It models these application rules:

- one settled channel rejects later proposals and counters
  ([`mcp-server/src/erebus_mcp/mock_client.py:198-208`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L198-L208),
  [`mcp-server/src/erebus_mcp/mock_client.py:227-237`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L227-L237));
- a counter must reference the other party’s known, unexpired, open offer, and the prior offer is
  marked `countered` without being revoked ([`mcp-server/src/erebus_mcp/mock_client.py:238-285`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L238-L285));
- deadlines are computed client-side at read/accept time
  ([`mcp-server/src/erebus_mcp/mock_client.py:174-179`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L174-L179),
  [`mcp-server/src/erebus_mcp/mock_client.py:287-307`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L287-L307));
- the acceptor is the payer, must pay an exact subset, and its selected mock notes are consumed
  ([`mcp-server/src/erebus_mcp/mock_client.py:316-379`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L316-L379));
- acceptance and payment are represented by one write to the mock settlement record
  ([`mcp-server/src/erebus_mcp/mock_client.py:358-379`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L358-L379)).

It does **not** model Poseidon derivations, encryption, action serialization, proving, screening,
RPC behavior, account signing, gas, chain reverts, block maturity, reorgs, or contract execution;
its own module describes it as “minus locking and crypto,” and its receipts and grants are locally
fabricated ([`mcp-server/src/erebus_mcp/mock_client.py:7-14`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L7-L14),
[`mcp-server/src/erebus_mcp/mock_client.py:358-386`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L358-L386)). Therefore a passing two-agent mock demo proves
MCP transport and application policy, not privacy-pool compatibility or testnet settlement.

The mock cannot organically create `AMOUNT_MISMATCH` because the accepted offer amount is its only
payment source; it injects that and prover/screening failures through the test-only `force_error`
hook ([`mcp-server/src/erebus_mcp/mock_client.py:16-24`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L16-L24),
[`mcp-server/src/erebus_mcp/mock_client.py:106-121`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L106-L121)).

There is also a concrete search divergence: `NoteBalance.can_pay` stops after 256 notes or 100,000
reachable sums, while the mock’s internal `_select_exact_indices` has no such limits
([`mcp-server/src/erebus_mcp/interface.py:73-92`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/interface.py#L73-L92),
[`mcp-server/src/erebus_mcp/mock_client.py:423-436`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L423-L436)). Tool-level payer acceptance preflights through
the bounded `NoteBalance`, so normal MCP calls are conservative; direct mock-client calls can
accept a subset the Rust-mirroring planner would reject ([`mcp-server/src/erebus_mcp/tools.py:186-197`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L186-L197)).

### 11.11 Custody and security boundary

For the seam backend, Python holds paths to the pool key and account key, not their file contents;
`SeamSettings` explicitly says this process never reads them, and [`server.py`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/server.py) forwards those paths
into `SeamConfig` ([`mcp-server/src/erebus_mcp/config.py:41-57`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L41-L57),
[`mcp-server/src/server.py:51-65`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/server.py#L51-L65)). The actual protection therefore depends on the Python layer
continuing not to open the files and on the lower Rust/seam path handling them correctly.

The source comment claiming that two identities in one Python process would put both pool keys in
the same heap is not literally supported by this implementation: `SeamErebusClient` stores a
`Seam`, while `SeamConfig` is constructed from key **paths**, and this module does not open either
file ([`mcp-server/src/erebus_mcp/seam_client.py:94-103`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L94-L103),
[`mcp-server/src/server.py:51-65`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/server.py#L51-L65), [`mcp-server/src/erebus_mcp/config.py:41-57`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L41-L57)). **I’m inferring that
one-process-per-identity still reduces authority and configuration mix-up risk, but the stated
“both key values in the Python heap” rationale should be corrected or verified against
[`sdk/py/src/erebus/_seam.py`](https://github.com/PoulavBhowmick03/Erebus/blob/main/sdk/py/src/erebus/_seam.py).**

MCP does carry sensitive disclosure material. The grant tool returns `viewing_key`, and reveal
accepts it as a plain string ([`mcp-server/src/erebus_mcp/tools.py:242-273`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L242-L273)). The `grantee` field is
metadata in the Python type, while the grant remains a bearer secret
([`mcp-server/src/erebus_mcp/interface.py:95-99`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/interface.py#L95-L99)). Consequently the MCP host, conversation/tool
transcript storage, and any caller that receives the result are inside the disclosure-secret trust
boundary. The tool does not provide secure delivery or recipient authentication
([`mcp-server/src/erebus_mcp/tools.py:243-247`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L243-L247)).

The launcher sources an env file into its process and exports the backend, role, CLI, and
`PYTHONPATH`; it then replaces itself with the server ([`scripts/erebus-mcp.sh:43-61`](https://github.com/PoulavBhowmick03/Erebus/blob/main/scripts/erebus-mcp.sh#L43-L61)). It also
protects the protocol stream by sending provisioning logs and validation errors to stderr
([`scripts/erebus-mcp.sh:34-43`](https://github.com/PoulavBhowmick03/Erebus/blob/main/scripts/erebus-mcp.sh#L34-L43), [`scripts/erebus-mcp.sh:55-58`](https://github.com/PoulavBhowmick03/Erebus/blob/main/scripts/erebus-mcp.sh#L55-L58)).

### 11.12 Design choices and tradeoffs

| Choice | What the source is trying to achieve | Cost or limitation |
|---|---|---|
| Python MCP layer, Rust protocol layer | Keep agent-framework integration in Python while leaving hashes, felts, salts, key access, and execution in one Rust implementation ([`mcp-server/src/erebus_mcp/seam_client.py:1-17`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L1-L17)). | Every real call crosses Python objects, JSON, and a child-process boundary; adapter shape drift must be tested ([`mcp-server/tests/test_seam_client.py:1-12`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_seam_client.py#L1-L12)). |
| Blocking seam moved with `asyncio.to_thread` | Keep MCP responsive during long Rust writes ([`mcp-server/src/erebus_mcp/seam_client.py:8-12`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L8-L12), [`mcp-server/src/erebus_mcp/seam_client.py:105-109`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L105-L109)). | Worker threads and child processes can overlap; the MCP layer itself does not serialize operations per identity ([`mcp-server/src/erebus_mcp/seam_client.py:105-109`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L105-L109)). **The overlap consequence is inferred; verify with a concurrent seam test.** |
| One identity and role per server | Bind authority once and make the dangerous payee-settlement path uncallable ([`mcp-server/src/erebus_mcp/config.py:28-38`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L28-L38), [`mcp-server/src/erebus_mcp/tools.py:179-184`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L179-L184)). | Operators who buy and sell need `both` or multiple processes; `both` weakens offer-time policy specificity ([`mcp-server/src/erebus_mcp/tools.py:109-113`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L109-L113), [`mcp-server/src/erebus_mcp/tools.py:179-197`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L179-L197)). |
| Flat tool arguments | Produce discoverable schemas for agents ([`mcp-server/src/erebus_mcp/tools.py:1-5`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L1-L5)). | The Python layer duplicates the offer field list and must stay aligned with Rust/CLI ([`mcp-server/src/erebus_mcp/tools.py:98-141`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L98-L141), [`mcp-server/src/erebus_mcp/seam_client.py:196-208`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L196-L208)). |
| Structured error inside a successful MCP call | Preserve machine-readable code and retryability instead of an opaque SDK-generated error string ([`mcp-server/src/erebus_mcp/tools.py:7-15`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L7-L15), [`mcp-server/src/erebus_mcp/tools.py:47-58`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L47-L58)). | MCP clients must inspect `ok`; checking only protocol-level `is_error` treats business failure as transport success ([`mcp-server/tests/test_tools.py:241-246`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_tools.py#L241-L246)). |
| Exact-note preflight in MCP | Prevent a payer agent from negotiating or accepting an amount its current denominations cannot pay ([`mcp-server/src/erebus_mcp/tools.py:71-87`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L71-L87), [`mcp-server/src/erebus_mcp/tools.py:98-141`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L98-L141)). | It adds reads and is a time-of-check/time-of-use hint, not an atomic reservation ([`mcp-server/src/erebus_mcp/tools.py:186-197`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L186-L197)). The race characterization is an inference from separate awaits. |
| Polling helper | Reduce repeated agent tool turns and yield the event loop while waiting ([`mcp-server/src/erebus_mcp/tools.py:208-240`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L208-L240)). | It still performs periodic reads, has no push notification, and does not validate positive `expected_count` or timeout values ([`mcp-server/src/erebus_mcp/tools.py:208-240`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L208-L240)). |
| Mock as default for direct server startup | Keep local MCP tests runnable without keys, gas, chain, or prover use ([`mcp-server/src/server.py:42-45`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/server.py#L42-L45), [`mcp-server/src/erebus_mcp/config.py:79-81`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L79-L81)). | A successful default demo can be mistaken for real settlement unless the backend is reported explicitly; only the launcher defaults to seam ([`scripts/erebus-mcp.sh:50-53`](https://github.com/PoulavBhowmick03/Erebus/blob/main/scripts/erebus-mcp.sh#L50-L53)). |
| Bearer viewing grant passed through MCP | Allow a third party to reconstruct without grantor-local state ([`mcp-server/src/erebus_mcp/tools.py:258-273`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L258-L273)). | Tool payloads and MCP transcript handling become part of the confidentiality boundary; named grantee is metadata, not MCP-enforced possession proof ([`mcp-server/src/erebus_mcp/interface.py:95-99`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/interface.py#L95-L99), [`mcp-server/src/erebus_mcp/tools.py:243-247`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L243-L247)). |

### 11.13 Source disagreements and defects to say out loud

1. **“The server holds no key material” needs qualification.** It does not open pool/account key
   files, but it does handle a bearer viewing key in grant/reveal payloads
   ([`mcp-server/src/server.py:15-18`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/server.py#L15-L18), [`mcp-server/src/erebus_mcp/config.py:41-57`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L41-L57),
   [`mcp-server/src/erebus_mcp/tools.py:242-273`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L242-L273)). Say “Python does not load spending-key file
   contents” instead of “the MCP server handles no secrets.”

2. **The multi-identity heap comment conflicts with the implemented seam boundary.** The comments
   say two identities would place both pool keys in one heap, but the Python objects shown here
   carry paths and defer file reads to Rust ([`mcp-server/src/erebus_mcp/seam_client.py:94-100`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L94-L100),
   [`mcp-server/src/erebus_mcp/config.py:41-57`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L41-L57)). The compiler/interpreter obeys the path-forwarding
   code, not the rationale comment.

3. **`DisclosedSettlement.is_consistent` treats missing payment evidence as success.** It returns
   true when `paid_amount` is `None` ([`mcp-server/src/erebus_mcp/interface.py:102-112`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/interface.py#L102-L112)), and MCP
   exports that boolean ([`mcp-server/src/erebus_mcp/tools.py:292-299`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L292-L299)). This means
   `is_consistent: true` can mean either “amounts match” or “no paid amount was supplied.” **That
   is a weakness, not a proof of settlement consistency; the API should use a tri-state/optional
   result or return false for missing evidence.**

4. **The mock selector is less bounded than the advertised/Rust-mirroring planner.** The interface
   caps subset search, but the direct mock settlement selector does not
   ([`mcp-server/src/erebus_mcp/interface.py:73-92`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/interface.py#L73-L92),
   [`mcp-server/src/erebus_mcp/mock_client.py:423-436`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L423-L436)). Tests through tools use the bounded
   preflight, but direct backend tests do not prove parity for large note sets.

5. **The source asserts MCP 2.0 behavior, but the package declares only a lower bound.** The entry
   point and tool comments say they were verified against `mcp==2.0.0`, while the dependency is
   `mcp[cli]>=1.2.0` with no upper bound or exact pin ([`mcp-server/src/server.py:10-13`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/server.py#L10-L13),
   [`mcp-server/src/erebus_mcp/tools.py:7-15`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L7-L15), [`mcp-server/pyproject.toml:6-9`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/pyproject.toml#L6-L9)). The lockfile may pin a
   workspace installation, but the published requirement alone does not guarantee those SDK
   internals.

6. **`wait_for_offers` accepts nonsensical counts and timeouts.** There is no positive-value check;
   zero/negative expected counts or timeouts satisfy the first loop exit immediately
   ([`mcp-server/src/erebus_mcp/tools.py:208-240`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L208-L240)). This is not a protocol vulnerability, but it is
   an agent-API validation gap.

### 11.14 Test evidence and its boundary

| Test | What it establishes | What it does not establish |
|---|---|---|
| `test_happy_path_end_to_end` | Two direct mock clients converge on a handle, exchange proposal/counter, consume payer notes, make a synthetic grant, and reveal a consistent mock record ([`mcp-server/tests/test_mock_client.py:31-67`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_mock_client.py#L31-L67)). | No MCP transport, Rust, cryptography, prover, RPC, or chain is involved ([`mcp-server/src/erebus_mcp/mock_client.py:7-14`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L7-L14)). |
| Mock rejection tests | Cover post-settlement writes, expiry, double settlement, payer direction, exact note consumption, self-acceptance, unknown counters, and representative injected error groups ([`mcp-server/tests/test_mock_client.py:70-192`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_mock_client.py#L70-L192)). | Injected prover/screening/amount errors prove propagation only, not that real upstream conditions produce them ([`mcp-server/src/erebus_mcp/mock_client.py:16-24`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L16-L24), [`mcp-server/src/erebus_mcp/mock_client.py:108-121`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L108-L121)). |
| Seam adapter tests | Cover offer/balance mapping, absence of settlement detail on ordinary read, reveal settlement mapping, disagreeing amounts, known and unknown errors, wide-integer string encoding, and opaque grant passthrough ([`mcp-server/tests/test_seam_client.py:69-195`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_seam_client.py#L69-L195)). | The seam is a stub and “nothing here reaches a chain” ([`mcp-server/tests/test_seam_client.py:1-12`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_seam_client.py#L1-L12), [`mcp-server/tests/test_seam_client.py:47-62`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_seam_client.py#L47-L62)). |
| MCP discovery test | Starts the server through stdio with the official client and asserts the exact nine tools and descriptions ([`mcp-server/tests/test_tools.py:1-17`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_tools.py#L1-L17), [`mcp-server/tests/test_tools.py:44-78`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_tools.py#L44-L78)). | It starts the default mock backend, not the seam ([`mcp-server/tests/test_tools.py:20-35`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_tools.py#L20-L35)). |
| MCP policy tests | Verify `can_pay_exactly`, reject an unpayable payer-authored offer without writing state, and structurally deny payee settlement ([`mcp-server/tests/test_tools.py:81-138`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_tools.py#L81-L138)). | They do not test role `both`, concurrent state changes, or Rust’s final enforcement ([`mcp-server/tests/test_tools.py:20-35`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_tools.py#L20-L35)). |
| Two-server MCP test | Runs separate payer and payee MCP subprocesses against shared mock state, has the payee author the offer, settles from the buyer, consumes the buyer’s mock note, and exposes settlement to the seller ([`mcp-server/tests/test_tools.py:141-190`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_tools.py#L141-L190)). | It proves topology and policy, not on-chain atomicity, key isolation, or privacy ([`mcp-server/src/erebus_mcp/mock_client.py:7-14`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L7-L14)). |
| Structured-error MCP test | Proves application failure remains machine-readable and is not flagged as an MCP transport error ([`mcp-server/tests/test_tools.py:222-248`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_tools.py#L222-L248)). | It covers one mock `OFFER_UNKNOWN` path, not every real seam error code. |

I could not find MCP tests for `wait_for_offers`, invalid configuration, launcher provisioning,
file permissions, concurrent tool calls, real `server.py → Seam → erebus-cli` execution, or live
testnet calls; the MCP test directory contains only the mock, seam-adapter, and stdio-mock files
listed above ([`mcp-server/tests/test_mock_client.py:1-192`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_mock_client.py#L1-L192),
[`mcp-server/tests/test_seam_client.py:1-195`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_seam_client.py#L1-L195), [`mcp-server/tests/test_tools.py:1-252`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_tools.py#L1-L252)). I also could
not find a test for `paid_amount=None` combined with the exported `is_consistent` value
([`mcp-server/src/erebus_mcp/interface.py:102-112`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/interface.py#L102-L112),
[`mcp-server/src/erebus_mcp/tools.py:292-299`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L292-L299)).

### 11.15 MCP implementation evidence and gaps

The source and tests support this claim: an external MCP client can discover the tool schemas,
drive two identity-bound server processes through the intended payer/payee sequence, receive
structured application errors, and exercise the adapter shapes expected from recorded CLI output
([`mcp-server/tests/test_tools.py:1-17`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_tools.py#L1-L17), [`mcp-server/tests/test_tools.py:44-248`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_tools.py#L44-L248),
[`mcp-server/tests/test_seam_client.py:1-12`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_seam_client.py#L1-L12)).

They do **not** by themselves prove that an MCP-originated request completed a Rust proof or an
on-chain transaction, because the full MCP transport tests use `MockErebusClient` and the seam
tests use `StubSeam` ([`mcp-server/tests/test_tools.py:20-35`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_tools.py#L20-L35),
[`mcp-server/tests/test_seam_client.py:47-62`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_seam_client.py#L47-L62)). Closing that evidence gap requires a test or
recorded run that launches [`server.py`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/server.py) with `EREBUS_BACKEND=seam`, invokes the real CLI, and verifies
the resulting transaction and disclosed record against Starknet; **I could not find that test in
[`mcp-server/tests/`](https://github.com/PoulavBhowmick03/Erebus/tree/main/mcp-server/tests).**

Before production, the MCP-specific work visible from this source is: fix the missing-payment
consistency result, validate polling arguments, test concurrent calls and mock/store behavior,
test startup/permission failures, pin or constrain the MCP SDK API relied upon, and add a real-seam
transport test ([`mcp-server/src/erebus_mcp/interface.py:102-112`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/interface.py#L102-L112),
[`mcp-server/src/erebus_mcp/tools.py:208-240`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L208-L240), [`mcp-server/pyproject.toml:6-9`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/pyproject.toml#L6-L9),
[`mcp-server/tests/test_tools.py:20-35`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_tools.py#L20-L35)).

### 11.16 Two-minute spoken explanation

“The MCP server is the agent-facing control layer, not another implementation of the privacy
protocol. One process is configured with one Starknet identity, one payer/payee role, and either a
mock backend or the real Python-to-Rust seam. It exposes nine tools: open, propose, counter, read,
balance planning, accept and settle, wait, grant, and reveal ([`mcp-server/src/server.py:28-76`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/server.py#L28-L76),
[`mcp-server/src/erebus_mcp/tools.py:89-273`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L89-L273)).

The most important policy rule is that accepting spends the caller’s notes. A seller therefore
leaves the final seller-authored offer and the buyer’s payer process accepts it. This is written in
the server instructions and, more importantly, enforced by disabling settlement on a payee-role
server. The payer also checks that an exact subset of its private note denominations can pay the
amount, because this settlement path creates no change ([`mcp-server/src/server.py:30-39`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/server.py#L30-L39),
[`mcp-server/src/erebus_mcp/tools.py:71-87`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L71-L87), [`mcp-server/src/erebus_mcp/tools.py:170-197`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L170-L197)).

On the real path, Python does not calculate Poseidon hashes, salts, proofs, or Starknet
transactions. It turns tool arguments into the seam’s dictionaries, moves the blocking subprocess
call onto a worker thread, and maps the Rust result back into stable Python objects. Pool and
account key-file contents are opened below this layer, although their paths are present in Python
configuration ([`mcp-server/src/erebus_mcp/config.py:41-57`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/config.py#L41-L57),
[`mcp-server/src/erebus_mcp/seam_client.py:1-17`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L1-L17),
[`mcp-server/src/erebus_mcp/seam_client.py:105-109`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/seam_client.py#L105-L109)).

Failures are returned as an application envelope instead of MCP errors. That lets an agent branch
on a stable error code and retryability flag, but it means the agent must inspect `ok`; a successful
MCP call can contain a failed settlement ([`mcp-server/src/erebus_mcp/tools.py:7-15`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L7-L15),
[`mcp-server/tests/test_tools.py:222-248`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_tools.py#L222-L248)).

The mock is good evidence for tool discovery, buyer/seller policy, and two-process orchestration.
It is not evidence for privacy, proofs, or chain settlement: it uses JSON, synthetic receipts, and
no locking or cryptography. The seam-adapter tests also stop at a stub. The missing MCP-level proof
is one test or recorded run from the official MCP client through the real seam to a verified
Starknet transaction ([`mcp-server/src/erebus_mcp/mock_client.py:7-14`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/mock_client.py#L7-L14),
[`mcp-server/tests/test_seam_client.py:1-12`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_seam_client.py#L1-L12), [`mcp-server/tests/test_tools.py:20-35`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/tests/test_tools.py#L20-L35)).

Finally, disclosure crosses this boundary as a bearer secret. Python does not hold spending-key
contents, but the MCP host sees the returned viewing key, so transcript handling and delivery are
part of the trust boundary. Also, the current Python consistency helper treats a missing payment
amount as consistent; that must not be presented as verified settlement consistency
([`mcp-server/src/erebus_mcp/tools.py:242-273`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/tools.py#L242-L273),
[`mcp-server/src/erebus_mcp/interface.py:102-112`](https://github.com/PoulavBhowmick03/Erebus/blob/main/mcp-server/src/erebus_mcp/interface.py#L102-L112)).”
