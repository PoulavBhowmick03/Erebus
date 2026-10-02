# Metropolis Implementation Roadmap

Written: 2026-09-20. Working branch: `metropolis`.
Target submission date: October 13, supplied by the project owner. Verify the portal cutoff and timezone before submission.
Status: M0-M4 locally complete at their stated scopes. M5 has a funded local prototype and is
incomplete. M6 implementation is locally complete and awaits owner review of its open decisions.
M7 is complete at its local scoped gate. M8-M9 are pending.
Unchecked items are not implemented or verified by this document.

This plan covers the complete Monad implementation: Rust core, private transport, agreements, settlement contracts, proving, recovery, disclosure, and agent integration.
Codex will perform implementation and verification in subsequent work sessions. The owner reviews protocol decisions and milestone results.
Each milestone ends with a reviewable diff, test evidence, and an update to this roadmap.

Related documents: [architecture](metropolis-architecture.md), [threat model](metropolis-threat-model.md), and [demo plan](metropolis-demo-plan.md).

## Product and release target

The Metropolis goal is a developer/agent product on Monad testnet, usable by an external developer or marketplace.
Users must be able to integrate agents, discover services, negotiate privately, settle, disclose a selected deal, and recover failures.
SDK, CLI, MCP tools, packages, and operational documentation form the product surface. A frontend is not a release requirement.
An internal demonstration alone does not satisfy the final product gate.

The planning default is a complete testnet developer release. Mainnet activation remains unresolved and requires a separate release decision.
Public-bound payment is an intermediate milestone, not a substitute for the shielded-payment criteria in M5 and M9.
Payment success and service delivery have separate states. Recovery of access issuance does not guarantee an uncooperative seller will deliver.

### Hybrid operating model

All services below are planned. Users retain custody on their own machines or servers.

| Component | Default operator | Access and responsibility |
|---|---|---|
| Agent policies and negotiation | Developer or marketplace | Its own terms, budgets, and counterparty permissions |
| Keys, payment authorization, and local prover | Developer or marketplace | Private keys and witnesses remain within its environment |
| Encrypted-message relay | Optional Erebus endpoint or self-hosted service | Ciphertext, routing metadata, message sizes, and timing |
| Transaction relayer | Optional Erebus endpoint or self-hosted service | Public proofs, transaction inputs, and submission metadata |
| Pool indexer | Optional Erebus endpoint or self-hosted service | Public chain events and potentially identifying query patterns |
| Settlement contracts | Monad testnet | Enforce settlement rules under an explicitly documented administration policy |

Hosted and self-hosted deployments use the same service implementations and versioned configuration.
The client encrypts messages before transmission, authorizes locally, and verifies settlement evidence.
Hosted services must not require spending keys, plaintext negotiation content, or plaintext proof witnesses.
The local proving requirement applies to the new shielded EVM backend. It does not change existing STRK20 infrastructure trust assumptions.

Self-custody does not hide network metadata or guarantee service availability.
Operators can delay messages and submissions, observe access patterns, or return incomplete index data.
Endpoint configuration, retry behavior, backups, and provider-switch recovery must be documented and tested.
Hosted access limits, retention, health monitoring, and abuse controls are explicit operating responsibilities.

## 1. Branch discipline

- Keep all implementation, configuration, tests, and documentation changes on `metropolis`.
- Before each work session, verify the current branch and working-tree status.
- Never commit, merge, rebase, reset, or push `main` as part of this work.
- Preserve unrelated work. Review branch differences before importing any change from another branch.
- Use explicit `metropolis` destinations for authorized pushes. Do not use an implicit push destination.
- Keep deployments separate from the existing Starknet release and its state directories.
- Record the source commit for each deployment and evidence run.

Planning baseline: `metropolis` at `df86407`, already two commits ahead of local `origin/metropolis`.
The local `main` tip at planning time was `daf33efc464294a2516700cb066a98214e921a39`.
These are local observations, not a fresh remote synchronization.

## 2. Target system

