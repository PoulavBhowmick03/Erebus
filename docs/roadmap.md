# Erebus product roadmap

Last source audit: 2026-08-14.

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

## 3. Current evidence

This section records what exists on 2026-08-16. Later sections describe the missing work.

### Committed and working

- Wire v2 uses AES-256-GCM-SIV and a 128-bit tag across five STRK20 note salts.
- A live Sepolia negotiation settled through two role-bound MCP servers.
- The settlement transaction is
  `0x14b38e9dbc65f0749be6da2fa05dd2713f8c4c893bac707961c73e616b34cb3`.
- `scripts/observer.py` recovers wire-v1 terms and does not recover wire-v2 content.
- The Rust client can open channels, write offers, read state, settle, shield, grant, and reveal.
- The Python seam uses protocol 2 and passes key-file paths to Rust.
- The MCP server exposes nine tools over stdio.
- Payer and payee roles prevent the payee from calling `accept_and_settle`.
- `wait_for_offers` reduces agent tool calls, but it still polls the chain.
- The public demo runs at `https://erebus-private-agents.vercel.app`.
- The repository is public and uses Apache-2.0.
- The current working tree passes 197 Rust tests, with 3 ignored tests.
- The current working tree passes 53 Python tests and 38 TypeScript tests.
- Clippy and rustdoc pass with warnings denied.

### Active but not shipped

The change-note implementation merged to `main` on 2026-08-15 as `8dd9b74`, via PR #14 from
branch `change_output_payback`.

- `sdk/rs/src/client.rs::select_notes` selects sufficient inputs.
- `sdk/rs/src/channel.rs::accept_and_settle_with_change` creates payer-owned change.
- `ChangeOutput::existing` writes to an open payer self-channel.
- `ChangeOutput::opening` opens the self-channel and token subchannel in the same action set.
- Settlement tests cover `5 STRK -> 3 STRK payment + 2 STRK change`, and both change branches.
- The `ErebusClient` trait signature is unchanged, so agent-side mocks need no update.
- The commit also carries the observer and friction-log changes, against Phase 0 step 1.

This is not yet a product feature. The MCP server, mock, reference agents, and guides still
require exact note sums, so the stack above the SDK now describes settlement behavior the SDK
no longer has. One `Unreviewed` marker remains, on `accept_and_settle_with_change` at
`sdk/rs/src/channel.rs:516`. `select_notes` and the client-side change block were reviewed
before the merge.

Two behavior changes ride along and are not only about change:

- `select_notes` previously failed when its subset search hit the state cap. It now falls
  through to a greedy largest-first selection, so a settlement that once errored can succeed
  with a larger input set.
- A settlement now emits a seventh note when the payer holds no exact subset, and six when it
  does. This is a new public observable. See §4.

### Not proven

- Erebus has no mainnet transaction.
- The public demo is a browser simulation. It does not use a wallet or submit a transaction.
- The sprint video does not exist.
- No external operator completed a clean install.
- No automated test starts the real MCP server, calls the Rust CLI, and reaches Starknet.
- No independent reviewer reviewed the wire or settlement code.
- No evidence supports full relationship privacy.
- No release artifact exists for the Rust CLI or Python packages.
- No Erebus-specific agent skill exists.

## 4. Privacy and trust boundary

