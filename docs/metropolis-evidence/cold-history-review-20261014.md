# Cold-History Review Handoff

Review branch: `review/metropolis-cold-history-20261014`.
Integration base: `19fd2b0` on `metropolis`, including the separate per-endpoint read-rate controls.
Original OpenCode base: `482d912cbc33e5f96de85f49b3006d631454b607`.
Original snapshot identity: `f6ec6bcc27f39bc39155bf439301a80ec6e0e4361c5083c12fbe6f6db4efb6dd`.

Only four storage files were transferred from OpenCode's isolated worktree.
Their content hashes matched the local handoff before transfer.
Other dirty files, private configuration, keys, RPC credentials and build artifacts were not transferred.
This branch is for review, not publication or integration into the shipping branch.
Published release `0.3.0.dev2026101307` is unchanged.

## Input Hashes

| File | SHA-256 |
|---|---|
| `sdk/shielded/src/index_store.rs` | `31bb4eaae5ee9786a2b0ed3272487f6b6ceeaf1f371d9a1547a3ebb0a30d30af` |
| `sdk/shielded/tests/index_segments.rs` | `253817b1bcff93b84f2656df057c3100990b9fdd9a08d5a151dec3fc10fc1bc9` |
| `sdk/shielded/tests/index_scale.rs` | `0a96687a6c2595bc7ffa555fe65bf8ed416f95989807547375a1ceffbaeb3f71` |

`tests/index_store.rs` changes only the v1 rescan assertion to version 3 and checks the segments array.
Verify its complete diff against the integration base.

## Reported Results

OpenCode reported these results on its original snapshot; they are not qualification of this review branch:

- Six segmented-store tests passed: bounded files, tail reuse, v2 migration, corruption, orphan cleanup and interrupted saves.
- Existing index-store tests passed, including restart, stale writer and domain checks.
- RPC finality, indexer and concurrency suites passed (3, 11 and 12 tests).
- A synthetic 600,000-block cache saved successfully in 17 segments, about 142 MB total.
- The legacy cache failed at its 128-MiB cap for that synthetic scale.
- Full load still rebuilds the index. Memory remains proportional to history size.
- RPC reads remain the dominant catch-up cost; storage improvement does not establish live throughput.

## Cloud Qualification

Use `/home/user/Erebus` or the actual cloud clone, not the Mac's absolute paths.
Fetch this branch, preserve any unrelated changes, and check out its exact commit.
Keep the 5-GiB disk guard. Share an existing Cargo target directory only when no other build uses it.
Do not delete another process's cache, wallets, keys or evidence.

Run from the repository root:

```sh
cargo fmt --check --manifest-path sdk/shielded/Cargo.toml
cargo test --release --locked --manifest-path sdk/shielded/Cargo.toml --test index_store --test index_segments
cargo test --release --locked --manifest-path sdk/shielded/Cargo.toml --test rpc_finality --test indexer --test indexer_concurrency
cargo clippy --release --locked --manifest-path sdk/shielded/Cargo.toml --lib --test index_store --test index_segments --test index_scale -- -D warnings
INDEX_SCALE_SKIP_MOCK=1 cargo test --release --locked --manifest-path sdk/shielded/Cargo.toml --test index_scale -- --ignored --nocapture --test-threads=1
```

Review migration commit ordering, stale writers, segment/domain binding, corruption, cleanup and rollback behavior.
Add missing regressions without weakening paired evidence or reservation/spendability rules.
Rerun affected wallet/payment recovery suites and installed migration where the required fixtures are available.
Record unavailable gates explicitly; synthetic scale tests are not live Monad settlement or an audit.

The base includes optional wallet/payment `rpc_requests_per_second` and `peer_rpc_requests_per_second` fields (1 through 40).
Those source controls are not in published 1307. They pace `PoolRpc` reads, not all EVM requests or a shared account quota.
Each pacer retains an eight-request burst. Account-wide concurrency still needs operator coordination.

Quicknode deployment-era reads passed at the probe's 40-RPS setting; Dwellir passed 100 reads at 10 RPS but rate-limited at 40.
A subsequent bounded source-wallet scan advanced only the primary cache; peer progress remains unverified.
Do not claim paired catch-up, a migrated live wallet, independent acceptance or complete live operation.
No release authorization, publication or live-write permission is added by this handoff.
