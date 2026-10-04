# Metropolis M8 Progress

Updated: 2026-10-04. Branch: `metropolis`. M8 is **not complete**.
The public-bound live workflow, bounded disclosure observation, local artifact installation,
and local HTTP access recovery have evidence. Installed local x402 recovery and auditing also
have evidence. Released packages, hosted services, live Monad x402, and independent external
acceptance remain open.
The local negotiation SDK now freezes agreement transcripts before signatures and authenticates separate shielded agreement keys.
The native negotiation command supports cold-start private offers and durable consent in both modes, without payment submission.

## Review Fixes (2026-10-03)

- The shielded lockfile now accepts `--locked` builds after the EVM dependency change.
- The Metropolis index has priority over PyPI. The native wheel includes `erebus-selfhost`.
  An isolated install verified 14 native hashes and the launcher hash, then started,
  checked, and stopped the installed relay from an operator path containing spaces.
- Literal environment parsing preserves spaces and shell metacharacters without evaluation.
- The native seller publishes verified agreement evidence to its configured access root.
  The public-bound two-MCP-agent test passed without an evidence-copy watcher: one
  broadcast, finalized recovery, and successful payload retrieval. This is Anvil evidence.
- Access results explain hash, payment, and delivery verification without changing the flags.
- The harness uses monotonic elapsed time from settlement-call start to payment verification.
  Block timestamps remain diagnostics; inclusion/finality measurements are still open.
- Permit2 payment signatures are explicitly documented as public calldata.

The deployed settlement artifact was rechecked read-only with
`scripts/check-evm-deployment.py` at finalized block hash
`0x4ba79809c01f237e9ec11f80aae75b9b7ce6f642ed790ff21ac77db9d2c8f4bf`.
Both runtimes were 15,652 bytes; four immutable reference ranges were masked,
the remaining bytes matched, and `verifierVersion()` returned 1.
This is artifact identity evidence, not an audit or a new payment.

The [submission gates](metropolis-submission-gates.md) distinguish the supplied rubric
from local engineering evidence. A new live isolated-key workflow, external rehearsal,
published packages, and Monad demo video remain required.

Verification after these changes: both-mode freeze/restart/publication test passed;
the watcher-free public-bound MCP payment/access test passed; two publication safety
tests passed; shielded fmt and all-target clippy with `-D warnings` passed.
The Python workspace reported 394 passed, 4 skipped, and three existing failures in
`scripts/tests/test_demo.py` (legacy web component paths and video expectations).
Those failures are not evidence of a green branch and remain unresolved here.

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

1. **Versioned packages and a published fresh-install path.** Owner decision: a separate
   Metropolis registry. `scripts/build-metropolis-registry.py` builds an unpublished `.devN` index
   (three wheels, native binaries in a host-tagged `erebus-cli` wheel, hashed `release.json`), and
   `scripts/check-metropolis-install.py` installs it into a fresh environment and verifies it. Hosting
   and publishing the index are not done. Item 4 remains open.
2. **Hosted and self-hosting packages (items 6-7).** The four services run locally with health
   surfaces and configuration; a hosted testnet deployment and a packaged self-hosting path do
   not exist.
3. **x402 composition (items 12-13).** The decision gate and the measured comparison plan are
   recorded in [the x402 decision](metropolis-m8-x402.md); no scheme is selected and no code
   exists.
4. **Stage measurements (item 14).** `erebus-negotiate` reports negotiation; `erebus-payment`
   reports deployment authentication, observation, funding, local signing, and submission, plus
   artifact installation and proof preparation in shielded mode. Inclusion, finality, and
   delivery are not yet reported separately.
5. **The external rehearsal.** The local installed rehearsal passes
   (`funded_http_access_recovers_a_lost_response_after_service_restart_without_a_second_payment`,
   Anvil + funded public-bound payment + HTTP access service + restart recovery), and so does the
   native shielded path (`negotiation_cli` `payment_driver` tests: separate agents negotiate a
   suite-2 deal, the buyer proves locally and submits once, and recovery completes after a dropped
   RPC response; Anvil, known-entropy prototype keys). It has not been
   repeated from a fresh environment using only published packages and public documentation.

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

## Two Agents over MCP (2026-10-03)

The deterministic harness is `agents/src/erebus_agents/metropolis_loop.py`.
Each agent holds its own MCP client to its own `metropolis`-mode server and supplies only the
shared operation ID. Both negotiate concurrently. The buyer checks funding and calls
`settle_deal` once. It then observes with `recover_deal` until the payment is finalized and the
winning commitment matches the deal. It never calls `settle_deal` again, including after a
pending result or a failed call. Unit tests in `agents/tests/test_metropolis_loop.py` cover those decisions.

