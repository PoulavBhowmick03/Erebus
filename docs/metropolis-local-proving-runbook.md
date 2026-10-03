# Metropolis Local Proving

The implementation downloads public artifacts and generates proofs locally.
It does not require a hosted prover or an operator setup ceremony.
The current tests use insecure prototype keys. No secure artifact release is published here.

## Trust boundary

Obtain the manifest digest from an independently trusted release source.
Do not trust a digest supplied only by the artifact server.
Authenticate live verifier code and ceremony evidence separately.
The manifest hash alone cannot establish either fact.

The manifest binds a chain ID, settlement contract, and verifier version.
It lists WASM, R1CS, and proving-key files for deposit, transfer, and withdrawal.
Each entry contains an HTTPS URL, exact byte length, and SHA-256 digest.
The installer caps individual files at 256 MiB.
The download service sees artifact requests and client network metadata, not the local witness.

## Local command

Build the Rust command:

```sh
cargo build --locked --manifest-path sdk/shielded/Cargo.toml --bin erebus-local-prove
```

Send one JSON request on standard input:

```json
{
  "manifest_file": "/absolute/path/release.json",
  "manifest_sha256": "<independently trusted lowercase SHA-256>",
  "circuit": "transfer",
  "cache_root": "/absolute/path/artifact-cache",
  "witness_file": "/absolute/path/private/transfer.json",
  "expected_public": ["<chain ID>", "<contract as decimal integer>", "<verifier version>", "<remaining public signals>"]
}
```

Replace all placeholders before use.
Deposit requires six public signals, transfer eleven, and withdrawal eight.
The first three must match the authenticated manifest domain.
The witness must be a regular, owner-only file, at most 512 KiB.

The command returns public Solidity calldata, downloaded bytes, cache hits, and separate installation and proof times.
It verifies the proof locally. It does not sign authorization or submit a payment.
A successful proof is not finalized payment evidence.

For development fixtures only, set `allow_test_artifacts` to `true`.
Literal loopback HTTP also requires `allow_loopback_http: true`.
Do not enable these exceptions for a secure release.

## MCP configuration

The MCP mode exposes `prove_local_transition` for an already prepared local witness.
It does not yet provide the complete installed negotiation and payment workflow.
Configure these operator-controlled environment variables:

```sh
export EREBUS_BACKEND=local-prover
export EREBUS_LOCAL_PROVER_CLI=/absolute/path/erebus-local-prove
export EREBUS_WITNESS_DIR=/absolute/path/private
export EREBUS_ARTIFACT_MANIFEST=/absolute/path/release.json
export EREBUS_ARTIFACT_MANIFEST_SHA256=<independently-trusted-digest>
export EREBUS_ARTIFACT_CACHE=/absolute/path/artifact-cache
```

The witness directory must have mode `0700`.
Python passes file paths to Rust, not witness contents through the model tool call.
The tool accepts a single bounded witness filename, circuit, and expected public signals.
The operator fixes the artifact source and cache, not the agent.

Published platform binaries and automatic release discovery remain open M8 work.
The component installation test builds local SDK and MCP wheels into an isolated environment.
This test does not prove installation from a public package registry.

## Failure recovery

The installer locks each content digest and persists verified files atomically.
After interruption, retry the same request.
An incomplete download is not a usable cache entry.
A corrupt regular cache file is replaced after a fresh verified download.
Symlink cache entries are rejected rather than followed.

Download outages affect first installation or cache repair.
With all verified artifacts cached, proving does not require the artifact server.
The host still needs enough CPU and memory for the selected circuit.
Operator RAM requirements have not been measured for the release target.

## Verification

After generating the existing M5 fixtures, run the real artifact tests:

```sh
cargo test --locked --manifest-path sdk/shielded/Cargo.toml --test artifacts -- --include-ignored
EREBUS_M8_TEST_LOCAL_PROOF=1 .venv/bin/python -m pytest -q mcp-server/tests/test_proving.py
```

The first command proves all three circuits from downloaded artifacts in fresh Rust processes.
The second builds isolated Python wheels and exercises a real MCP client.
Both are local development checks. Neither authenticates a live shielded deployment.