```mermaid
flowchart TD
    CFG[Startup configuration and peer settlement options] --> CTX[Fixed session context and required guarantees]
    CTX --> CORE[Rust core: negotiate, authorize, persist]
    CORE <--> MSG[Encrypted offchain transport and transcript storage]
    CORE --> AGR[Canonical agreement and blinded commitment]
    AGR --> COORD[Offchain settlement coordinator]
    COORD --> STARK[Existing STRK20 backend]
    COORD --> PUBLIC[EVM public-bound backend]
    COORD --> PRIVATE[EVM shielded backend and local prover]
    PUBLIC --> EVM[EVM chain adapter]
    PRIVATE --> EVM
    EVM --> MONAD[Monad contracts: evidence verification and atomic payment]
    MONAD --> RECEIPT[Verified receipt and recovery]
    RECEIPT --> DISC[Deal-scoped disclosure]
    RECEIPT --> SERVICE[Service access adapter]
```

The core uses backend capabilities. It does not branch on chain names throughout negotiation.
The session fixes the chain, asset, deployment, and required guarantees before authorization.
Proof generation belongs inside backends that require it. The common interface does not mandate a prover.
EVM chain access and settlement mechanisms remain separate modules. Monad is the first configured EVM deployment.

V1 provides private negotiation with contract-bound public payment. V2 adds shielded payment and private agreement verification.
Both are planned. V1 is a useful intermediate result, but it does not complete the shielded demo.
Payment finality does not guarantee delivery of an external service.

## 3. Current source map

These paths exist in the planning checkout. The extraction column describes proposed work, not existing abstractions.

| Layer | Current source | Planned treatment |
|---|---|---|
| Chain-neutral agreement core | `sdk/core` (added at M1) | Canonical encoding, commitments, authorizations, policy, and settlement capability types; must stay free of chain dependencies |
| Public API and orchestration | `sdk/rs/src/client.rs` | Separate protocol orchestration from STRK20 operations |
| Legacy settlement boundary | `sdk/rs/src/strk20_settlement.rs` (M1) | Check requirements before legacy settlement; reject canonical agreements instead of translating them silently |
| Negotiation state | `sdk/rs/src/negotiation.rs` | Preserve behavior tests, then remove wire-specific dependencies from shared logic |
| Channels and wire records | `channel.rs`, `subchannel.rs`, `wire.rs` under `sdk/rs/src/` | Retain STRK20 codecs and introduce a separately versioned offchain protocol |
| Agreement authorization | Currently distributed across client, wire, and transaction signing | Add explicit canonical agreements and bilateral authorization |
| Settlement action construction | `sdk/rs/src/channel.rs`, `action_set.rs`, `actions.rs` | Keep STRK20 action building within its backend |
| Proving and submission | `execution.rs`, `calldata.rs`, `prover.rs`, `rpc.rs`, `tx.rs` | Retain Starknet execution and add EVM execution separately |
| Keys and signatures | `keys.rs`, `signer.rs`, `signing.rs`, `hashes.rs` | Separate transport, agreement, spending, and transaction key roles |
| Durable state and recovery | `state.rs`, `operation.rs`, `journal.rs`, `reconcile.rs`, `resume.rs` | Preserve request binding and isolate state by backend and deployment |
| Note discovery | `read.rs`, `decrypt.rs`, `notecache.rs` | Keep STRK20 discovery and add EVM pool event scanning |
| Disclosure | `sdk/rs/src/disclosure.rs` | Add offchain transcript packages and independently verifiable payment linkage |
| CLI and onboarding | `sdk/rs/src/bin/erebus_cli.rs`, `onboarding/` | Add explicit backend configuration and versioned commands |
| Contracts | `contracts/README.md` describes Cairo conformance probes | Add an isolated EVM contract project |
| Agent integration | `sdk/py`, `mcp-server`, `agents` | Integrate after Rust API and settlement semantics stabilize |

Verified current boundary: `channel.rs::accept_and_settle_with_change` enforces amount equality in Rust.
The `negotiation.rs` module documents client-side expiry and acceptance rules.
A successful STRK20 proof does not itself establish the new agreement predicates proposed here.

## 4. Milestones

### M0. Baseline and implementation contracts

Dependencies: none. First implementation session.

- [x] Record branch, commit, toolchains, dependencies, and existing test outcomes.
- [x] Run existing offline Rust format, lint, test, and documentation commands from CI.
- [x] Add automatic CI coverage for `metropolis` without enabling release or deployment jobs.
- [x] Audit workflow triggers so branch pushes cannot publish the existing production release.
- [x] Trace `accept_and_settle` and record which layer enforces each invariant.
- [x] Write decision records for settlement modes, replay identity, authorization, and compatibility policy.
- [x] Correct contradictions between the architecture and demo documents before freezing interfaces.
- [x] Specify hosted and self-hosted service boundaries, supported environments, metadata retention, and operational ownership.
- [x] Define the fresh-environment external integration scenario and required evidence before implementation.

