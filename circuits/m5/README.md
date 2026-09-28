# M5 experimental funded pool

This is a local, test-only proof of a funded shielded transition. It is not an audited pool,
an installable Erebus backend, or a Monad deployment. The setup uses known test entropy.
Never deploy these generated verifiers or keys with real funds.

## Reproduce

Requirements: Node 22, Anvil 1.5.1, and Circom 2.2.3 pinned to source commit
`ad44e915a12bb047b05745c2884aad9cc8326bc6`.

```bash
cd circuits/m5
npm ci
npm run prototype
```

The first run creates a local powers-of-tau file and three circuit-specific test keys. It can
take several minutes; subsequent runs reuse hash-checked artifacts under ignored `build/`.
The script compiles all three circuits, deploys generated verifiers, Poseidon, a mock token,
and the pool to local Anvil, then executes:

```text
deposit 150 -> private agreement-bound transfer 70 + change 80 -> seller discovers 70 -> withdraw 70
```

The runner checks token balances, Merkle roots, note and deal replay, proof input mutation,
and rejection of a fee-on-transfer deposit. Results and artifact hashes are in
`build/run.json` and `build/artifact-manifest.json`.
It also pins `sdk/core/tests/fixtures/m5-note-vector.json`. The Rust core independently
recomputes both note openings, their spend nullifiers, and the deposit/transfer roots.
The pinned fixture exposes deterministic spend secrets for test reproducibility. Never use
these note openings or secrets outside local tests.

The runner also writes ignored `deposit-input.json`, `transfer-input.json`, and
`withdraw-input.json` files under `build/`. The experimental Rust prover reads these
test witnesses and the hash-pinned WASM, R1CS, and zkey files. From the repository root:

```bash
cargo test --manifest-path sdk/shielded/Cargo.toml --locked --test local_proof -- --ignored
```

The Rust test generates and self-verifies all three proofs, and rejects changed public
inputs before proving. A second test builds the transfer witness from Erebus Rust
agreement and note types, then checks every field against the independent JS input:

```bash
cargo test --manifest-path sdk/shielded/Cargo.toml --locked --test witness -- --ignored
```

The witness test now derives deposit and withdrawal inputs from local note openings,
and the transfer input from a wallet note and a locally verified Merkle path. It also
checks recipient note reconstruction, encrypted wallet restart, and discovery after
replaying verified public events. Both tests use ignored local files containing
deterministic test secrets.

To settle the funded flow with the Rust-generated transfer proof and check the real
Anvil logs with the Rust pool indexer:

```bash
cd circuits/m5
EREBUS_M5_NATIVE_PROOF=1 EREBUS_M5_RUST_SCAN=1 npm run prototype
```

The scan runs before Anvil exits. It verifies the chain ID, pool event shapes,
insertion roots, final root, and paths for all three inserted leaves. The extended
run restarts a persisted public cache and encrypted seller wallet, rehearses a
withdrawal reorg, switches to a second local RPC fork, and checks the optional
public indexer service. See [`sdk/shielded/README.md`](../../sdk/shielded/README.md)
for service configuration and trust limits. None of this is a Monad deployment.

The contract uses one ERC-20 asset, a depth-20 append-only tree, and unbounded known-root
history. Deposit and withdrawal amounts are public. Each private transfer publishes a deal
commitment, nullifiers, and two output commitments in one transaction. Nothing here establishes
an anonymity set, recipient metadata privacy, safe setup, or production-grade wallet recovery.

The local script still constructs its demo witnesses in JavaScript. Rust independently
constructs all three matching inputs and can prove them locally; its transfer proof is
accepted by the deployed pool in the optional native run. M5 remains open until those
pieces are integrated with accepted sessions, a settlement coordinator, an installable
SDK, reviewed artifact distribution, and Monad testnet evidence. The optional
indexer service exists locally but has no independent external integration run.
The generated keys remain unsafe for real value.
