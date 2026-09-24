# M4 private-transfer proof prototype

This directory proves one agreement-bound, one-input shielded transfer. It is an M4 research
artifact, not a token pool or a release artifact. The harness verifies the proof and consumes
nullifiers atomically on a local Anvil chain; it holds no token funds. M5 adds custody, a live
tree, note scanning, withdrawal, and the production artifact lifecycle.

## Reproduce

Requirements: macOS arm64 or Linux x86-64 with Rust, Node 22, Anvil 1.5.1, and Circom 2.2.3.
The Circom source tag is pinned to commit `ad44e915a12bb047b05745c2884aad9cc8326bc6`.

```bash
git clone --depth 1 --branch v2.2.3 https://github.com/iden3/circom.git /tmp/erebus-circom
test "$(git -C /tmp/erebus-circom rev-parse HEAD)" = ad44e915a12bb047b05745c2884aad9cc8326bc6
cargo install --path /tmp/erebus-circom/circom --locked
cd circuits/m4
npm ci
npm run prototype
```

`npm run prototype` compiles the circuit, creates a single-contributor test setup if needed,
proves a transfer locally, deploys a generated verifier and enforcement harness to Anvil,
settles once, and runs replay and mutation checks. It writes `build/metrics.json`,
`build/public.json`, `build/proof.json`, and generated proving artifacts. `build/` is ignored.
The pinned public vector is `fixtures/public.json`. The M1 service-record encoding is checked
against the Rust agreement fixture before the proof is generated.

The local setup entropy and deterministic test secrets make these artifacts unsuitable for
real value. Never deploy this verifier or use this `.zkey` with funds. Regenerating after a
circuit change creates a new verifier and invalidates previous proofs.

See [M4 decisions](../../docs/metropolis-m4-decisions.md) for the statement and
[M4 baseline](../../docs/metropolis-m4-baseline.md) for measured evidence.