Done: reproducible baseline, recorded failures, branch CI, and a source-backed boundary map.
The baseline also records product acceptance criteria and the testnet operating model.
Do not infer that the current tests pass from historical evidence.

Completed evidence: [M0 baseline](metropolis-m0-baseline.md) and [decision record](metropolis-decisions.md).
Workflow changes are locally verified but uncommitted and unpushed. Remote CI has not run for these changes.

### M1. Canonical agreement and shared Rust core

Dependencies: M0.

- [x] Define chain namespaces, deployment domains, asset identifiers, integer amounts, and protocol versions.
- [x] Specify exact encoding, commitment blinding, role-specific authorizations, expiry, fees, and required guarantees.
- [x] Bind the settlement mode and required guarantees into the authorized agreement.
- [x] Define a canonical service record: purchased resource, quantity, access recipient, and fulfillment conditions.
- [x] Bind the service record to the agreement and distinguish payment status from delivery status.
- [x] Specify per-deal and aggregate spending limits, permitted assets, and counterparty restrictions below the agent model.
- [x] Define whether signed revisions share one consumed deal identity.
- [x] Define authorization expiry and cancellation behavior without implying that local cancellation revokes onchain permission.
- [x] Add deterministic encoding vectors, malformed-input tests, and cross-domain replay cases.
- [x] Check commitment openings during authorization, including mutations that retain the original commitment.
- [x] Count settlement fees in spending limits and reservations; reject arithmetic overflow.
- [x] Reject unsupported suite/mode combinations and hosted proving when local proving is required.
- [x] Cross-check canonical byte encoding with an independent Python implementation.
- [x] Introduce backend capability negotiation, prepared settlement, receipt, and recovery types.
- [x] Isolate existing STRK20 code behind its actual guarantees without changing historical state or wire formats.

Done: shared protocol logic has no Starknet `Felt` dependency and no network-name conditionals.
Existing STRK20 behavior remains covered. Unsupported privacy requirements fail explicitly.
Service-record mutations invalidate authorization. Policy-denial cases have tests independent of agent prompts.

Completed locally 2026-09-20: [agreement specification](metropolis-agreement.md) and [M1 baseline](metropolis-m1-baseline.md).
Review corrections include the checked legacy adapter and regression tests for the four reported M1 gaps.
Suite 1 (keccak256 and secp256k1) is the public-bound path only. M4 selected and prototyped a
field-native suite 2, which remains rejected by the Rust core until M5 implements its mapping.
M1 completion covers the shared core and legacy isolation, not an executable EVM backend or a selected shielded proof system.

### M2. Private offchain Eleusis

Dependencies: M1 for stable message context. Transport experiments can start during M1.

- [x] Select an established key-agreement and authenticated-encryption implementation with a documented session protocol.
- [x] Specify peer authentication, key roles, nonce construction, key rotation, and session recovery.
- [x] Implement offer, counter, authorization, sequence, parent reference, and final transcript root.
- [x] Implement a minimal ciphertext relay or direct transport with acknowledgments and durable storage.
- [x] Bound message size, queue growth, retries, and retained transcript data.
- [x] Reject duplicate, reordered, cross-session, and conflicting messages according to the protocol specification.
- [x] Separate key material and confidential messages from CLI logs and model-visible output.
- [x] Add service publication and discovery with authenticated peer identity, endpoints, assets, networks, and supported guarantees.
- [x] Exclude private negotiations and reservation prices from public discovery records.
- [x] Package the relay for hosted and self-hosted operation with authentication, limits, retention, and health diagnostics.

Done: two independent Rust processes agree on one transcript after disconnects and restarts.
No chain transaction is required for the new offchain negotiation path.
Transport metadata exposure is documented and measured separately from content privacy.
An external client discovers a compatible seller without receiving a seller address manually from the Erebus team.

Acceptance evidence uses real subprocesses with reloaded identities and reopened stores, a
signed JSON directory loaded by the buyer process, and the packaged authenticated HTTP relay.

