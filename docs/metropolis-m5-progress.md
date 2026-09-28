# Metropolis M5 Progress: Funded Local Prototype

Updated: 2026-09-28. Branch: `metropolis`. M5 is **not complete**.

## Verified locally

- `sdk/core/src/shielded.rs` maps the fixed suite-2 agreement shape to circomlib-compatible
  Poseidon fields. A pinned vector produced by the M4 JS/Circom runner matches Rust deal
  commitments, deal nullifiers, and buyer/seller authorization messages. `sdk/core` now
  validates suite-2 terms in shielded mode only. Only the experimental `sdk/shielded`
  backend advertises suite 2; the EVM and `sdk/rs` backends still reject it at selection.
- Transcript hashing is no longer the agreement suite's job. The transport uses keccak256
  under its own `TRANSCRIPT_HASH_VERSION = 1`, whatever the suite. A root KAT
  (`cd876d46…`) matches the pre-split suite-1 code, so existing transcripts are unchanged.
- The same Rust module now computes M5 spend tags, note commitments, note nullifiers,
  Poseidon tree parents, and depth-20 membership roots. The funded JS/Anvil runner pins
  the input and recipient note openings plus both roots in a second fixture. Rust checks
  those values and rejects malformed field elements, zero spend secrets, and invalid
  tree indices. It also derives the expected recipient payment note directly from the
  accepted deal and matches the M4 circuit vector.
- `ErebusShieldedPool` holds one ERC-20 asset, checks exact deposit and withdrawal balance
  changes, maintains a depth-20 Poseidon tree, accepts prior roots, consumes deal and note
  nullifiers, and records two outputs atomically with proof verification.
- Deposit, transfer, and withdrawal circuits compile. The local `circuits/m5` runner generated
  three test keys, deployed their verifiers and the pool on Anvil, and completed a funded
  150 -> 70 payment + 80 change -> 70 withdrawal flow. The recipient reconstructed the
  payment output from its secret and agreement fields. Replay, public-input mutations,
  duplicate note insertion, fee-on-transfer deposit, and fee-on-transfer withdrawal
  were rejected. The runner
  derives its service digest from the pinned M1 Rust agreement vector; changing that
  digest or the seller signature invalidates the transfer witness.
- Rust derives and checks suite-2 BabyJubJub role signatures against pinned JS vectors.
  `verify_agreement` checks both roles, the commitment, and expiry for the fixed suite-2
  shape; it is not yet the general agreement backend. The experimental `sdk/shielded`
  crate builds deposit, transfer, and withdrawal proofs locally from the runner's test
  witnesses and hash-pinned artifacts. It checks public inputs before proving and verifies
  the resulting proof against the zkey verification key. CI runs this test after Anvil.
- Rust builds deposit, transfer, and withdrawal witnesses from typed note and agreement
  inputs. Each matches the independent JavaScript witness field-for-field. The transfer
  builder can select a spendable note from a local wallet and derive its current-root
  Merkle path from verified public events. It derives the buyer's change tag from
  a local spend secret, rather than accepting an unbacked destination tag. A helper
  reconstructs the buyer's change opening for wallet backup, and the witness test
  checks that the wallet recognizes it after an insertion. These
  builders are not yet connected to
  accepted-session state or transaction submission.
- `sdk/shielded::prepare_transfer` builds the witness from a wallet note, proves locally,
  and ABI-encodes `transferPrivate` calldata itself. `validate_prepared` checks the
  selector, the length, and the chain/pool/version/asset/Cdeal/Ndeal public inputs against
  the configured context. It does not verify the proof. In the second funded Anvil run,
  ethers decodes those Rust-encoded bytes, public inputs match the JavaScript witness,
  the deployed verifier accepts the proof, and the pool settles it. Re-run 2026-09-28 with
  unchanged artifact hashes.
