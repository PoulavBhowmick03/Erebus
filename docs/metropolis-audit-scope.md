# Metropolis Audit Scope

Preparation notes for an external reviewer of Erebus on Monad (Metropolis). Nothing here has been
audited. The deployment is test-only and single-operator, and its Groth16 setup is a test setup.
This page says what to review, what we believe the boundaries are, and what we already know is
weak. It is not a security claim.

Each statement points at the file it comes from. Where we infer, we say so.

## 1. Versions under review

| | |
|---|---|
| Source | `main` at `cbd6567` (WS5 segmented history, auditor checkpoint and USDC quickstart landed after the release) |
| Release | `0.4.0.dev2026101801`, built from `main` `8a2f97b`, native qualification run `39197664018` (macOS arm64, Linux x86_64), published 2026-10-19 ([roadmap](metropolis-roadmap.md), WS4) |
| Deployment record | [`contracts/evm/deployments/monad-testnet-shielded-v2.json`](../contracts/evm/deployments/monad-testnet-shielded-v2.json) |
| Record `manifest_sha256` | `268bf1abe34d738859a20d5db8cee96a30fd4dbb4b2e25b8b5b9da7084d59a4a` |
| Chain | Monad testnet, chain ID `10143` |
| WMON pool | `0x8868f35140f3602C80E2C76132740B6f0c0b409D` |
| USDC pool | `0x863f540471C0aD33c68D1218f31E8C9fe9cD50FB` |
| Public-bound settlement | `0xa5f0c864f434331bef9a7fc5e05450d598d24da4`, verifier version 1 ([`monad-testnet.json`](../contracts/evm/deployments/monad-testnet.json)) |

The record marks itself `"test_only": true` and `"audited": false`. It pins the runtime
`keccak256` of the pool, Poseidon, and the deposit, transfer and withdraw verifiers, and anchors at
block `69903054`. We infer that `manifest_sha256` is the digest of the proving-artifact bundle
manifest the deployment plan was prepared against; verify in
`circuits/m5/scripts/check-testnet-deployment.mjs` and `build-artifact-bundle.mjs`.

## 2. Components

### In scope

| Component | Paths | Notes |
|---|---|---|
| Shielded pool | `contracts/evm/src/ErebusShieldedPool.sol` | Immutable asset, Poseidon and verifier addresses; "No upgrade authority" (source comment). |
| Public-bound settlement | `contracts/evm/src/ErebusSettlement.sol`, `ErebusCodec.sol` | Canonical decoder, domain checks, `ecrecover` authorizations, deal nullifier ([contracts README](../contracts/evm/README.md)). |
| Contract tests | `contracts/evm/test/` | Adversarial matrix, vectors, reentrancy and fee-on-transfer mocks. |
| Circuits | `circuits/m5/deposit.circom`, `transfer.circom`, `withdraw.circom`, `lib/eddsa_strict.circom` | Groth16 verifiers are generated from these; they are not checked in as Solidity. Verify deployed runtimes against the record's pins. |
| Setup and bundle scripts | `circuits/m5/scripts/` (`testnet-setup.mjs`, `bundle-testnet-setup.mjs`, `build-artifact-bundle.mjs`, `checkpoint-history.mjs`, `check-testnet-deployment.mjs`) | Setup, artifact manifest and deployment checks. |
| Shared Rust core | `sdk/core`, `sdk/coordinator`, `sdk/journal` | Agreement encoding and spending policy, settlement coordination, the durable operation journal. Path dependencies of the crates below. |
| Transport | `sdk/transport` | Encrypted negotiation and `erebus-relay`. |
| EVM settlement | `sdk/evm` | Public-bound settlement, disclosure, `erebus-settle`, `erebus-tx-relayer`, `erebus-network-check`. |
| Shielded payments | `sdk/shielded` | Wallet, checkpoint, indexer, observation, proving artifacts, access service, relayer, disclosure, and the native commands under `src/bin/`. |
| Python binding | `sdk/py/src/erebus/_metropolis.py`, `_metropolis_wallet.py` | Should only marshal paths and results to the native commands. |
| MCP server | `mcp-server/src/erebus_mcp/metropolis.py`, `metropolis_errors.py`, `metropolis_wallet_tools.py`, `operation_refs.py` | The agent-facing tool surface. |
| Setup (`erebus-init --metropolis`) | `mcp-server/src/erebus_mcp/onboarding.py`, `metropolis_setup/` (`operator_init.py`, `quickstart.py`, `disclosure_config.py`, `stage_artifacts.py`) | Writes configs, keys (through Rust), MCP configs and prompts. |
| Release pipeline | `scripts/build-metropolis-registry.py`, `scripts/publish-metropolis-release.py`, `scripts/check-release-readiness.py` | How wheels and signed manifests reach users. |

`sdk/rs` is the Starknet client. None of the Metropolis crates above lists it as a path
dependency (checked in each `Cargo.toml`), so it is out of scope. The operations guide still
describes the call path as `sdk/py → sdk/rs`; we read that line as stale.

