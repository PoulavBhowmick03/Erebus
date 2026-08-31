# Erebus roadmap

**Current planning window:** September 1–7, 2026. The sprint deadline moved to Monday,
September 7.

This file contains unfinished work only. Completed work belongs in
[`status.md`](./status.md) and dated records under [`runs/`](./runs/). Release-specific work
belongs in [`v0.2-release-plan.md`](./v0.2-release-plan.md).

## 1. Objective for September 7

Present Erebus as a reproducible, mainnet-verified technical preview:

- two agents negotiate through the role-bound MCP servers;
- the payer settles from a shielded STRK20 note;
- the payment and change conserve value;
- interrupted writes reconcile without blind retries;
- a public observer cannot recover the terms;
- an authorized recipient can reconstruct one scoped deal;
- another operator can install and understand the system without this checkout.

The release claim remains: **Erebus hides the terms, not the relationship.** Channel-open
counterparties, submitting accounts, timing, action shape, note count, public deposits, and
fees remain visible.

## 2. Deadline priorities

### P0 — reconcile the second mainnet canary

The committed file `docs/runs/2026-08-31-mainnet-060-040-canary.md` describes a second
mainnet workflow. Do not add it to the manifest or public claims until it passes these
independent checks:

- verify every receipt independently against a mainnet RPC;
- confirm the shield, proposal, counter, and settlement touched the canonical pool;
- confirm payment/change conservation and final note balances;
- confirm `reconcile` reports committed effects with no pending action;
- confirm the observer and deal-scoped disclosure results;
- scan the record for secrets and protected paths;
- confirm the recorded source and artifact provenance.

If it passes, commit the run record, add only its four pool transactions to `strk20.json`,
and update `status.md`, the public demo, and the video script. ERC-20 allowance transactions
do not belong in the sprint manifest.

Exit: the repository contains either a verified second-canary record or a written reason for
excluding it.

### P0 — write one end-to-end operator guide

Rewrite [`runbook.md`](./runbook.md) as the single current path from an empty machine to a
verified Erebus deal. It must cover:

1. supported platforms and release-candidate installation;
2. two isolated identities and protected state directories;
3. RPC, Starkscan hosted-prover, chain, pool, and token configuration;
4. registration, allowance, shielding, maturity, and directional channels;
5. `doctor` and `reconcile` before every write sequence;
6. buyer and seller MCP startup with role boundaries;
7. proposal, counter, atomic settlement, payment/change verification;
8. observer inspection and recipient-bound selective disclosure;
9. interruption recovery with the original durable operation ID;
10. shutdown, secret handling, evidence capture, and cleanup;
11. local-prover fallback and its screening limitation;
12. a clear private-versus-public table for every stage.

All commands must be tested from a clean shell. Use placeholders for credentials and keys;
never print an env file, prover request/result, viewing grant, account key, or pool key.

Exit: an operator can follow the document without relying on chat history or a maintainer's
local files.

### P0 — external clean-install canary

Give release-candidate artifacts and the end-to-end guide to someone outside the original
implementation path.

- Install without this checkout or a Rust toolchain.
- Confirm `erebus-cli` reports `0.2.0` and Protocol 4.
- Confirm the MCP server exposes exactly thirteen tools.
- Run mock and read-only `doctor`/`reconcile` checks.
- Record every unclear instruction or hidden prerequisite.
- Run another funded mainnet write only with explicit authorization and a bounded fee plan.

Exit: a dated, secret-free report records the environment, artifacts, commands, failures,
and result.

### P1 — targeted release review

Review the code that can lose funds, repeat a write, or expose a pool key:

- `sdk/rs/src/prover.rs`: Starkscan authentication, submit/poll recovery, durable one-time
  result storage, file modes, and redaction;
- `sdk/rs/src/execution.rs`: proof expiry, exact-request pinning, submit recovery, and
  idempotency;
- operation journal and reconciliation: ambiguous outcomes, reservations, and restart
  behavior;
- Python/CLI/MCP boundaries: operation IDs, error names, amounts-as-strings, and secret-safe
  output;
- release automation: version parity, artifacts, checksums, SBOM, and secret scanning.