Completed locally 2026-09-21: [M2 decision record and session protocol](metropolis-m2-decisions.md) and [M2 baseline](metropolis-m2-baseline.md).
Implementation is [`sdk/transport`](../sdk/transport); the relay binary is `erebus-relay`.
Discovery is a verified JSON directory document; a hosted HTTP discovery service is not built.
The `transcript_root` field is populated by M2 but is not yet constrained by a settlement predicate.

### M3. EVM chain adapter and public-bound settlement

Dependencies: M1. This work can proceed alongside M2.

- [x] Select and pin the Rust EVM client library and Solidity testing toolchain.
- [x] Create the EVM contract project and a versioned Rust ABI boundary.
- [x] Implement bilateral authorization verification, domain checks, expiry, replay state, and atomic token payment.
- [x] Bind the payment amount, asset, recipient, and fee policy to the same authorized commitment.
- [x] Specify which settlement fields become public and which business fields remain committed and hidden.
- [x] Define supported token behavior and reject unsupported transfer semantics.
- [x] Implement RPC chain verification, signing, gas estimation, nonce management, submission, and receipt verification.
- [x] Add direct-contract adversarial tests, including reentrancy and failing token transfers.
- [x] Expose payment and network-fee estimates with explicit allowance and balance shortfalls before submission.

Done: one authorized deal pays once on a local EVM chain, with no SDK bypass for changed payment fields.
The backend explicitly declares public amounts and recipients. Public binding never masquerades as shielded settlement.

Completed locally 2026-09-21: [M3 decision record](metropolis-m3-decisions.md) and [M3 baseline](metropolis-m3-baseline.md).
Contracts are in [`contracts/evm`](../contracts/evm); the adapter is [`sdk/evm`](../sdk/evm).
The Rust ABI boundary hand-encodes the fixed calls and tests their exact bytes (DM3-2).
The deployment-manifest item remains open for M8 and is not part of local-chain completion.
The Solidity decoder is narrower than the M1 asset grammar: only lowercase `eip155:<id>/erc20:0x<40 hex>`
assets are accepted (DM3-7). Payment is public; hidden amount and recipient remain M4/M5.
`submit` and `verify` are separate calls with no durable journal or reconciliation loop; that is M6.

### M4. Shielded payment mechanism and proof specification

Dependencies: M1. Begin feasibility research early, alongside M2 and M3.

- [x] Evaluate reusable privacy primitives against agreement binding, Monad deployment, licensing, and source availability.
- [x] Select the pool integration or document why a custom pool is necessary.
- [x] Select the proof system, circuit language, hash suite, authorization scheme, and proving-key lifecycle.
- [x] Specify notes, ownership, membership, nullifiers, outputs, change, fees, and per-asset conservation.
- [x] Specify the exact shared inputs between agreement verification and payment verification.
- [x] Resolve output decryptability and seller recovery before accepting a ciphertext-digest-only design.
- [x] Prototype one private transfer and measure proof time, memory, proof size, and verifier gas.
- [x] Record local proving hardware requirements and reproducible installation of versioned proving artifacts.
- [x] Publish the private witness schema and public input schema with negative test vectors.

Done: a runnable prototype demonstrates the required proof relation and a documented path to atomic contract enforcement.
A specification or two unrelated valid proofs do not pass this milestone.

Completed locally 2026-09-24: [M4 decisions and schemas](metropolis-m4-decisions.md),
[M4 baseline and measurements](metropolis-m4-baseline.md), and the runnable
[`circuits/m4`](../circuits/m4) prototype. The local test key is versioned by an artifact
manifest and regenerated reproducibly from pinned tools, but is not a published production
key. The harness moves no tokens; actual shielded payment remains M5. Suite 2 is not yet
available in `erebus-core`, and Monad gas has not been measured.

### M5. Shielded contracts, prover, and note wallet

Dependencies: M4 and the EVM execution foundation in M3.

- [ ] Implement suite-2 M1-to-field mapping and authorizations in Rust with cross-language vectors.
- [ ] Implement or integrate deposit, commitment tree, root history, spend nullifiers, private transfer, and withdrawal.
- [ ] Integrate agreement authorization, payment binding, range constraints, and deal replay protection into verification.
- [ ] Atomically consume the deal and input notes and create recipient and change outputs.
- [ ] Implement local witness construction and proving without sending spend secrets to the relayer.
- [ ] Implement encrypted output delivery, scanning, note selection, and restart-safe wallet storage.
- [ ] Package the public-event indexer for hosted and self-hosted operation, with rebuild and reorg recovery procedures.
- [ ] Verify wallet backup restoration and note discovery after switching indexer endpoints.
- [ ] Document setup trust, verifier versioning, upgrade authority, and artifact reproducibility.
- [ ] Run circuit mutation tests, contract fuzz tests, and independent conservation properties.

