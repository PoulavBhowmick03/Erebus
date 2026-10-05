# Metropolis Status

Status date: 2026-10-05. Branch: `metropolis`.

This document consolidates the historical Metropolis milestone baselines, progress notes, decision
records, the x402 record, the demo plan, and the submission gates into one status source.
[metropolis-roadmap.md](metropolis-roadmap.md) owns milestone scope and sequencing;
[metropolis-agreement.md](metropolis-agreement.md) stays normative for agreement encoding;
[friction.md](friction.md) stays the separate friction log.

## Current status

M8 is **not complete**. `0.3.0.dev4` is published on the separate Metropolis index and was verified
from a fresh install on macOS arm64, and several team-operated live Monad testnet payments are
recorded, including the x402 `exact` Permit2 rail and a full public-bound workflow (2026-10-05),
each with seller restart, resource recovery, and independent auditor `payment_verified: true`. The
live public-bound run used
**unreleased source fixes** for concurrent observation, moving-chain catch-up, and agreement-lifetime
validation that are not part of the published `0.3.0.dev4` artifacts. Shielded settlement has
**not run on Monad**: suite-2 settlement, scoped disclosure, and the two-agent MCP delivery loop
have local Anvil evidence only, with known-entropy test keys and test-only proving artifacts. The
public-bound and x402 rails expose terms and payment metadata on chain (details below). External
third-party acceptance, hosted operation, and the recorded demo video remain open, so M8 and the
submission gates are not closed.

## Verified evidence

### Monad testnet deployments and live transactions

Chain ID 10143. Contract identity was verified before the address was trusted.

| Item | Value | Record |
|---|---|---|
| `ErebusSettlement` deployment | `0xa5f0c864f434331bef9a7fc5e05450d598d24da4`, verifier version 1, block 67495473 | Deploy transaction `0x6c7b810c837c038753815df985c3b9ce210be3b40df6fab4ec0682e1a271ca65`; manifest `contracts/evm/deployments/monad-testnet.json` |
| Deployment identity check | finalized block `0x1b01c4fac8ce92fa33496b80752a0bdfbed65eb2aeaec6c21aac878b551e2aa6`; runtime read by block hash; four immutable reference ranges masked; 15,652 bytes matched; `verifierVersion()` = 1 | `scripts/check-evm-deployment.py` |
| Deployment recheck | finalized block hash `0x4ba79809c01f237e9ec11f80aae75b9b7ce6f642ed790ff21ac77db9d2c8f4bf`; both runtimes 15,652 bytes; masked bytes matched; `verifierVersion()` = 1 | Read-only artifact identity evidence, not an audit |
| Test token | `0x902f79145059910ef875aecf4187c771b204ea14` | Minted to the buyer and approved for the runs below |
| Live rehearsal runtime pin | identical finalized `ErebusSettlement` runtime at both providers, keccak `0x5fa7befc14141996c531dd21a0c7f6c06cd5767a9e08334a1f61e0dfafadf94c` | Rehearsal `init`/`preflight`, 2026-10-04 |
| Network-check finalized anchor | finalized block 67489534 (`0x0c4bf7b2…`), head 67489537 | `erebus-network-check`, 16/16 checks on `testnet-rpc.monad.xyz` |
| Canonical Permit2 | `0x000000000022d473030f116ddee9f6b43ac78ba3` | x402 `exact` rail |
| Canonical `x402ExactPermit2Proxy` | `0x402085c248EeA27D92E8b30b2C58ed07f9E20001` | x402 `exact` rail |
| Canonical `x402UptoPermit2Proxy` | `0x4020A4f3b7b90ccA423B9fabCc0CE57C6C240002` | experimental `upto` model only |
| Multicall3 | `0xcA11bde05977b3631167028862bE2a173976CA11` | Monad testnet canonical |

| Live transaction | Commitment | Nullifier | Recorded outcome |
|---|---|---|---|
| Public-bound settlement `0x1f7ec208a7b03b835f224ac989cc59c9aee4c33634a2b04498b8394ddede4c09` | `4511793857dd5a2f50e5f3910b034a652127bfababaae9abedeae68c02be1abd` | `3cfeea17e120c471e435534a46036fa6542dbf19b9ef50dfd598628b1e757360` | `PaidFinalized`, `payment_finalized: true`; seller 3,000,000 base units after three settlements, buyer 0 |
| Public-bound independent-process run `0x6169b70f2efc1aa83b75f92b7b05074414cf24dcc83c3509133c707a0fdd1fda` (2026-10-02) | `ca7d4cb1c3752b48812d5e8bb6804f636d5e84f096cc573fad1da8960f03f3a1` | `e96d5a7c337202528a422d83fd5cc86f2a1408441cfb3ae58cf0bd0ab0bc091c` | Auditor `agreement_verified: true`, `payment_verified: true`, `delivery_verified: false`; a deterministic test seller key in that example was later removed from the independent path |
| x402 `exact` live payment `0x8a0272e69188ac3be52783ddfc4d4436496a58f2d5c63d808cd3320a1fa5df43` (2026-10-04) | `fa4628c2c2e57b659f2d36b86babf53ec8689e0d49a1708953e993dd1902a769` | — | Access first `pending`, one retry after seller restart, resource hash verified; auditor `agreement_verified: true`, `payment_verified: true`, `delivery_verified: false`; stages: negotiation 352 ms, first request to resource 12,915 ms. Re-verified 2026-10-05 from a refreshed encrypted grant with participant paths withheld: `payment_verified: true`, commitment unchanged |
| Public-bound full workflow `0x1ba9ddb363a0c916f8e41ca8eca7d4c524d1ba4722bf4ea37ed423547700ff73` (2026-10-05) | `477f9ad5c37010c8379721f2573d8d7c51158805cf465ebe72ac1cfc2a698f33` | `a6d31c388b4c13f8fdedf3ec81ac91e1a300a05bc576c56f134a4a8aadfaeeaa` | `consumedDeals == true`; seller 70, buyer 5; receipt status `0x1`; exactly one broadcast attempt, outcome `Submitted`; 38 access retries after a seller restart, resource hash verified; auditor `payment_verified: true` from the grant and public chain only; stages: negotiation 144 ms, settle + catch-up 976 s, settle-to-finality 1,044 s, first request to resource 876 s |

The earlier public-bound live attempt was stopped before broadcast on a transient paired-auth error;
its agreement expired (`2026-10-04T23:40:07Z`), the coordinator held no broadcast attempt, and
finalized reads showed the deal unconsumed. A fresh replacement reused the same funded participants.
Earlier retained operations must not be reused. All live runs above are **team-operated**, not
independent external acceptance.

### Published packages and build artifacts

