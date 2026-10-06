# Metropolis External Acceptance Handoff

This is the reproducible workflow an **independent outside operator** runs to accept the
Metropolis testnet release. The Erebus team's own runs, fresh virtual environments, and
subprocesses are not independent acceptance; this document is the handoff that an external
operator needs. The gate closes only when someone outside the team completes it and reports the
evidence below.

Current status: `0.3.0.dev5` is published on the Metropolis index and qualified on both
platforms, and `0.3.0.dev4` remains served by the cumulative index. Use the version the
[install guide](metropolis-install.md) documents.

## What the run proves

One public-bound deal on Monad testnet: two agents negotiate privately, the buyer submits
exactly one payment, the seller's access service survives a restart and re-issues the same
resource without a second charge, and an independent auditor verifies the finalized payment from
an encrypted grant and public chain data alone.

This rail is **public-bound**: the accepted agreement, blinding, bilateral signatures, amount,
parties, and timing are public in transaction calldata. Only the offchain negotiation history
stays private. Shielded settlement is a local prototype and has not run on Monad.

## Prerequisites

- macOS 11+ on Apple silicon, or Linux x86_64; Python 3.11+; `uv`.
- A funded Monad testnet account for the buyer's approval transaction, and a separate gas
  account for settlement. Testnet MON only; the combined budget for this exercise is
  **0.5 testnet MON**.
- Two distinct Monad RPC providers. `https://testnet-rpc.monad.xyz` plus
  `https://rpc.ankr.com/monad_testnet` or `https://rpc-testnet.monadinfra.com` are known to work;
  the second provider's `eth_getLogs` rate limit must tolerate eight concurrent queries.
- The deployed `ErebusSettlement` at `0xa5f0c864f434331bef9a7fc5e05450d598d24da4` (chain 10143,
  verifier version 1, deployment block 67495473), its manifest in
  `contracts/evm/deployments/monad-testnet.json`, and a standard ERC-20 to trade. The team's
  test token is `0x902f79145059910ef875aecf4187c771b204ea14`; mint or transfer at least the
  agreed price to the buyer and approve the settlement contract from the buyer.
- Token approval has no product command yet. `preflight` prints the exact `approve` calldata and
  amount; submit it with an operator wallet, not with the raw agreement key file.

## Steps

1. Install from the public index, outside any repository checkout, and verify the commands:

   ```sh
   uv venv --python 3.11 erebus-accept
   uv pip install --python erebus-accept/bin/python \
     --no-config \
     --index https://poulavbhowmick03.github.io/erebus-metropolis/simple/ \
     --default-index https://pypi.org/simple --index-strategy first-index \
     "erebus-mcp-server==<published version>"
   export PATH="$PWD/erebus-accept/bin:$PATH"
   printf '%s' '{"method":"version"}' | erebus-negotiate
   printf '%s' '{"method":"version"}' | erebus-payment
   ```

2. Write a rehearsal plan. The `scripts/metropolis-monad-rehearsal.py` harness is not part of the
   package index; take it from the repository at the published source commit or re-create the
   requests by hand. The plan fields and a worked example are in the
   [operations guide](metropolis-operations.md#10-live-monad-rehearsal). Key values:

   - `rail`: `public-bound`; `chain_id`: 10143; the two RPC URLs; the settlement contract and
     deployment block; the asset; the token and gas key paths (owner-only files).
   - `verification_timeout_seconds`, `agreement_lifetime_seconds`, `grant_lifetime_seconds`,
     `max_log_queries`, `max_concurrent_queries`, `max_ancestry`. The harness rejects a plan whose
     scan cannot finish before the agreement and delivery deadlines.
   - One payment of the agreed price; keep the combined spend inside 0.5 testnet MON.

3. Initialize participants and check readiness (read-only):

   ```sh
   python3 scripts/metropolis-monad-rehearsal.py init --plan plan.json --workdir DIR
   python3 scripts/metropolis-monad-rehearsal.py preflight --workdir DIR
   ```

   `preflight` exits 2 with the exact missing prerequisites until the buyer's token balance,
   allowance, and gas funding are present. It never signs.

4. Run the workflow once, with explicit authorization:

   ```sh
   python3 scripts/metropolis-monad-rehearsal.py run --workdir DIR --authorize-live-transactions
   ```

   Expected result in `DIR/run-record.json`:
   - `settle.submitted_this_call: true` on exactly one call; one transaction total.
   - `payment_observed_by_buyer.stage: Finalized`.
   - `access.resource_verified: true` after a seller-service restart.
   - `auditor.payment_verified: true`, `delivery_verified: false` (delivery is a seller claim,
     not an independent audit).

5. Verify independently, read-only: read `consumedDeals(deal_nullifier)` at the finalized block,
   the buyer's and seller's token balances, and the transaction receipt. The auditor result must
   come from the encrypted grant, the auditor key, and public RPC configuration only.

## Recovery rules (never pay twice)

- After any broadcast attempt, recovery is **observation only**. Never create another payment,
  permit, transaction, or agreement to recover this operation.
- A pending-history reply with `broadcast_attempts: 0` may continue the read-only scan and then
  call `settle` again. Any recorded attempt, ambiguous submission, or missing durable diagnostic
  switches permanently to observation.
- A timed-out or failed `settle` call is not evidence of nonpayment; observe.
- If the agreement or delivery deadline passes before broadcast, the operation cannot settle
  under those terms. Create a replacement agreement with a fresh deadline; do not reuse the
  expired operation and do not repeat token onboarding if the participant keys are unchanged.
- Interrupted scans and ancestry walks resume from durable checkpoints; never reset or skip
  history.

## Disclosure

The seller exports one recipient-bound, issuer-signed encrypted grant for the deal. The auditor
opens it with its own key, authenticates the accepted agreement, and verifies the finalized
payment from public chain data. The grant expires; it is a perishable artifact. Exporting a fresh
grant for the same deal after expiry is allowed and creates no payment. Grants never carry
spending authority or parent channel keys.

## Evidence to report

Report the installed version and source commit; the settlement transaction hash, block, and
status; the deal commitment and nullifier; `consumedDeals`; both token balances; the
`run-record.json` stage timings; the auditor result; the RPC providers used; and any friction.
Do not report the team's own runs as this acceptance.

## What remains after this run

Hosted services are preparation-only (Render blueprint, no paid provisioning), the demo video is
separate, and mainnet activation is a later release decision. Report the external run's evidence
to the team; M8 is complete only when this handoff has been run by an outside operator and the
other acceptance gates are met.
