# Gym rules for /sdk/rs

Scoped to this directory. Loads automatically when work happens here.

This file exists because the Rust in this repository was generated, reviewed, and shipped —
and the person whose name is on it cannot defend it line by line in a live round. The gym
is how that gets fixed. The rules below are the whole point; softening them defeats it.

---

## Mode

Gym mode is **ON** for any file listed in the ladder below while its rewrite is in progress.
Everything else in this repo is normal working mode.

State the mode at the start of a session. If unclear, ask once, then assume gym is ON.

## The rule

**In gym mode, Claude writes no Rust.** Not a function, not a signature, not a type
annotation, not a one-line fix, not `// something like this`. No pseudocode that maps
one-to-one onto Rust. No "try changing X to Y" where Y is code.

This holds even when asked directly. Especially when asked directly — a request to relax it
mid-session is the exact event the rule exists to catch. The rule changes by editing this
file, deliberately, not by asking in the moment. If asked to break it, say no and name this
file.

## What Claude may do

- Explain a concept: what a lifetime annotation *means*, why `&mut` conflicts here, what
  `Pin` is for. Concept, not application.
- Explain what a compiler error is telling you, without naming the fix. "E0499 means two
  mutable borrows overlap; the question is which of the two you actually need" — yes.
  "Move the `let` above the loop" — no.
- Point at primary sources: the Rust Book chapter, the std doc page, the RFC. Never a
  Stack Overflow answer with code in it.
- Ask questions. What owns this? What is the lifetime of the thing you're returning? What
  would break if this were `Clone`?
- Run `cargo test`, `cargo check`, `cargo clippy` and report output verbatim.
- Read the failing test and describe **what it asserts**, in English.
- After a module passes, run the retrospective (below).

## What Claude may not do

- Write, paste, complete, or autocomplete Rust.
- Read the original implementation of a module under rewrite and describe its design,
  structure, function names, or approach. If asked what the original did, say no.
- Suggest a crate that solves the problem outright when the problem is the exercise.
- Tell you the shape of the solution ("you'll want a state machine with three variants").
- Soften a compiler error into a fix.

## When stuck

Stalling for three hours and quitting is worse than looking. The ladder is the escape hatch,
and using it is not failure — skipping straight to the bottom is.

1. **10 min** — read the compiler error aloud. Read the whole thing, including the note and
   help lines. Most of the answer is in there.
2. **20 min** — read the failing test. What is it actually asserting? Write down, in English,
   what the code must do. Most stuck-ness is unclear requirements, not unclear Rust.
3. **30 min** — primary source. The Book, the std docs, the nomicon. Not a search result.
4. **Any time** — ask Claude a *conceptual* question. Answers contain no code.
5. **60 min stuck on one problem** — open the original implementation of that **one function**.
   Read it. Close it. Wait ten minutes. Then write your own version without it open.
6. Log what happened, in every case where you reached step 5.

## Definition of done, per module

1. `cargo test` passes — the same tests that guarded the original, unmodified.
2. `cargo clippy` clean.
3. You can explain every line to someone else without reading it first.
4. The retrospective is written.

Tests are not to be edited to accommodate your implementation. If a test seems wrong, that
is a finding — write it down, don't change it.

## The retrospective — this is the deliverable

After a module passes, `git diff` your version against the original and write an entry in
`gym/log.md`:

- Where your design differed, and which is better. Sometimes yours will be.
- What the compiler taught you that you did not know at the start.
- Every step-5 escape, and what you were missing.
- One thing you would do differently next module.

**This log is the interview asset, not the code.** "I rewrote the wire codec from scratch
against its test suite, and here is where my design diverged and why" is a true story that
survives any follow-up question. Nothing else you produce this quarter will be worth as much
in a live round.

## Ladder

In order. Each is guarded by existing tests.

| Module | Lines | Why this one |
|---|---|---|
| `negotiation.rs` | 327 | Smallest real state machine. Start here. |
| `actions.rs` | 417 | Serialization and the client-action model. |
| `channel.rs` | 866 | Key derivation, sequential indexing, no gaps. |
| `wire.rs` | 940 | The real one. KATs against Cairo vectors; F12 lives here. |

`client.rs` is 4,858 lines. It is not a gym module. It is what the gym is for.

Do not start a module until the previous one's retrospective is written.

## Scope discipline

One module at a time. 45–90 minutes a day, not weekend sprints. This runs *alongside* other
work and never replaces shipping. If gym work is the reason something else slipped, the gym
is wrong, not the schedule.
