# Metropolis M1 Baseline

Date: 2026-09-20. Outcome: local M1 verification complete after review corrections.
This records repository and offline Rust evidence. It does not establish a live Monad backend, an implemented shielded suite, or a remotely executed CI run.

## Checkout and toolchain

| Item | Observed value |
|---|---|
| Working directory | `/Users/odinson/Developer/erebus` |
| Branch | `metropolis` |
| Source HEAD | `05e6135` (merge of `origin/main` into `metropolis`) |
| Platform | Darwin arm64 |
| Rust | `rustc 1.96.0 (ac68faa20 2026-05-25)` |
| Rust packages | `erebus-sdk 0.3.0` (unchanged) and new `erebus-core 0.1.0` |
| `sdk/core/Cargo.lock` SHA-256 | `fb6cced151dbeff63216645ac3c61c8b13a0c176918aa69924cf7fbae4ac7f67` |
| `sdk/rs/Cargo.lock` SHA-256 | `e50a25cfca13b303a3c428a35d9c99f4cf5350723de5a77fe52625f68b20f539` |

All changes are uncommitted on `metropolis`. `main` is unchanged. Remote CI has not run for this work.

## What M1 added

| Path | Purpose |
|---|---|
| `sdk/core` (new crate) | Chain-neutral agreement core: canonical encoding, identifiers, deployment domain, service record, terms, suite registry, commitment, deal identity, authorizations, spending policy, settlement capability and receipt types |
| `sdk/core/src/encoding.rs` | Canonical writer and strict reader (big-endian fixed widths, `u16` length prefixes, tags, optional presence) |
| `sdk/core/src/ids.rs` | CAIP-2 namespaces, CAIP-19 assets, base units, bounded key/address/signature bytes |
| `sdk/core/src/terms.rs` | `AgreementTerms`, settlement mode, guarantee bitset, fee policy, validation, mode/guarantee consistency |
| `sdk/core/src/suite.rs` | Suite registry; suite 1 = keccak256 commitment + secp256k1 ECDSA authorization (low-`s`, 65-byte `r||s||v`) |
| `sdk/core/src/commitment.rs` | `Cdeal`, `Ndeal`, blinding |
| `sdk/core/src/auth.rs` | Role tags, authorization digest, expiry-aware and disclosure-time verification |
| `sdk/core/src/policy.rs` | Pure policy evaluation, denial reasons, reservation ledger and windows |
| `sdk/core/src/settlement.rs` | `BackendCapabilities`, selection that fails explicitly, prepared settlement, receipt with separate payment/delivery states, recovery mapping |
| `sdk/core/tests/` | Pinned vectors, truncation/trailing-byte rejection, replay/role/expiry/mutation cases |
| `sdk/rs/src/capabilities.rs` | Legacy STRK20 capabilities: hidden amount/recipient, no canonical suite, no proof-enforced agreement binding, hosted proving |
| `sdk/rs/src/strk20_settlement.rs` | Checked legacy adapter used by `Client::accept_and_settle`; rejects unsupported requirements before execution |
| `sdk/rs/tests/capabilities.rs` | Tests declaration and actual adapter rejection without operation records or network connections |
| `scripts/check_agreement_vectors.py` | Independent canonical-byte encoder using Python's standard library |
| `docs/metropolis-agreement.md` | Normative v1 specification: encoding, derivations, policy and backend semantics, M1 decisions |
| `.github/workflows/ci.yml` | `rust-core` job so the core crate's tests run in CI independently |

The existing settlement body moved unchanged into the crate-private `Client::settle_strk20` method.
The public method delegates through the checked legacy adapter. Historical wire formats, state records, and CLI request formats remain unchanged.
The CLI maps the new capability error to the existing non-retryable `INVALID_REQUEST` code.

## Review corrections

| Finding | Correction and evidence |
|---|---|
| Supplied terms were not checked against the signed commitment | Both authorization APIs now require the blinding and check the opening. Regression tests retain the original commitment while mutating terms |
| Fees bypassed spending limits | Evaluation and `reserve_agreement` count payment plus fee with checked arithmetic. Tests cover both limits, overflow, and uncertain reservations |
| Suite 1 accepted shielded mode | Encoding, decoding, commitments, and selection reject it. The original shielded fixture is retained as a rejection vector |
| STRK20 isolation was only a declaration | The real public settlement method now enters the checked adapter before legacy execution |
| Identifier rules differed from CAIP syntax | Namespace/asset bounds now follow CAIP-2 and CAIP-19 asset types; backend address normalization remains M3 work |

