# Operator-controlled mainnet Juno

This stack keeps the STRK20 pool private key inside the operator's trust boundary. Juno and
the transaction prover communicate on a private Docker network. Their host ports bind only
to `127.0.0.1`; use an SSH tunnel instead of exposing either service to the internet.

## Host

Use an always-on Linux x86_64 host with Docker Compose v2, at least 8 CPU cores, 32 GiB RAM,
and 400 GiB of fast NVMe storage. The pruned snapshot was 96,756,171,014 bytes on
2026-08-28, and extraction temporarily requires space for both the archive and database.

The host also needs `wget`, `tar`, and `zstd`, plus an Ethereum mainnet WebSocket RPC URL.
The Ethereum URL verifies Starknet state; it is not the Starknet RPC used by Erebus.

## Install

Copy this directory to the remote host, then run:

```sh
cd ops/juno
cp juno.env.example juno.env
chmod 600 juno.env
${EDITOR:-vi} juno.env
./bootstrap.sh
```

The download is resumable. Keep the compressed snapshot until Juno passes verification.

## Verify and start the prover

```sh
./verify.sh
docker compose --env-file juno.env --profile prover pull transaction-prover
docker compose --env-file juno.env --profile prover up -d
curl -sS http://127.0.0.1:3000/ \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"starknet_specVersion","params":[]}'
```

Do not start mainnet proving until `starknet_syncing` returns `false`.

## Connect Erebus without publishing the services

From the Erebus machine:

```sh
ssh -N \
  -L 6060:127.0.0.1:6060 \
  -L 3000:127.0.0.1:3000 \
  YOUR_REMOTE_HOST
```

Use these local settings:

```text
STARKNET_RPC_URL=http://127.0.0.1:6060/v0_10
PROVING_SERVICE_URL=http://127.0.0.1:3000
```

The prover itself uses `http://juno:6060/v0_10` inside Docker, so its key-bearing RPC calls
never leave the remote host. Self-hosting does not bypass protocol-enforced deposit screening.

STRK20 proving configuration: https://strk20-by-example.org/sdk/proving-config
