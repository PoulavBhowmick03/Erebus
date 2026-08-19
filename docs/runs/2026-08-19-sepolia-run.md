# Sepolia run — 2026-08-19

P0 item: "One full Sepolia run on merged code, with a receipt, observer output, and a
disclosure." Driven through the live MCP servers configured in a Claude Code session
(erebus-buyer = payer on `~/.erebus-g`, erebus-seller = payee on `~/.erebus-f`), so this
run also exercises the MCP path end to end. Main at `4791fc5` plus one uncommitted
roadmap edit (docs only).

## Timeline

- **doctor, both identities** — 10/10 pass on both. Buyer allowance 94 STRK (~47 writes),
  seller 96 STRK (~48). Buyer gas 78.26 STRK, seller 13.16 STRK. Head ~13702096, pool
  v2.0 live, prover spec 0.10.3-rc.2. The roadmap P0 note "allowance is the only failing
  check" is stale for these identities: it described the repo `.env` identity, not these.
- **note balances** — buyer(g) 0 spendable; seller(f) 2×1 STRK. A payer with no notes
  cannot settle, so a shield is required first (operator step per D13, not an agent tool).
- **terminal-pair check** — both state dirs hold one channel each, dated 2026-08-07, and
  they name each other as counterparty: g↔f is the settled pair from the wire-v2 reference
  run. One channel per pair, one deal per channel, so this pair cannot run again.
- **plan** — new payee identity X funded by STRK transfer from g (no faucet dependency);
  g is payer over the live MCP server; X driven via erebus-cli; f granted the viewing key
  as an independent third party for the disclosure leg. Prices: X asks 3 STRK, g counters
  2.5, X finals 2.75, g accepts — settlement pays 2.75 from a 5 STRK shielded note,
  exercising the change path live (2.25 back).
- **h discovered provisioned** — `~/.erebus-h` existed from 2026-08-01 with keys, env,
  empty state, ~7.9 STRK, and `doctor` says already registered; only allowance failed.
  Cast: payee = h (`0x17cfc3a6…ec1d`), payer = g, disclosure recipient = f.
- **funding + approve** —
  g→h 12 STRK: `0x02bd71499aa6dda085ae816fb3bb40c7daa745a79f5b7e2238230b3958d5c6ca`
  h approve 20 STRK to pool: `0x002549c16f92cbbc0fb4252aaaf3a47b886ac0a262af6925f242928825c4b532`
- **g shield 5 STRK** — `0xde8f3a300ef182aa53aee1e229ba9c10eda1e33c9253488c2495d87f882f65`,
  proved at block 13702298. Approve and shield both past proving depth (head−10) before
  the next write. Memo committed in the offers: sha256 of the memo text is
  `3057f771463d492fcb2adc045842950b1c7d05e73d64d73c21438174cd1b55ea`, so
  `memo_hash = 0x1c7d05e73d64d73c21438174cd1b55ea` (low 128 bits, the wire rule).
- **channel open, both directions** — g→h over the live MCP server, h→g over erebus-cli.
  Per F38 both writes put the counterparty address in public calldata; this run is
  evidence of that, not an exception to it.
