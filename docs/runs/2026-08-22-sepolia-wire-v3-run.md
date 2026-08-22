# Sepolia run — 2026-08-22 (wire v3)

The run `docs/wire-v3.md` requires before wire v3 can claim live evidence. Driven through
`scripts/demo.sh` over `erebus-cli`, not through MCP: this exercises the protocol, not the
agent transport. Branch `threat-model`, working tree uncommitted at the time of the run.

Cast, chosen because every previously used pair already holds a **wire-v2** channel and the
migration rule is that the client never retags an existing channel record:

| role | identity | address |
|---|---|---|
| payer / proposer (A) | `~/.erebus-d` | `0x6af6b7a6…477ee9` |
| payee / counter (B) | `~/.erebus-b` | `0xe67a3957…bfdda7` |

`b`↔`d` had no channel between them, so both directions opened fresh at wire v3.

## Transactions

| what | transaction | block |
|---|---|---|
| channel open, d → b | `0x4edc75bcebd41e8a1d0cb781b660dbde76635d9332ed3449f6c929d314da8` | — |
| channel open, b → d | `0x7b48f2cb482e7b1970ed36e46896665c854d7ffd4fd926e8882f407f358e3a3` | — |
| settlement, deal A (0.75 STRK) | `0x190c6b73584a2d2f3f56b09953bd67821f6f8899a696b4d3b025208442a8d02` | proved at 13860710 |
| settlement, deal B (0.5 STRK) | `0x6bb25aa98e16b6e935d1d25e6d3b24fd082754d6d2a3470f1909b3bc3898372` | proved at 13860854 |

Both channel records carry `wire_version: v3`. Handles: `ch_4c47b11b…a3eb8d` (A direction),
`ch_dbe1c7fa…24838e` (B direction).

**Gap against the spec.** `docs/wire-v3.md` asks the run to record "the offer transaction".
It is not here. `scripts/demo.sh` pipes each `propose_offer` envelope through a filter that
extracts `offer_id` and discards the rest, so the transaction hash never reaches the log. The
offers are on-chain and readable; only their transaction hashes were not captured. The script
should tee the envelope.

## Repeat deals, which is the point of wire v3

The A-direction channel ended the run holding **seven offers across three distinct deal IDs**,
two of them settled:

```
7912199861348341208   us:0                 1.00 STRK   proposed   (abandoned, see below)
14480521301412463257  us:5 them:0 us:10    0.75 STRK   settled    (deal A)
16230669557393962923  us:16 them:5 us:21   0.50 STRK   settled    (deal B)
```

Note indices are contiguous across deals — 0-4, 5-9, 10-14, 16-20, 21-25 in A's direction —
so the sequential-indexing invariant holds across the deal boundary, not just within a deal.

Deal IDs `14480521301412463257` and `16230669557393962923` both exceed `i64::MAX`, which is
why `deal_id` crosses the seam as a string. That is deliberate and has a known-answer test.
It is also the exact hazard two other fields got wrong; see F39 and F40.

## Disclosure was scoped, and this run proves it the hard way

Each deal's `grant_viewing_key` named one `deal_id` and a one-hour expiry. Each `reveal`
returned exactly three offers — that deal's offer, counter, and acceptance — from a channel
that by then held three deals. The abandoned deal and the other settled deal did not appear in
either disclosed record.

A run on a clean channel could not have shown this: with one deal in the channel, a grant that
leaked everything and a grant that leaked one deal produce identical output. The aborted first
attempt left a second deal in the channel by accident, and made the scoping claim falsifiable.

Settlement evidence in both records had `agreed_amount == paid_amount`.

## Balances, before and after

```
d   [1.00, 1.00]                    ->  [0.50, 0.25]     paid 1.25, took 0.75 back as change
b   [1.00, 1.00, 0.90]              ->  [..., 0.75, 0.50]  received 1.25
```

Both settlements selected a 1 STRK input for a smaller payment, so the change path ran twice.
Allowance consumed: `d` 12 STRK (6 writes), `b` 6 STRK (3 writes), from 30 STRK granted each.

## What fought us

Three things, in the order they appeared.

**1. `approve` reported failure for a write that succeeded.** Logged as F39. Provisioning the
two identities was the first step of the run and it returned `INTERNAL: response failed to
serialize: number out of range` for both, after both allowances had landed on-chain.

**2. `scripts/demo.sh` had never been run in its current form, and did not work.** The
transcript check compares `t["amount"]` to an int and `t["memo_hash"]` to `0x1234`, but both
now cross the seam as strings. The assertion failed with `amount 1000000000000000000 !=
1000000000000000000` — two identical-looking values, because the f-string prints a string and
an int the same way. The `:#x` format on `memo_hash` would have raised
`Unknown format code 'x' for object of type 'str'` immediately after, which is the same error
the 2026-08-19 run recorded from a different site. The rest of the script already coerces
correctly (`int(o["terms"]["amount"])`); only this block was stale. Fixed in this branch.

