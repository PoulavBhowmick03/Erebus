# Codex prompt: rewrite the prose in this repo in a plain human voice

Paste everything below the line into Codex, running in the repo root.

---

You are doing an editorial pass over the Erebus repository. The code works and is well
tested. What is wrong with it is the writing: comments, doc comments, docstrings, markdown,
commit-adjacent prose and test names were largely drafted by an LLM and read like it. Your
job is to rewrite that prose so it reads like a working engineer wrote it in a hurry and
meant every word.

This is a style pass. It is not a refactor, not a review, and not an opportunity to improve
anything about how the code behaves.

## The hard rules, in order of importance

1. **No behavioural change of any kind.** Not one token of executable code changes meaning.
   You may reflow a comment; you may not touch the statement under it. If you believe you
   have found a bug, write it down in a list at the end and leave the code alone.

2. **Run the gate after every batch and never leave it red.**
   ```
   cd sdk/rs && cargo test && cargo clippy --all-targets -- -D warnings \
     && RUSTDOCFLAGS='-D warnings' cargo doc --no-deps
   cd - && uv run pytest -q
   cd sdk/ts && pnpm vitest run
   ```
   Expected: 194 Rust tests passing with 3 ignored, 45 Python, 38 TypeScript. If a count
   changes, you broke something. `cargo doc` denies missing docs, so you may rewrite a doc
   comment but never delete one.

3. **Never touch these.** Fixture files under `sdk/rs/tests/fixtures/`, anything in
   `sdk/ts/` other than comments, `.env.example` values, `Cargo.lock`, `uv.lock`, and any
   string literal, test vector or hex constant anywhere. Reformatting a fixture silently
   destroys a known-answer test.

4. **Preserve every fact.** Specifically: friction numbers (F1 through F31), file and line
   citations like `privacy.cairo:664`, measured numbers (3.04 STRK, ~20 s, 119 bits, 2^-59),
   transaction hashes, addresses, dates, and the names of upstream symbols. You are changing
   how sentences are built, not what they claim. If a sentence's only content is decoration,
   delete the sentence rather than rewriting it.

5. **Do not weaken a safety comment.** Several comments exist because someone lost hours to
   the thing they describe: the `head - 10` proof anchor, `INDEX_NOT_SEQUENTIAL`, write-once
   note slots, the pool key travelling in `compile_actions` calldata, registration being
   irreversible. Those must survive in plainer words, not disappear.

## What "AI-ish" means here, measured

Counted in this repo right now:

| pattern | count | what to do |
|---|---|---|
| em dash `—` | 903 | remove nearly all of them |
| "deliberate/deliberately" | 67 | keep maybe five |
| "worth noting / worth knowing" | 7 | delete the phrase, keep the fact |
| "the whole point / that is the point" | 5 | delete |
| emoji in markdown (⏸ ⚠️) | 3 | replace with a word |
| comment lines in `sdk/rs/src` | 1766 of 8538 (21%) | aim for 12 to 15% |

The em dash is the single loudest signal. Replace each one by choosing what it was doing:

- Joining two independent clauses: use a full stop. Two sentences.
- Introducing a definition or list: use a colon.
- A parenthetical aside: use brackets, or cut the aside.
- Emphasis before a punchline: cut the punchline, state the fact.

Beyond punctuation, hunt these constructions:

- **Negative parallelism.** "This is not ceremony, it is the enforcement." "Not a bug, a
  constraint." Say what the thing is and stop.
- **Rule of three.** "hashing, salt encoding, felt arithmetic" appearing over and over.
  Two items, or one, or a plain plural.
- **The reversal.** "X is true. Y is true and irrelevant." Cut to the operative fact.
- **Signposting.** "Importantly", "Critically", "Note that", "It turns out", "The point is".
  Delete the signpost; the sentence still works.
- **Editorial verdicts in comments.** "which is the right call", "and that is correct",
  "worth being deliberate about". Comments state facts and reasons. They do not award marks.
