# Metropolis M6 Decisions: Coordinator, Relayer, and Recovery

Written: 2026-09-28. Updated 2026-10-01. Scope: roadmap M6. Status: **draft for owner review**;
the public-bound mechanism is integrated (see [M6 progress](metropolis-m6-progress.md)), and
several `Decision` lines below are still open. Mechanism sections state facts from the code;
every `Rationale` marked _owner_ is written by the owner.

M6 makes one authorized agreement produce at most one payment and a recoverable operation
state across crashes, dropped RPC responses, reorgs, and restarts, in both settlement modes.
It persists the spending-policy ledger and adds a public-bound relayer.

Related: [roadmap](metropolis-roadmap.md) M6, [architecture](metropolis-architecture.md) §2
and §6, [M3 decisions](metropolis-m3-decisions.md) DM3-9/DM3-10,
[decisions](metropolis-decisions.md) D02 and D04.

## DM6-1. What decides payment

**Mechanism.** `ErebusSettlement.settle` has no sender check. Anyone holding the terms opening
and both authorizations can submit until expiry, and the contract rejects
`block.timestamp >= expiry` and marks `consumedDeals[Ndeal]`. Every signed revision of a deal
shares one `Ndeal` (D02), so the contract already prevents double payment. The shielded pool
exposes the same shape: `consumedDeals`, `consumedNotes`, and an expiry check.

**Consequence.** Our own transaction's fate is not the payment's fate. "Our transaction never
landed" does not mean "unpaid": the seller, a relayer, or a copy of our calldata may have
settled the deal. The M6 failure to prevent is not a double payment. It is *reporting a deal as
unpaid when it is not*, which would release a policy reservation or invite renegotiation.

**Rule.** Expiry belongs to each signed revision, and a revision stays executable until its own
expiry or deal consumption ([agreement](metropolis-agreement.md) §7). So:

- A **revision** is proven unable to pay only by `consumedDeals[Ndeal] == false` at a final
  block whose timestamp is `>=` *that revision's* expiry.
- A **deal** is closed unpaid only when that holds for every revision that may have been
  signed.
- `consumedDeals[Ndeal] == true` does not identify the winning revision. The winner is found
  from the `DealSettled` log for `Ndeal` (the submitter may be anyone) and its calldata
  verified. Until then the deal is `ConsumedUnresolved`, and no reservation moves.
- Timeouts, RPC errors, reverts, advanced sender nonces, allowance changes, and local
  cancellation never release a reservation.

Implementation clarification (2026-09-28): unresolved consumption takes precedence
over a revision's final expiry. If the head reports consumption but the winner is
not verified, every revision remains held. This conservative rule matches the
classifier table; it does not identify an unknown payment as unpaid.

- Decision: _owner_
- Rationale: _owner_

## DM6-2. Journal: generalize the Starknet journal

**Mechanism.** `sdk/rs/src/journal.rs` already implements the durable mechanics M6 needs: an
explicit stage-edge table, `may_have_landed`, per-record advisory locks, atomic rename with a
directory fsync, stored signed transactions, leases, and prune. Its record types embed
Starknet values (`Felt` hashes and nonces, `rpc::Receipt`, `PreparedSnapshot`,
`WriteOperation`, `ChannelHandle`). On-disk schema is version 4; versions 1-3 are readable.
`Finding` and `ResumeOutcome` cross the `erebus-cli` seam (protocol 5), so their JSON is an
external contract.

**Decision (made).** One shared implementation: the chain-neutral mechanics move out of
`sdk/rs`, generic over backend-owned record types, so the Starknet path and the EVM backends
share them. The Starknet record types keep their exact serde layout; historical records load
and classify identically. Characterization tests of the current behavior land before any
source moves (architecture §6).

- Rationale: _owner_

**Open: where the shared code lives.**

| Option | Mechanism consequences |
|---|---|
| New crate (`sdk/journal` or `sdk/operations`) | Own lockfile and CI job; `sdk/rs`, `sdk/evm`, `sdk/shielded` add a path dependency. erebus-core stays free of serde derives, `fs2`, and file I/O |
| Module in `erebus-core` | No new crate; every core consumer (transport, a future third-party verifier) gains serde, `fs2`, and locking; `fs2` does not build for wasm |
| New crate `sdk/coordinator` | As `sdk/journal`, plus the deal classifier and diagnostics types in the same crate |

