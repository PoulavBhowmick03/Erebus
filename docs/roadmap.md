# Erebus product roadmap

Last source audit: 2026-08-19.

This document is the shared plan for Poulav and Ishita. It covers the protocol, SDKs, MCP
server, reference agents, skill, operations, security, documentation, and sprint delivery.

The current product is an operator-run technical preview. It is not production-ready.

## 1. Product definition

Erebus gives two autonomous agents a private negotiation record and an atomic shielded
settlement. A scoped viewing grant can reveal the record after settlement.

Erebus ships as operator-run infrastructure:

1. The Rust SDK owns protocol logic, keys, local state, proving, signing, and submission.
2. `erebus-cli` exposes one JSON request and one JSON response per process.
3. The Python SDK passes data between Python and the Rust CLI.
4. The Python MCP server exposes role-bound tools to agent frameworks.
5. The reference agents show buyer and seller policy behavior.
6. The Erebus skill will teach an agent how to install, operate, and diagnose the stack.
7. The operator kit will package the binary, configuration, health checks, and recovery tools.

Erebus does not deploy an application contract today. It composes the STRK20 privacy pool.
The files under `contracts/` are test probes.

### Product boundaries

The default product remains operator-run. Erebus will not host a multi-tenant key service.
Each operator controls its Starknet account, pool key, state, RPC path, and prover path.

The first product is not a dashboard. The public browser demo explains the flow and shows
evidence. Agents remain the product users.

The project will ship both interfaces:

- A Rust SDK for direct integration.
- A Python MCP server for agent frameworks.

The TypeScript SDK remains a differential oracle. It does not ship as a product client.

## 2. Release targets

The project needs three different finish lines. The sprint deadline is not the production
deadline.

| Target | Purpose | Required result |
|---|---|---|
| Sprint entry | Qualify for judging by 2026-08-31 | Public demo, public video, three mainnet pool transactions, and a complete `strk20.json` |
| `v0.1.0` technical preview | Let an external operator evaluate Erebus | One-command install, role-bound MCP flow, current protocol limits, and reproducible evidence |
| `v1.0` production release | Let an operator use real value under a written trust model | Final wire, crash recovery, reviews, audit, release controls, and a successful mainnet canary |

The sprint entry can finish without production readiness. The README and demo must keep
that distinction clear.

**D14 changes what the sprint entry can reach.** The owners decided on 2026-08-16 to stay on
Sepolia. The hub requires at least three mainnet hashes and scores a working mainnet product
at 30%, so an entry without them is incomplete against the published rules regardless of how
good the Sepolia evidence is. The rest of this document plans for that decision rather than
around it: mainnet work is recorded, deferred, and kept ready to execute if D14 reverses.

## 3. Current evidence

This section records what exists on 2026-08-19. Later sections describe the missing work.

### Committed and working

- Wire v2 uses AES-256-GCM-SIV and a 128-bit tag across five STRK20 note salts.
- Two live Sepolia negotiations settled. The wire-v2 reference run,
  `0x14b38e9dbc65f0749be6da2fa05dd2713f8c4c893bac707961c73e616b34cb3` (2026-08-07), and
  the merged-code run with change, a third-party disclosure, and observer output,
  `0x4191fe47a0b062605a7bbc08dd40eafdefcd52de4fd0288e8315eb48ee2f341` (2026-08-19, see
  `docs/runs/2026-08-19-sepolia-run.md`).
- `scripts/observer.py` recovers wire-v1 terms and does not recover wire-v2 content.
- The Rust client can open channels, write offers, read state, settle, shield, grant, and reveal.
- The Python seam uses protocol 2 and passes key-file paths to Rust.
- The MCP server exposes ten tools over stdio, installable as the `erebus-mcp-server`
  console script from wheels alone.
- Payer and payee roles prevent the payee from calling `accept_and_settle`.
- `wait_for_offers` reduces agent tool calls, but it still polls the chain.
- The public demo runs at `https://erebus-private-agents.vercel.app`.
- The repository is public and uses Apache-2.0.
- The Rust client grants and reads the pool's STRK allowance, and `agent.sh fund` sizes its
  approval as deposit plus the live fee.
- `erebus-cli doctor` inspects files, endpoints, pool, registration, allowance, and gas
  balance read-only, and reports a repair instruction per fault.
- Settlement receipts report selected input value and change.
- `main` passes 216 Rust tests, 70 Python tests, and 38 TypeScript tests.
- Clippy and rustdoc pass with warnings denied.
- CI runs all of the above on every push and pull request, plus a gitleaks history scan.
  TypeScript is not in it; see §5.6.
- `docs/privacy-model.md` is the canonical privacy boundary. The roadmap, `tech.md`,
  `friction.md`, and `privacy-observer-finding.md` point at it rather than restating it.

### Shipped since the 2026-08-17 audit

Everything the previous audit listed as "active but not shipped" has landed on `main` and
been exercised live. The short record, with pointers:

- **Change-note settlement, end to end.** PR #16 aligned MCP, mock, and agents with the
  Rust change path; `40b8543` mirrored `selected_input`, `change`, and string amounts
  through the seam. The 2026-08-19 settlement exercised it live: 5 STRK selected,
  2.75 paid, 2.25 returned, conservation asserted by test and observed on chain.
- **The allowance path, under the live 2 STRK fee.** Exercised through merged code on
  2026-08-19. Still open: the `.env` identity `0x032bb394...c805` holds a zero allowance;
  the evidence run used the `erebus-g`/`erebus-h`/`erebus-f` identities instead.
- **`doctor`, reachable from every layer.** CLI, Python seam (`2d69eda`), MCP tool
  (`40b8543`), and at MCP startup since 2026-08-19 (logged, not fatal;
  `EREBUS_SKIP_STARTUP_DOCTOR=1` skips). `new-identity.sh` ends with it and fails
  non-ready.
- **Every `u128` crosses every boundary as a string.** `40b8543` closed the MCP boundary;
  the 2026-08-19 `u128_boundary` fix closed the CLI boundary after a full-width
  `memo_hash` wedged every read of a live channel — the failure F-entry `F39` should
  record. Envelopes now carry `protocol: 2` and the seam refuses a mismatch by name.
- **An installable server.** `erebus_mcp.server` with a `[project.scripts]` entry point;
  platform wheels for `erebus-cli`; the canary drives the installed console script with
  nothing from the checkout. Publishing waits on the `v0.1.0` tag.

### Not proven

- Erebus has no mainnet transaction.
- The public demo is a browser simulation. It does not use a wallet or submit a transaction.
- The sprint video does not exist.
- No external operator completed a clean install.
- No automated test reaches Starknet. The seam integration test drives the real MCP
  server and the real CLI, but stops at `doctor` against dead endpoints.
- No independent reviewer reviewed the wire or settlement code.
- No evidence supports full relationship privacy.
- No release artifact is published. Wheels build and verify locally; the `v0.1.0` tag
  publishes them.
