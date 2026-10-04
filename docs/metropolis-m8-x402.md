# Metropolis M8: x402 Decision Record

Status: **scheme selected: `exact` over Permit2, seller-facilitated (owner, 2026-10-03); primitives built.** This records the gate
and the evidence needed before x402 code is written. The roadmap deliberately requires composition "only against a specified
scheme", because an x402 integration changes what is public and can introduce a second charge.

## What x402 would add

An x402 resource server answers `402 Payment Required`, the buyer pays, and the server issues
access. Erebus already separates payment from delivery and can bind access to a finalized
agreement and buyer (`sdk/shielded/src/access.rs`). x402 would standardize the HTTP handshake
in front of that access service.

## Non-negotiables

1. **Exactly one payment.** A retried or replayed x402 request must resolve the existing
   payment; it must never settle a second time. The access service's durable issuance and
   idempotent request digest already provide the pattern.
2. **Explicit privacy exposure.** Public-bound settlement exposes amount, asset, payer, and
   recipient. A shielded settlement hides amount and recipient but still reveals timing and the
   participating endpoints. The chosen scheme must state which fields become public.
3. **No silent downgrade.** A deal that requires `hidden-amount` or `hidden-recipient` must not
   fall back to a transparent x402 rail.
4. **Payment is not delivery.** A verified receipt still does not prove the service was
   delivered; the paid-but-undelivered outcome stays representable.

## Canonical Monad testnet contracts (from official docs, 2026-10-02)

| Contract | Address |
|---|---|
| Permit2 | `0x000000000022d473030f116ddee9f6b43ac78ba3` |
| x402 ExactPermit2Proxy | `0x402085c248EeA27D92E8b30b2C58ed07f9E20001` |
| x402 UptoPermit2Proxy | `0x4020A4f3b7b90ccA423B9fabCc0CE57C6C240002` |
| Multicall3 | `0xcA11bde05977b3631167028862bE2a173976CA11` |

## Decision required

Select the scheme to compose against:

- **exact** — one fixed payment per request. Simplest; matches the current access service.
- **upto** — a metered cap; the buyer authorizes a maximum and the seller settles usage.
- **aggr_deferred / batched** — accumulate usage and settle once. Cheapest per request, but
  changes when payment finality happens and lengthens seller exposure.

The measured comparison the roadmap asks for must record, per model: latency from request to
access, proof and submission cost, on-chain accounting, what becomes public, and how an
interrupted request recovers without a second charge. The owner selected `exact`; the
comparison against other schemes is still open.

## Composition (DM8-13)

`x402-exact` is a separate payment mode. The buyer signs a Permit2 `PermitWitnessTransferFrom`
for the agreed amount to the seller's `payTo`, with the proxy as spender and the **deal
nullifier as the nonce**. The seller's access service verifies and submits
`x402ExactPermit2Proxy.settle` itself. Permit2's nonce bitmap then marks the deal paid, so a
retried or replayed request cannot pay twice. It is checked against the non-negotiables above:

1. One payment: the nonce is consumed once per `(buyer, deal)`. Cross-rail double payment is
   prevented by binding the signed agreement domain to the canonical exact proxy, and
   rejecting combined ordinary-settlement and x402 access tools.
2. Exposure: buyer, `payTo`, amount, token, nullifier, and the Permit2 payment authorization signature are public. The Erebus agreement opening and bilateral agreement signatures are not sent by this rail.
3. No downgrade: a shielded deal cannot use this mode.
4. Payment is not delivery: access issuance stays separate and durable.

Built and pinned so far: `sdk/evm/src/x402.rs` (digest, signing, `settle` calldata, nonce
bitmap, and `decode_settle_call` for auditors), KATs against the x402 Python SDK
(`tests/x402_vectors.rs`), and Anvil runs on the canonical runtime bytecode pinned from Monad
testnet (`tests/x402_permit2_chain.rs`). The seller endpoint, durable buyer permit, paired
finalized observation, and explicit Python/MCP access mode are implemented.
`sdk/shielded/tests/x402_http.rs` verifies rejected mutations, a dropped broadcast response,
restart, durable delivery, and exactly one send on a fixture agreement.

**Independent auditor (2026-10-04).** `erebus-disclosure` (and `erebus-shielded-disclosure`)
now accept `rail: "x402_exact"` in `verify_payment`. The auditor needs only the encrypted
grant, its own key, and public configuration: it authenticates the pinned canonical Permit2
and exact-proxy runtimes at two RPCs, decodes the permit and signature from the finalized
transaction input, and requires matching finalized target, calldata, transfer, and nonce
evidence at both observers. It rejects a buyer-signed permit that pays another recipient,
tampered grants, neighboring deals, non-payment transactions, and unmined observations
(pending, exit 2). Test: `auditor_verifies_finalized_x402_payment_from_the_grant_alone`.