Done: deposit -> private negotiated payment -> recipient discovery -> recipient spend or withdrawal works end to end.
Wrong amount, asset, recipient, domain, proof, or replay cannot change contract state.
The recipient can restore state and withdraw test funds using documented commands and locally held secrets.

Local progress through 2026-09-27: a [funded Anvil prototype](metropolis-m5-progress.md) performs
deposit, agreement-bound private transfer, output reconstruction, and withdrawal. Rust now
matches the suite-2 signature vectors, reconstructs all three witnesses from typed note and
agreement inputs, and locally proves all three test transitions using hash-checked artifacts.
The transfer proof settles on Anvil. An encrypted local wallet and public RPC index now have
an integrated restart/reorg/endpoint-switch rehearsal. A public indexer service uses the
same verified cache and runs in the funded Anvil test. This is not M5 completion: it has
test-only keys, no installed shielded backend or independently operated service, and no
Monad testnet evidence.

### M6. Coordinator, relayer, and recovery

Dependencies: M1 and M3. Extend the same lifecycle to M5.

- [x] Resolve the backend once from session configuration and verify both peers accept the same settlement context.
- [x] Persist canonical intent before signing, proving, or submitting.
- [x] Separate prepared, submitted, unknown, included, finalized, reverted, and expired outcomes.
- [x] Bind relayer compensation and prevent copied submissions from redirecting value.
- [x] Handle concurrent calls, nonce replacement, RPC disagreement, dropped responses, and reorgs.
- [x] Reconcile from transaction and contract evidence before resubmission.
- [x] Keep journal migrations explicit and isolate note reservations across operations.
- [x] Enforce spending policies before authorization, with concurrent reservations and reconciliation of uncertain payments.
- [x] Add redacted operation diagnostics, health metrics, and runbooks for relay, relayer, indexer, and RPC failures.
- [x] Package the relayer with explicit fee funding, access limits, and provider-switch recovery.

Done: fault injection after every durable boundary yields at most one payment and a recoverable operation state.
Test both settlement modes. A local timeout never becomes evidence of an unpaid deal.
Policy limits survive concurrent requests and restarts. Operators can diagnose stalled operations without exporting private keys or transcripts.

Local progress through 2026-09-28: the shared journal and deal classifier are integrated.
The [local coordinator](../sdk/coordinator/README.md) persists intent and policy reservations
before signer/prover callbacks. It applies winner accounting and losing-revision release
atomically. These local checks do not replace the EVM transaction fault matrix.
See [M6 progress and remaining gates](metropolis-m6-progress.md).

Local progress through 2026-09-29: the public-bound EVM path is integrated end to end.
Canonical chain observation (receipts, complete settlement-log history, winner calldata and
authorization verification, explicit RPC `finalized`) is part of `sdk/evm`. Verified nonce-claim
release requires a finalized consumed nonce. Thirteen Anvil-backed tests cover the coordinated
lifecycle, a foreign winner, crashes at durable boundaries, the relayer's signed fee and
recipient binding, provider failover with funding diagnostics, and a TCP JSON-RPC fault proxy
that drops or falsifies responses. `erebus-tx-relayer` packages funding diagnostics and access
limits, and now requires relayer-owned persistent storage for journaled submission and recovery;
[the runbook](metropolis-m6-runbook.md) covers relay, relayer, indexer, and RPC failures.
Monad's documented `finalized` semantics match the implemented policy (documentation research;
live verification is M8). The shielded EVM path now uses the same durable coordinator and nonce
journal in a funded Anvil run, with native proof generation, restart, broadcast, and finalized
pool reconciliation. Honest partials: the local `Stage` collapses `included` into `submitted`
until finality, so the listed outcomes are separated across chain observation, the core
classifier, and the coordinator rather than one enum. As of 2026-09-30, combined funded
operation-write sweeps including fee replacement passed 68 injected boundaries for shielded mode and 56 for public-bound
mode, plus each baseline. RPC timeout sweeps hold reservations and recover from honest
evidence without another send. Paired-provider reads reject inconsistent evidence.
The owner selected mandatory paired operator recovery on 2026-10-01; the relayer and shielded
coordinator path enforce it without an automatic single-provider fallback.
Relayer CLI recovery wiring is locally verified.

