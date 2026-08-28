# Packaged Protocol 4 recovery canary on Sepolia

Run date: 2026-08-27

This run used three wheels built from the current checkout and installed into an empty
virtual environment. The installed CLI reported Protocol 4 and wire v3. The source
manifests still reported version `0.1.0`. Thus, this is packaged-current-source evidence.
It is not evidence for the published Protocol 2 `v0.1.0` artifacts or a published
`v0.2.0` release.

The run used two existing test identities and one existing wire-v3 channel pair on
Starknet Sepolia. It used low-value offers of 0.0001 STRK. No mainnet account or mainnet
prover was used.

## Prover failure and fix

The first packaged attempt returned JSON-RPC `-32603 Internal error`. The prover response
data contained `INVALID_SIGNATURE` after the Rust client preserved the error data.

The proof invocation used `signer.address()` as a private scalar. This regression entered
when the client moved behind `AccountSigner`. The fix now sends the unsigned invocation to
`AccountSigner::sign`. The same failed proposal operation then resumed through simulation,
proving, fee estimation, and submission.

The proposal transaction was:

`0xfc747...a9f3`

Its accepted block timestamp was `1787853770`.

## Exact signed-transaction resubmission

Operation:

`op_2727272727272727272727272727272727272727272727272727272727272727`

A local fault proxy held the signed transaction after Rust had persisted it. The stored
transaction was 318,927 bytes and had mode `0600`. The first process stopped at the
submission boundary.

Reconciliation reported `pending` and `wait`. It found this transaction hash:

`0x53e1018511d279f73dca58963081ad121f1b96a756fc9b5db968e46542e4f55`

Explicit resume sent the stored transaction bytes without rebuilding them. A later receipt
read returned Starknet RPC code 24, `Block not found`. The journal stayed at `submitted`.
Reconciliation then found the chain effect and supplied the local commit action. The final
resume returned `already_complete`.

The accepted block timestamp was `1787854050`. No second transaction was created.

## Expired-proof rebuild

Operation:

`op_2828282828282828282828282828282828282828282828282828282828282828`

The fault proxy held a 311,753-byte signed transaction with mode `0600`. The original
attempt used proving block `14140358` and expiry block `14140808`. Its transaction hash
was:

`0x55bde2f58f5f29a2618cf354b1145ad37139bf7f2a36b4ec7cd8a9eb17c9c07`

The test waited until the Sepolia head passed block `14140808`. Reconciliation alone kept
the operation at `pending`, because the account nonce did not yet prove that the attempt
was dead. Explicit resume combined the nonce result with the expired proof window. It then
rebuilt the operation under the same operation ID.

The rebuilt attempt used proving block `14140865` and produced this transaction:

`0x611b8250dde4199d18c96936d0aeccdddd3b8be6719a00512ae195eb31b987e`

Settlement selected `649900000000000000` base units, paid `100000000000000`, and returned
`649800000000000000` as change. The final journal had two attempts and one committed
effect. Its accepted block timestamp was `1787855151`.

## Spending-cap projection

The MCP `reconcile` tool ran from the same clean wheel installation. It projected both
settlements from the Rust journal into the Python spending ledger as committed operations.
It used each Starknet block timestamp for the UTC day.

The old version-1 spending file also contained 1.6 STRK that could not be attributed to an
operation. Migration retained this amount as `legacy_reserved`. This is intentionally
fail-closed. An operator must audit old records before it removes that reservation.

## Checks

- Rust workspace: 351 passed; two live-prover tests ignored.
- Python workspace: 154 passed.
- TypeScript oracle: 43 passed.
- The packaged MCP `reconcile` response used the seam backend and Sepolia network ID.
- Both stored fault-proxy payloads had mode `0600`.

## Boundaries

This run proves both Protocol 4 resume paths through a clean local package installation on
Sepolia. It also proves the Rust-to-MCP cap projection for accepted settlements. It does
not prove a published `v0.2.0` artifact, external-operator installation, mainnet behavior,
independent review, production custody, or private prover and RPC operation. The configured
prover and write RPC could still read the pool private key.

The final source adds two local hardening changes after the live transactions. Prover error
text now emits only reviewed diagnostic labels, because raw diagnostic data can contain the
pool key. Cap reconciliation also stops if it cannot reconstruct an accepted settlement's
terms. These changes do not alter either recovery path. Their tests are included in the
counts above.
