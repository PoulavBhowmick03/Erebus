# Metropolis M8: x402 Decision Record

Status: **open; no scheme selected.** This records the gate and the evidence needed before any
x402 code is written. The roadmap deliberately requires composition "only against a specified
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
interrupted request recovers without a second charge. No model is selected yet, and no x402
code exists.

## Owner decision

- Decision: _owner_
- Rationale: _owner_