| Artifact | Identity | Status |
|---|---|---|
| `0.3.0.dev4` | Built from clean `e9f10d0` by registry run 37186885901; both platforms and the isolated installed x402 rehearsals passed; four deduplicated wheels plus combined `release-manifest.json` | Published as a GitHub prerelease on `PoulavBhowmick03/erebus-metropolis`; HTTPS index `https://poulavbhowmick03.github.io/erebus-metropolis/simple/`; index links each wheel by SHA-256 |
| `0.3.0.dev1` | `f4714bb`, run 37180669150 | Not publishable: macOS wheel tagged `py3-none-macosx_10_9_universal2` while all 13 native binaries were thin arm64 with `minos 11.0` |
| `0.3.0.dev2` | `cbfc2e5`, run 37182102672 | Both platforms and the comparison passed, but no wheel shipped a license; not published. `171be31` added Apache-2.0 `LICENSE` to every wheel and `THIRD_PARTY_NOTICES` for all 706 linked crates |
| Local staged wheels | `0.3.0.dev20261003`, `0.3.0.dev2026100301`, `0.3.0.dev2026100302` under `artifacts/metropolis-registry-local`, `artifacts/metropolis-registry-local-2`, `artifacts/metropolis-review-fd111a7`, `artifacts/metropolis-review-fixes` | Local unpublished builds; not release evidence |
| Local release builds | `0.3.0.dev20261004` (`source_commit 8a1d365`, `dirty_source: true`); `0.3.0.dev2026100401`; `0.3.0.dev2026100402` (`f4714bba42b6909b860ba04d301ab23e4c3c3899`, `dirty_source: false`) | Isolated install checks verified 14 native binaries plus the self-host launcher, imports outside the checkout, relay health, and installed x402 recovery/auditor rehearsals |
| Wheel contents | Apache-2.0 `LICENSE` in every wheel; `erebus-cli` wheel also ships `THIRD_PARTY_NOTICES` for 706 crates; `erebus-selfhost` launcher; 14 native binaries | Published dev4 |

Qualification commits: `5a76bbd` (host OS tag and `MACOSX_DEPLOYMENT_TARGET=11.0`), `171be31`
(license and notices), `cbfc2e5` (build EVM contracts in the M5 job), `44c754a` (bound shielded
access requests), `2708983` (index every published release). The M5 CI job had failed since at
least `191dbf5` because it never ran `forge build`; run 37182102674 passed `access_http`,
`x402_http`, the x402 audit test, and the MCP agent tests for the first time. Linux x86_64
qualification is CI-only and was not verified locally (the emulated colima VM reset under load).

### Historical pins and local artifact identities

| Pin | Value |
|---|---|
| M0 source HEAD | `df86407ba72b542f5d3a851d14c1d0ce8515d1e5`; protected local main `daf33efc464294a2516700cb066a98214e921a39`; `rustc 1.96.0 (ac68faa20 2026-05-25)`, `cargo 1.96.0 (30a34c682 2026-05-25)`; `erebus-sdk 0.3.0`, edition 2021 |
| M0/M1 lockfiles | `sdk/rs` Cargo.lock SHA-256 `ef686af9742ace528186e84c0b374d276c47788fae5876fd452ba2e1e98933c0` (M0) and `e50a25cfca13b303a3c428a35d9c99f4cf5350723de5a77fe52625f68b20f539` (M1); `sdk/core` Cargo.lock SHA-256 `fb6cced151dbeff63216645ac3c61c8b13a0c176918aa69924cf7fbae4ac7f67`; `erebus-core 0.1.0`; M1 source HEAD `05e6135` |
| M0 upstream reference | `starknet-privacy` checkout `3dfe66fe2b59d7b95709ec719547fa88b8ef63f9`, file `packages/privacy/src/privacy.cairo` (`use_note`, `apply_actions`) |
| M4 fixtures and outputs | `circuits/m4/fixtures/public.json` (eleven pinned public field elements); generated `build/metrics.json` and `build/artifact-manifest.json`; manifest name `m4-prototype-v1`; `contracts/evm` sources `ErebusCodec.sol`, `ErebusSettlement.sol`, `MockERC20.sol`, tests `ErebusVectors.t.sol`, `ErebusSettlement.t.sol` |
| M5 fixtures and outputs | `sdk/core/tests/fixtures/m5-note-vector.json`; funded report `circuits/m5/build/run.json`; `sdk/core/tests/fixtures/agreement-v1-vectors.json`; depth-20 tree, 128 MiB shielded cache limit |
| M8 deployment tooling | `contracts/evm/deployments/monad-testnet.json`; `contracts/evm/deployments.example.json`; `scripts/check-evm-deployment.py`; `sdk/evm/src/readiness.rs` (`erebus-network-check`) |

### Test and runner evidence by milestone