- **h ask 3 STRK** — offer `ch_0b5bf482…:us:0`, memo_hash `0x1c7d05e73d64d73c21438174cd1b55ea`
  (low 128 bits of the memo's sha256; first live offer to carry a full-width digest tail).

## The bug this run found (fixed mid-run)

The first read after h's ask failed from every layer with
`request is not valid JSON: number out of range`. Root cause: `OfferTerms` serialized
`amount` and `memo_hash` as bare JSON numbers, and serde_json refuses integers above
`u64::MAX`, so the first offer with a memo_hash wider than 64 bits made **every read of
its channel fail permanently** — read_channel_state, wait_for_offers, and
accept_and_settle — while the note sat immutable on-chain. `d1731f4` had made all 128
bits reachable on the input side; the output side broke at bit 65. `amount` had the same
latent ceiling at ~18.4 STRK. The error label compounded it: `serialize()` in
`erebus_cli.rs` wrapped output-serialization failures in `BadRequest`, pointing the
diagnosis at the blameless request parser.

Fix (uncommitted, working tree): `u128_boundary` serde module in `client.rs` — amount as
decimal string, memo_hash as 0x-hex, deserialization tolerant of legacy numbers; new
`BadResponse` CLI error (code `INTERNAL`); `seam_client.py` `_terms` converts both back
to the frozen interface's ints; two new KATs (`full_width_terms_cross_the_json_boundary_
as_strings`, `legacy_numeric_terms_still_deserialize`). Full Rust suite + clippy green,
26 Python seam tests green. The wedged channel read back intact after the fix — no data
loss, the chain state was never wrong, only unserializable.

Deserves an F39 entry (Poulav to write). Two side-findings for it: reading a nonexistent
handle leaves a `.lock` file for it in the state dir; and the long-running MCP server
processes needed a reconnect to load the seam fix, so mid-run the MCP write path (whose
responses carry no terms) kept working while MCP reads failed — writes went via MCP,
reads via the CLI, and the MCP read leg should be re-verified after reconnect.

## Negotiation and settlement

- g counter 2.5 STRK via MCP: `ch_620b53e1…:us:0`, reply_to h's ask.
- h final 2.75 STRK via CLI: `ch_0b5bf482…:us:1`.
- g accept_and_settle on `…:them:1` — **settlement tx
  `0x4191fe47a0b062605a7bbc08dd40eafdefcd52de4fd0288e8315eb48ee2f341`**, proved at block
  13703774, nullifier `0x340a2f69…b346d`, `selected_input` 5 STRK, `paid` 2.75,
  `change` 2.25. Conservation: 5 = 2.75 + 2.25.
- Post-settlement notes: g `[2.25]`, h `[2.75, 1.0]` (the 1.0 is h's 2026-08-01
  provisioning shield, which is also when h registered). Conserved.

## Disclosure

g granted a viewing key scoped to the channel to f (`0x34f5aff…8299`), an identity that
took no part in the deal. f reconstructed the complete record from chain data alone: all
four offers in both directions with statuses, amounts, and memo hashes, plus the
settlement (`agreed_amount` = `paid_amount` = 2.75 STRK). DoD item 3 demonstrated.

Handling error, logged honestly: a botched shell command printed the bearer grant into
the driving session's transcript. The grant reads this one channel and cannot spend.
Treat this channel's record as public — which this document makes it anyway.

## Observer

`scripts/observer.py` against the settlement tx, no keys:
- **Content: not recovered.** The public wire-v1 decoder found no plausible transcript
  in 61 felts of calldata / 6 candidate salts.
- **Traffic: classified.** One salt with bit 119 set and bits 60..118 zero — the F31
  fingerprint, false-positive odds 2^-59. Exactly the documented boundary: terms hidden,
  presence visible. (The observer's version label was correct here, v2 traffic labeled
  v2 — the stale-claim note in status.md concerns wire-v1 inputs.)

## P0 checklist against this run

- [x] One full Sepolia run on merged code + this working tree, receipt, observer output,
      disclosure — this file.
- [ ] Video, strk20.json manifest — not this run's scope.

Identity roles this run: payer g `0x23ad2a76…b948` (MCP buyer), payee h `0x17cfc3a6…ec1d`
(CLI), disclosure recipient f `0x34f5aff…8299`. Channel pair g↔h now terminal (one deal
per channel).

## Readiness loop, same day (uncommitted work, by owner)

Five blockers from the run's readiness assessment, all addressed in the working tree:

**Poulav's to review** (`sdk/rs`, `sdk/py`, `scripts`):
- `client.rs`: `u128_boundary` serde module — amount/memo_hash cross the CLI boundary as
  strings; two KATs pin it (`full_width_terms…`, `legacy_numeric_terms…`).
- `erebus_cli.rs`: `BadResponse` error (code `INTERNAL`) so output failures stop wearing
  the request's label; `protocol: 2` on every envelope.
- `execution.rs`: stage lines on stderr during `execute` (simulate/prove/estimate/submit),
  outside the one-envelope stdout contract; `submit_call` stays silent.
- `state.rs`: `lock()` checks existence before creating the lock file; test pins that an
  unknown handle leaves no trace.
- `_seam.py`: `PROTOCOL = 2`, per-envelope mismatch check raising `SeamUnavailable` by
  name; exported from the package root.
- `new-identity.sh`: approve reads the live fee (was hardcoded 1 STRK vs the current
  2 STRK fee — a provisioned identity failed its first write); ends with `doctor`;
  runbook §1 and README now point at it.

**Ishita's to review** (`mcp-server`):
- `server.py` moved into `erebus_mcp` with `build_server()`/`main()` and a
  `[project.scripts]` entry point; shim kept at the old path for `mcp dev` and existing
  stdio configs. Done on Poulav's instruction ahead of coordination — review before it
  lands.
- Startup: seam-protocol handshake (fail by name) and `doctor` logged at boot
  (`EREBUS_SKIP_STARTUP_DOCTOR=1` to skip). `config.py` gains `startup_doctor`.
- `seam_client._terms`: decodes string amounts/hex memo_hash from the fixed CLI, tolerant
  of pre-fix numeric payloads.
- `tools.py`: the four write tools now state 1–4 minutes of expected silence and warn
  against abort-and-retry.
- `canary.yml`: launches the server by the installed console script — nothing from the
  checkout — so the canary now proves what `uvx erebus-mcp-server` gets.

Verified: cargo test (22 suites) + clippy `-D warnings` + fmt clean; 70 Python tests
green; local canary — three wheels into an empty venv, `erebus-mcp-server` served 10
tools and answered `get_note_balance` with no repo on any path.

## MCP read path, re-verified

The run left one leg unproven: the session's long-running MCP servers predated the seam
fix, so writes went over MCP while reads went through the CLI. Closed the same day.

A freshly started `erebus_mcp.server` (seam backend, identity g, real MCP client over
stdio) against the live settled channel `ch_620b53e1...`:

- `doctor` — returns its ten checks through the transport.
- `get_note_balance` — `2250000000000000000`, the change note from this run's settlement.
- `read_channel_state` — all four offers, both directions, amounts as decimal strings and
  `memo_hash` as `0x1c7d05e73d64d73c21438174cd1b55ea`: the full-width digest tail that
  wedged this channel now survives Rust → CLI → seam → MCP intact.
- `wait_for_offers` — same shape, returns immediately at `expected_count: 4`.

The session's pre-fix servers still fail on the same calls with
`Unknown format code 'x' for object of type 'str'` — an old `_terms` handing a string to a
hex format. That is precisely the stale-process class the startup handshake now reports by
name rather than as a type error mid-tool-call.
