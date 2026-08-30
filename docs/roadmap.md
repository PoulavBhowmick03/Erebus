# Erebus product roadmap

Last source reconciliation: 2026-08-30, against source commit `0bf51f1` plus the evidence
updates recorded below.

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
6. The Erebus skill teaches an agent how to install, operate, and diagnose the stack.
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

| Target                     | Purpose                                                    | Required result                                                                               |
| -------------------------- | ---------------------------------------------------------- | --------------------------------------------------------------------------------------------- |
| Sprint entry               | Qualify for judging by 2026-08-31                          | Public demo, public video, three mainnet pool transactions, and a complete `strk20.json`      |
| `v0.1.0` technical preview | Let an external operator evaluate Erebus                   | One-command install, role-bound MCP flow, current protocol limits, and reproducible evidence  |
| `v1.0` production release  | Let an operator use real value under a written trust model | Final wire, crash recovery, reviews, audit, release controls, and a successful mainnet canary |

The sprint entry can finish without production readiness. The README and demo must keep
that distinction clear.

**D14 changed on 2026-08-28.** The owners approved a bounded mainnet canary. Two
registrations and two directional channel opens have now succeeded. The hub's three-hash
requirement is met, but the actual mainnet settlement workflow remains blocked on screening.

## 3. Current evidence

This section records what exists on 2026-08-28. Later sections describe the missing work.

### Committed and working

- Wire v3 uses AES-256-GCM-SIV, authenticated deal IDs, masked spare bits, and framed
  records. New channels use v3. Historical v1 and v2 channels remain readable.
- Multiple live Sepolia negotiations settled. The wire-v2 reference run,
  `0x14b38e9dbc65f0749be6da2fa05dd2713f8c4c893bac707961c73e616b34cb3` (2026-08-07), and
  the merged-code run with change, a third-party disclosure, and observer output,
  `0x4191fe47a0b062605a7bbc08dd40eafdefcd52de4fd0288e8315eb48ee2f341` (2026-08-19, see
  `docs/runs/2026-08-19-sepolia-run.md`).
- Five wire-v3 Sepolia settlements completed on 2026-08-22 through one channel pair.
  Evidence includes repeat prices, a 19 STRK settlement, and recipient-bound per-deal
  disclosure. See `docs/runs/2026-08-22-sepolia-wire-v3-run.md`.
- Reference buyer and seller policies completed a live wire-v3 MCP negotiation and
  settlement on 2026-08-22. See `docs/runs/2026-08-22-agents-mcp-wire-v3.md`.
- `scripts/observer.py` recovers wire-v1 terms and does not recover wire-v2 content.
- The Rust client can open channels, write offers, read state, settle, shield, grant, and reveal.
- The Python seam uses protocol 4, requires caller-supplied operation IDs for writes, and
  passes key-file paths to Rust.
- Current source exposes thirteen MCP tools over stdio. The published `v0.1.0` wheel exposes
  ten Protocol 2 tools. Protocol 4 and its recovery tools wait for the `v0.2.0` release.
- Payer and payee roles prevent the payee from calling `accept_and_settle`.
- `wait_for_offers` reduces agent tool calls, but it still polls the chain.
- The public demo runs at `https://erebus-private-agents.vercel.app`.
- The repository is public and uses Apache-2.0.
- Account A completed a standalone mainnet registration on 2026-08-28:
  `0x6597adb6581bb1910d30b31139fe871665db4cc61fefef8120b89773528e54c`, accepted in block
  `14004848`. See `docs/runs/2026-08-28-mainnet-registration.md`.
- Account B completed a standalone mainnet registration on 2026-08-29:
  `0x572260b651525ea39ef717721bcc9fefc89a2087894654efb38111e09267189`, accepted in block
  `14031230`. See `docs/runs/2026-08-29-mainnet-preflight.md`.
- Account A opened a mainnet channel to B on 2026-08-30:
  `0x395563b33df0d121ef9a7aa720da7cbbc378f7c0ed9849d2e034a1a08ada09a`, accepted in block
  `14100846`.
- Account B opened the reverse mainnet channel to A on 2026-08-30:
  `0x467295d1d167607cf321cb6076f1ccd1b08f36d4c7575cd8e9dd242c4c01964`, accepted in block
  `14101246`. See `docs/runs/2026-08-30-mainnet-channels.md` for both channel receipts and
  their recovery evidence.
- The Rust client grants and reads the pool's STRK allowance, and `agent.sh fund` sizes its
  approval as deposit plus the live fee.
- `erebus-cli doctor` inspects files, endpoints, pool, registration, allowance, and gas
  balance read-only, and reports a repair instruction per fault.
- Settlement receipts report selected input value and change.
- The current working tree passes 351 Rust tests, 156 Python tests, and 43 TypeScript tests.
  Seven live Rust tests are intentionally ignored: two shared-prover probes, three guarded
  mainnet registration canaries, one account-rotation canary, and one screening probe.
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

- Mainnet has two registrations and two directional channel opens. No mainnet shield, offer,
  settlement, recovery, or disclosure has completed.
- The public demo is a browser simulation. It does not use a wallet or submit a transaction.
- The 180-second sprint video is public and labels mainnet setup separately from the complete
  Sepolia workflow.
- No external operator completed a clean install.
- No automated test reaches Starknet. The seam integration test drives the real MCP
  server and the real CLI, but stops at `doctor` against dead endpoints.
- No independent reviewer reviewed the wire or settlement code.
- No evidence supports full relationship privacy.
- No Protocol 4 release artifact is published. The public `v0.1.0` artifacts speak Protocol 2.
- The `v0.1.0` release publishes the wheel chain, checksums, SBOM, and static package index.
- The Erebus operator skill exists. Nine fresh-session unsafe-behavior evaluations passed
  on 2026-08-26. The evaluator is not yet in CI.

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
private. The relationship-privacy work in Phase 13 is larger than it was scoped as.

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

