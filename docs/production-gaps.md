# What remains before production

**Updated 2026-08-31.** Erebus is a mainnet-verified technical preview. It is not ready for
material real value.

Completed protocol and canary history lives in [`status.md`](./status.md) and
[`runs/`](./runs/). This file lists current gaps only.

## Release-candidate gaps

These block a reviewed `v0.2.0` publication, not the already completed mainnet canary:

- one current end-to-end operator guide tested from a clean shell;
- one external clean-install canary from release-candidate artifacts;
- targeted review of hosted proving, transaction recovery, and secret boundaries;
- a replacement public video showing the complete mainnet workflow;
- final public-link, hub, artifact, and secret verification;
- explicit owner authorization to publish.

See [`v0.2-release-plan.md`](./v0.2-release-plan.md).

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
- bounded journal retention without losing recovery evidence;
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

Do not add those features to the sprint release. Each requires a separate product and
security decision after the current operator path is reproducible.

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
