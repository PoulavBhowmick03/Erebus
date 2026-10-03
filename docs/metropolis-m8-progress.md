# Metropolis M8 Progress

Updated: 2026-10-03. Branch: `metropolis`. M8 is **not complete**.
The public-bound live workflow, bounded disclosure observation, local artifact installation,
and local HTTP access recovery have evidence. Released packages, hosted services, x402,
and a complete installed two-agent workflow remain open.
The local negotiation SDK now freezes agreement transcripts before signatures and authenticates separate shielded agreement keys.
The native negotiation command supports cold-start private offers and durable consent in both modes, without payment submission.

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

### Independent processes on the live deployment

The M8 workflow now runs as separate processes against the deployed contract:

1. **Buyer proposes** (`settle_public_bound`, `EREBUS_EVM_PROPOSAL_OUT`): writes canonical terms
   and the blinding. The hardened source holds the buyer key but no seller key.
2. **Seller signs** (`seller_sign`): a separate process with only its own key reads the
   proposal, checks that the terms name its key, and returns the seller authorization. It never
   sees the buyer key and never contacts a chain.
3. **Buyer settles** (`settle_public_bound`, `EREBUS_EVM_PROPOSAL_IN` +
   `EREBUS_EVM_SELLER_AUTHORIZATION_FILE` + `EREBUS_EVM_EVIDENCE_OUT`): verifies the seller
   authorization against the proposal, runs the coordinated lifecycle, and writes disclosure
   evidence.
4. **Independent auditor** (`erebus-disclosure export` then `verify_payment`): a separate
   process with only the encrypted grant, its own auditor key, and an RPC configuration.

Recorded live result (2026-10-02):

| Field | Value |
|---|---|
| Settlement transaction | `0x6169b70f2efc1aa83b75f92b7b05074414cf24dcc83c3509133c707a0fdd1fda` |
| Deal commitment | `ca7d4cb1c3752b48812d5e8bb6804f636d5e84f096cc573fad1da8960f03f3a1` |
| Deal nullifier | `e96d5a7c337202528a422d83fd5cc86f2a1408441cfb3ae58cf0bd0ab0bc091c` |
| Auditor result | `agreement_verified: true`, `payment_verified: true`, `delivery_verified: false` |

The historical live run used separate processes, but its buyer example still constructed a
deterministic test seller key. That source behavior is now removed from the independent path.
The local test seller requires explicit development opt-in. No new live run proves this change yet.
The proposal transport remains a plaintext test fixture, not the encrypted M2 negotiation flow.

**Disclosure observation fixed locally.** The CLI now uses the durable, bounded observer.
Anvil tests verify old deals across fresh processes with small query budgets.
Pending observation returns exit code `2` and never claims verified payment.
Corrupt cache state fails closed.

## New local developer components

`erebus-negotiate` connects signed discovery identities, Noise transport, typed negotiation, and durable authorizations.
Each participant uses its own configuration and keys. The seller receives no shared private draft file.
The buyer generates fresh deal randomness locally; recovery reuses the retained draft and synchronizes transcript roots.
The shielded seller stores its expected payment note before sending final consent.
The buyer retains one coordinator policy reservation and both signatures.
Copied-command tests cover direct acceptance, counteroffers, frozen restart, repeated consent, and both settlement modes.
Malformed requests, unsafe configuration files, changed identities, and invalid synchronization data fail closed.
This command has no RPC, proving, transaction-signing, or payment-submission path.
It does not complete the combined installed settlement/MCP flow.
See the [negotiation runbook](metropolis-negotiation-runbook.md).

Local verification on 2026-10-03 passed the transport and shielded `--locked --all-targets` suites.
The EVM independent-process negotiation test passed separately.
Strict Clippy, rustdoc, and formatting checks passed for the affected Rust components.
The funded tests marked `ignored` and live Monad writes were not rerun for this negotiation change.
The command test copies the binary to a temporary directory and clears each participant's environment.
This verifies independent component execution, not a published or cross-platform release.

