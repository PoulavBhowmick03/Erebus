# Metropolis Agreement Specification v1

Written: 2026-09-20. Status: normative for `erebus-core` at M1. The shielded suite remains an M4 gate.
Roadmap: [metropolis-roadmap.md](metropolis-roadmap.md). Decisions: [metropolis-decisions.md](metropolis-decisions.md), D01-D04.
Implementation: [`sdk/core`](../sdk/core). Vectors: [`sdk/core/tests/fixtures/agreement-v1-vectors.json`](../sdk/core/tests/fixtures/agreement-v1-vectors.json).
This document specifies the reviewed M1 encoding, suite 1, and policy semantics. It does not freeze a shielded suite; see section 12.

## 1. Scope and versioning

An agreement is one signed revision of one deal. It fixes the deployment, the asset and amount, the service promise, the settlement mode, the guarantees the settlement backend must provide, and the expiry of the authorization. Both roles authorize the same blinded commitment; the opening is revealed only to the settlement backend.

Three version axes are independent:

| Axis | Where | Rule |
|---|---|---|
| `protocol_version` | inside the agreement, `u16` | must equal `1`; unknown values are rejected explicitly |
| `suite_id` | inside the agreement, `u16` | selects the hash and authorization algorithms; unknown values are rejected explicitly |
| `verifier_version` | inside the deployment domain, `u32` | backend-defined; the EVM adapter must reject a mismatch |

Adding a suite does not change the encoding of existing agreements. Changing any field or encoding rule requires a new `protocol_version` and new vectors.

## 2. Identifiers and namespaces

| Concept | Form | Rule |
|---|---|---|
| Chain namespace | CAIP-2 `family:reference` | family `[-a-z0-9]{3,8}`; reference `[-_A-Za-z0-9]{1,32}`. Example: `eip155:10143` |
| Deployment domain | namespace plus optional settlement contract, optional pool, `verifier_version` | at least one of contract or pool must be present; the mode rules in section 3 apply |
| Asset identifier | CAIP-19 asset type `namespace:reference/asset_namespace:asset_reference` | asset namespace `[-a-z0-9]{3,8}`; asset reference `[-.%A-Za-z0-9]{1,128}`. NFT token-id suffixes are not supported |
| Amount | `u128`, base units | the agreement amount must be greater than zero; a fee of zero is valid |
| Key material | opaque bytes, 1..=64 | interpreted by the suite or backend, never by the shared core |
| Chain address | opaque bytes, 1..=64 | interpreted by the backend |

The core treats a chain family as opaque data. It never branches on `eip155`, `starknet`, or any other family name; a backend is selected by its declared capabilities (section 9).