- No Erebus-specific agent skill exists.

## 4. Privacy and trust boundary

Erebus uses the direct STRK20 SDK route because the operator controls the agent account and
pool key. The route matches the [STRK20 SDK model](https://strk20-by-example.org/sdk/getting-started).

### Private and public data

**Canonical source: [privacy-model.md](./privacy-model.md).** It carries the per-step and
per-category leak tables, the four known leaks in severity order, the infrastructure that sees
the pool key, and what a disclosed record proves versus asserts. Maintain it there; this
section keeps only what the roadmap needs to plan against.

The five leaks, in the order they should be worked:

0. **The counterparty address is in public calldata.** `open_channel` emits
   `ServerAction::Append(AppendInput { recipient_addr, ... })`, and `recipient_addr` is a
   plaintext `ContractAddress` because it is a storage map key. The edge "X opened a channel to
   Y" is written in the clear, twice per pair. Upstream of our encryption; no wire-level fix.
   F38.
1. **The fifth-salt fingerprint** — 59 zero-filled bits give every message a constant shape.
   Fix is random padding. F31.
2. **Submission linkability** — every write is signed by a public account. The pool already
   permits relayed submission (nothing binds submitter to pool identity), which would hide the
   sender but not leak 0's recipient. Not implemented.
3. **The public funding leg** — shielding is a real ERC-20 transfer. No fix within this design.
4. **Note count on settlement** — six notes on an exact subset, seven with change, leaking one
   bit about payer holdings per deal. Fix is a constant-count zero-valued change note.

Leak 0 was found on 2026-08-17 and supersedes the earlier claim that the counterparty was
private. The relationship-privacy work in Phase 10 is larger than it was scoped as.

Deposit screening is enforced by the pool. A self-hosted prover does not remove that
requirement. Selective disclosure reveals data for a legitimate request, but it does not
provide automatic compliance.

### Current privacy claim

The technical preview can claim confidential negotiation content and shielded settlement.
It cannot claim that the relationship is hidden.

Relationship privacy remains the long-term product goal. It needs a written threat model,
submission unlinkability, traffic analysis, and funding-correlation work.

## 5. Current problems by product area

### 5.1 Rust SDK and protocol

| Problem | Current mechanism | Required result |
|---|---|---|
| ~~Change support stops at the SDK~~ | Aligned through MCP, mock, and agents (PR #16, `40b8543`); exercised live 2026-08-19 | Done |
| ~~The allowance path is untested against a live fee~~ | Exercised 2026-08-19 through merged code under the 2 STRK fee, receipt recorded | Done |
| One deal per channel | Settlement breaks the fixed five-note message grid | Add framed entries and deal identifiers in the final wire |
| One directional channel per sender and recipient | STRK20 derives a channel key without an index | Reuse both directional channels for framed deals, or use fresh identities |
| Wire fingerprint | The fifth salt contains 59 fixed spare bits | Randomize the spare bits without weakening decoding |
| No second wire-v2 implementation | TypeScript still implements wire v1 | Port the final wire to TypeScript and publish vectors |
| No normative wire document | Rust is the only wire-v2 authority | Publish byte and bit layouts before external integration |
| Client-side deadlines | The pool does not enforce offer expiry | Keep the limit explicit and bind external policy to signed terms |
| Settlement agreement is client policy | The pool does not compare payment with the accepted offer | Keep the SDK amount check and publish this trust boundary |
| One token per client | `ClientConfig.token` fixes one token | Move token selection to channel or operation scope |
| Reads restart from index zero | `fetch_notes` walks each note on every read | Store a read cursor and cache the immutable prefix |
| No idempotency | A lost response can cause a duplicate operation | Add operation IDs and a durable submission journal |
| No state reconstruction command | Lost handles can be derived in principle | Rebuild handles and cursors from keys and chain state |
| No signer abstraction | Rust reads a raw account-key file | Add an account signer interface before production |
| Protocol code lacks review | `Unreviewed` markers cleared 2026-08-17, but the 2026-08-19 diff was pushed to `main` on Poulav's instruction before line review | Both owners review the 2026-08-19 push, then keep review-before-merge |

### 5.2 Python SDK

| Problem | Current mechanism | Required result |
|---|---|---|
| ~~No packaged Rust binary~~ | Platform wheels ship the binary; the arm64 macOS leg is verified, the other two legs have not run | Run the Linux and x86-64 macOS legs once before the tag |
| Duplicate response mapping | The seam adapter repeats Rust fields | Add schema compatibility tests for every method |
| ~~No protocol negotiation~~ | Every envelope carries `protocol: 2`; the seam refuses a mismatch by name, and the MCP server handshakes at startup (2026-08-19) | Done |
| Timeout is global | Each call uses one 300-second limit | Use operation-specific timeouts and report the failed stage |
| Key-path safety relies on convention | Python can access the named files | Add permission checks and secret-leak regression tests |
| ~~`CLAUDE.md` contradicts the source~~ | Corrected 2026-08-19: `CLAUDE.md` now says the binding speaks protocol 2 | Done |

The Python SDK must stay a binding. It must not add hashes, salts, felt arithmetic, note
selection, signing, or cryptography.

### 5.3 MCP server

| Problem | Current mechanism | Required result |
|---|---|---|
| ~~Exact-note rules conflict with Rust change~~ | PR #16: `_require_payable` checks `0 < amount <= total`, tool text rewritten | Done |
| Missing payment looks consistent | `paid_amount is None` returns `True` | Return an unknown result, or fail closed |
| ~~Wide `memo_hash` values fail at JSON clients~~ | `d1731f4` fixed the input; the 2026-08-19 `u128_boundary` fix carried the output as hex strings | Done |
| ~~Amounts above 2^53 lose precision at JSON clients~~ | Every `u128` crosses both boundaries as a string (`40b8543`, 2026-08-19) | Done |
| `wait_for_offers` accepts invalid limits | Counts and timeouts have no range checks | Reject zero and negative values before polling |
| Polling remains expensive | Each poll repeats note reads | Add discovery subscriptions after Q3 publishes a supported endpoint |
| Concurrency behavior is unproven | `asyncio.to_thread` starts independent calls | Reproduce overlaps, serialize writes, and add concurrency tests |
| Mock is the direct-start default | A local run can look like a real product | Show the backend in health output and require explicit production mode |
| MCP dependency is too broad | Package requires `mcp[cli]>=1.2.0` | Pin the tested major range and test supported versions |
| ~~No real-seam transport test~~ | `test_seam_integration.py` (2026-08-18) drives the real server and the real CLI | Done |
| Viewing grants enter tool results | MCP hosts and transcripts can retain the secret | Add a secure export path and clear operator warnings |
| ~~No health or readiness tool~~ | `doctor` is an MCP tool and runs at startup (2026-08-19), logged rather than fatal: an operator may start first and repair second | Done; refusal deliberately not adopted |

### 5.4 Reference agents and policy

| Problem | Current mechanism | Required result |
|---|---|---|
| ~~Reference agents call the mock directly~~ | `5362ada`: same policies over real MCP transport | Done |
| ~~Policy computes exact subset sums~~ | Aligned with change-note settlement in PR #16 | Done |
| Seller policy uses a fixed strategy | One threshold and one counter path | Keep this simple, but expose policy inputs as configuration |
| No crash behavior | The loop assumes every call returns once | Resume from channel state and operation journal |
| No operator approval policy | The agent can spend within its static budget | Add per-token, per-deal, and daily limits |
| No live regression | The recorded run is manual evidence | Add an opt-in canary that records receipts and disclosure output |

### 5.5 Erebus skill

The repository contains the generic `strk20-privacy-integration` skill. It does not contain
an Erebus operator skill.

The generic skill also has upstream drift. Its freshness check found newer `get-starknet`
packages, a removed sub-account package, and a new shadow-account package.

The Erebus skill must cover these tasks:

- Detect the installed CLI, Python packages, and MCP server.
- Create an operator plan without reading secret files.
- Explain payer and payee roles before a negotiation.
- Run `doctor` before a write.
- Read note balance before the payer names a price.
- Handle structured retry and terminal errors.
- Keep viewing grants out of normal chat output.
- Report public and private data with the wording in section 4.
- Distinguish mock, Sepolia, and mainnet results.
- Link receipts to Voyager and store evidence.

The skill needs evaluations for unsafe behavior. If the agent reads a key file or invents a
receipt, the evaluation must fail. It must also fail for payee settlement or false privacy claims.

### 5.6 Operations and release engineering

| Problem | Current mechanism | Required result |
|---|---|---|
| ~~No continuous integration~~ | Added 2026-08-17 in `.github/workflows/ci.yml`. Rust, Python, and gitleaks | Add TypeScript when the GitHub Packages dependency can be resolved in CI (F8) |
| Every package is `0.0.1` | One workspace version, decided 2026-08-17 | Tag `v0.1.0` |
| No *published* install | `uvx erebus-mcp-server` works from wheels; verified by the canary from an empty environment | Push the `v0.1.0` tag, which publishes wheels and the index |
| No compatibility manifest | Pool, prover, and SDK versions can drift | Pin the pool class, ABI, prover protocol, and oracle revision |
| ~~No `doctor` command~~ | Built 2026-08-17 in `sdk/rs/src/doctor.rs`, exposed as `erebus-cli doctor`, bound through the Python seam the same day | Wire it into the MCP server and the operator skill |
| No backup and restore process | Key and state loss can be permanent | Add encrypted backup, restore, and state-rebuild procedures |
| No monitoring | Logs show local events only | Add stage timing, transaction status, retry count, and RPC health |
| No secret-safe log policy | Viewing grants and paths can enter logs | Redact grants, keys, authorization headers, and RPC secrets |
| ~~No release provenance~~ | `SHA256SUMS` and a CycloneDX SBOM over 224 components, generated from lockfiles alone, `--check` on every push | The tag publishes them |

### 5.7 Sprint delivery, and mainnet as deferred work

D14 puts the sprint on Sepolia. This section states what that costs, then keeps the mainnet
findings intact so the decision stays reversible.

#### What the sprint entry can and cannot reach under D14

| Requirement | Status under D14 |
|---|---|
| Public repository and licence | Met |
| Public demo URL | Met, `https://erebus-private-agents.vercel.app` |
| Registered in `registry.json` | Met, PR merged 2026-08-14 |
| Three mainnet pool transactions | **Not reachable.** The hub verifies each hash on mainnet |
| Public three-minute video | Reachable. Sepolia evidence only |
| Complete `strk20.json` | Partial. `transactions` stays empty |

Thirty percent of the score is a working mainnet product, and the transaction check is
mechanical rather than a judgement call. Sepolia evidence does not substitute. The entry is
therefore incomplete by construction, and the remaining work is to make the Sepolia evidence
as strong as it can be and to say plainly in the video and README which network it ran on. An
entry that overstates its network is worse than one that is honestly partial.

#### Sepolia is not free either

`get_fee_amount` read 2 STRK on Sepolia on 2026-08-16, not the 0 recorded in `.env.example`
and repeated through these documents. It was zero when those notes were written.
`apply_actions` is the only state-changing entrypoint the pool exposes, and it calls
`collect_fee` before applying anything, a `transfer_from` against the caller. So Sepolia needs
a standing allowance too, and a missing one reverts with a bare `Contract error`, the shape of
F20. The allowance path is built and merged; see §3 and D12.

This is the useful half of the fee discovery: every mechanic the mainnet path needs is
exercised by a Sepolia run, at 2 STRK instead of 6.

#### Mainnet findings, retained

The mainnet pool is
`0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a`, charging 6 STRK per
`apply_actions`. The configured mainnet account has no contract and no balance.

A 2026-08-16 search found no published mainnet proving service anywhere: not in the upstream
README, not in `strk20-by-example.org`, not in either Starknet launch post. The endpoints
Erebus holds are `alpha-sepolia` hosts and cannot serve mainnet, so Q3 may have no answer.

The alternative is self-hosting. Upstream publishes the prover as a container in its
compatibility matrix, `ghcr.io/starkware-libs/starknet-privacy/transaction-prover:PRIVACY-0.14.3-RC.2`,
with the discovery service and proof interceptor at the same tag. The cost is the footnote
under that table: the prover needs a Pathfinder node at `PATHFINDER_STORAGE_STATE_TRIES=10000`.
A mainnet Pathfinder sync takes days of wall clock and hundreds of gigabytes of disk. That lead
time is why D14 exists, and it is also why reversing D14 late is not possible: the sync has to
start days before the first mainnet transaction, not on the day it is wanted.

Self-hosting does not remove deposit screening. See Q2.

Budget if D14 reverses: three calls need at least 18 STRK in pool fees, plus deployment,
approval, deposits, and gas. D8 sets about 30 STRK for a minimum run.

## 6. Decisions for Poulav and Ishita

These decisions change the plan. Record each answer in this table before the dependent work
starts.

| ID | Decision | Current position | Owner | Needed by |
|---|---|---|---|---|
| D1 | Anchor use case | One-off service purchase or bilateral RFQ | Both | Before product copy freezes |
| D2 | First external operator | Not selected | Both | Before the clean-install test |
| D3 | Repeat deals in `v0.1.0` | Not selected | Both | Before the final wire work |
| D4 | Technical-preview privacy claim | Confidential terms and shielded settlement | Both | Decided |
| D5 | Long-term privacy goal | Relationship privacy | Both | Decided, research remains |
| D6 | Disclosure audience | Auditors and arbitrators receive grants | Both | Decided |
| D7 | Platform evidence | Platforms receive a receipt, not a grant | Both | Mechanism not selected |
| D8 | Mainnet spend limit | About 30 STRK for the minimum sprint run | Poulav | Before funding |
| D9 | External review target | Final wire only, or wire v2 plus final wire | Poulav | Before audit booking |
| D10 | Skill distribution | Erebus repo only, upstream skill repo, or both | Ishita | Before `v0.1.0` |
| D11 | Support model | Best effort, named maintainer, or funded maintenance | Both | Before `v1.0` |
| D12 | Pool allowance mechanism | Standing approval, decided 2026-08-16 | Poulav | Decided, built |
| D13 | Who provisions allowance and notes | Operator at install; not an agent tool | Both | Before the operator product |
| D14 | Sprint network | Sepolia only, decided 2026-08-16 | Both | Decided. Reversing needs days of Pathfinder lead time |

### External questions

| ID | Question | Why it matters | Owner |
|---|---|---|---|
| Q1 | Can `compile_actions` avoid receiving the full pool key? | This decides the long-term custody model | Poulav to StarkWare |
| Q2 | Can an operator receive screening access for a self-hosted prover? | Self-hosted proving does not make deposits work alone | Poulav to StarkWare |
| Q3 | What are the supported mainnet prover and discovery URLs? | Deferred by D14. Still blocks the mainnet canary and `v1.0` | Poulav to StarkWare |
| Q4 | Which pool, prover, ABI, and SDK revisions form one supported set? | Version drift can cause silent note failures | Poulav to StarkWare |
| Q4a | Which transaction-prover tag matches deployed pool class `0x67dddd89`? | Upstream documents a class deployed on neither network, see below. Applies to Sepolia too | Poulav to StarkWare |
| Q5 | Which relayer or paymaster path supports direct SDK operators? | Submission unlinkability depends on it | Both to StarkWare or AVNU |

Q4 has a partial answer as of 2026-08-16. Upstream's compatibility matrix documents the
Privacy Pool at class hash
`0x52107fadffab71bdcbb6b2ccb68ba3e1b5558d94036538053e159d3076ad633`, tag `PRIVACY-0.14.3-RC.0`.
The live mainnet pool at `0x040337b1...812a` is class
`0x67dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d`, read over RPC on
2026-08-16. Sepolia runs the same class. Upstream therefore documents a pool class deployed on
neither network, so selecting a prover image by matching the published matrix would pair
Erebus against the wrong pool version. Sepolia and mainnet sharing one class is the useful half
of this: the mainnet port is configuration, not re-derivation, and no hash preimage,
storage slot, or calldata layout changes.

The local `starknet-privacy` checkout is behind upstream `main` and a `PRIVACY-0.14.3-RC.5` tag
now exists. Refresh it before pinning any revision.

## 7. Delivery plan

### Phase 0: Make the working tree reviewable

Target: 2026-08-14 to 2026-08-15. Mostly complete on 2026-08-16.

Owner: Poulav. Ishita reviews the shared interface changes.

Work:

1. ~~Split the change-note patch from observer and documentation edits.~~ Not done. `2c91448`
   carried the observer and documentation edits with the protocol change, and `94a1b4c` did
   the same. Both merged. Not worth unpicking; do it for the next protocol commit.
2. ~~Remove accidental prose corruption from `docs/friction.md`.~~ Done.
3. ~~Review `select_notes` bounds, overflow behavior, fallback behavior, and input ordering.~~
   Done. Its `Unreviewed` marker was cleared before the merge.
4. Review change-channel setup, note indices, salts, phase ordering, and collision checks.
   **Open.** This is the last `Unreviewed` marker, at `sdk/rs/src/channel.rs:516`. The claim
   to confirm is that contiguity is per subchannel, which is what makes it safe to interleave
   two channels' notes in one action set. Check `_client_apply_actions` in `privacy.cairo`.
5. Record the shared interface decision for change-making. **Open.** Owed to Ishita, and it
   blocks all of phase 2.
6. ~~Update the test baseline in one status document.~~ Done: 213 Rust, 56 Python, 38 TypeScript.
7. Create tracked issues for every accepted roadmap item. **Open.**

Exit:

- Each protocol change has a small commit and focused tests. Partly: tests yes, small no.
- No unrelated prose and protocol change share one commit. **Missed on both commits.**
- Poulav removes each `Unreviewed` marker only after a line review. One marker left.
- Ishita accepts the changed balance and settlement result shapes. **Open.**

### Phase 1: Complete the sprint entry on Sepolia

Target: 2026-08-16 to 2026-08-24. Rewritten for D14.

Owners: Poulav handles the chain run. Ishita handles the recording. Both review public claims.

The mainnet steps this phase used to carry are now §5.7's deferred block. What remains is
making the Sepolia evidence complete and honest.

Work:

1. Run `erebus-cli doctor` for each identity that will write, and act on its repairs until
   `ready` is true. It reads the live fee and allowance, so it replaces doing that by hand.
2. Size the approval from the planned write count rather than from one write.
   `AllowanceReport::covers` answers it, and fees and deposits draw on one budget.
3. Approve the pool from `0x032bb394...c805`, which currently reads zero.
4. Run one full negotiation and settlement end to end, through the MCP servers, on current
   `main`. This is the first execution of change notes, the allowance path, and the receipt
   fields against a chain that charges a fee.
5. Confirm the receipt succeeded, contains pool events, and that `selected_input` equals the
   paid amount plus `change`. That arithmetic has only ever been checked in tests.
6. Point `scripts/observer.py` at the new settlement with no key and record what it recovers.
7. Run a viewing-key disclosure against the same deal and record the reconstruction.
8. Rewrite the demo video script. The previous one at `docs/demo-video-script.md` was deleted
   on 2026-08-16 and it described a mainnet section that will not exist.
9. Record the three-minute walkthrough. State the network out loud and on screen. Show
   `doctor` failing on a missing allowance and then passing: under D14 the mainnet 30% is
   forfeit, so integration depth and operator quality are what is left to demonstrate, and a
   bare `Contract error` turned into a repair instruction is the clearest evidence of both.
10. Upload the video and put its URL in `strk20.json`.
11. Put the Sepolia transaction hashes in the demo evidence section, labelled as Sepolia.
12. Say in the README and on the demo page that no mainnet transaction exists.

Exit:

- One negotiation and settlement recorded end to end on current `main`.
- `demo_video` and `demo_url` are public without login.
- `transactions` stays empty, because the hub verifies mainnet only. That gap is stated
  rather than papered over.
- The public demo labels its simulated flow, its Sepolia evidence, and its absent mainnet
  evidence as three separate things.

### Phase 2: Align Rust, Python, MCP, and agents

Target: 2026-08-15 to 2026-08-21.

Owners: Poulav owns Rust. Ishita owns MCP and agents. Both own the seam contract.

Work:

1. Finish the change-note review from phase 0.
2. Replace exact-payability language with sufficient-balance language after both owners approve the interface.
3. ~~Return selected input value and change where an agent needs that fact.~~ Rust done: `SettlementReceipt.selected_input` and `.change`, decimal strings. Mirror in the seam and MCP.
4. Update `NoteBalance`, the mock, seam adapter, MCP tools, policies, and guides together.
5. Add one cross-layer test for `5 STRK -> 3 STRK + 2 STRK change`.
6. Change `memo_hash` transport to a canonical hex string.
7. Publish its preimage and truncation rules.
8. Make missing payment evidence return an unknown consistency result.
9. Fix the observer version classifier and retain v1 and v2 controls.
10. Reconcile README, `CLAUDE.md`, SDK README, runbook, and architecture status.

Exit:

- Rust, Python, MCP, mock, agents, and docs describe one settlement rule.
- A wide `memo_hash` survives MCP, Python, JSON, CLI, and Rust.
- No API reports a missing payment as a consistent settlement.
- The full local gate passes from a clean checkout.

### Phase 3: Make the MCP path a release candidate

Target: 2026-08-19 to 2026-08-24.

Owner: Ishita. Poulav reviews Rust boundary and chain evidence.

Work:

1. Add an MCP client implementation for the reference buyer and seller.
2. Run the same policy against mock MCP servers and seam MCP servers.
3. Add a real CLI transport test without replacing the CLI with `StubSeam`.
4. Add opt-in Sepolia and mainnet canary tests.
5. Serialize proof-bearing writes for one identity.
6. Add tests for concurrent read and write calls.
7. Add range checks to `wait_for_offers`.
8. Pin the supported MCP SDK major range.
9. Expose backend, role, chain, pool, and CLI version through a health tool.
10. Require explicit mock mode outside tests.
11. Add secret-redaction tests for viewing grants and configuration.

Exit:

- A real MCP client drives `server.py -> sdk/py -> erebus-cli`.
- The recorded canary includes a real receipt and revealed record.
- A payee process cannot settle through any supported entry point.
- Concurrent calls do not corrupt state or submit conflicting writes.

### Phase 4: Build the Erebus skill

Target: 2026-08-20 to 2026-08-26.

Owner: Ishita. Poulav reviews protocol and privacy instructions.

Work:

1. Create an Erebus-specific skill with install, plan, operate, and diagnose modes.
2. Make the skill inspect the repository and installed release before it acts.
3. Make the skill run `doctor` before each funded workflow.
4. Add payer and payee negotiation procedures.
5. Add structured failure procedures by retry class.
6. Add a disclosure procedure that keeps the grant out of ordinary output.
7. Add evidence capture for receipts, events, and Voyager links.
8. Add privacy wording from section 4.
9. Add mock, Sepolia, and mainnet labels to every result template.
10. Add evaluation fixtures for unsafe and false-success behavior.
11. Refresh or replace stale references inherited from the generic skill.
12. Decide D10 and publish the skill in the selected locations.

Exit:

- A fresh Codex or Claude session can discover and use Erebus.
- The skill never reads a private-key file.
- The skill refuses payee settlement.
- The skill does not call a mock result on-chain evidence.
- The skill reports content privacy and traffic privacy separately.

### Phase 5: Package the operator product

Target: 2026-08-20 to 2026-08-28.

Owners: Poulav owns Rust artifacts. Ishita owns Python packaging and operator experience.

Work:

1. ~~Add continuous integration for Rust, Python, TypeScript, docs, and secret scanning.~~
   Done 2026-08-17 as `c3960de`, minus TypeScript: `sdk/ts` builds against a GitHub Packages
   dependency that a bare checkout cannot resolve (F8), and a job gated on a token that may
   not exist would report green without testing anything.
2. ~~Build `erebus-cli` for macOS arm64, macOS x86-64, and Linux x86-64.~~ macOS arm64 and
   Linux x86-64 build and install clean, both verified 2026-08-19. Intel macOS is dropped:
   its `macos-13` runner is no longer assigned, and a cross-build from arm64 would ship a
   binary CI never executed. Unsupported for now, and the release notes say so.
3. Package each binary with the matching Python wheel. The binding resolves it with
   `shutil.which("erebus-cli")`, so the wheel must place it on `PATH`.
4. Make `uvx erebus-mcp-server` find the packaged binary.
5. ~~Add `erebus doctor` before the first write.~~ Built 2026-08-17 as `erebus-cli doctor`,
   bound through the Python seam the same day as `2d69eda` along with `allowance` and
   `approve`. Tests for those three landed 2026-08-17 by Ishita in `40b8543`
   (`sdk/py/tests/test_seam.py`). Still to do: the MCP server at startup, and the
   operator skill.
6. ~~Inspect key permissions, state permissions, RPC health, and prover compatibility.~~ Done.
   Mode bits are checked on unix only; Windows has none, so that arm is skipped rather than
   guessed at.
7. ~~Inspect chain ID, pool address, pool version, fee, and proof-validity window.~~ Done. The
   chain id is read from the RPC and compared with configuration rather than trusted, because
   it is part of every channel-key preimage and a mismatch reads as not-found everywhere.
8. Inspect registration, allowance, STRK balance, private balance, and note maturity.
   **Partly done.** Registration, allowance against the live fee, and public balance are in.
   Private note balance and maturity are not: `note_balance` is O(notes) over two passes, and
   a pre-flight that walks discovery twice is one an operator stops running.
9. ~~Report each failed inspection with one direct repair instruction.~~ Done, and enforced by
   a test rather than by convention.
10. Add encrypted backup and restore procedures.
11. Publish checksums, build metadata, dependency licenses, and an SBOM.
12. Run the install on a machine that does not contain the repository.

Exit:

- One command installs the MCP server and matching CLI.
- `doctor` finds all known setup failures before proving starts.
- The clean machine completes the mock flow and one funded canary.
- Release artifacts contain no keys, state files, or local endpoint secrets.

### Phase 6: Publish `v0.1.0`

Target: 2026-08-27 to 2026-08-31.

Owners: Both.

Work:

1. Freeze the interface and release candidate.
2. Run the full local gate from a clean checkout.
3. Run one low-value canary through the release artifacts. Sepolia while D14 holds. A
   `v0.1.0` released without a mainnet canary must say so in its release notes.
4. Make sure that the demo, README, roadmap, and manifest use the same evidence.
5. Publish install steps, current limits, recovery limits, and trust assumptions.
6. Tag `v0.1.0` and publish signed release artifacts.
7. Make sure that the sprint hub reads the final manifest.

Exit:

- An external operator can install and run the technical preview.
- Every public claim links to source, a test, or a transaction.
- The release notes state that the product is unaudited.
- The release notes state that relationship privacy is not complete.
- The release notes state which platforms are supported: Linux x86-64 and macOS arm64.
  Intel macOS is not.

### Phase 7: Reliability and recovery

Target: September 2026.

Owners: Poulav owns SDK state. Ishita owns agent recovery and operator output.

Work:

1. Add operation IDs to every write request.
2. Journal preflight, proof, transaction hash, receipt, and state commit.
3. Reconcile the journal with chain state after restart.
4. Add fault injection at every write boundary.
5. Rebuild channel handles and cursors from keys and chain state.
6. Cache immutable note prefixes and read from the last known cursor.
7. Add discovery-provider support after Q3 defines a supported endpoint.
8. Add multi-token client state.
9. Add account signer interfaces for hardware, wallet, or session signers.
10. Define backup, restore, key-loss, and key-rotation behavior.

Exit:

- A process can stop at every write stage and resume without duplicate state.
- A lost state directory can be rebuilt from keys and chain data.
- An unchanged channel read uses a constant number of RPC calls.
- One client can operate two configured tokens without state collision.

### Phase 8: Final wire and repeat deals

Target: October 2026. Start only after D3 and the threat model are complete.

Owner: Poulav. Ishita reviews API and agent behavior.

Work:

1. Write the wire and relationship threat model before code changes.
2. Design variable-width entries for offers, settlements, and future records.
3. Add deal identifiers inside the authenticated message.
4. Randomize spare bits and remove the fixed fifth-salt classifier.
5. Preserve legacy wire-v1 and wire-v2 reads.
6. Port the final wire to TypeScript.
7. Publish normative vectors and a byte-level specification.
8. Add two-deal tests for the same address pair.
9. Add observer tests for content, shape, cadence, and sender linkability.
10. Freeze the wire before the external cryptographic review.

Exit:

- Two agents complete two deals through the same directional channel pair.
- Rust and TypeScript agree on every published vector.
- Random public notes and Erebus records have no fixed shape classifier.
- The specification is sufficient for a third implementation.

### Phase 9: Disclosure and platform evidence

Target: October to November 2026.

Owners: Poulav owns cryptographic scope. Ishita owns delivery and operator workflow.

Work:

1. Compare Erebus grants with the selective-disclosure flow in Stellar Private Payments.
2. Separate disclosure generation, delivery, and independent verification.
3. Encrypt each grant to the intended recipient.
4. Add per-deal scope and define what expiry can stop.
5. Prevent normal MCP transcripts from storing the grant secret.
6. Select the platform receipt mechanism in D7.
7. Bind the receipt to deal ID, terms commitment, token, amount, and transaction hash.
8. Let a platform determine that settlement occurred without learning the counterparty.
9. Add walletless verification for the receipt format.
10. Document what each disclosure proves and does not prove.
11. State that revocation cannot erase data that a recipient already learned.

Exit:

- A copied grant is not sufficient without the recipient key.
- A per-deal grant cannot reveal another deal in the same channel.
- A platform can determine that one settlement occurred without a channel viewing grant.
- Missing payment evidence never produces a positive result.

### Phase 10: Relationship-privacy research

Target: no release date until the threat model has measurable exit conditions.

Owners: Both. External review is required.

Research areas:

1. Submission unlinkability through relayers, paymasters, or private sub-accounts.
2. Timing and cadence leakage across negotiation rounds.
3. Cover traffic, delay, and batching costs.
4. Correlation between public shielding and later settlement.
5. Account reuse and cross-deal linkage.
6. Prover, RPC, screener, and auditor metadata.

The work must define the observer, auxiliary data, anonymity set, and success metric.
Random wire padding alone does not complete this phase.

Exit:

- The threat model names each observer and its available data.
- The test harness measures linkage against a stated baseline.
- The public claim matches the measured result.
- If the result misses the target, the team removes the relationship-privacy claim.

### Phase 11: Review, audit, and `v1.0`

Target: after phases 7 through 10 settle the shipping scope.

Owners: Both. Independent reviewers own the external findings.

Work:

1. Finish internal line review for all protocol-critical Rust and Python code.
2. Audit the final wire, settlement, disclosure, and recovery design.
3. Resolve all critical and high findings.
4. Repeat dependency, license, and secret audits.
5. Reproduce release artifacts from the tagged source.
6. Run the final mainnet canary from packaged artifacts.
7. Publish support ownership, security contact, and incident procedure.

The final canary must include:

- Fresh registration and shielding.
- If repeat deals enter `v1.0`, two deals between the same pair.
- A payment that needs change.
- A process stop and recovery during settlement.
- A per-deal disclosure to an independent recipient.
- A platform receipt without a grant.
- Observer results for every public privacy claim.
- Measured gas, pool fee, and stage timing.

Exit:

- The external review covers the exact tagged code.
- The release has no unresolved critical or high finding.
- The canary passes through the packaged product.
- Public documentation matches the audited scope.

## 8. Dependency order

```text
Sepolia allowance -> live settlement run -> observer and disclosure evidence
                     -> video -> sprint entry            (the D14 path)

Pathfinder mainnet sync -> prover container -> mainnet access -> mainnet transactions
  (deferred by D14; days of lead time, so reversing D14 late is not possible)

Change-note review -> shared interface decision -> MCP and agent alignment
                     -> cross-layer settlement test

Read cursor -> efficient long poll -> discovery subscription

Threat model -> final wire -> TypeScript oracle and spec -> external crypto review

Operation journal -> crash recovery -> clean operator canary -> v1 release

Recipient-bound grants -> platform receipt -> disclosure review
```

Do not start the external wire review before the final wire freezes. Do not claim repeat
deals before the framing test passes.

## 9. Ownership and team process

### Ownership

| Area | Primary owner | Required reviewer |
|---|---|---|
| Rust SDK, wire, settlement, execution | Poulav | Poulav line review, then external reviewer |
| Cairo probes and upstream compatibility | Poulav | Poulav |
| Python SDK seam | Shared | Both |
| MCP server and tools | Ishita | Poulav for protocol boundaries |
| Reference agents and policy | Ishita | Poulav for payment direction |
| Erebus skill and evaluations | Ishita | Poulav for privacy and custody wording |
| Demo, video, and agent narrative | Ishita | Both |
| Mainnet accounts and transactions | Poulav | Ishita makes sure that evidence is understandable |
| Release gate and public claims | Shared | Both |

### Working rhythm through 2026-08-31

1. Hold one short handoff each day.
2. Review blockers, decisions, and live evidence only.
3. Put each interface change in the decision table before implementation.
4. Keep Rust protocol commits separate from MCP, agent, and documentation commits.
5. Attach a test or receipt to each completed task.
6. Run one integration session every two days.
7. Freeze public copy three days before the deadline.

After the sprint, use two planning sessions each week. One session covers protocol and
security. The other covers product integration, release, and operator feedback.

## 10. Priority board

This board and the phases in §7 are two indexes over the same work, not two plans. §7 says
when something happens and who owns it; this board says what blocks what. An item can be
"P1" here and "Phase 5 step 6" there, and `doctor` is exactly that. When they disagree, the
board decides what to do next and §7 decides what done means.

### P0: Sprint and safety blockers

- [x] Public repository and Apache-2.0 license.
- [x] Public browser demo and GitHub Website field.
- [x] Registered in the sprint `registry.json`. Merged 2026-08-14.
- [x] Merge the change-note work. Merged 2026-08-15 as `8dd9b74`, PR #14.
- [x] Merge the pool-allowance path. Merged 2026-08-16 as `79bf78e`, PR #15.
- [ ] Grant the Sepolia allowance to `0x032bb394...c805`. `doctor` reports it as the only
      failing check; everything else in that configuration passes. No longer blocks the
      evidence run, which used `erebus-g`/`erebus-h` instead; still worth closing so the
      runbook's default identity works.
- [x] One full Sepolia run on merged code, with a receipt, observer output, and a
      disclosure. Done 2026-08-19: settlement `0x4191fe47...f341` with change, a
      disclosure reconstructed by an uninvolved identity, and observer output showing
      content not recovered / traffic classified. It also found and fixed the
      u128-boundary read wedge. `docs/runs/2026-08-19-sepolia-run.md`.
- [ ] Rewrite the demo video script, deleted 2026-08-16.
- [ ] Public three-minute video that names its network.
- [ ] `strk20.json` with the video URL, and an explicit note that `transactions` is empty.
- [x] Clear the last `Unreviewed` marker at `sdk/rs/src/channel.rs:516`. Cleared 2026-08-17.
      No `Unreviewed` marker remains anywhere in `sdk/rs/src`.
- [x] Record the change-making interface decision with Ishita. Decided 2026-08-17: drop
      `can_pay_exactly`, and the receipt reports `selected_input` and `change`.
- [x] Align change-making across MCP, mock, and agents. PR #16: `_require_payable` checks
      `0 < amount <= total`, `can_pay` removed, tool text rewritten.
- [x] Mirror `selected_input` and `change` through the Python seam and MCP. Done 2026-08-17
      as `40b8543`, so an agent can now see what a settlement actually spent.
- [ ] Make missing payment evidence fail closed.
- [x] Fix wide `memo_hash` transport. Closed 2026-08-17 as `d1731f4`: MCP tools take a hex
      string or an int and return hex. The frozen `interface.py` seam still types it `int`;
      parsing happens at the MCP boundary where the JSON problem is. Breaking for readers.
- [x] Convert `OfferTerms.amount` to a decimal string. Done 2026-08-17 as `40b8543`, which
      also covered `total`, `agreed_amount`, `paid_amount`, and the note list on the way
      back. That was the last `u128` crossing the MCP boundary as a JSON number, so the
      whole class of silent rounding above 2^53 is now closed.

Deferred by D14, not cancelled: mainnet proving path, funded mainnet account, three mainnet
transactions. See §5.7.

### P1: Technical-preview release

- [x] Real MCP-to-CLI integration test. Done 2026-08-18 in
      `mcp-server/tests/test_seam_integration.py`. Drives a real MCP tool call through the
      Python seam into the real `erebus-cli`, which nothing did before: the tool tests use
      the mock, the seam-client tests use a stub holding payloads captured on 2026-07-31,
      and `sdk/py` mocks the subprocess. Frozen payloads cannot notice the binary changing
      shape, and those predate live wire v2, change notes, and string amounts.
      `doctor` is what makes it runnable without a chain, a prover, or funds: it is
      read-only and reports faults instead of raising, so an unreachable RPC exercises tool
      registration, config marshalling, key-file paths, the envelope, and report
      translation in under two seconds. Two of the three tests fail if the backend is
      switched to the mock, which is the check that they test what they claim.
      Still manual, because they need a funded identity: everything that writes.
- [x] MCP-client reference agents. Done 2026-08-17 by Ishita as `5362ada`.
- [ ] Erebus operator skill and unsafe-behavior evaluations.
- [ ] Both owners review the 2026-08-19 push (`749cdd4` sdk, `c8741ff` mcp-server,
      `bb4e636` scripts/docs). It landed on Poulav's instruction ahead of the usual
      review; Poulav owns the Rust lines, Ishita the mcp-server lines.
- [x] Run the remaining wheel legs once before the tag. Done 2026-08-19: Linux x86-64
      built, installed into a clean environment, and ran, for the first time. Intel macOS
      was dropped the same day rather than fixed; see Phase 5 step 2.
- [x] Re-verify the MCP read path against the fixed CLI over freshly started servers.
      Done 2026-08-19. A fresh `erebus_mcp.server` on the seam backend, driven by a real
      MCP client against the live settled channel `ch_620b53e1...`: `doctor`,
      `get_note_balance`, `read_channel_state`, and `wait_for_offers` all return, with the
      full-width `memo_hash` `0x1c7d05e73d64d73c21438174cd1b55ea` intact through the
      transport and amounts as strings. The session's old servers still fail, which is the
      stale-process case the startup handshake now names.
- [ ] Write F39: the u128 JSON-boundary wedge, its mislabeled error, and the stale-server
      skew it exposed. Material in `docs/runs/2026-08-19-sepolia-run.md`. Poulav's voice.
- [x] Platform wheels containing `erebus-cli`. Done 2026-08-17 as `5f7bc09` and `3c32561`.
      `erebus-cli` is its own package so the pure-Python half stays one wheel; a build hook
      forces the platform tag, because hatchling defaults to `py3-none-any` and a binary
      wheel tagged `any` installs anywhere and fails on first call. Built and verified
      locally on arm64 macOS; the Linux and x86-64 macOS legs have not run yet.
      Not published anywhere, so `uvx erebus-mcp-server` still does not work.
- [x] `doctor` and health tools. Rust done 2026-08-17. Python seam done 2026-08-17 as
      `2d69eda`, which also bound `allowance` and `approve` because `doctor`'s own repair
      advice for the commonest failure is "run approve". Tests and the MCP tool landed
      2026-08-17 by Ishita in `40b8543`. Still to do: call it at MCP startup and from the
      operator skill.
- [x] Continuous integration and secret scanning. Done 2026-08-17 as `c3960de`. Three jobs:
      Rust (fmt, clippy `-D warnings`, `test --all-targets`, docs `-D warnings`, publishes
      `erebus-cli`), Python (downloads that binary, runs the workspace suite), and gitleaks
      over full history. Green on first run.
      The Python job asserts the binary exists before pytest, because `sdk/py` tests skip
      themselves when it is absent: verified that a missing binary skips all 12 and still
      exits 0, so a lost artifact would have produced a green run that tested nothing.
      `sdk/ts` and the Cairo probes are excluded on purpose, reasons in the workflow file.
- [x] Clean-machine install and canary. Done 2026-08-18 in `.github/workflows/canary.yml`.
      Installs the wheels into an empty environment on Linux and macOS, on Python 3.11 and
      3.13, verifies checksums first, and drives a real MCP tool call through them. Needs no
      chain, prover, funds, or keys with value, because `doctor` reports faults instead of
      raising. Rehearsed locally end to end: the chain resolved from wheels alone, the
      server started, ten tools registered, `doctor` returned all ten checks.
      Two things this found. `shutil.which` returns `None` when the environment is not on
      `PATH`, which is the normal case for a launcher spawning the server, so the canary
      exercises `binary_path()` as well and that fallback is now verified rather than
      assumed. And the release is five wheels, not three: `erebus-mcp-server` depends on
      `erebus-sdk` depends on `erebus-cli`, so shipping only the binary leaves the chain
      unresolvable. The pure-Python wheels are now built alongside.
      Unblocked 2026-08-19: the server moved into the package (below) and the canary now
      launches the installed `erebus-mcp-server` console script, nothing from the checkout.
- [x] One current status document. Done 2026-08-18 as `docs/status.md`: one page, and the
      declared tiebreaker when documents disagree. Nine documents describe this system,
      written across three weeks in which the privacy claim changed twice, which is the
      whole reason it was needed.
      Test counts in it were verified by running all three suites, not copied forward.
- [x] Reconcile the stale guides. Done 2026-08-18. The `runbook.md` evidence boundary was
      rewritten: it said wire v2 had not completed a live run, which stopped being true on
      2026-08-07. `usecases.md` and `production-gaps.md` §4 carry dated notices naming the
      specific wrong claim and pointing at `status.md`, rather than being rewritten
      wholesale, because both are dated records and silently editing them would erase when
      the project believed what.
- [ ] Tagged `v0.1.0` release with checksums and SBOM. Version policy decided 2026-08-17:
      one version across the workspace, `0.0.1` now, incrementing toward `0.1.0`.
      Checksums and SBOM done 2026-08-18. `scripts/sbom.py` reads `Cargo.lock` and
      `uv.lock` and emits CycloneDX 1.5 covering 224 components, 182 cargo and 42 pypi. No
      network and no third-party tool, because a generator that resolved anything itself
      could disagree with what ships. Output is byte-identical across runs, so a diff means
      a dependency really moved, and it validates clean against the official CycloneDX 1.5
      schema. `--check` fails if any dependency lacks a hash and runs on every push.
      The `server.py` blocker cleared 2026-08-19: `erebus_mcp.server` with
      `build_server()`/`main()`, a `[project.scripts]` entry point, and a shim at the old
      path so `mcp dev` and existing stdio configs keep working. Done on Poulav's
      instruction ahead of the coordination the previous note asked for, so Ishita reviews
      it before it lands; the local canary rehearsal from wheels alone passed. What remains
      for the tag: both owners' review of the 2026-08-19 working tree, then push the tag.
      Publishing decided 2026-08-18 (D-wheels): GitHub Releases plus a static package index
      on GitHub Pages. Nothing leaves GitHub and no PyPI account is needed. Built the same
      day: the `release` job in `wheels.yml` fires on a `v*` tag, creates the release with
      wheels, raw per-target binaries, `SHA256SUMS` and `sbom.json`, then generates and
      deploys the index.
      GitHub Packages was checked and ruled out: it serves npm, RubyGems, Maven, Gradle,
      NuGet and Docker, and has no Python registry, so Release assets are files at URLs that
      no installer can resolve a dependency chain from. `scripts/build-index.py` writes the
      PEP 503 index that fixes that, holding links rather than files so Pages stays small.
      Rehearsed against a local server: `uv pip install --extra-index-url <index>
      erebus-mcp-server` resolved all three packages and the installed binary answered.
      `--extra-index-url` and not `--index-url`, because PyPI still serves `mcp`.
      One trap handled and tested: deploying Pages replaces the whole site, so an index
      built from one tag would delete every earlier version's links and break pinned
      installs. The job merges the live index in first, verified against a simulated
      earlier release.

### P2: Operator alpha

- [ ] Durable operation journal and idempotency.
- [ ] Crash and chain-state recovery.
- [ ] Read cursor, cache, and discovery support.
- [ ] Multi-token client.
- [ ] Signer abstraction.
- [ ] Backup and restore process.
- [ ] Secret-safe monitoring.

### P3: Protocol and privacy

- [ ] Written relationship threat model.
- [ ] Final framed wire with randomized spare bits.
- [ ] Repeat deals through the same directional channel pair.
- [ ] TypeScript final-wire oracle and normative vectors.
- [ ] Recipient-bound, time-limited, per-deal grants with documented limits.
- [ ] Outcome-only platform receipts.
- [ ] Submission unlinkability research.
- [ ] Independent cryptographic and security review.

## 11. Explicitly deferred work

The current plan does not include these products:

- A hosted multi-tenant Erebus service.
- A consumer dashboard or wallet.
- Free-text encrypted messaging.
- Multi-party negotiation.
- High-frequency order books.
- Sealed-bid auctions.
- Delivery-versus-payment enforcement.
- A token or economic layer.
- Cross-chain settlement.
- A custom pool deployment without a separate auditor and screening governance plan.

Add one only after D1 changes and the owners accept its security and maintenance cost.

## 12. Production finish line

`v1.0` is complete only when every statement is true:

- [ ] The final wire has two implementations and a public specification.
- [ ] Protocol-critical code has internal review and independent external review.
- [ ] An operator can install Erebus with one command.
- [ ] `doctor` finds configuration, fee, allowance, prover, RPC, and key-permission errors.
- [ ] The product resumes safely after each write-stage failure.
- [ ] The product can rebuild state from keys and chain data.
- [ ] Disclosure grants are recipient-bound and scoped to one deal.
- [ ] Platforms can inspect a settlement result without receiving a viewing grant.
- [ ] Every privacy claim has an observer test and a written evidence boundary.
- [ ] One packaged mainnet canary covers payment change, recovery, disclosure, and receipts.
- [ ] Release artifacts are reproducible and contain no secret material.
- [ ] A named maintainer owns security reports and release support.

## 13. Source map

The roadmap uses these repository authorities:

- `README.md` for public status and evidence.
- `ARCHITECTURE.md` for component boundaries and the shared interface.
- `sdk/rs/src/client.rs` for product workflow behavior.
- `sdk/rs/src/wire.rs` for the wire format.
- `sdk/rs/src/execution.rs` for the prove and submit path.
- `sdk/rs/src/disclosure.rs` for grant and reconstruction behavior.
- `sdk/py/src/erebus/_seam.py` for the Python-to-Rust boundary.
- `mcp-server/src/erebus_mcp/tools.py` for agent-facing behavior.
- `agents/src/erebus_agents/` for reference policy behavior.
- `docs/friction.md` for measured failures and workarounds.
- `docs/custody-design.md` for key ownership and deployment options.
- `strk20.json` for sprint evidence.

If this document disagrees with current source or live receipts, those sources take priority.
