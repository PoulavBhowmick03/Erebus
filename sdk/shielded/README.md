# Experimental shielded EVM client

This crate contains local proof generation, an encrypted note wallet, a public event
index, a journaled EVM submission path, and an optional indexer service. It is not an
installable settlement backend.

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

To run the coordinated funded Anvil check from a fresh local M5 setup:

```bash
cd circuits/m5
EREBUS_M5_NATIVE_PROOF=1 EREBUS_M6_COORDINATED=1 npm run prototype
```

This reserves a funded note in the encrypted wallet, generates its proof locally, and
persists the coordinator intent, signing plan, and signer nonce claim. A restart restores
the prepared proof and identical signed bytes without reproving. The example broadcasts
the pool call and recovers the spent note, change, and payment accounting from an explicit
finalized anchor. Anvil mines extra empty blocks to advance its finalized tag.
This is not an installed operator command or a production privacy claim.

To include the funded durable-write fault matrix, add `EREBUS_M6_MATRIX=1` to that
command. Each trial starts a fresh local Anvil fork at the same funded block and
injects one discovered write failure. Recovery checks one payment, one consumed
signer nonce, wallet change, and committed accounting. Initialization and proof
generation failures are separate checks. The runner reuses a locally generated
proof only for identical deployment, terms, and note state.

## Paired RPC recovery

`ShieldedChain::reconcile` requires two providers before changing wallet state,
accounting, or signer claims. Use separate public index caches and independently
operated providers for the same deployment. Never share wallet secrets with either.
`reconcile_agreed` is a compatibility name for the same path.
`reconcile_single_provider` is an explicit trusted-RPC experiment, not the operator default.

The paired path requires matching head and finalized anchors, deal evidence, and
finalized signer nonce evidence. A timeout or disagreement leaves reservations
held. Finalized wallet restoration also compares both complete pool histories before changing notes.
Honest providers at different heights can fail this check. Two endpoint URLs do not
prove independence, honesty, or consensus. The owner selected this operator default on 2026-10-01.

The funded local example exercises paired recovery and peer timeouts. Its positive
pair shares an Anvil upstream, so it tests the checks, not provider independence.

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
zero-value change output. `prepare_coordinated_transfer` stores the change opening
and input reservation before proving. `ShieldedChain::reconcile` replays finalized
pool evidence into the wallet before updating coordinator payment accounting.
Async callers must run the synchronous prover in a blocking context.

## Selected-Deal Payment Verification

`disclosure::verify_shielded_payment` verifies an opened suite-2 agreement against paired
finalized pool observations. It replays the selected transcript, checks both authorizations,
and derives the settlement identity from the signed opening. The auditor supplies two RPCs
and separate public caches; no wallet, spend key, or proving artifacts are needed.
Only public chain data goes to the RPCs. Incomplete scans return `HistoryPending` through
the observation error; retry with the same caches rather than treating the deal as unpaid.

This opened-evidence helper is a payment checker, not a full grant verifier.
The `erebus-shielded-disclosure` command first authenticates a version-2 grant signed directly
by a suite-2 participant. It then verifies payment through paired public observations.
The funded local harness passes through separate issuer and auditor CLI processes and MCP,
with participant storage unavailable during verification. The auditor needs no note wallet or prover.
The local RPC fixtures also pass. These checks still require trusted deployment code and verifying keys;
matching provider responses do not authenticate contract code or prove provider independence.
See the [disclosure section](../../docs/metropolis-operations.md#6-disclosure-and-auditor-operations) and the [status decisions appendix](../../docs/metropolis-status.md#decisions-appendix).
