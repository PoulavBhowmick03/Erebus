# Metropolis M6 Runbook: Relayer, Relay, Indexer, and RPC Failures

Scope: operating the M6 services on testnet. Every diagnostic here is redacted by construction:
no step asks for a spending key, a proof witness, an agreement opening, a signature, or a
transcript. If a procedure seems to need one of those, stop and escalate instead.

Related: [M6 decisions](metropolis-m6-decisions.md) DM6-8 and DM6-9,
[M6 progress](metropolis-m6-progress.md), [threat model](metropolis-threat-model.md) section 6.

## Ground rules

1. **Never delete a state directory to unblock an operation.** The coordinator snapshot and the
   signer journal are the only record that a payment may exist. Losing them turns a recoverable
   operation into an unrecoverable one. Back up the whole directory, including the initialization
   marker.
2. **Never treat a local timeout as non-payment.** A dropped broadcast response is recorded as
   `Unknown` and every reservation stays held. Resolve it by reconciling against the chain.
3. **Never export state as diagnostics.** `Diagnostic` values contain operation IDs, stages,
   attempt counts, and response categories only. Snapshots contain private terms and
   authorizations and are not safe to upload.
4. **One state root per buyer, one signer journal per chain and gas account, shared by every
   process using that account.** Two roots mean two policies and two nonce allocators.

## A stalled operation

Symptoms: `diagnostics()` reports a stage that does not advance, `unknown_attempts > 0`, or a
signer journal that returns `Busy`.

1. Read `diagnostics()`: stage, `broadcast_attempts`, `unknown_attempts`,
   `last_broadcast_outcome`, `replacements`.
2. Stage `BroadcastUnknown` or `Signed` with attempts means a submission may have reached the
   network. Do not resubmit by hand. Reconcile:
   `EvmChain::finalized_deal_evidence_resumable_agreed(history_journal, peer, peer_history, deal_nullifier, budget)`.
   On `Complete`, pass its evidence to `Coordinator::reconcile`.
   On `Pending`, retain reservations and repeat with the same checkpoint directory.
3. `Unknown` evidence (observation error) holds every reservation. Fix the provider and read
   again; do not interpret an RPC failure as absence.
4. Stage `Finalized` with a `Committed` reservation is done. Release the signer claim only from
   `verified_finalized_nonce_agreed`, never from a local expiry.
5. A signer stuck `Busy` on an unconsumed nonce needs a replacement or a cancellation signed by
   that account. Do not delete the journal.

## Transaction relayer (`erebus-tx-relayer`)

Use `erebus-tx-relayer --serve` for newline-delimited JSON requests in one process.
Each input line produces one response line. EOF stops the process. Rate-limit windows and
counters persist across requests, but reset on restart. The host supplies `EREBUS_RELAYER_CLIENT`;
this mode does not authenticate remote clients. Without `--serve`, the CLI handles one request.

Requests are size-limited before JSON parsing. An oversized frame terminates the process.
Malformed JSON returns an error without terminating `--serve` mode.

The CLI requires `EREBUS_RELAYER_STATE_ROOT` for `relay` and `recover`. The directory is
relayer-owned; never point it at the buyer's wallet or coordinator. All processes using
the gas account must share it. Use a dedicated gas account and back up the whole root.
It contains authorized public-bound terms and signatures; do not upload it as diagnostics.

Set `EREBUS_RELAYER_VERIFICATION_RPC_URL` to an independently operated RPC for the same deployment.
It must differ from every endpoint in `EREBUS_RELAYER_RPC_URLS` after URL normalization.
Those endpoints handle broadcast and failover. The designated verifier also checks recovery evidence.
If either selected provider fails or disagrees, retain reservations and repair the provider.
There is no automatic single-provider fallback. Identical anchors are required; provider lag can delay recovery.
Distinct URLs do not establish independence, honesty, or consensus.

