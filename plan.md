# Erebus Operator Alpha Plan

## Outcome

Ship `v0.2.0` as an operator alpha in which a process can stop during a write, restart,
reconcile the attempt without duplicating an on-chain effect, and tell the operator what is
safe to do next.

Wire v3, repeat deals, native deal subkeys, and recipient-bound per-deal disclosure are the
starting point. This milestone does not add another wire generation. Protocol 4 is the
coordinated CLI, Python, and MCP interface change needed for durable operations.

## Scope boundary

This milestone deliberately moves crash recovery, idempotency, and operator diagnostics into
scope now that the MVP definition of done is green. Update `CLAUDE.md` when the protocol-4
integration work begins so that it no longer describes this work as out of scope.

Still out of scope:

- Mainnet use or a production-readiness claim.
- A frontend, dashboard, or hosted multi-tenant service.
- Private transfers, swaps, bridges, free-form messaging, or platform receipts.
- Automatic retry of an ambiguous submitted transaction.
- Protocol logic, cryptography, entropy generation, or key handling in Python.

## Decisions already made

1. **Operation IDs are caller supplied.** Every write uses an ID with the form `op_` followed
   by 64 lowercase hexadecimal characters. The Rust SDK validates IDs but does not generate
   them.
2. **The caller persists intent before making the MCP call.** The persisted record contains
   the operation ID and canonical request parameters. A crash before persistence produces no
   call. A crash after persistence retains the ID needed to reconcile safely.
3. **The Python binding stays mechanical.** It forwards an operation ID but never generates
   one and owns no idempotency or protocol logic.
4. **Rust is authoritative for operation outcomes.** Its journal records what was prepared,
   proven, signed, submitted, accepted, and committed. MCP/Python owns spending policy and
   reservations, then rebuilds its derived ledger from Rust facts after a restart.
5. **Startup reconciliation is read-only.** It may classify an operation and report the next
   action, but it never automatically resubmits a transaction. Resubmission requires an
   explicit `resume_operation` call.
6. **Protocol 4 lands as one coordinated integration change.** The CLI, `sdk/py`, MCP server,
   fixtures, and compatibility tests move together. Every long-running MCP server must be
   restarted during the upgrade.
7. **The OpenAI Agents SDK example is a nonblocking companion.** It is useful integration
   evidence, but third-party packaging failures do not block the Erebus release.

## Operation contract

- Reusing an operation ID with the same canonical parameters returns or reconciles the
  existing operation.
- Reusing an operation ID with different parameters returns `OPERATION_CONFLICT` before any
  proof or submission work.
- The journal is stored below the identity state directory with mode `0600`, under the same
  operator trust boundary as channel state. Updates are locked, atomic, and durable.
- Before submission, Rust persists the exact signed transaction and its transaction hash.
  This closes the crash window in which the chain may accept a transaction whose local
  result was lost.
- An unreadable or contradictory journal fails closed and requires operator attention.

## Recovery modes

`resume_operation` has two distinct paths:

1. **The recorded proof is still valid.** Reconcile the recorded transaction hash and chain
   effect first. If it did not land, an explicit resume may resubmit the exact persisted
   signed transaction, preserving its hash.
2. **The recorded proof expired.** First prove that the old transaction did not land. Then
   read the pool's live proof-validity window, rebuild and re-prove from current chain state,
   re-estimate the fee, re-read the account nonce, sign a new transaction, and persist its new
   hash under the same operation ID. The re-read nonce may equal the earlier nonce when the
   earlier transaction never landed.

The compiled `DEFAULT_PROOF_VALIDITY_BLOCKS` value is not authoritative for recovery. The
write path must use the pool's live `get_proof_validity_blocks` value.

## Poulav: Rust SDK and protocol seam

Complete these tasks one at a time, with review and focused verification after each task.

1. **Foundational `OperationId` type.** Add a transparent Rust newtype, strict parsing,
   display, and validated serde. Do not connect it to requests yet.
   *Done 2026-08-23 — `d352f7e`.*