- **Balanced rhythm.** Long, evenly weighted sentences with matched clause lengths. Break
  them up unevenly. Real writing has short sentences next to long ones.
- **Hedged qualifiers stacked together.** "genuinely", "actually", "essentially",
  "effectively", "arguably", "in practice". Keep at most one per paragraph.
- **Bold mid-sentence for emphasis.** Fine in a heading, wrong inside prose.
- **Rhetorical questions.** There should be none.

## Voice to write in

Short declarative sentences. Present tense. Concrete nouns. The reader is a competent
engineer who has not seen this file before and is in a hurry.

Comments answer one of three questions and nothing else: why this and not the obvious
alternative, what breaks if you change it, where the authority for this claim lives. If a
comment answers none of those, delete it.

Before and after, taken from `sdk/rs/src/subchannel.rs`:

> **Before**
> ```
> /// The message index a new message would occupy, if the cursor is on the grid.
> ///
> /// Errors with [`IndexError::Misaligned`] rather than rounding. Rounding would skip an
> /// index and hit `INDEX_NOT_SEQUENTIAL`; truncating would overwrite and hit
> /// `NON_ZERO_VALUE`. There is no safe repair, so the caller has to know.
> ```
>
> **After**
> ```
> /// Index a new message would occupy. Requires the cursor to sit on a message boundary.
> ///
> /// Returns [`IndexError::Misaligned`] instead of rounding. Rounding up skips an index and
> /// the contract rejects it with `INDEX_NOT_SEQUENTIAL`. Rounding down overwrites and hits
> /// `NON_ZERO_VALUE`. Neither is recoverable, so the caller has to handle it.
> ```

Same facts, same citations, no em dash, no closing flourish.

## Test names

Names like `an_encrypted_note_carries_no_token_and_that_is_not_an_error` are sentences with
an opinion attached. Shorten to the property under test:
`encrypted_note_token_is_zero`. Move any explanation into a doc comment on the test.

Rename with `cargo test` between every file so a typo surfaces immediately. Check whether
the old name is cited in `docs/` and update the citation if so.

## Order of work, and commit as you go

Work in this order, committing after each group with a plain message. Small commits mean a
bad rewrite is one `git revert` away.

1. `sdk/rs/src/*.rs` module docs and doc comments, one file per commit. Start with
   `wire.rs`, `client.rs`, `channel.rs`, `hashes.rs`, since those are the ones a reviewer
   opens first.
2. `sdk/rs/src/bin/erebus_cli.rs` and `sdk/rs/tests/*.rs`, including test renames.
3. `sdk/py/` and `mcp-server/` docstrings and comments.
4. `scripts/*.sh` and `scripts/*.py` header comments.
5. Markdown in `docs/`, largest first: `friction.md`, `poulav.md`, `ishita.md`,
   `runbook.md`, `ARCHITECTURE.md`, `agent-brief.md`, then the rest.
6. `README.md` and `CLAUDE.md` last, and be conservative in `CLAUDE.md`: it is instructions
   the team follows, so clarity matters more than voice and the numbered constraints keep
   their numbering.

## Markdown specifics

Headings: sentence case, no trailing colons, no title case.

Tables stay. They are the least AI-ish thing in the repo and they carry real data.

The friction log is a deliverable StarkWare will read. Keep every F-number, every citation
and every measured figure. Cut the framing sentences around them. A friction entry should
open with what was attempted, then what happened, then the workaround. It should not open
with a thesis statement.

Delete section-closing summary paragraphs. If a section ends by restating what it said,
that ending is padding.

## Report at the end

Produce a short list of:

- files changed and roughly how many lines of prose were cut
- any place where removing the flourish left a claim that now looks unsupported, which
  usually means the claim was thin to begin with
- any suspected bug you found and did not touch
- anything you left AI-flavoured on purpose, and why

Do not add a concluding paragraph to that report.