- Decision: new crate `sdk/journal` (`erebus-journal`). It is generic over backend-owned record
  types through traits (`Stage`, `AttemptRecord`, `JournalRecord`), so the Starknet structs and
  their serde derives stay in `sdk/rs` unchanged. The alternative considered was an engine with
  a `RecordCodec` and a `LifecycleProfile`.
- Rationale: _owner_

**Legacy records: compatibility retained.** Ordinary advances serialize current fields while preserving the
old version. `record_result` instead upgrades the record to v4 without converting
`accepted_at`. For v1/v2 records that field can be a local wall-clock value; reconciliation
trusts it as a block timestamp once the record reports version 3 or later. The historical
characterization fixtures pin this behavior, including the timestamp confusion. They
establish compatibility, not the safety of a migration.

The owner selected option 1 on 2026-10-01: preserve existing write-back behavior.
This retains compatibility but leaves the timestamp ambiguity unresolved.
No legacy records, schema versions, runtime behavior, or golden files change for this decision.

A future explicit migration (D04) must preserve timestamp provenance or obtain validated chain timestamps.
Changing the version alone cannot convert the old value.

- Decision: preserve legacy behavior; timestamp provenance remains a known limitation, not a completed safety migration.

## DM6-3. Mode order

**Decision (made).** Public-bound EVM goes through the full lifecycle and fault matrix first;
the shielded pool follows on the same coordinator.

- Rationale: _owner_

## DM6-4. EVM operation record and stages

**Mechanism.** The M3 adapter signs and broadcasts in one call through alloy fillers, so no
nonce or raw bytes exist before broadcast, and `verify` does not check that the receipt's
block is canonical. A journaled path must sign locally with an explicit nonce and persist the
raw transaction before broadcast.

Proposed stages, one record per deal, in its own namespace
`<evm_state>/eip155-<chain>/<contract>/<signer>/` (D04):

```text
claimed -> reserved -> authorized -> prepared -> signed -> submitted -> included -> finalized -> committed
included -> submitted                    (reorg)
any non-terminal -> expired              (only with recorded final expiry evidence)
attempt: reverted -> new attempt         (the deal may still be settleable)
any non-terminal -> needs_attention
```

Before the buyer signer runs, a durable "authorization may exist" fence is recorded, and each
outbound delivery of an authorization is recorded before it is sent. Nonces are allocated
under a lock keyed by chain and sender, across deployments, because an EVM nonce belongs to the
account, not the contract.

Fee bumps are same-nonce, same-calldata replacements, each persisted before it is sent.
A new attempt with a new nonce is allowed only when the previous attempt reverted or its
nonce is consumed at a final block by a different transaction.

- Decision: _owner_
- Rationale: _owner_

**Integrated (2026-09-29).** The journaled path is the coordinated lifecycle: nonce claim,
durable plan, local signing, persisted raw bytes, journaled broadcast attempt, chain
observation, and reconciliation. `sdk/evm/tests/local_chain.rs` drives it on Anvil, including
crashes at durable boundaries and a foreign winner.

**Decision (2026-10-01, owner).** Remove the M3 unjournaled backend entry points.
`EvmSettlementBackend::submit` and `settle` are removed, together with the legacy `verify`
and `with_confirmations` APIs. There is no deprecated or feature-gated compatibility path.
The backend now prepares and estimates without retaining a signing wallet. Supported
submission uses the coordinator's persisted signing plan and exact transaction bytes;
operator reconciliation requires paired explicit `finalized` evidence under DM6-5.
`RelayService::submit` was already removed.

The M3 happy-path tests now use this coordinated lifecycle. Replay, mutated calldata, and
genuinely expired authorizations are sent directly only in adversarial contract tests, where
an included reverted transaction establishes contract rejection. Compile-fail doctests guard
the four removed methods. Low-level chain signing/broadcast primitives remain public for
backend composition; they do not provide durability without the coordinator.

- Rationale: _owner_

## DM6-5. Finality source and evidence quorum

**Mechanism.** DM3-9 uses a configured confirmation depth (default 1). The release rule in
DM6-1 depends on "final block" and on block timestamps being monotonic.

