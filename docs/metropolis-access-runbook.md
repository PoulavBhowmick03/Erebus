# Metropolis Access Service

This is a local developer component, not a published or hosted release.
See [M8 decisions](metropolis-m8-decisions.md) for authentication and delivery limits.

## Build and run

From the repository root:

```sh
cargo build --locked --manifest-path sdk/shielded/Cargo.toml --bin erebus-access-service
EREBUS_ACCESS_CONFIG=/absolute/path/access-config.json sdk/shielded/target/debug/erebus-access-service
```

The service listens on `127.0.0.1` only.
Put a TLS gateway in front before remote access.
The configuration file must be a regular, owner-only file.
The evidence directory must be a real directory with mode `0700`.

## Configuration

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
    "max_log_queries": 8,
    "max_ancestry": 64
  }
}
```

Replace `seller_key` with the full authorization key bytes.
The placeholder `[1]` is not a valid key.
Choose a stable service identity independently of individual agreements.
Do not put private signing keys in this configuration.

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

Use the authenticated deployment block and hash, not the placeholders.
No secure Monad shielded deployment is claimed by this document.

## Prepare a resource

Export the selected agreement from durable participant state.
Place its encoded evidence at `<evidence_root>/<lowercase commitment>.evidence` with mode `0600`.
Keep this directory private. Evidence contains the agreed terms and transcript.

The signed service record must name:

- The configured resource and seller.
- Quantity `1` and unit `snapshot`.
- Fulfillment method `http-access-v1`.
- SHA-256 of the exact payload bytes.
- The buyer agreement key as `access_recipient`.

The current service supports payloads up to 1 MiB and 16 concurrent requests.
It does not implement metered usage, streaming, refunds, or delegated access keys.

## Retrieve and recover

`GET /healthz` reports process availability, not verified RPC health.
Send an `AccessRequest` to `POST /v1/access`.
Use `access::request_digest` and sign it with the buyer agreement key.
The request contains the commitment, nonce, expiry, and signature.
Do not send the agreement opening or a private key over HTTP.

HTTP `202` means observation is pending.
HTTP `200` returns the payload and durable issuance identity.
Authentication failures return `401`; agreement failures return `400`.
Storage or observation failures return `503`.
Retry retrieval with a fresh valid request. Do not create another payment.

Keep `state_root`, private evidence, and payload storage persistent across restarts.
Back up all three before endpoint migration.
The service requires finalized chain observation even when issuance already exists.
Provider outages can therefore delay retrieval of a previously issued resource.
This applies to seller-side retrieval. The completed buyer cache works offline.

The service sees the buyer identity, selected agreement, request timing, and requested resource.
The gateway can also see client network metadata.
Self-custody does not hide these facts from the seller or guarantee endpoint availability.

## Buyer command

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

Use the existing buyer agreement key, not a new key.
The key file accepts 32 raw bytes or a 64-digit hexadecimal encoding.
For suite 2, those bytes are the existing agreement-signing seed.
Evidence and key files must be regular, owner-only files.
The endpoint must be HTTPS; redirects are not followed.
Local test HTTP requires `allow_loopback_http: true` and a literal loopback endpoint.

Exit code `2` means pending access. Retry retrieval, not settlement.
It also covers a seller-reported `paid_but_undelivered` outcome.
That response keeps independent `payment_verified: false` and names the seller claim separately.
Exit code `0` returns content metadata and a private local file path.
It does not print resource bytes or the key.
The buyer verifies the payload hash before durable publication.
After successful retrieval, the cache works offline and needs no signing key.
Corrupt or symlinked cache entries fail closed.

`resource_verified: true` means the bytes match the signed digest.
`seller_reported_payment_finalized: true` is the seller's claim.
The receipt does not independently verify chain payment or audit delivery.

## Buyer MCP configuration

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

The evidence directory must have mode `0700`.
The MCP tool is `retrieve_service_access(evidence_name)`.
The operator fixes the endpoint, key path, service identity, and cache.
The agent supplies only a bounded filename inside that evidence directory.
Python forwards paths to Rust and does not read the private key or resource contents.

For local tests only, set `EREBUS_ALLOW_LOOPBACK_ACCESS_HTTP=1`.
This mode exposes no payment tools.
The combined negotiation and settlement MCP workflow remains separate M8 work.

## Local verification

Build EVM artifacts, then run the funded public-bound HTTP test:

```sh
forge build --root contracts/evm
cargo test --locked --manifest-path sdk/shielded/Cargo.toml --test access_http -- --include-ignored
```

The test settles once, rejects another buyer, restarts the service, and retrieves the same issuance.
The seller balance and gas-payer nonce prove that retrieval did not repeat payment.
It uses local Anvil and does not prove hosted or live Monad service operation.
Set `EREBUS_TEST_MCP_PYTHON` to the absolute Python executable to include the real MCP client probe.

Run the isolated SDK/MCP wheel test after building the access binary:

```sh
EREBUS_M8_TEST_ACCESS_INSTALL=1 .venv/bin/python -m pytest -q mcp-server/tests/test_access.py
```

This test installs local wheels and copies the native command into a temporary environment.
It verifies production import paths, funded access, private caching, and offline recovery.
It also verifies the paid-but-undelivered status through CLI and MCP.
It does not publish packages or prove installation from a public registry.

The funded shielded runner requires the existing M5 development toolchain:

```sh
cd circuits/m5
EREBUS_M5_NATIVE_PROOF=1 EREBUS_M6_COORDINATED=1 EREBUS_M8_ACCESS=1 npm run prototype
```

It binds a replayable transcript and snapshot terms into the shielded agreement.
It verifies independent disclosure, authenticated access, restart recovery, and exactly one broadcast.
The generated `access-report.json` is inside the temporary `build/coordinated-*` state directory.
All proving keys and private seeds in this runner are test-only.
