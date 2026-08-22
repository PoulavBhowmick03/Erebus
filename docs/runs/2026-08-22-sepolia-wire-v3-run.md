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