Every named test recorded in the historical records is listed here; commands are in
[Reproducing evidence](#reproducing-evidence).

| Milestone | Named evidence |
|---|---|
| M0 | `scripts/tests/test_release_readiness.py` (12 tests); workflow-trigger and publication-guard assertions across `ci.yml`, `wheels.yml`, `canary.yml`, `published-canary.yml`; settlement enforcement map against upstream `3dfe66fe2b59d7b95709ec719547fa88b8ef63f9` |
| M1 | `original_commitment_cannot_authorize_changed_terms`; ignored vector regenerator `regenerate_vectors`; `scripts/check_agreement_vectors.py`; suite-1 external keccak256/EIP-155 vectors |
| M2 | `two_participants_agree_on_one_transcript`; `a_restart_rehandshakes_and_the_transcript_continues`; `independent_processes_rehandshake_reopen_state_and_agree_on_the_root`; `packaged_http_relay_enforces_auth_and_serves_ciphertext`; `a_replayed_ciphertext_is_rejected`; `a_message_from_another_session_is_rejected`; `a_mismatched_capability_prologue_fails_the_handshake`; `a_descriptor_for_a_different_deployment_does_not_support_the_filter`; transcript/session/message/descriptor/relay/store unit tests |
| M3 | `one_authorized_agreement_settles_exactly_once`; `a_replay_is_rejected_on_chain`; `the_contract_rejects_mutated_terms_and_expiry`; `prepare_rejects_terms_that_do_not_match_their_authorizations`; `an_unrelated_successful_receipt_is_not_settlement_evidence`; `the_rpc_chain_must_match_the_configured_chain`; `prepared_routing_fields_must_match_the_evidence`; Solidity KAT against `sdk/core/tests/fixtures/agreement-v1-vectors.json` plus the contract adversarial suite |
| M4 | `circuits/m4` runner: combined agreement+transfer circuit, offchain and Solidity verifier checks against `circuits/m4/fixtures/public.json`, negative mutations (amount, seller key, accepted root, buyer signature, change, expiry, zero blinding/spend secret, zero recipient tag, changed public payment commitment, replay) |
| M5 | `circuits/m5` funded runner: a 150 → 70 payment + 80 change → 70 withdrawal flow, replay and mutation rejection, fee-on-transfer rejection, 256 fuzzed amounts, reorg/restart/recovery; transcript-root KAT `cd876d46…`; Rust/JS witness field-for-field matches; `sdk/core/tests/fixtures/m5-note-vector.json`; report `circuits/m5/build/run.json` |
| M6 | `coordinated_settlement_finalizes_commits_and_releases_the_signer`; `a_foreign_winner_resolves_an_unknown_attempt_and_holds_the_unconsumed_nonce`; `crashes_at_durable_boundaries_yield_one_payment_and_recoverable_state`; `relayed_settlement_pays_the_published_fee_recipient`; `the_relayer_fails_over_providers_and_reports_funding_shortfall`; `a_dropped_broadcast_response_stays_unknown_and_reconciles_from_chain`; `a_falsified_or_dropped_rpc_response_is_never_payment_evidence`; `invalid_relayer_signer_or_deadline_preserves_durable_state_without_sending`; `internally_consistent_rpc_histories_can_disagree_about_one_deal`; `rejected_signing_preserves_every_file_across_historical_schemas`; `every_public_bound_operation_write_recovers_one_actual_payment`; coordinator `every_discovered` lifecycle sweep (both settlement modes) |
| M7 | `issuer_selects_evidence_from_durable_state_and_a_fresh_recipient_verifies`; disclosure suite (wrong recipient, wrong issuer, expiry, changed ciphertext, missing/altered transcript, neighboring deal, changed terms, backup recovery); funded native-proof Anvil flow with separate shielded CLI and MCP auditors over 75 funded crash-point trials |
| M8 | `two_mcp_agents_negotiate_and_settle_once_through_a_lost_broadcast`; `two_mcp_agents_negotiate_pay_and_retrieve_public_bound_with_one_send`; `two_mcp_agents_pay_over_x402_through_a_dropped_response_and_restarts`; `funded_http_access_recovers_a_lost_response_after_service_restart_without_a_second_payment`; `x402_http_submits_once_recovers_restart_and_rejects_mutations`; `negotiated_x402_settles_once_recovers_restart_and_audits_from_the_grant`; `auditor_verifies_finalized_x402_payment_from_the_grant_alone`; fixtures `hold_public_agent_environment`, `sdk/evm/tests/negotiation_flow.rs`, `sdk/shielded/tests/negotiation_cli.rs`, `sdk/shielded/tests/support/payment_driver.rs`, `sdk/shielded/tests/support/shielded_driver.rs`, `sdk/evm/tests/x402_vectors.rs`, `sdk/evm/tests/x402_permit2_chain.rs`, `sdk/evm/tests/x402_service_models.rs`, `sdk/shielded/tests/x402_http.rs`, `sdk/evm/tests/network_check.rs` |

Selected recorded counts (local unless stated): M1 `sdk/core` 76 passed/1 ignored and `sdk/rs` 368
passed/7 ignored; M2 56 unit + 8 integration; M3 25 Solidity + 9 unit + 7 integration (all
`--include-ignored`); M6 release-mode local-chain runs of 33 then 36 tests, 27 unit + 12 offline +
7 CLI + 3 non-Anvil in the ordinary run, funded sweeps of 56 public-bound and 68 shielded
write-failure trials (later 75 funded crash points in the M7 combined run), 24 journal, 19
coordinator lifecycle; M7 13 disclosure CLI tests, 57 transport unit + 8–11 disclosure + 8 Eleusis,
219 Python tests/2 skipped, public-bound Anvil 36, coordinator 25 lifecycle; M8 Python workspace 394
passed/4 skipped with three unresolved `scripts/tests/test_demo.py` failures over stale legacy web
component paths and video expectations.

### Measurements (local, not Monad latency)

| Measurement | Values | Boundary |
|---|---|---|
| M4 prototype (Apple M4 Pro, macOS arm64, Node 22.18.0) | 31,818 constraints; 11 public inputs; depth-four tree; witness+proof ≈0.7 s; peak Node RSS ≈1 GiB; 256-byte Groth16 proof; standalone verifier ≈285k gas; prototype settlement ≈403k gas on Anvil 1.5.1 | Proof-only transition, no token custody, deposit, tree update, or withdrawal; `m4-prototype-v1` local setup, not portable, not safe for funds |
| x402 service models (Anvil, chain 31337, `--block-time 1`, canonical runtimes pinned from Monad testnet, 20 requests at price 10, debug build) | Per-request `exact`: 20 settlements, 1,819,836 gas (90,991 per request), 5,000,000 charged at a 250,000 gas limit, p50 1,007 ms. Prepaid: 1 settlement, 89,146 gas, p50 8 ms, 1,006 ms one-time. Batched `upto`: 1 settlement of 17 used units, 110,101 gas, p50 3 ms, 756 ms one-time | Prepaid and batched are experiments in `sdk/evm/tests/x402_service_models.rs` only; `exact` is the product rail. Monad charges the gas limit, so the limit column is the cost model. This does not measure Monad inclusion/finality latency, gas price in currency, or concurrent-buyer contention |
| Negotiated x402 local run (debug) | negotiation 759 ms; permit preparation 17 ms; first observed inclusion 81 ms after submission; finalized verification 290 ms after submission; delivery 742 ms; proof null | Anvil; local monotonic clocks |
| Installed x402 checks | dev2026100401: negotiation 468 ms, permit prep + intentional failed HTTP 25 ms, inclusion 47 ms, finalized 139 ms, delivery 319 ms. dev2026100402: 475/25/51/144/304 ms | Anvil; the permit number is not signing-only latency |
| Public-bound agent loop (debug) | full deal 11.6 s: negotiation 0.3 s, local signing 0.09 s, submission 0.04 s, payment verified 2.3 s after submission, delivery 0.1 s | Anvil; inclusion/finality block timestamps are diagnostics and were removed from harness output |
| Shielded MCP harness (debug) | negotiation 6.7 s buyer / 4.5 s seller; artifact download 3.5 s for 32 MB; proof preparation 52 s; local signing 6.7 s; submission 4.2 s | Anvil, prototype keys and artifacts |
| Live public-bound (2026-10-05) | negotiation 144 ms; settle + catch-up 976 s; settle-to-finality 1,044 s; first request to resource 876 s | Local monotonic stage timings on live Monad; not a latency guarantee |
| Live log-query concurrency | 48 `eth_getLogs` queries: 51.7 s sequential vs 6.4 s at concurrency 16 (8.0x); extrapolated ≈21 minutes of log-query time for the live range | Live Monad public RPC; excludes authentication, ancestry catch-up, retries, and verification |

## Milestone outcomes M0-M8

| Milestone | Outcome | Status |
|---|---|---|
| M0 | Baseline, enforcement map, workflow isolation, and decisions D01-D08 | Complete locally; no live chain or remote CI claimed |
| M1 | Canonical agreement core (`sdk/core`), suite 1, normative spec §13, DM1 decisions | Complete locally |
| M2 | Offchain transport (`sdk/transport`): Noise XX, typed transcript, relay, descriptors | Complete locally; ordered transport only, not business negotiation |
| M3 | `contracts/evm` + `sdk/evm`: public-bound settlement, exactly-once replay state | Complete locally; public payment |
| M4 | Feasibility memo and Groth16/Circom agreement-bound transfer prototype | Complete locally as research/prototype; no production prover |
| M5 | Funded local shielded prototype: pool, depth-20 tree, wallet, indexer, native proof | **Not complete**; roadmap M5 criterion unchecked |
| M6 | `sdk/journal`, `sdk/coordinator`, relayer, observation, recovery, fault sweeps | Implementation locally complete; owner review of open DM6 items |
| M7 | Scoped disclosure: suite-1 and suite-2 grants, offline auditor, backup recovery | Complete at its local gate; Monad and packages were M8 |
| M8 | Monad deployment, packages, live public-bound and x402 runs, local product loop | **Not complete**; external acceptance, hosted services, and demo video open |

**M0 (2026-09-20).** Repository and offline Rust baseline: formatting, tests, Clippy, docs, and
`erebus-cli` build passed; seven live tests remained ignored. The settlement enforcement map
separated local idempotency from contract replay protection, and workflow guards established that
branch pushes cannot trigger packaging or publication. D01-D08 set product, agreement, custody,
compatibility, service, external-acceptance, post-M0 gate, and public-first x402 boundaries. No
commit, push, tag, deployment, or remote CI run was performed.

**M1 (2026-09-20).** A new chain-neutral `sdk/core` crate implemented canonical encoding, ids,
terms, suite registry, commitment, deal identity, authorization, policy, and settlement/receipt
types. Suite 1 is keccak256 commitment plus secp256k1 ECDSA; the shielded suite was deferred.
Review corrections made authorization check the commitment opening, counted fees inside limits,
rejected shielded mode for suite 1, and wired the real STRK20 settlement method through a checked
legacy adapter. The normative record is [metropolis-agreement.md](metropolis-agreement.md) §13
(DM1-1..DM1-6).

**M2 (2026-09-21).** The offchain Eleusis shipped as `sdk/transport`: X25519 identities, Noise XX
sessions with descriptor-bound prologues, canonical envelopes, per-author transcript chains with a
transcript root, a checksummed file store, a ciphertext relay, and signed service descriptors.
Sessions never resume after restart, which removes nonce-reuse risk; the transcript is the durable
object. Metadata (endpoints, mailbox ids, sizes, timing) is explicitly not hidden. M8 later added
typed price-only negotiation (DM8-6) and shielded-key identity binding (DM8-7) without changing the
M1 encoding or M2 hashes.

**M3 (2026-09-22).** `contracts/evm` and `sdk/evm` turned one authorized canonical agreement into
exactly one public payment. The contract re-derives the commitment and nullifier from the terms
opening and both signatures; exact balance deltas reject fee-on-transfer and rebasing tokens; there
is no owner, upgrade, or pause. Asset identifiers are restricted to lowercase
`eip155:<chainId>/erc20:0x<40 hex>`. Public: full accepted agreement, blinding, final signatures,
amount, asset, parties, fee, timing. M6 later removed the unjournaled `submit`/`settle`/`verify`/
`with_confirmations` APIs in favor of coordinated submission and paired finalized reconciliation
(DM6-4).

**M4 (2026-09-24).** The feasibility memo evaluated Monad facts, reusable privacy pools, proof
systems, in-circuit hash/signature options, and proving-key lifecycle; no candidate exposed a
verified hook for an external signed-agreement commitment. The selected prototype owns one combined
agreement-and-transfer relation: BN254 Groth16, Circom 2.2.3/circomlib 2.0.5, Poseidon domain tags
2001-2009, BabyJubJub EdDSA authorizations, reserved suite 2, and a depth-four tree. The prototype
proved and verified locally on Anvil but has no token custody; the setup is single-contributor and
the keys and artifacts are test-only.

**M5 (2026-09-24 to 2026-09-28).** A funded local prototype added the `ErebusShieldedPool` (one
ERC-20 asset, exact deltas, depth-20 Poseidon tree, consumed deal and note nullifiers), deposit/
transfer/withdrawal circuits, a Rust suite-2 mapping with pinned vectors, an encrypted note wallet
with reorg rewind, a verified public event index, and native local proof preparation. Rust and
JavaScript witnesses match field-for-field. The M5 gate remains open: installed/production
artifacts and ceremony, hosted indexer/wallet rehearsal, fee and gas validation, full Monad flow,
and the installed private workflow are unfinished; the npm toolchain had 20 advisories (5 high)
pending review.

**M6 (2026-09-28 to 2026-10-01).** M6 added the shared `sdk/journal`, the local `sdk/coordinator`
(durable intent, reservations, signer nonce claims, authorization fences, immutable signing plans),
EVM offline signing, bounded and paired observation, the public-bound relayer, a JSON-RPC fault
proxy, and crash/reorg/false-history sweeps in both settlement modes. Supported operator recovery
requires two matching RPC providers before accounting changes; this is a two-of-two consistency
policy, not Byzantine consensus. Open items are recorded below (DM6-1 owner sign-off, legacy
timestamp provenance, expiry caps and ledger scope, relayer quote/replacement handling). The
shielded path is self-submitted because the circuit fixes fee = 0, which links the buyer's
gas-paying address to the settlement (see [friction.md](friction.md)).

**M7 (2026-10-01 to 2026-10-02).** Scoped disclosure reached its local gate.
`verify_selected_agreement` replays only the selected deal and checks the transcript root,
commitment, nullifier, and both role authorizations. Grants are encrypted to a separate X25519
disclosure key: version 1 uses a suite-1 identity, version 2 signs directly with the participant
suite-2 key (domain `EREBUS_DEAL_DISCLOSURE_V2_SUITE2`, Poseidon tag 3001). The auditor verifies
payment through its own configured RPCs; a seller receipt or grant claim is never accepted as
payment evidence. Delivery remains explicitly unverified, and expiry is verifier policy, not
revocation. Monad deployment and package-only installation remained M8 work.

**M8 (2026-10-02 to 2026-10-05).** `ErebusSettlement` was deployed to Monad testnet and its identity
verified against the reviewed artifact. Native negotiation, payment, access, disclosure, local
proving, MCP, and self-hosting components were built and tested on Anvil, then packaged in a
separate registry; `0.3.0.dev4` is published and fresh-install verified. Live team-operated runs
completed the x402 `exact` rail and the full public-bound workflow, both with restart recovery and
independent auditor payment verification. M8 is still not complete: no independent external
acceptance, no hosted services (Render files are prepared and tested only; paid provisioning is not
authorized), no demo video, no live shielded Monad settlement, and the concurrent-observation,
moving-chain catch-up, and lifetime-validation fixes are uncommitted source, not in `0.3.0.dev4`.

## Decisions appendix

### M0 decisions D01-D08

| ID | Decision | Outcome |
|---|---|---|
| D01 | Product, modes, and custody | Monad testnet developer release with SDK, CLI, MCP, and self-hostable services. Public-bound and shielded stay distinct; a signed privacy requirement is never downgraded. Clients hold keys and authorize locally; the shielded EVM backend proves locally. Mainnet and real value remain separate release decisions. |
| D02 | Agreement and replay identity | Bilateral authorization of a blinded commitment to canonical versioned terms; one deal-wide settlement identity with a fresh 256-bit nonce per revision. First valid signed revision to settle consumes the deal; later counters do not revoke earlier signed permission. Cancellation is local only; cross-chain uniqueness is not promised. |
| D03 | Authorization roles and policy | Separate transport, agreement, note-spending, and submission keys; bind buyer and seller roles; persist the accepted commitment before submission. Policy limits live below the LLM; the signature suite and commitment hash were M1/M4 gates, and generic cross-key signature verification was rejected. |
| D04 | Compatibility and state | Keep Starknet codecs, records, and CLI behavior under regression; add versioned EVM records instead of rewriting history. Namespace state by backend, chain, deployment, and local identity; legacy STRK20 agreement enforcement is client-side, and a wrapper does not upgrade proof guarantees. |
| D05 | Hosted and self-hosted services | Native clients on Linux x86-64 and macOS arm64; Linux containers for services with persistent volumes and health endpoints. Relay, transaction relayer, and indexer have documented trust and availability boundaries; relay retention defaults to seven days; logs stay free of payloads, keys, and openings. Resource and rate limits were M2/M6 gates before hosting. |
| D06 | External acceptance scenario | A ten-step synthetic, published-instructions-only external run: install, discover, enforce policy, fund, negotiate, shielded Monad payment, recovery without a second payment, access recovery, one-deal disclosure, backup restore. Any private assistance counts as a failed acceptance step; no external participant means the gate stays open. |
| D07 | Decisions gated after M0 | M1 agreement suite, M2 transport, M3 EVM contracts, M4 proof/pool, and M8 shared-service launch each require a decision record plus evidence before dependents proceed. They are not permission to weaken guarantees. |
| D08 | Public-first execution and x402 shape | Complete the public-bound Monad flow before shielded research blocks agent integration, while stating that amount, recipient, asset, signers, and timing stay public. Private x402 is a use-case hypothesis; compare per-request, prepaid, and batched models before naming anything x402-compatible. |

### M1 decisions DM1-1..DM1-6

Normative record: [metropolis-agreement.md](metropolis-agreement.md) §13.

| ID | Decision | Outcome |
|---|---|---|
| DM1-1 | Canonical encoding | One self-delimiting form with strict decode, mirroring the existing `RequestBinding` discipline. Implemented and independently reconstructed by `scripts/check_agreement_vectors.py`. |
| DM1-2 | Suite registry | Suite 1 = keccak256 + secp256k1 ECDSA; the shielded suite was deferred and later reserved as suite 2 for the M4/M5 prototype. Only the experimental `sdk/shielded` backend advertises suite 2. |
| DM1-3 | Shared core crate | `sdk/core` is compile-time isolated from Starknet dependencies; `cargo tree` confirms no Starknet/Felt dependency. |
| DM1-4 | Expiry semantics | Reject at `now >= expiry`; cancellation is local only and restated before signing. |
| DM1-5 | Policy windows | `(now - window, now]`, inclusive limits, reservations held until committed or released; uncertain payments never free capacity early. |
| DM1-6 | Receipt states | Payment and delivery are separate states; unknown finality maps to reconciliation, never to a second payment. |

### M2 decisions DM2-1..DM2-9

| ID | Decision | Outcome |
|---|---|---|
| DM2-1 | Session protocol | Noise XX (`Noise_XX_25519_ChaChaPoly_BLAKE2s`, `snow`). Ephemeral sessions bounded by 4096 messages / 16 MiB plaintext; never resumed after restart; re-handshake creates fresh mailboxes. HPKE wrappers and hand-rolled crypto were rejected. |
| DM2-2 | Transport identity and peer authentication | Two layers: Noise static key on the wire plus a suite-1-signed `ServiceDescriptor` mapping to the agreement identity. M8/DM8-7 added an encrypted descriptor-signed binding for separate suite-2 agreement keys. |
| DM2-3 | Message envelope | Canonical envelope with `protocol_version`, delivering `session_id`, 16-byte `deal_id`, revision, author, contiguous sequence, parent hash, type, and a ≤8192-byte body. `Debug` redacts bodies, session ids, and key material. |
| DM2-4 | Transcript and root | Per-author hash chains under domain tags `EREBUS_MESSAGE_V1`, `EREBUS_TRANSCRIPT_LINK_V1`, `EREBUS_TRANSCRIPT_ROOT_V1`; the root is order-independent across directions. Amended in M5: transport uses transport-versioned keccak256 (`TRANSCRIPT_HASH_VERSION = 1`) regardless of agreement suite, so suite-1 roots are byte-identical. |
| DM2-5 | Relay and durable storage | Minimal authenticated ciphertext relay; mailbox ids derived from session id and role; idempotent `put`; 64 KiB blob, 256 blob, and 7-day default bounds; checksummed records with interrupted-tail recovery and owner-only permissions. The transcript persists before acknowledgment. |
| DM2-6 | Service publication and discovery | Signed portable descriptors with no transcript, reservation price, or deal data; configured directory endpoint; verify signature, expiry, and capabilities; suite 1 with shielded mode or cross-chain assets is rejected. M8 later bounded this to a price-only profile for typed negotiation. |
| DM2-7 | Limits | Fixed wire bounds (8192 body, 65536 envelope, 4096 messages/deal, 4096 messages/session, 16 MiB/session, 64 KiB blob, 256 blobs, 7-day retention, 4 endpoints, 16 assets). Nothing is silently truncated. |
| DM2-8 | Module boundary | Chain-neutral `sdk/transport`, no Starknet/EVM/proving dependency. M2 added no CLI or MCP wiring; M8 built the native negotiation command on it. |
| DM2-9 | Metadata exposure | Plaintext is hidden; endpoints, mailbox ids, sizes, timing, session/message counts, availability, and the fact of communication are not. An observer correlating endpoints and timing can still reconstruct a relationship graph. |

### M3 decisions DM3-A..DM3-H

| ID | Decision | Outcome |
|---|---|---|
| DM3-A | Contract re-derives | The contract recomputes `Cdeal` and `Ndeal` from the terms opening and both signatures; the adapter is not trusted. |
| DM3-B | Exact balance delta | Fee-on-transfer, rebasing, and short-delivery tokens are rejected. |
| DM3-C | Signature format | 65-byte `r\|\|s\|\|v` with raw recovery id, low-`s`, non-zero `r`/`s`, and `ecrecover` to the role key. |
| DM3-D | No administration | No owner, upgrade path, pause, or privileged function; only the constructor chain id and verifier version; changes require a new deployment. |
| DM3-E | Asset form | Exactly `eip155:<chainId>/erc20:0x<40 lowercase hex>`; no aliasing or silent normalization. |
| DM3-F | Live chain check | Contract and adapter both verify the live chain id, preventing cross-chain replay of old domains. |
| DM3-G | Receipt verification | Exact destination, calldata, and matching `DealSettled` event are required; unrelated successful transactions are not settlement evidence. |
| DM3-H | Guarantee enforcement | Unsupported guarantees fail in both adapter selection and the contract; direct calls cannot bypass capability rules. |
| DM3-9 | Finality and receipt | **Superseded by M6 on 2026-10-01.** Confirmation counts are not finality; supported recovery now requires paired explicit `finalized` evidence (DM6-4, DM6-5). |

### M4 and M5 selections (no DM IDs)

M4 selected BN254 Groth16, Circom 2.2.3 / circomlib 2.0.5, Poseidon with numeric domain tags
2001-2009, BabyJubJub EdDSA role authorizations, reserved suite 2, depth four in the prototype, a
local Node/snarkjs prover, and a single-contributor setup discarded for public value. The circuit
shape fixes protocol version 1, suite 2, shielded mode, guarantees `0x7`, and zero fee. The
feasibility memo left vendor licensing (Railgun, Unlink, Kohaku, Tornado Nova), ceremony, and
Monad verifier gas as owner decisions. M5 selected the funded prototype direction (one supported
asset, depth-20 pool tree, encrypted local wallet, verified public cache) but published no
production artifacts; its gate remains open.

### M6 decisions DM6-1..DM6-12

| ID | Decision | Outcome |
|---|---|---|
| DM6-1 | What decides payment | Implemented conservative rule: a revision is unpaid only by `consumedDeals == false` at a final block past that revision's expiry; unknown consumption holds every reservation; the winner is found from the `DealSettled` log and verified calldata. **Owner decision and rationale are still blank.** |
| DM6-2 | Journal generalization | Decided: one shared `sdk/journal` generic over backend record types; Starknet layout unchanged. Legacy v1/v2 `accepted_at` timestamp provenance remains ambiguous; the owner chose compatibility on 2026-10-01, so a future explicit migration is still required. |
| DM6-3 | Mode order | Decided: public-bound completes the lifecycle and fault matrix first, then shielded on the same coordinator. |
| DM6-4 | EVM operation record and stages | Integrated: explicit nonce, local signing, persisted bytes, journaled broadcast, observation, reconciliation. On 2026-10-01 the owner removed the unjournaled backend submission APIs; supported submission uses the coordinator, and operator reconciliation requires paired finalized evidence. |
| DM6-5 | Finality and quorum | Implemented `finalized`-tag-only observation with canonical anchor rechecks; local tests proved self-consistent RPC histories can disagree. On 2026-10-01 the owner made two matching providers mandatory before accounting, wallet, or nonce changes; this is consistency, not consensus, and honest providers at different heights can hold recovery. |
| DM6-6 | Concurrency | Decided 2026-09-29: one unresolved operation per chain and gas-paying signer, shared across deployments; release only on a strictly greater finalized account nonce; different signers proceed independently. |
| DM6-7 | Policy ledger persistence | Decided: one reservation per signed revision, persisted with coordinator intent, committed for the winner, released only after the final winner or final expiry. **Open:** a cap on authorized expiry and whether the ledger scope includes Starknet `accept_and_settle`. |
| DM6-8 | Relayer | Decided: public-bound only (fee and recipient are signed terms; the shielded circuit fixes fee 0, so shielded is self-submitted). **Open:** how a buyer obtains a relayer quote and how a different relayer is chosen after the fee recipient is committed. |
| DM6-9 | Fault injection | Implemented durable-boundary fault hooks and a JSON-RPC proxy for dropped/falsified broadcasts, timeouts, and impossible anchors. Funded sweeps (56 public-bound, 68 shielded, later 75 crash points) pass locally; provider independence is not established. |
| DM6-10 | Scope exclusions | No Monad network evidence, shielded relayer compensation, disclosure package (M7), or consensus proof; only the two-provider consistency policy. |
| DM6-11 | Relayer recovery | Implemented `DurableRelayer` in relayer-owned directories; it holds no participant key, never signs an agreement, persists before sending, observes before retrying, and cannot clear a fence. The CLI is trusted-host newline-delimited JSON, not an authenticated remote multi-client gateway. |
| DM6-12 | Bounded historical observation | Implemented resumable public-bound observation with pinned anchors, bounded log/ancestry budgets, reorg suffix restart, and shielded bounded scans (1,000 blocks/call, 128 MiB cache). Pending is not evidence. Paired recovery can be held by provider lag. Cache compaction and fresh-install throughput remain limits. |

### M7 decisions DM7-1..DM7-4

| ID | Decision | Outcome |
|---|---|---|
| DM7-1 | Direct suite-2 grant signing | The participant's suite-2 seed signs the grant locally; either role can issue, no delegation, no separate disclosure signer, no circuit or hosted prover. The recipient uses a separate X25519 key. |
| DM7-2 | Versioned grant format | Version 1 keeps suite-1 keccak/secp256k1 and cannot authenticate suite 2; version 2 is exclusively suite-2 with version, issuer, recipient, deal id, expiry, Noise handshake, frame count, frames, and a 96-byte signature. Unknown versions fail; no fallback. |
| DM7-3 | Disclosure signature domain | Version-2 prologue hashes domain `EREBUS_DEAL_DISCLOSURE_V2_SUITE2` with issuer, recipient, deal id, and expiry; the signature is `Poseidon([3001, high128, low128])`. Tag 3001 is separate from payment tags 2004/2005; tests reject disclosure signatures as payment authorizations. |
| DM7-4 | Observation and metadata boundary | The public header exposes issuer key, recipient key, deal id, expiry, and ciphertext length; observers can link a reused issuer key. Only the recipient decrypts the transcript and opening. Payment is verified through the auditor's own configured deployment and paired RPCs; provider agreement does not prove provider independence; payment verification does not verify delivery. |

### M8 decisions DM8-1..DM8-15

| ID | Decision | Outcome |
|---|---|---|
| DM8-1 | Buyer identity for access | The agreement key is the access recipient (`access_recipient == buyer_authorization_key`); no delegated access keys. Requests bind domain, service, commitment, nullifier, resource digest, nonce, and expiry; ≤5-minute lifetime; replay during validity is possible; retrieval is idempotent. |
| DM8-2 | Payment and delivery separate | The seller verifies resource, buyer, quantity, and payload digest and requires finalized payment evidence before durable issuance; pending does not issue and does not prove nonpayment. `delivery_verified: false`; recovery needs seller cooperation, which cannot be forced. |
| DM8-3 | Shared artifacts, local witnesses | `erebus-artifacts` authenticates a trusted manifest digest, verifies lengths/hashes, locks, writes atomically, rejects symlinks, and repairs corrupt entries; the witness stays local and is never uploaded. Manifest authentication does not establish ceremony security or verifier code. |
| DM8-4 | Bounded disclosure observation | The public-bound disclosure CLI persists observation progress, accepts a deployment start block and bounded budgets, exits `2` for pending, and requires matching finalized evidence for exit `0`. Invalid grants fail before RPC. |
| DM8-5 | Buyer content verification | The buyer signs a short-lived retrieval request, verifies payload SHA-256, persists verified content, and can read it later from cache without a key or network. It reports `resource_verified: true` and `seller_reported_payment_finalized: true` but keeps `payment_verified: false` and `delivery_verified: false`. |
| DM8-6 | Freeze negotiation before authorizations | One frozen transcript; profile 1 carries draft terms with a zero root and private blinding; counters change only price/revision and reference the previous digest; both acceptances seal the root before signatures. Final authorizations stay outside the log. Recovery is idempotent and fails closed on corrupt freeze records. |
| DM8-7 | Bind discovery identity to shielded identity | A suite-1-signed binding (`EREBUS_AGREEMENT_KEY_BINDING_V1`) attests the descriptor digest, deployment domain, suite-2 key, and validity window inside Noise; no shielded key enters the public descriptor; bindings do not enter the frozen root and are not payment consent. |
| DM8-8 | Native payment uses durable consent | The operator fixes mode, deployment pins, keys, asset, gas account, fee caps, and the shared signer journal; the runtime pin must come from an independently verified immutable deployment, with code at the first block and no code at the preceding block; any recorded broadcast attempt disables automatic submission; later calls observe with two RPCs and separate checkpoints. A transaction fenced before broadcast can stall and needs operator recovery. |
| DM8-9 | Separate Metropolis registry | Metropolis packages publish to a separate index, not the stable Starknet channel; the builder stages `.devN` packages without editing source metadata and bundles native binaries in a host-tagged wheel; publication requires owner approval. |
| DM8-10 | Public-bound product first | Finish the whole public-bound loop (negotiation, settlement, delivery, packaging, self-hosting, x402) before deploying a shielded testnet pool; shielded driver evidence stays local until then. |
| DM8-11 | x402 `exact` | The x402 scheme is `exact`, for now. |
| DM8-12 | Buyer evidence directory | Combined MCP mode enables `retrieve_service_access` when `EREBUS_ACCESS_SERVICE_URL` is set and fixes the evidence directory to `<state_root>/agent`; a different `EREBUS_ACCESS_EVIDENCE_DIR` is rejected. |
| DM8-13 | x402 `exact` over Permit2 | Separate `x402-exact` mode through canonical `x402ExactPermit2Proxy`, seller-facilitated; the Permit2 nonce is the deal nullifier, so `(buyer, deal)` can pay once and cross-rail double payment is prevented by binding the agreement domain to the exact proxy. **Binding implementation choice: owner review pending.** Exposure is listed in the claims boundary below. |
| DM8-14 | Paired observation uses one shared finalized snapshot | Owner review pending (source fix, 2026-10-04): honest providers may report different current tips and finalized heights while agreeing on canonical history; requiring identical tips stalled the live scan. Paired observation derives one shared snapshot (the lower finalized and lower head of the two providers) and requires identical canonical blocks at those heights. Runtime pins, canonical hashes, complete history, and nonce agreement are unchanged; a falsified nonce, a finalized-ahead-of-head answer, or a different canonical hash still fails closed. The harness also bounds polling and requires the agreement lifetime to cover the verification window plus settlement/delivery. Uncommitted source; not in `0.3.0.dev4`. |
| DM8-15 | Bounded observation issues contiguous ranges concurrently | Owner review pending (source fix, 2026-10-05): buyer, seller, and auditor each paid a sequential deployment-to-head scan bounded by the public RPC's 100-block `eth_getLogs` cap. `ObservationLimits` gained `max_concurrent_queries` (1..=32, hard ceiling 32, default 8); one window of contiguous, non-overlapping ranges runs concurrently and the durable checkpoint advances only after every query in the window succeeds. The scan start is unchanged, no range is skipped, and a missing response is never absence evidence. Ancestry walks the same canonical blocks in bounded concurrent windows with the same parent-hash, timestamp, and anchor checks. A pending-history reply reports durable `broadcast_attempts` and `stage`; the harness resumes read-only catch-up and calls `settle` again only with zero attempts, and observes only after any attempt, ambiguous submission, or missing diagnostic. The Rust expiry rules, canonical anchors, runtime pins, exact payment binding, and the one-payment fence are unchanged. Measured on the Monad public RPC: 48 queries took 51.7 s sequentially and 6.4 s at concurrency 16, about 21 minutes of log-query time for the live range (authentication, ancestry, retries, seller verification, and auditing are separate). Uncommitted source; not in `0.3.0.dev4`. |

**Open release decisions:** secure shielded artifacts, ceremony evidence, and authenticated verifier
deployment; hosting and publishing the Metropolis registry (channel decided in DM8-9), platform
coverage, and public artifact distribution; hosted-service resources, quotas, retention, and
incident ownership; the x402 scheme and the measured per-request/prepaid/batched comparison; mainnet
activation (the default remains a testnet developer release). The x402 prepaid and batched models
remain experiments unless selected.

## Claims boundary and exposure

- **Public-bound settlement exposes** amount, asset, payer, recipient, fee, timing, and the
  participating parties, plus the complete accepted canonical agreement, blinding, and both final
  signatures in calldata. It proves payment execution, not hidden amounts or identities. Only the
  offchain negotiation history stays confidential.
- **x402 `exact` exposes** buyer, `payTo`, amount, token, the deal nullifier, and the Permit2
  payment authorization signature. The Erebus agreement opening, blinding, and bilateral agreement
  signatures are not published by this rail, unlike `ErebusSettlement`. Generic third-party x402
  client interoperability is unverified; the HTTP profile requires an authenticated Erebus request
  and a matching x402 v2 header.
- **Shielded settlement hides** amount and recipient on chain but still reveals timing and the
  participating endpoints. **Shielded has not run on Monad.** Suite-2 evidence is local Anvil only,
  with known-entropy test keys, a single-contributor setup, and test-only proving artifacts that
  must never hold real value. Production setup, ceremony, artifact distribution, and verifier
  administration do not exist.
- **Payment is not delivery.** An encrypted grant alone does not prove payment; independent
  finalized chain evidence is required. Resource-hash verification does not prove independent
  payment or a delivery audit. Seller receipts and `PAYMENT-RESPONSE` are seller assertions.
  `paid_but_undelivered` remains representable.
- **Measurements are not guarantees.** Local Anvil timings, block timestamps, and diagnostics are
  not Monad inclusion or finality latency. Live stage timings are local monotonic observations, not
  latency promises. Prepaid and batched are experiments, not product rails.
- **Trust is not independence.** Two RPC URLs are not independent providers; two-of-two consistency
  is not Byzantine consensus. Team-operated rehearsals, including the live runs and the withheld-path
  audit re-verification, are not external acceptance. Hosted operation does not exist.
- **Metadata stays exposed.** Transport hides content, not endpoints, mailbox ids, sizes, timing, or
  the fact of communication (DM2-9). Grant headers expose issuer key, recipient key, deal id, and
  expiry; reused issuer keys are linkable (DM7-4).

## Submission gates (unchecked items)

Rubric: Product Quality and Completeness, Technical Excellence, Monad Integration, Track Fit and
Problem Relevance, and Innovation and Impact, each 20%. The supplied deadline was October 13, 2026,
23:59 ET. Keep the official rules document with the final submission; this record is not a rules
source.

- [ ] A demo video of at most three minutes shows the product operating and a Monad interaction.
- [ ] A third party runs the submitted path from the README, without team-held fixtures.
  The reproducible handoff is [External Acceptance](metropolis-external-acceptance.md);
  it has not been run by an outside operator yet.
- [ ] Every release and privacy claim matches reproducible evidence.
- [ ] Independent external auditor verification with participant state unavailable. A team-operated
  re-verification of the live x402 payment with the participant paths withheld verified
  `payment_verified: true` (2026-10-05); the auditor and operator were still the same team.
- [ ] An external third-party fresh-environment rehearsal. The `0.3.0.dev4` channel exists at
  `https://poulavbhowmick03.github.io/erebus-metropolis/simple/` and a team-operated public install
  was verified on macOS arm64 (2026-10-04); no third party has run it. Linux qualification is
  CI-only.
- [ ] A recorded Monad demo, linked from the README and submission.
- [ ] Hosted services. Render deployment files are prepared and tested only; paid provisioning is
  not authorized and no hosted service exists.
- [ ] Inclusion and finality observation timings on live Monad, not differences between block
  timestamps. Local monotonic stage timings exist for public-bound and x402.
- [ ] M5's gate and the complete installed shielded negotiate-pay-access workflow (DM8-10 defers
  shielded deployment until the public-bound product is complete).