**Implemented policy (2026-09-29).** The chain observation requires the RPC `finalized` block
tag explicitly and never substitutes a confirmation count or the `safe` tag. It pins a block
by number and hash, rechecks the anchor after each read, and walks parent links from head to the
oldest required anchor; a missing, non-canonical, or inconsistent anchor is an error, not
absence evidence. Receipts and winner transactions are validated against the canonical block,
including hash, position, sender recovery, and log locations. This is one RPC with internal
consistency checks, not a quorum.

**Disagreement evidence (2026-09-30, local).**
`internally_consistent_rpc_histories_can_disagree_about_one_deal` runs two Anvil endpoints
with identical deterministic chain IDs, contract addresses, and token deployments. One
settles the accepted deal; the other reaches finalized expiry without settling it. Both
endpoints pass their own canonical-history and contract-state checks, yet classify the same
agreement as `PaidFinalized` versus `ClosedUnpaid`. The diagnostic test applies neither result
to accounting. This verifies the trust limitation, not an implemented multi-provider guard.
At that point, the owner decision on matching providers remained open.
Provider failover alone is not a quorum.

**Optional paired reads (2026-09-30, local).** Public-bound and shielded observation now offer
explicit paired-provider APIs alongside the unchanged single-provider APIs. They require
matching deployment identities, distinct endpoints, matching head and finalized anchors,
matching deal evidence, and matching finalized signer nonce evidence. Errors or disagreement
prevent accounting updates. Both providers' anchors are rechecked after comparison.
Public-bound Anvil tests cover matching reads, divergent histories, and peer timeouts.
The funded shielded example covers paired recovery and peer timeouts with one payment send.
Its positive pair uses the same upstream behind two endpoints; it does not demonstrate
independent infrastructure. Exact anchor equality can reject honest providers at different
heights. This is an opt-in consistency guard, not proof of canonical consensus or a selected
operator default at that point. The owner selected mandatory paired verification on 2026-10-01.

**Monad verification (2026-09-29, documentation only).** Monad's JSON-RPC exposes standard
block tags mapped to its commitment states: `latest` is the proposed block (speculative,
no consensus vote), `safe` is voted (supermajority), and `finalized` is final — the Monad docs
state it is "irreversible without a hard fork" and direct value settlement to it; `pending`
behaves like `latest`. Monad mainnet is chain id 143; finality tracks block time (about 600 ms
on mainnet as of July 2026). So requiring `finalized` and refusing a confirmation-count fallback
matches Monad's own guidance. Two documented constraints matter to M6: EIP-4844 blob
transactions are rejected, and the `newPendingTransactions` subscription is unsupported, so the
relayer and indexer must not depend on either. This is documentation research; no live Monad
testnet RPC was exercised. Live verification is M8 evidence.

Sources: Monad JSON-RPC overview (`docs.monad.xyz/reference/json-rpc/overview`) and the Monad
Foundation post on 300 ms blocks and 600 ms finality (2026-07-23).

**Decision (2026-10-01, owner).** Supported EVM operator recovery requires two matching
RPC providers before changing accounting, wallet state, or finalized nonce claims.
An unavailable provider, incomplete history, or disagreement retains reservations.
There is no automatic downgrade to a single provider.

The public-bound relayer requires `EREBUS_RELAYER_VERIFICATION_RPC_URL` separately from
its broadcast/failover endpoints. Each selected broadcast endpoint must differ from the
verification endpoint after URL normalization. Each observer has its own history directory.
Broadcast failover keeps the designated verifier; it never replaces paired verification
with one-provider evidence. Funding diagnostics and health remain available without two RPCs.

`ShieldedChain::reconcile` now requires a peer chain, peer RPC, and separate index cache.
Both sources must agree on deal evidence, finalized signer nonce, and finalized wallet history.
`reconcile_agreed` remains a compatibility name for the same default path.
`reconcile_single_provider` is an explicitly named trusted-RPC experiment, not the operator default.
Low-level one-provider observation APIs remain available; callers must not treat them as paired evidence.