- `sdk/shielded` has an encrypted, file-backed note wallet with exclusive updates,
  atomic writes, chain-and-pool-bound associated data, reservations, and reorg rewind.
  The seller can reconstruct its payment note from accepted terms and its own spend
  secret. The funded fixture checks encrypted restart, event replay, and recipient
  discovery. Other tests cover wrong-key/domain rejection, corruption, and reorgs.
- A public pool-event index verifies insertion indices, Poseidon roots, and block
  linkage, rejects duplicate commitments and spend nullifiers, and derives inclusion
  paths before replaying relevant notes into the encrypted wallet. A read-only
  JSON-RPC reader checks chain ID, the pool's stored root and leaf count, and the
  three event types. Sync commits only after the whole batch passes. A deployment-bound
  public cache persists verified blocks and rebuilds the tree on load. The funded
  Anvil run restarts the scanner from that cache and matches all three leaves.
- `recover_wallet` replays a caller-selected confirmed prefix into the encrypted
  seller wallet. A local run checks spendable before withdrawal, consumed after it,
  spendable again after a reverted block, and consumed after a replacement withdrawal.
  It repeats recovery through a second local Anvil RPC fork. This is a shallow local
  reorg rehearsal, not a Monad finality test.
- The optional `erebus_pool_indexer` binary publishes bounded public block pages and
  health status from the same verified cache. The local funded runner starts it on a
  second RPC endpoint and checks authentication, root, and insertion count. Hosted
  and self-hosted use the same binary; neither has an external integration run yet.
- Direct Foundry pool state tests cover replay, invalid proof rejection, exact token
  deltas, failed-transfer atomicity, an old-root spend after multiple deposits, and
  256 fuzzed deposit/withdrawal amounts. They
  use a permissive mock verifier, so they do not establish proof soundness.
- The transfer circuit rejects mutations of both authorizations, agreed amount,
  service digest, recipient tag, input secret, change amount, and membership path.

These are local tests with deterministic secrets and **known, single-contributor setup
entropy**. The generated keys and verifiers cannot protect real value. No Monad RPC or
testnet transaction was used. The runner still constructs its demo witness in JavaScript;
Rust independently reconstructs all three test inputs, but the prover is not yet wired
to an EVM backend or operator-facing MCP flow. The new wallet encrypts its own snapshot,
but witness JSON and deterministic secrets are still present in local test files and
must never be used with real funds. The public cache and optional indexer service
are now persistent, but a hosted endpoint is not authoritative: root and leaf-count
checks cannot detect an omitted spend event that leaves the tree unchanged. Clients
  still need independent chain evidence and M6 reconciliation.
The wallet does not yet bind its ciphertext to the public cache's deployment-block
hash. That domain choice needs review before the wallet is packaged for operators.
The new BabyJubJub helper dependency is exercised against pinned JS vectors, but it has
not had a project security review.
The prior prototype npm dependency audit reported 20 vulnerabilities, including
5 high-severity findings; this toolchain requires review before packaging.

## Still required for the M5 gate

- Connect the Rust builders and encrypted wallet to actual accepted-session state,
  durable operation reservations, including persistence of the buyer's change opening
  before submission, and the EVM submission backend. The installed EVM and `sdk/rs`
  backends still do not advertise suite 2.
- Package the local prover into the SDK with verified released artifacts and no witness
  transmission to relay/indexing services. The current test setup is not releasable.
- Rehearse the public indexer and wallet across longer reorgs, independent providers,
  archive-RPC failures, cache corruption, and external hosted/self-hosted deployments.
  A local Anvil fork proves restart and endpoint replacement only in a controlled run.
- Add independent conservation and direct contract/circuit adversarial tests, more than one
  deposit/note pattern, and bounded root-history and lifecycle decisions.
- Validate fees, recipient privacy, gas, and full flow on Monad testnet. Publish a reviewed
  setup and installation process before any real-value use.

The M5 done criterion in the roadmap remains unchecked. An Anvil script succeeding is not an
independent developer integrating Erebus from versioned packages.