Do not mark these complete based on a local test, a document, a package build, or a team-operated
run.

### Demo requirements carried from the demo plan

The demo establishes two agents negotiating a service price privately, authorizing the same
agreement, and executing a matching payment; an auditor receives evidence for exactly one deal;
the seller releases an API capability after final settlement (a seller-controlled action). Use one
service and one supported asset, deterministic agent policies first and an LLM only after the
protocol passes the same tests. The settlement chain and privacy mode are chosen before the channel
opens; negotiating the chain is out of scope. A public-bound settlement exercises the coordinator
and backend boundary but does **not** satisfy the shielded demo gate; if shown, record it as an
architecture milestone, not a passing stage.

Measurable stages: source map, settlement feasibility (substitution fails), agreement
specification (valid and invalid vectors agree), private coordination (one transcript root across
retry/restart), Monad integration (local proof, relayed submission, note discovery, receipt
verification, replay failure, lost-response recovery), disclosure (auditor verifies one deal,
cannot decrypt a neighbor), service demo, and regression/rehearsal. The recording plan is a
90-second sequence: policies and private negotiation; matching commitments in both participant
records; public transaction and observer output with explicit metadata; finalized settlement and
successful API request; one-deal auditor disclosure; replay refusal and denied adjacent-deal
access. Measure the actual end-to-end duration before recording, label any time skip, and do not
present chain inclusion time as proof-generation or full-deal latency. The observer must run
without participant keys, transcript files, or auditor grants.