This cost one channel-open and one `propose_offer` proof — the abandoned deal above.

**3. The uniqueness filters contradict the feature.** `demo.sh` selects offers with
`if len(matches) != 1: FAIL`, matching on proposer and amount. The diff removed the
"this pair has already settled" guard so the script could run twice on one pair, but left
these filters, so a second run **at the same amount** fails with `found 2`. The two deals here
used 0.75 and 0.5 STRK to route around it. The script cannot yet demonstrate the repeat-deal
case it was edited to allow; it needs to match on `deal_id`, which the envelope now carries.

## An observation, not yet a finding

`read_channel_state` reports `settled: true` for the channel while deal
`7912199861348341208` in it is still `proposed`. Under one-deal-per-channel that flag was
unambiguous. Under repeat deals a channel-level boolean cannot express per-deal state, and
`demo.sh` used to gate on exactly this flag. Nothing depends on it now that the guard is gone,
but the field's meaning has quietly changed from "this channel is finished" to "at least one
deal here settled".

## What this run does not show

- Nothing went through MCP. The 2026-08-19 run covered that path at wire v2; it has not been
  re-verified at wire v3.
- No observer/linkage measurement was run against wire-v3 output. `docs/wire-v3.md` requires
  the fifth-salt classifier to score `0.5000` on v3 codec output; that check is not in this
  run.
- Deal sizes were 0.75 and 0.5 STRK. F40 means a deal above about 18.4 STRK would fail at
  `reveal`, and this run stayed far below that line.

---

# Second round — the same day, after fixing F39 and F40

The first round found two `u128`-to-JSON defects and logged them without fixing them. This
round fixes both and re-runs on Sepolia, deliberately **above** the boundary that made them
fail, because below it the fixed and unfixed code are indistinguishable.

Same pair, `d` → `b`, and the same wire-v3 channel `ch_4c47b11b…a3eb8d`. No new pair was
needed: repeat deals are the feature, so the fourth deal went into the existing channel.

## F39, checked directly

`approve` at 60 STRK — the exact call shape that returned
`INTERNAL: response failed to serialize: number out of range` in round one:

```json
{"ok":true,"result":{"approved":"60000000000000000000",
                     "tx_hash":"0x6c6014b489b398d41e2b69c489ff8b43b2a22184dcc498397435197779bc043"}}
```

An unplanned consequence worth recording: with the response working, it carries the
transaction hash, so `scripts/wait-for-depth.sh` can gate on it. In round one the failed
response swallowed the hash, and the block-depth wait that runbook §2 calls **not optional**
could not be performed at all. The serialization bug had silently disabled a safety check two
steps downstream of itself.

## F40, checked where it actually bit

`shield` 20 STRK to `d` (`0x7eec190dcdbef7fd1911c305bf8b49de027c290dcaff6923c4f1f0c4132c6e2`),
then a deal at **19 STRK** — above the ~18.4 STRK `u64::MAX` ceiling.

| what | transaction |
|---|---|
| settlement, deal D (19 STRK) | `0x60eace8bb6ee3027f0b659c22d765e1927f3578d3b0c8b9d7eb4c0ace8ea7be` |

The reveal returned, and returned strings:

```json
"settlement": { "agreed_amount": "19000000000000000000",
                "paid_amount":   "19000000000000000000" }
```

Before the fix this settlement would have landed on chain, irreversibly, and *then* failed to
disclose. That is the whole reason the amount was chosen: a 0.75 STRK deal exercises the same
code path and proves nothing about it.

The document is now internally consistent — `terms.amount` and `settlement.agreed_amount`
carry one quantity in one encoding, which was the specific complaint in F40.

## State after both rounds

One directional channel now holds **ten offers across four distinct deal IDs**, three of them
settled:

```
7912199861348341208   proposed                    (abandoned in round one)
14480521301412463257  countered settled settled   0.75 STRK
16230669557393962923  countered settled settled   0.50 STRK
2866947833165484364   countered settled settled   19.0 STRK
```

Balances: `d` `[1.0, 0.5, 0.25]`, `b` `[19.0, 1.0, 1.0, 0.9, 0.75, 0.5]`. Every settlement
took change.

## What this round still does not show

Unchanged from round one, and none of it is addressed here: the MCP path is unverified at
wire v3, the fifth-salt linkage measurement has not been re-run against v3 codec output, and
`scripts/demo.sh` still cannot demonstrate repeat deals on its own because its uniqueness
filters match on proposer and amount rather than on `deal_id`. This round routed around that
filter for the fourth time, using a distinct amount.

---

# Third round — closing the gaps the first two rounds left open