Syntax follows [CAIP-2](https://standards.chainagnostic.org/CAIPs/caip-2) and the asset-type portion of [CAIP-19](https://standards.chainagnostic.org/CAIPs/caip-19).
The asset must name the same chain as the deployment. Backend-specific address canonicalization remains an M3 requirement before policy accounting.
The core does not equate different textual aliases for an address.

## 3. Agreement terms

Fields are encoded in the order listed. "raw" means no length prefix; "bytes" means a `u16` big-endian length followed by that many bytes; "text" is bytes that must be valid UTF-8.

| # | Field | Encoding | Rules |
|---|---|---|---|
| 1 | `protocol_version` | `u16` | must be 1 |
| 2 | `suite_id` | `u16` | must name an implemented suite |
| 3 | `domain` | nested, section 3.1 | |
| 4 | `deal_id` | raw 16 bytes | shared by every revision of the deal |
| 5 | `revision` | `u32` | greater than zero; monotonic within a deal; M2 enforces transcript order |
| 6 | `transcript_root` | raw 32 bytes | the negotiation transcript root; derivation specified in [M2 decisions](metropolis-m2-decisions.md) DM2-4. All-zero only for a deal with no transcript (legacy wire-v1/v2 and the retained vectors); a revision that concludes an M2-negotiated deal carries the computed root. The field is committed but is not yet constrained by a settlement predicate. |
| 7 | `buyer_authorization_key` | bytes 1..=64 | suite 1 requires exactly 20 |
| 8 | `seller_authorization_key` | bytes 1..=64 | suite 1 requires exactly 20 |
| 9 | `payment_recipient` | bytes 1..=64 | backend-interpreted; a shielded backend requires a note key the seller controls |
| 10 | `asset_identifier` | text 1..=256 | CAIP-19 |
| 11 | `amount` | `u128` | greater than zero |
| 12 | `expiry` | `u64` | Unix seconds, greater than zero; verification rejects at `now >= expiry` |
| 13 | `fee` | `u128` | zero is valid |
| 14 | `fee_recipient` | optional bytes 1..=64 | present exactly when the fee is non-zero |
| 15 | `settlement_mode` | `u8` tag | 1 public-bound, 2 shielded |
| 16 | `required_guarantees` | `u32` bitset | section 8; unknown bits are rejected |
| 17 | `settlement_nonce` | raw 32 bytes | fresh random per deal; identical in every revision |
| 18 | `service` | nested, section 4 | |

### 3.1 Deployment domain

| Field | Encoding | Rules |
|---|---|---|
| `chain_namespace` | text 5..=41 | CAIP-2 |
| `settlement_contract` | optional bytes 1..=64 | required by public-bound settlement |
| `pool` | optional bytes 1..=64 | required by shielded settlement |
| `verifier_version` | `u32` | backend-defined |

### 3.2 Mode and guarantee consistency

`validate` enforces the mode rules in both directions so a signed agreement cannot downgrade a privacy requirement:

- Public-bound settlement must name a settlement contract and must not require `hidden-amount` or `hidden-recipient`.
- Shielded settlement must name a pool and must require both `hidden-amount` and `hidden-recipient`.
- Suite 1 supports only public-bound settlement. Encoding, decoding, commitment construction, and backend selection reject suite 1 with shielded mode.
- No shielded suite is executable until M4 selects and implements one. The shielded tag is reserved, not a working backend.

## 4. Service record

The service record is part of the committed terms, so changing any field changes the commitment and invalidates every authorization over the previous terms.

| Field | Encoding | Rules |
|---|---|---|
| `resource` | text 1..=256 | resource identifier, for example `gpu.h100.hour` |
| `quantity` | `u128` | greater than zero |
| `unit` | text 1..=32 | for example `gpu-hour` |
| `access_recipient` | bytes 1..=64 | key that receives the delivered service; distinct from the payment recipient |
| `delivery_deadline` | `u64` | Unix seconds, greater than zero |
| `fulfillment_method` | text 1..=64 | for example `http-access` |
| `fulfillment_digest` | raw 32 bytes | commitment to out-of-band fulfillment parameters; all zero when there are none |

Payment and delivery are not fields of the service record. They are outcome states on the receipt (section 9); a receipt with a finalized payment and no delivery is representable and is not completion.

## 5. Canonical encoding rules

1. Every integer is big-endian at its full fixed width (`u8`, `u16`, `u32`, `u64`, `u128`).
2. Variable byte strings and text are length-prefixed with a `u16`; text must be valid UTF-8; no field exceeds 4096 bytes.
3. Enums are one `u8` tag. Optional fields are one `u8` presence tag (0 or 1); a 0 tag is followed by no payload.
4. Field order is the order in the tables above. There is exactly one valid encoding of a value.
5. A decoder rejects truncated fields, trailing bytes, unknown tags, out-of-range lengths, and invalid UTF-8. It never truncates, pads, or repairs.
6. Semantic validation runs on decode: an encoding that parses but violates sections 3-4 is rejected with the specific rule named.

## 6. Commitment and blinding

```text
Cdeal = Hash_suite("EREBUS_DEAL_COMMITMENT_V1" || encode(terms) || blinding)
```

`blinding` is 32 fresh random bytes per deal. The blinding is treated as sensitive: it is the only thing that hides predictable terms, and persistence follows key-material rules. The commitment is the only value a participant signs as agreement consent. A verifier recomputes it from the opening (terms plus blinding); without the opening the commitment is meaningless.

Both authorization APIs require the opening and recompute `Cdeal` before accepting the signature.
`verify_authorization` then checks expiry; `verify_authorization_signature` preserves audit verification after expiry.
Keeping the original commitment while changing any term fails with `OpeningMismatch`.
These functions check one role. A settlement backend must require one valid buyer authorization and one valid seller authorization.

## 7. Deal identity and revisions

```text
Ndeal = Hash_suite("EREBUS_DEAL_NULLIFIER_V1" || encode(domain) || buyer_authorization_key || settlement_nonce)
```

- Every signed revision shares `deal_id`, `settlement_nonce`, the buyer key, the domain, and the suite. These inputs produce one `Ndeal`.
- `revision` and other negotiable terms are not inputs. Switching suites is not a revision; M4 must specify cross-suite replay behavior before adding a suite.
- The first valid signed revision to settle consumes the deal. A later counteroffer does not revoke earlier signed permission; revisions remain executable until their authorized expiry or deal consumption.
- Changing the deployment creates a different domain and a different `Ndeal`. This does not promise global cross-chain deal uniqueness.
- Pool note nullifiers are separate values and do not replace `Ndeal`.

Cancellation is local. Stopping local work, deleting a draft, or refusing to submit does not revoke a previously signed authorization and does not write anything on chain. Onchain revocation is not part of the v1 contract. Both behaviors must be displayed to the authorizing party before it signs.

## 8. Guarantees and settlement modes

| Guarantee | Bit | Meaning |
|---|---|---|
| `hidden-amount` | `0x1` | the paid amount is not public |
| `hidden-recipient` | `0x2` | the payment recipient is not public |
| `agreement-bound-settlement` | `0x4` | the settlement verifier enforces the agreement binding, not only client policy |
| `scoped-disclosure` | `0x8` | deal-scoped disclosure to a chosen recipient is possible |

`settlement_mode` selects the mechanism; `required_guarantees` states what the mechanism must provide. A backend is selected only if it declares the mode and every required guarantee (section 9). A missing guarantee is an error, never a downgrade.

The legacy STRK20 adapter declares `hidden-amount` and `hidden-recipient`, but not `agreement-bound-settlement` or local proving.
Its amount equality is a Rust check, not a pool-proof predicate; see the M0 enforcement map.
Scoped disclosure remains available through the existing wire-v3 grant API. The adapter does not promise it for historical wire-v1/v2 channels.
STRK20 advertises no canonical suite. It accepts legacy offer identifiers only, not suite-1 agreements.

## 9. Settlement backend contract

Types live in `erebus_core::settlement`. The table defines the planned lifecycle; it is not an implemented canonical backend trait.
The core does not mandate proofs. Future TEE or private-rollup backends need explicit trust declarations, not an assumption of equivalent privacy.

| Operation | Required behavior |
|---|---|
| `capabilities()` | declare supported canonical suites, modes, guarantees, and whether proving is local |
| `prepare(agreement, operation_ref)` | persist canonical intent, then produce a backend-specific prepared transition; backend evidence is opaque to the core |
| `submit(prepared)` | submit the same authorized transition and record its transaction identity |
| `status(operation_ref)` | unknown, pending, included, finalized, reverted, or expired |
| `verify(receipt)` | verify domain, agreement binding, state transition, and chain evidence |

A receipt carries the operation reference, commitment, deal nullifier, domain, mode, transaction and block references, finality, guarantees, and payment/delivery states.
`is_complete()` means finalized payment and issued delivery. It does not establish the shielded-product release gate or independently verify chain evidence.
Recovery follows finality:

| Finality | Required action |
|---|---|
| unknown | reconcile from chain and journal evidence; never submit a fresh payment |
| pending, included | await chain evidence; inclusion may still reorganize |
| finalized, reverted, expired | stop |

A local timeout produces `unknown`. It is never evidence that an authorized deal is unpaid.

`check_capabilities` checks the suite, mode, guarantees, local-proving requirement, domain, and asset chain.
The backend must also compare the requested domain with its configured deployment before preparing funds.
The local-proving requirement is operator configuration; it is not a new signed field in v1.

`Strk20SettlementBackend::settle` is the implemented legacy boundary. The existing `Client::accept_and_settle` delegates to it.
The adapter checks requirements before key access, journal writes, RPC calls, or proving. It then runs the unchanged legacy settlement body.
Existing wire formats, offer ids, state, and CLI request formats remain unchanged. Canonical EVM execution remains M3/M6 work.

## 10. Spending policy

Policy runs below the agent layer (decisions D03). Evaluation is a pure function over caller-supplied state: permitted assets, a per-deal maximum, per-asset aggregate ceilings with windows, a counterparty allowlist, and a counterparty denylist.

- Denial precedence is fixed so the reason is deterministic: asset permission, counterparty permission, per-deal maximum, aggregate ceiling. Denial beats the allowlist.
- The aggregate window is the half-open interval `(now - window, now]`. A record exactly `window` seconds old is outside it. An asset with no configured ceiling has no aggregate ceiling.
- Limits are inclusive: spend equal to the ceiling is permitted; one base unit more is denied. Overflow is a denial, not a wraparound.
- Both limits count `amount + fee` in the agreement asset. Addition overflow is denied even without an aggregate ceiling.
- `reserve_agreement` checks policy and reserves that total under one mutable ledger borrow. Rejected requests do not reserve capacity.
- The coordinator must serialize ledger access across processes and persist reservations before submission. The in-memory ledger is not a durable wallet.
- A reservation holds capacity from authorization until it is committed (settlement has chain evidence) or released (the operation provably had no effect). An uncertain payment stays reserved until reconciliation decides it. Durable persistence of the ledger is coordinator work (M6); the accounting rule is implemented and tested in `erebus_core::policy`.

Policy enforcement protects a configured operator. It is not a claim that a hostile host cannot change its own policy.

## 11. Vectors and conformance

- `sdk/core/tests/fixtures/agreement-v1-vectors.json` retains three valid suite-1 vectors and one shielded rejection vector from the initial implementation.
- Valid vectors pin canonical bytes, `Cdeal`, `Ndeal`, authorization digests, and signatures. The shielded bytes must fail semantic decoding; they are not supported agreements.
- `python3 scripts/check_agreement_vectors.py` independently reconstructs all four byte encodings from the specification using Python's standard library.
- This cross-language check covers serialization, not independent hash/signature computation. Primitive cryptography uses the external vectors below.
- Regenerate with `cargo test -p erebus-core --test agreement_vectors -- --ignored --nocapture regenerate`. Changing a pinned value is a protocol change and requires updating this document first.
- Suite 1 primitives are additionally pinned against external vectors in `sdk/core/src/suite.rs`: published keccak256 vectors and the EIP-155 signature example (digest, signature, and recovered address).
- `sdk/core/tests/encoding_rules.rs` rejects every truncation and trailing byte of valid vectors.
- Authorization tests cover changed terms with the original commitment, wrong blindings, all service fields, expiry, role substitution, and signature malleability.
- `sdk/rs/tests/capabilities.rs` exercises the actual adapter and checks that rejected requests create no operation records or network connections.

## 12. What this specification does not establish

- No shielded suite is selected. Suite 1 is the public-bound path; the shielded suite is an M4 decision fed by `docs/metropolis-m4-feasibility.md`. An agreement naming an unimplemented suite fails explicitly.
- No proof relation is specified. The settlement statement in `docs/metropolis-architecture.md` section 5 remains the design target for M4/M5.
- No EVM adapter, contract, coordinator, or disclosure package is implemented at M1. Those are M3 and M6. The offchain transport is M2 and is specified separately in [metropolis-m2-decisions.md](metropolis-m2-decisions.md); it populates `transcript_root` but does not change this encoding.
- No service delivery is guaranteed by payment. Payment and delivery are separate states by construction.

## 13. M1 decisions for owner review

| # | Decision | Rationale |
|---|---|---|
| DM1-1 | Canonical encoding as specified in section 5 | One self-delimiting form; strict decode; mirrors the existing `RequestBinding` discipline (`sdk/rs/src/operation.rs`) |
| DM1-2 | Suite registry with suite 1 = keccak256 + secp256k1 ECDSA; shielded suite deferred | EVM-native verification for public-bound; avoids freezing an impractical circuit interface (D03) |
| DM1-3 | Shared core as a separate crate `sdk/core` | Compile-time isolation from Starknet dependencies; independent test loop |
| DM1-4 | Rejection at `now >= expiry`; cancellation is local-only and restated before signing | D02; avoids implying that local cancellation revokes onchain permission |
| DM1-5 | Policy window `(now - window, now]`, inclusive limits, reservations held until committed or released | Deterministic denial reasons; uncertain payments never free capacity early |
| DM1-6 | Payment and delivery as separate receipt states; `unknown` finality maps to reconciliation | A local timeout must not become a second payment |