The funded test `two_mcp_agents_negotiate_and_settle_once_through_a_lost_broadcast` runs that loop
against two real stdio servers on Anvil with one-second blocks and the lossy proxy. Settlement
returns `pending` at `BroadcastUnknown`; one observation reaches `Finalized`. The proxy sees exactly
one raw transaction and the buyer's 80-unit change note is included.

Two headless Claude Code agents then drove the same loop through
`scripts/metropolis-agent-rehearsal.sh`. Each had no built-in tools and only its own server
(`--strict-mcp-config`, `--tools ""`), so neither could reach the other's server or the
repository. The prompts named the role and operation ID only. They did not mention the
no-second-payment rule. In two independent runs the buyer met the lost acknowledgement, chose
`recover_deal` over a second `settle_deal`, and cited the server's instructions. The seller made
one call. Each run sent one transaction and included the change note.

Debug-build timings from the harness run: negotiation 6.7 s (buyer) and 4.5 s (seller), artifact
download 3.5 s for 32 MB, proof preparation 52 s, local signing 6.7 s, and submission 4.2 s.
`local_signing` also covers re-verifying the selected agreement against the retained transcript
and the nonce-journal signing step, not only the signature.

Verified locally:

```sh
cargo test --locked --manifest-path sdk/shielded/Cargo.toml --test negotiation_cli shielded_driver -- --include-ignored
uv run --locked pytest agents/tests/test_metropolis_loop.py
```

This is Anvil with prototype keys and test artifacts, not Monad. The operation ID is agreed out
of band; there is no discovery handshake for it. Delivery through MCP was not part of this loop.

## Public-Bound Delivery, Timings, Installed Agents, and Self-Hosting (2026-10-03)

Owner decisions: finish the product public-bound first, then deploy a shielded testnet pool
(DM8-10); x402 composes against `exact` (DM8-11).

**Delivery and timings.** A finalized public-bound payment now reports `chain_times`: inclusion and
finalized-anchor block numbers and timestamps, read from the primary RPC as diagnostics only. The
agent loop retrieves the resource when the buyer's server exposes access. It retries a failed
retrieval by retrieving again and never by paying. It reports local elapsed time for
payment verification and delivery. Inclusion and finality timestamps are diagnostics,
not measured latency. In the combined MCP mode, buyer access is enabled by
`EREBUS_ACCESS_SERVICE_URL`, and the evidence directory is fixed to `<state_root>/agent` (DM8-12).

`two_mcp_agents_negotiate_pay_and_retrieve_public_bound_with_one_send` runs two real MCP servers
on Anvil (one-second blocks) through the lossy proxy. Settlement returns `pending` at
`BroadcastUnknown`, and observation reaches `Finalized`. The buyer retrieves the 37-byte payload
and its hash matches the agreed digest. The proxy sees one raw transaction, and the seller holds 70
tokens. The full deal took 11.6 s (debug build): negotiation 0.3 s, local signing 0.09 s,
submission 0.04 s, payment verified 2.3 s after submission, delivery 0.1 s. Anvil makes
inclusion and finality meaningless (`--slots-in-an-epoch 1`, whole-second timestamps). Those two
numbers are not latency measurements and have been removed from the current harness output.

**Installed packages only.** The registry was built from this tree and installed into a fresh
venv outside the repository. Two headless Claude Code agents then ran with each MCP server as
that venv's `erebus-mcp-server`, only the venv's `bin` on `PATH`, and no binary overrides. The
buyer negotiated, settled once, observed twice to `Finalized`, and retrieved the snapshot. The
seller made one call. One transaction was sent and the seller was paid 70. The chain fixture and
the seller's access service still run from the test (`hold_public_agent_environment`).

**Self-hosting.** `scripts/metropolis-selfhost.sh` and the
[self-hosting guide](metropolis-self-hosting.md) start, check, and stop the services from
installed binaries under one owner-only directory. Verified locally with installed binaries:
relay and access service healthy, relayer configuration accepted, state retained across
restart. The indexer was not exercised (shielded only).

**Friction found.** The seller's negotiated agreement never reaches its own access service
without an operator step ([F48](friction.md)). Agents misread the access flags `resource_verified`,
`delivery_verified`, and the seller-reported payment claim ([F49](friction.md)).

Verified locally:

```sh
cargo test --locked --manifest-path sdk/shielded/Cargo.toml --test negotiation_cli -- --include-ignored
uv run --locked pytest agents/tests/test_metropolis_loop.py mcp-server/tests/test_metropolis.py sdk/py/tests/test_metropolis_seam.py
```