`erebus-artifacts` installs manifest-authenticated public proving artifacts.
It verifies hashes and lengths, persists atomically, and repairs corrupt regular cache entries.
`erebus-local-prove` reads an owner-only local witness, generates a proof, and verifies it locally.
It uploads no witness and submits no transaction.
The manifest digest must come from an independently trusted release source.

The Python boundary and `EREBUS_BACKEND=local-prover` MCP mode expose this local proof path.
An isolated wheel test imports SDK and MCP code from a temporary installed environment.
It proves a deposit twice, with only three artifact downloads and cache reuse on the second run.
This is component installation evidence, not a published platform release.

Fresh Rust CLI tests prove deposit, transfer, and withdrawal using downloaded M5 artifacts.
Those artifacts have known test entropy and require explicit development opt-in.
They are not secure release keys.

The HTTP access service authenticates the existing buyer agreement key.
It checks the signed snapshot digest and matching finalized payment before durable issuance.
The public-bound funded HTTP test verifies pending rejection, wrong-buyer rejection, and restart recovery.
Seller balance and gas-payer nonce remain unchanged during retrieval retries.
The funded shielded runner also verifies access in a separate service process.
It rejects a wrong buyer and restores the same issuance after service restart.
The coordinator records exactly one broadcast throughout retrieval recovery.
The same run completes deposit, transfer, independent disclosure, output recovery, and withdrawal.
This is local Anvil evidence with known-entropy prototype keys, not a live shielded deployment.
The service exposes no payment submission path.
The buyer-side `erebus-access` command signs only a short-lived retrieval request.
It verifies returned bytes against the signed fulfillment digest and persists them in a private cache.
After a completed download, a fresh process can retrieve locally without the seller or signing key.
Tampered bytes, symlinks, malformed responses, and each durable-write fault fail closed or recover correctly.

Both funded modes pass retrieval through independent access commands and real MCP clients.
The public-bound test also passes through isolated SDK/MCP wheels and a copied native binary.
Production imports come from the temporary environment, not the source checkout.
This proves component installation, not a public platform package release.

The HTTP test corrupts issuance storage after finalized payment.
The service returns `paid_but_undelivered`, no payload, and `retry_without_payment: true`.
CLI and MCP preserve that seller-reported outcome without claiming independent payment verification.
Retrieval receipts report verified resource content, not independent payment or delivery evidence.
See [M8 decisions](metropolis-m8-decisions.md) and the [access runbook](metropolis-access-runbook.md).
See the [local proving runbook](metropolis-local-proving-runbook.md) for artifact trust and MCP configuration.

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

**Deploy to Monad testnet (item 2) is done** (see Deployed above).
The independent-process public-bound run is recorded above.
The complete product gate still requires released packages, hosted and self-hosted services,
installed agent integration, recoverable live access, x402, and stage measurements.

The remaining items are not started or only partially present:

- **CLI onboarding, funding diagnostics, backend selection, and receipt output (item 3).**
  The existing `erebus-cli` onboarding and `doctor` are Starknet/STRK20-oriented. There is no
  EVM backend selection or public-bound receipt output in the installed CLI.
- **Versioned testnet packages and fresh-install guide (item 4).** Not published. The M7
  disclosure binary installs locally (`cargo install --path sdk/shielded`), which is not a
  released package.
- **Local proving from the installed package (item 5).** Component wheel installation and
  hash-checked artifact installation pass locally. Platform packaging, secure artifacts,
  and authenticated public release distribution remain open.
- **Shared services and self-hosting (items 6-7).** The message relay, transaction relayer, and
  pool indexer run locally and have health surfaces and runbooks. Hosted deployment needs
  measured capacity, quotas, retention, and an incident owner (D05/D07).
- **Python boundary, MCP configuration, two-agent harness (item 8).** M7 added disclosure
  tools. Local proving and access have separate Python/MCP component paths.
  The new Rust negotiation SDK has typed offers, counters, bilateral acceptance, durable freeze,
  encrypted final authorizations, and descriptor-signed shielded-key bindings.
  A separate-process test exercises public-bound negotiation, restart, coordinator authorization,
  and an independent scoped auditor. It submits no payment.
  The complete installed negotiate-to-Monad-payment-to-access MCP workflow remains open.