**Negotiated two-process workflow (2026-10-04).** `negotiated_x402_settles_once_recovers_restart_and_audits_from_the_grant`
(`sdk/shielded/tests/support/payment_driver.rs`) runs real authenticated discovery and
encrypted Noise negotiation between separate buyer and seller processes, the seller publishes
the accepted agreement through its configured product access root, the buyer prepares one
permit, and the x402 access service submits it once. The test drops the first response,
restarts the service, retries with the same permit, retrieves the agreed payload, and asserts
exactly one broadcast and the agreed seller balance. It then exports an encrypted grant and
verifies the payment through the auditor path. Debug-build measurements from that run:
negotiation 759 ms, permit preparation 17 ms, first observed inclusion 81 ms after submission,
finalized verification 290 ms after submission, delivery 742 ms, proof null (public-bound has
no proving stage). These are local monotonic observations, not block-timestamp deltas.

This is not a live Monad run, a published package, a hosted service, or an external acceptance
rehearsal. The HTTP profile requires an authenticated Erebus request and a matching x402 v2
header; generic third-party x402 client interoperability is unverified.

## Operator Configuration

Use `backend.mode = "x402_exact"` in the access service config. Supply `namespace`, distinct
`rpc_url` and `peer_rpc_url`, `permit2_runtime_hash` and `proxy_runtime_hash` (32-byte arrays),
`transaction_key_file`, `signer_journal_root`, `gas_limit`, and decimal `max_fee_per_gas` and
`max_priority_fee_per_gas`. Both RPCs must authenticate the pinned canonical runtimes. The
gas account must be funded, and all users of that account must share its signer journal.
Keep `state_root` and the signer journal across restarts. One unresolved transaction holds
the account's claim; it does not authorize a replacement or another payment.

The signed agreement must name the exact proxy as its settlement contract, use suite 1 and
public-bound mode, and have zero fees. The buyer must separately approve the token to Permit2.
No approval is signed automatically. The native access request requires `x402_exact: true`;
the Python seam uses the same explicit flag. For the dedicated access MCP server, configure
`EREBUS_ACCESS_PAYMENT_RAIL=x402-exact`. Default `observe` never authorizes a payment. The
combined Metropolis buyer profile also accepts this rail if an access service is configured
and `EREBUS_PAYMENT_CONFIG` is absent. It exposes negotiation and access only, not ordinary
funding, settlement or recovery tools. The signed agreement still names the exact proxy.

New buyer permits expire within 120 seconds and never outlive the signed agreement. Retrying
an expired permit does not extend it; an already finalized payment can still deliver access.
The buyer persists one authorization before HTTP. The seller persists the exact transaction
and a broadcast fence before sending. After that fence, retries only observe: even a timeout
or rejected response cannot clear it. A crash between the fence and send can leave an operation
pending without a broadcast; it requires operator investigation, not another permit. Delivery
requires matching finalized transaction input, transfer and nonce evidence from both RPCs.
`PAYMENT-RESPONSE` and the resource receipt are seller assertions, not independent auditor
evidence. Buyer retrieval continues to report `payment_verified: false`.

This verifier checks the token's transfer event, not historical recipient balance deltas.
Use trusted standard ERC-20 assets; fee-on-transfer, rebasing and dishonest event-emitting
tokens are not qualified by this test evidence.

## Monad x402 harness (prepared, not run)

The local negotiated test is the harness. To run the same rail on Monad testnet, an operator
must supply, under explicit authorization:

1. A funded gas account key and its dedicated signer journal; all users of the account share it.
2. Two independently operated Monad RPC endpoints (the public endpoint plus a second provider)
   that both expose `finalized` and serve the canonical Permit2 and exact-proxy runtimes.
3. A standard ERC-20 approved to Permit2 by the buyer; the test token
   `0x902f79145059910ef875aecf4187c771b204ea14` was minted to the recorded buyer.
4. A negotiated suite-1 public-bound agreement whose domain names the canonical exact proxy
   (`0x402085c248EeA27D92E8b30b2C58ed07f9E20001`) and whose recipient is the seller's EVM key.

No live x402 broadcast is authorized by the current records. The gate stays open until an
operator runs it and records the transaction, permit nonce, and auditor result.

## Owner decision

- Decision: `exact`, for now (owner, 2026-10-03). Composition is built against `exact` first.
- Rationale: _owner_
- Not yet done: the measured comparison against `upto` and batched settlement. Choosing `exact`
  first does not complete that roadmap item.
