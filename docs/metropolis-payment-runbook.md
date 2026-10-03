# Native Payment Driver

Status: local public-bound and shielded drivers on `metropolis`. M8 remains open.

`erebus-payment` continues an agreement from [native negotiation](metropolis-negotiation-runbook.md).
It does not construct new terms, sign agreement consent, or select another settlement mode.
Operator configuration selects public-bound or shielded payment. Neither path can downgrade the signed agreement.
For public-bound settlement, payment amounts and parties remain public.
The full accepted canonical agreement, blinding, and final signatures appear in settlement calldata.
This includes the signed service fields. Rejected offers and the offchain transcript remain private.

## Build

From the repository root:

```sh
cargo build --locked --manifest-path sdk/shielded/Cargo.toml --bin erebus-payment
sdk/shielded/target/debug/erebus-payment --help
```

The binary reads one JSON request from stdin.
The request names an operator configuration file and the retained negotiation operation.
Keys, deployment pins, fees, and RPC endpoints come from that configuration, not the request.

## Public-Bound Configuration

Use an owner-only regular JSON file. Private keys contain exactly 32 raw bytes.
Use absolute paths. The owner must control these paths and their parent directories.
Use the same buyer state directory and price policy as the negotiation command.

The following example uses placeholders, not deployment evidence:

```json
{
  "version": 1,
  "state_root": "/absolute/buyer/state",
  "namespace": "eip155:10143",
  "settlement_contract": "0x<40 lowercase hex digits>",
  "verifier_version": 1,
  "runtime_keccak256": "<64 lowercase hex digits>",
  "first_block": 0,
  "first_hash": "0x<64 lowercase hex digits>",
  "rpc_url": "https://primary.example",
  "peer_rpc_url": "https://independent.example",
  "buyer_address": "0x<40 lowercase hex digits>",
  "asset": "eip155:10143/erc20:0x<40 lowercase hex digits>",
  "signer_address": "0x<40 lowercase hex digits>",
  "signer_journal_root": "/absolute/operator/signer-journal",
  "transaction_key_file": "/absolute/operator/gas.key",
  "maximum_price": "70000000",
  "gas_limit": 500000,
  "max_fee_per_gas": "3000000000",
  "max_priority_fee_per_gas": "1000000000",
  "timeout_seconds": 15,
  "log_block_range": 100,
  "max_log_queries": 8,
  "max_ancestry": 64
}
```

The numeric examples are not measured Monad recommendations.
`maximum_price` includes the signed payment fee. Gas uses a separate operator budget.
`gas_limit * max_fee_per_gas` determines the required native funding, not the current gas price.

Use an independently verified deployment record for the runtime hash, first block, and first hash.
The driver requires matching code at that block and an explicit finalized anchor.
It requires no code at the previous block, so a later start cannot omit earlier payments.
This check supports immutable deployments, not proxies or upgradeable contracts.
The runtime hash includes the configured immutable values.

All payments from the same gas account must share `signer_journal_root`, including other buyer directories.
Use a dedicated gas account. Outside transactions can leave an unresolved nonce claim.
The gas key can differ from the buyer's agreement key.
Do not change the sender or fee policy after a transaction plan exists.

The two RPC endpoints must differ after URL normalization.
The operator must arrange independent providers. Different URLs alone do not prove independence.
Matching RPC reads are consistency evidence, not consensus proofs against malicious providers.

## Commands

Send these JSON requests to `erebus-payment` on stdin.
The operation reference is the nonzero 32-byte identity used during negotiation.

```json
{"method":"funding","config_file":"/absolute/payment.json","operation_ref":"<64 lowercase hex digits>"}
```

For public-bound settlement, `funding` checks token allowance, token balance, estimated gas, and the native gas budget.
It never signs or submits a transaction.
It can persist preparation, history checkpoints, and independently justified reconciliation.
It reports `funding_required` with decimal shortfalls, or `ready` when these checks pass.
An insufficient token balance or allowance returns zero estimated gas without a reverting estimate.

Funding remains external in this slice.
The operator supplies test tokens, approves the exact settlement deployment, and funds the gas payer.
No automatic mint, approval, or faucet request exists in this command.

```json
{"method":"settle","config_file":"/absolute/payment.json","operation_ref":"<64 lowercase hex digits>"}
```

Before the first transaction signature, the driver verifies the retained transcript and both agreement authorizations.
The coordinator persists preparation and the nonce claim before local transaction signing.
It persists the exact signing plan and signed transaction before any network submission.
The broadcast fence records uncertainty before the RPC request.

Once any broadcast attempt exists, every further `settle` invocation only observes.
This includes unknown, rejected, and acknowledged responses.
The driver does not automatically rebroadcast, replace fees, or create another transaction.
A crash between the broadcast fence and network I/O can therefore require explicit operator recovery.
The underlying M6 APIs support controlled recovery; this command does not expose that choice yet.
Do not delete the operation or journals to force another payment.

```json
{"method":"observe","config_file":"/absolute/payment.json","operation_ref":"<64 lowercase hex digits>"}
```

`observe` never submits and needs no transaction key.
It uses separate durable checkpoints for each RPC, starting at the authenticated deployment block.
Both providers must agree on completed evidence before reconciliation.
Different heights, timeouts, corruption, and partial history retain financial reservations.
There is no confirmation-count fallback when `finalized` is unavailable.

Finalized payment recovery needs no retained negotiation transcript.
It uses the coordinator's verified opening and chain evidence.
Nonce cleanup requires separate matching finalized nonce evidence.
A cleanup failure does not turn a verified payment into permission to submit again.

## Output

Exit code `2` means pending observation or funding required, not nonpayment.
Exit code `1` means an error; retain the operation and its reservations.
Exit code `0` means the named operation completed or the read-only funding check returned `ready`.
`closed_unpaid` requires finalized nonpayment evidence; local expiry alone is insufficient.