Exit: every finding has a concrete failure sequence and is fixed, explicitly accepted, or
recorded as a release blocker.

### P1 — replace the sprint video

The current video predates the complete mainnet workflow. Record a new public video of no
more than three minutes showing:

- the agent-to-MCP-to-Rust-to-prover/RPC architecture;
- a screened mainnet shield;
- MCP proposal and counter;
- the atomic settlement receipt;
- payment and change conservation;
- recovery after interruption;
- public observer output versus authorized disclosure;
- the exact privacy and trust limits.

Do not show credentials, keys, protected paths, complete prover payloads, grants, or terminal
history containing secrets.

Exit: the public URL works without authentication and `strk20.json` points to it.

### P1 — evaluate new StarkWare tooling

When the announced tooling arrives:

- record its version, source, supported network, and trust boundary;
- compare it with the current Starkscan prover and RPC split;
- test it in isolation before changing the release candidate;
- adopt it only if it removes a current failure or materially improves reproducibility;
- keep the known-good path available until the replacement passes the same gates.

Exit: the decision and evidence are recorded. New tooling does not block the existing path.

### P1 — final public verification

- Run Rust, Python, TypeScript, dependency, skill, demo, and secret gates.
- Verify every manifest transaction succeeded and touched the canonical pool.
- Confirm the demo, video, repository, evidence, and explorer links open publicly.
- Confirm the sprint hub reads the latest commit and reports mainnet, demo, and video.
- Freeze code and documentation by September 6; reserve September 7 for fixes.

Exit: the final commit is green, public, secret-free, and represented accurately by the hub.

## 3. `v0.2.0` decision

Mainnet execution is no longer the technical blocker. Publication still requires the
external clean-install canary, end-to-end guide, targeted review, final artifact checks, and
explicit owner authorization.

If approved, follow [`v0.2-release-plan.md`](./v0.2-release-plan.md). The release must say
**mainnet-verified, experimental, and unaudited**. It must not say production-ready.

## 4. Privacy and trust boundary

[`privacy-model.md`](./privacy-model.md) is canonical. Planning must preserve these limits:

| Hidden from a public chain reader | Public or trusted-infrastructure-visible |
| --- | --- |
| Offer and counter terms | Submitting Starknet account |
| Private payment and change amounts | Pool interaction timing and note count |
| Private-note ownership and spent-note identity | Public shield/unshield legs and fees |
| Deal contents without a scoped grant | Counterparty address at channel opening |
| Unrelated deals when one scoped grant is revealed | Pool key at the prover and preflight RPC |

Deposit screening is enforced by the protocol. A self-hosted prover is not a screening
workaround. Selective disclosure is scoped access, not automatic compliance.

## 5. Deferred until after the sprint

Do not start these before the P0 and P1 work is complete:

- Deal Room or another application surface;
- private swaps, bridges, generic transfers, or new token selection APIs;
- outcome-only platform receipts;
- paymaster or submission-unlinkability experiments;
- encrypted backup and restore commands;
- production monitoring and incident automation;
- new wire formats or privacy claims;
- `v1.0` production-readiness work.

## 6. Post-sprint product gates

Before material real-value use, Erebus still needs:

- independent cryptographic and security review;
- encrypted backup, restore, and key-loss drills;
- secret-safe monitoring and incident response;
- spending limits enforced below the agent layer;
- multiple external operators;
- a named security and release maintainer;
- measured reliability and cost across repeated mainnet runs.

## 7. Sources

- Current truth: [`status.md`](./status.md)
- Mainnet evidence: [`runs/2026-08-31-mainnet-starkscan-workflow.md`](./runs/2026-08-31-mainnet-starkscan-workflow.md)
- Release work: [`v0.2-release-plan.md`](./v0.2-release-plan.md)
- Production boundary: [`production-gaps.md`](./production-gaps.md)
- Privacy boundary: [`privacy-model.md`](./privacy-model.md)
- STRK20 actions and proofs: https://strk20-by-example.org/actions-and-proofs
- STRK20 screening and disclosure: https://strk20-by-example.org/compliance
