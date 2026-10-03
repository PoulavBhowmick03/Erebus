# Metropolis Self-Hosting

Status: local package path verified on `metropolis` (2026-10-03). No hosted deployment exists.
Self-hosted and hosted deployments run the same binaries; only configuration differs.

## Install

Install the Metropolis package index (see the [install guide](metropolis-install.md#10-metropolis-prerelease-registry-local-unpublished)).
The `erebus-cli` wheel puts every native binary on the environment's `PATH`. The launcher below
resolves binaries from `PATH` only, so activate that environment or prepend its `bin` directory.

## Layout

```sh
scripts/metropolis-selfhost.sh init /srv/erebus
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

## Services

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
  "log_block_range":100,"max_log_queries":8,"max_ancestry":64}}
```

Each negotiated agreement must be published to `access/evidence/<deal commitment>.evidence`
before the buyer can retrieve. Nothing does this automatically yet ([F48](friction.md)).
The access service listens on loopback; put an authenticated TLS gateway in front for remote
buyers. The relayer has no listener: a trusted host process runs it per request or with
`--serve` over stdin. Its variables are documented in `erebus-tx-relayer`'s header and the
[M6 runbook](metropolis-m6-runbook.md).

## Operate

```sh
scripts/metropolis-selfhost.sh up /srv/erebus     # start relay, access, indexer; wait for health
scripts/metropolis-selfhost.sh check /srv/erebus  # probe every configured service, relayer included
scripts/metropolis-selfhost.sh down /srv/erebus
```

`up` is idempotent for running services. State survives `down`/`up`. This launcher is not a
supervisor: for production, run the same commands under systemd or launchd with restart.

## Verified

From a fresh venv installed only from the local Metropolis index, against a funded local Anvil
deployment: relay and access service reached health, the relayer configuration was accepted, and
both services came back healthy after `down`/`up` with state retained. The indexer was not
exercised; it needs a shielded pool. Not verified: TLS gateways, remote clients, or Monad.
