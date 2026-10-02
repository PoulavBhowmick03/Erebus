# Metropolis M6 Progress

Updated: 2026-10-01. Branch: `metropolis`. M6 remains **under owner review**. The public-bound EVM path is
integrated end to end on a local chain, with a real RPC fault proxy, a packaged relayer, and an
operator runbook. A funded Anvil run now connects native shielded proof preparation to the shared
coordinator's nonce reservation, signed transaction, broadcast, restart, and finalized pool
reconciliation. Funded durable-boundary matrices passed in both settlement modes.
M7 and M8 remain incomplete.

## Legacy Compatibility Decision: 2026-10-01

The owner selected preservation of the existing legacy journal write-back behavior.
No runtime behavior, schema versions, or historical fixtures change for this decision.
The v1/v2 `accepted_at` timestamp ambiguity after `record_result` upgrades remains unresolved.
Characterization establishes compatibility, not safe timestamp conversion.
See [DM6-2](metropolis-m6-decisions.md#dm6-2-journal-generalize-the-starknet-journal).

## Integrated

- Shared journal (`sdk/journal`) with atomic writes, locks, blobs, pruning, and durable-boundary
  fault hooks. The Starknet journal keeps its record formats and remains covered by its
  characterization fixtures.
- Deal classifier (`erebus_core::deal_state`) covering consumption, winner identity, finality,
  and expiry. Proof-free ledger `commit`/`release` are private; `reconcile_deal` applies a whole
  assessment atomically.
- Local coordinator (`sdk/coordinator`): durable intent and reservations before any signer,
  one identity-wide snapshot, authorization fence, immutable signing plan, replacement history,
  and journaled broadcast attempts. It never releases capacity on a timeout or local expiry.
- EVM offline signing (`sdk/evm::chain`): explicit nonce, fee, and gas; restored bytes validated
  against the stored plan and the accepted agreement; replacement fee floors.
- The owner selected removal of the legacy backend submission APIs on 2026-10-01.
  `EvmSettlementBackend` no longer exposes `submit`, `settle`, `verify`, or
  `with_confirmations`. It prepares and estimates without retaining a signing wallet.
  Successful M3 payment tests now use coordinator-backed submission and paired finalized
  reconciliation. Adversarial tests send invalid calls directly and require an included
  revert; they do not count local validation failure as contract evidence.
- Signer nonce claims: one unresolved operation per chain and gas-paying account, shared across
  deployments, resumed without a new RPC read, and released only by a finalized consumed nonce.
- Canonical observation (`sdk/evm::chain::observation`): `consumedDeals`, the complete
  settlement-log history through a pinned head, winner calldata and authorization verification,
  explicit RPC `finalized`, and canonical anchor rechecks. Errors map to `DealEvidence::Unknown`
  and hold every reservation.
- Bounded historical observation: public-bound recovery saves log and parent-walk cursors
  in an atomic `ObservationJournal`. Only `Complete` returns deal evidence. Pending scans
  survive restart and retain reservations. Completed finalized prefixes avoid repeated
  genesis scans and old-winner walks. Nonfinal reorgs restart unfinished suffixes; changed
  finalized anchors reject recovery. Optional paired reads use separate checkpoints.
- Shielded observation saves at most 1,000 new verified pool blocks per call.
  `observe_shielded_deal_bounded` supports smaller batches. Incomplete scans return
  `RecoveryError::HistoryPending`, not evidence. Reorg recovery searches the shared prefix
  without one RPC request per orphaned block. The same durable public index supports restart.
- Coordinated end-to-end lifecycle (Anvil): nonce reservation, durable intent, authorizations,
  preparation, signing, journaled broadcast, chain observation, reconciliation, and claim
  release, with crashes at durable boundaries and a foreign winner.
- Shielded preparation cross-layer test: one suite-2 agreement reserves coordinator spending
  capacity and a shielded input note under the same operation. A missing local proving artifact
  leaves both reservations held after both stores restart. It does not submit or observe a
  shielded payment. `prepare_coordinated_transfer` now exposes that durable handoff to SDK
  callers using the coordinator's persisted terms and authorizations.
- The operator default requires two matching RPCs. The public-bound relayer has a separate
  verification endpoint and per-observer checkpoints. Broadcast failover cannot downgrade
  verification. `ShieldedChain::reconcile` also requires a peer and separate cache; wallet
  restoration compares both finalized histories before changing note state.
- Pool RPC has a strict `finalized_head` read that checks the configured chain and rechecks
  the returned block by number. A local RPC test rejects missing, ahead-of-head, wrong-chain,
  and contradictory responses. Shielded observation now reads the pool asset, verifier version,
  and `consumedDeals` at pinned block hashes; it matches canonical `DealTransferred` events to
  locally recorded deal openings before passing evidence to coordinator reconciliation.
  A missing or contradictory read errors without releasing a reservation. This is one-RPC
  evidence, not a provider quorum, deployed-code audit, or end-to-end shielded settlement test.
- The public pool index retains transfer identities and rechecks the winning event against a
  canonical RPC block. Its v1 cache is discarded and rescanned because v1 did not retain those
  fields. Verified index batches are saved for resumable recovery.
- Shielded EVM submission uses the same account-wide signer journal and coordinator attempt
  history as public-bound settlement. The shielded wrapper checks the accepted context and
  transfer public inputs before reserving a nonce or signing. Restored bytes are checked against
  chain, pool target, full calldata, sender, nonce, gas, and fee caps. Fee replacements retain
  the same call and nonce. Pool observation uses the finalized anchor and recorded agreement
  openings to update coordinator accounting, then releases a finalized consumed signer nonce.
- Relayer: `RelayPolicy` admits only the published signed fee and recipient; `RelayService` adds
  per-client access limits, redacted counters, gas-payer funding diagnostics, and ordered
  provider failover. Providers must share the policy deployment and one signer. The public
  unjournaled `RelayService::submit` entry point has been removed. `erebus-tx-relayer` supports
  one-shot requests and persistent `--serve` mode.
  The latter retains rate-limit windows and counters until process exit.
- `RelayService::submit_journaled` sends an existing coordinator transaction without signing again.
  It records uncertainty before RPC calls and uses identical bytes across providers. Tests cover
  falsified hashes, dropped responses, and timeouts after node acceptance. Admission and
  transaction checks run before the durable attempt; rejected clients cannot exhaust its attempt
  history. The CLI now uses relayer-owned coordinator records and this journaled path.
  A dedicated gas account and shared state root are required across relayer processes.
- RPC fault proxy (DM6-9): a TCP JSON-RPC proxy in the tests forwards to Anvil and can drop a
  broadcast response after forwarding, delay broadcast or finality responses, return a different
  broadcast hash, or return an impossible `finalized` anchor. These faults do not release or commit
  reservations. After restart, honest chain evidence resolves dropped and timed-out broadcasts.
  A proxy counter checks that recovery does not send the transaction again.
- An Anvil snapshot/revert test removes a mined payment. Reconciliation retains the spending
  reservation and signer nonce claim. Rebroadcast uses the same transaction hash and produces
  exactly one payment on the surviving chain.
- Sender validation recovers the signer from the transaction signature. A regression test rejects
  modified RPC `from` metadata with unchanged signed bytes and transaction hash.
- Operator surface: `docs/metropolis-m6-runbook.md` covers a stalled operation, the transaction
  relayer, the message relay, the pool indexer, RPC failures, and a secrets-free escalation
  checklist. The relayer, message relay, and pool indexer each expose a health surface.

## Evidence

Local checks passed: 24 journal tests, 19 coordinator lifecycle tests, 79 core tests plus five
core doctests, 24 EVM unit tests, and 11 EVM offline integration tests. Formatting, Clippy with
`-D warnings`, and warning-denied documentation pass for `sdk/journal`, `sdk/coordinator`,
`sdk/evm`, and `erebus-core`. An earlier EVM run passed 21 local-chain tests
(20 backed by Anvil), 11 offline integration tests, and four CLI process tests.

The Anvil-backed tests in `sdk/evm/tests/local_chain.rs` include:

- `coordinated_settlement_finalizes_commits_and_releases_the_signer`
- `a_foreign_winner_resolves_an_unknown_attempt_and_holds_the_unconsumed_nonce`
- `crashes_at_durable_boundaries_yield_one_payment_and_recoverable_state`
- `relayed_settlement_pays_the_published_fee_recipient`
- `the_relayer_fails_over_providers_and_reports_funding_shortfall`
- `a_dropped_broadcast_response_stays_unknown_and_reconciles_from_chain`
- `a_falsified_or_dropped_rpc_response_is_never_payment_evidence`
- `invalid_relayer_signer_or_deadline_preserves_durable_state_without_sending`

The last test passed locally against Anvil on 2026-09-30. A different relayer signer
or zero timeout is rejected before recording any broadcast attempt. The stored signed
bytes, signing plan, spending reservation, and signer nonce claim remain unchanged;
the proxy records no send, the chain nonce stays unchanged, and no payment occurs.
This verifies a library boundary, not packaged CLI submission.

Funding diagnostics now use ordered provider failover with a 15-second per-provider
deadline. Library callers can select a deadline with `funding_with_timeout`; zero is
rejected before RPC work. Each successful read checks the configured chain ID before
and after estimating gas and reading the balance. Anvil tests pass for a dead primary,
an empty gas account through the backup, and a timed-out primary. These reads never
broadcast. CLI submission now requires explicit persistent storage and gas caps.

After the funding changes, the release-mode local-chain regression run passed 27 tests
on 2026-09-30. This run excluded only the exhaustive public-bound write sweep, which
has separate evidence above. Warning-denied Clippy and documentation also passed.
These results are local Anvil evidence, not remote CI or Monad testnet evidence.

The independently launched relayer CLI now accepts `relay` and `recover` with configured
persistent storage. It imports existing buyer/seller signatures, persists an operation replica
and its gas transaction, and observes before retrying a send. The replica is not the buyer's
spending-policy ledger; the buyer still enforces its own policy locally. `recover` never signs
or broadcasts and accepts an existing request after expiry or a fee-schedule change.
Two Anvil-backed CLI process tests passed: a dropped response survives restart and endpoint
switching with one payment, and finalized expiry closes an unpaid operation while retaining
an unused nonce claim. Responses include `gas_account_busy` separately from payment stage.
Malformed requests create no state. Explicit fee caps and gas limit are required for submission.
The final release-mode EVM local-chain run passed all 30 tests, including the exhaustive
public-bound write sweep and both CLI restart/recovery tests. This supersedes the earlier
27-test regression count. The ordinary run passed 24 unit, 12 offline, six CLI, and two
non-ignored local-chain tests; ignored Anvil tests were run separately as stated above.

After historical recovery changes, the release-mode local-chain run passed all 33 tests.
This includes restartable log scans, old winner ancestry, nonfinal reorgs, and CLI history
continuation before any nonce allocation or send. Paired-provider tests reject duplicate
checkpoint directories and conflicting histories. History checkpoint tests reject stale
writers, corruption, and future versions; each discovered write boundary recovers after restart.
The ordinary EVM run passed 27 unit, 12 offline, six CLI, and two non-ignored local-chain tests.
Ten shielded RPC tests passed, including bounded continuation across reopened caches and a
long orphaned suffix with a fixed new-block budget. These are local tests, not Monad evidence.
The funded native-proof runner also passed. A fresh shielded index resumed through 28
pending batches at three blocks per call. Each pending result retained spending and note
reservations. Completed history found the authorized finalized winner, and the coordinator
then finalized one payment with one send. Output recovery, withdrawal, and replay rejection passed.
Formatting, warning-denied Clippy, and warning-denied documentation passed for both crates.

After the mandatory paired-provider policy, the release-mode EVM local-chain run passed
all 36 tests on 2026-10-01. The relayer rejects a missing or duplicate verification endpoint
before creating state. A process test keeps spending and nonce reservations held when the
designated verifier fails, even with healthy broadcast backups. Honest paired recovery then
finalizes the existing payment without another send. A history test advances the chain while
one observer is pending and verifies that the completed observer retains matching pinned anchors.

The ordinary EVM run passed 27 unit, 12 offline, seven CLI, and three non-ignored local-chain tests.
The shielded ordinary suites passed, including 11 RPC tests. Six shielded unit tests and the
new stable crash-target test also passed. Paired wallet tests reject conflicting histories,
shared caches, and an unavailable peer without writing the encrypted wallet.

The funded paired shielded sweep passed all 74 discovered write failures and its baseline.
Both observers use separate caches. Injection targets include the stable relative path, durable
step, and occurrence; random temporary names and concurrent write order cannot change the target.
Each fault must fire exactly once and interrupt the operation before restart/retry.
Every trial recovers one transfer, one consumed nonce, wallet change, accounting, and nonce release.
The final funded history rehearsal resumes through 55 pending batches with reservations held.
The native proof, output recovery, withdrawal, and replay checks passed.
Public-bound funded recovery also uses paired deal and nonce reads in its 56-boundary sweep.
Positive pairs share Anvil infrastructure, so these runs verify mechanics, not provider independence.

The test proxies now reuse HTTP clients and advertise connection closure when serving one request
per socket. Earlier runs exposed local port exhaustion and a stale keep-alive promise; those
failed runs are not passing evidence. Final formatting, warning-denied Clippy and documentation,
and documentation-link checks passed. No commit, push, remote CI, or Monad transaction is claimed.

The optional M5 funded runner now launches `examples/coordinated_transfer.rs` when
`EREBUS_M6_COORDINATED=1`. It generates a native proof for its live deployment, restarts the
encrypted wallet and coordinator after persisting signed bytes, restores the prepared proof
without reproving, broadcasts the exact bytes, and advances Anvil finality. Finalized pool
evidence recovers the spent input and buyer change before updating payment accounting.
The runner also checks outputs, withdrawal, and replay rejection.
It uses known test entropy and Anvil's default test key. It is not Monad or production evidence.

Run from the repository root:

```sh
forge build --root contracts/evm
cargo test --manifest-path sdk/evm/Cargo.toml --locked --offline -- --ignored
cargo test --manifest-path sdk/coordinator/Cargo.toml --locked --release --test lifecycle every_discovered -- --ignored
cd circuits/m5 && EREBUS_M5_NATIVE_PROOF=1 EREBUS_M6_COORDINATED=1 npm run prototype
# Add EREBUS_M6_MATRIX=1 to run the funded shielded crash sweep in release mode.
```

## Remaining Gates

1. **Full fault matrix.** The proxy and boundary-crash tests cover dropped and falsified
   responses around broadcast and observation, including broadcast and finality timeouts.
   The real-Anvil public-bound observation sweep times out block, contract, and log reads,
   then verifies recovery without resubmission. The funded shielded example withholds the
   response after submitting a transfer, times out those observation methods, and recovers
   through honest finalized evidence with exactly one send. Wallet reservation and public
   cache tests discover and fail every fsync/rename boundary before restarting and retrying.
   The coordinator lifecycle sweep for both suites passed in release mode. It discovers
   write boundaries independently for each transition, injects each failure, and verifies
   restart/retry with one reservation and unchanged signed bytes. CI runs it separately.
   The combined funded shielded sweep, including fee replacement, passed all 68 discovered operation-write boundaries
   plus its baseline. Each trial uses a fresh Anvil fork at the same funded block, reuses a
   locally generated proof for identical terms and note state, and verifies one transfer
   event, one consumed nonce, recovered change, committed accounting, and nonce release.
   Its report is included in `circuits/m5/build/run.json` only after every trial succeeds.
   The matching public-bound sweep, including fee replacement, passed its baseline and all 56 discovered write failures.
   Each trial checks the exact buyer debit, payment and fee balances, one settlement event,
   one consumed nonce, committed accounting, and nonce release. A funded expiry test verifies
   that finalized absence releases spending but never clears an unconsumed signer nonce.
   A two-endpoint Anvil test confirms that internally consistent histories can disagree about
   the same deal (`PaidFinalized` versus `ClosedUnpaid`). It applies neither result to accounting;
   it establishes the single-provider trust limit. Optional paired-provider APIs now reject
   differing anchored observations or finalized signer nonces before applying accounting.
   The public-bound tests cover agreement, disagreement, duplicate endpoints, and timeout.
   The funded shielded run covers agreement and peer timeouts with one submitted payment.
   Its positive pair shares an upstream, so it tests mechanics, not provider independence.
   These APIs require identical head and finalized anchors; honest providers at different
   heights can therefore hold recovery. The owner selected mandatory paired operator
   recovery on 2026-10-01. Explicit low-level single-provider APIs remain available for
   trusted-RPC experiments; two URLs alone do not establish honesty.
   Individual storage sweeps use
   synthetic backend evidence and do not establish chain execution on their own.
2. **Installed shielded workflow.** The funded example ties note reservation, local proof
   generation, transaction submission, and finalized recovery together in one Rust process.
   Packaging this as an installed operator command belongs to M8.
3. **Monad finality.** The code requires the RPC `finalized` tag, matching Monad's documented
   guidance (`finalized` is irreversible and intended for value settlement; `pending` behaves as
   `latest`). This is documentation research only; no live Monad testnet RPC was exercised.
   That is M8 evidence.
4. M7 disclosure and the M8 installed workflow remain separate milestones.

## Open Review Findings

### Legacy API Removal Verification (2026-10-01)

Local checks after the owner-selected removal:

- EVM ordinary tests: 27 unit, 12 offline-signing, 7 relayer CLI, and 3 non-Anvil tests passed.
- The complete local-chain release suite passed all 36 tests, including the funded
  public-bound persistence sweep. Successful former M3 calls use journaled submission and
  paired finality. Direct replay, amount mutation, and valid-but-expired calls produce
  actual included reverts. An unrelated successful transaction retains the deal reservation.
- Four compile-fail doctests passed for the removed APIs. CI has an explicit doctest step.
- The shielded ordinary `--all-targets` suite passed against the changed EVM library.
  Artifact-dependent native proof tests remain ignored in this command; this check does not
  replace the earlier funded shielded run or establish a new proof run.
- EVM format, strict Clippy, and warnings-denied rustdoc checks passed locally.

These are local results, not remote CI or Monad deployment evidence.

### Journal Rejection Review (2026-10-01)

The current Starknet `persist_signed` implementation checks the stage before writing a blob.
The earlier write-before-rejection finding is therefore fixed in the current worktree.
No production change was needed in this review.

`rejected_signing_preserves_every_file_across_historical_schemas` adds disk-level coverage
across all six historical fixture sets, spanning schemas v1-v4. It exercises 58 forbidden
signing states, including restarted attempts. Each call is retried, then the journal is
reopened. The test compares the in-memory record and every file's name, inode, permissions,
and contents. This catches orphan blobs that an unsigned record's read API would hide.

Local results: 21 journal tests, 27 journal characterization tests, and 24 shared storage
tests passed. The new test passed independently too. Format and targeted strict Clippy
checks passed. Historical fixtures and goldens were not regenerated.

The broader reconciliation characterization run passed its two wire-shape tests but eight
RPC-backed tests could not bind local sockets under the restricted sandbox (`Operation not
permitted`). This is not a passing reconciliation run. Re-run that suite in an environment
that permits loopback sockets; do not skip or weaken its RPC checks.

### Proof-Type And Doctest Review (2026-10-01)

`FinalPayment` and `NoEffectProof` retain private fields and no public constructor.
The ledger's proof-free commit/release methods remain private. The core CI job already
runs `cargo test --locked --doc`; `--all-targets` alone would not run these guards.
The review adds a positive `FinalPayment` accessor example alongside its compile-fail
constructor example. Both evidence types now have successful public-API examples, so the
negative examples are not the only checks for their documented interfaces.

Local verification passed 103 core unit/integration tests, six doctests (four compile-fail
and two positive), and 19 ordinary coordinator lifecycle tests. The exhaustive coordinator
sweep remains ignored in the ordinary command and was not rerun in this review. Core format,
strict Clippy, and warnings-denied rustdoc passed. The coordinator README now describes the
integrated paired observation and finalized nonce-release paths instead of calling them
unimplemented.

These types prevent direct construction mistakes, not a dishonest backend's fabricated
reads. `reconcile_deal` still trusts its supplied backend evidence; independent anchored
verification remains the chain adapter's responsibility. This is local coverage, not a
remote CI result or Monad testnet evidence.

### Remaining Limitations

- Low-level chain signing and broadcast primitives remain available for backend composition.
  They do not persist intent or supply coordinator recovery on their own. Supported operator
  workflows use the journaled path; there is no legacy backend submission shortcut.
- The legacy one-shot observer still has total-work limits. Recovery applications must use
  `finalized_deal_evidence_resumable`; the relayer now does so. Checkpoints are trusted local
  cache state, not chain proofs. Network budgets do not bound cache replay or serialization.
  The shielded cache still has a 128 MiB limit; compaction is not implemented.
- The CLI's `--serve` mode retains limits and counters across newline-delimited requests.
  Process tests cover retained limits, malformed input, one-shot exit codes, bounded input,
  and refusal of unjournaled submission.
  State resets on process restart. Client identity comes from trusted host configuration, not requests.
  Operation records and nonce claims survive restart; counters do not. An authenticated
  remote multi-client gateway remains outside this newline-delimited CLI mode.

Nothing here establishes a Monad deployment or release readiness.