Not done at that checkpoint: publishing or hosting the registry, x402 `exact` composition, wallet onboarding, local
proving from installed packages (deferred with the shielded pool), and anything on Monad.

### x402 HTTP Composition (Local)

The access service now supports an explicit `x402_exact` backend. The buyer persists a fixed
Permit2 authorization before HTTP. The seller checks the signed agreement and matching v2
header, journals the exact gas-funded transaction, and fences its broadcast before sending.
Retries observe the same transaction at two configured finalized RPC anchors. No ordinary
payment fallback, new permit, or automatic resend is permitted.

`x402_http_submits_once_recovers_restart_and_rejects_mutations` runs separate native access
client/service processes with canonical runtime fixtures on Anvil. It rejects changed amount,
recipient, deadline and signature; recovers a lost broadcast acknowledgement and service
restart; retrieves the agreed payload; and asserts one send and exactly 70 tokens paid.
Buyer permit recovery is also tested after each of four durable-write boundaries. The two
RPC endpoints in this test are a proxy and the same Anvil node, not independent providers.
The agreement uses a fixture with an empty transcript, not an installed negotiation run.

Python and dedicated access MCP configuration require explicit x402 opt-in. Ordinary combined
settlement tools reject that rail. The client still reports `payment_verified: false`: its
resource hash check and the seller's payment assertion are not an independent auditor check.

Still open: the installed two-agent x402 rehearsal, independent x402 auditor verification,
live Monad composition, complete stage measurements, publication/hosting and external
fresh-environment acceptance. See [configuration and limits](metropolis-m8-x402.md).


## Negotiated x402, Independent Audit, Measurements, and Installed Rehearsal (2026-10-04)

All evidence below is local (Anvil and a fresh venv on macOS arm64) unless stated otherwise.
Nothing was committed, published, deployed, or broadcast.

**Negotiated two-process x402.** `negotiated_x402_settles_once_recovers_restart_and_audits_from_the_grant`
runs real authenticated discovery and encrypted Noise negotiation between separate buyer and
seller processes with separate key directories. The seller publishes the accepted agreement
through its configured `access_evidence_root` (no test watcher). The buyer prepares one fixed
Permit2 authorization; the x402 access service journals it, fences the broadcast, and submits
exactly one transaction. The test drops the first response, restarts the service, retries with
the same permit, retrieves the agreed payload, and asserts one send and the agreed seller
balance. It then exports an encrypted grant and verifies the payment through the auditor path.
Measured (debug build, local monotonic): negotiation 759 ms, permit preparation 17 ms, first
observed inclusion 81 ms after submission, finalized verification 290 ms after submission,
delivery 742 ms, proof null (public-bound has no proving stage).

**Independent x402 auditor.** `erebus-disclosure` and `erebus-shielded-disclosure` accept
`rail: "x402_exact"` in `verify_payment`. From the encrypted grant, auditor key, and public
configuration alone, the auditor authenticates both pinned canonical runtimes, decodes the
permit and signature from the finalized transaction input, and requires matching finalized
target, calldata, transfer, and Permit2 nonce evidence at two endpoints. Tests reject a
buyer-signed permit paying another recipient, tampered grants, neighboring deals, non-payment
transactions, and unmined observations (pending, exit 2). No participant state, spending key,
or prover is needed.

**Stage measurements.** The public-bound agent loop now reports `stages_ms` with negotiation
(both roles), proof (null for public-bound), signing, submission, first observed inclusion,
finalized verification, and delivery, all from local monotonic clocks. Block timestamps remain
diagnostics. No shielded comparison is inferred from public-bound numbers; shielded proof
timings remain driver-reported.

**Release packages.** `scripts/build-metropolis-registry.py --profile release` produced
`0.3.0.dev20261004` for macOS arm64 (`source_commit 8a1d365`, `dirty_source: true`), and
`scripts/check-metropolis-install.py` installed it into a fresh venv outside the repository:
14 binaries verified, no source imports, self-host relay healthy. The Linux x86_64 leg of
`.github/workflows/metropolis-registry.yml` is prepared but was **not** verified locally: the
available x86_64 environment is an emulated colima VM that repeatedly reset under a full
release build. Linux platform qualification therefore remains CI-only and unexecuted; do not
claim Linux packages from this run.