## Linkage, measured against live wire-v3 transactions

`docs/wire-v3.md` requires the historical fifth-salt classifier to score `0.5000` on wire-v3
output. It does:

```
M1  balanced accuracy  0.5000   target 0.5   (tp=0 fp=0 tn=10000 fn=4)
M2  accuracy           0.5030   target 0.5
```

The first round's positive was codec-derived, which the report listed as a limit. Three live
Sepolia settlements are now committed as positives
(`scripts/fixtures/observer-wire-v3-live-*.json`, real `apply_actions` calldata pulled from
chain), so M1 measures the deployed system and not only the codec. The classifier detects
none of the four.

`scripts/observer.py` run directly against each of the three transactions agrees: content
not recovered without a key, and not classified by the fixed fifth-salt shape.

The candidate-salt count the observer reports varies across the three (5, 9, 7 over 61
calldata felts each). That is the observer's in-range heuristic rather than the note count,
and a varying shape is the opposite of the constant fingerprint F31 exploited.

Remaining limits are unchanged and still stated in the report: negatives are synthetic rather
than sampled from live pool traffic, there is no timing-only baseline, and four positives is
a small sample — M1 recall has a resolution of one quarter.

## MCP path, verified at wire v3

The first round drove the protocol through `erebus-cli` only, leaving the MCP leg proven at
wire v2 (2026-08-19) but not v3. Closed: a freshly started `erebus_mcp.server` on the seam
backend, identity `d`, real MCP client over stdio, against the live channel.

- 10 tools served; startup handshake and startup `doctor` both pass.
- `doctor` — 9/10, the exception a `gas_balance` warning that was true at the time.
- `get_note_balance` — amounts as decimal strings.
- `read_channel_state` — 15 offers across 6 distinct deal IDs through the transport, every
  `terms.amount` a Python `str`, and `memo_hash` the full-width
  `0x00000000000000000000000000005678` rather than a truncated integer.

## `scripts/demo.sh` can now demonstrate repeat deals

Every step keys on the deal ID the run creates, established immediately after
`propose_offer` and threaded through both directional reads and the grant. The previous
`(proposer, amount)` filters could not survive a pair trading twice at one price, which is
the case wire v3 exists to support; all three earlier deals used distinct amounts to route
around it.

Verified by running the script twice at the **same** amount, 0.25 STRK. Both runs matched
exactly one offer at each step where the old filters would have found two.

The second attempt failed at `accept_and_settle` with `SUBMIT_FAILED` /
`"Insufficient ERC20 balance"`: `d`'s public STRK had fallen to 1.55 and the pool pulls a
2 STRK fee per write. A funding exhaustion, not a matching failure — it had already cleared
both filters under test. `d` was refunded 25 STRK from `e`
(`0x001c81680c8225182279f877d8a142313aa01bb9f6f16ed9e02b680d3252b65e`) and the deal re-run,
settling as `0x9cf2f86a3b1fc79abc4e0c6bfb0f4b797d9f0ed60c05319fc5926fb0d5e887`.

The channel now holds **two settled deals at the identical 0.25 STRK price**, plus the
abandoned third:

```
15370372084312329435  0.25 STRK  countered settled settled
14793700134513158960  0.25 STRK  countered proposed          (abandoned, out of fee STRK)
755997519791712192    0.25 STRK  countered settled settled
```

Eighteen offers across seven deal IDs in one directional channel. Under the old filters the
second of these was unreachable: `read A's offer from B's direction` would have matched two
offers and aborted before any proof was spent.

**Worth noting for anyone budgeting a run:** the pool fee is charged in public STRK through
`transfer_from`, so an identity can hold a healthy shielded balance and a healthy allowance
and still be unable to write. `doctor` reports this as a `gas_balance` warning, which is the
check to read before a long session rather than after it.

## A pinning weakness found while reviewing, and fixed

`sdk/ts/tests/gen-wire-v3-vectors.test.ts` called `writeFileSync` unconditionally inside a
`test()`. Every `pnpm vitest run` therefore rewrote `sdk/rs/tests/fixtures/ts-wire-v3.json`
— the known answer `sdk/rs` is pinned against.

Nothing had drifted; the file is byte-identical across regenerations. But the mechanism meant
a change to the TypeScript codec would silently redefine the Rust KAT: edit TS, run the
suite, commit, and `cargo test` still passes while both implementations move together. That
is exactly the failure the "nothing lands in `/sdk/rs` unpinned" rule exists to prevent, and
the differential test would have reported agreement while measuring nothing.

The generator now compares against the committed file and fails with an explanatory message
on any difference. Regeneration is deliberate: `UPDATE_WIRE_VECTORS=1 pnpm vitest run
gen-wire-v3-vectors`. Verified by tampering with one committed salt and confirming the suite
fails.
