# Erebus EVM settlement contracts

Public-bound settlement for one authorized Erebus agreement (Metropolis M3). The normative
description is [docs/metropolis-m3-decisions.md](../../docs/metropolis-m3-decisions.md); the Rust
adapter that drives these contracts is [sdk/evm](../../sdk/evm).

## What `ErebusSettlement` does

`settle(terms, blinding, buyerSignature, sellerSignature, token)`:

1. Decodes the canonical agreement terms strictly (truncation, trailing bytes, unknown tags,
   out-of-range lengths, and invalid values all revert).
2. Requires the configured chain id to equal `block.chainid` and derives the chain namespace.
3. Requires the decoded domain to match the namespace, contract, and verifier version.
4. Requires `block.timestamp < expiry`.
5. Recomputes `Cdeal = keccak256("EREBUS_DEAL_COMMITMENT_V1" || terms || blinding)` and verifies
   both role authorizations against it with `ecrecover`, requiring raw recovery ids and low-`s`.
6. Recomputes `Ndeal`, requires it unused, and marks it consumed **before** any external call.
7. Resolves the token from the asset identifier, requires it to match the supplied address, and
   transfers `amount` and `fee` from the buyer with an exact balance-delta check.

Payment amount, asset, payer, recipient, and fee are public. This is not shielded settlement.

## Layout

| Path | Purpose |
|---|---|
| `src/ErebusCodec.sol` | Canonical decoder and the three digests |
| `src/ErebusSettlement.sol` | The settlement contract |
| `src/MockERC20.sol` | Test token, with a fee-on-transfer mode for the negative test |
| `src/MockReentrantERC20.sol` | Token that re-enters `settle` during `transferFrom` |
| `test/ErebusVectors.t.sol` | Known-answer test against the Rust agreement vectors |
| `test/ErebusSettlement.t.sol` | Happy path and adversarial matrix |

There are no git submodules and no forge-std dependency; the tests declare a minimal `Vm`
cheatcode interface.

## Commands

```bash
forge build
forge test -vvv
forge fmt --check
```

The known-answer test reads
`../../sdk/core/tests/fixtures/agreement-v1-vectors.json` in place, so regenerate the vectors in
`sdk/core` rather than copying them here.
