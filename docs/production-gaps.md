# What remains before production

**Updated 2026-09-11.** Erebus v0.2.0 is a published, mainnet-verified technical preview. It is not ready for
material real value.

Completed protocol and canary history lives in [`status.md`](./status.md) and
[`runs/`](./runs/). This file lists current gaps only.

## Post-release operator gap

The release artifacts and public package index passed automated installation checks, and an
independent operator has since followed the published [runbook](./runbook.md) from a clean
machine. What remains is repetition: multiple external operators, across time, without
maintainer assistance.

The [targeted review](./v0.2-targeted-review.md) records completed fixes and residual risk.
It is not an independent security audit.

The [roadmap](./roadmap.md) tracks the first application.

## Custody and infrastructure

The prover and preflight RPC receive the pool private key. A hosted provider therefore sits
inside the identity's confidentiality boundary; self-hosting removes that provider but adds
node, storage, screening, uptime, and operations work.

Production needs:

- a written provider and incident policy;
- endpoint authentication, rotation, and revocation procedures;
- a supported self-hosted fallback with compatible RPC state;
- tested encrypted backup and restore tooling;
- a key-loss and state-loss drill;
- defined retention for journals, prover jobs, and evidence.

## Transaction safety

Protocol 4 has durable operation IDs and reconciliation, but production still needs:

- long-running failure tests against real provider timeouts and restarts;
- operational validation of journal pruning without losing unresolved recovery evidence;
- spending limits enforced in Rust and preserved across restarts;
- operator alerts for ambiguous operations, expired proofs, allowance drift, and RPC drift;
- a documented incident response process.

## Security review

No independent cryptographic or security review covers the Erebus wire, settlement binding,
disclosure design, hosted-prover transport, or recovery journal.

Production requires:

- internal line review of protocol-critical code;
- independent review with the exact release commit frozen;
- remediation or explicit acceptance of every finding;
- a vulnerability-reporting channel and named maintainer;
- release provenance that an operator can verify independently.

## Privacy limits

Erebus hides the terms, not the relationship.

Public or infrastructure-visible data includes:

- both counterparties at channel opening;
- the account submitting each pool action;
- timing, action shape, note count, and fees;
- public shield and unshield token legs;
- the pool key at the chosen prover and preflight RPC;
- the identity history available to the pool auditor.

Production documentation must not imply sender anonymity, traffic confidentiality, or
automatic compliance. Deposit screening is enforced by the protocol, and selective
disclosure reveals scoped information to an authorized recipient.

## Scale and operations

The current system is suitable only for bounded, low-frequency workflows. Before wider use,
measure and operate:

- provider latency, error rate, proof expiry, and retry behavior;
- RPC load and note-discovery behavior across long channels;
- pool fees and total transaction cost;
- concurrent negotiations and reservation contention;
- backup restore time and state rebuild time;
- alerting, support, and rollback procedures.

## Product gaps

The protocol does not provide delivery-versus-payment, escrow, refunds, deferred execution,
or outcome-only proofs. A scoped grant reveals a deal record; it does not prove external
delivery or expose only a single business outcome.

The [product plan](./product.md) covers agent-to-agent paid work on the existing wire.
Escrow, refunds, and outcome-only proofs require separate product and security decisions.
Standalone transfers and swaps are STRK20 platform features and are not Erebus gaps.

## Production finish line

Production readiness requires all of the following:

- repeated mainnet runs by multiple external operators;
- independent review with no unresolved critical or high finding;
- encrypted backup, restore, monitoring, and incident drills;
- durable spending policy below the agent layer;
- documented custody and provider boundaries;
- reproducible artifacts and provenance;
- public documentation matching the reviewed behavior;
- a named team responsible for security reports and release support.
