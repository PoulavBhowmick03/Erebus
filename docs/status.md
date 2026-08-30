# Status

**As of 2026-08-30.** One page, current, and the tiebreaker: where any other document in
this repository disagrees with this one, this one is right and the other is stale.

Nine documents describe this system and they were written across three weeks in which the
privacy claim changed twice. That is why this page exists.

---

## In one line

Erebus negotiates and settles privately between two agents on Starknet Sepolia. On mainnet,
A and B are registered and have opened channels in both directions; a shielded settlement
has not completed.

---

## What is true right now

| | |
|---|---|
| Network | Full workflow: Sepolia. Mainnet: A and B registered, then opened directional channels in blocks `14100846` and `14101246`; no shield, offer, settlement, or disclosure yet |
| Wire | Source default: v3 — framed messages, authenticated deal IDs, and three masked spare bits. Persisted v1/v2 reads remain supported |
| Live evidence | `0xc897e94b…92cb` (2026-08-22): BuyerPolicy/SellerPolicy negotiating
  autonomously over MCP on the seam backend at wire v3, settled atomically, and disclosed to
  an independent third party — definition-of-done 1, 3 and 4 at v3
  (`docs/runs/2026-08-22-agents-mcp-wire-v3.md`). Plus: `0x14b38e9d…4cb3` (wire v2, 2026-08-07),
  `0x4191fe47…f341` (merged code with change + disclosure + observer, 2026-08-19), and
  five wire-v3 settlements on 2026-08-22 through one channel pair with deal-scoped
  disclosure, including `0x60eace8b…a7be` at 19 STRK to clear the u64 boundary F39/F40 named
  and two at an identical 0.25 STRK price to demonstrate repeat deals
  (`docs/runs/2026-08-22-sepolia-wire-v3-run.md`). Protocol 4 packaged-source recovery also
  completed on 2026-08-27: exact resubmission `0x53e10185…4f55` and expired-proof rebuild
  `0x611b8250…987e` (`docs/runs/2026-08-27-packaged-recovery-canary.md`). Mainnet registration
  `0x6597adb6…e54c` succeeded on 2026-08-28 through a local RC.2 prover and Alchemy v0.10
  (`docs/runs/2026-08-28-mainnet-registration.md`). B registration `0x572260b6…7189`
  succeeded on 2026-08-29, followed by a proof-only shield probe that confirmed the local
  prover returns no required screening signature (`docs/runs/2026-08-29-mainnet-preflight.md`).
  Mainnet channel opens `0x395563b3…a09a` and `0x467295d1…1964` succeeded on 2026-08-30;
  both reconciled cleanly after proving depth (`docs/runs/2026-08-30-mainnet-channels.md`) |
| Version | Source manifests still say `0.1.0`, but `main` speaks Protocol 4. The published `v0.1.0` artifacts speak Protocol 2. Protocol 4 ships with `v0.2.0` after its release gates pass |
| Tests | 351 Rust passed (plus 7 intentionally ignored live tests), 156 Python, 43 TypeScript |
| In flight | Mainnet Accounts A and B are deployed, registered, and connected by one channel in each direction. Canonical-pool shielding still needs StarkWare screening access: the live screener key is non-zero, and the published interceptor needs operator-issued `/screen` partner credentials. |
| CI | green on every push: Rust, Python, secret scan, dependency hashes |
| Install | Published `v0.1.0`: Protocol 2 with ten MCP tools. Current source: Protocol 4 with thirteen tools. Linux x86-64 and macOS arm64 are supported. Intel macOS is unsupported. No Protocol 4 wheel is published yet |

## What Erebus does

Two agents open a channel, exchange offers and counters as encrypted state, and settle
atomically in one action set. Wire v3 can encrypt one deal's read capabilities to a
registered recipient with an explicit expiry. Historical wire-v1/v2 channels keep their
broader bearer-grant format.

Agents drive it through MCP, so any framework in any language can use it. No contract of our
own is deployed: the negotiation rides in note salts the pool already provides.

## What Erebus does not do

- **Hide who you are dealing with.** The counterparty's address is written in public
  calldata at channel-open. This is upstream of our encryption and no wire change fixes it.
  See F38 and [privacy-model.md](./privacy-model.md).
- **Hide that a negotiation happened.** Wire v3 removes the fixed v2 salt classifier, but
  the submitting account, transaction timing, action shape, and note count remain public.
- **Run the full workflow on mainnet.** Two registrations and two channel opens are proven.
  Shielding, offers, settlement, recovery, and disclosure are not.
- **Revoke facts already disclosed.** A wire-v3 expiry stops a later verification, but it
  cannot make a recipient forget a record opened before expiry.
- **Escrow, or deferred delivery.** Settlement is atomic, so there is no "agree now, deliver
  later". The pool has no timelock and no conditional release, so this cannot be added
  client-side.

## The honest privacy claim

> Erebus hides the terms, not the relationship.

