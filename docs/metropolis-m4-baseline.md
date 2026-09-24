# Metropolis M4 Baseline: Local Private-Transfer Proof

Recorded: 2026-09-24. Branch: `metropolis`. Decisions and exact schemas are in
[metropolis-m4-decisions.md](metropolis-m4-decisions.md).

## What passed locally

The [`circuits/m4`](../circuits/m4) runner compiles one combined agreement and transfer
circuit, generates a Groth16 proof locally, verifies it offchain and through a generated
Solidity verifier, then calls a local EVM harness. The harness checks the deployment and
expiry, consumes both the deal and note nullifiers, and records the signed payment and change
commitments in one transaction. The seller then reconstructs its output from its own secret
and agreed fields after a simulated restart. A second submission fails. Changed amount, recipient,
accepted root, buyer signature, change amount, expiry, and public payment output all fail.

The service digest in the test comes from `ServiceRecord::encode()` as pinned by the M1
agreement vector. The eleven public field elements are pinned in
[`circuits/m4/fixtures/public.json`](../circuits/m4/fixtures/public.json). The circuit uses
suite-2 semantics; the Rust core still rejects suite 2 until M5 adds a production mapping and
cross-language authorization vectors.

## One measured run

| Measurement | Result | Boundary |
|---|---:|---|
| Host | Apple M4 Pro, macOS arm64, Node 22.18.0 | Local machine |
| Circuit | 31,818 constraints; eleven public inputs; depth-four tree | Circom 2.2.3 |
| Witness plus proof | about 0.7 s | `snarkjs.groth16.fullProve`, one run |
| Peak Node RSS | about 1 GiB | Sampled every 10 ms during proof |
| Groth16 proof | 256 bytes | Eight 32-byte field coordinates, excluding calldata/public inputs |
| Standalone verifier transaction | about 285k gas | Explicit gas limit; trace contains 11 BN254 scalar multiplications and one pairing |
| Prototype settlement transaction | about 403k gas | Anvil 1.5.1, includes verifier, replay state, outputs, event |

The runner writes the exact per-run values and artifact hashes to ignored `build/metrics.json`
and `build/artifact-manifest.json`. These are measurements of a proof-only transition on
Anvil, with no token custody, deposit, tree update, or withdrawal. They do not predict the
gas or fee of the eventual Monad pool. Monad reprices BN254 operations relative to Ethereum
([official precompile schedule](https://docs.monad.xyz/developer-essentials/precompiles)),
and charges transaction gas limits rather than used gas
([official gas pricing](https://docs.monad.xyz/developer-essentials/gas-pricing)). A Monad
testnet estimate and actual receipt remain an M5/M8 gate.

The proof run peaked near 1 GiB RSS on the measured machine. Use a host with at least 4 GiB
available RAM for this test harness; that is an operational margin, not a measured minimum.
The Linux x86-64 CI job is configured to rebuild the circuit and run the same proof and Anvil
transition. CI results must be checked before claiming that Linux has passed remotely.

The generated verifier returns `false` when an internal precompile runs out of gas. A bare
`eth_estimateGas` on that boolean function selected a roughly 42k-gas false-return path in
Anvil. That number is not verification cost. The harness requires `true`, so estimating its
whole `settle` call found the full execution. See [F46](friction.md). M5 gas planning must
simulate the complete checked transition and verify the return value.

## Artifact and security boundary

The package lock pins JS dependencies; CI pins the Circom source commit and Foundry release.
The local `npm audit` reports 20 advisories, including five high-severity advisories in
transitive prototype tooling. M5 must review or replace these dependencies before packaging
the prover for other users; M4 does not claim a production-safe prover distribution.
`npm run prototype` regenerates a local setup when the R1CS changes and checks SHA-256 hashes
of cached zkey, verification key, and verifier source before reuse. The generated manifest
names `m4-prototype-v1` and identifies one exact local verifier/key installation. A fresh
setup produces a different zkey even with fixed source and inputs, so these keys are not a
portable released artifact. The test setup is single-contributor and its entropy is in the
runner: it is not safe for funds.

For a testnet release, M5 must publish one reviewed, versioned artifact set and an offline
hash-verification/install path, plus the exact source commit, trusted setup transcript,
verification-key hash, verifier bytecode hash, and administration model. The M4 harness itself
does not custody ERC-20 tokens; a successful transaction must not be described as a private
payment. The proof establishes that the proposed combined predicate is satisfiable and can be
enforced atomically by an EVM contract.

## Remaining M5 work

- Implement the suite-2 map and signature verification in Rust and pin cross-language vectors.
- Build the real deposit/tree/root-history/vault transition, fee path, and withdrawal.
- Build local note scanning, backup and restore, recipient spending, and a durable wallet.
- Select production Merkle depth, prover packages, setup ceremony, artifact distribution,
  verifier administration, and testnet deployment.
- Test actual Monad verifier gas, end-to-end private value movement, and anonymity-set limits.