This is a two-of-two consistency policy, not Byzantine consensus. Operators must select
independent infrastructure. Matching false responses from colluding providers can still pass.
Exact anchor equality can hold recovery when honest providers have different heights.
New gas-nonce allocation and funding estimation still use a selected broadcast provider;
the paired checks govern settlement accounting and finalized claim release.

- Rationale: _owner_

## DM6-6. Concurrency

**Mechanism.** The Starknet client refuses a new write while any journalled operation is
waiting or needs attention, under one identity lock.

**Decision (2026-09-29, owner delegated).** Keep one unresolved operation per chain
and gas-paying signer for the first M6 release. All settlement deployments using
that account share its slot. Different signers can proceed independently.

This reduces nonce-gap recovery cases at the cost of queued payments per signer.
Multiple ordered transactions remain a later extension, not an implicit fallback.
Use a dedicated transaction signer; an external wallet bypasses this local journal.

`SignerJournal` now persists the claim and immutable signing plan before signing.
`resume` recovers it after restart without a new nonce observation.
Local expiry, timeout, or a higher pending nonce never clears the claim.
RPC nonce acquisition now checks chain identity and matching latest/pending counts.
Verified claim release (2026-09-29): `SignerJournal::release` clears the claim only when a
finalized account nonce is strictly greater than the claimed nonce, proving the claimed nonce
was consumed and the signed bytes can never land. The observer may belong to any deployment on
the chain because an account nonce is account-wide. A resolved deal does not release a nonce
that was never consumed; the foreign-winner test asserts `NotConsumed` in that case.
Do not remove the journal to unblock a signer.

## DM6-7. Policy ledger persistence

**Mechanism.** `erebus_core::policy` counts a reservation against limits until it is committed
or released, so an uncertain payment stays reserved. Under DM6-1, a public-bound reservation is
held from buyer authorization until final consumption or final expiry.

The ledger's state rules stay in core. The local coordinator persists intent and
accounting together under the journal identity lock, without a second ledger file.
Raw `commit` and `release` are private. `restore` rebuilds a trusted local snapshot;
`reconcile_deal` applies a whole classifier assessment atomically. A losing
revision cannot release capacity before its winner's reservation is accounted for.
Private proof fields prevent accidental construction, not fabricated backend facts.
The chain adapter must establish those facts independently.

The current local coordinator holds the identity lock during local signing and
preparation callbacks. These callbacks must not perform network I/O. Transaction
submission and network reconciliation are not yet wired into this store.

**Open.**
- Reservation unit (decided): **one reservation per signed revision**. Each revision
  reserves its own amount plus fee, although at most one can pay. This over-reserves while
  several revisions are live and uses the core ledger unchanged. When a revision wins, its
  reservation is committed; losing revisions are released only after the winner is final or
  their own expiry is final. Rationale: _owner_
- Cap how far in the future an authorized expiry may be, since it bounds how long budget
  stays locked?
- Ledger scope: EVM only, or also Starknet `accept_and_settle`?

- Decision: _owner_
- Rationale: _owner_

## DM6-8. Relayer