Additional tests cover zero revisions, mismatched asset/deployment chains, local-proving requirements, and rolling-window endpoints.
Prepared settlements and receipts carry the deal nullifier explicitly.

## Verification

Commands ran from `sdk/core` and `sdk/rs` with the existing lockfiles.

| Command | `sdk/core` | `sdk/rs` |
|---|---|---|
| `cargo fmt --check` | Pass | Pass |
| `cargo clippy --all-targets -- -D warnings` | Pass | Pass |
| `cargo test --all-targets --locked` | 76 passed, 0 failed, 1 ignored | 368 passed, 0 failed, 7 ignored |
| `RUSTDOCFLAGS='-D warnings' cargo doc --no-deps` | Pass | Pass |
| `cargo build --bin erebus-cli` | n/a | Pass |

The ignored `sdk/core` test is the vector regenerator (`regenerate_vectors`). The seven ignored `sdk/rs` tests are the same live prover, signer-rotation, and mainnet probes recorded in the M0 baseline; no live infrastructure was used.

Chain-neutrality evidence: `cargo tree -p erebus-core` contains no `starknet` or `felt` crate. The core manifest lists only `thiserror`, `sha3`, and `k256` (plus their RustCrypto dependencies). Nothing in the core API accepts or returns a Starknet type, and no code branches on a chain family name; a chain namespace is opaque data.

## Test evidence for the M1 done criteria

| Criterion | Evidence |
|---|---|
| Shared protocol logic has no Starknet `Felt` dependency and no network-name conditionals | Separate crate; `cargo tree` shows no Starknet dependency; namespaces are opaque strings |
| Existing STRK20 behavior remains covered | `sdk/rs` suite unchanged and green; 7 live tests still ignored exactly as at M0 |
| Unsupported privacy requirements fail explicitly | Selection rejects unsupported suites/modes and local-proving requirements; actual legacy adapter tests check rejection before effects |
| Service-record mutations invalidate authorization | `original_commitment_cannot_authorize_changed_terms` covers every service field for both roles, without replacing the signed commitment |
| Policy-denial cases have tests independent of agent prompts | Pure policy tests include fees, overflow, reservation totals, and rolling-window boundaries |

## Vectors and external grounding

The fixture retains three valid public-bound vectors and one shielded rejection vector. The valid bytes, commitments, digests, and signatures are unchanged.
The Rust suite checks cryptographic values for the valid vectors and rejects the shielded vector during semantic decoding.

Suite 1 is checked against external keccak256 vectors and the EIP-155 signature example in `sdk/core/src/suite.rs`.
`python3 scripts/check_agreement_vectors.py` independently reconstructs all four canonical encodings from the specification. All four match.
This check covers encoding, not an independent cryptographic verifier. The complete disclosure verifier remains M7 work.
CI runs this check and rejects Starknet/Felt dependencies in the core crate.

## Known limits and follow-up

- Suite 1 is the public-bound path. No shielded suite is implemented; an agreement naming one fails explicitly. The suite decision is the M1/M4 joint gate, and the feasibility research is recorded in [metropolis-m4-feasibility.md](metropolis-m4-feasibility.md).
- No proof relation, EVM contract/adapter, canonical coordinator, offchain transport, or new disclosure package exists yet. Those remain later milestones.
- The STRK20 adapter supports legacy requests only. It does not translate canonical agreements or upgrade the existing pool proof.
- Scoped disclosure still uses the wire-v3 grant API. It is not promised across all historical wire versions by the legacy adapter.
- Payment and delivery states exist as types; no service adapter can set delivery yet (M8).
- OpenCode's earlier Python baseline recorded 238 passed, 2 skipped, and 3 failures in `scripts/tests/test_demo.py` from missing `web/components/Evidence.tsx`. The full Python suite was not rerun for these Rust corrections; no repository-wide green claim is made.
- The pre-existing README link to `docs/evidence.md` is broken in this checkout. Metropolis document links pass; the README is unchanged by this work.
- The M4 memo is a research draft for owner review. It selects no primitive and no prototype has run.

Next: M2 transport and M3 public-bound execution can use the reviewed suite-1 encoding.
M4 still owes primitive selection and a measured private agreement/payment proof prototype. M1 completion does not close that goal.
