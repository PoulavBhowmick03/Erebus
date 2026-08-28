You are auditing **Erebus** (`~/Developer/erebus`, branch `main`), which just shipped
`v0.1.0`. I need an independent end-to-end audit: does this actually work, and what is
quietly wrong?

Treat me as a peer, not a client. Do not soften findings, and do not pad the report with
things that are fine. If the codebase is in better shape than I think, say so plainly and
show what you checked.

## What Erebus is

Private coordination and settlement between two AI agents on Starknet. Two agents open an
encrypted channel, negotiate as structured state transitions carried in privacy-pool note
salts, and settle atomically through StarkWare's STRK20 shielded pool. A third party holding
a viewing grant can reconstruct the record afterwards.

Call path: `agents → mcp-server → sdk/py → sdk/rs → Starknet`. Python above the binding,
Rust below it. `/sdk/rs` owns all protocol logic and is the only holder of key material;
`/sdk/py` is a transport-only binding; `/sdk/ts` is a differential-test oracle that ships
nothing. Read `CLAUDE.md` and `ARCHITECTURE.md` first — they carry non-negotiable
constraints, and violating one is a finding.

## Why this audit matters more than usual

**Every failure mode in this protocol is silent.** A wrong hash preimage derives a storage
slot nobody wrote to, and the note is simply "not found" — no error, anywhere. There is no
written wire specification. Two implementations agreeing on the same Cairo vectors is the
strongest correctness signal available, and `/sdk/ts` is currently on wire v1 while
`/sdk/rs` is on v2, so that check is not currently running.

Assume defects are silent, not loud. A green test suite is weak evidence here.

## Bug classes actually found in the last 72 hours

These are the shapes I want you hunting for, because each one shipped, passed CI, and was
found by accident rather than by a test:

1. **A `u128` serialized as a JSON number.** `OfferTerms.amount`/`memo_hash` crossed the CLI
   boundary as bare JSON integers. serde_json refuses anything above `u64::MAX`, so the
   first live offer carrying a real 128-bit digest tail made *every read of that channel
   fail permanently* — the note was already on-chain and unfixable. Latent twin: amounts
   above ~18.4 tokens at 1e18 base units. **Hunt for every remaining numeric type that
   crosses a process or language boundary and can exceed 2^53 (JSON safe integer) or 2^64.**

   *Update 2026-08-22:* the twin was real and has been fixed — `ApprovalReceipt.approved`
   (F39) and `DisclosedSettlement.{agreed_amount,paid_amount}` (F40). Both now use the
   `u128_boundary` helpers, verified on Sepolia with a 60 STRK approve and a 19 STRK
   settlement. A grep found five structs holding `u128` fields and exactly those two reached
   `serde_json` through a derive; `AllowanceReport` and `NoteBalance` are correct only
   because `erebus_cli.rs` reshapes them by hand, so the invariant lives in a call site
   rather than in a type. **That is the part still worth attacking:** find the next call
   site that forgets, and say whether this should be a type rather than a convention.
2. **An error labeled as the wrong thing.** Output-serialization failures were wrapped in
   `CliError::BadRequest` and reported as "request is not valid JSON", sending diagnosis
   into the request parser when the request was blameless. **Hunt for error paths whose
   label contradicts their cause.**
3. **Version strings that drift.** `erebus.__version__` read `0.0.0` through three version
   bumps and shipped wrong in the v0.1.0 wheels, because nothing fails when it drifts.
   **Hunt for any constant duplicated between a manifest and code.**
4. **A documented command that does not work.** The README's install command
   (`uv tool install erebus-mcp-server`) exposes only the server's executable, not
   `erebus-cli`, while the config *required* `EREBUS_CLI` — so the documented install
   produced a server that could not start. **Execute every documented command.**
5. **A stale long-running process against a new binary.** A running MCP server outliving a
   CLI upgrade produced a Python `TypeError` mid-tool-call rather than a version mismatch.
6. **A file deleted by accident in a commit** (`docs/status.md`, 133 lines, the declared
   tiebreaker document) because nothing tests documentation.

## What I want you to do

Verify by execution wherever you can. Reading is necessary but not sufficient.

