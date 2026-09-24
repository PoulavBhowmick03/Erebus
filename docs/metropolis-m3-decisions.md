# Metropolis M3 Decisions: EVM Chain Adapter and Public-Bound Settlement

Written: 2026-09-21. Scope: roadmap M3, public-bound EVM settlement.
Status: implementation decision record. Contracts live in [`contracts/evm`](../contracts/evm);
the adapter is [`sdk/evm`](../sdk/evm).

M3 turns one authorized canonical agreement into exactly one on-chain payment. Payment amount,
asset, payer, and recipient are public in this mode. That is the deliberate trade: it validates
the whole architecture — authorized agreement, atomic payment, normalized receipt — without
claiming payment privacy. Shielded settlement remains M4/M5 and is not implied by this record.

Related: [architecture](metropolis-architecture.md) sections 5-6, [agreement](metropolis-agreement.md),
[decisions](metropolis-decisions.md) D01-D04, and the D07 M3 gate.

## DM3-1. Toolchain

- **Contracts:** Foundry 1.5.1 with `solc = "0.8.24"` pinned in `foundry.toml`. No git
  submodules and no forge-std: the tests declare a minimal `Vm` cheatcode interface, so the
  checkout stays self-contained and reviewable.
- **Rust:** `alloy` 1.8 (JSON-RPC, local signing, nonce and gas filling, receipts). It is the
  maintained successor to ethers-rs and the only new heavy dependency.
- **Local chain:** `anvil` 1.5.1, spawned by the adapter tests and by the `evm` CI job.

## DM3-2. Contract boundary: the contract re-derives, it does not trust the adapter

`PreparedSettlement` carries routing and identity, not payment terms (see the M1 boundary). So
the EVM evidence is:

```text
SettlementEvidence = { terms: canonical bytes, blinding: [32], buyer_signature, seller_signature }
```

The contract decodes the canonical terms itself, recomputes
`Cdeal = keccak256("EREBUS_DEAL_COMMITMENT_V1" || terms || blinding)`, verifies both role
authorizations against `Cdeal`, derives `Ndeal` from the domain, buyer key, and settlement nonce,
and only then moves tokens. A relayer that mutates any payment field produces a different
commitment or a failed signature and the transaction reverts. The adapter's local checks are a
usability layer; they are not the enforcement point.

The adapter also treats `PreparedSettlement` as untrusted persisted data. Before submission, it
recomputes the commitment and nullifier from the evidence. It then compares the domain, mode,
and required guarantees with the routing fields. A mismatch stops before transaction creation.

`computeCommitment` and `computeDealNullifier` are exposed as pure views so a caller can
cross-check the adapter against the chain.

## DM3-3. Supported token behavior

- ERC-20 `transferFrom` with either the empty or the 32-byte-`true` return convention is
  accepted (SafeERC20 semantics).
- **Exact balance delta is required:** the recipient's balance must increase by exactly the
  authorized value. Fee-on-transfer, rebasing, and any other short-delivery token is rejected.
  A payment that arrives short is not the authorized payment.
- The token address must have code. `payment_recipient` and the token must be non-zero.
- The contract makes no arbitrary calls and holds no funds between transactions.

## DM3-4. Signing scheme

Suite 1 only: keccak256 digests with secp256k1 ECDSA.

- Authorization digest: `keccak256("EREBUS_DEAL_AUTHORIZATION_V1" || domainBytes || roleTag || Cdeal)`,
  with role tags 1 (buyer) and 2 (seller). The domain is the exact canonical domain slice out of
  the terms bytes, so a signature for another deployment cannot verify.
- Signature format: 65 bytes `r || s || v` with `v` the raw recovery id `0` or `1`. The contract
  requires `v <= 1`, non-zero `r` and `s`, low-`s` (`s <= secp256k1n/2`), and `ecrecover` to
  return the role's key.
- The buyer key is both the authorizing identity and the payer of the transfer; the seller key
  authorizes but need not hold funds.

## DM3-5. Fees

