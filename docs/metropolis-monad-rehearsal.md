# Metropolis Live Monad Rehearsal

Status (2026-10-04): **executable and preflight-verified against Monad testnet; not run live.**
No live Monad transaction is authorized by the current records. Running it requires that
authorization, funded accounts, and an approval the product has no command for yet.

`scripts/metropolis-monad-rehearsal.py` drives the workflow with installed product commands only:
`erebus-negotiate`, `erebus-payment`, `erebus-access-service`, `erebus-access`,
`erebus-shielded-disclosure`, `erebus-network-check`, and `erebus-settle`. It never imports
repository code and never constructs keys, descriptors, or terms itself.

## Plan

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
 "max_fee_per_gas": "200000000000", "max_priority_fee_per_gas": "2000000000", "gas_limit": 500000}
```

`rail` is `public-bound` (`ErebusSettlement`, buyer pays gas) or `x402-exact` (canonical Permit2
and `x402ExactPermit2Proxy`; the seller's access service pays gas). `gas_key_file` belongs to the
buyer on the public-bound rail and to the seller on x402. `rpc_url` and `peer_rpc_url` must be
different providers: paired finalized observation rejects one endpoint under two names.

## Phases

```sh
python3 scripts/metropolis-monad-rehearsal.py init --plan plan.json --workdir DIR
python3 scripts/metropolis-monad-rehearsal.py preflight --workdir DIR
python3 scripts/metropolis-monad-rehearsal.py run --workdir DIR --authorize-live-transactions
```

- `init` creates `DIR/buyer`, `DIR/seller`, and `DIR/auditor`, each with its own fresh keys
  (`prepare_operator`, auditor `keygen`), exchanges only public descriptors, writes each side's
  template with `prepare_terms`, and copies the gas key owner-only. Nothing is broadcast.
- `preflight` is read-only. It runs `erebus-network-check` on both RPCs and requires both to serve
  identical finalized runtime code (the settlement contract, or Permit2 and the proxy). It checks
  that `deployment_block` is the contract's first block, that every participant file is
  owner-only, and that the gas account differs from both agreement keys. It also reads token balance,
  allowance, and native funds, and prints each missing prerequisite with exact values. It exits 2
  until everything is present.
- `run` refuses without `--authorize-live-transactions`. It negotiates in two processes, then pays
  once. On the public-bound rail it runs `erebus-payment` funding and settlement, then observes on
  both RPCs until finalized. On x402 the buyer's first `erebus-access` request authorizes the
  payment. Next it starts the seller's access service, retrieves, restarts the service, and
  retrieves again by retrieval alone. Last, the seller exports an encrypted grant and the auditor
  verifies payment from that grant and the public chain only. The x402 transaction hash comes from
  the seller's own durable journal. Results and monotonic stage timings go to
  `DIR/run-record.json`.

## Prerequisites the product does not yet automate

Buyer onboarding has no product command. Preflight prints the approval as an explicit step:
`approve(spender, amount)` from the buyer's agreement key to `ErebusSettlement` (public-bound) or
canonical Permit2 (x402). The buyer then needs native gas for that one transaction. Funding
the buyer's token balance and the gas account is likewise external.

## Evidence so far

**Live, read-only (Monad testnet, 2026-10-04).** `init` and `preflight` ran against
`testnet-rpc.monad.xyz` paired with `rpc-testnet.monadinfra.com`. Both passed all network checks
and returned identical finalized `ErebusSettlement` runtime (keccak
`0x5fa7befc14141996c531dd21a0c7f6c06cd5767a9e08334a1f61e0dfafadf94c`), and the origin check passed.
Preflight then stopped at funding: the fresh buyer, seller, and gas accounts hold nothing. No
transaction was sent. `rpc.ankr.com/monad_testnet` also passed the network checks;
`monad-testnet.drpc.org` did not.

**Script validation, not Monad evidence.** To show `run` executes, the unmodified script ran against
a local Anvil chain configured as chain 10143, with `ErebusSettlement` and a token deployed from the
repository's Forge artifacts and the canonical x402 runtime installed. Funding and approvals were
done with `cast` as preflight printed them. Both rails completed: one settlement, paired
finalization on the public-bound rail, access recovery across a seller restart, and auditor
`payment_verified: true` for the negotiated commitment. These are local timings and say nothing
about Monad.

**What a live run must record:** settlement transaction, deal commitment and nullifier, auditor
result, and `run-record.json` stage timings, against the authorized accounts.
