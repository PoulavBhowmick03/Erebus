# Three-minute sprint video

Keep the network label visible during every receipt. Do not show environment files, RPC
credentials, account keys, pool keys, viewing grants, or complete prover requests.

## 0:00–0:25 — What Erebus does

“Erebus lets two agents negotiate encrypted terms and settle through the STRK20 privacy
pool. It hides the terms, not the relationship. This browser demonstration is a simulation;
the linked receipts are the chain evidence.”

Show the public demo title and its simulation label.

## 0:25–0:55 — Architecture

“Each agent calls role-bound MCP tools. The Python server passes the request to the Rust SDK.
Rust owns the keys, state, proving, signing, and submission. The transaction prover reads
Starknet state through an RPC and produces the proof used by the STRK20 pool.”

Show the architecture diagram. Do not open a terminal containing credentials.

## 0:55–1:35 — Recorded mainnet evidence

“On Starknet mainnet, both identities registered with the canonical pool. They then opened
one channel in each direction. These four transactions succeeded. Channel setup makes the
relationship public, and it is not a completed settlement.”

The published video records that four-transaction snapshot. It predates the later full
mainnet canaries. Account C subsequently registered, and the screened workflows described
below completed on 2026-08-31.

## 1:35–2:20 — Complete mainnet workflow

“The complete bounded workflow now runs on Starknet mainnet. Starkscan returned a screened
proof for a 1 STRK shield. Two role-bound agents negotiated through MCP, settled 0.8 STRK,
returned 0.2 STRK change, and reconstructed the result for an authorized recipient. Durable
operation IDs and reconciliation prevented a blind retry when the terminal closed.”

“A second bounded canary tested a different path. The buyer opened at 0.48 STRK, the seller
countered at 0.6, and the buyer accepted. It paid 0.6 STRK, returned 0.4 STRK change, and left
an unrelated 0.2 STRK note untouched.”

Show both mainnet run records, their eight pool receipts, conservation checks, reconciliation
results, observer results, and disclosure results. Link the run records on screen:

- `docs/runs/2026-08-31-mainnet-starkscan-workflow.md`
- `docs/runs/2026-08-31-mainnet-060-040-canary.md`
- `docs/runs/v0.2-mainnet-canary.json`

## 2:20–2:45 — Observer and disclosure

“A public observer sees accounts, timing, action shape, note count, and the counterparty at
channel open. It cannot recover wire-v3 offer terms. A registered recipient with a scoped
grant can reconstruct one deal. The grant gives no spending authority.”

Show the observer result beside the authorized disclosure result.

## 2:45–3:00 — Honest limit and links

“Two screened mainnet canaries now cover shielding, different MCP negotiation paths, atomic
settlement, observer resistance, and scoped disclosure. Erebus still exposes the relationship,
timing, action shape, and note count. The repository, receipts, and run records are linked here.”

Finish on the public demo and repository URLs.