Content confidentiality is demonstrated: an observer script with no key recovers the full
terms from wire v1 and nothing from wire v2. Traffic confidentiality is not, and the four
known leaks are listed in severity order in [privacy-model.md](./privacy-model.md).

Never describe this as private in an absolute sense.

---

## Which document to trust for what

| Question | Read | Confidence |
|---|---|---|
| What leaks, and what does not | [privacy-model.md](./privacy-model.md) | current, canonical |
| What fought us, and how | [friction.md](./friction.md) | current, 38 entries |
| What to do next | [roadmap.md](./roadmap.md) | current after the 2026-08-28 reconciliation |
| How to reproduce a run | [runbook.md](./runbook.md) | current for Protocol 4, with historical receipt tables |
| Historical source walkthrough | [tech.md](../tech.md) | historical snapshot; wire-v3 sections are stale |
| Does this fit my use case | [usecases.md](./usecases.md) | current after the 2026-08-28 reconciliation |
| What is missing for production | [production-gaps.md](./production-gaps.md) | current summary with a preserved historical baseline |
| Key custody reasoning | [custody-design.md](./custody-design.md) | current as a decision record |
| What a lost key or state directory costs | [custody-operations.md](./custody-operations.md) | current; behaviour only, no tooling |
| The pitch | [poc.md](./poc.md) | current |
| How to operate it as an agent | [skills/erebus/SKILL.md](../skills/erebus/SKILL.md) | current, all nine unsafe-behavior evals pass (`skills/erebus/evals/results-2026-08-26.md`) |

### Historical documents and known limits

These documents preserve dated evidence and do not describe the current source:

- **`tech.md`** describes the 2026-08-05 tree. Its banner names the obsolete areas.
- **`poc.md`** preserves the original wire-v2 design record. Its banner points to the
  current wire-v3 evidence.
- **`docs/runs/`** records exact past configurations and protocol versions. Do not update a
  past run to look like a current run.
- **`scripts/observer.py`** classifies wire-v1 traffic as wire v2. The recovery results are
  unaffected — v1 content is recovered, v2 is not — but the version label it prints is not
  trustworthy.

---

## What ships, and what does not

**Ships:** `/sdk/rs` is the implementation and the only holder of key material. `/sdk/py` is
a binding with no protocol logic. `/mcp-server` exposes the tools. `/agents` are reference
agents.

**Does not ship:** `/sdk/ts` is the differential-test oracle and exists so two independent
implementations can be checked against the same vectors. It implements historical wire v1
and the wire-v3 cryptographic/bit-level oracle.
`/contracts` holds throwaway probes and is nearly empty, which is correct.

## Where the keys go

Three distinct keys, and conflating them is the usual mistake:

- **Starknet account key** — signs transactions. Custody. Never leaves the Rust process,
  and behind an `AccountSigner` it need never enter it: a hardware, wallet, or session
  signer produces the signature without the SDK holding a key at all.
- **Pool private key** — the STRK20 identity. Confidentiality. Sent in `compile_actions`
  calldata to two operator-chosen endpoints: the prover and the preflight RPC. Both can
  reconstruct that identity's full history. The submitted transaction does not carry it.
- **Pool auditor key** — pool-wide, StarkWare's, set once at registration, no rotation.

Python never sees account, pool, parent-channel, or native deal keys in plaintext. The MCP
server carries the encrypted grant long enough to write a new mode-`0600` file, then returns
only metadata and the path. The capsule does not enter the model transcript.

---

## The next work

1. ~~Grant the Sepolia allowance, then do one full run on merged code.~~ Done 2026-08-19:
   `0x4191fe47…f341`, with change, a third-party disclosure, and observer output. It found
   and fixed a read-wedging bug on the way; see `docs/runs/2026-08-19-sepolia-run.md`.
2. ~~Move `server.py` into the package with an entry point.~~ Done 2026-08-19. The
   `v0.1.0` tag publishes the wheels and the index.
3. ~~Finish the protocol-4 product seam.~~ Done 2026-08-26: every chain write takes a
   caller-supplied operation ID through MCP, Python, CLI, and Rust; channel state returns a
   settlement list; protocol mismatches fail by name.
4. ~~Preserve recovery error names through MCP.~~ Done 2026-08-27. The seam preserves all
   four Protocol 4 recovery and funding codes.
5. ~~Finish spending reservation reconciliation.~~ Done 2026-08-27. Reservations are
   atomic and fail closed. Rust reconciliation owns outcomes, and committed daily spend
   uses Starknet block timestamps.
6. ~~Run the packaged Sepolia recovery canary.~~ Done 2026-08-27 from a clean local wheel
   install. Exact resubmission and expired-proof rebuild both completed. See
   [the run record](./runs/2026-08-27-packaged-recovery-canary.md).
7. **Publish Protocol 4 as `v0.2.0`.** Update source package versions, build the supported
   wheel set, publish checksums and an SBOM, then run the installed-artifact canary.
