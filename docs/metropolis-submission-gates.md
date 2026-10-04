# Metropolis Submission Gates

## Rubric

The organizer rubric supplied by the team replaces the earlier fallback scoring:
Product Quality and Completeness, Technical Excellence, Monad Integration,
Track Fit and Problem Relevance, and Innovation and Impact each weigh 20%.
Track 04 targets infrastructure that other applications build on.
Keep the official rules document with the final submission; this record is not a rules source.

The supplied deadline is October 13, 2026, 23:59 ET. Treat these as mandatory gates,
not optional presentation improvements:

- [ ] A demo video of at most three minutes shows the product operating and a Monad interaction.
- [ ] A third party runs the submitted path from the README, without team-held fixtures.
- [x] README distinguishes prior Starknet work from new Metropolis work.
- [x] README discloses AI-assisted development.
- [ ] Every release and privacy claim matches reproducible evidence.

## Claims Boundary

Public-bound Monad transactions prove payment execution, not hidden amounts or identities.
The full accepted agreement, blinding, and bilateral signatures are public on that path.
The two-agent MCP loop and shielded proving currently have local evidence only.
Prototype proving artifacts are test-only. Historical live runs do not prove a later fix.
An encrypted grant does not alone prove payment; independent finalized chain evidence is required.
Resource hash verification does not prove independent payment or a delivery audit.

## Still Required

- A new Monad negotiation/payment/access run with isolated buyer and seller secrets.
  Local Anvil evidence exists for public-bound and for x402 exact; no live x402 broadcast is
  authorized by the current records.
- Independent auditor verification with participant state unavailable. Local evidence now
  covers both rails (public-bound grant verification and the x402 finalized-calldata path);
  the live Monad counterpart is not run.
- An external fresh-environment rehearsal. The package channel now exists: `0.3.0.dev4` is
  published at `https://poulavbhowmick03.github.io/erebus-metropolis/simple/` and a team-operated
  public install was verified on macOS arm64 (2026-10-04). The text below records how we got there.
- Previously: a published Metropolis package channel and an external fresh-environment rehearsal. The
  release registry builds and passes isolated-install checks locally; a team-operated
  installed-package rehearsal (negotiation, one payment, recovery, access, disclosure) passed
  on macOS arm64. Nothing is published, and no third party has run it. Linux x86_64 remains
  CI-only and unexecuted locally.
- A recorded Monad demo, linked from the README and submission.
- Inclusion and finality observation timings, not differences between block timestamps.
  Local monotonic stage timings now exist for public-bound and x402; live Monad timings do not.

Do not mark these complete based on a local test, a document, or a package build.
