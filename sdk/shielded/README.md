# Experimental shielded EVM client

This crate contains local proof generation, an encrypted note wallet, a public event
index, and an optional indexer service. It is not an installable settlement backend.
Suite 2 is still rejected by the general `erebus-core` agreement selector.

The current Groth16 artifacts use known test entropy. Never use these verifiers,
keys, fixture secrets, or the funded runner with real value.

## Local checks

From the repository root:

```bash
cargo test --manifest-path sdk/shielded/Cargo.toml --locked --all-targets
cargo test --manifest-path sdk/shielded/Cargo.toml --locked --test witness -- --ignored
cargo test --manifest-path sdk/shielded/Cargo.toml --locked --test local_proof -- --ignored
```

The ignored tests require `circuits/m5/build/` from `npm run prototype` in
`circuits/m5`. To run the funded Anvil flow with a Rust transfer proof, wallet
recovery, reorg rehearsal, and the public service:

```bash
cd circuits/m5
EREBUS_M5_NATIVE_PROOF=1 EREBUS_M5_RUST_SCAN=1 npm run prototype
```

The runner creates deterministic secrets under ignored `build/`. It also starts a
second local Anvil endpoint, restarts the wallet reader, and checks the indexer API.
It does not use Monad testnet.

## Public indexer service

`erebus_pool_indexer` runs the same way when hosted or self-hosted. Set all required
deployment values before starting it. The deployment hash is the **block hash** of
the pool creation block, not the transaction hash.

```bash
export EREBUS_INDEXER_RPC=http://127.0.0.1:8545
export EREBUS_INDEXER_CHAIN_ID=<chain-id>
export EREBUS_INDEXER_POOL=0x<40-hex-digits>
export EREBUS_INDEXER_DEPLOYMENT_BLOCK=<block-number>
export EREBUS_INDEXER_DEPLOYMENT_HASH=0x<64-hex-digits>
export EREBUS_INDEXER_ROOT=/private/path/to/public-index
export EREBUS_INDEXER_CONFIRMATIONS=<operator-selected-depth>
export EREBUS_INDEXER_TOKEN=<long-random-token>
cargo run --manifest-path sdk/shielded/Cargo.toml --locked --bin erebus_pool_indexer
```

It binds to `127.0.0.1:8081` by default. `EREBUS_INDEXER_BIND` and
`EREBUS_INDEXER_PORT` change that address; a non-loopback bind requires a token.
`EREBUS_INDEXER_POLL_SECONDS` defaults to 5. Put TLS in front of a remotely
accessible deployment. The service provides `GET /healthz` and
`GET /v1/blocks?after=<number>&limit=<1..128>` with `Authorization: Bearer` when
a token is configured. Responses are capped at 1 MiB and 4,096 events.

The cache is public chain data, but it is bound to chain ID, pool, and deployment
block hash. Every load rebuilds the tree from cached events. On reorg, the scanner
rewinds the changed suffix and persists the replacement before publishing it.
To rebuild a corrupt cache, stop the service, retain it for diagnosis, configure a
new empty root directory with the same deployment identity, and start the service.
The upstream RPC must provide historical blocks, logs, and contract state back to
the deployment block; otherwise initial rebuild fails closed.

Clients must independently check block hashes and pool state against a chosen RPC.
The service does not make its JSON a cryptographic proof of complete logs. In
particular, root and leaf-count checks cannot detect a missing withdrawal or spend
event that did not change the tree. A faulty endpoint can delay or omit such data;
the contract still rejects a spent note at submission. A hosted indexer also sees
client IPs, request timing, and requested block ranges. It receives no note
openings, spending keys, plaintext agreements, or proof witnesses.

## Local wallet recovery

`recover_wallet` compares the wallet, cache, and RPC chain and pool identities. The
cache additionally pins the deployment block hash; the encrypted wallet does not.
Recovery scans only through `head - confirmations`, persists the public cache, then replays
that prefix into the encrypted wallet. A crash between the two writes is repaired
by repeating recovery. The caller supplies and backs up a 32-byte wallet key;
neither the indexer service nor the RPC receives it. A confirmation count is an
operator risk setting, not proof of finality. Wallet state may change after a
reorg, and submission still requires contract-side checks and M6 reconciliation.

The transfer witness derives its change destination tag from a locally held spend
secret. `ChangeNote::owned_note` reconstructs its wallet opening, except for a
zero-value change output. An operator must store the opening before submitting
the transfer; current test helpers do not provide the durable M6 operation that
makes this automatic.
