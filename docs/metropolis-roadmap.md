# Metropolis Roadmap

Updated: 2026-10-12. Branch: `metropolis`. Submission target: October 13, 2026.

This file tracks tasks and owners. Detailed runs, decisions and hashes belong in
[operations](metropolis-operations.md) and [install](metropolis-install.md).
The `sdk/core` agreement vectors remain normative for protocol formats.

Checked items have evidence at the stated scope. Local tests, team-run testnet workflows
and published installed workflows are separate gates. Agent reports are not independent audits.

## Product Target

Install Erebus, discover a service, negotiate privately, pay from shielded notes,
recover interruptions, retrieve the resource, and disclose one deal to an auditor.
Support qualified ERC-20 assets and MON through WMON. SDK, CLI and MCP are the product surface.

Polished testnet completion means a new operator can use public packages and docs only:
install, initialize, fund, discover, negotiate, pay through an explicit submission mode,
recover, retrieve, audit, repeat-spend and exit the pool. Every step must have usable
progress/errors, bounded operator policy, restart recovery and matching SDK/CLI/MCP behavior.
No checkout-only helper, private team config or manual JSON editing may be required.
Security-reviewed launch additionally requires independent setup, audit and outside acceptance.

Current shielded deployment: test-only, single-operator setup, unaudited, Monad testnet only.
Monad Private Settlement is a possible future backend, not a dependency.
A frontend and paid hosting are not required for the self-hosted testnet loop.

## Current Baseline

| Milestone | Current state |
|---|---|
| M0-M2 | Baseline, agreement core, encrypted transport and authenticated discovery implemented and tested locally. |
| M3 | Public-bound settlement runs on Monad testnet; payments are public. |
| M4 | Agreement/payment proof relation implemented; research gate complete, not an audit. |
| M5A | C1-C4 implemented, tested and frozen in `5ba05ee`; independent review remains open. |
| M5B | Signed test-only bundles hosted; native qualification and public HTTPS proving passed. Independent ceremony open. |
| M5C | Wallet/assets/access pass local gates; live completion and sponsorship remain open. |
| M5D | Adversarial tests pass; final integrated rerun and independent audit remain open. |
| M6-M7 | Recovery/disclosure pass local gates; live recipient-only shielded auditing now passes in a separate team process. |
| M8S | WMON/USDC pools deployed; one WMON private payment finalized. Full installed live workflow remains open. |
| M8O | Testnet prerelease `0.3.0.dev2026101307` published; native qualification and fresh public installation passed on both platforms, including installed init and relayer. Live workflow remains open. |
| M8E | Generated operator configs and agent wallet tools shipped in 1307; installed onboarding qualified natively. Cold-history and final live ergonomics remain open. |
| M9 | Outside acceptance and operating video remain open. |

### Live Evidence

- [x] Deploy and authenticate Poseidon, three verifiers and WMON/USDC pools.
- [x] Wrap MON, approve the WMON pool and finalize a proving deposit.
- [x] Finalize one private WMON payment with one broadcast attempt and committed accounting.
- [x] Verify that payment through the product's finalized-evidence path.

| Rail | Transaction |
|---|---|
| Shielded WMON | `0xe01666d8c4e16efe8195da8047674c71a325ee20ff250f02f4fdf570a04934fa` |
| Public-bound | `0x1ba9ddb363a0c916f8e41ca8eca7d4c524d1ba4722bf4ea37ed423547700ff73` |
| Public x402 exact | `0x8a0272e69188ac3be52783ddfc4d4436496a58f2d5c63d808cd3320a1fa5df43` |

The shielded run mixed installed negotiation with frozen source-built payment commands and warm history caches.
It is team-run evidence, not a published installed-only workflow.
Live resource retrieval after seller restart and recipient-only auditing now pass with source-built commands.
USDC payment, withdrawal/unwrap and the final installed-only workflow remain open.

## Ownership

| Owner | Work |
|---|---|
| Codex | Integration/evidence review, roadmap/status, deployment integration and approved installed live runs. |
| OpenCode | Delegated production integration, cold-history recovery and reproducible regression tests in isolated worktrees. |
| Claude Code | Packages, registry/install scripts, workflow, native qualification and install guide. |
| Poulav | Funding/release decisions, contributor/auditor arrangements, outside tester, video and submission. |

