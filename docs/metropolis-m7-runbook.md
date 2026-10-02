# Disclosure Operation And Backup

Both commands export and verify suite-1 public-bound and suite-2 shielded agreements.
`erebus-disclosure` verifies public-bound payment. `erebus-shielded-disclosure` also verifies shielded payment.
Send one JSON request on stdin. Each invocation emits one JSON response.
These commands are development-branch tools, not published Metropolis release packages.

1. Generate a `DisclosureIdentity` for the auditor and store its private key in an owner-only file.
   Give only its public key to the issuer. Keep this key separate from Eleusis transport keys.

   ```json
   {"method":"keygen","key_file":"auditor.key"}
   ```

   Use `key_info` with the same `key_file` to recover the public key without replacing the key.
2. Keep the issuer's `FileTranscriptStore` and agreement opening available until a grant has
   been exported. Relay retention is seven days and is not a transcript backup.
3. Build canonical selected evidence from durable participant state. With `erebus-disclosure`,
   call `select`:

   ```json
   {"method":"select","state_root":"<coordinator dir>","operation_ref":"<64 hex>",
    "store_root":"<transcript store dir>","namespace":"<store namespace>",
    "transcript_hash_version":1,"evidence_file":"<new owner-only path>"}
   ```

   It reads the opening and both authorizations from the coordinator snapshot, replays the
   participant transcript, verifies the transcript root and both signatures, and writes a new
   owner-only evidence file. SDK callers can call `SelectedAgreement::from_store` directly with
   the same inputs. A missing or mismatched snapshot fails closed and writes nothing.
   The accessor verifies snapshot authorizations; the transcript check happens during selection.
   It does not change snapshots, but tightens directory permissions and acquires a journal lock.
   Keep the state directory and its parents under the operator's control.
   A write or sync failure can leave an incomplete output. Do not overwrite it; retry with a
   new path after resolving the storage fault.
4. Export an encrypted grant using the auditor's public key and a buyer or seller issuer key.
   `issuer_key_file` contains the participant's raw 32-byte key in an owner-only file.
   For suite 1, this is the secp256k1 private key. For suite 2, this is the participant's agreement seed.
   `expires_at` is a future Unix timestamp in seconds.

   ```json
   {"method":"export","evidence_file":"selected.evidence","issuer_key_file":"issuer.key",
    "recipient_public_key":"<64 hex>","grant_file":"deal.grant","expires_at":4102444800}
   ```

   This creates a new owner-only backup and refuses to overwrite one. SDK callers can use
   `DisclosureGrant::write_backup`. Deliver the encrypted grant, not the selected evidence file.
5. The auditor loads its disclosure key and the grant backup. For a public-bound EVM deal,
   verify the agreement offline using a trusted expected participant address:

   ```json
   {"method":"verify_agreement","grant_file":"deal.grant","key_file":"auditor.key",
    "expected_issuer":"<0x participant address>"}
   ```

   This does not establish payment. To check payment, use `verify_payment` with the same fields
   and an independently configured deployment:

   ```json
   {"method":"verify_payment","grant_file":"deal.grant","key_file":"auditor.key",
    "expected_issuer":"<0x participant address>","deployment":{
      "namespace":"<eip155 chain namespace>","settlement_contract":"<0x contract address>",
      "verifier_version":1,"rpc_url":"<trusted RPC URL>"}}
   ```

   SDK callers can use `verify_public_bound_disclosure` with an independently configured
   `EvmChain`. Both paths require a working trusted RPC and finalized matching payment evidence.
   Neither verifies service delivery.

For a suite-2 grant, `expected_issuer` is `0x` followed by the 128 lowercase hex digits of `Ax || Ay`.
Use the same `verify_agreement` request for offline verification.
Use `erebus-shielded-disclosure` for payment verification with this deployment configuration:

```json
{"method":"verify_payment","grant_file":"deal.grant","key_file":"auditor.key",
 "expected_issuer":"<0x suite-2 participant key>","deployment":{
   "namespace":"<eip155 chain namespace>","settlement_contract":"<0x pool address>",
   "verifier_version":2,"rpc_url":"<primary RPC>","peer_rpc_url":"<independent peer RPC>",
   "first_block":123,"first_hash":"<0x deployment block hash>","cache_root":"<public cache directory>"}}
```

The deployment anchor and verifier version must come from a trusted deployment manifest.
The tool creates separate `primary` and `peer` public caches. It never needs a note wallet or proving artifacts.
If observation is pending or unavailable, retain those caches and retry the same request.
Never interpret an unavailable result as non-payment. Never submit another payment from the auditor command.

The grant contains the selected deal transcript, accepted terms, blinding, and authorizations.
It contains no parent Eleusis session key or spending key. Its public header reveals the
issuer address or suite-2 public key, recipient public key, deal ID, expiry, and ciphertext size. The recipient can
read the disclosed business terms after opening it. Public-bound EVM settlement also exposes
the payment on chain.

Back up the recipient's private disclosure key separately. The encrypted grant alone cannot be
opened after that key is lost. If the issuer loses the transcript and has no local backup before
export, the grant cannot be reconstructed after relay data expires. A damaged or partial grant
backup fails decoding or signature verification; do not infer that the deal was unpaid.

## Missing Grant Recovery

