# Metropolis M7 Progress

Updated: 2026-10-02. Branch: `metropolis`. M7 is complete at its local verification gate.
Monad deployment and published package installation remain M8 work.

`erebus_transport::disclosure::verify_selected_agreement` replays the messages selected for one
deal. It checks the transcript root against the accepted terms, recomputes the blinded deal
commitment and nullifier, and verifies both role-specific authorizations. It verifies signatures
after expiry because expiry does not erase evidence of an agreement.

`DisclosureGrant` encrypts that evidence to a separate X25519 disclosure key using Noise N.
The issuer signs the recipient key, deal, expiry, and ciphertext.
Version 1 uses a suite-1 identity. Version 2 uses the participant's suite-2 agreement key directly.
The grant has a bounded, versioned binary format and an owner-only file backup. A recipient
can open it from that backup without either participant or the relay. Tests cover a package
larger than one Noise frame, wrong recipient, wrong issuer, expiry, changed ciphertext,
missing or altered transcript messages, a neighboring deal, and changed terms.

For public-bound EVM settlement, `verify_public_bound_disclosure` opens the grant and queries
its own RPC for finalized deal evidence. It accepts only a payment whose nullifier, commitment,
amount, and fee match the disclosed agreement. It also requires the grant issuer to be the
buyer or seller authorization address. An Anvil test rejects an unpaid deal, a valid but
different amount, and a grant signed by a third party.

An additional Anvil test writes the encrypted grant and a separate auditor key to disk. A
fresh auditor subprocess loads only those files and its own RPC configuration, then verifies
the finalized public-bound payment. This proves offline handoff and independent verification
of the selected deal in a local test, not an installed operator workflow.

## Shielded Payment Verification

`erebus_shielded_prover::disclosure::verify_shielded_payment` accepts an opened suite-2 selected agreement.
It verifies the transcript, canonical opening, and both participant authorizations before RPC observation.
It derives the settlement context and signed revision from that opening, not from a supplied receipt.

The helper uses the existing paired pool observer with separate public caches.
Both sources must agree on canonical head and finalized anchors, consumed state, and the matching pool event.
Only a finalized winner with the disclosed commitment, nullifier, amount, and zero fee passes.
A different authorized revision of the same deal does not pass.
The auditor needs no note wallet, spend key, prover, or participant process.
The verified SDK result contains the opened selected agreement and independently observed settlement evidence.
The encrypted grant carries the signed lookup context; it does not carry a trusted issuer receipt.

This opened-evidence helper verifies payment against a trusted pool deployment, not grant issuance or service delivery.
The CLI authenticates the version-2 grant before invoking it.
The owner selected direct suite-2 signing; see [M7 decisions](metropolis-m7-decisions.md).
Deployment code and verifying-key authentication remain release requirements; two RPC URLs do not prove provider honesty or independence.

Verification on 2026-10-01:

- Seven new local tests passed for signed evidence, payment matching, and rejection before RPC requests.
- All 13 socket-free shielded library tests passed; strict Clippy and rustdoc passed.
- Two RPC fixture tests compile but could not run: the sandbox rejected loopback server binds with `Operation not permitted`.
- Those fixtures test observation wiring, not funded transfers or proof verification. A funded independent auditor run remains required.

## SDK Backup Recovery

`SelectedAgreement::from_store` loads one deal from participant transcript storage.
It requires retained accepted terms, blinding, authorizations, and the transcript hash version.
It verifies the transcript root, commitment, nullifier, and both signatures before returning export evidence.
Missing messages produce `TranscriptUnavailable`; inconsistent evidence fails verification.
This helper does not query the chain or submit a payment.

The retention tests exchange encrypted messages between two Noise sessions through a file relay.
Each participant saves its transcript independently.
The tests expire and prune relay messages, then reopen participant storage.
They establish these recovery cases:

- A saved grant opens with a restored auditor key after both participant transcripts and relay messages are deleted.
- A missing grant can be reissued from retained agreement evidence and the reopened participant transcript.
- Missing transcripts or inconsistent signed evidence prevent reconstruction.
- Missing grant and participant evidence do not produce a replacement or imply an unpaid deal.

The fixture retains agreement openings in memory; it does not test an installed participant application's durable agreement backup.
These are SDK tests, not a separate-process issuer workflow or a shielded-payment verification test.