M6 status 2026-10-01: **implementation locally complete; owner review pending.**
The exhaustive coordinator sweep was rerun and passed in both settlement modes
(`every_discovered_lifecycle_write_is_restartable_in_both_settlement_modes`, ~17.5 minutes):
fault injection after every discovered durable boundary yields at most one payment and a
recoverable operation state. All ten checklist items are checked. What remains is not code:
the open owner decisions in [M6 decisions](metropolis-m6-decisions.md) — DM6-5 (provider
quorum vs one RPC), DM6-7 (authorized-expiry cap and ledger scope), DM6-8 (relayer quote
flow), and the documented v1/v2 `accepted_at` ambiguity in DM6-2 — plus the recorded
limitations (trusted local observation checkpoints, 128 MiB shielded index cache with no
compaction, CLI `--serve` counters resetting on restart, no authenticated multi-client
gateway). Live Monad evidence is M8.
Public-bound history now resumes bounded log scans and ancestry walks from durable checkpoints;
the relayer uses this path before signing or sending. Shielded observation saves bounded
verified batches and holds reservations until history is complete. Local tests cover restart,
old winners, nonfinal reorgs, and paired-provider disagreement. Cache replay and the shielded
cache-size limit remain operational constraints. On 2026-10-01 the owner selected removal
of the legacy unjournaled backend APIs. Successful M3 payment tests now use coordinator-backed
submission and paired finality; invalid direct calls remain only as contract rejection tests.
M6 remains under final owner review;
remaining decision-record questions are not resolved implicitly by the RPC-policy choice.
See [M6 progress](metropolis-m6-progress.md).

### M7. Scoped disclosure

Dependencies: M2, M6, and the relevant settlement backend.

- [x] Define a recipient-bound package containing transcript evidence, agreement opening, authorizations, and settlement evidence.
- [x] Provide issuer authentication and independent receipt verification.
- [x] Export only per-deal capabilities, never parent session keys or spending secrets.
- [x] Implement explicit transcript backup and availability behavior.
- [x] Test wrong recipients, altered evidence, neighboring deals, and missing relay data.
- [x] Document backup and restoration of disclosure evidence without a dependency on hosted relay retention.

Done: a third process verifies the selected agreement and payment without either participant being online.
Document that expiry and revocation cannot erase plaintext already disclosed.

Local progress: `sdk/transport` verifies one selected agreement and exports a recipient-bound,
issuer-signed grant with an owner-only backup. `sdk/evm` independently checks finalized public-bound
payment evidence against the disclosed deal on Anvil, including from a fresh auditor subprocess
with only its grant, key, and RPC configuration. On 2026-10-01, SDK tests verified saved-grant
recovery after relay and transcript loss, and missing-grant reissue from retained signed evidence.
Installed agreement-opening restoration now has a durable read path:
`read_disclosure_opening` plus the CLI `select` method rebuild canonical evidence from the
coordinator snapshot and transcript store. The CLI issuer/recipient workflow is exercised
end to end in separate processes: `select` (issuer, from durable state) -> `export` ->
`verify_agreement` (fresh recipient, files only, participant directory deleted).
This verifies the agreement, not payment or delivery. On 2026-10-02 the same fresh-auditor
workflow passed through the disclosure-only MCP server, with Python passing paths to Rust.
See [M7 progress](metropolis-m7-progress.md).
The shielded SDK checks an opened agreement through paired finalized pool observation; its
local predicates and the 11 loopback RPC fixture tests pass on a machine that permits local
socket binds. The M5 harness now binds a replayable Rust-generated transcript into the funded
agreement, and the coordinated runner verifies that transcript before settlement.
The owner selected direct suite-2 grant signing. Version-2 grants authenticate the buyer or seller without a separate suite-1 issuer.
The funded local run passes through separate issuer and auditor CLI processes and the official MCP client.
Participant storage is unavailable during auditor verification. The auditor needs no note wallet, spend key, or prover.
M7 is complete at this local gate; Monad deployment and package-only installation remain M8 requirements.
See [M7 decisions](metropolis-m7-decisions.md) for signature domains and public grant metadata.

### M8. Monad deployment and agent integration

Dependencies: M2, M6, M7. Full private demonstration also requires M5.