The fee and its recipient are committed fields of the agreement, so a changed fee changes the
commitment and invalidates both authorizations. The fee recipient must be present exactly when
the fee is non-zero. The buyer's allowance must cover `amount + fee`; the adapter's `estimate`
reports the required allowance, the current allowance and balance, both shortfalls, and the gas
estimate before submission.

## DM3-6. Deployment identity and administration

The contract has no owner, no upgrade path, no pause, and no privileged function. Its only
configuration is the constructor: the numeric chain id and the verifier version. The constructor
requires the configured chain id to equal `block.chainid`. The contract derives the canonical
`eip155:<chainId>` namespace from this value. The adapter also compares `eth_chainId` with its
deployment configuration before each network operation. The contract repeats the chain-id check
for each settlement, so a later chain-id change cannot reactivate an old deployment domain.

`consumedDeals` is write-once per deal nullifier. A protocol change requires a new deployment.
The new domain invalidates old authorizations. Mainnet activation remains a separate release
decision (D01).

## DM3-7. Asset identifier form

The accepted asset identifier is exactly `eip155:<chainId>/erc20:0x<40 lowercase hex>`, with the
chain id equal to the deployment namespace. Uppercase hex, other asset namespaces, wrong chains,
and short references are rejected by both the adapter and the contract. Ambiguous aliases for the
same token address are a protocol bug, so only one form is accepted.

This is narrower than the M1 asset grammar (`[-.%A-Za-z0-9]{1,128}`). An agreement whose asset
is outside this form fails loudly at the adapter or the contract; it is never silently
normalized.

## DM3-8. What is public, and what guarantees are claimed

| Public | Hidden |
|---|---|
| amount, asset, payer, payment recipient, fee and fee recipient, timing, contract, function | negotiation, unshared terms, other deals |

The adapter declares exactly one guarantee: `agreement-bound-settlement`. It does not declare
`hidden-amount` or `hidden-recipient`, and it does not declare `scoped-disclosure`. A deal that
requires a guarantee this backend lacks fails selection. The contract also rejects every
guarantee bit except `agreement-bound-settlement`, so a direct caller cannot bypass this rule.

## DM3-9. Finality and the receipt

`verify` requires the exact transaction destination and calldata that the prepared evidence
produces. A successful transaction must also contain one matching `DealSettled` event from the
configured contract. The event must match the commitment, nullifier, buyer, recipient, token,
amount, and fee.

Inclusion is `Finality::Included`. It becomes `Finalized` after the configured confirmations.
The default is one confirmation, and a local chain can use zero. A reverted matching transaction
has `PaymentStatus::Reverted`. Delivery is always `NotStarted` because payment is not delivery.

## DM3-10. What this record does not establish

- No shielded settlement, no proof system, no pool (M4/M5).
- No coordinator, durable operation journal, reconciliation, or relayer (M6). `submit` returns a
  transaction hash and `verify` is a separate call; a caller that crashes between them must
  reconcile, and that is M6 work.
- No service delivery and no x402 composition.
- No deployment automation beyond the test helpers; M8 owns reproducible testnet deployment.
- No cross-chain behavior. The domain binds one deployment and does not promise global deal
  uniqueness.

## DM3 decisions for owner review

| # | Decision | Rationale |
|---|---|---|
| DM3-A | Contract re-derives the commitment from the terms opening | The adapter is not a trusted component; enforcement must be on chain |
| DM3-B | Exact-balance-delta token check | Rejects fee-on-transfer and rebasing tokens, which would otherwise deliver less than authorized |
| DM3-C | Raw recovery id in signatures, low-`s` enforced | Matches the M1 suite-1 format and rejects malleable signatures |
| DM3-D | No owner, no upgrade, no pause | Smallest administration surface for a testnet release; changes are new deployments |
| DM3-E | Asset form narrowed to lowercase `0x` addresses | One textual form per token; no silent aliasing |
| DM3-F | Contract and RPC both verify the live chain id | A configured namespace string alone does not stop cross-chain replay |
| DM3-G | Receipt verification matches calldata and the settlement event | An unrelated successful transaction is not settlement evidence |
| DM3-H | Unsupported guarantees fail in the adapter and contract | A direct contract call cannot bypass backend capability selection |