**Installed rehearsal with disclosure.** A fresh venv installed only from the local registry
(`/tmp/erebus-installed`) ran both headless agents against the installed `erebus-mcp-server`,
with only the venv's `bin` on `PATH` and no binary overrides. The agents negotiated, settled
once through the lossy proxy, recovered by observation, and retrieved the snapshot; the held
environment then exported a grant and verified the payment with the installed disclosure
binary. Result: one raw transaction, seller balance 70, `agreement_verified: true`,
`payment_verified: true`, `delivery_verified: false`. This is a team-operated rehearsal, not
independent external acceptance; the environment fixture and Anvil remain operator-held.

**Linux build attempt.** The release registry was copied into the x86_64 colima VM and the
toolchain was installed there (cargo 1.99.0, uv 0.12.23), but the VM reset its SSH and Docker
endpoints under load before the build completed. The exact CI job remains the qualification
path; record the failure honestly rather than substituting a cross-build.

## Review Corrections and Installed x402 Gate (2026-10-04)

The agent driver's `stages_ms` now converts all second-valued durations to milliseconds.
The calldata decoder rejects oversized offsets, trailing bytes and nonzero ABI padding
without unchecked range arithmetic. The release workflow runs pytest in the project
environment and deduplicates matching pure-Python assets before upload.

`check-metropolis-install.py --rehearse-x402` passed against locally built release packages
`0.3.0.dev2026100401` on macOS arm64. It verifies installed hashes and imports, then runs the
Anvil fixture with installed negotiation, access-service, buyer-access and auditor binaries.
The installed access MCP server verifies durable cached retrieval without native overrides
or source imports. The fixture still orchestrates the local chain from this checkout; this
is not independent external acceptance or a fresh-machine guide without a fixture.

Measured in that run: negotiation 468 ms, native permit preparation plus failed HTTP 25 ms,
first observed inclusion 47 ms, finalized verification 139 ms, delivery 319 ms, proof null.
The permit measurement includes an intentional failed HTTP request; it is not signing-only
latency. One transaction was sent, the agreed resource was recovered after restart, and the
independent auditor verified the payment from its grant.

The owner approved publication, and `PoulavBhowmick03/erebus-metropolis` now exists with
HTTPS Pages enabled. No package assets have been published yet. Render is the requested
hosting target; the owner selected preparation only, with no paid provisioning. No hosted service or
new Monad transaction is claimed. The prepaid/batch comparison and complete live workflow
remain open, alongside Linux qualification and external acceptance.

### Clean Revision Package Check

Commit `f4714bba42b6909b860ba04d301ab23e4c3c3899` was pushed to `origin/metropolis`.
A release-profile macOS arm64 build at `0.3.0.dev2026100402` recorded
`dirty_source: false`. Its isolated install check verified 14 native binaries, the self-host
launcher, package imports outside the checkout, relay health, and the installed local x402
recovery/auditor rehearsal. It reported `installed_x402_rehearsal: true`,
`live_payment: false`, and `published: false`.

That run measured negotiation 475 ms, permit preparation plus intentional failed HTTP 25 ms,
first observed inclusion 51 ms, finalized verification 144 ms, and delivery 304 ms.
The proof stage was absent. These remain local Anvil measurements, not Monad latency.

