# Metropolis Operations

Consolidated operational guide for the Metropolis branch: installed product commands, operator
configuration, recovery boundaries, and the live Monad evidence. It merges the access, payment,
negotiation, local-proving, self-hosting, Monad rehearsal, M6, M7, and M8 runbooks into one
document.

Status at consolidation (2026-10-05): `0.3.0.dev4` is published on the separate Metropolis
package index (2026-10-04); the local package path was verified on 2026-10-03. No hosted
deployment exists. Both payment rails have one **team-operated** live Monad testnet payment
(§10); team-operated evidence is not third-party acceptance. Hosted services, Render
provisioning, Linux platform qualification, and independent external acceptance remain open.
M8 is not complete. The public-bound settlement contract is deployed on Monad testnet; no secure
Monad shielded pool deployment is claimed by this document.

Component status lines carried over from the merged runbooks stay with their sections where they
remain accurate.

Related documents: [architecture](metropolis-architecture.md),
[agreement format](metropolis-agreement.md), [threat model](metropolis-threat-model.md),
[roadmap](metropolis-roadmap.md),
[published-package install guide](metropolis-install.md#10-published-metropolis-packages), and
[friction](friction.md).

## Table of contents

1. [Overview](#1-overview)
2. [Configuration and keys](#2-configuration-and-keys)
3. [Buyer payment and recovery](#3-buyer-payment-and-recovery)
4. [Seller access service](#4-seller-access-service)
5. [Negotiation command](#5-negotiation-command)
6. [Disclosure and auditor operations](#6-disclosure-and-auditor-operations)
7. [Local proving](#7-local-proving)
8. [Shared services and self-hosting](#8-shared-services-and-self-hosting)
9. [Render preparation](#9-render-preparation)
10. [Live Monad rehearsal](#10-live-monad-rehearsal)
11. [Failure recovery](#11-failure-recovery)
12. [Security and exposure notes](#12-security-and-exposure-notes)

---

## 1. Overview

Erebus settles agent agreements on EVM under two rails:

- **public-bound** — `ErebusSettlement`; the buyer pays gas. The accepted agreement, blinding,
  amount, parties, and final signatures are public in settlement calldata.
- **x402-exact** — canonical Permit2 `PermitWitnessTransferFrom` through
  `x402ExactPermit2Proxy`; the seller's access service pays gas. Public exposure is listed in
  §12.

Shielded settlement is a separate, prototype-only driver (`mode: "shielded"`) for suite-2
agreements. It is not a live product deployment.

Installed commands used by operators and the rehearsal:

| Command | Role |
|---|---|
| `erebus-negotiate` | Deterministic price negotiation and durable consent for both settlement modes |
| `erebus-payment` | Continues a retained negotiation into public-bound or shielded payment |
| `erebus-settle` | Read-only public-bound `capabilities`, `funding`, `receipt`, `address` |
| `erebus-access-service` | Seller-side HTTP access service |
| `erebus-access` | Buyer-side retrieval command |
| `erebus-disclosure`, `erebus-shielded-disclosure` | Disclosure export and independent auditor verification |
| `erebus-local-prove`, `erebus-artifacts` | Local proving and authenticated artifact installation |
| `erebus-network-check` | Chain compatibility self-check |
| `erebus-selfhost` | Self-hosting launcher |
| `erebus-tx-relayer`, `erebus-relay`, `erebus_pool_indexer` | Transaction relayer, message relay, pool indexer |
| `erebus-mcp-server` | MCP surface for agents |

The call path is `agents → mcp-server → sdk/py → sdk/rs → chain`. Python forwards paths; Rust
owns the protocol and key handling. Key values never cross the Python boundary.

---

## 2. Configuration and keys

### File and path discipline

- Configurations, key files, evidence files, and witnesses are **regular, owner-only files**.
  Directories that hold them (`evidence_root`, witness directories, artifact caches, state
  roots) are owner-only; the access evidence directory must be a real directory with mode `0700`.
- Use absolute paths. The owner must control the paths and their parent directories.
- Private keys contain exactly **32 raw bytes** unless a command's schema says otherwise. Some
  key files also accept a 64-digit hexadecimal encoding (`erebus-access` buyer key); the shielded
  payment config uses `buyer_key_hex` (128 lowercase hex digits, authenticated BabyJubJub
  agreement key). Key **paths** cross requests; key values do not.
- The access service config must not contain private signing keys. `seller_key` there is the full
  authorization key bytes; the placeholder `[1]` is not a valid key.
- Requests name an operator config file and an operation reference; keys, deployment pins, fees,
  and RPC endpoints come from the config, not the request.
- Never commit, upload, or attach key material, seeds, bearer tokens, grants, signed transaction
  bytes, coordinator snapshots, wallet files, transcripts, or agreement openings. Paths to key
  files are fine in operator documentation.

### Shared state rules

- One state root per buyer.
- One signer journal per chain and gas account, shared by every process using that account.
  Never point a relayer or payment state root at the buyer's wallet or coordinator.
- Back up the whole state root, including the initialization marker. Back up access
  `state_root`, private evidence, and payload storage before endpoint migration.
- All participants must recover the same frozen transcript root before further negotiation,
  authorization, or settlement.

### Observation budget (shared by buyer, seller, and auditor)

The current live observation configuration, as used by the rehearsal harness
(`scripts/metropolis-monad-rehearsal.py`) and accepted by `erebus-payment`, `erebus-settle`,
the access service, and the disclosure verifier:

| Field | Current live value | Bounds | Meaning |
|---|---|---|---|
| `log_block_range` | `100` | fixed by public Monad RPC | blocks per `eth_getLogs` query |
| `max_log_queries` | `256` | 1..=1024 | log queries per invocation; bounds each invocation |
| `max_concurrent_queries` | `8` | 1..=32 | in-flight `eth_getLogs` ranges per verification slice; sample payment/access configs show `16` |
| `max_ancestry` | `8192` | 1..=8192 | parent links per invocation; sample configs show `64` |

Rules that do not change with concurrency:

- The same contiguous 100-block ranges and canonical blocks are read; no range is skipped.
- A slice checkpoints only after **every** concurrent query in its window succeeds. An error or
  interruption resumes from the same start block and never treats missing data as absence.
- Concurrency changes wall-clock time only, never which ranges are scanned.
- Ancestry windows verify the same parent-hash and timestamp linkage locally; the
  per-invocation budget still bounds the call.
- Size `max_log_queries` so one slice fits the caller's deadline (the access request is valid
  for two minutes) and keep `max_concurrent_queries` within the provider's rate limits.
- `max_ancestry` is set high so an ancestry catch-up outruns the moving tip.
- Authentication, ancestry catch-up, retries, seller verification, and auditor verification are
  separate and not included in scan estimates.

**Measured:** against the Monad public RPC, 48 `eth_getLogs` queries took 51.7 s sequentially and
6.4 s at concurrency 16 (8.0x). Extrapolated to the live range that is about 21 minutes of
log-query time, not an end-to-end guarantee.

### Lifetimes and scan estimation

| Field | Default | Rule |
|---|---|---|
| `agreement_lifetime_seconds` (older name `delivery_window_seconds` accepted) | 6 h harness default; example plan `14400` | `erebus-negotiate` sets the payment expiry to negotiation time plus this value. It must exceed `verification_timeout_seconds` by at least one hour. `init`, `preflight`, and `run` reject a shorter value; `preflight` also rejects a participant config whose stored lifetime differs from the plan. A mismatch means the operator must initialize a replacement; the harness never edits a config to extend an agreement. |
| `grant_lifetime_seconds` | `3600`; example plan `7200` | Must cover the auditor's cold scan plus a margin (harness: estimated scan + 600 s). The harness exports the grant with that lifetime. |
| `verification_timeout_seconds` | up to `86400` | Bounds the whole verification phase. Never lower it below the remaining scan work. |

Harness scan estimation (pessimistic): each 100-block query is priced at 2 s,
`queries = ceil((height - deployment_block) / 100)`, and
`estimated = ceil(queries * 2 / max_concurrent_queries)`. `preflight` reads the live finalized
height and rejects a plan when:

- one observation slice (`100 * max_log_queries` blocks) cannot fit the 120-second access
  request (lower `max_log_queries`);
- `agreement_lifetime_seconds < estimated + 900` s (settlement margin);
- the delivery deadline leaves less than `2 * estimated + 900` s (buyer and seller scans);
- `grant_lifetime_seconds < estimated + 600` s (audit margin).

Never reset retained checkpoints to "start over" — a later start could omit a payment.

### Test-only exceptions

`allow_test_artifacts` and `allow_loopback_http` default to `false` in strict configs.
Local tests may set `allow_test_artifacts: true` for development fixtures and
`allow_loopback_http: true` for literal loopback HTTP. `EREBUS_ALLOW_LOOPBACK_ACCESS_HTTP=1`
does the same for the access MCP mode. Do not enable these exceptions for a secure release.
Prototype artifacts have known test entropy; development opt-in is not a secure release
ceremony.

---

## 3. Buyer payment and recovery

Status: local public-bound and shielded drivers on `metropolis`. M8 remains open.

`erebus-payment` continues an agreement from native negotiation (§5). It does not construct new
terms, sign agreement consent, or select another settlement mode. Operator configuration selects
public-bound or shielded payment. **Neither path can downgrade the signed agreement.**

For public-bound settlement, payment amounts and parties remain public. The full accepted
canonical agreement, blinding, and final signatures appear in settlement calldata. This includes
the signed service fields. Rejected offers and the offchain transcript remain private.

### Build

From the repository root:

```sh
cargo build --locked --manifest-path sdk/shielded/Cargo.toml --bin erebus-payment
sdk/shielded/target/debug/erebus-payment --help
```

The binary reads one JSON request from stdin. The request names an operator configuration file
and the retained negotiation operation.

### Public-bound configuration

Use an owner-only regular JSON file. Private keys contain exactly 32 raw bytes. Use absolute
paths. The owner must control these paths and their parent directories. Use the same buyer state
directory and price policy as the negotiation command.

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
  "max_log_queries": 256,
  "max_ancestry": 64,
  "max_concurrent_queries": 16
}
```

- The numeric examples are not measured Monad recommendations.
- `maximum_price` includes the signed payment fee. Gas uses a separate operator budget.
- `gas_limit * max_fee_per_gas` determines the required native funding, not the current gas
  price.
- Use an independently verified deployment record for the runtime hash, first block, and first
  hash. The driver requires matching code at that block and an explicit finalized anchor. It
  requires no code at the previous block, so a later start cannot omit earlier payments. This
  check supports immutable deployments, not proxies or upgradeable contracts. The runtime hash
  includes the configured immutable values.
- All payments from the same gas account must share `signer_journal_root`, including other buyer
  directories. Use a dedicated gas account. Outside transactions can leave an unresolved nonce
  claim. The gas key can differ from the buyer's agreement key. Do not change the sender or fee
  policy after a transaction plan exists.
- The two RPC endpoints must differ after URL normalization. The operator must arrange
  independent providers. Different URLs alone do not prove independence. Matching RPC reads are
  consistency evidence, not consensus proofs against malicious providers.

### Commands

Send these JSON requests to `erebus-payment` on stdin. The operation reference is the nonzero
32-byte identity used during negotiation.

```json
{"method":"funding","config_file":"/absolute/payment.json","operation_ref":"<64 lowercase hex digits>"}
```

For public-bound settlement, `funding` checks token allowance, token balance, estimated gas, and
the native gas budget. It never signs or submits a transaction. It can persist preparation,
history checkpoints, and independently justified reconciliation. It reports `funding_required`
with decimal shortfalls, or `ready` when these checks pass. An insufficient token balance or
allowance returns zero estimated gas without a reverting estimate.

Funding remains external in this slice. The operator supplies test tokens, approves the exact
settlement deployment, and funds the gas payer. No automatic mint, approval, or faucet request
exists in this command.

```json
{"method":"settle","config_file":"/absolute/payment.json","operation_ref":"<64 lowercase hex digits>"}
```

- Before the first transaction signature, the driver verifies the retained transcript and both
  agreement authorizations.
- The coordinator persists preparation and the nonce claim before local transaction signing. It
  persists the exact signing plan and signed transaction before any network submission. The
  broadcast fence records uncertainty before the RPC request.
- **Once any broadcast attempt exists, every further `settle` invocation only observes.** This
  includes unknown, rejected, and acknowledged responses. The driver does not automatically
  rebroadcast, replace fees, or create another transaction.
- A crash between the broadcast fence and network I/O can therefore require explicit operator
  recovery. The underlying M6 APIs support controlled recovery; this command does not expose
  that choice yet.
- **Do not delete the operation or journals to force another payment.**

```json
{"method":"observe","config_file":"/absolute/payment.json","operation_ref":"<64 lowercase hex digits>"}
```

- `observe` never submits and needs no transaction key.
- It uses separate durable checkpoints for each RPC, starting at the authenticated deployment
  block. Both providers must agree on completed evidence before reconciliation.
- Different heights, timeouts, corruption, and partial history retain financial reservations.
- There is no confirmation-count fallback when `finalized` is unavailable.
- Finalized payment recovery needs no retained negotiation transcript. It uses the coordinator's
  verified opening and chain evidence.
- Nonce cleanup requires separate matching finalized nonce evidence. A cleanup failure does not
  turn a verified payment into permission to submit again.

### Output and exit codes

| Exit | Meaning |
|---|---|
| `0` | The named operation completed, or the read-only funding check returned `ready`. |
| `1` | An error; retain the operation and its reservations. |
| `2` | Pending observation or funding required, **not nonpayment**. |

`closed_unpaid` requires finalized nonpayment evidence; local expiry alone is insufficient.
`payment_verified: true` requires the finalized winner to match this agreement commitment.
`delivery_verified` remains false. Use the [access client](#4-seller-access-service) for resource
retrieval.

The response includes separate local durations for deployment authentication, observation,
funding, signing, and submission. Shielded first preparation also reports artifact installation
and local proof preparation time. Proof preparation includes witness construction, proof
generation, and local verification. Local signing includes re-verifying the selected agreement
against the retained transcript and the nonce-journal signing step. These durations do not
measure inclusion latency, finality latency, or delivery.

### Shielded configuration

The suite-2 driver uses the same stdin requests, but a different strict operator configuration.
Use `mode: "shielded"`, not the public-bound configuration above. Set these fields:

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

- Use absolute paths. Decimal amounts and fees are strings. Runtime hashes are 64 lowercase hex
  digits, optionally prefixed with `0x`.
- The signed fee must be zero; the current private circuit does not support a relayer payment
  fee.
- The pool runtime must exist at its deployment block and finalized anchor, and not at the
  preceding block. The driver also checks the four immutable dependency runtimes at that
  finalized anchor.
- These pins require independently reviewed deployment evidence; matching providers are not an
  audit.
- Shielded `funding` replays finalized note history and checks a buyer-owned note and the gas
  budget. It does not download artifacts, generate a proof, reserve a new input, sign, or submit.
  `ready` means these funding checks passed, not that proving or a transaction simulation
  succeeded.
- The first `settle` verifies the frozen negotiation, authenticates the manifest, and downloads
  checked artifacts. It persists the selected input and random change opening in the encrypted
  wallet before proving. Proof failures retain that choice. Retries cannot substitute another
  agreement, input, or zero-change opening.
- Preparation and signing use the existing shielded SDK and shared coordinator, not a separate
  payment protocol. The driver simulates the prepared pool call before signing and enforces the
  fixed gas limit.
- Separate checkpoint files retain deal history and finalized wallet history for both RPCs.
  Partial scans return pending and do not release reservations.
- After an attempted broadcast, recovery needs neither transaction key, artifacts, manifest
  contents, nor negotiation transcript. It still needs the wallet encryption key to replay
  finalized notes before coordinator reconciliation.
- An expired unpaid deal retains the note reservation for explicit no-effect recovery; this
  command does not force-release it.
- Local cache replay and wallet-event checks scale with retained history, even when new scans are
  bounded.
- This driver does not create or fund a wallet, perform withdrawals, publish artifacts, or deploy
  a pool. Prototype artifacts have known test entropy. Development opt-in is not a secure
  release ceremony.
- Output excludes terms, amounts, authorization signatures, raw transactions, keys, and RPC
  response text. Funding shortfalls and estimated gas are operator diagnostics; they reveal
  financial requirements to that operator.

### Local evidence

From the repository root:

```sh
forge build --root contracts/evm
cargo test --locked --manifest-path sdk/shielded/Cargo.toml --test negotiation_cli payment_driver -- --include-ignored
```

The test copies binaries into a fresh directory and clears participant process environments.
Buyer and seller use separate agreement and transport keys. They negotiate a data snapshot from
60 units to 70 units and authorize the same nonzero transcript root. The copied payment command
submits their actual agreement to Anvil.

The RPC proxy drops the broadcast acknowledgement after forwarding the transaction. Further
`settle` calls never submit again. A fresh observer verifies finalized payment with the signing
key and negotiation transcript unavailable. The test rejects altered code, an incorrectly
shifted deployment block, concurrent driver calls, and conflicting consumed-state reads. Tiny
paired history budgets yield pending checkpoints and resume to completed evidence.

The seller service persists issuance before a discarded response. After a service restart, the
buyer retrieves the same issuance and verifies the signed payload hash. The buyer then restores
content from its local cache with the agreement key and service unavailable. A separate auditor
verifies the encrypted grant and finalized payment with participant directories offline. Exactly
one `eth_sendRawTransaction` request passes through the payment proxy during the complete
workflow.

At the payment-driver checkpoint this was local public-bound evidence, not a live Monad run or
shielded product completion. The local proxy and upstream are one test node, not independent
production providers. The live Monad evidence is in §10; installed Python/MCP composition,
native shielded proving and funding, and hosted endpoints remain open.

---

## 4. Seller access service

The binaries ship in the published `erebus-cli` wheel (`0.3.0.dev4`); the build commands below
are for unreleased source changes. There is no hosted deployment.

### Build and run

From the repository root:

```sh
cargo build --locked --manifest-path sdk/shielded/Cargo.toml --bin erebus-access-service
EREBUS_ACCESS_CONFIG=/absolute/path/access-config.json sdk/shielded/target/debug/erebus-access-service
```

The service listens on `127.0.0.1` only. Put a TLS gateway in front before remote access.
The configuration file must be a regular, owner-only file. The evidence directory must be a real
directory with mode `0700`.

### Configuration

Public-bound configuration shape:

```json
{
  "service_id": [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
  "seller_key": [1],
  "suite_id": 1,
  "resource": "dataset.snapshot.v1",
  "payload_file": "/absolute/path/dataset.snapshot",
  "evidence_root": "/absolute/path/evidence",
  "state_root": "/absolute/path/access-state",
  "port": 8080,
  "backend": {
    "mode": "public_bound",
    "namespace": "eip155:10143",
    "settlement_contract": "0xa5f0c864f434331bef9a7fc5e05450d598d24da4",
    "verifier_version": 1,
    "rpc_url": "https://testnet-rpc.monad.xyz",
    "from_block": 67495473,
    "log_block_range": 100,
    "max_log_queries": 256,
    "max_ancestry": 64,
    "max_concurrent_queries": 16
  }
}
```

Replace `seller_key` with the full authorization key bytes. The placeholder `[1]` is not a valid
key. Choose a stable service identity independently of individual agreements. Do not put private
signing keys in this configuration. `max_concurrent_queries` (1..=32, default 8) bounds in-flight
`eth_getLogs` ranges per verification slice; it changes wall-clock time only, never which ranges
are scanned. A cold scan continues across buyer retries from the same durable checkpoint. Size
one slice to fit the two-minute access request so the first request counts.

For shielded settlement, replace `backend` with:

```json
{
  "mode": "shielded",
  "chain_id": 10143,
  "pool": "<authenticated pool address>",
  "rpc_url": "<first provider URL>",
  "peer_rpc_url": "<second provider URL>",
  "first_block": 0,
  "first_hash": [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
}
```

Use the authenticated deployment block and hash, not the placeholders. No secure Monad shielded
deployment is claimed by this document.

### Prepare a resource

Export the selected agreement from durable participant state. Place its encoded evidence at
`<evidence_root>/<lowercase commitment>.evidence` with mode `0600`. Keep this directory private.
Evidence contains the agreed terms and transcript.

The signed service record must name:

- The configured resource and seller.
- Quantity `1` and unit `snapshot`.
- Fulfillment method `http-access-v1`.
- SHA-256 of the exact payload bytes.
- The buyer agreement key as `access_recipient`.

The current service supports payloads up to 1 MiB and 16 concurrent requests. It does not
implement metered usage, streaming, refunds, or delegated access keys.

### Retrieve and recover

- `GET /healthz` reports process availability, not verified RPC health.
- Send an `AccessRequest` to `POST /v1/access`. Use `access::request_digest` and sign it with the
  buyer agreement key. The request contains the commitment, nonce, expiry, and signature. Do not
  send the agreement opening or a private key over HTTP.
- HTTP `202` means observation is pending. HTTP `200` returns the payload and durable issuance
  identity. Authentication failures return `401`; agreement failures return `400`. Storage or
  observation failures return `503`.
- Retry retrieval with a fresh valid request. Do not create another payment.
- Keep `state_root`, private evidence, and payload storage persistent across restarts. Back up
  all three before endpoint migration.
- The service requires finalized chain observation even when issuance already exists. Provider
  outages can therefore delay retrieval of a previously issued resource. This applies to
  seller-side retrieval. The completed buyer cache works offline.
- The service sees the buyer identity, selected agreement, request timing, and requested
  resource. The gateway can also see client network metadata. Self-custody does not hide these
  facts from the seller or guarantee endpoint availability.

Access authentication and delivery limits:

- The owner selected the existing agreement key for the first release. The service requires
  `access_recipient == buyer_authorization_key`. Separate access keys and delegated access are
  not supported by this profile.
- An access request authorizes retrieval, not payment. Its digest binds the deployment domain,
  service identity, deal commitment, deal nullifier, resource digest, request nonce, and request
  expiry. Suite 1 signs the Keccak digest. Suite 2 signs Poseidon over tag `3002` and both
  128-bit digest limbs. Disclosure uses tag `3001`; payment authorization uses separate role
  tags.
- Requests expire within five minutes. Remote access requires TLS. A stolen request can be
  replayed during its validity period. Retrieval of an immutable snapshot is idempotent, not a
  consumable usage allowance. This profile does not select a prepaid or batched x402 billing
  model.
- The service persists issuance before it returns the payload. After a lost response, the same
  buyer can retrieve the same issuance. The service has no transaction signing or payment
  submission path. Pending observations do not issue access and do not prove nonpayment.
- The delivery deadline does not cancel a paid entitlement. Late issuance is reported
  explicitly. The service response reports `delivery_verified: false`. A response from the seller
  is not an independent audit of delivery.
- Recovery requires the seller to retain the agreed resource and private agreement evidence.
  The seller must restore issuance storage and continue serving the endpoint. The service cannot
  force an uncooperative seller to deliver or refund.

### Buyer command

Build the access command:

```sh
cargo build --locked --manifest-path sdk/shielded/Cargo.toml --bin erebus-access
```

Send one JSON request on standard input:

```json
{
  "method": "retrieve",
  "evidence_file": "/absolute/path/private/deal.evidence",
  "buyer_key_file": "/absolute/path/private/buyer.key",
  "service_url": "https://your-service.example/v1/access",
  "service_id": "<trusted nonzero service identity as 64 lowercase hex digits>",
  "cache_root": "/absolute/path/buyer-access-cache"
}
```

- Use the existing buyer agreement key, not a new key. The key file accepts 32 raw bytes or a
  64-digit hexadecimal encoding. For suite 2, those bytes are the existing agreement-signing
  seed.
- Evidence and key files must be regular, owner-only files. The endpoint must be HTTPS;
  redirects are not followed. Local test HTTP requires `allow_loopback_http: true` and a literal
  loopback endpoint.
- Exit code `2` means pending access. Retry retrieval, not settlement. It also covers a
  seller-reported `paid_but_undelivered` outcome. That response keeps independent
  `payment_verified: false` and names the seller claim separately.
- Exit code `0` returns content metadata and a private local file path. It does not print
  resource bytes or the key. The buyer verifies the payload hash before durable publication.
  After successful retrieval, the cache works offline and needs no signing key. Corrupt or
  symlinked cache entries fail closed.
- `resource_verified: true` means the bytes match the signed digest.
  `seller_reported_payment_finalized: true` is the seller's claim. The receipt does not
  independently verify chain payment or audit delivery.

### Buyer MCP configuration

Configure the access-only mode:

```sh
export EREBUS_BACKEND=access
export EREBUS_ACCESS_CLI=/absolute/path/erebus-access
export EREBUS_ACCESS_EVIDENCE_DIR=/absolute/path/private/evidence
export EREBUS_ACCESS_BUYER_KEY_FILE=/absolute/path/private/buyer.key
export EREBUS_ACCESS_SERVICE_URL=https://your-service.example/v1/access
export EREBUS_ACCESS_SERVICE_ID=<trusted-service-identity>
export EREBUS_ACCESS_CACHE=/absolute/path/buyer-access-cache
```

The evidence directory must have mode `0700`. The MCP tool is
`retrieve_service_access(evidence_name)`. The operator fixes the endpoint, key path, service
identity, and cache. The agent supplies only a bounded filename inside that evidence directory.
Python forwards paths to Rust and does not read the private key or resource contents.

For local tests only, set `EREBUS_ALLOW_LOOPBACK_ACCESS_HTTP=1`. This mode exposes no payment
tools. The combined negotiation and settlement MCP workflow remains separate M8 work.

### x402-exact access backend

The access service supports an explicit `x402_exact` backend (see §12 for exposure). Operator
configuration facts:

- Use `backend.mode = "x402_exact"` in the access service config. Supply `namespace`, distinct
  `rpc_url` and `peer_rpc_url`, `permit2_runtime_hash` and `proxy_runtime_hash` (32-byte
  arrays), `transaction_key_file`, `signer_journal_root`, `gas_limit`, and decimal
  `max_fee_per_gas` and `max_priority_fee_per_gas`. Both RPCs must authenticate the pinned
  canonical runtimes. The gas account must be funded, and all users of that account must share
  its signer journal. Keep `state_root` and the signer journal across restarts. One unresolved
  transaction holds the account's claim; it does not authorize a replacement or another
  payment.
- The signed agreement must name the exact proxy as its settlement contract, use suite 1 and
  public-bound mode, and have zero fees. The buyer must separately approve the token to Permit2.
  No approval is signed automatically.
- The native access request requires `x402_exact: true`; the Python seam uses the same explicit
  flag. For the dedicated access MCP server, configure `EREBUS_ACCESS_PAYMENT_RAIL=x402-exact`.
  Default `observe` never authorizes a payment. The combined Metropolis buyer profile also
  accepts this rail if an access service is configured and `EREBUS_PAYMENT_CONFIG` is absent. It
  exposes negotiation and access only, not ordinary funding, settlement or recovery tools. The
  signed agreement still names the exact proxy.
- New buyer permits expire within 120 seconds and never outlive the signed agreement. Retrying
  an expired permit does not extend it; an already finalized payment can still deliver access.
- The buyer persists one authorization before HTTP. The seller persists the exact transaction
  and a broadcast fence before sending. After that fence, retries only observe: even a timeout
  or rejected response cannot clear it. A crash between the fence and send can leave an
  operation pending without a broadcast; it requires operator investigation, not another permit.
  Delivery requires matching finalized transaction input, transfer and nonce evidence from both
  RPCs. `PAYMENT-RESPONSE` and the resource receipt are seller assertions, not independent
  auditor evidence. Buyer retrieval continues to report `payment_verified: false`.
- This verifier checks the token's transfer event, not historical recipient balance deltas. Use
  trusted standard ERC-20 assets; fee-on-transfer, rebasing and dishonest event-emitting tokens
  are not qualified by this test evidence.
- The full x402 design record and measured service-model comparison are outside this operations
  guide; only the operational surface is repeated here.

### Seller access handoff from negotiation

For a seller serving paid snapshots, set `access_evidence_root` in its private negotiation
configuration to the access service's absolute `evidence_root` directory. Create that directory
with mode `0700` before starting either process. The native seller verifies the frozen transcript
and both authorizations before publishing `<deal_commitment>.evidence` with mode `0600`.
Publication syncs the file and directory, permits identical retries, and rejects conflicting
existing evidence. No watcher or Python cryptography is required. The access service must still
validate its configured seller, service, and payment. Buyers cannot configure this publication
path. A handoff error is not permission to delete state or authorize a new payment; retry the
same negotiation operation.

### Local verification

Build EVM artifacts, then run the funded public-bound HTTP test:

```sh
forge build --root contracts/evm
cargo test --locked --manifest-path sdk/shielded/Cargo.toml --test access_http -- --include-ignored
```

The test settles once, rejects another buyer, restarts the service, and retrieves the same
issuance. The seller balance and gas-payer nonce prove that retrieval did not repeat payment. It
uses local Anvil and does not prove hosted or live Monad service operation. Set
`EREBUS_TEST_MCP_PYTHON` to the absolute Python executable to include the real MCP client probe.

Run the isolated SDK/MCP wheel test after building the access binary:

```sh
EREBUS_M8_TEST_ACCESS_INSTALL=1 .venv/bin/python -m pytest -q mcp-server/tests/test_access.py
```

This test installs local wheels and copies the native command into a temporary environment. It
verifies production import paths, funded access, private caching, and offline recovery. It also
verifies the paid-but-undelivered status through CLI and MCP. It does not publish packages or
prove installation from a public registry.

The funded shielded runner requires the existing M5 development toolchain:

```sh
cd circuits/m5
EREBUS_M5_NATIVE_PROOF=1 EREBUS_M6_COORDINATED=1 EREBUS_M8_ACCESS=1 npm run prototype
```

It binds a replayable transcript and snapshot terms into the shielded agreement. It verifies
independent disclosure, authenticated access, restart recovery, and exactly one broadcast. The
generated `access-report.json` is inside the temporary `build/coordinated-*` state directory.
All proving keys and private seeds in this runner are test-only.

---

## 5. Negotiation command

Scope: local Rust developer components on `metropolis`, not a published agent release.

### Code entry points

| File | Responsibility |
|---|---|
| `sdk/transport/src/socket.rs` | Bounded TCP framing and descriptor-bound Noise sessions |
| `sdk/transport/src/negotiation.rs` | Canonical proposals, alternating counters, acceptance, frozen final terms |
| `sdk/transport/src/store.rs` | Persist-before-ack transcript and atomic freeze record |
| `sdk/transport/src/binding.rs` | Descriptor-signed attestation of a shielded agreement key |
| `sdk/transport/src/negotiation/peer.rs` | Persist/send/receive helpers and separate final authorization callbacks |
| `sdk/transport/src/negotiation/bootstrap.rs` | Authenticated identity exchange and first offers without a shared private draft |
| `sdk/coordinator/src/lib.rs` | Buyer policy reservation, signing fence, and durable authorizations |
| `sdk/shielded/src/bin/erebus_negotiate.rs` | Native deterministic price negotiation and durable consent for both settlement modes |

- `NegotiationPeer::public_bound` requires agreement keys equal to the authenticated descriptor
  addresses.
- `NegotiationPeer::shielded` exchanges private identity bindings before typed negotiation.
- Both constructors verify capabilities against the operator-selected draft.
- These synchronous APIs require a blocking worker when called from an async application.

### Cold-start SDK

`NegotiationBootstrap` requires signed descriptors and the operator-selected settlement context,
not a private draft on both machines. For suite 2, it authenticates the remote agreement key
through the private signed binding. The seller also sends its payment tag through Noise. It never
sends its spending secret. The final seller signature authorizes that payment tag; the identity
binding alone does not.

The buyer uses `create_proposal` to insert authenticated identities and generate fresh local
randomness. This replaces the deal ID, settlement nonce, blinding, revision, transcript root, and
access recipient. It preserves the selected deployment, asset, guarantees, service promise,
fees, and deadline. The buyer calls `offer` to persist the draft before delivery. The seller
calls `receive_offer` with its own service-policy check before storage or acknowledgement. It
receives the canonical terms and blinding only through Noise.

After a fresh handshake, `NegotiationPeer::synchronize` re-delivers retained events in role
order. Both participants must recover the same root before further negotiation or authorization.
Invalid counts, changed events, and unequal roots close the connection without discarding
durable state. Synchronization control messages never enter the negotiation root. Session limits
still apply; this operation does not bypass re-handshake requirements.

### Native command

Build and check the local command from the repository root:

```sh
cargo build --manifest-path sdk/shielded/Cargo.toml --locked --bin erebus-negotiate
printf '%s' '{"method":"version"}' | sdk/shielded/target/debug/erebus-negotiate
cargo test --manifest-path sdk/shielded/Cargo.toml --locked --test negotiation_cli
```

The version response states `settlement_submission: false`. This command negotiates and
authorizes an agreement. It does not prove, fund, sign a transaction, or submit payment. It has
no RPC configuration or submission path. The complete settlement command and combined MCP
workflow remain open M8 work.

Each participant has a separate owner-only JSON configuration and its own keys. All key files
contain exactly 32 raw bytes. The terms template contains canonical `AgreementTerms` bytes.
Signed descriptor files contain public JSON. The configured TCP endpoint must occur in the
signed seller descriptor. This local profile uses literal socket addresses, not automatic
hostname discovery or a hosted relay.

Example operator configuration, with paths replaced by that operator's existing local files:

```json
{
  "version": 1,
  "role": "buyer",
  "state_root": "/operator/erebus/state",
  "transport_key_file": "/operator/erebus/transport.key",
  "agreement_key_file": "/operator/erebus/agreement.key",
  "discovery_key_file": "/operator/erebus/discovery.key",
  "local_descriptor_file": "/operator/erebus/buyer.json",
  "peer_descriptor_file": "/operator/erebus/seller.json",
  "terms_template_file": "/operator/erebus/service.terms",
  "endpoint": "127.0.0.1:9020",
  "maximum_price": "75",
  "minimum_price": "0",
  "max_deal_lifetime_seconds": 3600,
  "timeout_seconds": 30,
  "seller_spend_secret_file": null,
  "seller_wallet_file": null,
  "seller_wallet_key_file": null
}
```

- For suite 1, the agreement key also signs the discovery descriptor. For suite 2, the separate
  discovery key signs the identity binding.
- A shielded seller must also configure its spending secret, encrypted wallet, and wallet
  encryption key. Before sending seller consent, the command persists the expected payment note
  in that wallet. An expected note is not an included or spendable payment.
- For a paid snapshot, set `access_evidence_root` to the access service's `evidence_root`; see
  §4 for the handoff rules.

Start one seller process, then one buyer process, with their separate configuration paths:

```json
{"method":"negotiate","config_file":"/operator/erebus/config.json","operation_ref":"1111111111111111111111111111111111111111111111111111111111111111"}
```

- Use a new nonzero operation reference for each new deal. Reuse the same reference after
  interruption.
- The buyer starts at its template price and accepts no counter above `maximum_price`. The
  seller accepts an offer at or above `minimum_price`; otherwise it counters with its template
  price. This command supports that deterministic single-counter policy, not general-purpose
  bargaining strategies.
- The buyer generates the payment expiry from its configured lifetime; the seller checks its own
  lifetime bound. The remaining service fields and fee policy must match the seller's template.
- `freeze_only: true` stops after bilateral acceptance, before any signature or spending
  reservation. Repeat without that flag to reconnect, synchronize, and authorize the retained
  agreement.
- After uncertain delivery, repeat the same request on both participants. **Never generate a new
  deal to hide an error.**
- The command serializes calls for one operator state directory. It is not a concurrent
  marketplace daemon.

An authorized response reports the commitment and a private `evidence_file` path. That journal
blob contains `SelectedAgreement` bytes, despite the storage engine's `.tx` filename suffix. It
can feed the existing disclosure and access components. The buyer also retains coordinator
intent, consent, and one policy reservation for later settlement. Both `payment_verified` and
`delivery_verified` remain `false`.

The copied-command test runs without checkout files or environment variables in the child
processes. It exercises both modes, direct acceptance, a counteroffer, frozen restart, repeated
authorization, and private seller-note retention. This is local component evidence, not a
published package or live Monad payment.

### Negotiation boundary

- The caller selects an initial canonical draft and fresh random blinding per deal. The draft has
  a zero transcript root and revision `1`. It fixes the deployment, asset, identities, service,
  expiry, fee policy, guarantees, and settlement nonce. This profile negotiates only price; a
  different service or deployment requires a new deal.
- The buyer sends the initial offer. Participants alternate counters, each bound to the previous
  proposal digest. The nonproposer accepts the latest proposal; the proposer confirms it. The
  first acceptance prevents further counters. The second seals the transcript root before final
  signatures.
- For buyer authorization, call `Coordinator::record_intent` and
  `Coordinator::authorize_buyer` before encrypted signature delivery. For the received seller
  authorization, use `Coordinator::accept_seller` as the persistence callback.
- The seller must persist both final authorizations in private durable state before
  acknowledgement. The callback boundary does not implement seller storage automatically.
- Neither a frozen root nor stored signatures prove payment.

### Local verification

From the repository root, run the transport tests:

```sh
cargo test --manifest-path sdk/transport/Cargo.toml --locked --all-targets
```

The tests cover both suites, exact-body retries, changed context, expiry, corrupt freeze records,
concurrent append/freeze, key substitution, signature rejection, and failed persistence
callbacks. They also verify that existing M1 commitment and M2 root functions reproduce the new
agreement.

Run the independent-process integration:

```sh
cargo test --manifest-path sdk/evm/Cargo.toml --locked --test negotiation_flow
```

One parent test starts separate buyer and seller workers twice, then an independent auditor. The
worker entry point is ignored by default and invoked by that parent test. Expected: the parent
test passes; both peers retain one root across restart and authorize one commitment. The buyer
retains one reservation, and the auditor reports a verified agreement. No RPC provider, prover,
or chain funding is required. The test does not settle a payment or prove resource delivery.

### Recovery and privacy limits

- After uncertain delivery, close the connection and establish a fresh Noise handshake. Use
  `NegotiationPeer::synchronize` to reconcile retained events before resuming. Use
  `NegotiationPeer::redeliver` for an explicit already-persisted local event. Re-delivery
  changes only the session envelope, not the transcript-relevant body. It does not generate new
  signatures, cancel earlier authorizations, or release policy capacity.
- If a freeze record is missing after complete acceptance, replay repairs it. If a freeze record
  is **corrupt, stop; do not delete it to force progress.** Restore trusted private state and
  inspect the retained log before resuming.
- Final agreement signatures and chain observation remain separate recovery steps.
- Transcript stores contain plaintext commercial terms and the blinding; keep them private.
  Key bindings and final authorizations travel encrypted but do not hide endpoints or traffic
  timing.
- No per-message public signature proves individual authorship to an auditor. Both final
  signatures authorize the disclosed transcript root, not the truth of business statements.

Cross-component work still open at the negotiation component's last revision: the combined
installed CLI/Python/MCP workflow, funded live private negotiation, package distribution,
hosted/self-hosted rehearsal, x402, and stage measurements. Later evidence supersedes parts of
that list: packages are published and x402 plus a live Monad rehearsal have run (§10).

---

## 6. Disclosure and auditor operations

Both `erebus-disclosure` and `erebus-shielded-disclosure` export and verify suite-1 public-bound
and suite-2 shielded agreements. `erebus-disclosure` verifies public-bound payment.
`erebus-shielded-disclosure` also verifies shielded payment. Send one JSON request on stdin.
Each invocation emits one JSON response. These commands are development-branch tools, not
published Metropolis release packages.

1. Generate a `DisclosureIdentity` for the auditor and store its private key in an owner-only
   file. Give only its public key to the issuer. Keep this key separate from Eleusis transport
   keys.

   ```json
   {"method":"keygen","key_file":"auditor.key"}
   ```

   Use `key_info` with the same `key_file` to recover the public key without replacing the key.

2. Keep the issuer's `FileTranscriptStore` and agreement opening available until a grant has
   been exported. Relay retention is seven days and is not a transcript backup.

3. Build canonical selected evidence from durable participant state. With `erebus-disclosure`,
   call `select`:

   ```json
   {"method":"select","state_root":"<coordinator dir>","operation_ref":"<64 hex>",
    "store_root":"<transcript store dir>","namespace":"<store namespace>",
    "transcript_hash_version":1,"evidence_file":"<new owner-only path>"}
   ```

   It reads the opening and both authorizations from the coordinator snapshot, replays the
   participant transcript, verifies the transcript root and both signatures, and writes a new
   owner-only evidence file. SDK callers can call `SelectedAgreement::from_store` directly with
   the same inputs. A missing or mismatched snapshot fails closed and writes nothing. The
   accessor verifies snapshot authorizations; the transcript check happens during selection. It
   does not change snapshots, but tightens directory permissions and acquires a journal lock.
   Keep the state directory and its parents under the operator's control. A write or sync
   failure can leave an incomplete output. Do not overwrite it; retry with a new path after
   resolving the storage fault.

4. Export an encrypted grant using the auditor's public key and a buyer or seller issuer key.
   `issuer_key_file` contains the participant's raw 32-byte key in an owner-only file. For
   suite 1, this is the secp256k1 private key. For suite 2, this is the participant's agreement
   seed. `expires_at` is a future Unix timestamp in seconds.

   ```json
   {"method":"export","evidence_file":"selected.evidence","issuer_key_file":"issuer.key",
    "recipient_public_key":"<64 hex>","grant_file":"deal.grant","expires_at":4102444800}
   ```

   This creates a new owner-only backup and refuses to overwrite one. SDK callers can use
   `DisclosureGrant::write_backup`. Deliver the encrypted grant, not the selected evidence file.

5. The auditor loads its disclosure key and the grant backup. For a public-bound EVM deal,
   verify the agreement offline using a trusted expected participant address:

   ```json
   {"method":"verify_agreement","grant_file":"deal.grant","key_file":"auditor.key",
    "expected_issuer":"<0x participant address>"}
   ```

   This does not establish payment. To check payment, use `verify_payment` with the same fields
   and an independently configured deployment:

   ```json
   {"method":"verify_payment","grant_file":"deal.grant","key_file":"auditor.key",
    "expected_issuer":"<0x participant address>","deployment":{
      "namespace":"<eip155 chain namespace>","settlement_contract":"<0x contract address>",
      "verifier_version":1,"rpc_url":"<trusted RPC URL>"}}
   ```

   SDK callers can use `verify_public_bound_disclosure` with an independently configured
   `EvmChain`. Both paths require a working trusted RPC and finalized matching payment evidence.
   Neither verifies service delivery. Pending observation is exit code `2`, **not verified
   payment**; never interpret an unavailable result as non-payment.

For a suite-2 grant, `expected_issuer` is `0x` followed by the 128 lowercase hex digits of
`Ax || Ay`. Use the same `verify_agreement` request for offline verification. Use
`erebus-shielded-disclosure` for payment verification with this deployment configuration:

```json
{"method":"verify_payment","grant_file":"deal.grant","key_file":"auditor.key",
 "expected_issuer":"<0x suite-2 participant key>","deployment":{
   "namespace":"<eip155 chain namespace>","settlement_contract":"<0x pool address>",
   "verifier_version":2,"rpc_url":"<primary RPC>","peer_rpc_url":"<independent peer RPC>",
   "first_block":123,"first_hash":"<0x deployment block hash>","cache_root":"<public cache directory>"}}
```

The deployment anchor and verifier version must come from a trusted deployment manifest. The
tool creates separate `primary` and `peer` public caches. It never needs a note wallet or proving
artifacts. If observation is pending or unavailable, retain those caches and retry the same
request. Never interpret an unavailable result as non-payment. Never submit another payment from
the auditor command.

The grant contains the selected deal transcript, accepted terms, blinding, and authorizations. It
contains no parent Eleusis session key or spending key. Its public header reveals the issuer
address or suite-2 public key, recipient public key, deal ID, expiry, and ciphertext size. The
recipient can read the disclosed business terms after opening it. Public-bound EVM settlement
also exposes the payment on chain.

Back up the recipient's private disclosure key separately. The encrypted grant alone cannot be
opened after that key is lost. If the issuer loses the transcript and has no local backup before
export, the grant cannot be reconstructed after relay data expires. A damaged or partial grant
backup fails decoding or signature verification; do not infer that the deal was unpaid.

### Missing grant recovery

If the grant is missing, retain the complete transcript, accepted terms, blinding, and both
authorizations. Use `SelectedAgreement::from_store` to reconstruct verified export evidence from
participant storage. Use the auditor's public disclosure key to seal a replacement grant. Save
the replacement at a new or vacant backup path. The backup writer refuses to overwrite existing
files, including damaged backups.

This recovery needs no relay messages after retention expires. Relay retention and grant expiry
are separate deadlines. If the grant has expired, an authorized issuer must create a new grant
with a new deadline.

If both the grant and reconstruction evidence are missing, report unavailable disclosure
evidence. Do not infer payment status from missing files or submit another payment.
Durable-opening recovery is available: `read_disclosure_opening` reads the opening and both
authorizations from the coordinator snapshot without a session configuration, and the `select`
CLI method rebuilds evidence from it plus the transcript store. The CLI workflow test deletes
participant storage before a fresh auditor verifies the agreement. This offline check does not
verify payment or delivery. The funded local harness separately verifies shielded payment
through fresh CLI and MCP processes with participant storage unavailable.

### Disclosure-only MCP

On the `metropolis` development branch, build `erebus-disclosure` from `sdk/evm`. For shielded
payment, build `erebus-shielded-disclosure` from `sdk/shielded` and select that executable
instead. Use the branch's Python environment; published Starknet packages do not include this
new surface. Start with an existing private artifact directory:

```sh
mkdir -m 700 ~/.erebus-disclosure
export EREBUS_BACKEND=disclosure
export EREBUS_DISCLOSURE_DIR="$HOME/.erebus-disclosure"
export EREBUS_DISCLOSURE_CLI="/absolute/path/to/erebus-disclosure"
erebus-mcp-server
```

For a source checkout, run `.venv/bin/python -m erebus_mcp.server` instead of the installed entry
point. No Starknet address, wallet, or prover URL is required for this mode. The operator must
control the artifact directory and its parents. Do not use a symlink directory.

Auditor tools are `create_disclosure_key`, `disclosure_key_info`, and
`verify_disclosed_agreement`. The key defaults to `auditor.key` in the artifact directory;
`EREBUS_DISCLOSURE_KEY_FILE` can select another operator-controlled path. Copy only the encrypted
grant into that directory. Supply the expected participant identity from an independent trusted
source. Tools return public verification facts, not plaintext terms.

Set all four issuer settings to enable selection and export:

- `EREBUS_DISCLOSURE_STATE_ROOT`: coordinator state directory.
- `EREBUS_DISCLOSURE_STORE_ROOT`: retained transcript store directory.
- `EREBUS_DISCLOSURE_NAMESPACE`: transcript namespace.
- `EREBUS_DISCLOSURE_ISSUER_KEY_FILE`: participant signing key-file path.

Then use `select_deal_disclosure` followed by `export_deal_disclosure`. Artifact names must be
single filenames, not paths. Python never opens the signing key. Keep grants and auditor keys
backed up; missing files do not imply non-payment.

Set `EREBUS_DISCLOSURE_DEPLOYMENT` to the deployment JSON shown above to enable
`verify_disclosed_payment`. This RPC configuration is fixed by the operator, not the model.
Failures return structured `DISCLOSURE_UNAVAILABLE` results and do not submit transactions.
Suite-2 export signs locally with the participant seed. No separate suite-1 disclosure key is
needed. The same tools support both modes; `mode` in the verification result states which
agreement was verified.

### Shielded payment check

For an opened suite-2 agreement, call
`erebus_shielded_prover::disclosure::verify_shielded_payment`. Configure two independently
operated pool RPCs with separate `IndexStore` caches and the same trusted deployment anchor. The
function verifies the transcript and authorizations before reading public pool state. It requires
a finalized matching payment, not a caller-supplied receipt or confirmation count. It sends no
transcript, amount, blinding, note opening, or private key to either RPC.

If observation returns `HistoryPending`, reopen the same caches and retry. If a source fails or
disagrees, report verification as unavailable. Do not infer an unpaid deal or authorize another
payment.

This opened-evidence function does not authenticate a grant issuer by itself. The CLI first opens
the version-2 grant and verifies the issuer against the disclosed buyer or seller key. SDK
callers must call `DisclosureGrant::open` with the independently expected participant key before
making the full disclosure claim. The deployment code and verifying keys need independent
authentication; RPC agreement alone is insufficient. The disclosure RPC fixtures in
`sdk/shielded/src/disclosure.rs` exercise observation wiring, not a funded transfer. The separate
funded harness covers the fresh CLI/MCP auditor path.

Local reproduction from the repository:

```sh
uv sync --all-packages --locked
cd circuits/m5
EREBUS_TEST_MCP_PYTHON="$PWD/../../.venv/bin/python" \
  EREBUS_M5_NATIVE_PROOF=1 EREBUS_M6_COORDINATED=1 \
  NODE_OPTIONS=--max-old-space-size=4096 npm run prototype
```

This requires the documented M5 compiler, Node dependencies, Rust toolchain, and Anvil. The test
setup uses known entropy. Never use it for real value. These developer prerequisites do not
satisfy M8's package-only operator onboarding gate.

Expiry stops future opens through the verifier, not through cryptography. A recipient with the
key and ciphertext can bypass that policy. Expiry cannot erase plaintext already disclosed.

---

## 7. Local proving

The implementation downloads public artifacts and generates proofs locally. It does not require
a hosted prover or an operator setup ceremony. The current tests use insecure prototype keys. No
secure artifact release is published here.

### Trust boundary

- Obtain the manifest digest from an independently trusted release source. Do not trust a digest
  supplied only by the artifact server.
- Authenticate live verifier code and ceremony evidence separately. The manifest hash alone
  cannot establish either fact.
- The manifest binds a chain ID, settlement contract, and verifier version. It lists WASM, R1CS,
  and proving-key files for deposit, transfer, and withdrawal. Each entry contains an HTTPS URL,
  exact byte length, and SHA-256 digest.
- The installer caps individual files at 256 MiB.
- The download service sees artifact requests and client network metadata, not the local witness.

### Local command

Build the Rust command:

```sh
cargo build --locked --manifest-path sdk/shielded/Cargo.toml --bin erebus-local-prove
```

Send one JSON request on standard input:

```json
{
  "manifest_file": "/absolute/path/release.json",
  "manifest_sha256": "<independently trusted lowercase SHA-256>",
  "circuit": "transfer",
  "cache_root": "/absolute/path/artifact-cache",
  "witness_file": "/absolute/path/private/transfer.json",
  "expected_public": ["<chain ID>", "<contract as decimal integer>", "<verifier version>", "<remaining public signals>"]
}
```

- Replace all placeholders before use.
- Deposit requires six public signals, transfer eleven, and withdrawal eight. The first three
  must match the authenticated manifest domain.
- The witness must be a regular, owner-only file, at most 512 KiB.
- The command returns public Solidity calldata, downloaded bytes, cache hits, and separate
  installation and proof times. It verifies the proof locally. It does not sign authorization or
  submit a payment. A successful proof is not finalized payment evidence.
- For development fixtures only, set `allow_test_artifacts` to `true`. Literal loopback HTTP also
  requires `allow_loopback_http: true`. Do not enable these exceptions for a secure release.

### MCP configuration

The MCP mode exposes `prove_local_transition` for an already prepared local witness. It does not
yet provide the complete installed negotiation and payment workflow. Configure these
operator-controlled environment variables:

```sh
export EREBUS_BACKEND=local-prover
export EREBUS_LOCAL_PROVER_CLI=/absolute/path/erebus-local-prove
export EREBUS_WITNESS_DIR=/absolute/path/private
export EREBUS_ARTIFACT_MANIFEST=/absolute/path/release.json
export EREBUS_ARTIFACT_MANIFEST_SHA256=<independently-trusted-digest>
export EREBUS_ARTIFACT_CACHE=/absolute/path/artifact-cache
```

The witness directory must have mode `0700`. Python passes file paths to Rust, not witness
contents through the model tool call. The tool accepts a single bounded witness filename,
circuit, and expected public signals. The operator fixes the artifact source and cache, not the
agent.

Published platform binaries and automatic release discovery remain open M8 work. The component
installation test builds local SDK and MCP wheels into an isolated environment. This test does
not prove installation from a public package registry.

### Failure recovery

- The installer locks each content digest and persists verified files atomically.
- After interruption, retry the same request. An incomplete download is not a usable cache
  entry.
- A corrupt regular cache file is replaced after a fresh verified download.
- Symlink cache entries are rejected rather than followed.
- Download outages affect first installation or cache repair. With all verified artifacts
  cached, proving does not require the artifact server.
- The host still needs enough CPU and memory for the selected circuit. Operator RAM requirements
  have not been measured for the release target.

### Verification

After generating the existing M5 fixtures, run the real artifact tests:

```sh
cargo test --locked --manifest-path sdk/shielded/Cargo.toml --test artifacts -- --include-ignored
EREBUS_M8_TEST_LOCAL_PROOF=1 .venv/bin/python -m pytest -q mcp-server/tests/test_proving.py
```

The first command proves all three circuits from downloaded artifacts in fresh Rust processes.
The second builds isolated Python wheels and exercises a real MCP client. Both are local
development checks. Neither authenticates a live shielded deployment.

---

## 8. Shared services and self-hosting

Status: the local package path was verified on `metropolis` (2026-10-03); `0.3.0.dev4` is
published on the separate Metropolis index (2026-10-04). No hosted deployment exists.
Self-hosted and hosted deployments run the same binaries; only configuration differs.

### Install

Install the Metropolis package index (see the
[install guide](metropolis-install.md#10-published-metropolis-packages)). The `erebus-cli` wheel
puts every native binary on the environment's `PATH`. The launcher below resolves binaries from
`PATH` only, so activate that environment or prepend its `bin` directory.

### Layout

```sh
erebus-selfhost init /srv/erebus
```

`init` creates an owner-only (`0700`) tree and a relay configuration with a random bearer token:

| Path | Holds | Back up |
|---|---|---|
| `relay.env`, `relay/data/` | Relay configuration and ciphertext mailboxes | Data is disposable after retention |
| `access.json`, `access/evidence/`, `access/issuance/` | Access configuration, published agreements, durable issuances | Yes: issuance state is what lets a lost response recover without a second charge |
| `indexer.env`, `indexer/data/` | Shielded pool indexer (optional) | Rebuildable from chain |
| `relayer.env`, `relayer/state/` | Transaction relayer configuration and durable operation journal | Yes |
| `run/`, `logs/` | Pid files and service logs | No |

A service is enabled when its configuration file exists.

### Services

| Service | Binary | Configuration | Health |
|---|---|---|---|
| Message relay | `erebus-relay` | `relay.env`: `EREBUS_RELAY_ROOT`, `_PORT`, `_TOKEN`, `_RETENTION` | `GET /healthz` with the bearer token |
| Access service | `erebus-access-service` | `access.json` via `EREBUS_ACCESS_CONFIG` | `GET /healthz`; loopback only |
| Pool indexer | `erebus_pool_indexer` | `indexer.env`: `EREBUS_INDEXER_*` | Shielded only; process liveness |
| Transaction relayer | `erebus-tx-relayer` | `relayer.env`: `EREBUS_RELAYER_*` | `{"method":"health"}` on stdin |

`access.json` is strict JSON (unknown fields rejected). `service_id` and `seller_key` are JSON
byte arrays, not hex. A public-bound example:

```json
{"service_id":[...32 bytes...],"seller_key":[...20-byte seller agreement address...],"suite_id":1,
 "resource":"dataset.snapshot.v1","payload_file":"/srv/erebus/payload",
 "evidence_root":"/srv/erebus/access/evidence","state_root":"/srv/erebus/access/issuance","port":8091,
 "backend":{"mode":"public_bound","namespace":"eip155:10143","settlement_contract":"0x...",
  "verifier_version":1,"rpc_url":"https://...","from_block":67495473,
  "log_block_range":100,"max_log_queries":256,"max_ancestry":64,"max_concurrent_queries":16}}
```

Each negotiated agreement must be published to `access/evidence/<deal commitment>.evidence`
before the buyer can retrieve. Configure the seller negotiation process's `access_evidence_root`
to this directory for durable local publication. A remote service's disk is not the local
seller's disk: remote publication still requires a colocated seller process or an explicit
authenticated operator transfer. The hosted relay is not wired into the native TCP negotiation
path. The access service listens on loopback; put an authenticated TLS gateway in front for
remote buyers. The relayer has no listener: a trusted host process runs it per request or with
`--serve` over stdin. Its variables are documented below.

### Operate

```sh
erebus-selfhost up /srv/erebus     # start configured services
erebus-selfhost check /srv/erebus  # probe configured services
erebus-selfhost down /srv/erebus
```

`up` is idempotent for running services. State survives `down`/`up`. This launcher is not a
supervisor: for production, run the same commands under systemd or launchd with restart.

### Verified

From a fresh venv installed only from the local Metropolis index, against a funded local Anvil
deployment: relay and access service reached health, the relayer configuration was accepted, and
both services came back healthy after `down`/`up` with state retained. The indexer was not
exercised; it needs a shielded pool. Not verified: TLS gateways, remote clients, or Monad.

### Transaction relayer (`erebus-tx-relayer`)

Use `erebus-tx-relayer --serve` for newline-delimited JSON requests in one process. Each input
line produces one response line. EOF stops the process. Rate-limit windows and counters persist
across requests, but reset on restart. The host supplies `EREBUS_RELAYER_CLIENT`; this mode does
not authenticate remote clients. Without `--serve`, the CLI handles one request.

Requests are size-limited before JSON parsing. An oversized frame terminates the process.
Malformed JSON returns an error without terminating `--serve` mode.

The CLI requires `EREBUS_RELAYER_STATE_ROOT` for `relay` and `recover`. The directory is
relayer-owned; never point it at the buyer's wallet or coordinator. All processes using the gas
account must share it. Use a dedicated gas account and back up the whole root. It contains
authorized public-bound terms and signatures; do not upload it as diagnostics.

`EREBUS_RELAYER_VERIFICATION_RPC_URL` must be an independently operated RPC for the same
deployment. It must differ from every endpoint in `EREBUS_RELAYER_RPC_URLS` after URL
normalization. Those endpoints handle broadcast and failover. The designated verifier also
checks recovery evidence. If either selected provider fails or disagrees, retain reservations
and repair the provider. There is no automatic single-provider fallback. Identical anchors are
required; provider lag can delay recovery. Distinct URLs do not establish independence, honesty,
or consensus.

Set `EREBUS_RELAYER_MAX_FEE_PER_GAS`, `EREBUS_RELAYER_PRIORITY_FEE_PER_GAS` (wei), and
`EREBUS_RELAYER_GAS_LIMIT` explicitly. `EREBUS_RELAYER_RPC_TIMEOUT_SECONDS` defaults to 15.
Funding must cover the configured gas limit at the maximum fee cap before a new nonce claim is
created. Persisted signing plans survive fee-cap changes across restarts.

History work uses three optional nonzero budgets:

| Variable | Default | Per-call work |
|---|---|---|
| `EREBUS_RELAYER_LOG_BLOCK_RANGE` | 2000 | Blocks per log query |
| `EREBUS_RELAYER_LOG_QUERIES` | 1024 | Log queries |
| `EREBUS_RELAYER_ANCESTRY_LINKS` | 8192 | Parent links |

The relayer saves separate observer cursors under `STATE_ROOT/history`. Back up the whole root.
If recovery reports `relayer history scan pending`, repeat `recover` with the original request.
Pending history does not allocate a nonce, sign, submit, or release capacity. After recovery
returns `Prepared`, use `relay` to submit. Recovery itself never submits. If a checkpoint is
malformed or a finalized anchor changes, stop and diagnose the cache or provider. Do not remove
financial state or treat incomplete history as nonpayment.

Requests:

- Send `{"method":"relay","evidence":"<original-authorized-evidence-hex>"}` to persist and
  submit.
- Send `{"method":"recover","evidence":"<original-authorized-evidence-hex>"}` to observe an
  existing operation after restart or endpoint switching. Keep the original request in the
  client's own durable state. Recovery never signs or broadcasts, works after expiry, and does
  not require the current fee schedule to match the original request.

`status:ok` means the request succeeded, not that a payment finalized. Inspect `stage`:
`Submitted` and `BroadcastUnknown` retain uncertainty. Only `Finalized` confirms payment.
`ClosedUnpaid` may still report `gas_account_busy:true`: an unused nonce claim requires
account-signed replacement or cancellation, not journal deletion. A local timeout never closes a
deal. The former `EvmSettlementBackend::submit`, `settle`, `verify`, and `with_confirmations`
methods have been removed. Confirmation counts cannot finalize payment.

For SDK callers migrating from M3:

1. Open the coordinator with the selected settlement context and shared buyer state.
2. Persist intent, reserve policy capacity, and record both authorizations through the
   coordinator.
3. Prepare with `EvmSettlementBackend::prepare`, reserve the gas account nonce through
   `SignerJournal`, and persist the signing plan and signed bytes with `sign_transaction`.
4. Send through `EvmChain::broadcast_journaled` or the journaled relayer path.
5. Read `finalized_deal_evidence_resumable_agreed` with two providers and separate history
   stores. Pending scans or errors hold reservations; do not infer payment from a broadcast
   response.
6. Reconcile complete evidence through the coordinator. Release the signer claim only from
   `verified_finalized_nonce_agreed` showing that its nonce was consumed.

The migrated tests in `sdk/evm/tests/local_chain.rs` demonstrate this sequence. Low-level chain
primitives remain available, but do not create a journal or recovery workflow for you.

Funding diagnostics try the configured providers in order. Each provider has a 15-second
deadline; a failed or timed-out read tries the next endpoint. A successful read checks the live
chain ID before and after work. If every endpoint fails, funding is unavailable, not sufficient.
These reads do not submit transactions. Library callers can configure the deadline with
`RelayService::funding_with_timeout`.

### Message relay (`erebus-relay`)

The relay exposes ciphertext mailboxes and a bearer-token health surface. The relay sees mailbox
ids, sizes, and timing, never plaintext. Do not add payload logging to diagnose it. Clients
retain their own transcripts and re-handshake after a relay outage.

### Pool indexer (`erebus-pool-indexer`)

The indexer serves shielded public events from the configured deployment block and supports
rebuild from RPC. It never accepts note openings and cannot spend. Its outage delays discovery
only. An endpoint switch means a new host with an empty cache; sync from the deployment block.
Wallet openings are local and unaffected.

---

## 9. Render preparation

The [Render blueprint](../packaging/metropolis/render/render.yaml) prepares one paid,
single-instance relay with persistent storage. It uses the same Rust relay binary, requires a
bearer token, exposes public liveness only through the gateway, and disables automatic
deployments. **No Render service is deployed or verified yet.** The owner selected preparation
only on 2026-10-04. **Paid provisioning is not authorized.**

The Docker image also supports `EREBUS_HOSTED_SERVICE=access` behind a loopback Caddy gateway.
Provision owner-only `/data/access.json`, use port 8081, and keep payload, evidence, issuance,
payment and signer journals on `/data`. The image does not generate keys, accepted agreements,
or authorization to spend. An x402 access configuration must keep `transaction_key_file` and
`signer_journal_root` under the persistent disk; startup rejects a path that escapes it, and
requires the transaction key to be an owner-only regular 32-byte file. It never creates or
rewrites an existing key or signer journal. Do not start a hosted x402 facilitator without
explicit approval for its funded testnet gas account. Back up durable state before replacing
disks or services.

Render deployment needs account access and an approved paid persistent-disk plan. A free
stateless deployment does not satisfy the recovery gate. The image and gateway have not yet been
built or exercised on Render; configuration tests do not prove hosted operation.

---

## 10. Live Monad rehearsal

### 10.1 Monad facts and prerequisites

From the official docs (fetched 2026-10-02):

| Fact | Value |
|---|---|
| Testnet chain ID / currency | 10143 / MON |
| Testnet RPC | `https://testnet-rpc.monad.xyz` (50 rps; 25 rps for `eth_call`/`eth_estimateGas`), `https://rpc.ankr.com/monad_testnet`, `https://rpc-testnet.monadinfra.com` |
| Faucet | `https://faucet.monad.xyz` |
| Explorers | `https://testnet.monadvision.com`, `https://testnet.monadscan.com` |
| Current testnet release | v0.15.2 / `MONAD_NINE`; the chain was reset from genesis on 2025-12-16 |
| Foundry | **v1.8 or later** for Monad execution (`network = "monad"`). The legacy Monad Foundry fork is deprecated. This repo's local toolchain is 1.5.1, so the `[profile.monad]` in `contracts/evm/foundry.toml` is inert until the toolchain is upgraded; RPC deployment works either way |
| Size limits | 128 KB contract size, 256 KB initcode; `ErebusSettlement` is about 15 KB runtime |

Canonical testnet contracts that matter to later work: Permit2
`0x000000000022d473030f116ddee9f6b43ac78ba3`, Multicall3
`0xcA11bde05977b3631167028862bE2a173976CA11`, and the x402 proxies
`0x402085c248EeA27D92E8b30b2C58ed07f9E20001` (Exact) and
`0x4020A4f3b7b90ccA423B9fabCc0CE57C6C240002` (Upto).

**Warning: deploying spends testnet funds and is irreversible.** Do it only with an explicit
operator decision and a dedicated, funded testnet key. Never use a key that holds value on any
network.

### 10.2 Verify the network before deploying

```sh
cargo build --manifest-path sdk/evm/Cargo.toml --locked --bin erebus-network-check
echo '{"chain_id":10143,"rpc_url":"https://testnet-rpc.monad.xyz","timeout_ms":10000}' \
  | ./sdk/evm/target/debug/erebus-network-check
```

Exit 0 requires every check to pass: chain ID, an explicit `finalized` block, hash-pinned
`eth_getTransactionCount`/`eth_getBalance`/`eth_getCode`/`eth_getLogs`, rejection of an unknown
block hash, and the BN254 add, mul, and pairing precompiles (valid and invalid vectors). The
report never contains the endpoint. A passing report is network compatibility evidence, not
deployment or payment evidence.

Recorded live result: all 16 checks passed against `testnet-rpc.monad.xyz`, chain ID 10143,
finalized block 67489534, head 67489537 (2026-10-02).

### 10.3 Build and record the artifact

```sh
cd contracts/evm
forge build
git -C ../.. rev-parse HEAD
```

The deployment manifest (`deployments/<network>.json`, shape in `deployments.example.json`)
records the chain ID, address, transaction, verifier version, artifact path, and source commit.
Commit the manifest, never the key.

### 10.4 Deploy

`ErebusSettlement`'s constructor is `(uint256 expectedChainId, uint32 expectedVerifierVersion)`
and reverts with `ChainIdMismatch` when the live chain differs, so a wrong-network deployment
cannot succeed.

```sh
export MONAD_RPC=https://testnet-rpc.monad.xyz
# Keep the key in a file readable only by you; never pass it on a shared command line in logs.
read -rs MONAD_DEPLOY_KEY
BYTECODE=$(python3 -c "import json;print(json.load(open('out/ErebusSettlement.sol/ErebusSettlement.json'))['bytecode']['object'][2:])")
ARGS=$(cast abi-encode "constructor(uint256,uint32)" 10143 1)
cast send --rpc-url "$MONAD_RPC" --private-key "$MONAD_DEPLOY_KEY" --json \
  --create "${BYTECODE}${ARGS#0x}"
```

Take `contractAddress` from the JSON receipt. `forge create` is an alternative, but its
constructor-argument parser rejects the plain argument list this constructor needs, so the
`cast send --create` form above is the tested path.

### 10.5 Verify the deployment before trusting it

Finality first: Monad's `finalized` tag trails the head. Wait until the deployment block is at
or below `finalized` before reading it.

```sh
python3 scripts/check-evm-deployment.py \
  --rpc-url "$MONAD_RPC" --address "$CONTRACT" --chain-id 10143 --verifier-version 1
```

Exit 0 requires the on-chain runtime code to match the reviewed artifact byte-for-byte with
immutable slots masked, and `verifierVersion()` to equal the manifest. Record the output in the
manifest's evidence. If the code differs, the address is not the reviewed contract: stop.

Recorded deployment: `ErebusSettlement` is live on Monad testnet at
`0xa5f0c864f434331bef9a7fc5e05450d598d24da4` (chain 10143, verifier version 1), deployed in
block 67495473 by transaction
`0x6c7b810c837c038753815df985c3b9ce210be3b40df6fab4ec0682e1a271ca65`. The manifest is
`contracts/evm/deployments/monad-testnet.json`. The runtime is 15,652 bytes; four immutable
reference ranges were masked and the remaining bytes matched the artifact byte-for-byte, and
`verifierVersion()` returned 1. The deployer key is dedicated testnet material kept outside the
repository in a mode-`0600` file. This is artifact identity evidence, not an audit.

What this section does not cover: shielded pool deployment and verifier-key publication; the
shared services (each has its own README and §8 of this guide); access issuance and x402
composition (see §4 and §12); and mainnet. Mainnet is a separate release decision after testnet
evidence and review. This guide does not authorize it.

### 10.6 Rehearsal harness

Status (2026-10-05): **both rails executed live.** The `x402-exact` rail completed one
team-operated Permit2 payment with seller restart, resource recovery, and independent auditor
verification (`0x8a0272e6…5df43`). The `public-bound` rail then completed the whole workflow on
Monad testnet with the observation fixes: one payment (`0x1ba9ddb3…ff73`), exactly one broadcast
attempt, buyer `Finalized`, seller access across a restart, and independent auditor verification
(`agreement_verified: true`, `payment_verified: true`, `delivery_verified: false`). An earlier
replacement was stopped before broadcast on a transient paired-auth error and expired; a fresh
replacement reused the same funded participants. Hosted services and independent external
acceptance remain out of scope here.

`scripts/metropolis-monad-rehearsal.py` drives the workflow with installed product commands only:
`erebus-negotiate`, `erebus-payment`, `erebus-access-service`, `erebus-access`,
`erebus-shielded-disclosure`, `erebus-network-check`, and `erebus-settle`. It never imports
repository code and never constructs keys, descriptors, or terms itself.

One JSON file describes the operator's real configuration:

```json
{"rail": "public-bound", "chain_id": 10143,
 "rpc_url": "https://testnet-rpc.monad.xyz", "peer_rpc_url": "https://rpc-testnet.monadinfra.com",
 "bin_dir": "/path/to/venv/bin",
 "settlement_contract": "0xa5f0c864f434331bef9a7fc5e05450d598d24da4", "deployment_block": 67495473,
 "asset_contract": "0x902f79145059910ef875aecf4187c771b204ea14",
 "negotiation_endpoint": "127.0.0.1:9431", "access_port": 9432,
 "payload_file": "/path/to/snapshot", "resource": "dataset.snapshot.v1",
 "buyer_start_price": 60, "seller_price": 70, "buyer_maximum_price": 75,
 "gas_key_file": "/path/to/owner-only/gas.key",
 "max_fee_per_gas": "200000000000", "max_priority_fee_per_gas": "2000000000", "gas_limit": 500000,
 "negotiation_timeout_seconds": 300, "verification_timeout_seconds": 7200,
 "agreement_lifetime_seconds": 14400, "grant_lifetime_seconds": 7200,
 "max_log_queries": 256, "max_concurrent_queries": 16}
```

- `rail` is `public-bound` (`ErebusSettlement`, buyer pays gas) or `x402-exact` (canonical
  Permit2 and `x402ExactPermit2Proxy`; the seller's access service pays gas). `gas_key_file`
  belongs to the buyer on the public-bound rail and to the seller on x402. `rpc_url` and
  `peer_rpc_url` must be different providers: paired finalized observation rejects one endpoint
  under two names.
- `agreement_lifetime_seconds` is the new agreement's lifetime. `erebus-negotiate` sets the
  payment expiry to negotiation time plus this value, so it must exceed
  `verification_timeout_seconds` by at least an hour for settlement and delivery; `init`,
  `preflight`, and `run` all reject a shorter value, and `preflight` also rejects a participant
  config whose stored lifetime differs from the plan. The older name `delivery_window_seconds`
  is accepted for the same value. `preflight` also estimates the scan from the live finalized
  height and rejects a plan whose lifetime, delivery deadline, or grant cannot cover it. A
  mismatch means the operator must initialize a replacement; the harness never edits a config to
  extend an agreement.
- `max_log_queries` (1..=1024), `max_concurrent_queries` (1..=32), and `max_ancestry` (1..=8192)
  set the observer budget shared by the buyer, seller, and auditor; see §2 for the current live
  values and validation rules. `grant_lifetime_seconds` must cover the auditor's cold scan plus
  a margin; the harness exports the grant with that lifetime.
- When `settle` returns `history_pending` with durable `broadcast_attempts: 0`, the harness
  continues the read-only catch-up and calls `settle` again. **Any recorded attempt, an ambiguous
  submission, or a missing durable diagnostic switches permanently to observe-only; no path
  creates another agreement or payment.**
- A plan may set `reuse_from` to an existing workdir. `init` then copies that workdir's owner-only
  participant keys, public descriptors, auditor key, and gas key, keeping the already-funded
  buyer and seller addresses, so a replacement agreement does not repeat onboarding. No
  operation state, journal, or authorization is copied, and the source directory is never
  modified. Because the reused descriptors advertise their original `negotiation_endpoint`, a
  replacement must keep the same endpoint.

### 10.7 Phases

```sh
python3 scripts/metropolis-monad-rehearsal.py init --plan plan.json --workdir DIR
python3 scripts/metropolis-monad-rehearsal.py preflight --workdir DIR
python3 scripts/metropolis-monad-rehearsal.py run --workdir DIR --authorize-live-transactions
```

- `init` creates `DIR/buyer`, `DIR/seller`, and `DIR/auditor`, each with its own fresh keys
  (`prepare_operator`, auditor `keygen`), exchanges only public descriptors, writes each side's
  template with `prepare_terms`, and copies the gas key owner-only. Nothing is broadcast.
- `preflight` is read-only. It runs `erebus-network-check` on both RPCs and requires both to
  serve identical finalized runtime code (the settlement contract, or Permit2 and the proxy). It
  checks that `deployment_block` is the contract's first block, that every participant file is
  owner-only, and that the gas account differs from both agreement keys. It reads the live
  finalized height and rejects a plan whose agreement lifetime, delivery deadline, or grant
  cannot cover the estimated scan. It also reads token balance, allowance, and native funds, and
  prints each missing prerequisite with exact values. **It exits 2 until everything is present.**
- `run` refuses without `--authorize-live-transactions`. It negotiates in two processes, then
  pays once. On the public-bound rail it runs `erebus-payment` funding and settlement, then
  observes on both RPCs until finalized. On x402 the buyer's first `erebus-access` request
  authorizes the payment. Next it starts the seller's access service, retrieves, restarts the
  service, and retrieves again by retrieval alone. Last, the seller exports an encrypted grant
  and the auditor verifies payment from that grant and the public chain only. The x402
  transaction hash comes from the seller's own durable journal. Results and monotonic stage
  timings go to `DIR/run-record.json`.
- Polling is bounded but resumable. Each product-command slice is capped at 1,800 seconds and a
  killed call is retried from its durable checkpoints; a pending history scan must advance or
  the poll fails closed; read-only polls retry a transient provider error until ten minutes pass
  with no successful reply; a timed-out or failed `settle` call recovers by `observe` only. The
  whole verification phase is bounded by `verification_timeout_seconds`, which may be configured
  up to 86,400 seconds. **Never lower it below the remaining scan work, and never reset the
  retained checkpoints to "start over" — a later start could omit a payment.**

### 10.8 Prerequisites the product does not yet automate

Buyer onboarding has no product command. Preflight prints the approval as an explicit step:
`approve(spender, amount)` from the buyer's agreement key to `ErebusSettlement` (public-bound) or
canonical Permit2 (x402). The buyer then needs native gas for that one transaction. Funding the
buyer's token balance and the gas account is likewise external.

### 10.9 Evidence so far

**Live, read-only (Monad testnet, 2026-10-04).** `init` and `preflight` ran against
`testnet-rpc.monad.xyz` paired with `rpc-testnet.monadinfra.com`. Both passed all network checks
and returned identical finalized `ErebusSettlement` runtime (keccak
`0x5fa7befc14141996c531dd21a0c7f6c06cd5767a9e08334a1f61e0dfafadf94c`), and the origin check
passed. Preflight then stopped at funding: the fresh buyer, seller, and gas accounts hold
nothing. No transaction was sent. `rpc.ankr.com/monad_testnet` also passed the network checks;
`monad-testnet.drpc.org` did not.

**Script validation, not Monad evidence.** To show `run` executes, the unmodified script ran
against a local Anvil chain configured as chain 10143, with `ErebusSettlement` and a token
deployed from the repository's Forge artifacts and the canonical x402 runtime installed.
Funding and approvals were done with `cast` as preflight printed them. Both rails completed: one
settlement, paired finalization on the public-bound rail, access recovery across a seller
restart, and auditor `payment_verified: true` for the negotiated commitment. These are local
timings and say nothing about Monad.

**Live x402-exact run (Monad testnet, 2026-10-04), team-operated.** Published `0.3.0.dev4`
commands completed `run` on the x402 rail: private authenticated negotiation, one
seller-facilitated Permit2 payment, a seller-service restart, resource recovery by retrieval
alone, and independent auditor verification from the encrypted grant. `run-record.json`:

| Field | Value |
|---|---|
| Settlement transaction | `0x8a0272e69188ac3be52783ddfc4d4436496a58f2d5c63d808cd3320a1fa5df43` |
| Deal commitment | `fa4628c2c2e57b659f2d36b86babf53ec8689e0d49a1708953e993dd1902a769` |
| Access | first status `pending`, one retry after restart, resource hash verified |
| Auditor | `agreement_verified: true`, `payment_verified: true`, `delivery_verified: false` |
| Stages | negotiation 352 ms, first request to resource 12,915 ms (local monotonic) |

On 2026-10-05 the live payment was re-verified with the installed dev4 auditor binary and a
fresh encrypted grant (the retained grant had expired), with the participant directories
temporarily withheld: `payment_verified: true`, commitment unchanged. This is team-operated
evidence, not third-party acceptance. It is not a hosted service, an MCP/LLM run, or external
acceptance. The HTTP profile requires an authenticated Erebus request and a matching x402 v2
header; generic third-party x402 client interoperability is unverified.

**Public-bound live run (Monad testnet, 2026-10-05), team-operated.**
`public-bound-replacement-2` reused the funded participants (`reuse_from`) and completed the
full workflow:

| Field | Value |
|---|---|
| Settlement transaction | `0x1ba9ddb363a0c916f8e41ca8eca7d4c524d1ba4722bf4ea37ed423547700ff73` |
| Deal commitment | `477f9ad5c37010c8379721f2573d8d7c51158805cf465ebe72ac1cfc2a698f33` |
| Finalized reads | `consumedDeals == true`; seller 70, buyer 5; receipt status `0x1` |
| Broadcast attempts | exactly one, outcome `Submitted` |
| Access | 38 retries after a seller restart; resource hash verified |
| Auditor | `payment_verified: true` from the grant and public chain only |

The buyer's settle phase resumed the log delta and the concurrent ancestry catch-up instead of
exiting on `history_pending`; exactly one transaction was submitted. Recorded stages: negotiation
144 ms, settle + catch-up 976 s, settle-to-finality 1,044 s, first request to resource 876 s. The
seller and auditor each paid their own cold scan. Earlier retained operations (the original
public-bound state and the first replacement) remain unconsumed and must not be reused. This is
**team-operated evidence, not third-party acceptance.**

**Earlier live public-bound settlement (2026-10-02), team-operated.** The coordinated lifecycle
completed against the live deployment with transaction
`0x1f7ec208a7b03b835f224ac989cc59c9aee4c33634a2b04498b8394ddede4c09`, deal commitment
`4511793857dd5a2f50e5f3910b034a652127bfababaae9abedeae68c02be1abd`, deal state `PaidFinalized`.
The later independent-process run used a deterministic test seller key in the buyer example;
that source behavior is removed from the independent path. At that checkpoint the proposal
transport was a plaintext test fixture, not the encrypted negotiation flow.

**What a live run must record:** settlement transaction, deal commitment and nullifier, auditor
result, and `run-record.json` stage timings, against the authorized accounts.

---

## 11. Failure recovery

Scope: operating the shared services and recovering settlement. Every diagnostic here is
redacted by construction: no step asks for a spending key, a proof witness, an agreement
opening, a signature, or a transcript. **If a procedure seems to need one of those, stop and
escalate instead.**

### 11.1 Ground rules

1. **Never delete a state directory to unblock an operation.** The coordinator snapshot and the
   signer journal are the only record that a payment may exist. Losing them turns a recoverable
   operation into an unrecoverable one. Back up the whole directory, including the
   initialization marker.
2. **Never treat a local timeout as non-payment.** A dropped broadcast response is recorded as
   `Unknown` and every reservation stays held. Resolve it by reconciling against the chain.
3. **Never export state as diagnostics.** `Diagnostic` values contain operation IDs, stages,
   attempt counts, and response categories only. Snapshots contain private terms and
   authorizations and are not safe to upload.
4. **One state root per buyer, one signer journal per chain and gas account, shared by every
   process using that account.** Two roots mean two policies and two nonce allocators.

### 11.2 A stalled operation

Symptoms: `diagnostics()` reports a stage that does not advance, `unknown_attempts > 0`, or a
signer journal that returns `Busy`.

1. Read `diagnostics()`: stage, `broadcast_attempts`, `unknown_attempts`, `last_broadcast_outcome`,
   `replacements`.
2. Stage `BroadcastUnknown` or `Signed` with attempts means a submission may have reached the
   network. Do not resubmit by hand. Reconcile:
   `EvmChain::finalized_deal_evidence_resumable_agreed(history_journal, peer, peer_history, deal_nullifier, budget)`.
   On `Complete`, pass its evidence to `Coordinator::reconcile`. On `Pending`, retain reservations
   and repeat with the same checkpoint directory.
3. `Unknown` evidence (observation error) holds every reservation. Fix the provider and read
   again; do not interpret an RPC failure as absence.
4. Stage `Finalized` with a `Committed` reservation is done. Release the signer claim only from
   `verified_finalized_nonce_agreed`, never from a local expiry.
5. A signer stuck `Busy` on an unconsumed nonce needs a replacement or a cancellation signed by
   that account. Do not delete the journal.

### 11.3 Transaction relayer symptoms

| Symptom | Diagnosis | Action |
|---|---|---|
| `{"status":"error","error":"relayer access limit exceeded"}` | `{"method":"health"}` shows `rate_limited` rising | Raise `EREBUS_RELAYER_MAX_REQUESTS`/`WINDOW_SECONDS`, or throttle the client |
| `{"method":"funding"}` reports `funded:false` with a `shortfall` | The gas payer is empty | Fund the relayer account; re-run `funding` before retrying |
| Submission fails on every provider (`submit_failed` rising) | One or more `EREBUS_RELAYER_RPC_URLS` are down | The service already tries the next provider in order; replace the dead URL and retry the same admitted agreement |
| `"agreement does not match relayer fee policy"` | The deal was signed with a different fee or recipient | Expected: the relayer cannot admit it. The buyer must re-authorize against the published schedule |
| `"agreement outside relayer lifetime window"` | Expiry is past, or further away than `EREBUS_RELAYER_MAX_LIFETIME` | Expected. Do not widen the window without recording the reason |

The relayer never releases the buyer's reservation. A relayer outage leaves the operation
`Unknown` and recoverable; it does not make the deal unpaid.

### 11.4 Message relay symptoms

| Symptom | Diagnosis | Action |
|---|---|---|
| `GET /healthz` fails | Process or listener is down | Restart; clients retain their own transcripts and re-handshake |
| `401` on mailbox access | Wrong or missing `EREBUS_RELAY_TOKEN` | Fix the token; do not disable auth outside a local run |
| `507` on `POST /v1/mailbox/{id}` | Mailbox reached its blob bound | Clients resume from their cursor; the relay never truncates |
| Blobs missing after the retention window | Retention is 7 days by default | Clients reconstruct from their durable transcript; the relay is not archival |

### 11.5 Pool indexer symptoms

| Symptom | Diagnosis | Action |
|---|---|---|
| `/healthz` reports a `last_error` | The upstream RPC failed during sync | Fix or switch the RPC; the next sync attempt resumes from the stored cursor |
| Served blocks disagree with the client's RPC | Reorg or stale cache | The client verifies blocks against its own anchor and rewinds. Rebuild the cache from the deployment block if the store is suspect |
| Endpoint switched | New host, empty cache | Sync from the deployment block; wallet openings are local and unaffected |

The indexer never accepts note openings and cannot spend. Its outage delays discovery only.

### 11.6 RPC failure playbook

The operator default for one versus two providers is not yet selected. The existing
single-provider APIs trust the configured RPC after internal consistency checks. For opt-in
paired recovery, public-bound callers can use `EvmChain::finalized_deal_evidence_agreed` and
`EvmChain::verified_finalized_nonce_agreed` before applying accounting or releasing signer
claims. Shielded callers use `ShieldedChain::reconcile_agreed`, with separate index caches and
matching deployment identities for both providers.

The paired path requires identical pinned head and finalized anchors. Different heights,
timeout, or disagreement must hold reservations; do not bypass the check by interpreting failure
as an unpaid deal. Independently operated providers reduce shared failure risk, but two URLs
alone establish neither independence nor honesty.

1. **Chain check first.** Every adapter call checks `eth_chainId` before and after work; a
   mismatch is a stop, not a retry against the wrong chain.
2. **`finalized` unavailable** is an error (`FinalityUnavailable`). Do not fall back to a
   confirmation count; fix the provider.
3. **Dropped or falsified responses** produce `Unknown` outcomes and failed observations. The
   reservation and the nonce claim stay held. Reconcile from a trusted provider before any
   resubmission.
4. **Reorg:** an included-but-not-final settlement may still be reorganized out. Only
   `finalized` evidence commits or releases. If a previously final anchor changes, treat the
   provider as untrusted and escalate.

Shielded observation also yields `HistoryPending` after each incomplete batch. Repeat with the
same `IndexStore`. The default batch is at most 1,000 new blocks. `ShieldedChain::reconcile`
requires both chain observers and separate public index caches. It compares both finalized
wallet histories before changing notes, accounting, or signer claims. Network budgets do not
limit local cache replay. The shielded cache rejects files above 128 MiB. An oversized cache
requires an index-storage change; repeated observation cannot bypass that limit.

### 11.7 Escalation checklist

Collect, without exporting secrets:

- the `Diagnostic` list for the operation, and the signer journal's `Busy`/`NotConsumed` status;
- the transaction hash from `signed_transaction` if one was broadcast;
- the `finalized` block number and hash the provider reports, and whether `eth_chainId` matches;
- relayer `health` counters and the current `funding` report;
- relay/indexer `/healthz` output.

Do not attach a coordinator snapshot, a wallet file, a transcript, or an agreement opening to an
issue or a chat. Those are the private material the protocol exists to protect.

### 11.8 Component recovery index

- **Payment:** once any broadcast attempt exists, every further `settle` observes only; observe
  needs no transaction key and uses separate durable checkpoints per RPC. `closed_unpaid`
  requires finalized nonpayment evidence. Finalized recovery needs no negotiation transcript;
  nonce cleanup needs separate matching finalized nonce evidence. Never delete journals. See §3.
- **Shielded payment:** after an attempted broadcast, recovery needs neither transaction key,
  artifacts, manifest contents, nor transcript, but still needs the wallet encryption key to
  replay finalized notes. An expired unpaid deal retains the note reservation for explicit
  no-effect recovery. See §3.
- **x402 access:** after the seller's broadcast fence, retries only observe; a crash between the
  fence and send requires operator investigation, not another permit. See §4.
- **Access retrieval:** retry retrieval with a fresh valid request; never create another payment.
  Exit code `2` is pending or `paid_but_undelivered`, not nonpayment. Buyer caches are offline
  after a completed download; corrupt or symlinked cache entries fail closed. See §4.
- **Negotiation:** after uncertain delivery, close the connection and re-handshake, then
  `synchronize`; `redeliver` only for an already-persisted local event. A missing freeze record
  is repaired by replay; a corrupt one means stop and restore trusted private state. Never
  generate a new deal to hide an error. See §5.
- **Disclosure:** retain caches and retry on pending observation; reconstruct a missing grant
  from `SelectedAgreement::from_store`; a damaged backup is not evidence of nonpayment; an
  expired grant needs a new seal with a new deadline. See §6.
- **Local proving:** retry the same request after interruption; incomplete downloads are not
  cache entries; corrupt regular files are re-downloaded; symlinks are rejected. See §7.
- **Shared services:** `up` is idempotent; state survives `down`/`up`; the launcher is not a
  supervisor. See §8 and §11.3-§11.5.

---

## 12. Security and exposure notes

### What public-bound settlement exposes

- Payment amounts and parties remain public.
- The full accepted canonical agreement, the blinding, and the final signatures appear in
  settlement calldata. This includes the signed service fields.
- Rejected offers and the offchain negotiation transcript remain private; the final service
  terms do not.
- A finalized payment is public evidence forever; only the disclosure wrapper can expire.

### What x402-exact exposes

- Buyer, `payTo`, amount, token, the deal nullifier (used as the Permit2 nonce), and the Permit2
  payment authorization signature are public.
- The Erebus agreement opening and bilateral agreement signatures are **not** sent by this rail.
- `PAYMENT-RESPONSE` and the resource receipt are seller assertions, not independent auditor
  evidence. Buyer retrieval continues to report `payment_verified: false`.
- A deal that requires `hidden-amount` or `hidden-recipient` must not fall back to this
  transparent rail. A shielded deal cannot use x402 mode.
- The verifier checks the token's transfer event, not historical recipient balance deltas.
  Fee-on-transfer, rebasing, and dishonest event-emitting tokens are not qualified.

### Shielded prototype status

- The current shielded tests use insecure prototype keys and artifacts with known test entropy.
  A successful local proof is not finalized payment evidence.
- `allow_test_artifacts` / `allow_loopback_http` and the funded M5 runner are development-only
  and must not be enabled for a secure release.
- No secure artifact release or secure Monad shielded deployment is claimed by this guide.

### Key material

- Key material never leaves owner-only files. Requests carry key **paths**, not key values;
  Python forwards paths and never opens keys or witnesses.
- Never commit, upload, or attach keys, seeds, bearer tokens, grants, signed transaction bytes,
  coordinator snapshots, wallet files, transcripts, or agreement openings.
- Restrict every state, evidence, witness, and cache directory to the operator and its parents.

### Metadata exposure

- **Access service:** sees the buyer identity, selected agreement, request timing, and requested
  resource. The TLS gateway can also see client network metadata. Self-custody does not hide
  these facts from the seller or guarantee endpoint availability.
- **Access requests:** a stolen request can be replayed during its validity period; retrieval of
  an immutable snapshot is idempotent, not a consumable allowance. Remote access requires TLS.
- **Message relay:** sees mailbox ids, sizes, and timing, never plaintext. Retention is seven
  days by default; the relay is not archival.
- **Transaction relayer:** sees public inputs and network metadata; it cannot change authorized
  outputs. Its state root contains authorized public-bound terms and signatures and is not safe
  to upload as diagnostics.
- **Pool indexer:** retains and serves public events from the configured deployment block, with
  block identity and cursor; it can omit or serve stale data, and clients verify chain evidence
  and recover by rescan.
- **Artifact download service:** sees artifact requests and client network metadata, not the
  local witness.
- **Negotiation transport:** key bindings and final authorizations travel encrypted, but this
  does not hide endpoints or traffic timing. No per-message public signature proves individual
  authorship to an auditor. Both final signatures authorize the disclosed transcript root, not
  the truth of business statements.
- **Disclosure grant:** the encrypted grant contains the selected transcript, accepted terms,
  blinding, and authorizations. Its public header reveals the issuer address or suite-2 public
  key, recipient public key, deal ID, expiry, and ciphertext size. The recipient can read the
  disclosed business terms after opening it. Expiry stops future opens through the verifier, not
  through cryptography: a recipient with the key and ciphertext can bypass that policy, and
  expiry cannot erase plaintext already disclosed.
- **Transcript stores:** contain plaintext commercial terms and the blinding; keep them private.

### Hosting and deployment boundaries

- There is no hosted deployment. The Render blueprint is prepared but **not deployed**, and paid
  provisioning is not authorized. A free stateless deployment does not satisfy the recovery
  gate.
- Hosted services are preparation-only: the image does not generate keys, accepted agreements,
  or authorization to spend. Do not start a hosted x402 facilitator without explicit approval
  for its funded testnet gas account.
- Distinct RPC URLs do not prove independent providers, honesty, or consensus. Paired reads are
  consistency evidence, not consensus proofs against malicious providers.
- Independent audit is not implied by a seller response or by matching providers. Only a
  separate auditor process with the encrypted grant, its own key, and independently configured
  deployment anchors verifies payment, and even that does not verify delivery.
- All live Monad evidence in this guide is team-operated. It is not third-party acceptance.

