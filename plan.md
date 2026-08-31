# Erebus September 7 plan

This is the execution calendar. [`docs/roadmap.md`](./docs/roadmap.md) defines scope and
acceptance checks. [`docs/v0.2-release-plan.md`](./docs/v0.2-release-plan.md) defines the
release gate. Completed work is recorded in [`docs/status.md`](./docs/status.md) and
[`docs/runs/`](./docs/runs/).

## September 1 — evidence integrity

- Independently verify and integrate, or explicitly exclude,
  `docs/runs/2026-08-31-mainnet-060-040-canary.md`.
- Add only independently verified canonical-pool transactions to `strk20.json`.
- Align the status page, public demo, and video script with the selected evidence.

## September 2 — end-to-end documentation

- Rewrite `docs/runbook.md` as the single clean-machine operator guide.
- Cover installation, protected configuration, two identities, hosted proving, readiness,
  shielding, channels, MCP negotiation, settlement, recovery, observer inspection,
  disclosure, and shutdown.
- Test every command from a clean shell without printing secrets.

## September 3 — external operator

- Hand release-candidate artifacts and the guide to an operator outside the implementation
  path.
- Confirm `0.2.0`, Protocol 4, thirteen MCP tools, mock operation, `doctor`, and `reconcile`.
- Record and fix hidden prerequisites or unclear steps.

## September 4 — review and new tooling

- Review hosted-prover recovery, exact-request idempotency, operation reconciliation, and
  secret redaction.
- Evaluate the announced StarkWare tooling in isolation.
- Keep the known-good Starkscan path unless the new path passes the same checks.

## September 5 — reviewer demonstration

- Record a new three-minute video using the complete mainnet workflow.
- Show screened shielding, MCP proposal/counter, atomic settlement, conservation, recovery,
  observer output, and scoped disclosure.
- Deploy the updated public demo and verify every link without authentication.

## September 6 — release decision and freeze

- Run all source, artifact, dependency, skill, demo, and secret gates.
- Request explicit owner approval for `v0.2.0`.
- If approved, follow the publish sequence in `docs/v0.2-release-plan.md` and verify fresh
  Linux and macOS installs from the public package index.
- Freeze code and documentation after the final green commit.

## September 7 — submission buffer

- Confirm the sprint hub reads the final commit, mainnet transactions, demo, and video.
- Recheck public URLs, transaction receipts, package artifacts, and privacy wording.
- Make fixes only. Do not add protocol or product features.

## Scope guard

Until the submission is frozen, do not start Deal Room, swaps, bridges, new wire formats,
paymaster work, or production operations. Mainnet writes require explicit authorization and
a bounded fee plan.
