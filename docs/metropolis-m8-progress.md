# Metropolis M8 Progress

Updated: 2026-10-02. Branch: `metropolis`. M8 is **not complete**. Network readiness and a
verified Monad testnet deployment are done; the installed developer workflow and the access
service are not.

## Deployed

`ErebusSettlement` is live on Monad testnet at
`0xa5f0c864f434331bef9a7fc5e05450d598d24da4` (chain 10143, verifier version 1), deployed in
block 67495473 by transaction
`0x6c7b810c837c038753815df985c3b9ce210be3b40df6fab4ec0682e1a271ca65`. The manifest is
`contracts/evm/deployments/monad-testnet.json`.

Identity was verified before the address was trusted:
`scripts/check-evm-deployment.py` pinned the finalized block
(`0x1b01c4fac8ce92fa33496b80752a0bdfbed65eb2aeaec6c21aac878b551e2aa6`), read the runtime code
by block hash, masked the four immutable slots, matched the artifact byte-for-byte (15,652
bytes), and read `verifierVersion()` = 1. The deployer key is dedicated testnet material kept
outside the repository in a mode-`0600` file.

### Live public-bound settlement

The coordinated lifecycle completed on the live deployment. A test token
(`0x902f79145059910ef875aecf4187c771b204ea14`) was minted to the buyer and approved; then
`sdk/evm/examples/settle_public_bound.rs` ran the full flow against Monad testnet: durable
intent, both authorizations, preparation, nonce claim, local signing, journaled broadcast,
finalized observation, and reconciliation. Result:

| Field | Value |
|---|---|
| Transaction | `0x1f7ec208a7b03b835f224ac989cc59c9aee4c33634a2b04498b8394ddede4c09` |
| Deal commitment | `4511793857dd5a2f50e5f3910b034a652127bfababaae9abedeae68c02be1abd` |
| Deal nullifier | `3cfeea17e120c471e435534a46036fa6542dbf19b9ef50dfd598628b1e757360` |
| Deal state | `PaidFinalized`, `payment_finalized: true` |
| Seller balance | 3,000,000 base units after three settlements; buyer 0 |

**Live-chain fix.** The one-shot observer scans from genesis and the public RPC caps
`eth_getLogs` at a 100-block range, so it cannot serve a chain 67M blocks tall. The resumable
observer now has `finalized_deal_evidence_resumable_from`, which starts a fresh scan at the
deployment block and checkpoints it; the caller must pass a block at or before the first
possible settlement. The example waits for the winner to reach `finalized` before reporting a
receipt, which is why Monad's trailing finalized anchor is exercised.

## Verified

**Monad network parameters and verifier support (checklist item 1).** Official Monad sources
(fetched 2026-10-02) confirm chain ID 10143 for testnet, the `latest` / `safe` / `finalized` /
`pending` block tags with `finalized` as the irreversible tag, the public RPC endpoints and
their rate limits, the faucet, the current release (`v0.15.2` / `MONAD_NINE`, reset from
genesis on 2025-12-16), and that Monad execution requires **Foundry v1.8 or later** (the legacy
Monad Foundry fork is deprecated). `contracts/evm/foundry.toml` carries a `[profile.monad]`
for that toolchain; this repository's local Foundry is 1.5.1, so the profile is inert here and
the RPC path is unaffected. The docs also fix the size limits (128 KB code, 256 KB initcode;
the settlement contract is about 15 KB) and the canonical Permit2, Multicall3, and x402 proxy
addresses. A read-only diagnostic (`sdk/evm/src/readiness.rs`, `erebus-network-check`) then
checked the live public RPC:

```sh
echo '{"chain_id":10143,"rpc_url":"https://testnet-rpc.monad.xyz","timeout_ms":10000}' \
  | ./sdk/evm/target/debug/erebus-network-check
```

All 16 checks passed against `testnet-rpc.monad.xyz` (2026-10-02):

- chain ID 10143; explicit `finalized` block 67489534 (`0x0c4bf7b2…`), head 67489537;
- hash-pinned `eth_getTransactionCount`, `eth_getBalance`, `eth_getCode`, and `eth_getLogs`;
- an unknown block hash is rejected by the RPC;
- BN254 add, mul, pairing-accept, and pairing-reject vectors return the EIP-197 results, so the
  precompiles a Groth16 verifier needs behave correctly on this network.

