# Sepolia run — 2026-08-22, autonomous agents over MCP at wire v3

Closes the three definition-of-done criteria that the wire-v3 migration had left proven only
at wire v2. Everything here runs through the MCP server; nothing uses `scripts/demo.sh`.

## Why a separate run was needed

The earlier 2026-08-22 runs drove the protocol through `erebus-cli` directly. That proves the
protocol and says nothing about the agent layer. Criteria 1, 3, and 4 were last demonstrated
on 2026-08-19 at wire v2.

## The blocker, and why it dissolved

The configured `erebus-buyer` (`g`) and `erebus-seller` (`f`) MCP servers share a settled
wire-v2 channel from 2026-08-07. One deal per channel is terminal at v2, so that pair cannot
open a new deal, and an existing channel record never gets retagged to v3.

Repointing them was not necessary. `run_negotiation_over_mcp` takes `buyer_params` and
`seller_params` as injectable `StdioServerParameters`, so the run spawns its own seam-backed
MCP servers on a free pair. The agent code is unchanged; only the server parameters differ,
which is the whole point — the same loop that runs against the mock ran against Sepolia.

## Cast

| role | identity | address |
|---|---|---|
| buyer / payer | `~/.erebus-g` | `0x23ad2a76…c48b948` |
| seller / payee | `~/.erebus-d` | `0x6af6b7a6…d72477ee9` |
| disclosure recipient | `~/.erebus-c` | `0x73dde958…504445b8b` |

`g`↔`d` had no prior channel, so both directions opened fresh at wire v3. `c` is a
participant in neither side of this deal.

## 1. Two agents autonomously negotiate and reach agreement

`BuyerPolicy(budget = 2 STRK)` against `SellerPolicy(reserve = 1.5 STRK)`, three rounds
allowed. No amount was scripted; both sides decided from policy.

```
channel_opened   buyer ch_844d3a46…734d50   seller ch_938eff1d…41834d
buyer_decision   round 0  propose
proposed         buyer    ch_844d3a46…:us:0        1.6 STRK
seller_decision  round 0  counter
countered        seller   ch_938eff1d…:us:0        1.6 STRK
buyer_decision   round 1  accept
settled          buyer    0xc897e94b5663144c11ae7d3269d93caf4bfc7e442f030cffa4fdc28fdc92cb
```

The seller countering at the buyer's own number is agreement, not haggling: in Erebus the
accepting identity pays, so `SellerPolicy` never returns `ACCEPT` — it restates the terms and
leaves them for the buyer to fund.

Deal `12092566798693785331`, three offers, one atomic settlement.

Balances confirm value moved: `g` `[2.25]` → `[0.65]`, having spent a 2.25 STRK note on a
1.6 STRK payment and taken 0.65 back as change; `d` gained a 1.6 STRK note.

## 3. An independent third party reconstructs the record

`grant_viewing_key` from the buyer's MCP server, naming deal `12092566798693785331`, grantee
`c`, and a one-hour expiry. The capsule is written to a mode-`0600` file and never appears in
the MCP result or the transcript — the tool returns only the path and metadata.

`c` then called `reveal` on its own MCP server and reconstructed:

- participants `g` and `d`
- 3 offers, all carrying deal `12092566798693785331` and no other
- `agreed_amount` and `paid_amount` both `1600000000000000000`, `is_consistent: true`

`c` is neither buyer nor seller. This is the criterion-3 case the earlier v3 runs did **not**
establish: those granted to `$B`, the counterparty, who already knew the deal.

## 4. The whole loop through the MCP server

Every write and read above went through `erebus_mcp.server` on the seam backend over stdio:
`open_channel`, `get_note_balance`, `read_channel_state`, `propose_offer`, `counter_offer`,
`accept_and_settle`, `grant_viewing_key`, `reveal`. Both servers reported
`doctor: ready, all checks passed` at startup.

The agent layer touched no Erebus internals: it holds no key material, computes no salts, and
addresses channels only by opaque handle.

## What this run does not show

- One negotiation shape, converging in two rounds. No walk-away, no expiry, no rejection path
  was exercised live; those have unit coverage only.
- `mcp_loop.py` still cannot be pointed at a live chain by its own entry point:
  `server_params()` hardcodes the mock backend and `PROVING_SERVICE_URL: http://unused.invalid`,
  and `demo_mcp.py` hardcodes `0xbuyer`/`0xseller`. This run supplied seam-backed parameters
  from outside. Giving the agent package a documented live mode is a real gap for anyone
  trying to reproduce this.
- The disclosure step is an operator action, not something the agents negotiate. The loop
  reports it as `available_as_an_explicit_recipient_bound_operator_step` and stops there.