If the grant is missing, retain the complete transcript, accepted terms, blinding, and both authorizations.
Use `SelectedAgreement::from_store` to reconstruct verified export evidence from participant storage.
Use the auditor's public disclosure key to seal a replacement grant.
Save the replacement at a new or vacant backup path.
The backup writer refuses to overwrite existing files, including damaged backups.

This recovery needs no relay messages after retention expires.
Relay retention and grant expiry are separate deadlines.
If the grant has expired, an authorized issuer must create a new grant with a new deadline.

If both the grant and reconstruction evidence are missing, report unavailable disclosure evidence.
Do not infer payment status from missing files or submit another payment.
Durable-opening recovery is available: `read_disclosure_opening` reads the
opening and both authorizations from the coordinator snapshot without a session configuration,
and the `select` CLI method rebuilds evidence from it plus the transcript store. The CLI
workflow test deletes participant storage before a fresh auditor verifies the agreement.
This offline check does not verify payment or delivery.
The funded local harness separately verifies shielded payment through fresh CLI and MCP processes with participant storage unavailable.

## Disclosure-Only MCP

On the `metropolis` development branch, build `erebus-disclosure` from `sdk/evm`.
For shielded payment, build `erebus-shielded-disclosure` from `sdk/shielded` and select that executable instead.
Use the branch's Python environment; published Starknet packages do not include this new surface.
Start with an existing private artifact directory:

```sh
mkdir -m 700 ~/.erebus-disclosure
export EREBUS_BACKEND=disclosure
export EREBUS_DISCLOSURE_DIR="$HOME/.erebus-disclosure"
export EREBUS_DISCLOSURE_CLI="/absolute/path/to/erebus-disclosure"
erebus-mcp-server
```

For a source checkout, run `.venv/bin/python -m erebus_mcp.server` instead of the installed entry point.
No Starknet address, wallet, or prover URL is required for this mode.
The operator must control the artifact directory and its parents. Do not use a symlink directory.

Auditor tools are `create_disclosure_key`, `disclosure_key_info`, and
`verify_disclosed_agreement`. The key defaults to `auditor.key` in the artifact directory;
`EREBUS_DISCLOSURE_KEY_FILE` can select another operator-controlled path.
Copy only the encrypted grant into that directory. Supply the expected participant identity
from an independent trusted source. Tools return public verification facts, not plaintext terms.

Set all four issuer settings to enable selection and export:

- `EREBUS_DISCLOSURE_STATE_ROOT`: coordinator state directory.
- `EREBUS_DISCLOSURE_STORE_ROOT`: retained transcript store directory.
- `EREBUS_DISCLOSURE_NAMESPACE`: transcript namespace.
- `EREBUS_DISCLOSURE_ISSUER_KEY_FILE`: participant signing key-file path.

Then use `select_deal_disclosure` followed by `export_deal_disclosure`.
Artifact names must be single filenames, not paths. Python never opens the signing key.
Keep grants and auditor keys backed up; missing files do not imply non-payment.

Set `EREBUS_DISCLOSURE_DEPLOYMENT` to the deployment JSON shown above to enable
`verify_disclosed_payment`. This RPC configuration is fixed by the operator, not the model.
Failures return structured `DISCLOSURE_UNAVAILABLE` results and do not submit transactions.
Suite-2 export signs locally with the participant seed. No separate suite-1 disclosure key is needed.
The same tools support both modes; `mode` in the verification result states which agreement was verified.

## Shielded Payment Check

For an opened suite-2 agreement, call `erebus_shielded_prover::disclosure::verify_shielded_payment`.
Configure two independently operated pool RPCs with separate `IndexStore` caches and the same trusted deployment anchor.
The function verifies the transcript and authorizations before reading public pool state.
It requires a finalized matching payment, not a caller-supplied receipt or confirmation count.
It sends no transcript, amount, blinding, note opening, or private key to either RPC.

If observation returns `HistoryPending`, reopen the same caches and retry.
If a source fails or disagrees, report verification as unavailable.
Do not infer an unpaid deal or authorize another payment.

This opened-evidence function does not authenticate a grant issuer by itself.
The CLI first opens the version-2 grant and verifies the issuer against the disclosed buyer or seller key.
SDK callers must call `DisclosureGrant::open` with the independently expected participant key before making the full disclosure claim.
The deployment code and verifying keys need independent authentication; RPC agreement alone is insufficient.
The disclosure RPC fixtures in `sdk/shielded/src/disclosure.rs` exercise observation wiring,
not a funded transfer. The separate funded harness covers the fresh CLI/MCP auditor path.

Local reproduction from the repository:

```sh
uv sync --all-packages --locked
cd circuits/m5
EREBUS_TEST_MCP_PYTHON="$PWD/../../.venv/bin/python" \
  EREBUS_M5_NATIVE_PROOF=1 EREBUS_M6_COORDINATED=1 \
  NODE_OPTIONS=--max-old-space-size=4096 npm run prototype
```

This requires the documented M5 compiler, Node dependencies, Rust toolchain, and Anvil.
The test setup uses known entropy. Never use it for real value.
These developer prerequisites do not satisfy M8's package-only operator onboarding gate.
See [M7 decisions](metropolis-m7-decisions.md) for the grant format and public metadata boundary.

Expiry stops future opens through the verifier, not through cryptography.
A recipient with the key and ciphertext can bypass that policy.
Expiry cannot erase plaintext already disclosed.
