# Metropolis M3 Baseline: Public-Bound EVM Settlement

Recorded: 2026-09-22. Branch: `metropolis`. Scope: roadmap M3.
Decisions are in [metropolis-m3-decisions.md](metropolis-m3-decisions.md).

## What shipped

**Contracts** (`contracts/evm`, Foundry 1.5.1, solc 0.8.24):

| File | Owns |
|---|---|
| `src/ErebusCodec.sol` | Strict canonical decoder, commitment / authorization / nullifier digests, asset-token parsing |
| `src/ErebusSettlement.sol` | Domain, expiry, authorization, and replay checks; atomic exact-amount token payment |
| `src/MockERC20.sol` | Test token with an optional transfer fee (used to prove fee-on-transfer is rejected) |
| `test/ErebusVectors.t.sol` | Known-answer test against the Rust agreement vectors |
| `test/ErebusSettlement.t.sol` | Happy path plus the adversarial matrix |

**Adapter** (`sdk/evm`, `erebus-evm`, alloy 1.8):

| Module | Owns |
|---|---|
| `deployment` | Chain namespace, chain id, contract, verifier version, asset-token resolution |
| `evidence` | Canonical `terms + blinding + both signatures` carried in `backend_evidence` |
| `abi` | Hand-encoded `settle`, constructor, `mint`, `approve`, `balanceOf`, `allowance` calldata |
| `backend` | `capabilities`, `check`, `prepare`, `estimate`, `submit`, `verify`, `settle` |

## Evidence

```bash
cd contracts/evm && forge build && forge test
# 25 passed (23 settlement + 2 vector/KAT)

cd sdk/evm
cargo fmt --check                       # clean
cargo clippy --locked --all-targets -- -D warnings   # clean
RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps   # clean
cargo test --locked -- --include-ignored   # 9 unit + 7 integration
```

The Solidity known-answer test loads
[`sdk/core/tests/fixtures/agreement-v1-vectors.json`](../sdk/core/tests/fixtures/agreement-v1-vectors.json)
in place and, for every vector, requires the Solidity decoder to reproduce the canonical bytes'
commitment, deal nullifier, and both authorization digests, and requires `ecrecover` to recover
the pinned buyer and seller keys from the pinned signatures. The shielded vector must fail
decoding. This is what pins the two languages together; without it, the Solidity suite would only
prove that Solidity is self-consistent.

The adapter tests (`sdk/evm/tests/local_chain.rs`) spawn `anvil` and prove the M3 completion
target:

- `one_authorized_agreement_settles_exactly_once`: prepare, estimate (allowance, balance, gas),
  submit, verify; recipient receives exactly the amount, fee recipient exactly the fee, buyer
  balance zero, receipt finalized with `agreement-bound-settlement`, delivery not started.
- `a_replay_is_rejected_on_chain`: the same authorized revision cannot settle twice; the contract
  has consumed the deal nullifier.
- `the_contract_rejects_mutated_terms_and_expiry`: a relayer that bypasses the adapter's local
  checks and mutates the amount, or presents an expired agreement, cannot settle.
- `prepare_rejects_terms_that_do_not_match_their_authorizations`: local failure with no RPC call,
  proven by an unreachable RPC URL.
- `an_unrelated_successful_receipt_is_not_settlement_evidence`: a successful payment to an
  unrelated address cannot produce an Erebus receipt.
- `the_rpc_chain_must_match_the_configured_chain`: the adapter rejects a live RPC chain-id
  mismatch before submission.
- `prepared_routing_fields_must_match_the_evidence`: changed routing fields cannot produce a
  transaction from valid backend evidence.

The Solidity suite adds direct-contract adversarial cases the adapter cannot exercise: replay,
mutated amount/recipient, swapped authorizations, wrong token, wrong namespace/contract/verifier
version, expiry, fee-recipient mismatch, unknown guarantee bits, truncation and trailing bytes,
insufficient allowance, fee-on-transfer rejection, and a reentrant token that attempts to settle
the same deal during `transferFrom` (the replay state is written before the external call, so the
nested attempt reverts and the whole transaction reverts).

The post-review cases also reject a constructor chain-id mismatch, scoped-disclosure claims, and
invalid UTF-8 text. The adapter verifies the live RPC chain, the exact transaction calldata, and
the complete settlement event before it reports payment success.

## What M3 does not claim

- Payment is **public**: amount, asset, payer, recipient, fee, and timing are visible on chain.
  This does not satisfy hidden-amount or hidden-recipient, and the adapter does not declare them.
- No shielded suite, proof system, or pool (M4/M5).
- No coordinator, durable operation journal, reconciliation loop, or relayer service (M6).
  `submit` and `verify` are separate calls; crash recovery between them is M6.
- No service delivery, no x402, no deployment automation beyond test helpers (M8).
- The Solidity decoder is narrower than the M1 asset grammar: only lowercase
  `eip155:<id>/erc20:0x<40 hex>` assets are accepted. An agreement outside that form fails
  loudly; see DM3-7.

## Friction

The protocol's 16-byte `deal_id` does not align to an EVM 32-byte word, and the first Solidity
decoder read 32 bytes for it. The known-answer test caught it immediately. See `docs/friction.md`
F45.
