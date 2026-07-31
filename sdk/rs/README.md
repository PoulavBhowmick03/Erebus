# Erebus Rust SDK

`erebus-sdk` is the protocol implementation. It builds STRK20 client actions, preflights
them against `compile_actions`, requests a proof, submits `apply_actions`, discovers notes
by derived ids, and reconstructs scoped disclosures.

## CLI protocol 2

`erebus-cli` reads one JSON request from stdin, writes one JSON envelope to stdout, and
exits. Every stateful request carries this config:

```json
{
  "rpc_url": "http://your-operator-controlled-pathfinder",
  "prover_url": "http://your-operator-controlled-prover",
  "pool_address": "0x...",
  "chain_id": "0x534e5f5345504f4c4941",
  "account_address": "0x...",
  "pool_key_file": "/operator/secrets/pool.key",
  "account_key_file": "/operator/secrets/account.key",
  "state_dir": "/operator/state/erebus",
  "token": "0x..."
}
```

The values of both private keys must not appear in JSON or argv. Rust opens the supplied
paths when an operation needs them. `state_dir` contains mode-`0600` secret-bearing channel
records under a mode-`0700` directory.

### Provision the two keys

They belong to one pool identity but serve different systems:

- `account_key_file` contains the private key for the deployed Starknet account at
  `account_address`. Create and fund that account with `sncast account create` /
  `sncast account deploy`, then place its raw `0x`-prefixed private felt in a mode-`0600`
  file. The CLI does not read the sncast account registry.
- `pool_key_file` contains an independent STRK20 pool identity key. No service issues it.
  Generate it inside Rust so the value never crosses the JSON seam:

```sh
mkdir -m 700 /absolute/operator/path/erebus
printf '%s\n' \
  '{"method":"generate_pool_key","params":{"path":"/absolute/operator/path/erebus/pool.key"}}' \
  | target/release/erebus-cli
```

`generate_pool_key` refuses relative paths and existing files, creates the destination mode
`0600` on Unix, and returns only the path and public key. Registration is automatic on the
first `shield` or `open_channel`; do not generate a replacement for an address that is
already registered.

Methods:

- `generate_pool_key`, local provisioning utility; no network or Python key value involved
- `open_channel`
- `propose_offer`
- `counter_offer`
- `read_channel_state`
- `accept_and_settle`
- `grant_viewing_key`
- `reveal`
- `shield`, administrative funding helper, outside the seven-method negotiation surface

## Negotiation wire v2

New channels encrypt/authenticate the canonical 400-bit negotiation record with
AES-256-GCM-SIV, then fragment the 50-byte ciphertext plus 16-byte tag across five public
note salts. HKDF-SHA-256 binds key/nonce derivation to the chain, pool, directional channel
key, token and message index; chain/pool/token/index are authenticated data.

State created before this change has no `wire_version` and loads as public wire v1. It
remains readable for disclosure, but every v1 write fails with `LegacyReadOnly`. Viewing
grant v1 remains readable; new grants include the chain, pool scope and wire version in
their checksum.

Build and check:

```sh
cargo build --release --bin erebus-cli
cargo test
cargo clippy --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
```

## Secret boundaries

- A `ChannelHandle` is `ch_` plus 256 random bits. It is not a channel key.
- Channel keys and cursors stay in the Rust state store.
- The proving request necessarily contains the pool identity key in
  `compile_actions` calldata. Use an operator-controlled prover.
- The preflight `starknet_call(compile_actions)` sends the same key to `rpc_url`. The write
  path therefore also requires an operator-controlled RPC/Pathfinder, not a public endpoint.
- Pool-key exposure reveals the identity's private history and derived locations. It does
  not by itself authorize a spend: the pool's simulated `__execute__` also validates the
  Starknet account signature over the action set.
- The account signing key never enters proving calldata, but the local Rust process needs
  it to sign the proof invocation and final account transaction.
- `grant_viewing_key` is the intentional exception: it exports a self-contained bearer
  secret covering both directions of one counterparty relationship and one token. Its
  `grantee` field is metadata; secure delivery is the operator's responsibility.

## Current MVP limits

- One configured token per client instance.
- Settlement selects unspent notes that sum exactly to the offer amount. It will not burn
  change; general change-note construction is not implemented.
- Reverse directional channels are paired by excluding keys already claimed in local state.
  An ambiguous match fails rather than guessing.
- Channel state records the last accepted write block. A dependent write waits until that
  block is visible at the `head - 10` proof anchor; settlement discovers candidate inputs at
  that same historical anchor, so it cannot select a fresh note the proof cannot observe.
- Calls do not yet carry idempotency keys. A process/transport failure after chain inclusion
  but before the response can orphan an `open_channel` handle or make a caller retry an
  already-written offer as a second offer. Cursor recovery prevents index reuse; it does not
  recover the lost application result.
- Shield, two-direction wire-v1 negotiation, atomic settlement and independent disclosure
  have landed on Sepolia. Wire v2 is verified offline and still needs a fresh live run,
  fee measurement and independent cryptographic review.
- `sdk/py` still speaks protocol 1. Updating the shared Python/MCP integration is P2.3 and
  was intentionally left out of this Rust-only pass.
