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

## 0:55–1:35 — Mainnet evidence

“On Starknet mainnet, both identities registered with the canonical pool. They then opened
one channel in each direction. These four transactions succeeded. Channel setup makes the
relationship public, and it is not a completed settlement.”

Show the four mainnet Voyager receipts from `strk20.json`. Show `SUCCEEDED`, block number,
and the canonical pool address for at least one channel transaction.

## 1:35–2:20 — Complete Sepolia workflow

“The complete workflow currently runs on Starknet Sepolia. Two role-bound agents negotiate
through MCP, settle from shielded notes, return change, and reconstruct the result for an
authorized recipient. Durable operation IDs and reconciliation prevent blind retries.”

Show the recorded MCP settlement and disclosure evidence in
`docs/runs/2026-08-22-agents-mcp-wire-v3.md` and the packaged recovery evidence in
`docs/runs/2026-08-27-packaged-recovery-canary.md`.

## 2:20–2:45 — Observer and disclosure

“A public observer sees accounts, timing, action shape, note count, and the counterparty at
channel open. It cannot recover wire-v3 offer terms. A registered recipient with a scoped
grant can reconstruct one deal. The grant gives no spending authority.”

Show the observer result beside the authorized disclosure result.

## 2:45–3:00 — Honest limit and links

“Mainnet shielding still needs screening access from the pool operator. Until that arrives,
mainnet negotiation and settlement are not claimed. The repository, live demo, receipts,
and reproducible run records are linked here.”

Finish on the public demo and repository URLs.