`payment_verified: true` requires the finalized winner to match this agreement commitment.
`delivery_verified` remains false. Use the [access client](metropolis-access-runbook.md) for resource retrieval.
The response includes separate local durations for deployment authentication, observation, funding, signing, and submission.
Shielded first preparation also reports artifact installation and local proof preparation time.
Proof preparation includes witness construction, proof generation, and local verification.
Local signing includes re-verifying the selected agreement against the retained transcript and the nonce-journal signing step.
These durations do not measure inclusion latency, finality latency, or delivery.

## Shielded Configuration

The suite-2 driver uses the same stdin requests, but a different strict operator configuration.
Use `mode: "shielded"`, not the public-bound configuration above.
Set these fields:

| Fields | Meaning |
|---|---|
| `version`, `mode` | `1` and `shielded` |
| `state_root`, `namespace`, `asset`, `maximum_price` | Same buyer state, chain, asset, and policy as negotiation |
| `pool`, `verifier_version` | Exact deployment named by the suite-2 agreement |
| `buyer_key_hex` | 128 lowercase hex digits for the authenticated BabyJubJub agreement key |
| `runtime_keccak256`, `first_block`, `first_hash` | Independently verified pool runtime and deployment origin |
| `poseidon_keccak256`, `deposit_verifier_keccak256`, `transfer_verifier_keccak256`, `withdraw_verifier_keccak256` | Trusted Keccak runtime hashes of the pool's immutable dependencies |
| `rpc_url`, `peer_rpc_url` | Distinct primary and peer endpoints |
| `wallet_file`, `wallet_key_file` | Existing encrypted note wallet and its owner-only 32-byte encryption key |
| `manifest_file`, `manifest_sha256`, `artifact_cache` | Owner-only release manifest, independently trusted digest, and local cache |
| `allow_test_artifacts`, `allow_loopback_http` | Development-only exceptions; both default to false |
| `signer_address`, `signer_journal_root`, `transaction_key_file` | Dedicated gas payer, shared nonce journal, optional local signing key |
| `gas_limit`, `max_fee_per_gas`, `max_priority_fee_per_gas` | Fixed operator gas and fee caps |
| `timeout_seconds`, `max_scan_blocks` | Per-request RPC timeout of 1-30 seconds; 1-1000 new blocks per provider scan |

Use absolute paths. Decimal amounts and fees are strings. Runtime hashes are 64 lowercase hex digits, optionally prefixed with `0x`.
The signed fee must be zero; the current private circuit does not support a relayer payment fee.
The pool runtime must exist at its deployment block and finalized anchor, and not at the preceding block.
The driver also checks the four immutable dependency runtimes at that finalized anchor.
These pins require independently reviewed deployment evidence; matching providers are not an audit.

Shielded `funding` replays finalized note history and checks a buyer-owned note and the gas budget.
It does not download artifacts, generate a proof, reserve a new input, sign, or submit.
`ready` means these funding checks passed, not that proving or a transaction simulation succeeded.

The first `settle` verifies the frozen negotiation, authenticates the manifest, and downloads checked artifacts.
It persists the selected input and random change opening in the encrypted wallet before proving.
Proof failures retain that choice. Retries cannot substitute another agreement, input, or zero-change opening.
Preparation and signing use the existing shielded SDK and shared coordinator, not a separate payment protocol.
The driver simulates the prepared pool call before signing and enforces the fixed gas limit.

Separate checkpoint files retain deal history and finalized wallet history for both RPCs.
Partial scans return pending and do not release reservations.
After an attempted broadcast, recovery needs neither transaction key, artifacts, manifest contents, nor negotiation transcript.
It still needs the wallet encryption key to replay finalized notes before coordinator reconciliation.
An expired unpaid deal retains the note reservation for explicit no-effect recovery; this command does not force-release it.
Local cache replay and wallet-event checks scale with retained history, even when new scans are bounded.

This driver does not create or fund a wallet, perform withdrawals, publish artifacts, or deploy a pool.
Prototype artifacts have known test entropy. Development opt-in is not a secure release ceremony.

Output excludes terms, amounts, authorization signatures, raw transactions, keys, and RPC response text.
Funding shortfalls and estimated gas are operator diagnostics; they reveal financial requirements to that operator.

## Local Evidence

From the repository root:

```sh
forge build --root contracts/evm
cargo test --locked --manifest-path sdk/shielded/Cargo.toml --test negotiation_cli payment_driver -- --include-ignored
```

The test copies binaries into a fresh directory and clears participant process environments.
Buyer and seller use separate agreement and transport keys.
They negotiate a data snapshot from 60 units to 70 units and authorize the same nonzero transcript root.
The copied payment command submits their actual agreement to Anvil.

The RPC proxy drops the broadcast acknowledgement after forwarding the transaction.
Further `settle` calls never submit again.
A fresh observer verifies finalized payment with the signing key and negotiation transcript unavailable.
The test rejects altered code, an incorrectly shifted deployment block, concurrent driver calls, and conflicting consumed-state reads.
Tiny paired history budgets yield pending checkpoints and resume to completed evidence.

The seller service persists issuance before a discarded response.
After a service restart, the buyer retrieves the same issuance and verifies the signed payload hash.
The buyer then restores content from its local cache with the agreement key and service unavailable.
A separate auditor verifies the encrypted grant and finalized payment with participant directories offline.
Exactly one `eth_sendRawTransaction` request passes through the payment proxy during the complete workflow.

This is local public-bound evidence, not a new live Monad run or shielded product completion.
The local proxy and upstream are one test node, not independent production providers.
Installed Python/MCP composition, native shielded proving and funding, released packages, and hosted endpoints remain open.