### Out of scope

- The legacy Starknet backend: `sdk/rs`, `sdk/ts`, `contracts/` outside `contracts/evm`, and the
  Starknet MCP backend.
- `circuits/m4` (the earlier prototype; its CI job was removed in `6d23cae`).
- `web/`, `demo/`, and the reference agents and use-case demos in `agents/`.
- Test-only mocks (`MockERC20`, `MockReentrantERC20`, `MockWMON`) except as test fixtures.
- The x402-exact rail through the third-party `x402ExactPermit2Proxy`, except where Erebus code
  builds or checks its requests.

## 3. Trust boundaries and assets

| Asset | Where it lives | Stated boundary |
|---|---|---|
| Key material (gas keys, spend secrets, agreement and discovery keys, auditor keys) | Owner-only files, e.g. `erebus/keys/` and each role's `keys/` | Rust creates and reads keys; "Key **paths** cross requests; key values do not" ([operations](metropolis-operations.md) §2). Quickstart gas keys are made by `erebus-settle generate_testnet_key`; "Python only handles paths" (`quickstart.py`). |
| Shielded notes | Encrypted wallet snapshots | Snapshots are encrypted; transcripts and witness inputs are plaintext under file permissions (operations §2, [threat model](metropolis-threat-model.md) §2). |
| Disclosure grants | Files exported for an auditor | Scoped to one deal; must hold neither session keys nor spending secrets. Expiry is an access rule, not erasure; revocation cannot retract disclosed data (threat model §5). |
| Operation journals | State roots | Durable intent before broadcast; the signer journal binds target, calldata hash, fees, gas and value (threat model §8, M5C). Records and blobs are not encrypted; symlink checks assume trusted parent directories (threat model §2). |
| Chain observation | Two configured RPC providers | Paired finalized observation: both must agree on chain, finalized block and hash. Provider agreement is not a cryptographic state proof ([checkpoint design](metropolis-evidence/ws1-checkpoint-design.md) §2). |
| Fresh-wallet checkpoint | `<state_root>/history-checkpoint.json` | Pool frontier read from both providers at one finalized block and recomputed locally to `currentRoot`; earlier events are not available to a checkpointed index (checkpoint design §1–2). |
| Proving artifacts | Artifact cache | Manifest lists HTTPS URL, length and SHA-256 per file; non-test manifests need a detached publisher signature; the publisher address is pinned independently; the manifest does not authenticate ceremony evidence (operations §3, §7). |
| Releases | `poulavbhowmick03/erebus-metropolis` prereleases and Pages index | `publish-metropolis-release.py` requires a successful qualification run on the source commit, owner authorization, clean-tree packages, and verified native reports before upload; releases are never overwritten (script docstring). `init` checks installed command hashes against the release manifest ([install guide](metropolis-install.md)). |

## 4. Threat model

The full draft is [docs/metropolis-threat-model.md](metropolis-threat-model.md). Its status line
says "proposed requirements", so its tables are targets, not verified guarantees. For this review:

1. **Chain reader.** Sees contracts, gas sender, commitments, nullifiers, roots, timing, expiry,
   fees, and public deposits and withdrawals. Should not learn negotiation, private amount or
   recipient on the shielded rail. Public-bound and x402 payments expose terms and parties by
   design.
2. **Malicious counterparty.** Can bypass the SDK and call contracts directly. Can withhold
   delivery after payment; atomicity covers settlement state only (threat model §6). Should not
   be able to substitute terms, redirect or reduce payment, replay a deal, or broaden disclosure
   (threat model §3 attack table).
3. **Malicious or lagging RPC.** One provider is checked against the other at a finalized block.
   Two colluding providers can return a false frontier or omit the same spend-only event without
   changing a root (checkpoint design §2, threat model §2). Lag shows as `pending` or
   `PROVIDER_UNAVAILABLE`, not as a wrong result (quickstart Troubleshooting); please test that.
4. **Compromised MCP client or agent.** An agent can be prompt-injected into calling any tool it
   holds. Requests name a config file and an operation reference; keys, pins, fees and endpoints
   come from the config (operations §2). Wallet tools are policy-capped and `fund_wallet` exists
   only when policy grants wrap, approve and deposit (roadmap D4). Only allowlisted native error
   messages reach the agent (quickstart; [friction](friction.md) F64, F68). Tool results can still
   expose amounts and commitments to the agent's model provider (threat model §2).
5. **Local attacker on the same machine.** Owner-only permissions do not protect against the
   owning account, root, copied backups or a compromised host; journals and transcripts are not
   encrypted at rest (threat model §2).
6. **Release supply chain.** The install pins an exact version with the Metropolis index first and
   PyPI only for dependencies ([README](../README.md)). Trust rests on the publisher key, the GitHub
   account and Pages index, GitHub-hosted runners, and pinned third-party dependencies (Circom,
   circomlib, snarkjs, arkworks, ark-circom; README "How this was built"). The Groth16 setup was
   single-operator with unrecorded worker entropy (operations §3).