- [ ] Verify current Monad network parameters and chosen verifier support against official sources and live RPC behavior.
- [ ] Deploy to testnet with reproducible artifacts, configuration, and verified contract identities.
- [ ] Add CLI onboarding, funding diagnostics, backend selection, and receipt output.
- [ ] Publish versioned testnet packages and a fresh-install guide covering identity, discovery, funding, settlement, disclosure, recovery, and withdrawal.
- [ ] Make local proving work from the installed Erebus MCP/SDK package, with automatic hash-checked shared artifact installation; require no source checkout, Circom installation, separate prover service, or operator setup ceremony.
- [ ] Deploy optional shared testnet services and document their metadata exposure, retention, access limits, and availability behavior.
- [ ] Provide a self-hosting package using the same service implementations, with endpoint configuration and persistent storage instructions.
- [ ] Update the Python boundary, MCP configuration, and deterministic two-agent harness.
- [ ] Add the HTTP service adapter and bind access to the finalized agreement and intended buyer.
- [ ] Persist access-issuance state and retry interrupted delivery without repeating payment or granting access to another buyer.
- [ ] Expose paid-but-undelivered outcomes and document the seller cooperation required for recovery.
- [ ] Implement x402 composition only against a specified scheme, with exactly one payment and explicit privacy exposure.
- [ ] Compare per-request settlement, prepaid allocation, and batched usage using measured latency, cost, accounting, privacy, and recovery behavior before selecting the x402 service model.
- [ ] Measure negotiation, proof, submission, inclusion, finality, and service delivery separately.

Done: independent buyer, seller, observer, and disclosure processes complete the real Monad workflow.
Both hosted endpoints and the self-hosting package support the documented workflow.
An interrupted service response can recover access without a second charge.
Mainnet deployment is a separate release decision after review and testnet evidence.

### M9. Hardening and submission

Dependencies: M0-M8 for the complete target.

- [ ] Run the threat-model attack matrix through direct contracts and SDK calls.
- [ ] Run existing Starknet regression tests and new EVM, circuit, transport, and recovery suites.
- [ ] Audit public calldata, logs, RPC payloads, relay metadata, and output shape for the stated privacy claims.
- [ ] Rehearse from a fresh environment and archive sanitized, reproducible evidence.
- [ ] Have an external developer integrate their agents using only versioned packages and public documentation, without undocumented team intervention.
- [ ] In the independent run, generate a private settlement proof locally from a fresh MCP installation without separate prover setup; record artifact download size, RAM, proof time, and failure behavior.
- [ ] Require discovery, shielded settlement, scoped disclosure, interrupted-operation recovery, and withdrawal in that external run.
- [ ] Rehearse self-hosting, backup restoration, and endpoint switching from a separate fresh environment.
- [ ] Record installation friction, policy-denial tests, metadata exposure, and service-failure diagnostics with the release evidence.
- [ ] Record the demo with real timings and clearly labelled edits.
- [ ] Separate prior Erebus work from development completed during Metropolis.
- [ ] Verify portal requirements, deadline timezone, mentor feedback, and any selected sponsor criteria.
- [ ] Prepare the submission package and document remaining limitations and review status.

Done: every submission claim points to code, a reproducible test, or a dated chain artifact.
The final product gate requires the external integration run and the self-hosting rehearsal above.
If no external developer completes the run, record the gate as incomplete even when internal demonstrations pass.
Any unfinished milestone remains explicitly unfinished. The schedule does not redefine completion.

## 5. Execution sequence and dates

Dates are planning targets, not estimates supported by implementation measurements.
Keep the complete scope visible and update forecasts after the first proof and contract measurements.

| Target window | Primary work | Work that can proceed alongside it |
|---|---|---|
| Sep 20-22 | M0 baseline and M1 protocol decisions | M4 privacy primitive and verifier feasibility |
| Sep 23-26 | M1 core, M2 transport, M3 public-bound settlement | M4 proof prototype |
| Sep 27-Oct 2 | M5 private settlement and M6 recovery | M7 disclosure package |
| Oct 3-7 | M8 Monad and agent integration | M9 adversarial tests and observer evidence |
| Oct 8-10 | Full-system regression and fresh-environment rehearsal | Submission evidence and demo recording |
| Oct 11-13 | Repairs, final evidence, and submission | Verify portal cutoff before the final day |