- **HTTP service adapter and access issuance (items 9-11).** Implemented locally for immutable
  snapshots. Both funded modes pass Rust and MCP retrieval and recovery.
  Buyer-side tooling has isolated component installation evidence.
  Live Monad service operation, released packages, and the full installed private workflow remain open.
- **x402 composition (item 12).** Not built, and it needs a specified scheme first.
- **Billing comparison and stage measurements (items 13-14).** Negotiation, proof, submission, inclusion, finality, and
  delivery are not instrumented into one measured run.

## What is needed

1. The installed workflow (items 3-5, 8): a buyer-side CLI that drives the journaled
   settlement on the deployed contract, test-token funding, the fresh-install guide, and the
   Python/MCP boundary. `erebus-settle` covers capabilities, funding, and receipt.
   `erebus-payment` now connects native public-bound negotiation to durable signing, submission,
   and paired recovery locally. Native shielded settlement and packaged composition remain open.
2. Installed and live access integration (items 9-11), plus specified x402 composition.
3. Stage measurements on the live run.

M8's Done criterion — independent buyer, seller, observer, and disclosure processes completing
the real Monad workflow, with hosted and self-hosted support and recoverable access issuance —
is unmet. The deployment is testnet evidence only, not mainnet.

## Local negotiation integration (2026-10-03)

Owner decisions: freeze negotiation before final signatures; add a signed shielded-key identity binding.
The M1 encoding and M2 hashes are unchanged.
The new TCP wrapper authenticates both descriptors, bounds frames and whole-operation deadlines,
and discards every failed Noise connection.

`sdk/evm/tests/negotiation_flow.rs` starts separate buyer and seller processes.
They negotiate price, freeze the same root, exit, and reconnect with a fresh handshake.
They re-deliver accepted messages without changing that root.
The buyer's coordinator reserves capacity before signing and stores the seller authorization.
The seller persists final authorizations separately and exports an encrypted scoped grant.
A fresh auditor verifies the agreement using only that grant and its own key.
The test reports `payment_verified: false` because it submits no chain transaction.

Shielded peer tests exchange private descriptor-signed key bindings and verify both final agreement signatures.
They reject key substitution before negotiation and leave the negotiation transcript empty during identity binding.
They do not prove a live shielded Monad settlement or an installed combined agent product.
See the [negotiation runbook](metropolis-negotiation-runbook.md) for commands and boundaries.

## Native Negotiation to Payment and Access (2026-10-03)

The copied `erebus-payment` command continues a durably authorized native negotiation.
It checks the operator-pinned runtime and deployment block through both configured RPCs.
It uses the coordinator's preparation, nonce claim, local signing, and broadcast fence.
Once a broadcast attempt exists, later invocations observe only.
It never downgrades a shielded agreement to public-bound payment.

The funded local test now runs the actual 60-to-70-unit data-snapshot negotiation through payment,
delivery recovery, and independent payment disclosure.
Separate buyer and seller processes hold their own keys.
The proxy loses the broadcast acknowledgement after forwarding exactly one transaction.
Recovery completes with the transaction key and negotiation transcript unavailable.
Conflicting consumed-state responses cannot finalize payment or trigger another submission.
The service restarts after a discarded issuance response; the buyer retrieves that same issuance.
The buyer verifies the payload hash and restores it from cache without the key or service.
An auditor verifies the encrypted grant and payment with participant directories offline.

Verified locally:

```sh
cargo test --locked --manifest-path sdk/shielded/Cargo.toml --test negotiation_cli payment_driver -- --include-ignored
```

The test uses Anvil, not Monad. Public-bound payments reveal amount and parties.
Settlement calldata also reveals the full accepted agreement, blinding, and final signatures.
The offchain negotiation history remains private, not the final service terms.
This does not close native shielded integration, released packages, hosted services, x402,
the combined MCP workflow, or the live independent-product acceptance gate.
See the [payment runbook](metropolis-payment-runbook.md) for configuration and recovery limits.