Verification: 57 transport unit tests and 9 disclosure integration tests passed.
The disclosure suite includes four new backup-recovery tests.

## Durable Issuer Read Path (2026-10-01)

`erebus_coordinator::read_disclosure_opening` reads the durable opening and both authorizations
for one recorded revision directly from a coordinator state directory. It needs no session
configuration, never queries or submits to a chain, and fails closed on a missing initialization
marker, a buyer mismatch, an invalid snapshot, or an unknown operation.
It checks and bounds the marker before reading it. Snapshot validation verifies stored
authorization signatures. It leaves snapshots unchanged, but opens the private directory
with journal permissions and a lock. The operator must control that directory and its parents.

The `erebus-disclosure` CLI gains a `select` method: it reads that opening, replays the
participant's `FileTranscriptStore`, verifies the transcript root and both signatures, and writes
a new owner-only canonical evidence file. The existing `export` method consumes that file. This
closes the gap where evidence could only be produced by an SDK caller holding the opening in
memory.

Verification: five coordinator tests cover read-back, absent or unknown state, corrupt and
oversized markers, symlink markers, and invalid stored signatures. Missing state does not
create a state directory. A CLI test asserts that missing state produces no evidence file.

The CLI issuer/recipient workflow is exercised end to end in separate processes
(`issuer_selects_evidence_from_durable_state_and_a_fresh_recipient_verifies`): one process
persists a coordinator state and a transcript store; a fresh `select` process rebuilds canonical
evidence from them; a fresh `export` process seals a grant to an auditor key; a fresh
`verify_agreement` process runs in a separate directory containing only the grant and auditor
key, after the participant directory is deleted. It needs no chain or prover. The test also
checks owner-only evidence, no overwrite, and rejection of an unavailable transcript namespace.
Agreement verification explicitly reports payment and delivery as unverified.

Review verification on 2026-10-01:

- 24 coordinator tests passed; the exhaustive storage sweep remains ignored in this run.
- EVM checks passed: 27 unit, 12 disclosure CLI, 12 offline signing, 7 relayer CLI, and
  3 non-Anvil local-chain tests. The 33 Anvil tests were ignored in the all-targets run.
- Installed `erebus-disclosure` from the local source into a separate temporary directory.
  All 12 disclosure CLI tests also passed against that installed executable.
  This is a local installation check, not evidence of a published release or clean machine onboarding.
- Strict Clippy and rustdoc passed for coordinator and EVM.
- The two Anvil disclosure tests could not run: the sandbox rejected localhost binds with
  `Operation not permitted`. The funded subprocess test now also invokes the CLI's
  `verify_payment`; that added path compiles but still needs an unrestricted Anvil run.

The three remaining implementation gates from this earlier review are now verified below:
direct suite-2 signing, the shielded CLI/MCP path, and a funded independent auditor run.

## MCP And Transcript Integration (2026-10-02)

`erebus.DisclosureSeam` forwards typed, path-only JSON requests to `erebus-disclosure`.
It does not read keys, implement cryptography, or return raw process output on failure.
Unexpected response fields and incorrect payment/delivery claims fail closed.
The separate binary protocol is version 1; it does not change the Starknet seam protocol.

`EREBUS_BACKEND=disclosure` starts a disclosure-only MCP server without Starknet or prover
configuration. Auditor tools return a public key or verification facts, not private evidence.
Issuer tools use configured coordinator and transcript stores and an issuer key-file path.
Artifact names are restricted to one bounded filename in an existing owner-only directory.
Payment tools use the operator's fixed deployment configuration, not a model-supplied RPC.

The fresh-auditor workflow also runs through the official MCP stdio client after participant
storage is deleted. CI requires this path, builds the disclosure binary, and runs the Python
disclosure tests without a missing-binary skip. The completed shielded workflow is recorded below.
Neither path is a published Metropolis package.

The M5 harness calls `sdk/transport/examples/m5_transcript.rs` to build canonical offer and
counter messages and saves `transcript.json`. The root is committed into the suite-2 agreement
before authorization and proof construction. The coordinated funded runner persists those
messages, restores the agreement opening from coordinator storage, and checks
`SelectedAgreement::from_store` before settlement. A replay test checks the exported root,
cross-deal rejection, and changed-message detection. Circuits and proving keys are unchanged.

