# Runbook: clean-machine operator guide

This is the current `main` guide for reproducing the documented operator flow from a clean
shell. It is written for the Protocol 4 source tree and the published `v0.2.0` artifacts.

This guide follows the proof lifecycle used throughout the repository:

- provision and inspect the identity;
- approve the pool allowance;
- wait for the approval to reach proving depth;
- shield a note to register the identity;
- negotiate through MCP;
- settle atomically;
- reconcile and resume only when the journal says to;
- inspect observer output;
- disclose one deal to a registered recipient;
- shut down cleanly.

Privacy wording here is bounded to what the evidence shows. Erebus hides the terms, not the
relationship. The counterparty address is public at channel-open, the submitter is public,
and the prover plus preflight RPC both see the pool private key during writes.

For privacy scope and the known leaks, read [privacy-model.md](./privacy-model.md) first.
For the implementation and trust boundaries, read [custody-design.md](./custody-design.md)
and [local-prover.md](./local-prover.md).

## Clean shell

Start from a fresh shell with no exported overrides unless a step says otherwise. Do not
reuse a shell that has stale `EREBUS_*`, `RPC`, `CLI`, or `REQ` values in it.

```bash
export REPO=~/Developer/erebus
```

If you need a local build, build `erebus-cli` first:

```bash
cd "$REPO/sdk/rs"
cargo build --bin erebus-cli
```

Then set the helper paths:

```bash
export CLI="$REPO/sdk/rs/target/debug/erebus-cli"
export REQ="$REPO/scripts/erebus-request.py"
```

## 1. Install

Install the current checkout from source when you need unreleased changes. The published
`v0.2.0` wheel speaks Protocol 4 and supports reconciliation, resume, and state rebuild.

```bash
cd "$REPO"
uv sync --all-packages
uv run pytest
```

If you only want the release artifacts, follow the README install section instead. This
runbook assumes source-built current main.

## 2. Create an identity

Each identity has:

- a Starknet account;
- an account signing key file;
- a pool identity key file;
- a state directory.

Use the repository helper for a clean-machine bootstrap:

```bash
scripts/new-identity.sh bootstrap erebus-a ~/.erebus-a <funder-account>
```

That script performs:

- account creation;
- funding;
- account deployment;
- pool key generation;
- account-key extraction;
- env-file creation;
- fee-aware allowance approval;
- proving-depth wait;
- one shield, which also registers the identity;
- `doctor`.

If you need the faucet path instead, use the `create` and `activate` phases from the script
header.

## 3. Verify the identity

Run `doctor` before any write. It is read-only and checks the setup in dependency order.

```bash
python3 "$REQ" "$REPO/.env" doctor '{}' | "$CLI"
```

If `doctor` is not ready, fix the named `repair` items before continuing.

## 4. Hosted prover

For the mainnet path documented in the repository, use the hosted prover configured by the
identity env file. The hosted path is still inside the pool-key trust boundary because the
prover and preflight RPC both receive the pool private key.

The relevant env value is:

```bash
PROVING_SERVICE_URL=https://api.starkscan.co/v1/SN_MAIN/prove
```

If you use Starkscan, the env must also include `STARKSCAN_API_KEY` with `prove` scope.
See [local-prover.md](./local-prover.md) for the local fallback and its screening limits.

## 5. Shield and register

Shielding funds a note and registers the identity. The shield transaction is the first write
that also makes the identity available as a channel counterparty.

```bash
ENV=~/.erebus-a/env
python3 "$REQ" "$ENV" shield '{"amount":"1000000000000000000"}' | "$CLI"
```

If the approval is too fresh, the shield can fail because the proof is built against a
historical block. Wait for the approval to reach proving depth with:

```bash
bash "$REPO/scripts/wait-for-depth.sh" 0x_TRANSACTION_HASH
```

## 6. Negotiate through MCP

Run one server per identity and bind the role explicitly:

```bash
scripts/erebus-mcp.sh ~/.erebus-a/env payer
scripts/erebus-mcp.sh ~/.erebus-b/env payee
```

For an MCP client, the tool surface is:

- `doctor`
- `get_note_balance`
- `open_channel`
- `propose_offer`
- `counter_offer`
- `read_channel_state`
- `wait_for_offers`
- `accept_and_settle`
- `reconcile`
- `resume_operation`
- `rebuild_state`
- `grant_viewing_key`
- `reveal`

The payer must call `get_note_balance` before naming a price. The payee must not call
`accept_and_settle`. Every write needs an `operation_id` and the caller should persist the
canonical intent before the call.

Example shell flow:

```bash
scripts/agent.sh ~/.erebus-a/env balance
scripts/agent.sh ~/.erebus-a/env open "$(scripts/agent.sh ~/.erebus-b/env whoami)"
scripts/agent.sh ~/.erebus-b/env wait <channel-handle> 1
scripts/agent.sh ~/.erebus-b/env counter <channel-handle> <offer-id> 600000000000000000
scripts/agent.sh ~/.erebus-a/env accept <channel-handle> <offer-id>
```

The exact offer amounts, deadlines, and memos belong in the agent policy or the clean-shell
record you are building. The important point for the guide is the lifecycle, not a single
example price.

## 7. Settle

Settlement is atomic. It consumes the payer's notes, returns change if needed, and keeps the
channel pair available for later deals.

The relevant MCP call is `accept_and_settle`. The documented result includes:

- `tx_hash`
- `nullifiers`
- `proved_at`
- `selected_input`
- `change`

## 8. Recover

Recovery is explicit and operator-driven.

- Use `reconcile` first.
- Use `resume_operation` only when the classification says the operation is resumable.
- Keep the original `operation_id`.
- Do not mint a replacement ID for an uncertain write.

The repository’s recovery model distinguishes:

- no effect yet, safe to retry;
- effect exists, commit local state;
- effect exists, commit the journal;
- ambiguous, needs operator attention.

If the result says the proof expired, rebuild the proof under the same operation ID only
when the journal and classification permit it.

## 9. Observer test

Run the observer against the public chain record and compare it with the authorized reveal.
The observer should not recover the deal terms from wire v3.

The guide should treat this as a negative control:

- public observer output;
- authorized disclosure output;
- explicit note that public calldata still reveals the relationship and timing.

## 10. Disclosure

Use recipient-bound disclosure for one deal and one registered recipient. The raw CLI and
the MCP wrapper differ:

- CLI `grant_viewing_key` returns a grant object.
- CLI `reveal` consumes that grant object.
- MCP `grant_viewing_key` writes the grant to a file.
- MCP `reveal` reads it back from a file.

The disclosure path should say:

- the grant is scoped to one deal;
- the grant has an expiry;
- the grant gives no spending authority;
- opening a disclosure does not erase what was already disclosed;
- the recipient’s pool key must match the grant.

## 11. Shutdown

When the run is complete:

- stop the MCP server process;
- delete any temporary grant files you created for the test;
- remove any temporary capture files or proxy files;
- keep the journal and evidence records that prove what happened.

## 12. What to record

For the clean-shell validation pass, record:

- the exact command;
- whether it succeeded;
- the first failure if it did not;
- any prerequisite that was not obvious from the docs;
- any command that needed a retry because of depth, timing, or environment.

Do not hide missing prerequisites. If a command needs a secret, endpoint, or funded account,
say so directly.

## Reference order

Use this guide together with:

- [README.md](../README.md)
- [reference.md](./reference.md)
- [status.md](./status.md)
- [privacy-model.md](./privacy-model.md)
- [local-prover.md](./local-prover.md)
- [custody-design.md](./custody-design.md)