2. **Write-operation contract.** Require `OperationId` on every chain-writing Rust client
   method. Define canonical parameter binding and reject same-ID/different-parameter reuse.
   *Complete in the 2026-08-23 working tree. All six writes require a caller-supplied id;
   the binding includes the submitting account and selected wire version, and the journal
   compares the full canonical request before any RPC or proving work. An identical replay
   returns the recorded typed result instead of running the write again. The Rust library
   never generates an id. The protocol-3 CLI still creates a per-call bridge id until task 10
   makes caller-supplied, durable ids part of the public seam.*
3. **Durable Rust journal.** Add the versioned operation record, explicit lifecycle states,
   mode-`0600` locked storage, atomic replacement, and directory sync where required.
   *Done 2026-08-23 — `b784f3f`, extended by the current working tree to schema v2 and an
   identity-wide write lock. One record per id lives under `state_dir/operations`, with stage
   derived from the latest retained attempt. State and journal atomic renames both sync their
   parent directories. Records, lock files, and stored transactions are still unpruned.*
4. **Persist-before-submit execution.** Record preflight inputs, proof metadata, exact signed
   transaction bytes, and transaction hash before calling the RPC submission method. Record
   the receipt and local state commit separately.
   *Complete in the 2026-08-23 working tree. Journal schema v2 retains the canonical request,
   live prepared checks, proof anchor and validity, exact signed bytes and hash, receipt,
   planned local mutation, and final result. Bytes are synced before submission; the node's
   returned hash must match. Receipts, local state, and the committed result are persisted as
   separate crash boundaries. Channel handles are chosen and journalled before an open is
   submitted, state-file renames now sync their directory, and an identity-wide write lock
   removes the cross-operation TOCTOU window. Retention and pruning remain separate work.*
5. **Read-only startup reconciliation.** Reconcile journal entries against transaction
   receipts, relevant chain state, and local channel state. Classify ambiguous cases as
   `needs_attention`; never submit during startup.
   *Complete in the 2026-08-23 working tree. `Client::reconcile()` is read-only and compares
   durable or live receipts, the submitting account nonce, and the exact planned local channel
   mutation. Every Rust write is gated by this scan: pending, ambiguous, or locally incomplete
   older operations block a new submission. Startup never submits or repairs by itself;
   explicit CLI, Python, and MCP recovery remain task 10.*
6. **Explicit dual-mode resume.** Implement the valid-proof exact-resubmission path and the
   expired-proof rebuild path described above.
   *Complete in the 2026-08-23 working tree. A still-valid attempt resubmits the stored wire
   transaction unchanged, verifies the returned hash, waits for and persists its receipt, and
   finishes the planned local mutation and result. A reverted, nonce-dead, unsigned, or
   proof-expired attempt can rebuild only after reconciliation proves that it cannot produce
   an effect. Rust opens a retained second attempt under the same id and reissues the stored
   canonical request through the ordinary write method, which re-reads state, proof validity,
   fee, allowance, balance, nonce, and proof inputs. Replacement-attempt creation is private
   to SDK recovery. Mock-RPC tests cover both modes; live-chain evidence remains task 11.*
7. **Live prepared-stage checks.** Read live proof validity, the pool's current per-write fee,
   public STRK balance, and allowance before proving. `shield` requires the deposit plus fee;
   other proof-bearing writes require the fee. Recheck before creating a replacement proof.
   *Done 2026-08-23 — `03c058c`, taken ahead of task 6. `DEFAULT_PROOF_VALIDITY_BLOCKS` no
   longer exists on the write path; `execute` takes the live value. One correction to this
   task as written: the allowance and the public balance are not the same quantity. The
   allowance bounds what the pool pulls (fee plus deposit, exactly as specified), but the
   balance must also carry the transaction's own gas, which nothing pulls and which is not
   knowable before proving. F27 measured ~3 STRK, now reserved as `DEFAULT_GAS_RESERVE` —
   the only estimated number in the checks. Rechecking before a replacement proof is task
   6's to call.*
8. **Type-owned wide-number serialization.** Put the decimal-string invariants for
   `AllowanceReport` and `NoteBalance` on their Rust types and remove manual response
   reshaping from `erebus_cli.rs`.
   *Done 2026-08-23. Both types now own their full-width decimal-string representation and
   the CLI serializes them directly. `NoteBalance` preserves the protocol-3
   `{notes, total, pending}` shape, calculates `total` with checked arithmetic, and rejects
   inconsistent input. The wire shape did not change, so this did not bump the protocol.*
