# Metropolis M2 Baseline: Offchain Eleusis

Recorded: 2026-09-21. Branch: `metropolis`. Scope: roadmap M2, private offchain Eleusis.
Decisions and the normative protocol are in [metropolis-m2-decisions.md](metropolis-m2-decisions.md).

## What shipped

New chain-neutral crate [`sdk/transport`](../sdk/transport) (`erebus-transport`):

| Module | Owns |
|---|---|
| `identity` | X25519 transport key with owner-only persistence, plus the secp256k1 authorization identity; redacted `Debug` |
| `session` | Noise XX (`Noise_XX_25519_ChaChaPoly_BLAKE2s`), capability-binding prologue, session id, peer-role/envelope binding, per-direction keys, session bounds |
| `message` | Canonical envelope, message types, body digest, redacted `Debug` |
| `transcript` | Per-author chains, ordering/replay/conflict rejection, `transcript_root` derivation |
| `store` | File-backed, lock-guarded transcript with checksummed records, interrupted-tail recovery, and Unix owner-only permissions |
| `relay` | Session-scoped ciphertext mailboxes, file-backed and in-memory implementations, limits and retention |
| `descriptor` | Signed service descriptors, validity/suite-mode/chain checks, discovery filter and verified JSON directory |

Plus `src/bin/erebus_relay.rs`, the self-hostable relay service (`GET /healthz`,
`POST /v1/mailbox/{id}`, `GET /v1/mailbox/{id}?after=`), and a `rust-transport` CI job that keeps
the crate free of chain and proving dependencies.

## Evidence

Run from `sdk/transport`:

```bash
cargo fmt --check          # clean
cargo clippy --all-targets -- -D warnings   # clean
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps   # clean
cargo test --all-targets   # 56 unit + 8 integration passing; 1 subprocess worker ignored directly
```

The integration suite (`tests/eleusis.rs`) is the M2 completion evidence:

- `two_participants_agree_on_one_transcript`: two independent participants with separate
  identities, sessions, and on-disk stores negotiate offer/counter/authorization over the relay
  and compute the same non-zero `transcript_root`.
- `a_restart_rehandshakes_and_the_transcript_continues`: a dropped session is never resumed; a
  fresh handshake gets a new session id and fresh directional mailboxes; the deal transcript
  continues from the persisted head and reaches the same root on both sides.
- `independent_processes_rehandshake_reopen_state_and_agree_on_the_root`: two actual Rust
  subprocesses load a signed JSON directory, negotiate over TCP, exit, reload identities,
  reopen separate stores, re-handshake, and agree on one non-zero root.
- `packaged_http_relay_enforces_auth_and_serves_ciphertext`: the packaged relay starts with its
  persistent store, rejects an unauthenticated health request, and serves an authenticated opaque
  blob through its HTTP API.
- `a_replayed_ciphertext_is_rejected`, `a_message_from_another_session_is_rejected`: delivery
  replay and cross-session messages fail authentication.
- `a_mismatched_capability_prologue_fails_the_handshake`: a substituted descriptor (a capability
  downgrade) fails the handshake.
- `a_descriptor_for_a_different_deployment_does_not_support_the_filter`: discovery does not
  match a seller on another chain.

Transcript ordering, replay, fork, and cross-deal rejection are also covered by unit tests in
`transcript.rs`; session-id/role binding and strict byte limits in `session.rs`; invalid revision
and sequence values in `message.rs`; suite-mode contradictions in `descriptor.rs`; relay bounds,
idempotency, retention, mailbox capacity, and private permissions in `relay.rs`; and store
ordering, interrupted-tail recovery, corruption detection, private permissions, and path safety
in `store.rs`.

## What M2 does not claim

- No shielded settlement suite, EVM adapter, or contract (M3-M5).
- Discovery is a verified JSON directory document, not a hosted HTTP discovery service. The
  transport for fetching it is the configured directory endpoint of D05 and is not built here.
- The `erebus-relay` binary is not wired into the CLI or MCP surface; agents reach the transport
  through the crate API.
- Metadata is not hidden. Endpoints, mailbox ids, sizes, timing, and availability are visible to
  the relay and any network observer (decision DM2-9). Content privacy and metadata exposure are
  separate claims. The session test records the fixed 16-byte Noise AEAD expansion, and relay
  tests expose blob length, cursor, and expiry without exposing plaintext.
- Owner-only filesystem modes are enforced and tested on Unix. M2 does not configure Windows ACLs;
  a Windows operator must provide an access-controlled storage directory.
- The `transcript_root` rule (computed for a deal with messages, zero otherwise) is defined in
  the M2 decision record and enforced by the transport; the agreement crate still treats the
  field as opaque committed bytes. Binding it into an authorization predicate remains settlement
  work.

## Friction

The agreement reserved `transcript_root` with no rule for who populates it or when zero is
valid; see `docs/friction.md` F44.