Set `EREBUS_RELAYER_MAX_FEE_PER_GAS`, `EREBUS_RELAYER_PRIORITY_FEE_PER_GAS` (wei), and
`EREBUS_RELAYER_GAS_LIMIT` explicitly. `EREBUS_RELAYER_RPC_TIMEOUT_SECONDS` defaults to 15.
Funding must cover the configured gas limit at the maximum fee cap before a new nonce
claim is created. Persisted signing plans survive fee-cap changes across restarts.

History work uses three optional nonzero budgets:

| Variable | Default | Per-call work |
|---|---|---|
| `EREBUS_RELAYER_LOG_BLOCK_RANGE` | 2000 | Blocks per log query |
| `EREBUS_RELAYER_LOG_QUERIES` | 1024 | Log queries |
| `EREBUS_RELAYER_ANCESTRY_LINKS` | 8192 | Parent links |

The relayer saves separate observer cursors under `STATE_ROOT/history`. Back up the whole root.
If recovery reports `relayer history scan pending`, repeat `recover` with the original request.
Pending history does not allocate a nonce, sign, submit, or release capacity.
After recovery returns `Prepared`, use `relay` to submit. Recovery itself never submits.
If a checkpoint is malformed or a finalized anchor changes, stop and diagnose the cache or provider.
Do not remove financial state or treat incomplete history as nonpayment.

Shielded observation also yields `HistoryPending` after each incomplete batch.
Repeat with the same `IndexStore`. The default batch is at most 1,000 new blocks.
`ShieldedChain::reconcile` requires both chain observers and separate public index caches.
It compares both finalized wallet histories before changing notes, accounting, or signer claims.
Network budgets do not limit local cache replay. The shielded cache rejects files above 128 MiB.
An oversized cache requires an index-storage change; repeated observation cannot bypass that limit.

Send `{"method":"relay","evidence":"<original-authorized-evidence-hex>"}` to persist
and submit. Send `{"method":"recover","evidence":"<original-authorized-evidence-hex>"}`
to observe an existing operation after restart or endpoint switching. Keep the original
request in the client's own durable state. Recovery never signs or broadcasts, works after
expiry, and does not require the current fee schedule to match the original request.

`status:ok` means the request succeeded, not that a payment finalized. Inspect `stage`:
`Submitted` and `BroadcastUnknown` retain uncertainty. Only `Finalized` confirms payment.
`ClosedUnpaid` may still report `gas_account_busy:true`: an unused nonce claim requires
account-signed replacement or cancellation, not journal deletion. A local timeout never
closes a deal. The former `EvmSettlementBackend::submit`, `settle`, `verify`, and
`with_confirmations` methods have been removed. Confirmation counts cannot finalize payment.

For SDK callers migrating from M3:

1. Open the coordinator with the selected settlement context and shared buyer state.
2. Persist intent, reserve policy capacity, and record both authorizations through the coordinator.
3. Prepare with `EvmSettlementBackend::prepare`, reserve the gas account nonce through
   `SignerJournal`, and persist the signing plan and signed bytes with `sign_transaction`.
4. Send through `EvmChain::broadcast_journaled` or the journaled relayer path.
5. Read `finalized_deal_evidence_resumable_agreed` with two providers and separate history stores.
   Pending scans or errors hold reservations; do not infer payment from a broadcast response.
6. Reconcile complete evidence through the coordinator. Release the signer claim only from
   `verified_finalized_nonce_agreed` showing that its nonce was consumed.

The migrated tests in `sdk/evm/tests/local_chain.rs` demonstrate this sequence. Low-level
chain primitives remain available, but do not create a journal or recovery workflow for you.

| Symptom | Diagnosis | Action |
|---|---|---|
| `{"status":"error","error":"relayer access limit exceeded"}` | `{"method":"health"}` shows `rate_limited` rising | Raise `EREBUS_RELAYER_MAX_REQUESTS`/`WINDOW_SECONDS`, or throttle the client |
| `{"method":"funding"}` reports `funded:false` with a `shortfall` | The gas payer is empty | Fund the relayer account; re-run `funding` before retrying |
| Submission fails on every provider (`submit_failed` rising) | One or more `EREBUS_RELAYER_RPC_URLS` are down | The service already tries the next provider in order; replace the dead URL and retry the same admitted agreement |
| `"agreement does not match relayer fee policy"` | The deal was signed with a different fee or recipient | Expected: the relayer cannot admit it. The buyer must re-authorize against the published schedule |
| `"agreement outside relayer lifetime window"` | Expiry is past, or further away than `EREBUS_RELAYER_MAX_LIFETIME` | Expected. Do not widen the window without recording the reason |