The [dual-platform qualification run](https://github.com/PoulavBhowmick03/Erebus/actions/runs/37180669150)
builds both platforms from this revision. Publication waits for both isolated install checks
and the pure-Python wheel comparison; starting that workflow is not qualification evidence.

**Hosting boundary (owner, 2026-10-04).** Prepare and test the Render deployment files only.
Do not provision paid services. The hosted-service acceptance gate remains open; a prepared
blueprint and a local self-hosting test do not close it.

## Qualification, MCP x402, Model Comparison, and Live Rehearsal (2026-10-04)

**Qualification findings.** Registry run 37180669150 (`0.3.0.dev1`, `f4714bb`) passed CI checks but
is not publishable. Its macOS wheel was tagged `py3-none-macosx_10_9_universal2`, yet all 13
native binaries are thin arm64 with `minos 11.0`. Hatch had tagged the wheel from the runner's
universal2 Python. `5a76bbd` tags from the host OS and machine and pins `MACOSX_DEPLOYMENT_TARGET=11.0`.
Its rebuild, run 37182102672 (`0.3.0.dev2`, `cbfc2e5`), passed both platforms and the comparison. Full
artifact verification then found **no license file in any wheel**. `171be31` ships Apache-2.0
`LICENSE` in every wheel and, in the native wheel, `THIRD_PARTY_NOTICES` for all 706 crates linked into
the binaries. For the 96 crates that publish no license text, the notices give their copyright holders
and append the canonical texts. Neither `dev1` nor `dev2` was published.

**CI.** The M5 job had failed on every run since at least `191dbf5`. It installed Foundry but never ran
`forge build`, so `access_http` could not find `contracts/evm/out`, and every later step in the job
(the x402 HTTP and audit tests) never ran. `cbfc2e5` builds the contracts. Run 37182102674 then passed
`access_http`, `x402_http`, the x402 audit test, and the MCP agent tests for the first time in CI.
It failed only where the shielded access request outran the test client's 10 s timeout on a runner
2.5x slower than local; `44c754a` gives access requests their own bound. The Python job's three
`scripts/tests/test_demo.py` failures are a stale legacy check: `scripts/check-demo.py` still enforces
the pre-redesign Starknet landing page (`LeakLedger.tsx`, `Evidence.tsx`, a self-hosted video),
which the 2026-09-15 web redesign removed. They are unrelated to Metropolis and left unfixed
pending a decision on what the redesigned site must claim.

**x402 through MCP.** The agent driver has an explicit `x402-exact` profile with no fallback. The buyer's server must
expose exactly `negotiate_deal` and `retrieve_service_access`; the first retrieval is the paid request, and
every retry is a retrieval. `two_mcp_agents_pay_over_x402_through_a_dropped_response_and_restarts`
negotiates and pays through two MCP servers. It drops the response to the first paid request, restarts
the seller service and the buyer's MCP processes, and then requires one broadcast, one retained permit,
resource recovery, and independent auditor verification. The installed rehearsal runs it against
installed servers whose `PATH` holds only installed commands; it passed twice from a fresh environment.
The driver previously lost `HarnessError` inside the MCP task group (an `ExceptionGroup`), and the x402
access tool pointed payment verification at a `recover_deal` that x402 mode does not have. Both are fixed.

**Service-model comparison.** Measured on the canonical Permit2, exact, and upto runtimes; see
[the x402 record](metropolis-m8-x402.md#measured-service-model-comparison-2026-10-04). Prepaid and
batched are experiments only.

**Operator setup and the live rehearsal.** No product command created participant keys, signed
descriptors, or the canonical terms template, so neither a third party nor a live rehearsal could
start from packages. `erebus-negotiate` now has `prepare_operator` and `prepare_terms`, and
`erebus-settle` has a read-only `address` method. A test negotiates between participants created only
by these requests. [`scripts/metropolis-monad-rehearsal.py`](metropolis-monad-rehearsal.md) uses them for an
executable live Monad run. Its read-only `init` and `preflight` passed against Monad testnet and stopped at
funding; no transaction was sent. On a local chain configured as 10143, the unmodified `run` phase
completed both rails; that is script validation, not Monad evidence.

**Registry index.** The registry's Pages workflow indexed only the dispatched release, so a second
version would have made the first uninstallable. `2708983` changes the template to index every
published release. Applying it to `erebus-metropolis` needs a `gh` token with `workflow` scope; the
current token has only `repo`.

## Published Metropolis Packages (2026-10-04)

`v0.3.0.dev4` is published as a prerelease on `PoulavBhowmick03/erebus-metropolis`, served by the
HTTPS index `https://poulavbhowmick03.github.io/erebus-metropolis/simple/`. It was built from clean
`e9f10d0` by registry run 37186885901, which passed both platforms, the isolated installed x402
rehearsals (including the MCP agent test), and the cross-runner pure-wheel comparison.
`scripts/publish-metropolis-release.py` then re-verified both artifacts locally and published four
deduplicated wheels plus the combined manifest. It checked source revision, version, clean tree,
wheel, native, and launcher hashes, architectures, tags, and licenses. Every uploaded asset's
digest matches the manifest, and the Pages index links each wheel by SHA-256. No earlier version was
published, and the stable Starknet index was not touched.

Public install, verified from a fresh environment outside the checkout with a fresh uv cache:
- imports resolve inside the environment;
- all 14 native binaries and the self-host launcher match the published manifest;
- the operator setup commands work, and `erebus-selfhost` serves a healthy relay;
- the installed MCP server exposes the expected tool sets for the seller, the settlement buyer, and
  the x402 buyer.

Both funded x402 rehearsals also passed against these binaries. They use checkout fixtures, so this
is team-operated evidence, not independent external acceptance. GitHub release-asset downloads timed
out repeatedly from this machine during publication; uv's `UV_HTTP_TIMEOUT=300` was needed once.

Open: the registry's Pages workflow still indexes only the dispatched release until a token with
`workflow` scope applies the all-releases template (`2708983`). A second publication before that
would drop `0.3.0.dev4` from the index.
