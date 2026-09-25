# M5 experimental funded pool

This is a local, test-only proof of a funded shielded transition. It is not an audited pool,
an installable Erebus backend, or a Monad deployment. The setup uses known test entropy.
Never deploy these generated verifiers or keys with real funds.

## Reproduce

Requirements: Node 22, Anvil 1.5.1, and Circom 2.2.3 pinned to source commit
`ad44e915a12bb047b05745c2884aad9cc8326bc6`.

```bash
cd circuits/m5
npm ci
npm run prototype
```

The first run creates a local powers-of-tau file and three circuit-specific test keys. It can
take several minutes; subsequent runs reuse hash-checked artifacts under ignored `build/`.
The script compiles all three circuits, deploys generated verifiers, Poseidon, a mock token,
and the pool to local Anvil, then executes:

```text
deposit 150 -> private agreement-bound transfer 70 + change 80 -> seller discovers 70 -> withdraw 70
```

The runner checks token balances, Merkle roots, note and deal replay, proof input mutation,
and rejection of a fee-on-transfer deposit. Results and artifact hashes are in
`build/run.json` and `build/artifact-manifest.json`.

The contract uses one ERC-20 asset, a depth-20 append-only tree, and unbounded known-root
history. Deposit and withdrawal amounts are public. Each private transfer publishes a deal
commitment, nullifiers, and two output commitments in one transaction. Nothing here establishes
an anonymity set, recipient metadata privacy, safe setup, or production-grade wallet recovery.

The local script constructs witnesses in JavaScript. M5 remains open until the Rust suite-2
authorization path, local SDK prover, durable note wallet, indexer/reorg recovery, reviewed
artifact distribution, and adversarial contract/circuit tests are completed.
