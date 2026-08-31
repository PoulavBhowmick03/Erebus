# Optional screened local transaction prover

The primary Erebus mainnet canary uses Starkscan's operator-approved hosted prover. This
stack remains available for operators with separate screening-partner credentials who keep
proving inside their infrastructure. It is not required for the Starkscan path.

This Compose stack connects the pinned `PRIVACY-0.14.3-RC.2` proof interceptor to the
working local ARM64 prover. The interceptor is reachable only on the private Compose network;
only the prover remains available on host loopback port 3000.

The stack is prepared but cannot perform mainnet screening until the proxy operator supplies:

- `SCREENING_URL`
- `SCREENING_PARTNER_NAME`
- `SCREENING_PARTNER_SECRET`

The canonical mainnet pool address is already fixed in the generated configuration. Do not
put the partner secret in this repository.

## Finish the setup after access arrives

Write the protected env interactively:

```sh
scripts/write-screening-env.sh
```

Stop the existing standalone prover to release port 3000, then start the paired stack:

```sh
docker stop strk20-prover
ops/screened-prover/run.sh up -d
ops/screened-prover/run.sh ps
ops/screened-prover/run.sh logs --tail 100 proof-interceptor transaction-prover
```

The old standalone container remains stopped and recoverable. If the paired stack fails, run
`ops/screened-prover/run.sh down` and `docker start strk20-prover`.

Verify the prover and confirm that screening is active:

```sh
curl -sS http://127.0.0.1:3000/ \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"starknet_specVersion","params":[]}'

ops/screened-prover/run.sh exec proof-interceptor \
  node -e "fetch('http://127.0.0.1:8080/metrics').then(r=>r.text()).then(t=>console.log(t.match(/proof_interceptor_screening_results_total[^\\n]*/g)||[]))"
```

The metric must become non-zero after a real deposit proof request. A healthy `/health`
response alone does not prove that `SCREENING_URL` is working.

The prover is explicitly fail-closed with `BLOCKING_CHECK_FAIL_OPEN=false`; the interceptor
uses `SCREENING_FAIL_OPEN=false` and blocks non-pool requests. This setup does not bypass
protocol-enforced screening.

## Exact first mainnet run after access arrives

Do not open new channels. Reuse the verified handles from the 2026-08-30 run:

- A to B: `ch_b7afee5fd1f75ddc8425e8ca8b7879b4780588f81163a581f258289c238d9af8`
- B to A: `ch_c94f5afb8ad73af9f78baf4be45099ca7f57f28c230fa3a1104bef70b0d04fb2`

Before the first proof, confirm the Colima VM still has swap with `swapon --show`. The
20 GiB VM exhausted memory during one channel proof. The Compose stack limits Rayon to four
threads, but the 8 GiB swap file created during the channel run must be enabled again after
a VM restart.

Use this order:

1. Run `doctor` and `reconcile` for both protected identity envs. Stop on an unresolved or
   ambiguous operation.
2. Record the screening counter from `/metrics`.
3. Read Account A's live allowance and the pool fee. The recorded plan needs 19 STRK of
   allowance: 1 STRK deposit, plus three 6 STRK pool fees for shield, propose, and settle.
   Account B needs 6 STRK for one counter. Stop if either value differs.
4. Submit one 1 STRK `shield` for A with a caller-supplied durable operation ID. Do not use
   `agent.sh fund`: it replaces the standing allowance with only deposit plus one fee.
5. Confirm the screening counter increased, the receipt succeeded, and the shield is at
   least 10 blocks deep. Confirm `get_note_balance` reports the 1 STRK note as spendable.
6. Start A's MCP server as `payer` and B's as `payee` with `scripts/erebus-mcp.sh`. Through
   the MCP clients, read the two channel handles above instead of calling `open_channel`.
7. Run one bounded deal: A proposes at most 1 STRK, B counters, and A accepts the
   B-authored offer. Persist a caller intent and operation ID before every write.
8. Wait for each receipt to reach proving depth before the next dependent write. On a
   timeout, call `reconcile`; never create a replacement operation ID.
9. Verify the settlement receipt, selected input, paid amount, change, note balances,
   observer output, and post-run reconciliation. Grant and reveal one scoped viewing key
   only after the settlement is confirmed.
10. Add the verified settlement hash to `strk20.json`. Re-record the video only if there is
    enough time to upload it and verify the public link before the deadline.

This sequence needs an MCP client to supply the existing handles. The bundled
`agents/src/erebus_agents/demo_mcp.py` is not the mainnet runner: it always uses the mock
backend and opens fresh channels.

STRK20 screening: https://strk20-by-example.org/compliance