Critical dependency: agreement specification -> constrained private payment -> atomic verifier/pool transition -> recoverable Monad run -> evidence.
Public-bound settlement can progress independently of proving work, but it cannot satisfy the private settlement completion criteria.

## 6. First implementation session

1. Verify `metropolis` and record the current source baseline.
2. Run the existing Rust CI commands and classify failures before editing protocol code.
3. Enable branch CI and verify that release workflows remain isolated.
4. Write the agreement schema and backend capability decision record.
5. Add canonical encoding and domain/replay tests as the first new Rust behavior.
6. Begin the private settlement feasibility prototype against that same agreement schema.

Use existing modules first. Introduce separate crates only where dependency isolation or independent testing requires them.
Candidate boundaries are core, transport, settlement coordination, STRK20, EVM, and disclosure.
The final module layout follows the inspected dependency graph, not this list alone.

## 7. After Metropolis

- Qualify the generic EVM backend on another chain with independent deployment and finality tests.
- Improve Starknet agreement enforcement without claiming parity from an adapter wrapper alone.
- Add further privacy mechanisms behind explicit capabilities and trust assumptions.
- Evaluate chain negotiation, cross-chain settlement, escrow, and service-delivery guarantees as separate protocol extensions.
- Obtain external security review before a production-value launch.

## Progress log

| Date | Completed | Evidence |
|---|---|---|
| 2026-09-20 | Roadmap and branch isolation for planning | Switched from clean `main` to existing `metropolis` before edits |
| 2026-09-20 | M0 local completion | [Baseline, enforcement map, workflow audit](metropolis-m0-baseline.md), and [decisions](metropolis-decisions.md) |
| 2026-09-20 | M1 canonical agreement and shared Rust core | [Agreement specification](metropolis-agreement.md) and [M1 baseline](metropolis-m1-baseline.md) |
| 2026-09-20 | M4 feasibility research (not a completion) | [M4 feasibility memo](metropolis-m4-feasibility.md) |
| 2026-09-21 | M2 private offchain Eleusis | [M2 decision record](metropolis-m2-decisions.md) and [M2 baseline](metropolis-m2-baseline.md) |
| 2026-09-21 | M3 EVM public-bound settlement | [M3 decision record](metropolis-m3-decisions.md) and [M3 baseline](metropolis-m3-baseline.md) |
| 2026-09-24 | M4 private-transfer proof prototype | [M4 decision record](metropolis-m4-decisions.md), [M4 baseline](metropolis-m4-baseline.md), and `circuits/m4` |
| 2026-09-25 | M5 funded local prototype, not milestone completion | [M5 progress and open gates](metropolis-m5-progress.md) and `circuits/m5` |
| 2026-09-29 | M6 public-bound coordinator, observation, release, and relayer (M6 still open) | [M6 progress and remaining gates](metropolis-m6-progress.md) |
| 2026-09-30 | M6 bounded historical recovery in both settlement modes (RPC default still open) | [M6 progress](metropolis-m6-progress.md) and [DM6-12](metropolis-m6-decisions.md#dm6-12-bounded-historical-observation) |
| 2026-10-01 | Mandatory two-provider operator recovery | [M6 policy decision](metropolis-m6-decisions.md#dm6-5-finality-source-and-evidence-quorum) |
| 2026-10-01 | Remove legacy unjournaled backend submission and confirmation-based finality | [M6 operation lifecycle decision](metropolis-m6-decisions.md#dm6-4-evm-operation-record-and-stages) |
| 2026-10-01 | M7 installed issuer read path (`read_disclosure_opening` + CLI `select`) | [M7 progress](metropolis-m7-progress.md) and `sdk/coordinator/tests/lifecycle.rs` |
| 2026-10-01 | M6 exhaustive sweep rerun in both settlement modes | [M6 progress](metropolis-m6-progress.md); coordinator `--ignored` test |
| 2026-10-02 | M7 direct suite-2 grants and independent funded CLI/MCP auditor | [M7 decisions](metropolis-m7-decisions.md) and [M7 progress](metropolis-m7-progress.md) |

M0-M4 are complete locally at their scoped gates. M5 has a funded local prototype and remains
incomplete. M6 implementation is locally complete and awaits owner review of its open decisions.
M7 is complete locally: all six checklist items and the independent funded auditor criterion pass in both settlement modes.
M8-M9 remain incomplete.
