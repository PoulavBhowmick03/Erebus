# Mainnet 0.8 STRK run, two subagents — 2026-09-11

The fourth complete bounded Erebus workflow on Starknet mainnet, run on launch day. Two
Claude Code subagents drove it, one per MCP server, coordinating only through
`wait_for_offers` on the shared channel.

**This run establishes less independence than the 2026-09-07 one.** Both subagents inherited
the parent session's full tool set, so "touch only your own server" was enforced by prompt
rather than by architecture. The 2026-09-07 Claude Code + Codex run remains the stronger
evidence for definition-of-done 4; this one is a repeat-deal and disclosure check, not a
cross-framework result.

## Frozen inputs

| Item | Value |
| --- | --- |
| Network | `SN_MAIN` |
| Source | `e4dbc34` |
| Client binary | `sdk/rs/target/debug/erebus-cli` (see friction F41) |
| Prover | Starkscan asynchronous mainnet relay |
| Token | STRK |
| Pool fee | 6 STRK per `apply_actions` |
| Shield | none — existing notes funded the deal |
| Opening offer | 0.4 STRK, buyer-authored |
| Agreement | 0.8 STRK from A to B, seller-authored |
| Change | 0.1 STRK to A |
| Deal ID | `319440722280485962` |

Both existing directional channels were reused; no channel was opened. Channel
`ch_b7afee5f…9af8` is A's view, `ch_c94f5afb…4fb2` is B's.

Allowances were sized to the writes planned — A 13 STRK for two writes, B 7 STRK for one —
and both read back 1 STRK afterwards.

## Mainnet transactions

| Action | Transaction | Block | Driver |
| --- | --- | ---: | --- |
| Allowance, A (13) | `0x927b8e8a1d0ca8a600c72ec33ad0d5ce23d1afb0334c0d828eb2cb190c80ae` | — | operator |
| Allowance, B (7) | `0x76d30773c56e5b0da815de9f00c9de72fe8f94e38e20437c322586ae9ce3acc` | — | operator |
| Buyer proposal, 0.4 | `0x946af7cd82d445f527142ae4bc38b919bf7b737471b1ea1806041963dea7ec` | — | subagent A |
| Seller counter, 0.8 | `0x15f05ca916300d7ee01308a0d388a9c739c91736f79606aceb99b82b9f1edc9` | — | subagent B |
| Atomic settlement, 0.8 | `0x43329b0ff5d7865e0be3d4180e608229a5e8fd0d90312f4189f7aa9b8acadcd` | 14711839 | subagent A |

The two allowance transactions are ordinary ERC-20 approvals on the account key. They carry
no proof and touch no pool state.

Account A went from 25.202741 to 7.154826 STRK, Account B from 11.293062 to 2.844175 STRK —
about 26.5 STRK in pool and network fees combined for three `apply_actions` writes.

## Checks

- **Payment and change conserved.** A's notes went 1.1 → 0.3 (0.2 and 0.1); B's went
  3.4 → 4.2 (2.0, 0.8, 0.8, 0.6). A spent one 0.9 note, 0.8 went to B, 0.1 returned as
  change, and A's unrelated 0.2 note was left untouched.
- **Scoped disclosure verified.** Account C
  (`0x6b293619b447480677ee2da22dbb7c442c4ae0251de9889f9cbb375ef51bad6`) reconstructed only
  this deal's three offers out of the twelve now in the channel. The three earlier deals
  stayed unreadable.

## Operational findings

- **`DEFAULT_PROVING_BLOCK_LAG = 10` is the gate before any write**
  (`sdk/rs/src/execution.rs:40`). `compile_actions` simulates at `head - 10`, so a fresh
  `approve` must be ten blocks deep or the write simulates against the old allowance and
  reverts inside `collect_fee` with a bare `Contract error`. This — not only the stale
  release binary — is the real cause behind the 2026-09-07 run's three failed attempts.
- **`approve` replaces the standing allowance**, it does not add to it.
- There is no standalone `approve` in `scripts/agent.sh`; only `fund`, which also shields.

## State after this run

**Both accounts are low.** A has 7.154826 STRK gas and 0.3 in notes; B has 2.844175 and 4.2.
Both allowances read 1 STRK, below the 6 STRK per-write fee, so **no further write can
succeed without a new `approve`**. A can fund roughly one more write; B cannot afford one.

## What this run does not establish

A fourth bounded workflow, not evidence of capacity, uptime, independent security review, or
safe use with real value. The subagent independence caveat above limits what it says about
external agent frameworks. The relationship, transaction timing, action shape, and note count
remain public exactly as [privacy-model.md](../privacy-model.md) describes.