Earlier restricted-environment verification on 2026-10-02:

- 19 disclosure-specific Python tests and 8 existing server tests passed.
- The separate issuer/recipient workflow passed through the actual MCP stdio server after
  participant storage was deleted (`EREBUS_TEST_MCP_PYTHON` set for the Rust CLI test).
- EVM all-targets checks passed: 61 tests; 33 Anvil tests remained ignored in this run.
- Transport checks passed: 57 unit, 9 disclosure, 6 socket-free Eleusis, and 1 transcript
  fixture test. Two socket tests were filtered after confirmed localhost bind failures.
- Shielded library checks passed: 13 tests; two RPC fixture tests were filtered.
- Strict Clippy and rustdoc passed for transport, EVM, and shielded crates.
  The updated funded runner compiles; its funded execution is not verified here.
- The full Python run was not green. Three onboarding fixtures could not bind localhost,
  three old seam tests initially failed at the restricted uv cache, and three demo tests
  referenced absent frontend components. The seam tests passed with `UV_NO_SYNC=1` and a
  writable cache; unrelated frontend tests were not changed.
- `uv sync --offline --all-packages` could not resolve hatchling in the writable cache.
  Python checks used the existing workspace environment, not a fresh install.

## Completed Local Gate (2026-10-02)

The owner selected direct suite-2 grant signing. `DisclosureGrant::seal_shielded` rejects a seed that does not belong to either participant.
Version 2 uses the participant key and a domain-separated BabyJubJub signature over the complete encrypted grant.
Opening verifies the independently expected issuer, recipient, expiry, transcript, commitment opening, both authorizations, and participant membership.
Version 1 remains supported. It cannot wrap a suite-2 agreement as a substitute for version-2 issuer authentication.
The core test pins independent circomlibjs disclosure-message vectors and rejects those signatures as payment authorizations.

The shared CLI protocol handles keys, selection, export, and offline agreement verification for both suites.
`erebus-disclosure` observes public-bound payment. `erebus-shielded-disclosure` also observes shielded payment through paired RPCs and separate caches.
The Python binding and disclosure-only MCP use the same methods and return verification facts, never plaintext terms or spending keys.

The funded M5 harness now creates a real replayable transcript before authorization.
After one private payment, separate issuer commands rebuild evidence from durable state and encrypt a grant to a fresh auditor.
The harness renames participant storage out of reach before starting auditor commands.
The auditor receives only its key, encrypted grant, and trusted public deployment configuration.
It verifies the agreement offline, verifies finalized payment through its own public caches, and repeats verification without paying again.
The official MCP stdio client verifies that same funded payment. Delivery remains explicitly unverified.

Current local evidence:

- 13 disclosure CLI tests pass, including suite-2 durable selection and a fresh MCP auditor after participant storage is deleted.
  All 13 also pass against `erebus-shielded-disclosure` installed into a separate temporary prefix.
  This local source installation is not M8's package-only fresh-environment release gate.
- Transport passes 57 unit tests, 11 disclosure tests, 8 Eleusis tests, and the transcript fixture test.
  A validly signed and encrypted third-party grant fails participant authentication on opening.
- The funded native-proof Anvil flow passes with separate shielded CLI and MCP auditors.
  The combined run also passes all 75 funded crash-point trials and writes redacted disclosure facts into `build/run.json`.
- Both disclosure RPC fixtures and all 11 shielded finality/history tests pass with loopback sockets enabled.
- The full SDK/MCP/agent Python suites pass: 219 tests, 2 skipped.
- The public-bound Anvil regression suite passes all 36 tests, including the independent auditor process.
- The coordinator exhaustive sweep passes all 25 lifecycle tests, including both settlement modes.
- Core signature-domain tests pass. Strict Clippy and rustdoc pass for the shared CLI.

These results are local macOS evidence. They do not prove Linux CI, live Monad behavior, a published release, or production privacy.
The proving artifacts still use known test entropy; no ceremony or production verifier readiness is claimed.
Participant authorization and grant metadata remain visible to the auditor as documented.

See [disclosure operation and backup](metropolis-m7-runbook.md).

Disclosure expiry is a verifier policy, not cryptographic revocation.
A recipient with the key and ciphertext can bypass that policy or retain already disclosed plaintext.
