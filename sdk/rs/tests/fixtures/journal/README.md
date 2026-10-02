# Historical operation-journal fixtures

Test data for `tests/journal_characterization.rs` and `tests/reconcile_characterization.rs`
(Metropolis M6, slice 1). Everything here is fake: operation ids, bindings, hashes, nonces,
handles and "transactions" are made-up values chosen to be recognisable. There is no key
material in any file. Nothing here was ever on a chain.

The records were **written by the journal code of each historical commit**, not by hand and
not by today's code. They exist so the M6 refactor can prove that records from every schema
still load, classify, and serialize exactly as they do now.

## Sets

| Directory | Commit | `version` | What that commit's schema added |
|---|---|---|---|
| `v1-b784f3f/` | `b784f3f` | 1 | First journal. No stored transaction, no nonce, no channel. Restart reopens at `prepared`. |
| `v1-7b7b4c8/` | `7b7b4c8` | 1 | `transaction_stored` and `<id>.<n>.tx`; `prepared -> signed` edge. |
| `v1-448146e/` | `448146e` | 1 | `account_nonce` and `channel` (both first appeared in `f9bda51`; `448146e` has the same record schema and changes `restart` to reopen at `claimed`). |
| `v2-b1fe0e3/` | `b1fe0e3` | 2 | `request`, `result`, `prepared`, `completion`, `receipt`; `claimed -> committed`; `signed` counts as may-have-landed. |
| `v3-d3dab59/` | `d3dab59` | 3 | No new field: `accepted_at` now means the accepted block's timestamp, not the local clock. |
| `v4-93593ed/` | `93593ed` | 4 | `simulation_hash`. Current `JOURNAL_VERSION`; `sdk/rs/src/journal.rs` is unchanged since, apart from one doc comment. |

`d09cd4c` (prune) and `24fcb06` (cleanup) touched `journal.rs` without changing the record
schema, so they have no set of their own. The commit ids and what each changed were checked
with `git log -p -- sdk/rs/src/journal.rs`.

Each directory holds the files the journal wrote under `operations/`: `<id>.json` records and
`<id>.<attempt>.tx` stored transactions. Runtime `.lock` files are not kept; the journal
creates them. The tests copy a set into a temporary state directory as
`<tmp>/operations/*`, directory `0700`, files `0600`.

## Record codes

An operation id is `op_<era><code>` followed by 60 zeros. The era byte names the set
(`1a` b784f3f, `1b` 7b7b4c8, `1c` 448146e, `20` b1fe0e3, `30` d3dab59, `40` 93593ed). Every set
has codes `01`-`0b`; schema 2+ sets also have `0c`-`0e`.

| Code | Latest stage | How it was driven |
|---|---|---|
| `01` | claimed | claim (v2+: with request) |
| `02` | prepared | v2+: completion, prepared snapshot; proof anchor (block 100, valid to 550; v4: simulation hash); `prepared` |
| `03` | proven | `02`, then `proven` |
| `04` | signed | `03`, then (448146e+) nonce `0x5`, then `persist_signed` (b784f3f: amend hash + `signed`) |
| `05` | submitted | `04`, then `submitted` |
| `06` | accepted | `05`, then (v2+) receipt in block 120, `accepted_at`, `accepted`. A `propose_offer` with a channel (448146e+). |
| `07` | committed | as `06` with block 121, then (v2+) result, `committed` |
| `08` | reverted | `05`, then (v2+) reverted receipt in block 122, `reverted` |
| `09` | needs_attention | `05`, then `needs_attention` |
| `0a` | submitted, 2 attempts | `05` at nonce `0x5`, `restart`, second attempt anchored at 200 (valid to 650), nonce `0x6`, submitted |
| `0b` | fresh 2nd attempt | `05`, then `restart` only (reopens at `prepared` in b784f3f/7b7b4c8, `claimed` after) |
| `0c` | committed | v2+: `open_channel` claimed, result recorded, `claimed -> committed` with no transaction |
| `0d` | claimed | v2+: request-less `claim` |
| `0e` | prepared | v2+: request-less `claim`, then `prepared` |

Bindings are `RequestBinding::builder(op, 0x534e5f5345504f4c4941, 0x4e4f, 0x53545f)
.u128_be(1000 + code)`. Transaction hashes are `0x<era><code><attempt>beef`. Timestamps are
the commit's date at 00:00 UTC plus one second per journal call. `accepted_at` is that local
clock value for v1/v2 (as the clients of that era wrote `now()`), and `1_787_000_000 + block`
for v3/v4 (the block timestamp). In `b784f3f` the shipped client only ever claimed ids; the
later stages of that set were driven through that commit's journal API directly, which
serializes the same struct.

## How they were produced

All six sets were built from their commit on rustc 1.96.0 with each commit's own
`Cargo.lock` (`--locked --offline`). None needed a fallback to hand-writing.

```bash
sdk/rs/tests/fixtures/journal/generator/regenerate.sh            # rewrite the sets in place
sdk/rs/tests/fixtures/journal/generator/regenerate.sh /tmp/check # or write them elsewhere
```

For each commit, `regenerate.sh`:

1. `git archive --format=tar <commit> sdk/rs | tar -x -C $WORK/<commit>`, with `$WORK` a
   `mktemp -d` outside the repository.
2. Appends `generator/gen_v1.rs.tmpl` (schema 1) or `generator/gen_v2plus.rs.tmpl`
   (schema 2+) to that copy's `src/journal.rs` as a `#[cfg(test)] mod fixture_gen`, after
   substituting the commit, era byte and base time and keeping only the lines tagged for that
   commit's API level (the tags are explained at the top of each template).
3. Runs `FIXTURE_OUT=$WORK/out/<commit> cargo test --locked --offline --lib
   fixture_gen::generate -- --ignored` in that copy, which writes a journal under
   `$WORK/out/<commit>/operations/`.
4. Copies `*.json` and `*.tx` from there into `<set>/` byte for byte, then deletes `$WORK`.

Generation is deterministic: rerunning the script into another directory and `diff -r`
against this one shows no difference (checked when the fixtures were added).

## Goldens

`golden/` holds the expected output of the current code over these records, compared byte for
byte by the characterization tests:

| Path | Pinned by | Content |
|---|---|---|
| `reserialized/<set>.jsonl` | `journal_characterization` | each legacy record re-encoded by current code, one per line |
| `write-back/*.json` | `journal_characterization` | a legacy record after `advance`, `persist_signed`, or `record_result` |
| `backfill/*.json` | `journal_characterization` | a record after a request backfill at `claimed` |
| `reconcile/findings.json` | `reconcile_characterization` | `reconcile::reconcile` over every record |
| `reconcile/rpc-calls.json` | `reconcile_characterization` | every JSON-RPC call that made, in order |
| `reconcile/client-findings.json` | `reconcile_characterization` | `Client::reconcile` (what `erebus-cli reconcile` returns) |
| `resume/plans.json` | `reconcile_characterization` | `resume::plan` for every record at heads 500 and 700 |
| `resume/client-outcomes.json` | `reconcile_characterization` | `Client::resume_operation` on selected records |
| `resume/outcome-variants.json` | `reconcile_characterization` | every `ResumeOutcome` variant's JSON |

They were generated from the current code, so they describe current behaviour, including
behaviour the tests mark `CHARACTERIZATION: ... flagged for review`. After an intended
behaviour change, regenerate them and review the diff:

```bash
cd sdk/rs && EREBUS_BLESS_JOURNAL_GOLDENS=1 cargo test --locked \
  --test journal_characterization --test reconcile_characterization
```