The report never contains the endpoint. Tests: three unit tests cover invalid configuration,
endpoint redaction in `Debug`, and the fixed BN254 vectors; an ignored Anvil integration test
(`sdk/evm/tests/network_check.rs`) proves the pass path, the wrong-chain fail-closed path, and
that a report does not echo the URL.

**Deployment verification tooling.** `scripts/check-evm-deployment.py` pins `finalized`, reads
the runtime code by block hash, masks immutable slots from the artifact's
`deployedBytecode.immutableReferences`, and compares the code byte-for-byte; with
`--verifier-version` it also reads `verifierVersion()` at the pinned block. Verified against a
local deployment: the correct version passes, a wrong version fails. The manifest shape is
`contracts/evm/deployments.example.json`, and the procedure is
[M8 runbook](metropolis-m8-runbook.md).

**Buyer onboarding and receipt output (checklist item 3).** `erebus-settle` is a read-only,
one-request-per-process command:

- `capabilities` reports the public-bound backend's declared suites, modes, guarantees, and
  local-proving flag, so a caller can select it without a connection.
- `funding` reports the buyer's token allowance and balance against the signed amount plus fee
  and the gas payer's native shortfall, before anything is authorized.
- `receipt` reads one durable agreement opening from the coordinator state, observes finalized
  chain evidence, classifies the deal and revision, and reports the commitment, nullifier,
  anchor, winner, and payment-finalized state.

It signs nothing, submits nothing, and never prints the endpoint, terms, or signatures. The
read-only backend constructor (`EvmSettlementBackend::read_only`) cannot submit. Tests: five CLI
tests (help, malformed input, capabilities, receipt failure paths, funding failure paths) and
two funded Anvil tests: `funding` reports `funded: true` with zero shortfalls for a funded
buyer, and `receipt` reports `paid_finalized` with the winner's amount and fee after a real
settlement. The rest of the installed workflow (packages, fresh-install guide, Python/MCP) is
not built.

## Not done

**Deploy to Monad testnet (item 2) is done** (see Deployed above), and a live public-bound
settlement finalized on it. What is not done is the *independent-process* workflow: the live
run used one process that holds both authorization keys and the observer. Separate buyer,
seller, observer, and disclosure processes, the installed packages and fresh-install guide,
the Python/MCP boundary, the access service, x402, and stage measurements remain.

The remaining items are not started or only partially present:

- **CLI onboarding, funding diagnostics, backend selection, and receipt output (item 3).**
  The existing `erebus-cli` onboarding and `doctor` are Starknet/STRK20-oriented. There is no
  EVM backend selection or public-bound receipt output in the installed CLI.
- **Versioned testnet packages and fresh-install guide (item 4).** Not published. The M7
  disclosure binary installs locally (`cargo install --path sdk/shielded`), which is not a
  released package.
- **Local proving from the installed package (item 5).** Proof artifacts are local M5 build
  outputs with test keys; there is no hash-checked shared-artifact installation.
- **Shared services and self-hosting (items 6-7).** The message relay, transaction relayer, and
  pool indexer run locally and have health surfaces and runbooks. Hosted deployment needs
  measured capacity, quotas, retention, and an incident owner (D05/D07).
- **Python boundary, MCP configuration, two-agent harness (item 8).** M7 added disclosure
  tools; the MCP/Python path still speaks the Starknet wire, not the Monad settlement flow.
- **HTTP service adapter and access issuance (items 9-11).** Not built. Payment and delivery
  are separate states in the core, but there is no adapter that binds access to a finalized
  agreement and buyer, persists issuance, or retries delivery without a second charge.
- **x402 composition (item 12).** Not built, and it needs a specified scheme first.
- **Stage measurements (item 13).** Negotiation, proof, submission, inclusion, finality, and
  delivery are not instrumented into one measured run.

## What is needed

1. The installed workflow (items 3-5, 8): a buyer-side CLI that drives the journaled
   settlement on the deployed contract, test-token funding, the fresh-install guide, and the
   Python/MCP boundary. `erebus-settle` currently covers capabilities, funding, and receipt.
2. The access service (items 9-11) and x402 composition, now unblocked by the canonical
   testnet addresses in the [M8 runbook](metropolis-m8-runbook.md).
3. Stage measurements on the live run.

M8's Done criterion — independent buyer, seller, observer, and disclosure processes completing
the real Monad workflow, with hosted and self-hosted support and recoverable access issuance —
is unmet. The deployment is testnet evidence only, not mainnet.