Erebus uses the direct STRK20 SDK route because the operator controls the agent account and
pool key. The route matches the [STRK20 SDK model](https://strk20-by-example.org/sdk/getting-started).

### Private and public data

| Private from a public chain reader | Public or visible to infrastructure |
|---|---|
| Wire-v2 offer content | The submitting Starknet account |
| Private transfer amount and recipient | Pool interaction timing and frequency |
| Spent-note identity | Public shield and unshield amounts |
| Channel content without a grant | The fixed fifth-salt shape in wire v2 |
| Other relationships outside a scoped grant | Pool usage and proof-bearing transaction size |
| Change amount and change-note content | Whether a settlement created payer change |

The change note added on `change_output_payback` widens the last row. A settlement creates six
notes when the payer holds an exact subset and seven when it does not, so note count leaks one
bit about the payer's holdings on every deal. The amounts stay private. This compounds the
fifth-salt shape already listed above, because both let a reader classify and count Erebus
transactions without reading one. Record it against F31 rather than as a separate finding.

The prover and preflight RPC receive the pool key in `compile_actions` calldata. They can
derive the identity history. The submitted `apply_actions` transaction does not contain that
key.

The account signing key stays in the Rust process. The Python process passes only its file
path. The Python process does handle bearer viewing grants, so MCP transcripts belong inside
the disclosure trust boundary.

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
| Change support is uncommitted | Rust selects sufficient notes and builds payer change | Review, commit, and run the same behavior through MCP and agents |
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
| Protocol code lacks review | Several files still contain `Unreviewed` markers | Complete internal review before the external audit |

### 5.2 Python SDK

| Problem | Current mechanism | Required result |
|---|---|---|
| No packaged Rust binary | `Seam` expects `erebus-cli` on disk | Ship the binary in platform-specific wheels |
| Duplicate response mapping | The seam adapter repeats Rust fields | Add schema compatibility tests for every method |
| No protocol negotiation | The seam assumes protocol 2 | Refuse incompatible CLI versions at startup |
| Timeout is global | Each call uses one 300-second limit | Use operation-specific timeouts and report the failed stage |
| Key-path safety relies on convention | Python can access the named files | Add permission checks and secret-leak regression tests |

The Python SDK must stay a binding. It must not add hashes, salts, felt arithmetic, note
selection, signing, or cryptography.

### 5.3 MCP server

| Problem | Current mechanism | Required result |
|---|---|---|
| Exact-note rules conflict with Rust change | `_require_payable` uses exact subset sums | Agree on the interface and update all tool behavior together |
| Missing payment looks consistent | `paid_amount is None` returns `True` | Return an unknown result, or fail closed |
| Wide `memo_hash` values fail at JSON clients | The tool schema uses a JSON integer | Accept a canonical hex string and parse it in Rust |
| `wait_for_offers` accepts invalid limits | Counts and timeouts have no range checks | Reject zero and negative values before polling |
| Polling remains expensive | Each poll repeats note reads | Add discovery subscriptions after Q3 publishes a supported endpoint |
| Concurrency behavior is unproven | `asyncio.to_thread` starts independent calls | Reproduce overlaps, serialize writes, and add concurrency tests |
| Mock is the direct-start default | A local run can look like a real product | Show the backend in health output and require explicit production mode |
| MCP dependency is too broad | Package requires `mcp[cli]>=1.2.0` | Pin the tested major range and test supported versions |
| No real-seam transport test | MCP tests use a mock or `StubSeam` | Start `server.py`, call the real CLI, and inspect its result |
| Viewing grants enter tool results | MCP hosts and transcripts can retain the secret | Add a secure export path and clear operator warnings |
| No health or readiness tool | Startup checks only configuration fields | Add `doctor` and a read-only health tool |

### 5.4 Reference agents and policy

| Problem | Current mechanism | Required result |
|---|---|---|
| Reference agents call the mock directly | `run_negotiation` accepts `MockErebusClient` | Add an MCP-client reference flow |
| Policy computes exact subset sums | `_payable_amounts` mirrors the old settlement rule | Update policy after the change-note interface decision |
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
| No continuous integration | Developers run the gate locally | Add Rust, Python, TypeScript, docs, and secret-scan jobs |
| Every package is `0.0.0` | No release version exists | Define one version policy and release `v0.1.0` |
| No install command | Operators build Rust and source the workspace | Make `uvx erebus-mcp-server` install the matching CLI |
| No compatibility manifest | Pool, prover, and SDK versions can drift | Pin the pool class, ABI, prover protocol, and oracle revision |
| No `doctor` command | Errors appear during proofs | Make checks before any proof-bearing operation |
| No backup and restore process | Key and state loss can be permanent | Add encrypted backup, restore, and state-rebuild procedures |
| No monitoring | Logs show local events only | Add stage timing, transaction status, retry count, and RPC health |
| No secret-safe log policy | Viewing grants and paths can enter logs | Redact grants, keys, authorization headers, and RPC secrets |
| No release provenance | GitHub has source only | Publish checksums, build metadata, and a software bill of materials |

### 5.7 Mainnet and sprint delivery

The mainnet pool is
`0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a`.
The pool charged 6 STRK per `apply_actions` during the 2026-08-14 audit.

The configured mainnet account has no contract and no balance. The sprint guide still does
not publish the mainnet proving-service URL.

A 2026-08-16 search found no published mainnet proving service anywhere: not in the upstream
README, not in `strk20-by-example.org`, not in either Starknet launch post. The endpoints
Erebus holds are `alpha-sepolia` hosts and cannot serve mainnet. Q3 may therefore have no
answer, so the plan needs a second branch.

The second branch is self-hosting. Upstream publishes the prover as a container in its
compatibility matrix, `ghcr.io/starkware-libs/starknet-privacy/transaction-prover:PRIVACY-0.14.3-RC.2`,
with the discovery service and proof interceptor at the same tag. The cost is the footnote
under that table: the prover needs a Pathfinder node at `PATHFINDER_STORAGE_STATE_TRIES=10000`.
A mainnet Pathfinder sync takes days of wall clock and hundreds of gigabytes of disk, which is
longer than any other open item and cannot be compressed by adding effort. If Q3 comes back
empty, this sync is the critical path and must start the same day.

Self-hosting does not remove deposit screening. See Q2.

The SDK has no allowance path. There is no `approve`, no `allowance`, and no `fee_amount`
anywhere in `sdk/rs`. `apply_actions` is the only state-changing entrypoint the pool exposes,
and it calls `collect_fee` before applying anything, which is a `transfer_from` against the
caller. Sepolia charges zero, so this never surfaced. On mainnet, a missing allowance reverts
with a bare `Contract error`, the same shape as F20. `sdk/rs/src/execution.rs:192` submits a
single call, so Phase 1 step 6 is either a separate standing-approval transaction or a new
multicall path, not a configuration flag. That choice is unmade.

| Missing item | Blocker | Required result |
|---|---|---|
| Three transactions | No funded signer and no mainnet prover URL | Fund, deploy, submit, and inspect three successful pool calls |
| Demo video | No recording exists | Record the public demo and the verified mainnet receipts |
| Final manifest | Transaction and video fields are empty | Update `strk20.json` and wait for the hub refresh |
| Mainnet evidence on demo | The page shows a pending card | Add the three Voyager links after submission |

Three calls require at least 18 STRK in pool fees. Deployment, approval, deposits, and gas
need more funds. The current operating budget is about 30 STRK for the minimum run.

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

### External questions

| ID | Question | Why it matters | Owner |
|---|---|---|---|
| Q1 | Can `compile_actions` avoid receiving the full pool key? | This decides the long-term custody model | Poulav to StarkWare |
| Q2 | Can an operator receive screening access for a self-hosted prover? | Self-hosted proving does not make deposits work alone | Poulav to StarkWare |
| Q3 | What are the supported mainnet prover and discovery URLs? | The sprint and mainnet canary depend on them | Poulav to StarkWare |
| Q4 | Which pool, prover, ABI, and SDK revisions form one supported set? | Version drift can cause silent note failures | Poulav to StarkWare |
| Q4a | Which transaction-prover tag matches deployed pool class `0x67dddd89`? | Upstream documents a class deployed on neither network, see below | Poulav to StarkWare |
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

Target: 2026-08-14 to 2026-08-15.

Owner: Poulav. Ishita reviews the shared interface changes.

Work:

1. Split the change-note patch from observer and documentation edits.
2. Remove accidental prose corruption from `docs/friction.md`.
3. Review `select_notes` bounds, overflow behavior, fallback behavior, and input ordering.
4. Review change-channel setup, note indices, salts, phase ordering, and collision checks.
5. Record the shared interface decision for change-making.
6. Update the test baseline in one status document.
7. Create tracked issues for every accepted roadmap item.

Exit:

- Each protocol change has a small commit and focused tests.
- No unrelated prose and protocol change share one commit.
- Poulav removes each `Unreviewed` marker only after a line review.
- Ishita accepts the changed balance and settlement result shapes.

### Phase 1: Complete the sprint entry

Target: 2026-08-14 to 2026-08-17.

Owners: Poulav handles mainnet. Ishita handles the recording. Both review public claims.

Work:

1. Ask the sprint support channel for the mainnet proving-service URL and for Q4a. Send this
   on the day the phase starts, before any other step. The latency is external, and the
   answer may be that no hosted mainnet prover exists.
2. If no hosted prover exists, start the Pathfinder mainnet sync the same day and stand up the
   prover container against it. That sync is the longest lead time in the sprint. See §5.7.
3. Put an Alchemy key in local configuration, or use the documented public RPC.
4. Fund the configured mainnet address within the D8 limit.
5. Deploy the Starknet account.
6. Read the pool fee again before approval.
7. Choose the allowance mechanism, standing approval or per-settlement multicall, and build
   it. The SDK has none today. See §5.7.
8. Approve the pool for all deposits and pool fees in the planned calls.
9. Submit registration plus shield, then two small top-ups.
10. Make sure that all three receipts succeeded and include pool events.
11. Put the three hashes in `strk20.json`.
12. Add the hashes to the demo evidence section.
13. Record the three-minute walkthrough from `docs/demo-video-script.md`.
14. Upload the video and put its URL in `strk20.json`.
15. Make sure that the hub shows all four requirements after its refresh.

Exit:

- `transactions` contains three verified mainnet hashes.
- `demo_video` and `demo_url` are public without login.
- `contracts` remains empty unless Erebus deploys a contract.
- The public demo labels its simulated flow and live evidence separately.

### Phase 2: Align Rust, Python, MCP, and agents

Target: 2026-08-15 to 2026-08-21.

Owners: Poulav owns Rust. Ishita owns MCP and agents. Both own the seam contract.

Work:

1. Finish the change-note review from phase 0.
2. Replace exact-payability language with sufficient-balance language after both owners approve the interface.
3. Return selected input value and `change_required` where an agent needs that fact.
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

1. Add continuous integration for Rust, Python, TypeScript, docs, and secret scanning.
2. Build `erebus-cli` for macOS arm64, macOS x86-64, and Linux x86-64.
3. Package each binary with the matching Python wheel.
4. Make `uvx erebus-mcp-server` find the packaged binary.
5. Add `erebus doctor` before the first write.
6. Inspect key permissions, state permissions, RPC health, and prover compatibility.
7. Inspect chain ID, pool address, pool version, fee, and proof-validity window.
8. Inspect registration, allowance, STRK balance, private balance, and note maturity.
9. Report each failed inspection with one direct repair instruction.
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
3. Run one low-value mainnet canary through the release artifacts.
4. Make sure that the demo, README, roadmap, and manifest use the same evidence.
5. Publish install steps, current limits, recovery limits, and trust assumptions.
6. Tag `v0.1.0` and publish signed release artifacts.
7. Make sure that the sprint hub reads the final manifest.

Exit:

- An external operator can install and run the technical preview.
- Every public claim links to source, a test, or a transaction.
- The release notes state that the product is unaudited.
- The release notes state that relationship privacy is not complete.

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
Proving path -> mainnet access -> sprint transactions -> video and final manifest
  (proving path = hosted prover URL, or Pathfinder mainnet sync -> prover container)
Pool allowance -> mainnet access

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

### P0: Sprint and safety blockers

- [x] Public repository and Apache-2.0 license.
- [x] Public browser demo and GitHub Website field.
- [ ] Mainnet proving path: a hosted URL, or a self-hosted prover on a synced mainnet Pathfinder.
- [ ] STRK allowance to the pool, in code and on the funded account.
- [ ] Funded and deployed mainnet account.
- [ ] Three verified mainnet pool transactions.
- [ ] Public three-minute video.
- [ ] Complete `strk20.json` visible on the hub.
- [x] Merge the change-note work. Merged 2026-08-15 as `8dd9b74`.
- [ ] Clear the last `Unreviewed` marker at `sdk/rs/src/channel.rs:516`.
- [ ] Align change-making across Rust, Python, MCP, mock, and agents.
- [ ] Make missing payment evidence fail closed.
- [ ] Fix wide `memo_hash` transport.

### P1: Technical-preview release

- [ ] Real MCP-to-CLI integration test.
- [ ] MCP-client reference agents.
- [ ] Erebus operator skill and unsafe-behavior evaluations.
- [ ] Platform wheels containing `erebus-cli`.
- [ ] `doctor` and health tools.
- [ ] Continuous integration and secret scanning.
- [ ] Clean-machine install and canary.
- [ ] One current status document and reconciled guides.
- [ ] Tagged `v0.1.0` release with checksums and SBOM.

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
