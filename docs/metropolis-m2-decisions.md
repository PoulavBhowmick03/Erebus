# Metropolis M2 Decisions: Private Offchain Transport

Written: 2026-09-21. Scope: the offchain Eleusis transport for `metropolis` (roadmap M2).
Status: implementation decision record. The code lives in [`sdk/transport`](../sdk/transport).
This document is normative for the transport protocol; [metropolis-agreement.md](metropolis-agreement.md)
stays normative for the agreement encoding.

M2 is the first milestone where the negotiation graph leaves the chain. The existing STRK20
wire layer (`channel.rs`, `wire.rs`, `subchannel.rs`) is untouched and remains the historical
compatibility contract. This record covers the new path only.

Related: [architecture](metropolis-architecture.md) sections 2-4, [threat model](metropolis-threat-model.md)
sections 2 and 5, [decisions](metropolis-decisions.md) D03, D04, D05, and D07.

## DM2-1. Session protocol

Selected: **Noise XX** (`Noise_XX_25519_ChaChaPoly_BLAKE2s`), implemented by the maintained
[`snow`](https://crates.io/crates/snow) crate. XX gives mutual authentication and forward
secrecy from static transport keys, with a documented handshake state machine. HPKE (RFC 9180)
was the other candidate named in the architecture; it is a KEM plus AEAD building block, not a
session protocol, and would need an authentication wrapper designed and reviewed before use.
Hand-rolling X25519 + HKDF + ChaCha20-Poly1305 was rejected: it is the largest silent-failure
surface in the milestone.

Session identity and keys:

- Each participant holds a **transport identity key**: an X25519 static keypair, separate from
  the suite-1 agreement authorization key, the note-spending key, and the transaction key (D03).
- `TransportIdentity::generate_and_store` creates a new owner-only key file without overwriting an
  existing identity; `load` rejects non-owner Unix permission bits. Applications may instead
  reconstruct an identity from key bytes supplied by their own secret store.
- `session_id` is the Noise handshake hash (32 bytes). Both peers compute it; it is not
  transmitted as a negotiation field.
- The two directional transport keys are distinct by the Noise construction. Per-direction keys
  are never reused across a re-handshake.

Handshake **prologue** binds, in canonical bytes:

| Field | Why |
|---|---|
| transport `protocol_version` u16 | Rejects a peer speaking an unknown version |
| initiator descriptor digest `[32]` | Binds the initiator's advertised identity and capabilities |
| responder descriptor digest `[32]` | Binds the responder's advertised identity and capabilities |

The descriptor digests carry the chain namespace, asset set, suite set, and guarantee set, so a
guarantee downgrade or a substituted endpoint fails the handshake rather than being filtered
only at discovery time. The prologue is identical on both sides because each peer has the
other's descriptor from discovery.

### Nonce construction, rotation, and recovery

Noise transport nonces are internal per-direction counters. Reusing a counter under the same key
is catastrophic, so the policy is deliberately conservative:

- A session is **ephemeral**. It is bounded by `MAX_MESSAGES_PER_SESSION` (4096) and
  `MAX_SESSION_BYTES` (16 MiB of plaintext). Reaching either bound forces a re-handshake.
- Sessions are **never resumed after a process restart**. A restarted participant performs a new
  handshake. This guarantees no counter is ever replayed, because the crashed session's keys are
  discarded with the process. There is no "resume the counter" path to get wrong.
- Crash safety does not require persisting counters: the *transcript* is the durable object
  (DM2-4), and continuity across a restart is reconstructed from it, not from the dead session.
- Relay mailboxes are derived from `session_id` and sender role. A re-handshake therefore creates
  two fresh directional mailboxes. A restarted client never feeds ciphertext from the dead
  session into the new Noise state, and relay cursors never cross a session boundary.

This trades a cheap re-handshake on restart for removing the nonce-reuse failure mode entirely.
Forward secrecy within a session, and rekey-by-rehandshake, are the rotation story.

## DM2-2. Transport identity and peer authentication

Peer authentication is two-layered:

1. **Endpoint identity.** The Noise static key authenticates the peer on the wire and is bound
   into the session keys.
2. **Agreement identity.** A [`ServiceDescriptor`] binds the Noise static public key to a
   suite-1 secp256k1 address by a suite-1 signature the seller produces. A client accepts a
   descriptor only if the signature verifies against the descriptor's stated seller address.

Neither layer alone is sufficient. Noise proves possession of the transport key; the descriptor
signature proves that the agreement identity (the key that will authorize settlement) stands
behind that transport key. A relay or discovery directory cannot substitute either without
failing verification, so it cannot redirect a buyer to a different seller.

That direct agreement-key mapping applies to suite 1.
M8 adds an encrypted descriptor-signed binding for separate suite-2 agreement keys.
See [DM8-7](metropolis-m8-decisions.md); the public descriptor encoding is unchanged.

## DM2-3. Message envelope and canonical encoding

Every message is carried inside one Noise transport message. The envelope plaintext is:

| # | Field | Encoding | Rule |
|---|---|---|---|
| 1 | `protocol_version` | `u16` | must be 1 |
| 2 | `session_id` | raw 32 | the delivering session; envelope metadata |
| 3 | `deal_id` | raw 16 | shared by all revisions and messages of the deal |
| 4 | `revision` | `u32` | greater than zero |
| 5 | `author` | `u8` tag | 1 buyer, 2 seller |
| 6 | `sequence` | `u64` | per author, per deal; starts at 1; contiguous |
| 7 | `parent_hash` | raw 32 | the author's previous message link; zero for the first |
| 8 | `message_type` | `u8` tag | 1 offer, 2 counter, 3 authorization |
| 9 | `body` | bytes 0..=8192 | type-specific, canonical |

The physical envelope is `session_id || encode_body`; the table lists logical fields.
`encode_body` starts with `protocol_version` and excludes `session_id`.
The envelope is authenticated by Noise AEAD. `session_id` stays in the envelope but is
**excluded from the transcript digest** (DM2-4): the transcript is per deal and must survive a
re-handshake after a restart, so it cannot be keyed by an ephemeral session.
The session API checks the envelope `session_id` against the delivering handshake hash and checks
`author` against the authenticated endpoint role before returning a `Message`. Raw transport
plaintext methods are crate-private, so callers cannot bypass these checks.

Field order and integer widths follow the agreement's canonical encoding rules
([metropolis-agreement.md](metropolis-agreement.md) section 5); the transport reuses
`erebus_core::encoding`. There is exactly one encoding of a body.

`MessageType` bodies are opaque to the transport at M2: the offchain Eleusis state machine
carries them, and the transport guarantees ordering and authorship, not business meaning. A
body larger than `MAX_BODY_BYTES` (8192) is rejected at construction and on decode.

Diagnostics: `Debug` for an envelope prints `deal_id`, `revision`, `author`, `sequence`, and
`message_type`. It never prints the body or `session_id`. Key material types implement
`Debug` as a redacted placeholder. Message contents and keys do not reach CLI logs or
model-visible output (roadmap M2 item 7).

## DM2-4. Transcript, ordering, and the transcript root

The transcript is per deal, kept per author. Each author's messages form a hash chain:

```text
message_digest_i = Hash_suite("EREBUS_MESSAGE_V1" || encode(body_i))
link_0           = 0x00..00
link_i           = Hash_suite("EREBUS_TRANSCRIPT_LINK_V1" || author_tag || link_{i-1} || message_digest_i)
head_author      = link_last
transcript_root  = Hash_suite("EREBUS_TRANSCRIPT_ROOT_V1" || deal_id || head_buyer || head_seller)
```

The root is order-independent across the two directions: the buyer chain and the seller chain
are hashed in a fixed role order, so an interleaving race between the peers cannot produce two
different roots. Both peers compute the same root from the same message set, with no shared
clock.

`Hash_suite` is the agreement suite hash (keccak256 for suite 1), so the root is recomputable by
anything that can verify the agreement.

> **Amended during M5 (2026-09).** Suite 2 has no byte-hash function: its commitment is a
> typed Poseidon field map. `Hash_suite` above is therefore replaced by keccak256 under
> transport hash version 1 (`sdk/transport/src/hashing.rs`, `TRANSCRIPT_HASH_VERSION`),
> selected independently of the agreement suite. The preimages and domain tags are
> unchanged, so every suite-1 root and stored transcript is byte-identical. The root is
> recomputable by anything with keccak256 and the messages. The suite-2 circuit binds it as
> two opaque 128-bit halves and does not recompute it.

**Rule for the reserved agreement field.** A revision that concludes an M2-negotiated deal MUST
carry the computed `transcript_root`. It is all-zero only for a deal with no transcript, which
covers legacy wire-v1/v2 revisions and the existing zero vectors. There is no
`protocol_version` bump: the field already exists, and zero retains its existing meaning.

Rejection rules, all enforced by the transcript before a message is appended:

| Attack | Rejection |
|---|---|
| Duplicate message | `sequence` is not exactly the author's `head_sequence + 1` |
| Reordered message | same rule; a gap or an old sequence is rejected |
| Forked / conflicting message | `parent_hash` does not equal the author's current chain head |
| Cross-deal message | `deal_id` does not match the transcript |
| Cross-session ciphertext | the message does not authenticate under the session keys |
| False envelope session id | `session_id` does not equal the delivering handshake hash |
| False author role | `author` does not equal the authenticated peer role |
| First message non-zero parent | the first message for an author must carry a zero parent |

The signed final root commits to the agreed transcript. It does not prove that unshared messages
exist or that business claims are true (architecture section 4).

## DM2-5. Relay and durable storage

The transport is a **minimal ciphertext relay**: an authenticated mailbox that stores opaque
blobs. The relay never holds a session key and cannot read a message.

- A **mailbox id** is a 32-byte opaque value derived from the Noise `session_id` and sender role.
  It changes after every re-handshake. The relay sees mailbox ids, blob sizes, timing, and
  availability only.
- `put(mailbox, blob)` returns an acknowledgment carrying an assigned cursor and an expiry.
  `put` is idempotent on an identical blob.
- `get(mailbox, after_cursor)` returns stored blobs after that cursor.
- Bounds: `MAX_BLOB_BYTES` 64 KiB, `MAX_BLOBS_PER_MAILBOX` 256, default retention 7 days (D05).
  A full mailbox or an over-size blob is rejected, not truncated.
- Clients persist the authenticated transcript **before** sending an acknowledgment (D05). The
  relay's acknowledgment confirms transport delivery, not permanent archival.
- Transcript/session state is namespaced by identity, deal, and deployment (D04) in a file-backed
  store guarded by an OS advisory lock (`fs2`), matching the existing single-writer discipline in
  `sdk/rs`.
- Transcript records carry a checksum. A complete record with a bad checksum fails closed; an
  incomplete final record from an interrupted append is removed under the exclusive store lock.
  Record files and relay files are `0600`, and their directories are `0700`, on Unix systems.

The relay is packaged as a library (for tests and self-hosting) and a small HTTP service
(`erebus-relay`) with bearer-token authentication, per-mailbox limits, retention, and a health
endpoint. Hosted and self-hosted deployments run the same binary with different configuration
(D05).

## DM2-6. Service publication and discovery

A `ServiceDescriptor` is a signed, portable record a seller publishes. It contains the seller's
suite-1 address, the transport static public key, endpoints, chain namespace, asset identifiers,
supported suites, settlement modes, supported guarantees, an issuance time, and an expiry. It
contains **no** negotiation transcript, reservation price, or deal-specific data; the type has
no fields for them, so their absence is structural rather than a filtering rule.

A client discovers a compatible seller through a **configured directory endpoint** that returns
descriptors. The client verifies each signature, expiry, and the advertised capabilities, then
filters by chain namespace, asset, and required guarantees. Discovery and negotiation require no
chain transaction (architecture section 2).

At M2 only suite 1 exists, and suite 1 supports public-bound settlement only. A descriptor that
advertises suite 1 with shielded mode, hidden amount, hidden recipient, or an asset from another
chain is rejected. Shielded advertisements remain unavailable until M4 selects and implements a
compatible suite.

## DM2-7. Limits

| Constant | Value | Bounds |
|---|---|---|
| `MAX_BODY_BYTES` | 8192 | Message body |
| `MAX_MESSAGE_BYTES` | 65536 | Ciphertext envelope |
| `MAX_MESSAGES_PER_DEAL` | 4096 | Transcript growth |
| `MAX_MESSAGES_PER_SESSION` | 4096 | Forces re-handshake |
| `MAX_SESSION_BYTES` | 16 MiB | Forces re-handshake |
| `MAX_BLOB_BYTES` | 65536 | Relay blob |
| `MAX_BLOBS_PER_MAILBOX` | 256 | Relay queue growth |
| `RELAY_RETENTION_SECONDS` | 604800 | Default relay retention (7 days) |
| `MAX_DESCRIPTOR_ENDPOINTS` | 4 | Discovery record |
| `MAX_DESCRIPTOR_ASSETS` | 16 | Discovery record |

Every wire bound is enforced at construction, decode, and before a session operation crosses the
limit. Nothing is silently truncated. The only repaired data is an incomplete final transcript
record identified during locked crash recovery; complete records with bad checksums are rejected.

## DM2-8. Module boundary

New crate `sdk/transport`, chain-neutral, depending only on `erebus-core` for encoding and the
agreement suite hash (amended in M5: the transcript hash is now transport-versioned keccak256,
see DM2-4). It has no Starknet, EVM, or proving dependency, matching the core's
isolation rule. Future `sdk/rs` wiring remains a thin adapter in M8; M2 does not add CLI or MCP
wiring. This is the "transport" boundary named in roadmap section 6.

## DM2-9. Metadata exposure

The transport hides message plaintext. It does **not** hide, and this milestone does not claim
to hide: participant endpoints, mailbox identifiers, message sizes, message timing, session
count, message counts, availability, or the fact that two parties are communicating. These are
the "Transport operator" row of the threat model (section 2) and are the accepted cost of an
offchain transport. Content privacy and metadata exposure are reported separately; an observer
that correlates endpoints and timing can still reconstruct a relationship graph even though
every ciphertext is secure.

## Open items and evidence

- `snow` version and algorithm suite are pinned in `sdk/transport/Cargo.toml`; changing them is a
  transport protocol change.
- Two real subprocesses negotiate over TCP, exit, reload their transport identities, reopen
  separate stores, re-handshake, load a signed JSON directory, and agree on one root in
  `sdk/transport/tests/eleusis.rs`.
- The original M2 gate covers ordered, authenticated transport, not business negotiation.
  M8 now adds the price-only profile below; arbitrary service-term negotiation remains unsupported.

## M8 extension: typed negotiation and freeze boundary

The owner selected a frozen negotiation prefix before final agreement authorizations ([DM8-6](metropolis-m8-decisions.md)).
The envelope tags and transcript hash version stay unchanged.
`sdk/transport/src/negotiation.rs` defines profile version `1` inside the opaque message body.
The body starts with `u16 profile_version`, then one `u8 event_tag`.

| Event | Envelope type | Body after version and event tag | Stored in negotiation |
|---|---|---|---|
| 1 proposal | Offer or Counter | parent proposal digest `[32]`, length-prefixed encoded proposal | Yes |
| 2 acceptance | Authorization | selected proposal digest `[32]` | Yes |
| 3 final authorization | Authorization | length-prefixed canonical authorization | No |
| 4 identity binding | Authorization | length-prefixed canonical key binding | No |

A proposal encodes length-prefixed canonical draft terms, then raw blinding `[32]`.
Its digest is `Keccak256("EREBUS_NEGOTIATION_PROPOSAL_V1" || encoded_proposal)`.
Draft terms require a zero transcript root.
The initial buyer offer fixes all context; later counters change only amount and revision.
Each counter alternates authors and binds the preceding proposal digest.
The nonproposer accepts first; the proposer then confirms the same draft.
Neither acceptance contains a signature over the eventual agreement.

Both acceptance messages contribute to the sealed M2 root.
The final terms insert that root before either party signs the M1 commitment.
Final authorization envelopes reference the frozen author's next sequence and head but never extend its log.
Identity-binding envelopes use revision `1`, sequence `1`, and a zero parent as separate channel control.
Noise authenticates their session and author; they are not transcript sequence entries.

The low-level append store still rejects duplicates.
The typed store allows exact-body re-delivery after a fresh handshake without another append.
A checksum-protected, atomic freeze record prevents any later generic append under the same lock.
Checksums detect corruption; local owner-only storage remains trusted, not remotely authenticated evidence.
The SDK closes a connection after failed incoming validation or failed authorization persistence.
