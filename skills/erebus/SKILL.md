---
name: erebus
description: Operate Erebus, private agent-to-agent negotiation and shielded settlement on Starknet, through its MCP server. Use when installing, configuring, running, or diagnosing an Erebus MCP identity; negotiating an offer as payer or payee; settling, granting, or reading disclosure; interpreting a settlement error or a doctor report; or explaining what Erebus does and does not hide. Covers install, plan, operate, and diagnose modes for one identity's MCP server.
---

# Erebus operator skill

Erebus lets two agents negotiate a deal privately and settle it atomically through
Starknet's STRK20 shielded pool. This skill operates one already-configured MCP identity —
it does not write Erebus's own code, and it does not integrate STRK20 into someone else's
app (that is a different skill; this one is scoped to running Erebus itself).

**Tone.** Be concise. Lead with the result, not the reasoning. Every result you report
carries a mock/Sepolia/mainnet label — see "Label every result" below, non-negotiable.

## Hard rules — read before doing anything

These are load-bearing. Breaking any one of them is a safety failure, not a style choice.

1. **Never read, print, or ask for the contents of a private-key file.** `POOL_KEY_FILE` and
   `ACCOUNT_KEY_FILE` are paths. The MCP server never opens them — only the Rust binary
   does, for the duration of one call (ARCHITECTURE.md §3). This skill inherits that
   boundary: read a key's *path* to confirm it's configured, never its bytes.
2. **A payee identity never calls `accept_and_settle`.** The server already refuses it
   structurally (`INVALID_REQUEST`, "configured as payee"), but this skill's own procedures
   must never suggest, script, or work around that call for a payee. A payee's move is
   always to counter with its final offer and wait for the payer to accept.
3. **Never report a mock result as on-chain evidence.** `EREBUS_BACKEND=mock` produces no
   transaction, no nullifier, no chain state. If a result came from the mock, say so in the
   same sentence as the result — not in a footnote.
4. **Content privacy and traffic privacy are two different claims — report them
   separately.** Negotiation terms and settlement amounts are private (message privacy).
   That a channel was opened, with whom, and roughly how often, is not (F38, F31). Never
   collapse these into one "it's private" statement. See "Privacy wording" below.

## Modes

### Install

Before doing anything else, establish what's actually on this machine:

- Which `erebus-mcp-server` is installed — packaged (`erebus-mcp-server` on `PATH`) or a
  checkout (`uv run python -m erebus_mcp.server` from a cloned repo)? `shutil.which
  erebus-mcp-server` or `uv run which erebus-mcp-server` answers this.
- Is `erebus-cli` on `PATH`? The Python binding resolves it with `shutil.which`; the seam
  backend cannot run without it.
- Read the identity's env file (never its key files) to confirm `AGENT_ADDRESS`,
  `PROVING_SERVICE_URL`, and either `EREBUS_BACKEND=mock` or the full seam config are
  present. Config validates all of this at startup and fails loudly if it's missing — if
  the server won't start, this is the first thing to check, not the prover.
- Never assume a version. `erebus-cli version` (or the seam's version handshake the server
  already runs at startup) tells you the protocol number; a stale binary against a newer
  server fails by name, not by a confusing type error mid-call.

Do not proceed to Operate mode on an installation you haven't checked.

### Plan

Before naming or accepting a price, establish the shape of the deal:

- **Which role is this identity?** `EREBUS_SETTLEMENT_ROLE` is `payer`, `payee`, or `both`.
  A payer spends its own notes on `accept_and_settle`; a payee never does (see Hard Rule 2).
- **Payer: call `get_note_balance` before naming anything.** Since the change-note fix, any
  amount `0 < amount <= total` is payable — settlement covers the price and returns the
  excess as a new note. There's no narrower "exact subset" question to ask.
- **Payee: decide a reserve, not an opening ask.** The payee never accepts; it counters at
  its floor and leaves the final payer-authored offer for the payer to accept. Confirming
  agreement *is* countering at the agreed amount.
- **Pick a deadline deliberately.** An offer has no `withdrawn` state — it's either accepted
  or it expires (ARCHITECTURE.md §4). A short deadline is the only way to bound how long a
  stale offer stays acceptable.

### Operate

1. **Run `doctor` before every funded workflow — not just at startup.** Startup's check is
   a snapshot; a proof-bearing call can be minutes later, and mode bits, allowance, or RPC
   health can change in between. `doctor` is read-only and always safe to call again.
   `ready: false` means a write will fail now — read each unhealthy check's `repair` field
   and act on it before attempting the write, not after it fails.
2. **Payer negotiation:** `open_channel(counterparty)` → `propose_offer` or wait for the
   payee's offer via `wait_for_offers`/`read_channel_state` → when an offer is acceptable
   (`amount <= total` per Plan mode), `accept_and_settle(handle, offer_id)`.
3. **Payee negotiation:** `open_channel(counterparty)` → read the payer's offer →
   `counter_offer` at the reserve (or accept the payer's price by countering at that same
   amount) → wait. Never call `accept_and_settle`.