9. **Repeat-deal state shape.** Replace the ambiguous channel-level `settled` flag and
   singular settlement response with `settlements: Vec<SettlementRecord>`, keyed by deal ID.
   Each record carries acceptance, accepted offer, agreed amount, paid amount, and a nullable
   consistency result.
10. **Protocol-4 Rust seam.** Expose the operation and recovery shapes through the CLI only
    after the Rust behavior is complete. Add mismatch and response-shape tests.
11. **Fault-injection matrix.** Stop execution after each durable boundary: prepared, proven,
    signed/hash-persisted, submitted, and accepted-before-state-commit. At every point verify
    no duplicate chain effect, stable parameter binding, read-only startup, correct explicit
    resume mode, and exactly-once local outcome.

## Ishita: Python, MCP, skills, and agents

1. **Durable caller intent.** Generate operation IDs above `sdk/py`, atomically persist and
   sync the ID plus canonical intent before the MCP call, and reuse that ID after a restart.
2. **Protocol-4 Python binding.** Marshal the required operation ID and recovery results
   without adding derivation, hashing, entropy, or state-machine logic. Reject a mismatched
   CLI at startup.
3. **MCP result context.** Include backend and network in every tool result so an agent can
   distinguish mock evidence from Sepolia evidence. Tighten boundary checks such as
   `wait_for_offers` ranges and nullable settlement consistency.
4. **Rust-authoritative cap reconciliation.** Keep cap configuration and initial reservations
   in Python, but derive outcomes from the Rust journal after restart:
   - accepted or committed settlements count exactly once;
   - submitted or `needs_attention` operations keep their reservation;
   - reverted operations, or attempts proven never submitted, release it;
   - a missing Python ledger is rebuilt from Rust settlement records, including direct CLI
     settlements for the identity;
   - an unreadable Rust journal fails closed;
   - a Python-only reservation is released only after confirming that no old process still
     holds the identity lock;
   - daily UTC accounting uses the Rust journal's chain-acceptance time.
5. **Idempotent secure grant export.** Replaying the same operation must not overwrite a
   different file or leak the encrypted grant into the model transcript.
6. **Agent recovery behavior.** Teach reference agents and the Erebus skill to persist IDs,
   inspect operation status, resume explicitly, and stop for operator action on ambiguity.
   Update stale skill text to the current deal-scoped grant and wire-v3 behavior.
7. **External framework example.** Add a clean-install OpenAI Agents SDK quickstart as
   companion evidence after the stable protocol-4 MCP surface exists. Do not make its
   third-party dependency health a release gate.

## Integration order

1. Freeze the protocol-4 request, response, error, journal, and settlement-record schemas.
2. Poulav implements and fault-tests the Rust behavior behind the existing seam.
3. Ishita can work in parallel on MCP result metadata, boundary validation, and mock fixtures.
4. Land the coordinated CLI, Python, and MCP protocol-4 change together.
5. Restart all long-running MCP servers and run startup-mismatch tests.
6. Add caller intent persistence, cap reconciliation, skill behavior, and agent resume.
7. Run packaged recovery canaries and then the optional external-framework quickstart.

## Release gates

`v0.2.0` is gated on Erebus-owned behavior:

- Every chain write requires and validates a caller-supplied operation ID.
- Same-ID/same-parameters is idempotent; same-ID/different-parameters conflicts.
- The five-by-five fault-injection matrix passes.
- Startup performs reconciliation without automatic submission.
- Both valid-proof and expired-proof explicit resume paths pass.
- Spending reservations rebuild deterministically from the Rust journal.
- Protocol mismatch fails at startup, and the coordinated-upgrade runbook requires server
  restarts.
- Wide integers remain strings through Rust, CLI, Python, MCP, and JavaScript boundaries.
- Repeat-deal channel state reports a settlement list with unambiguous per-deal records.
- A clean packaged install completes a low-value Sepolia recovery canary.

The OpenAI Agents SDK quickstart ships alongside this evidence when available, but does not
block the release. Mainnet, an independent cryptographic review, and production custody
remain later gates and must not be implied by `v0.2.0`.