Preserve the uncommitted next-candidate changes; do not reset, stash or switch the active dirty worktree.
The owner-authorized local merge into `main` is `849dc37`; it is not pushed.
Keep future production work on `metropolis` unless the owner requests another integration.
Use the applicable authorization before committing, pushing, signing, publishing, deploying or spending.
Record source/binary identities and rerun affected gates after changes. Keep secrets and witnesses out of reports.
Maintain the 5 GiB disk guard. Emulated Linux evidence does not count as native Linux qualification.

### Final Execution Lanes

| Owner | Completion batch |
|---|---|
| Codex | Integration/evidence review, roadmap reconciliation, installed-to-live execution planning and authorized M5C/M8S lifecycles. Preserve Claude's qualification snapshot. |
| Claude Code | Qualify/package installed init and each frozen integration; M8O clean public install, operations/rollback and runnable guides; release/demo kit. No moving-tree rebuilds. |
| OpenCode | Production integration and regressions; cold-history storage/throughput improvements in a separate worktree, with migration, restart, corruption and paired-evidence tests. |
| Poulav | Record/host the operating video, arrange a real outside operator and submit through the portal with its exact cutoff/timezone. |

Packaging owns packaging files only. OpenCode owns the delegated production lane;
Codex owns integration evidence and authorized live stores. Keep Claude's qualification
snapshot separate from cold-history changes and preserve the published release.

## Deadline Execution Order

1. Poulav: verify portal requirements/cutoff and start the video/outside-test lane now.
2. OpenCode: close the funded installed-relayer binary gate on loopback Anvil chain 10143; hand off an exact candidate to Claude.
3. Claude: close onboarding qualification without bypassing publisher/binary pins; qualify the installed agent workflow and operations.
4. OpenCode: finish cold-history recovery separately, then integrate and rerun affected gates on a new candidate.
5. Codex/Claude: review the commit scope; qualify a clean revision natively and publish only with exact-version authorization.
6. Codex: run installed live WMON recovery, repeat spend and withdrawal/unwrap, then USDC and sponsorship. Never repay the finalized deal.
7. Poulav: complete video and portal submission; cross-check every claim against the evidence ledger.

The full product checklist below remains in scope after the submission cutoff.
Record each unfinished gate in the submission; do not remove it to declare completion.

