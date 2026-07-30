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

Methods:

- `open_channel`
- `propose_offer`
- `counter_offer`
- `read_channel_state`
- `accept_and_settle`
- `grant_viewing_key`
- `reveal`
- `shield` — administrative funding helper, outside the seven-method negotiation surface

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
- The account signing key never enters proving calldata, but the local Rust process needs
  it to sign the proof invocation and final account transaction.
- `grant_viewing_key` is the intentional exception: it exports a self-contained bearer
  secret covering both directions of one counterparty relationship and one token. Its
  `grantee` field is metadata in MVP v1; secure delivery is the operator's responsibility.

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
- The complete pipeline is pinned against local JSON-RPC fixtures, and the proof wire was
  probed against Sepolia. A successful protocol-2 shield/settlement has not yet landed on
  Sepolia; shielding remains gated by the deployed pool's screening attestation.
- `sdk/py` still speaks protocol 1. Updating the shared Python/MCP integration is P2.3 and
  was intentionally left out of this Rust-only pass.
