# Erebus Rust SDK

This page describes current `main` and CLI Protocol 4. The published `v0.2.0` binary speaks
Protocol 4. The older `v0.1.0` binary speaks Protocol 2.

`erebus-sdk` is the protocol implementation. It builds STRK20 client actions, preflights
them against `compile_actions`, requests a proof, submits `apply_actions`, discovers notes
by derived ids, and reconstructs scoped disclosures.

## CLI protocol 4

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
  "token": "0x...",
  "wire_version": "v3"
}
```

The values of both private keys must not appear in JSON or argv. Rust opens the supplied
paths when an operation needs them. `state_dir` contains mode-`0600` secret-bearing channel
records under a mode-`0700` directory, plus two subdirectories: `operations/`, the durable
operation journal, and `notecache/`, cached immutable note prefixes.

The three are not equally precious. Channel records are rebuildable from the pool key and the
chain (`rebuild_state`), and the note cache is pure derived data that is discarded whenever it
does not parse. **The journal is the one that cannot be reconstructed**, because it is the only
record that a transaction was signed. Back it up with the channel records, from the same
moment: see [custody-operations.md](../../docs/custody-operations.md).

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
- `reconcile`, read-only journal classification
- `resume_operation`, explicit recovery under the original operation ID
- `rebuild_state`, additive channel-state recovery from keys and chain data
- `grant_viewing_key`
- `reveal`
- `shield`, administrative funding helper, outside the seven-method negotiation surface

## Negotiation wire v3

New channels default to wire v3. Each authenticated plaintext contains a 64-bit deal ID and
the 400-bit negotiation record. AES-256-GCM-SIV produces a 58-byte ciphertext and a 16-byte
tag. Five public note salts carry the result. A derived mask fills the three spare bits.
HKDF-SHA-256 binds the key, nonce, and authenticated data to the chain, pool, directional
channel key, token, and physical frame start. Set `wire_version` to `v2` only for an old
counterparty. Existing channel records never change version implicitly.

Wire v3 uses framed physical note indices. Offer and counter frames use five notes.
Acceptance frames use five data notes and one payment note. A new deal can start after a
settlement. Exact and change settlements both create a seventh payer-owned change note.

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
- `grant_viewing_key` is the intentional exception. On wire v3 it exports native subkeys and
  exact STRK20 read capabilities for one deal, encrypted to the grantee's registered pool
  key until an explicit expiry. It exports no parent channel key. Historical wire-v1 and
  wire-v2 grants remain broader bearer secrets.

## Current MVP limits

- One configured token per client instance.
- Settlement selects unspent notes that cover the offer amount and returns change to the
  payer. Wire v3 always writes the change frame, including when the change amount is zero.
- Reverse directional channels are paired by excluding keys already claimed in local state.
  An ambiguous match fails rather than guessing.
- Channel state records the last accepted write block. A dependent write waits until that
  block is visible at the `head - 10` proof anchor; settlement discovers candidate inputs at
  that same historical anchor, so it cannot select a fresh note the proof cannot observe.
- Every chain write carries a caller-supplied operation ID. Rust binds the ID to the
  canonical request before proving or submission. The journal stores signed transaction
  bytes and the hash before submission. `reconcile` is read-only. `resume_operation`
  performs an exact resubmission or a proven-dead rebuild under the original ID.
- Historical wire-v1 and wire-v2 channels remain readable. Wire v1 is read-only.
- Wire-v3 repeat deals and recipient-bound per-deal disclosure are implemented. The client
  rejects the old whole-channel grant on v3.
- The Python SDK and MCP server speak CLI protocol 4 and default newly opened channels to
  wire v3.
- Local fault tests cover every durable write boundary. A clean local wheel installation
  completed exact resubmission and expired-proof rebuild on Sepolia on 2026-08-27. See
  `docs/runs/2026-08-27-packaged-recovery-canary.md`.