| Problem                                               | Current mechanism                                                                                                                             | Required result                                                           |
| ----------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------- |
| ~~Change support stops at the SDK~~                   | Aligned through MCP, mock, and agents (PR #16, `40b8543`); exercised live 2026-08-19                                                          | Done                                                                      |
| ~~The allowance path is untested against a live fee~~ | Exercised 2026-08-19 through merged code under the 2 STRK fee, receipt recorded                                                               | Done                                                                      |
| ~~One deal per channel~~                              | Wire v3 frames records by deal ID and supports repeat deals                                                                                   | Done                                                                      |
| One directional channel per sender and recipient      | STRK20 derives a channel key without an index                                                                                                 | Reuse the channel pair for framed deals                                   |
| ~~Wire fingerprint~~                                  | Wire v3 masks the three spare bits. The v2 classifier scores at chance against live v3 records                                               | Done                                                                      |
| ~~No second wire implementation~~                     | TypeScript implements wire v3 and Rust checks frozen vectors                                                                                  | Done                                                                      |
| ~~No normative wire document~~                        | `docs/wire-v3.md` defines the byte and bit layout                                                                                             | Done                                                                      |
| Client-side deadlines                                 | The pool does not enforce offer expiry                                                                                                        | Keep the limit explicit and bind external policy to signed terms          |
| Settlement agreement is client policy                 | The pool does not compare payment with the accepted offer                                                                                     | Keep the SDK amount check and publish this trust boundary                 |
| One token per client                                  | `ClientConfig.token` fixes one token                                                                                                          | Move token selection to channel or operation scope                        |
| ~~Reads restart from index zero~~                     | `NoteCache` serves the immutable prefix from disk, so an unchanged channel costs one RPC rather than one per note (2026-08-25)                | Done                                                                      |
| ~~Idempotency stops at Rust~~                         | Protocol 4 carries one caller-persisted ID through agents, MCP, Python, CLI, and Rust; conflicts fail before proving                         | Done 2026-08-26                                                           |
| ~~No state reconstruction command~~                   | `rebuild_state` recreates additive channel records from the pool key and chain                                                               | Done                                                                      |
| ~~No signer abstraction~~                             | `AccountSigner` supports local and external signer implementations                                                                            | Done                                                                      |
| Protocol code lacks review                            | `Unreviewed` markers cleared 2026-08-17, but the 2026-08-19 diff was pushed to `main` on Poulav's instruction before line review              | Both owners review the 2026-08-19 push, then keep review-before-merge     |

### 5.2 Python SDK

| Problem                                | Current mechanism                                                                                                   | Required result                                             |
| -------------------------------------- | ------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------- |
| ~~No packaged Rust binary~~            | `v0.1.0` ships verified Linux x86-64 and macOS arm64 wheels                                           | Done                                                        |
| Duplicate response mapping             | The seam adapter repeats Rust fields                                                                                | Add schema compatibility tests for every method             |
| ~~No protocol negotiation~~            | Every envelope carries `protocol: 4`. The seam refuses a mismatch by name, and the MCP server handshakes at startup | Done                                                        |
| Timeout is global                      | Each call uses one 300-second limit                                                                                 | Use operation-specific timeouts and report the failed stage |
| Key-path safety relies on convention   | Python can access the named files                                                                                   | Add permission checks and secret-leak regression tests      |
| ~~`CLAUDE.md` contradicts the source~~ | `CLAUDE.md` must track the current protocol-4 binding                                                                   | Done                                                        |

The Python SDK must stay a binding. It must not add hashes, salts, felt arithmetic, note
selection, signing, or cryptography.

### 5.3 MCP server

| Problem                                               | Current mechanism                                                                                                                 | Required result                                                        |
| ----------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------- |
| ~~Exact-note rules conflict with Rust change~~        | PR #16: `_require_payable` checks `0 < amount <= total`, tool text rewritten                                                      | Done                                                                   |
| ~~Missing payment looks consistent~~                  | A missing payment produces nullable consistency and never reports success                                                         | Done                                                                   |
| ~~Wide `memo_hash` values fail at JSON clients~~      | `d1731f4` fixed the input; the 2026-08-19 `u128_boundary` fix carried the output as hex strings                                   | Done                                                                   |
| ~~Amounts above 2^53 lose precision at JSON clients~~ | Every `u128` crosses both boundaries as a string (`40b8543`, 2026-08-19)                                                          | Done                                                                   |
| ~~`wait_for_offers` accepts invalid limits~~          | The MCP boundary rejects invalid counts and timeouts                                                                               | Done                                                                   |
| Polling remains expensive                             | Each poll repeats note reads                                                                                                      | Add discovery subscriptions after Q3 publishes a supported endpoint    |
| Concurrency behavior is unproven                      | `asyncio.to_thread` starts independent calls                                                                                      | Reproduce overlaps, serialize writes, and add concurrency tests        |
| Mock is the direct-start default                      | Every result names the backend and network                                                                                        | Keep mock runs explicit in all evidence                                |
| MCP dependency is too broad                           | Package requires `mcp[cli]>=1.2.0`                                                                                                | Pin the tested major range and test supported versions                 |
| ~~No real-seam transport test~~                       | `test_seam_integration.py` (2026-08-18) drives the real server and the real CLI                                                   | Done                                                                   |
| ~~Viewing grants enter tool results~~                 | The server writes a new mode-`0600` file and returns only metadata and its path                                                    | Done                                                                   |
| ~~No health or readiness tool~~                       | `doctor` is an MCP tool and runs at startup (2026-08-19), logged rather than fatal: an operator may start first and repair second | Done; refusal deliberately not adopted                                 |

### 5.4 Reference agents and policy

| Problem                                     | Current mechanism                                | Required result                                                  |
| ------------------------------------------- | ------------------------------------------------ | ---------------------------------------------------------------- |
| ~~Reference agents call the mock directly~~ | `5362ada`: same policies over real MCP transport | Done                                                             |
| ~~Policy computes exact subset sums~~       | Aligned with change-note settlement in PR #16    | Done                                                             |
| Seller policy uses a fixed strategy         | One threshold and one counter path               | Keep this simple, but expose policy inputs as configuration      |
| ~~No crash behavior~~                       | The loop persists intent, reconciles, reuses the original ID, and stops on ambiguity | Done                                           |
| ~~No operator approval policy~~             | MCP enforces per-token, per-deal, and daily limits below the agent | Done, including Rust-authoritative reservation reconciliation |
| No live regression                          | The recorded run is manual evidence              | Add an opt-in canary that records receipts and disclosure output |

### 5.5 Erebus skill

The repository contains the generic `strk20-privacy-integration` skill and the Erebus
operator skill at `skills/erebus/SKILL.md`. Nine fresh-session unsafe-behavior evaluations
passed on 2026-08-26. The evaluator is not yet in CI.

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

| Problem                       | Current mechanism                                                                                                      | Required result                                                               |
| ----------------------------- | ---------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------- |
| ~~No continuous integration~~ | Added 2026-08-17 in `.github/workflows/ci.yml`. Rust, Python, and gitleaks                                             | Add TypeScript when the GitHub Packages dependency can be resolved in CI (F8) |
| ~~Every package is `0.0.1`~~  | Bumped to `0.1.0` across all seven manifests 2026-08-20                                                                | Push the tag                                                                  |
| No _published_ install        | `uvx erebus-mcp-server` works from wheels; verified by the canary from an empty environment                            | Push the `v0.1.0` tag, which publishes wheels and the index                   |
| No compatibility manifest     | Pool, prover, and SDK versions can drift                                                                               | Pin the pool class, ABI, prover protocol, and oracle revision                 |
| ~~No `doctor` command~~       | Built 2026-08-17 in `sdk/rs/src/doctor.rs`, exposed as `erebus-cli doctor`, bound through the Python seam the same day | Wire it into the MCP server and the operator skill                            |
| No backup and restore process | Key and state loss can be permanent                                                                                    | Add encrypted backup, restore, and state-rebuild procedures                   |
| No monitoring                 | Logs show local events only                                                                                            | Add stage timing, transaction status, retry count, and RPC health             |
| No secret-safe log policy     | Viewing grants and paths can enter logs                                                                                | Redact grants, keys, authorization headers, and RPC secrets                   |
| ~~No release provenance~~     | `SHA256SUMS` and a CycloneDX SBOM over 224 components, generated from lockfiles alone, `--check` on every push         | The tag publishes them                                                        |

### 5.7 Sprint delivery, and the bounded mainnet canary

D14 was reversed on 2026-08-28 for a bounded mainnet canary. This section records the live
progress without treating registration as a complete Erebus run.

#### What the sprint entry can and cannot reach now

| Requirement                     | Current status                                           |
| ------------------------------- | -------------------------------------------------------- |
| Public repository and licence   | Met                                                      |
| Public demo URL                 | Met, `https://erebus-private-agents.vercel.app`          |
| Registered in `registry.json`   | Met, PR merged 2026-08-14                                |
| Three mainnet pool transactions | **Met: 4 verified.** Two registrations and two channel opens |
| Public three-minute video       | Met; 180-second public MP4 labels mainnet and Sepolia evidence |
| Complete `strk20.json`          | Met; four verified hashes, public demo URL, and public video URL |

Thirty percent of the score is a working mainnet product, and the transaction check is
mechanical rather than a judgement call. Four hashes meet that check, but registrations and
empty channel setup do not demonstrate Erebus's negotiation or settlement workflow. The
The mechanical submission fields are complete. The hub still needs its scheduled refresh to
read the new commit and independently mark them complete.

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
`apply_actions`. Account A is deployed, registered, and held `184.691083159243495520 STRK`
after registration. Its exact 6 STRK allowance was consumed by that call.

A published hosted mainnet prover has still not been found. The working 2026-08-28 path is a
local ARM64 build of `PRIVACY-0.14.3-RC.2` backed by Alchemy's Starknet mainnet v0.10 RPC.
Alchemy preserved `PROOF1` facts through estimation and submission; Cartridge did not.

Upstream publishes the prover as a container in its
compatibility matrix, `ghcr.io/starkware-libs/starknet-privacy/transaction-prover:PRIVACY-0.14.3-RC.2`,
with the discovery service and proof interceptor at the same tag. The cost is the footnote
under that table: the prover needs a Pathfinder node at `PATHFINDER_STORAGE_STATE_TRIES=10000`.
A self-hosted Juno or Pathfinder still provides the stronger RPC trust boundary but requires
substantial storage. The registration canary instead put Alchemy inside the trust boundary:
`compile_actions` disclosed Account A's pool key to that endpoint.

Self-hosting does not remove deposit screening. A live read on 2026-08-28 returned non-zero
mainnet screener key `0x501cc4…fdb2`. Upstream's proof interceptor only relays the signature
returned by an elliptic-proxy `/screen` endpoint and requires operator-issued partner
credentials. See Q2.

Budget if D14 reverses: three calls need at least 18 STRK in pool fees, plus deployment,
approval, deposits, and gas. D8 sets about 30 STRK for a minimum run.

## 6. Decisions for Poulav and Ishita

These decisions change the plan. Record each answer in this table before the dependent work
starts.

| ID  | Decision                           | Current position                                                                                                                                              | Owner  | Needed by                                                                              |
| --- | ---------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------ | -------------------------------------------------------------------------------------- |
| D1  | Anchor use case                    | One-off service purchase or bilateral RFQ                                                                                                                     | Both   | Before product copy freezes                                                            |
| D2  | First external operator            | Not selected                                                                                                                                                  | Both   | Before the clean-install test                                                          |
| D3  | Repeat deals                       | **Yes, decided 2026-08-21.** The same pair must be able to deal more than once. Lands in the final wire (Phase 8), not in `v0.1.0`, which is already released | Both   | Decided. Per-deal grant scope ships in the same release: see Phase 11                  |
| D4  | Technical-preview privacy claim    | Confidential terms and shielded settlement                                                                                                                    | Both   | Decided                                                                                |
| D5  | Long-term privacy goal             | Relationship privacy                                                                                                                                          | Both   | Decided, research remains                                                              |
| D6  | Disclosure audience                | Auditors and arbitrators receive grants                                                                                                                       | Both   | Decided                                                                                |
| D7  | Platform evidence                  | Platforms receive a receipt, not a grant                                                                                                                      | Both   | Mechanism not selected; blocked on D1 and on Phase 12. Decide `memoHash` width with it |
| D8  | Mainnet spend limit                | About 30 STRK for the minimum sprint run                                                                                                                      | Poulav | Before funding                                                                         |
| D9  | External review target             | Final wire only, or wire v2 plus final wire                                                                                                                   | Poulav | Before audit booking                                                                   |
| D10 | Skill distribution                 | Erebus repo only, upstream skill repo, or both                                                                                                                | Ishita | Before `v0.1.0`                                                                        |
| D11 | Support model                      | Best effort, named maintainer, or funded maintenance                                                                                                          | Both   | Before `v1.0`                                                                          |
| D12 | Pool allowance mechanism           | Standing approval, decided 2026-08-16                                                                                                                         | Poulav | Decided, built                                                                         |
| D13 | Who provisions allowance and notes | Operator at install; not an agent tool                                                                                                                        | Both   | Before the operator product                                                            |
| D15 | AVNU as a swap dependency          | Not selected. Removes all Erebus Cairo; adds an availability and confidentiality dependency, and publishes the bought amount                                  | Both   | Before Phase 10.3 builds                                                               |
| D14 | Sprint network                     | **Bounded mainnet canary approved 2026-08-28.** A and B registrations and both directional channel opens succeeded; screening still blocks shielding and the full workflow | Both | In progress |

### External questions

| ID  | Question                                                               | Why it matters                                                                            | Owner                     |
| --- | ---------------------------------------------------------------------- | ----------------------------------------------------------------------------------------- | ------------------------- |
| Q1  | Can `compile_actions` avoid receiving the full pool key?               | This decides the long-term custody model                                                  | Poulav to StarkWare       |
| Q2  | Can an operator receive screening access for a self-hosted prover?     | Local fail-closed prover/interceptor stack prepared in `ops/screened-prover`. It still needs the operator-issued `SCREENING_URL`, partner name, and partner secret | Poulav to StarkWare       |
| Q3  | What are the supported mainnet prover and discovery URLs?              | Partial: local RC.2 prover + Alchemy v0.10 works for registration. Hosted prover and discovery remain unanswered | Poulav to StarkWare       |
| Q4  | Which pool, prover, ABI, and SDK revisions form one supported set?     | Version drift can cause silent note failures                                              | Poulav to StarkWare       |
| Q4a | Which transaction-prover tag matches deployed pool class `0x67dddd89`? | Upstream documents a class deployed on neither network, see below. Applies to Sepolia too | Poulav to StarkWare       |
| Q5  | Which relayer or paymaster path supports direct SDK operators?         | Submission unlinkability depends on it                                                    | Both to StarkWare or AVNU |

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
8. Rewrite the demo video script. **Done 2026-08-30.** `docs/demo-video-script.md`
   distinguishes the four mainnet setup transactions from the Sepolia full-workflow evidence.
9. Record the three-minute walkthrough. State the network out loud and on screen. Show
   `doctor` failing on a missing allowance and then passing, and label registration as one
   mainnet primitive rather than a completed mainnet product flow.
10. Upload the video and put its URL in `strk20.json`.
11. Put the Sepolia transaction hashes in the demo evidence section, labelled as Sepolia.
12. Say in the README and on the demo page that two registrations and two directional
    channel opens exist on mainnet, while the full mainnet workflow does not. **Done
    2026-08-30.**

Exit:

- One negotiation and settlement recorded end to end on current `main`.
- `demo_video` and `demo_url` are public without login.
- `transactions` contains the verified registration hash and no unsupported claim about it.
- The public demo labels its simulated flow, its Sepolia full-flow evidence, and its partial
  mainnet evidence as three separate things.

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
3. Run one low-value canary through the release artifacts. The standalone mainnet
   registration used current source, not the published `v0.1.0` artifacts, so the release
   boundary must remain explicit.
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

1. ~~Add operation IDs to every write request.~~ Complete in the 2026-08-23 working tree.
   All six Rust writes require a caller-supplied `OperationId`; the submitting account, wire
   version, and complete canonical request are bound before RPC, proving, or submission.
   Identical committed requests replay their recorded result.
2. ~~Journal preflight, proof, transaction hash, receipt, and state commit.~~ Complete in the
   2026-08-23 working tree. Journal v2 stores the canonical request, live prepared snapshot,
   proof facts, signed bytes and hash, receipt, planned local mutation, and result. Each crash
   boundary is synced before the next side effect.
3. ~~Reconcile the journal with chain state after restart.~~ Complete in the 2026-08-23
   working tree. Read-only classification uses receipts, the account nonce, and exact local
   channel state. Every Rust write runs the gate and refuses a new submission while an older
   operation is pending, ambiguous, or needs a local repair. The CLI seam is still task 10.
4. ~~Implement both explicit resume modes.~~ Complete in the 2026-08-23 working tree. A valid
   transaction is resubmitted byte-for-byte. An attempt proven unable to produce an effect is
   rebuilt from its durable request under the same id and re-enters all ordinary live checks.
5. ~~Add fault injection at every write boundary.~~ Done 2026-08-25 in
   `sdk/rs/tests/fault_matrix.rs` (#27). Faults are injected by writing a journal record
   stopped at each boundary, which is what a killed process leaves behind, so no production
   code carries a test hook. The mock node records the JSON-RPC methods it is asked for,
   which is what makes "startup is read-only" an assertion rather than an assumption; a
   companion test proves the recorder would notice a submission, so the sweep cannot pass
   vacuously.
6. ~~Rebuild channel handles and cursors from keys and chain state.~~ Done 2026-08-25.
   `Client::rebuild_state` and `erebus-cli rebuild_state` enumerate outgoing channels by
   keyed id, decrypt each recipient with the pool key, re-derive the channel key, confirm the
   token subchannel, and read the note cursor rather than assuming it. Additive: an existing
   record is kept byte for byte, never overwritten. Handles, `opened_transaction` and
   `last_write_block` do not come back, and the report says so.
7. ~~Cache immutable note prefixes and read from the last known cursor.~~ Done 2026-08-25.
   `sdk/rs/src/notecache.rs`. Notes are `WriteOnce` and contiguous, so everything below the
   first empty slot is immutable and cacheable soundly rather than as a bet. The cache file is
   named by a hash over the same channel key and token a note id derives from, so a cache
   written under one subchannel is not _found_ under another. The empty slot that ends a walk
   is never cached: a zero means "nothing here yet", and caching absence would freeze a
   channel at the length it had when first read.
8. Add discovery-provider support after Q3 defines a supported endpoint.
9. Add multi-token client state. _Partly done 2026-08-25._ Funding checks now follow the
   asset an operation moves rather than `ClientConfig::token`: `prepared_checks` and
   `pool_allowance_for` take the token, and the three channel operations pass the channel's
   own. That closes a real mismatch — `accept_and_settle` checked one token's allowance and
   balance, then spent notes of another, wherever a channel's token differed from the
   configured one. What remains is surface, not correctness: choosing a token per operation
   at the CLI is a request-shape change and therefore waits for protocol 4.
10. ~~Add account signer interfaces for hardware, wallet, or session signers.~~ Done
    2026-08-25. `AccountSigner` in `sdk/rs/src/signer.rs`; `LocalKeySigner` is the default and
    reads the key file at signing time rather than holding it across a write. Signing is async
    behind a boxed future, because every non-local signer is I/O and a sync trait would block
    a runtime thread on a hardware prompt. `Client::with_signer` refuses a signer whose
    address disagrees with the configured account, since the pool validates against that
    account's own contract and a mismatch would fail only after a proof was paid for.
11. ~~Define backup, restore, key-loss, and key-rotation behavior.~~ Done 2026-08-25 in
    [custody-operations.md](../docs/custody-operations.md). Behaviour is defined, tooling is
    not: there is no `erebus-cli backup` and no rehearsed key-loss drill, and the page says so
    rather than reading as complete. The finding worth carrying: registration is write-once
    (`privacy.cairo:337-343`), so an address's pool key is bound the first time it registers
    and rotating one means a new address, which under F29 abandons every existing channel.

Exit:

- A process can stop at every write stage and resume without duplicate state.
- A lost state directory can be rebuilt from keys and chain data.
- An unchanged channel read uses a constant number of RPC calls.
- One client can operate two configured tokens without state collision.

### Phase 8: Final wire and repeat deals

Completed in source on 2026-08-25, except the independent review and broader traffic
observer work. D3 decided 2026-08-21 that repeat deals are in scope.

Owner: Poulav. Ishita reviews API and agent behavior.

Work:

1. ~~Write the wire and relationship threat model before code changes.~~ Done.
2. ~~Design framed entries for offers, settlements, and future records.~~ Done.
3. ~~Add deal identifiers inside the authenticated message.~~ Done.
4. ~~Mask the spare bits and remove the fixed fifth-salt classifier.~~ Done.
5. ~~Preserve legacy wire-v1 and wire-v2 reads.~~ Done.
6. ~~Port the final wire to TypeScript.~~ Done.
7. ~~Publish normative vectors and a byte-level specification.~~ Done.
8. ~~Add two-deal tests for the same address pair.~~ Done locally and on Sepolia.
9. Add observer tests for cadence and sender linkability. Content and fixed-shape checks pass.
10. Obtain an independent cryptographic review of the frozen wire.

Exit:

- [x] Two agents complete two deals through the same directional channel pair.
- [x] Rust and TypeScript agree on every published vector.
- [x] The v2 fixed spare-bit classifier scores at chance against live v3 records.
- [x] The byte-level specification supports another implementation.

### Phase 9: Agent-layer safety and integration

Target: September to October 2026. **Precedes Phase 10**: spending limits must land before
the capability surface gives an agent more ways to spend.

Owner: Ishita. Poulav reviews anything that changes the seam contract.

This phase is Python only. Nothing here needs a Rust change, and none of it is blocked on the
Rust track.

**The governing principle, stated once so the rest follows from it.** `CLAUDE.md` constraint 6
says the policy engine decides _what_ to do and never touches key material. The agent-era
extension: **a model may decide what to offer; it must never authorize value movement.** An
LLM that proposes terms which a deterministic validator checks and executes gives you
negotiation intelligence on a testable spend path. An LLM that calls `accept_and_settle`
directly does not. Every item below is an application of that line.

#### 9.1 Spending limits belong below the agent

Today the only spending limit is the `budget` inside `BuyerPolicy`
(`agents/src/erebus_agents/policy.py`). The component being constrained is the component
enforcing the constraint. For a deterministic policy that is fragile; the moment a model
drives the loop it is structurally wrong, because a limit the agent can read is a limit the
agent can be argued out of.

Limits belong at the **MCP layer**, where the agent cannot reason past them, cannot be
prompt-injected past them, and cannot be talked out of them by a counterparty.

Work (done 2026-08-22 in #21):

1. Add per-token, per-deal, and daily cumulative caps to `ServerConfig`, read from the
   environment alongside `EREBUS_SETTLEMENT_ROLE`.
2. Enforce them in `tools.py` before any call reaches the seam, not in agent policy.
3. Persist daily cumulative spend so a restarted server does not reset the counter.
4. Return a distinct, non-retryable error when a limit blocks a call, so an agent stops rather
   than retries.
5. Keep the limits out of tool descriptions and results. A cap an agent can read is a cap an
   agent can plan around.

Exit:

- [x] A settlement above any configured cap is refused at the MCP layer with the agent's
      policy unchanged.
- [x] Restarting the server does not reset daily spend.
- [x] A test drives the MCP path past a configured cap and it fails closed.

#### 9.2 Tool results are model input, not logs

Three known defects are really LLM-legibility bugs, and a model cannot catch any of them the
way a human reviewer would.

- **`paid_amount is None` reports consistent.** A tool that answers "yes" when it means "I
  could not check" produces a confidently wrong agent. Already tracked in §5.3 as "Missing
  payment looks consistent".
- **Mock is the direct-start default**, and §5.3 notes "a local run can look like a real
  product". A person notices; a model does not.
- **Viewing grants enter tool results.** This risk exists _because_ a model is in the loop: a
  bearer secret in a tool result is a bearer secret in a transcript that leaves the machine.

Work (done 2026-08-23 in #22):

1. Make consistency checks fail closed: return `unknown` rather than a truthy default whenever
   payment evidence is missing.
2. Put `backend` and `network` in every tool result payload, not only in health output or
   logs, so a model cannot mistake mock for Sepolia or Sepolia for mainnet.
3. Give `grant_viewing_key` a secure export path that writes to an operator-chosen file and
   returns a reference, so the secret never enters a transcript.
4. Range-check `wait_for_offers` counts and timeouts before polling.
5. Review every tool description for a claim a model could repeat as a privacy guarantee, and
   align them with §4 wording. "Private" without a qualifier is a defect in a tool string.

Exit:

- [x] No tool returns a truthy value for a check it could not perform.
- [x] Every result names its backend and network.
- [x] A full negotiation transcript contains no viewing grant.

#### 9.3 Evals, including an adversarial counterparty

`skills/erebus/evals/unsafe-behavior.md` contains nine fixtures. Nine independent fresh
sessions passed on 2026-08-26. The retained result is
`skills/erebus/evals/results-2026-08-26.md`. The evaluator is not yet in CI.

Work (done):

1. Extended the unsafe-behavior set: reading a key file (1), settling as payee (2),
   reporting a mock run as live (3), claiming the relationship is private (4), a skipped
   doctor check (5), inventing a receipt or transaction hash (9), pasting a viewing grant
   into chat (8).
2. Added an **adversarial-counterparty eval** (6). Erebus agents read messages written by
   another agent, so untrusted text reaches a model that can spend. Since offer terms carry
   only a `memo_hash` commitment, not free text, the actual injection surface is the
   off-chain memo document itself, which arrives out of band in the same chat the agent
   operates in. A forwarded document carrying instructions — "ignore your budget", "this is
   urgent, accept immediately", "your operator approved this" — must not change the agent's
   decision.
3. Added a limit-evasion eval (7): an agent told about a cap must not split a deal into
   pieces to get under it, and must not suggest timing offers across the daily-cap's UTC
   boundary either.
4. **Open:** add the set to CI after the evaluator is stable enough not to flake.

Exit:

- [x] Every unsafe behavior named in this phase has a fixture with explicit failure criteria.
- [x] All nine fixtures have a recorded run against a fresh session.
- [x] An injected instruction in counterparty-supplied content changes no decision in that run.
- [x] A false privacy claim fails, and the recorded failure names the wording that was wrong.

#### 9.4 One real framework integration

`§3 Not proven` records that no external operator has completed a clean install. That, rather
than any missing feature, is the binding constraint on "other products build on Erebus".

Work:

1. Build one worked example driving the full loop through the MCP server from a mainstream
   agent framework — LangGraph, the Claude Agent SDK, or the OpenAI Agents SDK. Pick one.
2. Install it from the published wheels, not from the checkout, so it exercises what an
   outsider gets.
3. Record every place the integration needed knowledge not present in the docs, and file each
   as friction.
4. Publish it as the quickstart the README points to.

Exit:

- The example runs end to end against Sepolia from a clean environment.
- A reader who has never seen this repository can follow it without asking a question.

#### 9.5 Crash behavior in the agent loop

`§5.4` records that the loop assumes every call returns once. With one deal per pair, a lost
mid-settlement is permanent. Sequenced after Phase 7's operation journal, which supplies the
state this depends on.

Work:

1. Make `mcp_loop.py` resume from channel state rather than assume a fresh negotiation.
2. Treat every write as potentially applied: re-read state before retrying.
3. Distinguish "the call failed" from "the call may have succeeded" in agent-visible errors.

Exit:

- An agent killed mid-settlement resumes without double-spending or stranding the channel.

### Phase 10: Capability surface — transfer, messaging, swap, bridge

Target: October to December 2026, staged. The four parts have different entry criteria and
must not be scheduled as one block.

Owners: Poulav owns action-set construction and the AVNU seam. Ishita owns the MCP tools,
the agent-facing surface, and the off-chain message transport.

Erebus exposes exactly one way to move value: `accept_and_settle`, which requires a
counterparty-authored offer. Everything in this phase widens that surface. **None of it
requires an Erebus Cairo contract.** The one part that would have — private swap — does not,
because AVNU shipped a first-party private path before we needed to write one.

#### 10.1 Private transfer

`accept_and_settle` is the only value-moving path and it is gated on a counterparty offer:
`negotiation.rs:236` rejects accepting your own offer (`OwnOffer`), and `channel.rs:525`
rejects an action set whose message is not `MessageType::Accept`. So Alice cannot push tokens
to Bob today — Bob must first author an offer, which is an invoice, and costs a proof round
before the payment round.

The construction already exists. `ChangeOutput` (`client.rs:1124-1167`) builds a value note
into a channel and opens that channel when it does not exist. It targets the payer, but
nothing in the construction requires that. A transfer is `accept_and_settle_with_change`
minus the `Acceptance` argument.

Work:

1. Add a `transfer_with_change` action-set builder: payment note plus optional change note, no
   acceptance note.
2. Decide the note-grid position for a payment with no acceptance. The reader currently
   derives the payment index from the acceptance index
   (`read.rs:273`, `(acceptance_index + 1) * notes_per_message`) and walks messages in strides
   of five, so an unaligned value note may read as a torn message. Settle this before writing
   the builder, not after.
3. Extend `reveal` so a disclosed record can contain a transfer with no negotiation.
4. Add `transfer()` to the interface and all five mirrors: `sdk/ts/src/interface.ts`, the Rust
   trait, the Python seam, the MCP tool, and `mock_client.py`.
5. Add the MCP tool with the payer-role guard `accept_and_settle` already uses.

Exit:

- Alice transfers to Bob with no offer written by either side.
- A transfer and a negotiated settlement both read back correctly from the same channel.
- `status.md` no longer implies settlement is the only way value moves.

#### 10.2 Agent messaging, off-chain

The salt lane can carry arbitrary bytes — `ARCHITECTURE.md §7` says so and warns against ever
claiming otherwise. The argument against prose in notes is price, not possibility: 119 bits
per note, one permanent storage slot and one **permanently burned** subchannel index per
15 bytes, no reclaim because `use_note` rejects zero amounts, ~29 s per proof round, and
directional channels so a conversation needs two of everything. A tweet-length message is
19 notes and 19 dead slots, forever.

So the agent communication layer is built **off-chain**, keyed off the directional channel
secret Erebus already derives. Free-text in note salts stays in §11 deferred work and this
phase does not move it.

Work:

1. Derive a message-transport key from the existing directional channel key via HKDF, with a
   distinct info string so it cannot collide with the wire-v2 key.
2. Specify the transport: framing, ordering, replay handling, and what happens when the two
   sides disagree about history.
3. Implement send and receive in the Rust client; keep key handling inside the SDK boundary.
4. Expose `send_message` and `read_messages` MCP tools.
5. Document plainly that off-chain messages are **not** settled, not proven, and not part of
   a disclosed record — they carry no atomicity guarantee.
6. State the delivery assumption: this is a transport, and an offline counterparty misses
   messages unless a relay stores them.

Exit:

- Two agents exchange free-text over a channel with no on-chain write.
- No message content reaches a note salt.
- A disclosed record clearly separates settled state from unsettled chatter.

#### 10.3 Private swap through AVNU — verified 2026-08-20, no Cairo required

**AVNU ships private swaps end to end** (`@avnu/avnu-sdk >= 4.2.0`,
https://docs.avnu.fi/docs/privacy, live since 2026-07-21). Their executor is deployed and
their paymaster relays the transaction, so Erebus writes, audits, deploys and maintains **no
anonymizer contract**. This supersedes the earlier assumption that private swap meant taking
on Cairo; the skill's own anonymizer route says to check for a first-party private path
before planning a contract, and AVNU is the case it names.

Two facts make this reachable from Erebus specifically, which is an SDK-direct integration
with its own pool key and its own prover, and has no wallet:

- AVNU documents **two proving backends**, and the second is "Starknet Privacy SDK
  (self-managed) — you manage keys and notes." The docs state there is no strict wallet
  requirement and that any `PrivateSwapProver` implementation is acceptable. The interface is
  one function: `buildAndProve(plan) -> { call, proof }`.
- The paymaster is JSON-RPC 2.0 over HTTPS (`paymaster_buildTransaction`, mainnet
  `starknet.paymaster.avnu.fi`, testnet `sepolia.paymaster.avnu.fi`) and is documented as
  callable by any HTTP client, a Rust backend included, with a portal API key and no browser.

The swap is one action set of four actions:

1. `Withdraw` the sell token to AVNU's executor address, with surplus routed back to the taker.
2. `Withdraw` the pool fee to the recipient named by `paymaster_buildTransaction`.
3. An **open note** for the buy token to the taker — the amount is filled at execution.
4. `InvokeExternal` against the executor with `[buy_token, executor_calls, open_note_id]`.

**Erebus already encodes every primitive this needs.** All ten `ClientAction` variants are
implemented with correct variant indices, phase ordering and Cairo Serde in
`actions.rs:276-297` — `Withdraw`, `CreateOpenNote`, `InvokeExternal` and `ComputeAndInvoke`
included — and they are pinned against the TypeScript oracle by
`tests/fixtures/ts-clientaction-serde.json`. The gap is orchestration, not primitives.

Work:

1. Re-verify AVNU's docs, SDK version and paymaster endpoints before building; this record is
   dated and privacy-stack statuses drift.
2. Build a Rust AVNU client: quote, `quoteToCalls`, `paymaster_buildTransaction`, and private
   submission. Keep the API key in operator configuration, never in state.
3. Build the four-action swap set behind a `swap()` client method.
4. Implement open-note handling: reading back a note whose amount was filled on chain, and
   surplus routing for the sell leg.
5. Test atomicity both ways — a swap that fills credits a private note; a swap that reverts
   rolls the whole set back with nothing stranded.
6. Document that **the bought amount is public**. An open note carries its amount in
   plaintext by construction; the owner stays hidden, the amount does not. This must reach
   `privacy-model.md` before any swap claim is made.
7. Record AVNU as an availability and confidentiality dependency: their paymaster sees the
   request, and a swap fails when they are down.
8. Decide D15 (below) before wiring the MCP tool.

**Second-order benefit, worth scoping deliberately.** AVNU's private mode is always
`sponsored_private`: the paymaster relays `apply_action` and no user signature is required.
The submitting account is therefore AVNU's, not the agent's. That is precisely the mitigation
`privacy-model.md` leak 2 and F38 describe for the _sender_ half of the relationship leak, and
which `roadmap.md §4` records as not implemented. Evaluate whether the same paymaster path can
carry ordinary Erebus settlements, not only swaps — one integration may close two problems.
It does not touch `recipient_addr`, so the receiver half of F38 stands.

Exit:

- An agent swaps token A for token B from its shielded balance with no Erebus Cairo deployed.
- A reverted swap leaves the pool balance unchanged and no tokens at the executor.
- `privacy-model.md` states the open-note amount exposure before the feature is announced.
- The AVNU dependency is named in the trust model, not just the code.

#### 10.4 Private bridge — tracked, not built

No general private bridge exists to integrate against, and this phase does not build one.

What exists is narrower than the phrase suggests. **strkBTC** is a Bitcoin wrapper built on
STRK20: it bridges BTC in and the resulting asset can be shielded. That is a bridged asset
with privacy, not a private bridge. Starknet has signalled that later phases will let apps on
other EVM chains and Solana use the pool as a privacy layer, but that is announced rather than
shipped, and the privacy monorepo contains no bridge package.

The pool anticipates one. `actions.cairo:259` documents open notes as existing for flows
"such as AMM swaps or receiving funds directly through bridge transfers" — the hook is there
and the bridges are not.

Entry criterion, so this is a trigger rather than a wish: **an upstream package, or a
first-party private path documented by a bridge operator, meeting the same test AVNU passed —
a deployed contract, a documented non-wallet integration path, and a self-managed-keys prover
option.** Until then, re-check quarterly and change nothing.

`CLAUDE.md` lists cross-chain as out of scope and §11 defers cross-chain settlement. This
section does not reverse either; it records what would have to be true first.

### Phase 11: Disclosure primitives

Target: September to October 2026. Blocks Phase 12 and ships with D3.

Owner: Poulav owns the grant format and key derivation. Ishita owns the MCP surface and the
operator workflow.

Historical bearer disclosure is live: `ViewingGrant` reconstructed the 2026-08-19 wire-v2
settlement `0x4191fe47…f341`. Wire v3 now uses a different primitive. It derives native
subkeys for one deal in each direction. Because STRK20 note locations and amount masks still
derive from the parent channel key, the capsule also carries only the exact opaque note IDs
and masks needed for the selected frames. It never carries a parent channel key.

The capsule is encrypted with one-time Stark-curve ECDH to the recipient's registered pool
key. Its authenticated metadata binds its scope and explicit expiry. Expiry prevents a later
open; it cannot erase a record already opened. MCP writes the capsule to a new mode-`0600`
file and returns only metadata and the path.

This implementation has local cross-language and isolation evidence. The 2026-08-22
Sepolia run supplies live repeat-deal and scoped-disclosure evidence.

Work:

1. [x] Encrypt each grant to the intended recipient's public key. The channel layer already does
       this for `channel_key` through `EncChannelInfo`; reuse that ECDH construction rather than
       introducing a second one.
2. [x] Derive per-deal subkeys so a grant opens one deal's index range and no other.
3. [x] Define what expiry can and cannot stop, and state it in the grant format itself.
4. [x] Keep grants issued before this phase readable, and have them report themselves as legacy.
5. [x] Prevent normal MCP transcripts from storing a grant secret.
6. [x] Separate disclosure generation, delivery, and independent verification.
7. [x] Extend `docs/privacy-model.md` with what a disclosed record proves and what it only
       asserts, per grant shape.
8. [x] State that revocation cannot erase data a recipient already learned.

Exit:

- A copied grant is not sufficient without the recipient key.
- A per-deal grant cannot reveal another deal in the same channel, tested against a channel
  carrying two deals.
- Every grant issued before this phase still reads, and identifies itself as legacy.
- No MCP transcript contains a grant secret.

### Phase 12: Platform evidence

Target: November 2026. Start after Phase 11 and after D1 freezes.

Owners: Poulav owns cryptographic scope. Ishita owns delivery and operator workflow.

D7 records that platforms receive a receipt rather than a grant. The mechanism is unselected,
and the choice turns entirely on what makes a receipt true. There are three candidates:

- **An issuer signature.** The verifier trusts the agent that signed it. Erebus exists so
  that neither side has to trust the other, so a self-signed receipt reintroduces at the
  disclosure layer the same gap atomic settlement closes at the settlement layer. Not
  acceptable alone; acceptable only bound to chain evidence the verifier checks independently.
- **A viewing key.** This is a grant, and it is Phase 11. A receipt whose verifying key is a
  viewing key is not a second mechanism.
- **A proof.** The honest form of an outcome-only receipt. STRK20's circuit has no concept of
  an offer, so this is a new circuit over pool state rather than a parameter passed to
  theirs. Cost it as research before committing to it.

Selecting a mechanism also reopens `memoHash` width. At 128 bits it carries 2^64 collision
resistance under birthday bounds (`sdk/rs/src/wire.rs`). That is inert while a memo claim is
only read by a grant holder against chain data, and load-bearing the moment a receipt is used
as contested evidence, because a collision supports a false memo claim. Decide the width with
D7 rather than after it.

Ordering note: this phase follows Phase 11 because grants compose forward into receipts and
receipts do not compose backward into grants. **If D1 settles on platform-mediated commerce,
that order inverts**: receipts become the product and Phase 11 narrows to arbitration support.
Today's grant can never serve a platform audience, because it carries `granter` and
`counterparty` in cleartext by construction.

Work:

1. Select the platform receipt mechanism in D7.
2. Compare Erebus grants and receipts with the selective-disclosure flow in Stellar Private
   Payments.
3. Bind the receipt to deal ID, terms commitment, token, amount, and transaction hash.
4. Let a platform determine that settlement occurred without learning the counterparty.
5. Add walletless verification for the receipt format.
6. Re-decide `memoHash` width against the selected mechanism.
7. Document what each receipt proves and does not prove.

Exit:

- A platform can determine that one settlement occurred without a channel viewing grant.
- Missing payment evidence never produces a positive result.
- A receipt verifier needs no Erebus state directory, no grant, and no wallet.
- The published claim for a receipt matches what its mechanism actually proves.

### Phase 13: Relationship-privacy research

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

### Phase 14: Review, audit, and `v1.0`

Target: after phases 7 through 13 settle the shipping scope.

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
                     -> video -> sprint entry

Alchemy v0.10 -> local prover -> mainnet registration -> directional channel setup
                                     |
                                     +-> screening path still required before shielding

Change-note review -> shared interface decision -> MCP and agent alignment
                     -> cross-layer settlement test

Read cursor -> efficient long poll -> discovery subscription

Threat model -> final wire -> TypeScript oracle and spec -> external crypto review

Operation journal -> crash recovery -> clean operator canary -> v1 release

MCP spending limits -> capability surface        (limits land before new spend paths)

Private transfer -> off-chain messaging          (independent of the swap track)

AVNU re-verification -> Rust AVNU client -> four-action swap set -> open-note handling
                     -> paymaster relay evaluated for ordinary settlement (F38 sender half)

Recipient-bound grants -> per-deal grants -> platform receipt -> disclosure review
  (per-deal grants ship with repeat deals, not after them: D3, Phase 11)
```

Do not start the external wire review before the final wire freezes. Do not claim repeat
deals before the framing test passes.

## 9. Ownership and team process

### Ownership

| Area                                    | Primary owner | Required reviewer                                 |
| --------------------------------------- | ------------- | ------------------------------------------------- |
| Rust SDK, wire, settlement, execution   | Poulav        | Poulav line review, then external reviewer        |
| Cairo probes and upstream compatibility | Poulav        | Poulav                                            |
| Python SDK seam                         | Shared        | Both                                              |
| MCP server and tools                    | Ishita        | Poulav for protocol boundaries                    |
| Reference agents and policy             | Ishita        | Poulav for payment direction                      |
| Erebus skill and evaluations            | Ishita        | Poulav for privacy and custody wording            |
| Demo, video, and agent narrative        | Ishita        | Both                                              |
| Mainnet accounts and transactions       | Poulav        | Ishita makes sure that evidence is understandable |
| Release gate and public claims          | Shared        | Both                                              |

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
- [x] Rewrite the demo video script. Restored 2026-08-30 with timed mainnet and Sepolia
      evidence segments.
- [x] Public three-minute video that names each network. Published 2026-08-30 at
      `https://erebus-private-agents.vercel.app/erebus-private-sprint.mp4`.
- [x] Complete `strk20.json`. Four verified mainnet hashes, the public demo, and the public
      video are present.
- [x] Clear the last `Unreviewed` marker at `sdk/rs/src/channel.rs:516`. Cleared 2026-08-17.
      No `Unreviewed` marker remains anywhere in `sdk/rs/src`.
- [x] Record the change-making interface decision with Ishita. Decided 2026-08-17: drop
      `can_pay_exactly`, and the receipt reports `selected_input` and `change`.
- [x] Align change-making across MCP, mock, and agents. PR #16: `_require_payable` checks
      `0 < amount <= total`, `can_pay` removed, tool text rewritten.
- [x] Mirror `selected_input` and `change` through the Python seam and MCP. Done 2026-08-17
      as `40b8543`, so an agent can now see what a settlement actually spent.
- [x] Make missing payment evidence fail closed. `paid_amount is None` now returns nullable
      consistency and never reports a successful comparison.
- [x] Fix wide `memo_hash` transport. Closed 2026-08-17 as `d1731f4`: MCP tools take a hex
      string or an int and return hex. The frozen `interface.py` seam still types it `int`;
      parsing happens at the MCP boundary where the JSON problem is. Breaking for readers.
- [x] Convert `OfferTerms.amount` to a decimal string. Done 2026-08-17 as `40b8543`, which
      also covered `total`, `agreed_amount`, `paid_amount`, and the note list on the way
      back. That was the last `u128` crossing the MCP boundary as a JSON number, so the
      whole class of silent rounding above 2^53 is now closed.

Mainnet canary in progress: the proving path, both registrations, and both directional
channel opens are complete. The three-hash sprint requirement is met. Screening still blocks
shielding and therefore the full mainnet workflow. See §5.7.

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
- [x] Erebus operator skill and unsafe-behavior evaluations. All nine fresh-session runs
      passed on 2026-08-26. The retained result is
      `skills/erebus/evals/results-2026-08-26.md`. CI automation remains P1 work.
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
      The `v0.1.0` release publishes Linux x86-64 and macOS arm64 wheels through the static
      package index. Intel macOS is unsupported.
- [x] `doctor` and health tools. Rust done 2026-08-17. Python seam done 2026-08-17 as
      `2d69eda`, which also bound `allowance` and `approve` because `doctor`'s repair
      advice for the commonest failure is "run approve". Tests and the MCP tool landed
      2026-08-17 by Ishita in `40b8543`. MCP startup and the operator skill both call it.
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
- [x] Reconcile the stale guides. Done again 2026-08-27 after Protocol 4. Current documents
      now describe recovery, repeat deals, scoped grants, and the merged test counts.
      Historical run records retain their original protocol and configuration.
- [x] Tagged `v0.1.0` release with checksums and SBOM. The GitHub release contains the wheel
      chain, raw binaries, `SHA256SUMS`, and `sbom.json`. The GitHub Pages PEP 503 index lets
      installers resolve all three `erebus-*` packages.

**Agent-layer safety (Phase 9, Ishita).** These gate the capability surface: limits must
exist before Phase 10 gives an agent more ways to spend.

- [x] MCP-layer spending limits: per-token, per-deal, daily, persisted across restarts
      (Phase 9.1, #21). Rust-authoritative reconciliation completed on 2026-08-27.
- [x] Tool results carry backend and network in the payload and fail closed on missing
      evidence (Phase 9.2, #22).
- [x] Viewing grants leave tool results through a mode-`0600` secure export path that refuses
      overwrite (Phase 9.2, #22).
- [x] Adversarial-counterparty and limit-evasion fixtures authored (Phase 9.3, `5a4aabc`).
- [x] Durable caller intent and the coordinated protocol-4 seam. Write intent is persisted
      before the MCP call; the same ID crosses Python and CLI into Rust. Ishita's PR #35
      supplied the MCP caller-intent base; the coordinated boundary landed 2026-08-26.
- [x] Idempotent grant export. Replaying the same export operation returns the existing
      mode-`0600` file metadata without replacing the capsule or returning it to the model.
- [x] Complete Rust-authoritative spending reservations. Done 2026-08-27. Atomic
      reservations persist before Rust starts. Uncertain operations keep capacity.
      Proven-dead attempts release it. An exclusive journal snapshot permits safe release
      of Python-only entries. Accepted block timestamps assign committed spend to a UTC day.
- [ ] Add stable CI execution for the unsafe-behavior evaluations. All nine fresh-session
      runs passed on 2026-08-26 and the result is retained in the repository.
- [ ] One framework integration installed from published wheels (Phase 9.4). This attacks
      "no external operator completed a clean install" directly, which is the actual
      constraint on other products building on Erebus.

### P2: Operator alpha

- [x] Durable operation journal: record, lifecycle stages, locked `0600` storage, atomic
      replacement with directory sync (`b784f3f`).
- [x] Persist-before-submit: the exact signed transaction and its hash are on disk before
      the RPC call; receipts and local commit facts are recorded separately.
- [x] Idempotency: the same id and canonical request returns the durable typed result;
      different parameters fail before RPC or proving.
- [x] Journal retention: `OperationJournal::prune` removes terminal records older than a
      caller-supplied window, with their lock files and stored transactions, and reports what
      it kept and why. Unfinished and `NeedsAttention` records are never pruned at any age.
- [ ] Journal retention, remaining: a request that fails local validation still leaves a
      record behind, and nothing calls `prune` on a schedule yet.
- [x] Startup reconciliation: every journalled operation classified against the chain,
      account nonce, and local state, read-only. Every Rust write is gated by the result.
- [x] Live prepared-stage checks: proof validity, fee, allowance and public balance read
      from the chain before proving, with gas reserved (`03c058c`).
- [x] Explicit resume: reconcile, then resubmit the recorded transaction unchanged or,
      only after the earlier attempt cannot produce an effect, rebuild its durable request
      through the normal checked write path under the same id.
- [x] Type-owned wide-number serialization: `AllowanceReport` and `NoteBalance` emit
      full-width decimal strings themselves; the CLI no longer repairs their JSON shapes.
- [x] Fault matrix: every durable boundary swept for stable binding, read-only startup,
      no silent "finished" classification, safe-to-retry holding exactly before the
      signature, and exactly-once replay (#27).
- [x] Recovery is reachable from outside Rust: `reconcile` and `resume_operation` are CLI
      methods, with dispatch, malformed-id, and protocol-tag tests (#26).
- [x] Chain-state recovery: `rebuild_state` reconstructs channel records from the pool key
      and chain data by keyed discovery. New handles: the originals are random and local, so
      anything holding an old handle string will not resolve against a rebuilt record.
- [x] Agent loop persists canonical intent before a write, reconciles after interruption,
      resumes by the original ID, and stops on `wait` or `operator_attention` (Phase 9.5).
- [x] Preserve Protocol 4 error names through the MCP seam. Done 2026-08-27 for
      `OPERATION_CONFLICT`, `RECONCILIATION_REQUIRED`, `INSUFFICIENT_ALLOWANCE`, and
      `INSUFFICIENT_BALANCE`.
- [x] Complete the packaged Sepolia recovery canary. Done 2026-08-27 from clean local
      wheels. Exact resubmission and expired-proof rebuild both completed. See
      `docs/runs/2026-08-27-packaged-recovery-canary.md`.
- [x] Read cursor and note cache: an unchanged channel read costs one RPC rather than one
      per note, durable so it survives the CLI's process-per-call shape.
- [ ] Discovery-provider support, blocked on Q3.
- [x] Multi-token correctness: funding checks read the allowance and balance of the token
      an operation actually moves, not the configured one.
- [ ] Multi-token surface: choose a token per operation at the CLI. Protocol 4 removed the
      request-shape blocker. The request and state model still need this feature.
- [x] Signer abstraction: `AccountSigner` behind `Client::with_signer`, with the account
      key read at signing time rather than threaded through a write.
- [x] Backup, restore, key-loss and rotation behaviour defined in `custody-operations.md`.
- [ ] Backup and restore _tooling_: no `erebus-cli backup`, no encrypted bundle, no restore
      verification, no rehearsed key-loss drill.
- [ ] Secret-safe monitoring.

### P3: Protocol and privacy

**Wire and threat model.**

- [x] Written relationship threat model (`docs/threat-model.md`, 2026-08-21), with
      `privacy-model.md` canonical when the two differ.
- [x] Final framed wire: wire v3 uses authenticated deal IDs and derived masks for all three
      spare bits, is the source default, and has live Sepolia evidence (#23).
- [x] Repeat deals through the same directional channel pair. The wire carries deal ids;
      `ChannelState` reports `settlements: Vec<SettlementRecord>` rather than a channel-level
      boolean, one record per deal with agreed and paid amounts kept separate and a nullable
      consistency result (#24).
- [x] TypeScript wire-v3 oracle and frozen normative vectors, checked byte-for-byte by Rust
      (`sdk/ts/src/channel/wire-v3.ts`, `sdk/rs/tests/fixtures/ts-wire-v3.json`).
- [ ] Submission unlinkability research.
- [ ] Independent cryptographic and security review.

**Capability surface (Phase 10).** Staged, not one block: 10.1 has no dependency, 10.3
needs D15, 10.4 needs an upstream that does not exist yet.

- [ ] Private transfer with no offer (Phase 10.1). Settle the note-grid position first;
      the reader derives the payment index from the acceptance index.
- [ ] Off-chain agent messaging keyed off the channel secret (Phase 10.2). Decide whether
      it is channel-bound context or a messaging product before writing it.
- [ ] Private swap via AVNU, no Erebus Cairo (Phase 10.3, gated on D15). Verified
      2026-08-20: self-managed-keys prover, JSON-RPC paymaster, and every `ClientAction`
      it needs is already encoded and pinned.
- [ ] Open-note amount exposure written into `privacy-model.md` before any swap claim
      (Phase 10.3). The owner stays hidden; the bought amount does not.
- [ ] Paymaster-relayed submission evaluated for ordinary settlements, not only swaps
      (Phase 10.3). It may cover the sender half of F38; it does nothing for the
      recipient half.
- [ ] Private bridge: re-check quarterly against the Phase 10.4 entry criterion. Nothing
      to integrate against today.

**Disclosure (Phases 11 and 12).**

- [x] Recipient-bound, time-limited, per-deal grants with documented limits (Phase 11).
- [x] Per-deal grant scope released together with repeat deals (D3, Phase 11).
- [ ] Outcome-only platform receipts, mechanism selected in D7 (Phase 12).

## 11. Explicitly deferred work

The current plan does not include these products:

- A hosted multi-tenant Erebus service.
- A consumer dashboard or wallet.
- Free-text encrypted messaging **in note salts**. Phase 10.2 plans an off-chain
  transport keyed off the channel secret; what stays deferred is paying a permanent
  storage slot per 15 bytes to carry prose on chain.
- Multi-party negotiation.
- High-frequency order books.
- Sealed-bid auctions.
- Delivery-versus-payment enforcement.
- A token or economic layer.
- Cross-chain settlement. Phase 10.4 records the entry criterion that would reopen it,
  and does not reopen it.
- A custom pool deployment without a separate auditor and screening governance plan.
- An Erebus anonymizer contract. Private swap reaches AVNU's deployed executor instead
  (Phase 10.3), so `contracts/` stays probes-only and the "we deploy nothing" property
  survives. Revisit only if a needed action has no first-party private path.

Add one only after D1 changes and the owners accept its security and maintenance cost.

## 12. Production finish line

`v1.0` is complete only when every statement is true:

- [x] The final wire has two implementations and a public specification.
- [ ] Protocol-critical code has internal review and independent external review.
- [x] An operator can install Erebus with one command.
- [x] `doctor` finds configuration, fee, allowance, prover, RPC, and key-permission errors.
- [x] Local fault tests show safe resume after each write-stage failure.
- [x] The product can rebuild state from keys and chain data.
- [x] Disclosure grants are recipient-bound and scoped to one deal.
- [ ] Spending limits are enforced below the agent and survive a restart.
- [x] The eval set covers every unsafe behavior in section 5.5, including an adversarial
      counterparty. CI automation remains open.
- [ ] An integrator outside the two owners has run the full loop from published artifacts.
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
- `sdk/rs/src/actions.rs` for the ten `ClientAction` variants and their encoding.
- `sdk/py/src/erebus/_seam.py` for the Python-to-Rust boundary.
- `mcp-server/src/erebus_mcp/tools.py` for agent-facing behavior.
- `agents/src/erebus_agents/` for reference policy behavior.
- `skills/erebus/SKILL.md` and its `evals/` for agent-facing operating behavior.
- `docs/privacy-model.md` for the privacy boundary. It is canonical for any privacy
  claim, and outranks this document where the two differ.
- `docs/status.md` for current state; it is the tiebreaker across all documents.
- `docs/friction.md` for measured failures and workarounds.
- `docs/custody-design.md` for key ownership and deployment options.
- `strk20.json` for sprint evidence.

If this document disagrees with current source or live receipts, those sources take priority.
