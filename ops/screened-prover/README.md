# Screened local transaction prover

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

STRK20 screening: https://strk20-by-example.org/compliance
