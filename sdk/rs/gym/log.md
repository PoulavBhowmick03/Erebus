# Gym log

One entry per module, written after the tests pass. Rules in `../CLAUDE.md`.

The value here is the record of decisions, not the code. Write it for a reader who will
ask "why did you do it that way?" in six weeks.

---

## Template

### <module>.rs — <date started> to <date passed>

**Time:** <hours, honestly>

**Where my design differed from the original**
<Diff it. What did you do differently? Which is better, and why? Say so when yours is.>

**What the compiler taught me**
<Specific. "E0499 twice on the same struct, because I wanted a &mut to two fields at once —
learned that splitting the borrow means splitting the struct or the function.">

**Escapes (step 5)**
<Every time you opened the original. What were you missing? This is the most useful section
and the one you will want to skip.>

**What I'd do differently next module**
<One thing.>

---