### 1. Does it build and pass its own gate?
```
cd sdk/rs && cargo test && cargo clippy --all-targets -- -D warnings \
  && RUSTDOCFLAGS='-D warnings' cargo doc --no-deps && cargo fmt --check
cd ../.. && uv sync --all-packages && uv run pytest
```
Expected as of 2026-08-28: 351 Rust passed, two live-prover Rust tests ignored, and 154
Python tests passed. If the counts differ from `docs/status.md`, report a finding.

### 2. Does the published release actually work?
Install `erebus-mcp-server` from the public index
(`https://poulavbhowmick03.github.io/Erebus/simple`) into a clean environment — not from the
checkout — and drive a real MCP tool call through it. Note: release assets can be slow;
use `UV_HTTP_TIMEOUT=600`. Verify `SHA256SUMS` against the actual artifacts, and check that
what the wheels report as their version matches the tag.

### 3. Boundary-crossing data
Trace every type that crosses Rust → CLI JSON → Python seam → MCP → agent. For each, ask:
can it exceed 2^53? 2^64? Is it a string on one side and a number on the other? Is there a
known-answer test pinning it at full width? `sdk/rs/src/client.rs` (`u128_boundary`),
`sdk/rs/src/bin/erebus_cli.rs`, `sdk/py/src/erebus/_seam.py`, and
`mcp-server/src/erebus_mcp/{seam_client,interface,tools}.py` are the path.

### 4. Protocol invariants
From `CLAUDE.md`, these must hold and are silent when broken:
- `__execute__` is never called on-chain; every state change goes through `apply_actions`
  with a proof. No code path skips proof generation.
- Note indices within a channel/subchannel are contiguous — never skipped or reordered.
- Salt types are **not** uniform across encryption hash functions (an unresolved upstream
  audit finding). Verify each call site rather than assuming.
- Key material never crosses above `/sdk/rs`. Python handles key *paths* only.
- `/sdk/py` contains no protocol logic. **Tripwire: if any test there asserts a computed
  value, the package has become a third implementation.**

### 5. Concurrency and failure
There is no operation journal and no crash recovery yet (that is planned work, not a
finding in itself). What I want to know is the blast radius: what happens if a process dies
between proof generation and submission, or between submission and state commit? Can a
retry double-spend or corrupt channel state? `sdk/rs/src/state.rs` holds the locking, and
`sdk/rs/src/execution.rs` the write pipeline. Note that one channel per address pair and one
deal per channel means a corrupted channel is *permanently* unusable.

### 6. Documentation truth
`docs/status.md` declares itself the tiebreaker. Check every factual claim in `README.md`,
`docs/reference.md`, and `docs/status.md` against the source: tool signatures, environment
variable names, CLI method names, error codes, test counts, supported platforms. Run the
commands. Follow the links.

### 7. Security posture
Not a formal review — flag what stands out. Key handling and file modes, whether any secret
can reach a log or an MCP transcript (viewing grants are bearer secrets), the subprocess
boundary, and anything in CI that could leak. `docs/custody-design.md` and
`docs/privacy-model.md` state the intended boundaries; tell me where the code disagrees with
them.

## What I do not want

- Style opinions, formatting nits, or "consider adding more tests" without a specific
  failure that test would catch.
- Findings about planned work already tracked in `docs/roadmap.md` §7 Phase 7+ (crash
  recovery, wire padding, repeat deals, external audit). Assume I know. The exception is if
  you think one is *more urgent* than the roadmap treats it — say that and why.
- Speculation presented as fact. If you infer something without running it, label it as an
  inference and name the check that would settle it.

## Output

Rank strictly by "what breaks in production, silently, first." For each finding:

1. **What is wrong** — one sentence.
2. **Where** — `file:line`.
3. **How it fails** — concrete inputs or sequence producing the wrong result. If you
   reproduced it, show the command and the output.
4. **Confidence** — confirmed by execution, or inferred from reading.
5. **Fix** — the specific change, not a direction.

End with:
- Anything you could **not** check and why, so I know the audit's boundary.
- Your honest read on whether this is safe to hand to an external operator on testnet, and
  what you would want fixed before that.