The frozen release input tree is `e4e4f8323ba40e34ac4701d04d52d55f1ca5182328d9b2bf81a75fcc8f495130`.
It was committed in `5ba05ee`; packaging-only fix `07e6949` leaves the production inputs unchanged.
Hosted native qualification [38227563253](https://github.com/PoulavBhowmick03/Erebus/actions/runs/38227563253) passed.
Private prerelease [0.3.0.dev2026101202](https://github.com/PoulavBhowmick03/erebus-metropolis/releases/tag/v0.3.0.dev2026101202) is published.
Native fresh public-install [38255174875](https://github.com/PoulavBhowmick03/Erebus/actions/runs/38255174875) passed on macOS/Linux.
These checks prove package installation and synthetic offline proving, not live payment completion.
The installed init integration is committed in `ab6dc3b`, merged locally into `main`, and not yet published.

M5R remains a separate workstream. Integrate it only with time to rerun affected gates; otherwise submit the self-submitted boundary explicitly.
Independent ceremony, audit and outside acceptance require actual participants. The deadline does not waive these gates.

### Current Integration Handoff

OpenCode's 2026-10-13 rehearsal candidate is based on `482d912` with dirty source identity
`f6ec6bcc27f39bc39155bf439301a80ec6e0e4361c5083c12fbe6f6db4efb6dd`.
Its local test record is in `.local/metropolis/candidate-20261010/opencode-stable-candidate-20261013.json`.
This is not a clean release freeze. Claude is qualifying that isolated snapshot.
The paired-recovery masking investigation found bounded delayed error reporting, not
spend authorization without paired evidence; its separate reproducer passed three tests.
Installed qualification, live asset/relayer loops and cold seller catch-up remain open.
Cold-history improvements must use a separate candidate and cannot silently replace this snapshot.

Concrete blockers: integrate relayer inventory; require funded installed-relayer and generated-agent reports;
qualify the changed onboarding candidate; then approve a clean revision and exact version.
Its focused release tests passed 57 with the expected missing-relayer inventory failure.
Subsequent integration resolved the inventory gap and added publication report guards.
Focused onboarding/publication/registry tests passed 126. The installed relayer passed
five funded local chain-10143 tests; generated-agent/native report execution remains open.

Installed payment-CLI delegation now passes against the local `0.3.0.dev2026101306`
packages: durable delegation before transport, withheld acknowledgement, restart,
unavailable relay and no direct fallback; buyer sends 0, sponsor sends 1, payment
verified. The identity-bound adapter reproduced it in two fresh installs. Both
combined runs failed only the oversized-body 413 check with a transport reset.
The [bound report](metropolis-evidence/installed-delegation-20261013.json) keeps
the overall gate false. Native qualification,
MCP sponsorship qualification and live relayed settlement remain open.

Shipping target: `0.3.0.dev2026101307`, authorized by the owner's 2026-10-13
autonomous-shipping request. The bounded HTTP drain fixes the oversized-request
reset in source: six relayer-binary regressions and six HTTP unit tests passed.
New installed/native package qualification and publication remain unchecked.

The subsequent rebuilt local `1307` package passed the full installed-relayer
adapter (8/8, no gaps), fresh installation and both-pool real HTTPS proving.
This closes local installed relayer qualification, not native CI or publication.
The eight tests include separate installed CLI and MCP sponsorship lifecycles;
both verify recovery with buyer sends 0 and sponsor sends 1. Native and live
qualification remain open.

### Product Completion Batches

These batches summarize the full checklists below. Local implementation is not
installed qualification; installation and synthetic proving are not live settlement.

| Owner | Batch | Required evidence |
|---|---|---|
| OpenCode | Installed sponsored submission | Actual packaged service on loopback chain 10143: submit, status, restart, lost response, duplicates, receipt recovery and no direct buyer fallback. Preserve the production chain restriction. |
| OpenCode | Cold-history recovery | Measured long-history throughput/storage; safe cache compatibility; interruption/corruption/rollback tests; cold seller and change recovery followed by spend and exit. |
| OpenCode | Final production regression | CLI/SDK/MCP policy parity, token-admin failures, signer/reservation fences, provider disagreement, redaction and legacy rails on the exact integrated identity. |
| Claude | Installed onboarding and agents | Pin-checked init, buyer/seller/auditor setup, wallet-policy, doctor and endpoint recovery using candidate binaries; fresh SDK/MCP negotiation, funding, payment, recovery, access and disclosure. |
| Claude | Operator readiness | Owner-only stores, backup/restore, process restart/shutdown, bounded limits, actionable diagnostics, configuration compatibility, rollback and unsupported-platform behavior. |
| Claude | Release qualification | Complete binary/license inventory, both-pool HTTPS proving, fresh native macOS/Linux installs, funded local rehearsals, release retention and public runnable examples. |
| Codex | Integration and evidence | Review source/schema identities and failures; keep roadmap/README/status consistent; prepare exact installed-to-live commands and accounting checks. |
| Codex | Installed live lifecycle | Both assets: funding, negotiation, payment, restart, seller/change recovery, repeat spend, withdrawal; WMON unwrap; sponsored sender; access/disclosure and policy-denial evidence. |
| Codex/Claude | Measured product boundary | Separate install/download/negotiation/proof/submission/finality/catch-up/delivery timings; public calldata/log observer report; state remaining metadata and test-setup limits. |
| Poulav | Release and submission decisions | Approve exact commit/push/version scope; provide missing token/gas funding; operating video, portal requirements and evidence-linked submission. |
| Independent participants | Trust and outside acceptance | Independent setup contributions/verification, audit and retesting, and a new operator completing the public-docs workflow without private assistance. |

Do not publish to solve a qualification gap. Candidate onboarding needs a scoped
rehearsal mechanism or equivalent offline qualification that preserves trust checks.
A new rehearsal mechanism changes package inputs and needs a new candidate identity.
No placeholder funding, skipped history, synthetic live receipts or AI-run independence.

## Remaining Work in Dependency Order

### M5A. Freeze and Review the Shielded Design

- [x] Trace circuits, pool, witness mapping, wallet, recovery and disclosure invariants.
- [x] Implement strict EdDSA acceptance matching Rust, including subgroup checks (C1).
- [x] Bind note commitments/nullifiers to chain, pool and verifier version (C2).
- [x] Constrain change ownership and retain encrypted spending material durably (C3).
- [x] Reject equal agreement keys in the SDK; disclose pool-level self-deal allowance (C4).
- [x] Preserve agreement encoding and frozen negotiation before authorizations.
- [x] Document gas-payer exposure and the current zero-fee submission policy.
- [x] Close the merge and freeze the approved candidate in public commit `5ba05ee2999aecb390c2d8276f084af38dc4dda3`.
- [ ] Resolve remaining security findings against the frozen revision.

Completion gate: approved design, pinned implementation and explicit findings record. This is not an audit.

### M5B. Artifact Setup and Distribution

- [x] Verify the selected phase-1 transcript and automated phase-2 test chains.
- [x] Build test-only keys, matching verifiers and complete source/artifact inventories.
- [x] Generate WMON/USDC pool-bound manifests and verify their verifier identities.
- [x] Require pinned publishers and authenticated manifests, not hashes alone.
- [x] Test reuse, interruption, tamper, wrong domain/version/publisher and disk failures.
- [x] Prove all three transitions from installed macOS packages for both deployed pool domains.
- [x] Record earlier Linux prototype proving evidence with its native/emulated scope.
- [x] Prepare HTTPS hosting checks and native dual-platform qualification workflows.
- [ ] Receive independent phase-2 contributions and separate verification/destruction attestations.
- [ ] Produce independently contributed final keys, verifiers and matching manifests.
- [x] Sign, upload and verify both approved pool manifests/artifacts over real HTTPS (test-only, single-operator).
- [x] Qualify the published test-only hosted inputs on native macOS arm64 and Linux x86_64.

Completion gate: installed proofs use authenticated, qualified artifacts. Prototype distribution does not close independent setup.
Sagar/Sudipta are designated contributors and Ishita the verifier; independent participant evidence remains absent.

### M5C. Complete Asset, Wallet, and Service Integration

- [x] Implement fenced approve/deposit/withdraw/wrap/unwrap/sync commands.
- [x] Bind signed transactions to chain, sender, nonce, target, value and calldata.
- [x] Require exact receipt/event evidence and match withdrawal consumption to its own finalized successful attempt.
- [x] Persist attempts before sending and retain reverted/replaced transaction history.
- [x] Implement explicit same-nonce, same-intent fee replacement and observation-only recovery.
- [x] Complete installed local ERC-20 and wrapped-native workflows with random operators.
- [x] Recover recipient/change notes and reuse them in further private deals locally.
- [x] Test stale backups, index corruption, endpoint changes and actual local withdrawal rollback.
- [x] Run chain-level durable-write sweeps across wallet operation kinds.
- [x] Implement buyer-bound snapshot/metered access with durable issuance and restart recovery.
- [x] Keep payment, resource hash, issuance and delivery flags separate.
- [x] Test paired snapshots/read retries without lost progress, revived notes or extra sends.
- [x] Add bounded longer retrieval requests and authenticated auditor pending/progress handling locally.
- [x] Certify an indexed finalized payment without scanning unrelated later blocks; retain full scans for absence and wallet recovery.
- [ ] Qualify WMON/USDC behavior, issuer/admin risks and exact accounting on Monad.
- [x] Complete live paid-resource retrieval after seller restart and recipient-only auditor verification, using source-built commands.
- [ ] Complete live recovery, repeat spending, withdrawal and WMON unwrap.
- [ ] Integrate sponsorship with the durable payment fence (M5R).

Completion gate: installed buyer/seller/auditor flows pass locally. Live assets and sponsorship have separate gates.

### M5R. Shielded Relayer and Submission Privacy

Proposed first scope: an off-chain, operator-sponsored service using the existing pool.
The relayer signs the outer transaction with its own gas account.
Forwarding buyer-signed bytes does not hide the buyer.
No deliberate batching delay, extra settlement contract or user-paid fee is assumed.
Implementation details and deployment/spending need the applicable owner approval.

- [x] Specify proof-only requests, Bearer authentication, bounded rate/work limits, lifetime gas-cap budget and service identity (single-principal source implementation).
- [ ] Accept only proofs/public inputs; reject witnesses, spending keys and agreement openings.
- [x] Add a bounded proof-only request codec and derive service identity from chain/pool/nullifier (local schema tests; transport/authentication remain open).
- [x] Verify external transfer proofs against pinned keys locally; reject noncanonical fields, invalid points and changed public inputs.
- [x] Add read-only paired admission: authenticate creation/head runtimes, verify the proof, simulate exact zero-value calldata and recheck anchors (controlled RPC tests).
- [x] Test admission against a funded local pool: unknown roots, consumed deals/notes and reused outputs are refused without admission sends.
- [x] Wire admission and funding checks into the HTTP service before allocating sponsored gas (funded local test).
- [x] Wire paired admission and full gas-cap funding checks into the backend engine before nonce allocation (funded local test).
- [ ] Persist intent, nonce claim, signed bytes and attempts before broadcast.
- [x] Add sponsor-owned exact-call storage, nonce integration, sign-once recovery and atomic initial-send fencing (local tests).
- [ ] Return durable status/hash and resolve lost responses through finalized evidence.
- [x] Recover the backend's own retained hash through paired finalized receipts and exact pool effects after restart; test a duplicate request without another send (local Anvil).
- [x] Bind buyer recovery to the relayed operation; persist the destination before HTTP and forbid direct fallback (native buyer CLI, local Anvil).
- [x] Persist a buyer delegation fence before outbound transport; reject direct signing after delegation, including restart and storage faults (coordinator tests).
- [ ] Test concurrent requests, copied proofs, restart, exhausted funds, replacement and provider disagreement.
- [ ] Verify the public sender is the relayer, with no buyer/seller key in transfer calldata.
- [ ] Test redaction and document relayer IP/timing visibility and censorship risks.
- [ ] Measure direct versus relayed submission, inclusion and finality on the same candidate.
- [x] Implement explicit sponsorship in CLI/SDK/MCP; prohibit silent direct-submission fallback (focused native/Python tests; installed qualification still open).
- [ ] Rehearse self-hosting and alternative relayers without new payment authorization.
- [ ] Run an approved live relayed payment and archive its public observer report.
- [ ] Include the relayer in audit scope and final installed qualification.
- [ ] Ship an authenticated HTTP service and CLI with body limits, deadlines and redacted errors.
- [x] Implement the service binary and authenticated HTTP adapter with body limits, deadlines and redacted errors (local funded gate; not packaged/deployed).
- [x] Persist one configured principal's rate limits and lifetime gas-budget reservations across restarts; bound distinct operations and in-flight work (unit tests).
- [ ] Qualify quota crash boundaries, concurrency and deployment behind an authenticated TLS gateway; do not claim multi-tenant isolation.
- [ ] Return explicit queued/busy states for the shared gas account; never allocate a second nonce lane.
- [x] Retain fee-only replacement attempts and reconcile an older winner (unit and funded local tests; installed/live qualification open).
- [x] Add explicit owner resume for retained claims/bytes and fixed-ceiling replacement; forbid automatic resend or fresh-nonce fallback (local tests).
- [ ] Qualify installed operator recovery, expired held claims and exhausted sponsorship without discarding payment evidence.
- [x] Integrate the loopback chain-10143 fixture and verify an exact packaged relayer hash through submit, observe, restart, explicit dropped-send resume, duplicates and buyer-transport failure (five local funded tests; full payment-CLI/native/live gates remain open).

Completion gate: one relayed payment finalizes, recovery never pays twice, and the relayer is the public sender.
This hides direct submission identity only. Funding, withdrawals and small anonymity sets remain linkable.
Relayer compensation requires a separate reviewed authorization design; never add unconstrained fees.
Stealth withdrawals and timing/batching defenses are later work, not current guarantees.

### M5D. Adversarial Testing and Independent Audit

- [x] Test identity expiry/clock failures and strict Rust/circuit signature acceptance.
- [x] Run witness mutations, differential vectors, proof/public-input mutations and replay negatives.
- [x] Test exact contract expiry and deposit/withdraw callback rollback.
- [x] Run real-proof conservation sequences and independent tree/wrapper invariants.
- [x] Run funded recovery, provider conflict/lag and interrupted-history regressions locally.
- [x] Test bounded read retries without lost progress, revived notes or extra sends.
- [x] Inspect proof-only RPC/calldata/log shapes and neighboring-deal isolation.
- [x] Harden journal paths, artifact requests, relay/indexer inputs and model-visible diagnostics.
- [ ] Rerun affected suites on the frozen candidate, including legacy regressions.
- [ ] Finish cross-surface leakage review, including M5R and live observer evidence.
- [x] Review candidate CLI/SDK/MCP, relay and disclosure boundaries with focused local redaction/observer tests; document agent-visible amounts and onboarding error hardening. Live observer and deployed logging review remain open.
- [ ] Commission an independent circuit/contract/wallet/service/setup audit.
- [ ] Publish exact scope/revision, report, findings and limitations.
- [ ] Fix findings and obtain reviewer retesting; leave no unresolved critical/high findings.

Completion gate: the frozen candidate passes tests and independent review/retesting. AI review and CI are not an independent audit.

### M8S. Monad Shielded Testnet Qualification

- [x] Obtain testnet deployment/run authorization and fund dedicated accounts.
- [x] Deploy/authenticate WMON/USDC pools, Poseidon and verifiers through paired finalized reads.
- [x] Negotiate a suite-2 agreement with separate installed participant commands.
- [x] Wrap, approve, deposit and settle one private WMON payment using product commands.
- [x] Verify finalized payment, one broadcast attempt and committed accounting.
- [ ] Run all commands from one qualified installed release with participant key isolation.
- [ ] Complete USDC payment and both assets' recovery/repeat-spend/withdrawal workflows.
- [ ] Complete WMON unwrap with exact finalized accounting.
- [x] Retrieve the paid resource after seller restart without another payment, using source-built commands.
- [x] Verify a grant with participant paths withheld and authenticated runtime pins in a separate team-operated auditor process.
- [ ] Run live prepaid access, lost-response recovery and policy denials.
- [ ] Archive public calldata/log exposure and metadata-correlation limits.
- [ ] Measure install, download, negotiation, proof, submission, inclusion, finality, recovery and delivery separately.
- [x] Publish a clean-source, native dual-platform-qualified test-only private prerelease after approval.

Completion gate: both approved assets complete reproducible installed live flows. Deployment or one payment alone does not pass.

### M8O. Operations and Developer Release

Release update (2026-10-13): `0.3.0.dev2026101307` is published from clean commit `674e5a7`.
[Native qualification](https://github.com/PoulavBhowmick03/Erebus/actions/runs/38431534413)
and [fresh public installation](https://github.com/PoulavBhowmick03/Erebus/actions/runs/38443540443)
passed on macOS arm64 and Linux x86_64. These are local-chain and installation gates, not live settlement.

- [x] Publish public-bound/x402 dev5 and retain dev4 in the cumulative registry.
- [x] Prepare private onboarding, allowance/funding diagnostics and self-hosting tools.
- [x] Qualify r5 packages on macOS with both pool domains and local funded rehearsals.
- [x] Archive r5 after source changes and retain its candidate/artifact identities.
- [x] Qualify r6 locally on macOS for both pools; preserve its superseded source identity and reports.
- [x] Qualify r7 locally on macOS with the withdrawal fix, both pool domains and installed funded rehearsals.
- [x] Qualify the published private release on native macOS/Linux with real hosted inputs.
- [ ] Rehearse final-release persistence, backups, endpoints, limits, health and secret-free diagnostics.
- [ ] Update guides/examples to the qualified revision and require all five disclosure runtime pins.
- [ ] Verify release rollback/withdrawal, notices, platform limits and registry retention.
- [x] Publish approved `0.3.0.dev2026101202` and verify native fresh public installation outside the checkout.
- [ ] Deploy promised hosted services only with separate account/budget authorization.
- [x] Qualify and publish the installed `erebus-init --metropolis` integration; preserve plain Starknet init (1307, native qualification above).
- [x] Include every new relayer/tool binary in the inventory, hashes, licenses and installed smoke tests (1307, installed-relayer gate and public-install hash checks).
- [ ] Test interrupted downloads, cache/disk failures, unsupported platforms and actionable recovery from the final release.
- [ ] Supply one runnable buyer/seller/auditor example using public pins, not checkout fixtures.
- [x] Remove the need to source deployment replay bytes from a private operator journal: add a pinned, paired read-only public exporter (14 offline tests; six real deployment transactions verified; native invocation passed in the 1307 workflow).
- [ ] Verify clean unattended process restart, owner-only persistent stores and bounded graceful shutdown.
- [ ] Document configuration/schema compatibility and safe upgrade/rollback without losing reservations.
- [ ] Provide release-pinned health, progress, gas-budget and redacted incident diagnostics.

Completion gate: public docs/packages support the declared workflow without unpublished team helpers.
Render remains preparation-only. Hosted services are optional unless the release promises them.

### Onboarding Workstreams (2026-10)

Goal: anyone can run two agents on Monad testnet from a fresh machine. None of this is in the
published `0.3.0.dev2026101307`; it ships in `0.4.0.dev2026101801`.

- [x] **WS1, fresh-wallet history checkpoint** (`main` d9a3ab9): a fresh wallet starts from a paired
  finalized frontier instead of replaying the pool's history; the payment command shares the
  wallet's caches; throttled reads back off. Live on Monad testnet: two freshly onboarded agents,
  two private payments, the second from a change note.
- [x] **WS2, agent flow** (`main` f2ec69d): stable error codes with `next_action`, created and
  persisted operation IDs, a bounded seller wait, settlement after the wallet settles, retrieval by
  `operation_ref`, and `fund_wallet`. Live on Monad testnet with no operation IDs passed by the agents.
- [x] **WS3, one-command testnet quickstart** (`main` 08e75d4):
  `erebus-init --metropolis quickstart up|start|status|renew|down`, native gas-key generation and
  descriptor renewal, background history sync in the access service, MCP configs and prompts for
  Claude Code, Codex and Cursor. Live from a fresh checkout of `main` 08e75d4 (2026-10-17), installed
  from a rehearsal registry built from it: `quickstart up` ready in 75 s; `fund_wallet` (wrap,
  approve, deposit) done at 9.7 min; two negotiated deals, the first with a buyer joining 45 s after
  the seller, the second paid from the first's change note; both payments verified by observation;
  each resource retrieved by `operation_ref` 334 s and 357 s after its payment, with matching
  hashes. 47 minutes in all; five buyer transactions costing 1.533 MON, none from the seller.
- [x] **WS4, published release** `0.4.0.dev2026101801` (source `main` 8a2f97b, native qualification
  run 39197664018 on macOS arm64 and Linux x86_64, published 2026-10-19): one-line `uv tool install`,
  `quickstart up` pins its own release, resumable `quickstart prefetch`, and background history sync
  that yields to requests. Candidate run from a clean-HOME install: two deals, the second from the
  change note, 24 min. Published-package run: install, `quickstart up`, one deal delivered 97 s after
  the verified payment, 21 min, 1.227 MON buyer gas.
- [ ] **Next:** an outside tester using public docs only, then many-buyer sellers.
- Decisions (2026-10-17): **D2**, the next combined release is versioned `0.4.0.devN`, so it sorts
  above Starknet's `0.3.0`. **D3**, many-buyer sellers and descriptors published by URL wait until an
  outside tester has used the one-to-one flow. **D4**, `fund_wallet` may chain wrap, approve and
  deposit: it exists only when the operator's policy grants all three, and every step stays capped.
  WS1-WS3 were reviewed by the repository owner before the merge.

### M8E. Agent Ergonomics and End-to-End Product Surface

Owners: OpenCode completes delegated production integration and regressions;
Claude qualifies installed onboarding/agent workflows; Codex reviews integration
evidence and executes the authorized installed live lifecycle.

- [x] Generate buyer/seller/auditor configs from release pins and deployment identities, with Rust-owned keys.
- [x] Add read-only operator doctor and installed `erebus-init --metropolis` routing in source.
- [x] Preserve the existing Starknet initializer and Metropolis MCP backend; test both after integration.
- [ ] Ship the initializer in a qualified public wheel and run buyer/seller/auditor setup outside the checkout.
- [ ] Generate configs with safe defaults; require explicit signer, endpoints, spending policy and submission mode.
- [ ] Expose wallet status, allowance, notes and operation progress through SDK/MCP with secret-free responses.
- [x] Implement opt-in paired finalized account diagnostics and checkpoint lag in native wallet, SDK and MCP (local/funded tests; public release qualification open).
- [ ] Expose wrap, approve, deposit, withdraw, unwrap, sync and explicit fee replacement through SDK/MCP.
- [x] Implement fixed-config wallet SDK/MCP action tools with amount/recipient limits, explicit replacement fee caps and required deposit references (focused Python tests; full installed workflow open).
- [ ] Bind mutations to operator-owned assets, recipients, amount/fee limits and durable operation references.
- [ ] Refuse arbitrary targets/calldata/key paths; never auto-approve spending or release a held reservation.
- [ ] Keep pending/reverted/late/corrupt states distinct; report the next safe action without automatic recharging.
- [ ] Expose seller-reported issuance time/late status in the immutable-resource retrieval receipt and SDK/MCP, including restart/cache reads. Legacy caches must report timing unknown rather than inventing it; keep resource/payment/delivery verification distinct.
- [ ] Show bounded catch-up progress, checkpoint age and stalled-provider diagnostics across fresh processes.
- [ ] Exercise authenticated discovery and encrypted negotiation through separate installed buyer/seller MCP servers.
- [ ] Exercise payment, restart recovery, resource access and auditor verification through those same servers.
- [x] Verify the generated two-agent installed workflow on both local mock-asset pools: MCP funding, one settlement, buyer/seller restart, resource hash and scoped audit (candidate af7e69d8; rehearsal pins, not native/live/publication evidence).
- [ ] Complete cold seller/change-note recovery, repeat payment and pool exit from retained encrypted backups.
- [ ] Verify SDK, CLI and MCP produce equivalent results and enforce the same spending/submission policy.
- [x] Wire hosted native generated-agent qualification to paired public deployment replay and exact-version prepublication pins; require feature reports in platform comparison (77 local checks pass; native execution and relayer-report adapter remain open).
- [ ] Document and test shielded-backed paid API access separately from public x402; never imply standard x402 is private.
- [ ] Measure cold/warm install/proving/history catch-up and complete request timings; document remaining bottlenecks.
- [ ] Run a new-operator acceptance exercise using only public packages/docs, without hand-edited JSON or team state.

Completion gate: the whole two-agent workflow is usable from a fresh installed release,
with policy-bound wallet operations, explicit sponsorship and recoverable errors.
The source initializer and parsing smoke tests alone do not close this gate.

### M9. Independent Acceptance and Submission

- [x] Prepare the outside acceptance handoff and operating-demo script.
- [ ] Run the private workflow with an outside operator using only public docs/packages.
- [ ] Record both assets, local proving, recovery, access, disclosure and withdrawal/unwrap.
- [ ] Record and host an operating video with a real Monad interaction.
- [ ] Verify portal rubric, mandatory artifacts, deadline/timezone and applicable bounty conditions.
- [ ] Separate Starknet work, Metropolis work, AI assistance and prototype limitations.
- [ ] Cross-check claims against exact revisions, manifests, test reports and live receipts.
- [ ] Freeze the submission revision; pin its public install, deployment, demo and evidence links.
- [ ] Explain public gas/funding/withdrawal metadata, small anonymity set and single-operator setup in the demo.

Completion gate: outside acceptance and mandatory submission artifacts exist. Team rehearsals and scripts do not substitute.

## Privacy and Asset Boundary

Wrapping is asset conversion, not privacy. Shielded proofs hide inner-payment amounts and spending keys.
Observers still see gas senders, pool/asset, expiry, commitments, nullifiers and timing.
Deposits/withdrawals expose addresses and amounts. A small pool does not establish relationship anonymity.

Current private transfers are self-submitted. No stealth-address or shielded-relayer guarantee is implemented.
Public-bound and standard x402 payments remain public. Never silently downgrade a shielded request.
Keep keys and proving local. Artifact/indexer services introduce metadata and availability dependencies.

Each pool supports one immutable ERC-20. MON uses `MON -> WMON -> shielded notes -> WMON -> MON`.
USDC has issuer/admin powers. Arbitrary ERC-20, ETH/WETH, direct native-MON pools and mainnet assets are not qualified.
Record transfer behavior, decimals, runtime identity, backing and admin risks for each supported asset.

## Mainnet and Later Work

- [ ] Complete independent setup, audit/retesting and outside acceptance for the launch scope.
- [ ] Qualify mainnet assets, wrappers, gas exposure and deployment identities.
- [ ] Publish monitoring, incident response, recovery and upgrade/immutability policies.
- [ ] Obtain separate mainnet launch and funding authorization.

Later: stealth withdrawal destinations, optional batching/timing defenses, additional qualified assets,
other EVM deployments and confidential-domain adapters. These are not current testnet guarantees.