## 5. Known limits and accepted risks

From the docs as of `cbd6567`. "On main" marks WS5 work merged after release `0.4.0.dev2026101801` and not yet published; "in progress" marks work not on main.

| Limit | Source | Status |
|---|---|---|
| Test-only, single-operator, unaudited deployment; Groth16 test setup | deployment record, [JUDGES.md](../JUDGES.md), operations §3 | accepted for testnet |
| Public-bound and x402 payments expose accepted terms and parties | README, threat model §2 | by design |
| Gas sender, timing, deposits and withdrawals are public; small anonymity set | README, roadmap "Privacy and Asset Boundary" | accepted |
| Private transfers are self-submitted; no stealth addresses or shielded-relayer guarantee | roadmap "Privacy and Asset Boundary" | accepted; live relayer loop still being completed (JUDGES.md) |
| History cache was one file capped at 128 MiB (about two days of continuous sync) | WS1 design §4 | segmented append-only history **on main** (`5428c55`); about 40 bytes per block, no cap; a reorg deeper than about 1,000 blocks into saved history is refused |
| Auditor payment verification replayed from the deployment block | operations §6 | auditor checkpoint **on main** (`6252273`); deals settled at or before the checkpoint report `settled_before_checkpoint`; see the trust statement in operations §6 |
| USDC has issuer/admin powers | roadmap | USDC quickstart end to end **on main** (`0ea1c80`), verified live with two deals |
| One seller serves the one buyer it was connected to | quickstart "Known limits", roadmap D3 | many-buyer sellers **in progress** (branch `feat/ws5-many-buyers`; review focus: transport v2 routed handshake, uniform access refusal) |
| Paired providers are a trust assumption, not a state proof | checkpoint design §2 | accepted |
| Delivery is not guaranteed by settlement; no escrow or fair exchange | threat model §6 | accepted |
| Signed permission stays usable until expiry or settlement; no onchain cancellation | threat model §4 | accepted |
| Descriptors and identity bindings last 30 days | quickstart §4 | accepted; `renew` exists |
| Monad charges the full gas limit, about 0.31 MON per transaction at 3,000,000 gas | friction F69 | not changed yet |
| Public RPCs throttle (ankr HTTP 429) | quickstart, friction F67 | mitigated by backoff and pacing |

## 6. Suggested focus areas

1. **Circuits.** Constraint completeness in `transfer.circom`, especially the change tag
   (`changeSpendTag == Poseidon(2006, changeSpendSecret)`), note-domain binding
   (`Poseidon(chainId, contractAddress, verifierVersion)`), range and conservation constraints,
   and `eddsa_strict.circom` on-curve and subgroup checks (threat model §8).
2. **Pool contract.** Nullifier and deal consumption before external calls, `knownRoots` never
   evicted, zero-leaf refusal, ERC-20 balance-delta handling, and whether the verifier and domain
   checks match what the circuits bind.
3. **Rust and circuit agreement.** Every hash preimage and domain tag in `sdk/shielded` matches the
   circuits. Failures here are silent: a wrong preimage looks like a missing note.
4. **Checkpoint.** The frontier-recompute argument and the `nextLeafIndex` count binding in the
   checkpoint design; what a checkpointed wallet can and cannot prove about earlier spends.
5. **Signer journal and recovery.** That no path rebroadcasts a completed operation or sends a
   second payment after a lost response; replacement changes only fee caps (threat model §8).
6. **MCP surface.** What a fully hostile agent can do with each tool under a given policy; whether
   any tool accepts a path, target, calldata or amount outside the operator config.
7. **Error and output redaction.** That no envelope, log or `Debug` output carries keys, witnesses,
   openings or keyed RPC URLs (threat model §2, friction F64).
8. **Release path.** What an attacker holding one of the publisher key, the GitHub account, or the
   Pages index can make a user install.

## 7. Questions for the auditor

- Can a valid transfer proof be produced whose public inputs settle a different amount, asset or
  recipient than the authorized agreement?
- Is the identity-key rejection complete across Rust verification, commitment construction and the
  circuit?
- Is there any state of two agreeing providers short of collusion that makes a checkpoint accept a
  wrong frontier?
- Can another local user (not the owner and not root) influence a state root, journal or artifact
  cache, for example through a parent directory or a path replaced between check and open?
- The record sets `artifact_domain_requires_rebinding: true`. Only the deployment scripts
  (`prepare-testnet-deployment.mjs`, `check-testnet-deployment.mjs`) write it, and no Rust or
  Python code reads it. We have not documented what it is meant to gate. Does anything depend on
  it, and should something?
- The record's `"live_transactions_sent": 0` is a constant written by
  `check-testnet-deployment.mjs`, the read-only checker, not a count of deployment transactions
  (the record lists six creations). Is that field misleading enough to rename?
