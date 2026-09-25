# Metropolis M5 Progress: Funded Local Prototype

Recorded: 2026-09-25. Branch: `metropolis`. M5 is **not complete**.

## Verified locally

- `sdk/core/src/shielded.rs` maps the fixed suite-2 agreement shape to circomlib-compatible
  Poseidon fields. A pinned vector produced by the M4 JS/Circom runner matches Rust deal
  commitments, deal nullifiers, and buyer/seller authorization messages. Suite 2 remains
  disabled for general agreement validation and settlement selection.
- `ErebusShieldedPool` holds one ERC-20 asset, checks exact deposit and withdrawal balance
  changes, maintains a depth-20 Poseidon tree, accepts prior roots, consumes deal and note
  nullifiers, and records two outputs atomically with proof verification.
- Deposit, transfer, and withdrawal circuits compile. The local `circuits/m5` runner generated
  three test keys, deployed their verifiers and the pool on Anvil, and completed a funded
  150 -> 70 payment + 80 change -> 70 withdrawal flow. The recipient reconstructed the
  payment output from its secret and agreement fields. Replay, public-input mutations,
  duplicate note insertion, and a fee-on-transfer deposit were rejected. The runner
  derives its service digest from the pinned M1 Rust agreement vector; changing that
  digest or the seller signature invalidates the transfer witness.

These are local tests with deterministic secrets and **known, single-contributor setup
entropy**. The generated keys and verifiers cannot protect real value. No Monad RPC or
testnet transaction was used. The runner constructs its test witness in JavaScript; it is
not yet the Rust SDK or an operator-facing MCP flow.
The prototype npm dependency audit currently reports 20 vulnerabilities, including
5 high-severity findings; this toolchain requires review before packaging.

## Still required for the M5 gate

- Implement and test suite-2 BabyJubJub signatures in Rust, including validation, role
  authorization, and canonical byte encoding. Then connect the canonical agreement package
  to the exact circuit witness instead of constructing test terms inside the runner.
- Build a local SDK prover with verified released artifacts and no witness transmission to
  relay/indexing services. The current setup is not releasable.
- Build encrypted, persistent note state; scan public events; select spendable notes; restore
  a recipient after process restart; and reconcile reorgs and endpoint switching.
- Add independent conservation and direct contract/circuit adversarial tests, more than one
  deposit/note pattern, and bounded root-history and lifecycle decisions.
- Validate fees, recipient privacy, gas, and full flow on Monad testnet. Publish a reviewed
  setup and installation process before any real-value use.

The M5 done criterion in the roadmap remains unchecked. An Anvil script succeeding is not an
independent developer integrating Erebus from versioned packages.
