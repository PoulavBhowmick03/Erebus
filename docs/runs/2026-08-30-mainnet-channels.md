# Mainnet directional channel setup — 2026-08-30

Erebus opened one STRK20 channel in each direction between its two registered mainnet
identities. Both transactions succeeded against the canonical pool. They did not shield
funds, exchange offers, settle a payment, or disclose a deal.

## Environment

- Source commit: `0bf51f10910eb7c8ff22c70fbd95c91d788a0ba4`
- Pool: `0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a`
- Prover image: `strk20-prover:privacy-0.14.3-rc.2-arm64`
- Prover service version: `0.19.0-rc.2`
- Prover JSON-RPC version: `0.10.3-rc.2`
- Chain: `SN_MAIN`
- Upstream state and submission RPC: Alchemy Starknet mainnet v0.10
- Pool fee: 6 STRK for each `apply_actions`
- Proving lag: 10 blocks

## Accepted transactions

| Direction | Transaction | Block | UTC timestamp | Network fee | Channel handle |
| --- | --- | ---: | --- | ---: | --- |
| A to B | `0x0395563b33df0d121ef9a7aa720da7cbbc378f7c0ed9849d2e034a1a08ada09a` | `14100846` | `2026-08-30T13:53:49Z` | `2.770015726942285760 STRK` | `ch_b7afee5fd1f75ddc8425e8ca8b7879b4780588f81163a581f258289c238d9af8` |
| B to A | `0x0467295d1d167607cf321cb6076f1ccd1b08f36d4c7575cd8e9dd242c4c01964` | `14101246` | `2026-08-30T14:05:01Z` | `2.770018219704277440 STRK` | `ch_c94f5afb8ad73af9f78baf4be45099ca7f57f28c230fa3a1104bef70b0d04fb2` |

Both canonical receipts reported `SUCCEEDED` and `ACCEPTED_ON_L2`. The first receipt was
checked at 47 blocks of depth and the second at 16 blocks. Both exceeded the configured
10-block proving lag. Post-write channel reads succeeded and returned no settlement, which
is the expected state for a newly opened channel.

## Recovery evidence

The first B-to-A proving attempt exhausted the 20 GiB Colima VM and killed the prover before
the SDK signed a transaction. `reconcile` classified the durable operation as `no_effect`
and `safe_to_retry`, with no transaction hash. The operator added 8 GiB of VM swap, limited
Rayon to four threads, and resumed the same caller-supplied operation ID. The resumed attempt
succeeded. No replacement operation ID was created.

After both receipts, `reconcile` reported `effect`, `next_action: none`, and agreement
between chain effect, local state, and result for each identity. Both `doctor` reports were
ready. Account A retained a 19 STRK pool allowance. Account B retained 6 STRK.

## Evidence boundary

These are real mainnet channel-opening transactions and bring the sprint manifest to four
successful canonical-pool transactions: two registrations and two channel opens. Opening a
channel publicly exposes the counterparty relationship. Mainnet shielding remains blocked
on operator-issued screening access, so mainnet offers, settlement, and disclosure have not
been demonstrated. The complete agent negotiation, shielded settlement, observer test,
recovery, and selective-disclosure evidence remains on Sepolia.