**Decision (made).** The relayer is public-bound only. `ErebusSettlement` binds `fee` and
`feeRecipient` into the signed terms, so a relayer is paid by the deal and cannot redirect
value. The shielded circuit fixes fee = 0, so shielded settlement is self-submitted in M6; the
gap and the resulting linkage (the buyer's gas-paying address is the transaction sender) are
logged in `docs/friction.md`.

- Rationale: _owner_

**Implemented (2026-09-29).** The relayer publishes its schedule through the `policy` request
(`erebus-tx-relayer`) before the buyer authorizes. `RelayService` enforces per-client access
limits, reports gas-payer funding with an explicit shortfall, and tries configured providers in
order so one dead endpoint does not strand an admitted agreement.

**Open.** The fee recipient is committed before authorization, so the relayer is chosen before
signing. How does a buyer obtain a relayer quote, and what happens if that relayer is down?
Mechanism fact: `ErebusSettlement.settle` has no sender check, so the buyer can always
self-submit the same authorized bytes; a different relayer with a different fee recipient
cannot, because the fee and recipient are signed terms. Choosing another relayer therefore
requires re-authorization.

- Decision: _owner_

## DM6-9. Fault injection

**Mechanism.** Every durable boundary (record flush, blob write, ledger write, and both sides
of broadcast) calls a fault hook that is a no-op in production. The boundary list is recorded
from a happy-path run rather than maintained by hand, so a new boundary automatically joins
the matrix. The matrix runs on Anvil with a JSON-RPC proxy that can drop or falsify responses.

Per run it asserts: at most one `DealSettled` for `Ndeal`; buyer balance change is zero or
amount plus fee; final stage matches chain state; reservation committed iff paid-final and
released iff expired-final; no nonce gap.

**Implemented (2026-09-29, public-bound).** A TCP JSON-RPC proxy in `sdk/evm/tests/local_chain.rs`
forwards to Anvil and can (a) forward `eth_sendRawTransaction` and then drop the response,
(b) return a different broadcast hash without forwarding, and (c) return an impossible
`finalized` anchor. Tests assert that a dropped response stays `Unknown` and is resolved by
honest chain evidence without a second send, and that a falsified response never commits or
releases. Crashes after the signed-transaction and broadcast-attempt boundaries are covered on
the real chain. Not yet implemented: a delay/timeout sweep, a fault injected at every durable
boundary in one automated matrix, provider disagreement between two endpoints, and the same
matrix for shielded mode.

**Additional local evidence (2026-09-30).** A public-bound Anvil test times out block reads,
contract reads, and log queries individually. Each error holds accounting and the signer
claim; subsequent honest observation commits the existing payment without resubmission.
The funded shielded example performs the same observation timeout sweep after a withheld
broadcast response, and checks finalized wallet recovery with exactly one send. Wallet and
public-index writers expose the existing durable-step hook; their tests discover each write
boundary and verify restart/retry without partial snapshots. These are separate checks, not
yet the single funded matrix described above. The single-provider trust model in DM6-5
still cannot detect a self-consistent false history from its selected RPC.

**Combined funded shielded sweep (2026-09-30).** `EREBUS_M6_MATRIX=1` runs the coordinated
example in release mode. A baseline discovers 60 operation-write boundaries across the
coordinator, account signer journal, encrypted note wallet, and public index. Each subsequent
trial fails one boundary, reopens the stores, checks chain evidence before retrying a send,
and recovers one actual transfer event and consumed nonce, change, accounting, and nonce
release. All 60 failures and the baseline passed locally. Initialization and proof-generation
failures are separate tests; the matrix reuses one locally generated proof with identical
deployment, terms, input, path, and change across reverted Anvil snapshots. It uses known test
entropy and is not Monad deployment evidence. The matching funded public-bound sweep and
provider-disagreement gate remain open.

**Combined funded public-bound sweep (2026-09-30).** The ignored Anvil test
`every_public_bound_operation_write_recovers_one_actual_payment` passed its baseline and
all 48 discovered operation-write failures. Each trial checks the buyer's exact amount-plus-fee
debit, seller and fee-recipient balances, one settlement event, one consumed signer nonce,
committed accounting, and nonce release. It reopens the coordinator and signer journal after
failure and reconciles chain evidence before sending. The funded expiry test separately checks
that final non-consumption releases spending while an unconsumed signer nonce stays claimed
across restart. CI runs the exhaustive sweep in release mode. Provider-disagreement policy
and evidence remain unresolved; neither sweep is evidence of a multi-provider quorum.

**Replacement extension (2026-09-30).** Both funded sweeps now sign one fee-bumped
replacement before broadcasting. This adds the durable replacement-plan and signed-byte
boundaries to the discovered matrix. The public-bound sweep passed all 56 injected failures
and its baseline; the shielded sweep passed all 68 and its baseline. Replacement validation
preserves sender, nonce, target, calldata, and gas limit while enforcing the fee bump. The
same one-payment and one-consumed-nonce assertions remain in each recovered trial.

**Shielded harness isolation (2026-09-30).** Repeated snapshot restoration intermittently
failed historical `eth_call` reads in Anvil. The shielded matrix now starts a fresh local fork
at the same funded base block for each trial and shuts it down afterwards. All 68 injected
failures and the baseline passed with this harness, followed by the paired-provider funded
recovery check. Terms, deployment, note state, and the reused local proof remain identical.

## DM6-10. What this record does not establish

- No Monad network evidence (M8).
- No shielded relayer compensation.
- No disclosure package (M7).
- No consensus proof or provider independence. DM6-5 selects mandatory two-provider consistency checks.

## DM6-11. Independently operated relayer recovery

**Implementation (2026-09-30, local).** `DurableRelayer` reuses the coordinator in
relayer-owned, operation-specific directories. It attaches the request's existing
authorizations; it holds neither participant key and never signs an agreement. These
records track submission, not buyer budgets. Buyer policy enforcement remains local
to the buyer's authoritative coordinator. One shared signer journal gates the dedicated
gas account across operations, buyers, and deployments.

Admission precedes storage and nonce allocation. Intent, authorizations, backend evidence,
signing plan, and signed bytes are persisted before sending. Retries observe canonical chain
evidence first and reuse durable bytes. Recovery is observation-only, accepts expired requests,
and remains available after the service fee schedule changes. New admission still requires
the current fee policy. Existing signing plans remain authoritative after gas-cap changes.

The CLI requires a private state root and explicit gas caps. It exposes `gas_account_busy`
separately from the payment stage. Local process tests verify a dropped response, restart,
provider switching, one actual payment, finalized nonce release, and expired non-consumption
without clearing an unused nonce claim. No participant keys are passed to the CLI.

The current CLI is newline-delimited JSON operated by a trusted host, not an authenticated
remote multi-client API. Rate limits and counters are process-local; operation and nonce
records are durable. Observation now requires the designated verification provider alongside
a selected broadcast endpoint; see DM6-5. This is public-bound settlement,
not a shielded payment service, remote CI evidence, or a Monad deployment.

## DM6-12. Bounded historical observation

**Implementation (2026-09-30, local).** Public-bound observation now has a durable
continuation API: `EvmChain::finalized_deal_evidence_resumable`. An `ObservationJournal`
stores the pinned anchors, next log range, parent-walk position, and verified finalized prefix.
Each call limits log queries and parent links. `Pending` is not settlement evidence.
The coordinator must retain every reservation until the call returns `Complete`.

Every continuation rechecks chain identity and canonical anchors. Nonfinal reorgs restart
unfinished work from the verified finalized prefix. A changed finalized anchor rejects recovery.
After completion, later calls scan the new suffix instead of genesis or the old winner again.
Atomic versioned checkpoints reject corruption and stale concurrent writers. No file lock
crosses network I/O. These checkpoints are trusted local caches, not chain proofs.

The independent relayer uses this API before nonce allocation, signing, or submission.
Its history budgets are configurable. `recover` advances pending history without sending.
The legacy one-shot observation method remains available and retains its total-work limits.
New recovery applications must use the continuation API rather than retry that method unchanged.

The paired-provider API uses separate checkpoint stores. Either pending scan holds
recovery. Completed results must match at identical head and finalized anchors. Provider lag
can therefore delay recovery. Two endpoints do not establish independent providers or consensus.
The relayer requires this paired path under the owner decision in DM6-5.
A completed observer retains its pinned scan while its peer finishes historical work.
This avoids repeated refreshes to newer heads before the pair can compare completed evidence.

Shielded observation saves at most 1,000 new pool blocks per call by default.
`observe_shielded_deal_bounded` accepts a smaller budget. An incomplete scan returns
`RecoveryError::HistoryPending`, never partial deal evidence. Restart uses the same verified
index cache. A binary search finds the shared prefix after a nonfinal reorg, instead of one
RPC read per orphaned block. Root, leaf count, event continuity, and finalized anchors still
must agree before accounting changes.

These budgets bound network scan work, not local replay, memory, or serialization.
The shielded index keeps its 128 MiB cache limit and stores empty blocks for ancestry.
An oversized cache fails closed; it does not become empty history. Index compaction and
fresh-install synchronization throughput remain operational limits, not claims resolved by
this implementation. A shielded scan reads a fresh target on each retry; a growing chain
can require further retries. The existing explicit wallet-restoration APIs can still scan
multiple batches in one invocation.

Local evidence covers restart, pending reservations, canonical suffix replacement, checkpoint
write failures, corruption, stale writers, old winner ancestry, and paired-provider disagreement.
The relayer process test verifies that pending history allocates no nonce and sends no payment.
No result here establishes Monad behavior, remote CI, or a production release.
