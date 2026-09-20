# Metropolis M0 Baseline

Date: 2026-09-20. Outcome: local M0 verification complete.
This records repository and offline Rust evidence. It does not establish a live Monad backend or a remotely executed CI run.

## Checkout and toolchain

| Item | Observed value |
|---|---|
| Working directory | `/Users/odinson/Developer/erebus` |
| Branch | `metropolis` |
| Source HEAD | `df86407ba72b542f5d3a851d14c1d0ce8515d1e5` |
| Protected local main | `daf33efc464294a2516700cb066a98214e921a39` |
| Platform | Darwin arm64 |
| Rust | `rustc 1.96.0 (ac68faa20 2026-05-25)` |
| Cargo | `cargo 1.96.0 (30a34c682 2026-05-25)` |
| Rust package | `erebus-sdk 0.3.0`, edition 2021 |
| Cargo.lock SHA-256 | `ef686af9742ace528186e84c0b374d276c47788fae5876fd452ba2e1e98933c0` |

Starting edits: README, Metropolis architecture/demo docs, and an untracked Metropolis roadmap.
Those edits were preserved. No commits, pushes, tags, workflow dispatches, or deployments were performed.
CI follows `stable`, so the recorded local toolchain is evidence for this run, not a pinned minimum Rust version.
Cargo.lock pins the dependency resolution. Notable direct dependencies include Starknet types/crypto, Tokio, reqwest, AES-GCM-SIV, HKDF, and fs2.
No new runtime dependencies were introduced for M0.

## Rust verification

Commands ran from `sdk/rs` with the existing lockfile and local dependency cache.

| Command | Result |
|---|---|
| `cargo fmt --check` | Pass, exit 0 |
| `cargo test --all-targets` | Pass, exit 0; seven explicitly ignored live tests not run |
| `cargo clippy --all-targets -- -D warnings` | Pass, exit 0 |
| `RUSTDOCFLAGS='-D warnings' cargo doc --no-deps` | Pass, exit 0 |
| `cargo build --bin erebus-cli` | Pass, exit 0 |

Ignored cases are two shared-prover calls, one signer rotation, three mainnet registration probes, and one mainnet screening probe.
They require deliberate live infrastructure or transaction execution. No live funds or external proving service were used.
The local fingerprint regression tests ran normally. The old CI comment describing that target as ignored was stale.
The test baseline includes settlement, negotiation, codecs, journal, recovery, disclosure, CLI, and mocked execution/prover behavior.
Full Python, TypeScript, Cairo, Linux, and hosted GitHub CI runs are not asserted by this Rust baseline.

## Settlement enforcement map

| Stage and source | Enforcement today | Boundary for the EVM work |
|---|---|---|
| `client.rs::begin_operation` | Binds operation ID to intent, holds a journal lease, reconciles prior effects | Local idempotency is distinct from contract replay protection |
| `client.rs::accept_and_settle` | Verifies local ownership/scope, synchronizes offers, selects notes, builds acceptance and change | Currently mixes orchestration and STRK20 data access |
| `negotiation.rs::OfferBook::check_acceptable` | Rejects own offers, wrong message kinds, expired offers, and already-settled deals | Client policy, bypassable by a custom client |
| `channel.rs::accept_and_settle_with_change` | Rejects amount mismatch, output collisions, invalid wire, and missing v3 change | Amount equality is Rust-side, not an Erebus pool-proof predicate |
| `action_set.rs` and `channel.rs` | Order input consumption and acceptance/payment/change creation in one action set | Atomic grouping does not prove business meaning |
| `execution.rs::Executor::execute` | Same-block preflight, proving, byte-for-byte comparison of returned actions | Prover and preflight RPC receive pool-key material |
| `execution.rs::finish_proven` | Rejects stale proof, estimates transaction, signs, persists hash before broadcast, verifies receipt | A submission timeout requires reconciliation |
| Upstream `privacy.cairo::use_note` | Verifies note ownership/existence and derives a spend nullifier | Note spend protection is distinct from a once-only Erebus deal |
| Upstream client action execution | Accounts for token inputs/outputs and asserts valid balances | Proves pool transition rules, not negotiated service terms |
| Upstream `apply_actions` / `validate_proof` | Verifies proof facts, binds actions and pool address, checks proof age, applies writes atomically | Pool acceptance does not verify bilateral Erebus consent or business expiry |
| `disclosure.rs` | Opens scoped records and distinguishes agreement from payment evidence | New offchain transcripts need separate storage and authorization evidence |

Rust sources are under [`sdk/rs/src`](../sdk/rs/src).
Upstream source was inspected in the sibling `starknet-privacy` checkout at `3dfe66fe2b59d7b95709ec719547fa88b8ef63f9`.
The inspected file was `packages/privacy/src/privacy.cairo`, including `use_note` and `apply_actions`.
The sibling checkout has unrelated test edits. Its inspected `privacy.cairo` file was unchanged.
These are source observations against the pinned upstream revision, not live bytecode verification.

## Workflow isolation

| Workflow | Trigger and outcome after M0 |
|---|---|
| `ci.yml` | Pushes to main or metropolis, pull requests, manual dispatch. Read-only contents permission; no publication jobs |
| `wheels.yml` | Version tags or manual dispatch. Gate permits main or version tags, not manual metropolis packaging |
| `canary.yml` | Successful Wheels run or manual dispatch. Artifact installation only, no publishing permissions |
| `published-canary.yml` | Published release or manual dispatch. Index publication permits release events or main, not metropolis dispatch |

All four YAML files parsed locally. Assertions verified branch triggers, CI permissions, and exact publication guards.
The focused release-readiness suite passed: `uv run pytest -q scripts/tests/test_release_readiness.py`, 12 tests.
Branch pushes cannot trigger Wheels, Canary, or index publication.
Version tags and deliberate release publication remain separate privileged operations; these guards do not establish tag ancestry or repository branch protection.
No remote branch-protection or external hosting settings were changed or verified.
CI execution requires a later authorized push. No GitHub Actions success is claimed here.

## Decisions and acceptance

The [M0 decision record](metropolis-decisions.md) defines settlement modes, deal-wide replay, authorization roles, compatibility, custody, retention, and operating responsibilities.
It also specifies the external fresh-environment integration and self-hosting acceptance scenarios.
Exact encoding and cryptographic suite are M1/M4 implementation gates. M0 does not freeze unsupported cryptographic choices.
The architecture now agrees that proof generation is backend-internal and alternative privacy mechanisms require explicit trust declarations.
The demo distinguishes public payment binding from confidential payment binding.

## Known limits and follow-up

- The README already links twice to missing `docs/evidence.md`. New Metropolis links resolve; the older documentation defect remains recorded.
- Existing architecture links to historical docs do not certify current deployed behavior.
- Live EVM feasibility, end-user packaging, self-hosting, and external adoption remain later milestone gates.
- No external developer acceptance run has occurred during M0.

Next: M1 canonical agreement and capability specification, alongside M4 proof/pool feasibility.
