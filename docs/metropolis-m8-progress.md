# Metropolis M8 Progress

Updated: 2026-10-02. Branch: `metropolis`. M8 is **not complete**. Network readiness is
verified; deployment and the installed developer workflow are not.

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
tests (help, malformed input, capabilities, receipt failure paths, funding failure paths) and a
funded Anvil test that reports `funded: true` with zero shortfalls for a funded buyer. A funded
Anvil `receipt` happy path and the rest of the installed workflow (packages, fresh-install
guide, Python/MCP) are not built.

## Not done

**Deploy to Monad testnet (item 2).** Everything is ready except the funded key and the
operator's broadcast decision. No transaction has been signed or submitted. This is the gate:
deploying spends testnet funds and is irreversible.

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

1. A funded Monad testnet key and explicit authorization to broadcast, for item 2. The key must
   be dedicated testnet material and must not be committed.
2. A decision on the remaining M8 scope: the installed workflow (items 3-5, 8) and the access
   service (items 9-11) are each substantial. They can proceed in parallel with deployment, but
   the milestone's Done criterion needs the deployed contract first.

M8's Done criterion — independent buyer, seller, observer, and disclosure processes completing
the real Monad workflow, with hosted and self-hosted support and recoverable access issuance —
is unmet. Nothing here is mainnet evidence.