The relayer never releases the buyer's reservation. A relayer outage leaves the operation
`Unknown` and recoverable; it does not make the deal unpaid.

Funding diagnostics try the configured providers in order. Each provider has a 15-second
deadline; a failed or timed-out read tries the next endpoint. A successful read checks
the live chain ID before and after work. If every endpoint fails, funding is unavailable,
not sufficient. These reads do not submit transactions. Library callers can configure
the deadline with `RelayService::funding_with_timeout`.

## Message relay (`erebus-relay`)

| Symptom | Diagnosis | Action |
|---|---|---|
| `GET /healthz` fails | Process or listener is down | Restart; clients retain their own transcripts and re-handshake |
| `401` on mailbox access | Wrong or missing `EREBUS_RELAY_TOKEN` | Fix the token; do not disable auth outside a local run |
| `507` on `POST /v1/mailbox/{id}` | Mailbox reached its blob bound | Clients resume from their cursor; the relay never truncates |
| Blobs missing after the retention window | Retention is 7 days by default | Clients reconstruct from their durable transcript; the relay is not archival |

The relay sees mailbox ids, sizes, and timing, never plaintext. Do not add payload logging to
diagnose it.

## Pool indexer (`erebus-pool-indexer`)

| Symptom | Diagnosis | Action |
|---|---|---|
| `/healthz` reports a `last_error` | The upstream RPC failed during sync | Fix or switch the RPC; the next sync attempt resumes from the stored cursor |
| Served blocks disagree with the client's RPC | Reorg or stale cache | The client verifies blocks against its own anchor and rewinds. Rebuild the cache from the deployment block if the store is suspect |
| Endpoint switched | New host, empty cache | Sync from the deployment block; wallet openings are local and unaffected |

The indexer never accepts note openings and cannot spend. Its outage delays discovery only.

## RPC failure playbook

The operator default for one versus two providers is not yet selected. The existing
single-provider APIs trust the configured RPC after internal consistency checks.
For opt-in paired recovery, public-bound callers can use
`EvmChain::finalized_deal_evidence_agreed` and
`EvmChain::verified_finalized_nonce_agreed` before applying accounting or releasing
signer claims. Shielded callers use `ShieldedChain::reconcile_agreed`, with separate
index caches and matching deployment identities for both providers.

The paired path requires identical pinned head and finalized anchors. Different
heights, timeout, or disagreement must hold reservations; do not bypass the check
by interpreting failure as an unpaid deal. Independently operated providers reduce
shared failure risk, but two URLs alone establish neither independence nor honesty.

1. **Chain check first.** Every adapter call checks `eth_chainId` before and after work; a
   mismatch is a stop, not a retry against the wrong chain.
2. **`finalized` unavailable** is an error (`FinalityUnavailable`). Do not fall back to a
   confirmation count; fix the provider.
3. **Dropped or falsified responses** produce `Unknown` outcomes and failed observations. The
   reservation and the nonce claim stay held. Reconcile from a trusted provider before any
   resubmission.
4. **Reorg:** an included-but-not-final settlement may still be reorganized out. Only
   `finalized` evidence commits or releases. If a previously final anchor changes, treat the
   provider as untrusted and escalate.

## Escalation checklist

Collect, without exporting secrets:

- the `Diagnostic` list for the operation, and the signer journal's `Busy`/`NotConsumed` status;
- the transaction hash from `signed_transaction` if one was broadcast;
- the `finalized` block number and hash the provider reports, and whether `eth_chainId` matches;
- relayer `health` counters and the current `funding` report;
- relay/indexer `/healthz` output.

Do not attach a coordinator snapshot, a wallet file, a transcript, or an agreement opening to an
issue or a chat. Those are the private material the protocol exists to protect.
