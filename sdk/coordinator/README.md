# Local Settlement Coordinator

This crate implements the local intent and policy boundary for Metropolis M6.
The crate itself does not submit transactions or read a chain. The public-bound EVM integration
lives in `sdk/evm`: `SignerJournal` allocates one nonce per chain and gas-paying account,
`EvmChain::broadcast_journaled` records a broadcast attempt around the exact persisted bytes,
and `EvmChain::finalized_deal_evidence_resumable_agreed` produces complete paired-provider
`DealEvidence` that `reconcile` accepts. Pending history holds reservations.
See `sdk/evm/tests/local_chain.rs` for the coordinated lifecycle, crash-boundary recovery, and
the foreign-winner case.

`Coordinator::record_intent` saves the canonical agreement opening and its reservation
in one atomic snapshot. `authorize_buyer` saves a signature fence before invoking
the local signer. `prepare` requires both verified authorizations before invoking
the backend callback. Reopening the store restores reservations without evaluating
them against a changed policy.

`sign_transaction` stores one immutable signed transaction before returning bytes
for broadcast. Both callbacks must run locally and must not broadcast.
The backend validator checks the transaction against the agreement and trusted
sender, nonce, fee, and gas parameters. A retry checks the stored bytes without
calling the signer again. It syncs the snapshot again before returning.

The coordinator stores an immutable backend signing plan before calling the signer.
Both callbacks receive that stored plan. Retries reject a different plan, even if
the previous signer failed. `transaction_plan` retrieves it after restart or expiry.
The EVM `SigningPlan` encodes sender, nonce, fee caps, and gas limit in 69 bytes.
The agreement supplies the chain, contract, and payment data.
The coordinator does not allocate nonces. It stores replacement history through
`sign_replacement`, which persists each replacement plan before calling the local signer.
The backend validator must check the same sender, chain, nonce, payment, and gas,
plus a sufficient fee bump against the previous signed transaction.
The initial plan and all signed bytes remain immutable. A failed signer requires
a retry with the same replacement plan. The limit is 16 replacements per operation.
The public-bound EVM integration first reserves a nonce through
`erebus_evm::chain::SignerJournal`, then stores the returned signing plan here.
That journal permits one unresolved operation per chain and gas-paying signer.
`EvmChain::reserve_nonce` checks chain identity and account nonce observations first.
`SignerJournal::release` requires a verified finalized consumed nonce. Supported operator
recovery obtains that evidence through `EvmChain::verified_finalized_nonce_agreed`.
An unpaid expired deal does not release an unconsumed gas-account nonce.
After signing, call `begin_broadcast_attempt(operation_ref, now)` before any network request.
It persists a new `Unknown` attempt and returns an opaque token plus an owned transaction payload.
The payload contains raw bytes, the stored plan, and the prepared settlement through explicit accessors.
Its `Debug` output is redacted. No file lock survives the call, so the caller can await network I/O.
The caller then passes the token and a `BroadcastOutcome` to `finish_broadcast_attempt`.
Only a matching operation and token can finish that attempt.

Every retry appends an attempt. Restart preserves incomplete attempts as `Unknown`.
New responses do not erase earlier attempts. Conflicting responses fail closed.
Submission, rejection, and timeout never release reservations or prove payment.
The limit is 1,024 broadcast attempts per operation. Full histories require explicit recovery.
`broadcast_history` exposes attempt indices, transaction indices, and local outcomes without payloads or tokens.

`signed_transaction` retrieves the original transaction after restart, expiry, or reconciliation.
`signed_transaction_at` retrieves a replacement by index. Both re-sync storage before returning signed bytes.
`replacement_plans` also retrieves unsigned replacement plans for recovery.
These APIs expose private recovery data. They do not authorize a network request.
Begin always selects the latest replacement and refuses an unsigned replacement plan.
Plans are limited to 1 KiB; transactions are limited to 128 KiB.
Older unsigned snapshots remain readable. Prototype snapshots with signed bytes
but no plan fail closed. Do not delete them or recreate their spending history.
They require an explicit recovery procedure before further signing.

All sessions for one buyer must use the same private state directory. The identity
lock serializes writers across threads and processes. The snapshot format is version 1;
unknown versions fail closed. The store caps snapshots at 16 MiB and 1,024 intents.
It does not prune payment history automatically.
An initialization marker makes a missing snapshot fail closed. Back up the entire
directory, including that marker. Complete loss of the directory still requires
external recovery; creating a new directory does not restore spending history.

## Trust Boundaries

- The caller obtains the peer context through authenticated transport. Comparing
  two context values does not authenticate either participant.
- The caller selects an actual backend before opening the coordinator. The
  coordinator checks its declared capabilities, not its implementation.
- `reconcile` accepts already verified backend facts. Do not populate these facts
  from an exported receipt flag, local timeout, or unverified RPC response.
  For supported public-bound EVM operator recovery, use
  `EvmChain::finalized_deal_evidence_resumable_agreed` with two providers and separate history stores. It
  verifies the canonical block, the complete settlement-log history, the winner's calldata,
  both authorizations, and the emitted event. Its errors must map to `DealEvidence::Unknown`,
  which holds every reservation.
  Pending history also holds reservations. Single-provider APIs are low-level alternatives,
  not evidence that satisfies the operator default. Provider independence remains an operator responsibility.
- A known final winner commits its reservation and releases losing revisions in
  one snapshot. Unknown consumption retains every reservation.
- Backend evidence must contain no private witness. The backend owns that check;
  the coordinator checks the evidence's routing and agreement identity.
- Snapshots contain private terms, blinding, and authorizations in `0600` files
  under a `0700` directory. They are not encrypted at rest. Use protected local
  storage and encrypted backups; never upload snapshots as diagnostics.
- Diagnostics expose local operation IDs, stages, attempt counts, replacement counts, and local response categories.

## Checks

From the repository root:

```bash
cargo test --manifest-path sdk/coordinator/Cargo.toml --offline --locked
cargo clippy --manifest-path sdk/coordinator/Cargo.toml --offline --locked --all-targets -- -D warnings
cargo fmt --manifest-path sdk/coordinator/Cargo.toml --check
```

Tests cover both agreement suites, concurrent policy reservations, restart,
authorization fences, context mismatches, and injected storage failures.
Broadcast tests cover restart uncertainty, token matching, late responses, and lock release before network I/O.
Replacement tests cover immutable history, plan fences, bounded growth, and backend validator rejection.
Fault tests stop begin, finish, retrieval, and replacement writes at each journal storage boundary.
The chain facts in these tests are synthetic. They do not prove at-most-once
payment on an EVM chain or completion of M6.

The public-bound integration and funded shielded example now cover backend nonce allocation,
replacement validation, paired chain reconciliation, note reservations, change recovery, and
durable-boundary crash matrices. These are separate local integration checks, not evidence
from the synthetic tests in this crate. The relayer uses its own durable coordinator records.
Installed operator workflows and Monad testnet verification remain M8 gates. See
[M6 progress](../../docs/metropolis-status.md) for commands, evidence, and limitations.
