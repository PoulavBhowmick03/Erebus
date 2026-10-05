# Metropolis Roadmap

Working branch: `metropolis`. Target submission date: October 13, 2026 (verify the portal cutoff
and timezone before submission). Status details, evidence, decisions, and acceptance gates live
in [Metropolis Status](metropolis-status.md); this file is the concise plan.

## Product and release target

A developer/agent product on Monad testnet that an external developer or marketplace can adopt:
integrate agents, discover services, negotiate privately, settle, disclose one deal, and recover
failures. The product surface is SDK, CLI, MCP tools, packages, and operational documentation;
a frontend is not a release requirement. An internal demonstration alone does not satisfy the
final product gate.

Public-bound payment is an intermediate milestone, not a substitute for shielded settlement.
Payment finality and service delivery are separate states. Mainnet activation is a separate
release decision.

## Operating model

| Component | Default operator | What it exposes |
|---|---|---|
| Agent policies and negotiation | Developer or marketplace | Its own terms, budgets, counterparty permissions |
| Keys, payment authorization, local prover | Developer or marketplace | Private keys and witnesses stay in its environment |
| Encrypted-message relay | Optional or self-hosted | Ciphertext, routing metadata, sizes, timing |
| Transaction relayer | Optional or self-hosted | Public proofs, transaction inputs, submission metadata |
| Pool indexer | Optional or self-hosted | Public chain events and query patterns |
| Settlement contracts | Monad testnet | Settlement rules under a documented admin policy |

Hosted and self-hosted deployments run the same implementations. Hosted services must not require
spending keys, plaintext negotiation, or plaintext witnesses. Self-custody does not hide network
metadata or guarantee availability. See [operations](metropolis-operations.md).

## Milestones

| Milestone | Scope | Status |
|---|---|---|
| M0 Baseline | Branch discipline, CI isolation, decision records, boundary map | Locally complete |
| M1 Canonical agreement | Chain-neutral encoding, commitments, authorizations, policy | Locally complete |
| M2 Private negotiation | Noise transport, transcript store, relay, discovery | Locally complete |
| M3 EVM public-bound settlement | Contract, adapter, signing, submission, receipts | Locally complete |
| M4 Shielded mechanism | Pool integration, proof system, witness/public-input schemas, prototype | Locally complete (prototype) |
| M5 Shielded contracts and prover | Deposit, private transfer, withdrawal, local wallet, indexer | Funded local prototype; incomplete |
| M6 Coordinator, relayer, recovery | Durable intent, policy reservations, reconciliation, one-payment fence | Locally complete; owner review pending |
| M7 Scoped disclosure | Recipient-bound grants, independent verification, backup/restore | Locally complete |
| M8 Monad and agents | Deployment, packages, x402, access, agent integration, live workflow | Partial; external acceptance open |
| M9 Hardening and submission | Adversarial tests, external integration, demo, self-hosting rehearsal | Pending |

M8's Done criterion is independent buyer, seller, observer, and disclosure processes completing
the real Monad workflow, with hosted and self-hosted support and recoverable access issuance.
The public-bound and x402 rails have completed team-operated live runs; hosted operation,
independent external acceptance, and the demo video remain open. M5 remains incomplete.

## M9 acceptance gates

- Run the threat-model attack matrix through direct contracts and SDK calls.
- Rehearse from a fresh environment and archive sanitized, reproducible evidence.
- Have an external developer integrate their agents using only versioned packages and public
  documentation, without undocumented team intervention.
- In that independent run: discovery, settlement, scoped disclosure, interrupted-operation
  recovery, and (for the shielded release) local proof generation and withdrawal.
- Rehearse self-hosting, backup restoration, and endpoint switching from a fresh environment.
- Record a three-minute demo with real timings and clearly labelled edits.
- Verify portal requirements, deadline timezone, and sponsor criteria; prepare the submission
  package and document remaining limitations.

Every submission claim must point to code, a reproducible test, or a dated chain artifact. If no
external developer completes the run, record the gate as incomplete even when internal
demonstrations pass.

## After Metropolis

- Qualify the generic EVM backend on another chain with independent deployment and finality tests.
- Improve Starknet agreement enforcement without claiming parity from an adapter wrapper.
- Add further privacy mechanisms behind explicit capabilities and trust assumptions.
- Evaluate cross-chain settlement, escrow, and delivery guarantees as separate extensions.
- Obtain external security review before a production-value launch.
