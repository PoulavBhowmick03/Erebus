# Erebus reference

Operator and integrator reference for Erebus. The [README](../README.md) covers what Erebus
is and how to install it; this covers running it.

This page describes current `main` and CLI Protocol 4. The published `v0.1.0` wheel speaks
Protocol 2 and exposes ten tools. Build the current checkout until `v0.2.0` is published.

- [Set up an identity](#set-up-an-identity)
- [Configure](#configure)
- [Check before you spend](#check-before-you-spend)
- [The MCP tool surface](#the-mcp-tool-surface)
- [Errors and retries](#errors-and-retries)
- [The CLI protocol](#the-cli-protocol)
- [Building from source](#building-from-source)

For the reproducible walkthrough see [runbook.md](./runbook.md); for what leaks and what
does not, [privacy-model.md](./privacy-model.md); for current state,
[status.md](./status.md).

---

## Set up an identity

An Erebus identity is a Starknet account plus two key files, registered with the pool and
holding shielded notes. Getting there takes six on-chain steps, and one script does all of
them:

```bash
scripts/new-identity.sh bootstrap erebus-a ~/.erebus-a <funder-account>
```

That runs: create the account → fund it → deploy → generate the pool key and extract the
account key → write an env file → approve the pool for the live per-write fee → wait for
the approval to reach proving depth → shield 1 STRK (which also registers the identity) →
`doctor`. It exits non-zero if `doctor` is not ready.

Without a funded account to pay from, use the faucet flow instead — `create`, fund the
printed address by hand, then `activate`. Both are documented in the script's header and in
[runbook.md](./runbook.md).

> **Registration is irreversible and writes the identity's pool private key encrypted to
> the pool's auditor on-chain.** From that moment the auditor can decrypt everything that
> identity ever does. Use testnet keys only.

**Three keys, and conflating them is the usual mistake:**

| Key | Purpose | Who sees it |
|---|---|---|
| Starknet account key | Signs transactions. Custody | Never leaves the Rust process |
| Pool private key | The STRK20 identity. Confidentiality | Sent in `compile_actions` calldata to your prover and preflight RPC — both must be operator-controlled |
| Pool auditor key | Pool-wide, set once at registration | StarkWare's, no rotation |

Python never sees key material, only file paths. See
[custody-design.md](./custody-design.md).

## Configure

The server reads its configuration from the environment and fails at startup naming
whatever is missing.

**Always required:**

| Variable | Meaning |
|---|---|
| `AGENT_ADDRESS` | This identity's Starknet account address |
| `PROVING_SERVICE_URL` | Your prover. It receives the pool private key, so it must be one you control |
| `EREBUS_SETTLEMENT_ROLE` | `payer`, `payee`, or `both`. A payee server structurally refuses `accept_and_settle` |

**Backend selection:** `EREBUS_BACKEND` is `mock` (default — no chain, no keys, no gas) or
`seam` (the real Rust client). `seam` additionally requires:

| Variable | Meaning |
|---|---|
| `STARKNET_RPC_URL` | Preflight RPC. Also receives the pool key — operator-controlled |
| `POOL_ADDRESS` | The STRK20 privacy pool |
| `STARKNET_CHAIN_ID` | e.g. `0x534e5f5345504f4c4941` for Sepolia. Part of every channel-key preimage, so a mismatch reads as "not found" everywhere |
| `TOKEN_ADDRESS` | The ERC-20 being settled |
| `POOL_KEY_FILE`, `ACCOUNT_KEY_FILE` | Paths, mode `0600`. Never read by Python |
| `EREBUS_STATE_DIR` | Channel state, mode `0700` |

**Optional:** `EREBUS_CLI` (explicit binary path; defaults to the packaged one),
`EREBUS_SKIP_STARTUP_DOCTOR=1` (skip the boot-time inspection when starting offline), and
the `EREBUS_MOCK_*` knobs for mock runs.

`scripts/new-identity.sh` writes a complete env file. Start the server with:

```bash
set -a && . ~/.erebus-a/env && set +a
EREBUS_BACKEND=seam EREBUS_SETTLEMENT_ROLE=payer erebus-mcp-server
```

## Check before you spend

Every setup fault in this stack surfaces the same way: `apply_actions` reverts with a bare
`Contract error` naming nothing, **after** a proof has been generated and paid for. `doctor`
answers those questions first, read-only, in one pass:

```bash
erebus-cli doctor   # or the `doctor` MCP tool, or automatically at server startup
```

Ten checks in dependency order — both key files and their modes, the state directory, RPC,
prover, chain id read from the node and compared with config, pool identity and version,
registration against the key file, allowance against the live fee, and public balance.
Every non-passing check carries a `repair` string naming one direct action.

`ready: false` means a write will fail right now. A `skipped` check is not a pass — it
means the thing it would have verified is unverified.

## The MCP tool surface

Thirteen tools. Amounts are decimal strings and `memo_hash` is a hex string. A JSON number
loses precision above 2^53, and 1 STRK is 1e18.

| Tool | Signature | Notes |
|---|---|---|
| `doctor` | `()` | Read-only. Always safe to call |
| `get_note_balance` | `()` | Payer must call before naming a price |
| `open_channel` | `(operation_id, counterparty)` | Returns `channel_handle`. Not private. See F38 |
| `propose_offer` | `(operation_id, channel_handle, amount, token, deadline, memo_hash)` | The payee asks. The payer offers |
| `counter_offer` | `(operation_id, channel_handle, reply_to, amount, token, deadline, memo_hash)` | Does not withdraw the offer it replies to |
| `read_channel_state` | `(channel_handle)` | Every visible offer plus the settlement list |
| `wait_for_offers` | `(channel_handle, expected_count, timeout_seconds=300)` | One tool call instead of a poll loop. A timeout is not an error |
| `accept_and_settle` | `(operation_id, channel_handle, offer_id)` | **Payer only.** Spends the caller's notes. Settles one deal. The channel pair can start another |
| `reconcile` | `()` | Read-only. Classifies journaled operations and never submits |
| `resume_operation` | `(operation_id)` | Explicitly resumes one safe operation or returns the required operator action |
| `rebuild_state` | `()` | Rebuilds missing channel records from the pool key and chain data |
| `grant_viewing_key` | `(operation_id, channel_handle, deal_id, grantee, expires_at, output_path)` | Encrypts one deal to a registered recipient. Writes a new mode-`0600` file and returns no secret |
| `reveal` | `(grant_path)` | Reconstructs the selected deal. The configured pool key must match the recipient |

Every result is an envelope: `{"ok": true, "backend", "network", "result": {...}}` or
`{"ok": false, "backend", "network", "error": {"code", "message", "retryable"}}`. `backend`
("mock" or "seam") and `network` are on every result, success or failure, so a transcript
alone tells a model whether it is talking to a real chain and which one.

**Two protocol rules that surprise people.** One channel pair can carry multiple deal IDs,
and settling one deal does not close the pair. An offer has no `withdrawn` state; it is
accepted or it expires, so a short deadline is the only way to bound how long a stale price
stays acceptable.

Every write takes `operation_id`. Use `op_` plus 64 lowercase hexadecimal characters.
Persist the ID and canonical intent before the MCP call. Reuse both after a restart.

## Errors and retries

Branch on the group, not the individual code. Every error also carries `retryable` — trust
it over guessing from the name.

| Group | Codes | What to do |
|---|---|---|
| The offer is wrong | `OFFER_EXPIRED`, `OFFER_UNKNOWN`, `ALREADY_SETTLED`, `NOT_YOUR_OFFER`, `AMOUNT_MISMATCH`, `INSUFFICIENT_NOTES`, `INDEX_CONFLICT` | Build a different offer. Retrying verbatim will not help |
| Funding or identity policy | `INSUFFICIENT_ALLOWANCE`, `INSUFFICIENT_BALANCE` | Change the allowance or fund the account before a new attempt |
| Durable operation state | `OPERATION_CONFLICT`, `RECONCILIATION_REQUIRED` | Keep the original ID. Inspect `reconcile` and follow its operator action |
| Transient | `SCREENING_UNAVAILABLE`, `PROVER_UNAVAILABLE`, `PROOF_EXPIRED`, `SUBMIT_FAILED` | Retry with backoff. `PROOF_EXPIRED` needs a fresh proof, not a resend |
| Terminal | `SCREENING_REJECTED` | Stop. Not transient |
| Opaque | `PROOF_FAILED` | The prover refused and gave no reason. Report it as unexplained |
| Before any protocol code ran | `INVALID_REQUEST`, `IDENTITY_UNAVAILABLE` | Fix the request or the key path. Never a chain-state problem |

The Rust CLI, Python binding, and MCP adapter preserve the Protocol 4 funding and recovery
codes. Use `reconcile` before you classify an interrupted or uncertain write.

A write takes 1–4 minutes. The binary prints stage names to stderr. **Do not create a new ID
for a write that appears stuck.** Call `reconcile`. Then use `resume_operation` with the
original ID only when the result permits it.

## The CLI protocol

`erebus-cli` reads one JSON request on stdin and writes one envelope on stdout. Key *paths*
cross the boundary; key values never do.

```bash
echo '{"method":"doctor","params":{"config":{...}}}' | erebus-cli
```

```json
{"ok": true, "protocol": 4, "result": {"ready": true, "checks": [...]}}
```

`protocol` is the contract version. A consumer should refuse a mismatch by name rather than
failing on a changed shape later — `erebus-sdk` does this on every call, and the MCP server
handshakes at startup.

Methods: `version`, `generate_pool_key`, `doctor`, `balance`, `allowance`, `approve`,
`shield`, `open_channel`, `propose_offer`, `counter_offer`, `read_channel_state`,
`accept_and_settle`, `reconcile`, `resume_operation`, `rebuild_state`,
`grant_viewing_key`, and `reveal`. All except `version` and `generate_pool_key` take a
`config` object. Every chain write also takes `operation_id`.

From Python, `erebus-sdk` wraps this:

```python
from erebus import Seam, SeamConfig
seam = Seam(config=SeamConfig(rpc_url=..., prover_url=..., ...))
report = seam.doctor()
```

`/sdk/py` is a binding, not a client: it marshals arguments and returns results, and
contains no hashing, salt encoding, or felt arithmetic. A second implementation would be a
second place for a wrong preimage to hide silently.

## Building from source

```bash
git clone https://github.com/PoulavBhowmick03/Erebus && cd Erebus
cd sdk/rs && cargo test --all-targets && cd ../.. # 351 passed, 2 ignored
uv sync --all-packages && uv run pytest          # 154 tests
```

`uv sync` without `--all-packages` skips the workspace members' editable installs and the
`erebus-*` packages will not be importable.

The TypeScript SDK is a differential-test oracle and ships nothing; it needs a sibling
checkout of `starkware-libs/starknet-privacy` (see [friction.md](./friction.md)
F8). Toolchain: scarb 2.17.0 / starknet-foundry 0.59.0, Node 20+, Rust stable.

---