Store sanitized evidence under a dated `docs/runs/` directory: source commit, contract addresses,
chain id, pool/verifier versions, dependency revisions, reproduction commands, deterministic
policies, public transaction hashes, receipts, block identifiers, finality rule, raw observer
inputs, separately measured negotiation/proof/submission/inclusion/finality/delivery durations,
negative results for replay/wrong-recipient/changed-amount/expired/mixed-proof cases,
crash-after-broadcast recovery, one-payment confirmation, and a synthetic disclosure package
labeled as intentionally disclosed. Never publish private keys, local state directories,
credentials, or real confidential transcripts. Prior Starknet work stays separate from new
Metropolis transport, agreement, EVM proof, and Monad evidence; the organizer message permitting
significant new development belongs with the submission records, and this status does not
independently verify rules, dates, prizes, or sponsor eligibility.

## Reproducing evidence

Commands are recorded here as they appeared in the historical records. Local commands run from the
repository root unless stated. Live commands require operator configuration, funded accounts, and
explicit authorization.

```sh
# M0/M1: Rust and agreement vectors
cd sdk/rs && cargo fmt --check && cargo test --all-targets && \
  cargo clippy --all-targets -- -D warnings && cargo build --bin erebus-cli
cd sdk/core && cargo test --all-targets --locked
python3 scripts/check_agreement_vectors.py

# M2: transport
cd sdk/transport && cargo fmt --check && \
  cargo clippy --all-targets -- -D warnings && cargo test --all-targets

# M3: contracts and EVM adapter
cd contracts/evm && forge build && forge test
cd sdk/evm && cargo test --locked -- --include-ignored

# M4/M5: circuits, funded prototype, coordinator and crash matrices
cd circuits/m5 && EREBUS_M5_NATIVE_PROOF=1 EREBUS_M6_COORDINATED=1 npm run prototype
# add EREBUS_M6_MATRIX=1 for the funded shielded crash sweep in release mode

# M6: local-chain, coordinator, and relayer checks
forge build --root contracts/evm
cargo test --manifest-path sdk/evm/Cargo.toml --locked --offline -- --ignored
cargo test --manifest-path sdk/coordinator/Cargo.toml --locked --release --test lifecycle every_discovered -- --ignored

# M7/M8: native negotiation, payment, delivery, and disclosure
cargo test --locked --manifest-path sdk/shielded/Cargo.toml --test negotiation_cli -- --include-ignored
cargo test --locked --manifest-path sdk/shielded/Cargo.toml --test negotiation_cli payment_driver -- --include-ignored
cargo test --locked --manifest-path sdk/shielded/Cargo.toml --test negotiation_cli shielded_driver -- --include-ignored
uv run --locked pytest agents/tests/test_metropolis_loop.py \
  mcp-server/tests/test_metropolis.py sdk/py/tests/test_metropolis_seam.py

# x402 service-model comparison (local Anvil, experimental models)
cd contracts/evm && forge build && cd ../../sdk/evm && \
  cargo test --test x402_service_models -- --ignored --nocapture

# M8 deployment identity and readiness
echo '{"chain_id":10143,"rpc_url":"https://testnet-rpc.monad.xyz","timeout_ms":10000}' \
  | erebus-network-check
python3 scripts/check-evm-deployment.py   # operator configuration and contract pins

# M8 registry build/install check (local unpublished index)
uv run --locked python scripts/build-metropolis-registry.py \
  --version 0.3.0.dev20261004 --profile release --build --output artifacts/metropolis-registry
uv run --locked python scripts/check-metropolis-install.py --registry artifacts/metropolis-registry
# add --rehearse-x402 for the installed x402 recovery/auditor rehearsal

# M8 live rehearsal (operator plan, explicit authorization, funded accounts)
python3 scripts/metropolis-monad-rehearsal.py init --plan plan.json --workdir DIR
python3 scripts/metropolis-monad-rehearsal.py preflight --workdir DIR
python3 scripts/metropolis-monad-rehearsal.py run --workdir DIR --authorize-live-transactions
```

Published install path: [metropolis-install.md](metropolis-install.md) §10; the same commands and
platform qualifiers are in the [README](../README.md). The package channel, rubric boundary, and
external-acceptance checklist are in the
[submission gates section](#submission-gates-unchecked-items) above and in
[metropolis-roadmap.md](metropolis-roadmap.md). Installed commands, operator configuration, and
recovery boundaries are in [metropolis-operations.md](metropolis-operations.md). Architecture,
threat model, and agreement limits stay in [metropolis-architecture.md](metropolis-architecture.md),
[metropolis-threat-model.md](metropolis-threat-model.md), and
[metropolis-agreement.md](metropolis-agreement.md). Record new stack friction in
[friction.md](friction.md); existing Metropolis friction entries referenced by this document
include F12, F38, F43-F46, F48-F50, and F57.