4. **Structured failures, by what to do about them** (`SettlementErrorCode`, grouped in
   `interface.py`, not by individual code — an agent branches on the group):
   - *Don't retry, the offer is wrong:* `OFFER_EXPIRED`, `OFFER_UNKNOWN`,
     `ALREADY_SETTLED`, `NOT_YOUR_OFFER`, `AMOUNT_MISMATCH`, `INSUFFICIENT_NOTES`,
     `INDEX_CONFLICT`. Build a different offer or call `get_note_balance` again; retrying
     the same call verbatim will not help.
   - *Retry may succeed:* `SCREENING_UNAVAILABLE`, `PROVER_UNAVAILABLE`, `PROOF_EXPIRED`,
     `SUBMIT_FAILED`. Safe to retry with backoff; a proof is valid for 450 blocks so
     `PROOF_EXPIRED` specifically means the retry needs a fresh proof, not just a resend.
   - *Terminal for this counterparty or deposit:* `SCREENING_REJECTED`. Stop; this is not a
     transient condition.
   - *Opaque:* `PROOF_FAILED` — the prover refused and gave no reason (JSON-RPC -32603
     carries none). Report it as unexplained, don't invent a cause.
   - *Seam-level, before any protocol code ran:* `INVALID_REQUEST`, `IDENTITY_UNAVAILABLE`.
     These fail before a proof is attempted — cheap to retry after fixing the request or the
     key path, never a chain-state problem.
   - *MCP-layer, before the seam is ever called:* `SPENDING_LIMIT_EXCEEDED`. An
     operator-configured cap, not a protocol failure. Do not retry at a smaller amount to
     route around it — a daily cumulative cap catches that. Stop and tell the operator.
   Every error carries `retryable: bool` — trust it over guessing from the code name.
5. **Disclosure.** `grant_viewing_key(handle, grantee, export_path)` writes a bearer grant
   to `export_path` on this machine, mode 0600, and never returns it — the tool result only
   confirms the write. Whoever holds the file can reconstruct the whole relationship
   (ARCHITECTURE.md §3, "disclosure is the intentional exception"). Deliver the file to the
   grantee through an out-of-band channel; this tool does not do that. Never paste the file's
   contents into a chat transcript, a log line, or any output the grantee didn't specifically
   ask to receive. `reveal(viewing_key)` needs no prior local state; it works from the grant
   alone, on any machine.
6. **Evidence capture.** After a real (non-mock) settlement, record `tx_hash`, every
   `nullifier`, and — if the backend is Sepolia or mainnet — the Voyager link for that
   transaction on the matching network. Don't guess the URL format from memory; look up
   Voyager's current URL convention for the network in question rather than fabricating one
   — a wrong link is worse than no link. Do this capture immediately; the receipt is not
   retrievable again from `accept_and_settle`.

### Diagnose

Read a `doctor` report top to bottom, in the order the checks ran. `ready: false` doesn't
mean everything failed — most checks are usually `pass`. For each check that isn't `pass`,
report its `status`, `detail` (what was actually observed), and `repair` (the one direct
action) together, not the status alone. A `skipped` check is not evidence of health; it
means the thing it would have verified is unverified — report it as unknown, not as fine
(this mirrors `payment_consistency`'s `unknown` state: absence of a check is not evidence of
a pass).

## Privacy wording

Quote [`docs/privacy-model.md`](../../docs/privacy-model.md) and nothing else for privacy
claims — it is this repository's single canonical source, kept current on purpose so no
other document has to restate it. The one sentence to reuse verbatim when a short answer is
needed: **"Erebus hides the terms, not the relationship."**

Never say "Erebus is private" without qualification. Always split the claim in two:

- **Hidden:** negotiation content (amount, token, deadline, memo hash, message type,
  `replyTo`), and the settlement amount and recipient.
- **Not hidden:** that a channel was opened, and — as of F38 — the counterparty's address,
  written to public calldata at `open_channel` itself. Also not hidden: pool-interaction
  timing and frequency, and (F31) a fixed fifth-salt shape that lets an observer count and
  time Erebus traffic without reading it.

If asked "is this private," the honest answer names both halves, not just the first.

## Label every result

Every result you report — a balance, an offer, a settlement receipt, a disclosed record —
carries one of three labels, stated in the same sentence as the result:

- **mock** — `EREBUS_BACKEND=mock`. No chain, no transaction, no real value moved.
- **Sepolia** — a real testnet transaction. Real proof, real nullifiers, worth nothing.
- **mainnet** — real value moved. As of this writing Erebus has not run on mainnet; do not
  produce a mainnet-labeled result that didn't actually happen on mainnet.

A result with no label is a bug in the report, not a stylistic omission — fix it before
sending the result on.

## Evaluation fixtures

See [`evals/unsafe-behavior.md`](./evals/unsafe-behavior.md) for the scenarios this skill
must be run against before it's trusted: key-file reads, payee-settlement attempts, and
mock-as-evidence claims.

## Related

- [ARCHITECTURE.md](../../ARCHITECTURE.md) — the frozen interface, the seam, the trust
  boundary.
- [docs/privacy-model.md](../../docs/privacy-model.md) — the only source for privacy claims.
- [docs/runbook.md](../../docs/runbook.md) — the seven-step reproducible walkthrough this
  skill's Operate mode summarizes.
- [mcp-server/README.md](../../mcp-server/README.md) — how to actually launch a server for
  one identity.
