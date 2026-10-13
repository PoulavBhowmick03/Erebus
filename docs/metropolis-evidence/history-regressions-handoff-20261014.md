# History-regression tests: handoff to the corrected cold-history lane (2026-10-14)

Base commit: `f969f0786516e985913123dcf454761d575be8a8` (`review/metropolis-cold-history-20261014`).
This branch adds tests only. It is **not integration-ready**: Claude's baseline testing found
F1–F3 on the base, and these tests must be rerun against the corrected candidate with every
assertion preserved.

## Files and hashes

| File | SHA-256 |
|---|---|
| `sdk/shielded/tests/rate_controls.rs` | `14b31520b4a813af285739a2632c2f4cd9746150a72f069eafdccf8c944bc79c` |
| `sdk/shielded/tests/peer_scan_failures.rs` | `99494c04ed5bfcd0c908b5e06fb2177c6d1c36cb9ae738af1c785bdb96778ef6` |

No production, authorization or package file is touched.

## Commands and results (on the base commit)

```sh
cargo test --release --locked --manifest-path sdk/shielded/Cargo.toml --test rate_controls
# 2 passed, 0 failed (5.6 s)

cargo test --release --locked --manifest-path sdk/shielded/Cargo.toml --test peer_scan_failures
# 3 passed, 0 failed (5.7 s)

cargo clippy --release --locked --manifest-path sdk/shielded/Cargo.toml \
  --test rate_controls --test peer_scan_failures -- -D warnings
# clean
```

## What the tests pin

`rate_controls.rs`:
- the library refuses a zero rate with `RpcError::Configuration` and accepts positive rates;
- `DEFAULT_MAX_REQUESTS_PER_SECOND == 40`;
- at 5 rps a mock provider observes the documented eight-request burst and spaced later
  requests, with a stall bound.

`peer_scan_failures.rs`:
- a rate-limited peer is masked per bounded call while the primary prefix is pending, then
  surfaces as a distinct method-level RPC error (`eth_getLogs`), with the peer checkpoint
  retained at zero, the primary checkpoint monotonic across reopened stores, the wallet bytes
  unchanged on every failing call, and a clean heal after the peer recovers;
- a conflicting shared anchor fails before any masking, with no checkpoint movement and no
  wallet write;
- a finalized-anchor regression fails closed as an RPC error and is never reported as pending.

## Scope: library rate versus binary config

The `1..=40` cap is the wallet/payment **config** policy, enforced before any request; the
library accepts any positive rate. The config-layer refusal is not covered here because it
requires a full valid operator configuration (funded harness). That remains a gap for the
corrected candidate.

## Observability limitation (follow-up, not a release change)

`PoolRpc` maps every JSON-RPC error to a generic method-level `RpcError::Request(method)`, so a
provider rate limit is indistinguishable from any other read failure in the surfaced error.
Classification is still distinct from absence or pending progress, so this is an observability
gap, not a safety one. A redacted cause class (for example `rate_limited`) is proposed as a
separate follow-up and must not be added as a last-minute release change.

## Requested integration

Incorporate both files into the corrected branch and rerun them there, preserving every
assertion. If a corrected behavior legitimately changes a pinned classification, record the new
evidence and adjust the assertion explicitly rather than weakening it silently.

## Addendum 2026-10-14: binary rate bounds and shared-account semantics

Two further scoped test files were added on this branch.

### `sdk/shielded/tests/rate_config_bounds.rs` (`39460561db443e7942e9b01913beda84303c0700ae5cd06ae3a94d129d564fa3`)

Drives the actual `erebus-payment` and `erebus-wallet` binaries with counting mock providers:

- primary and peer rates 0 and 41 are each rejected **before any read, signing or broadcast**
  (zero requests observed; the payment binary names the rate error, the wallet binary fails
  config validation);
- rates 1 and 40, and omitted fields, pass the config gate (the failure that follows is later
  state, not the rate error), and neither binary signs or broadcasts;
- default-40 behavior is pinned at the library level in `rate_controls.rs`.

Scope gap recorded: reaching the diagnostics/read path after acceptance needs a funded
coordinator/wallet state (funded fixtures), so the binary acceptance assertions stop at
"config gate passed". The binary-level 1..=40 cap is proven for both binaries; the
omitted-field default (40) is proven at the library level only.

### `sdk/shielded/tests/relayer_shared_account.rs` (`a7ce9a0f3a0603ea6d2ac324204f73b6281969eac9201010df0ac7673fd38fbe`)

- `execution_lock_is_exclusive_per_sponsor_root_and_releases_on_drop`: pins the exclusion
  mechanism behind `RelayJournal::try_execution_lock` that `SponsoredRelay` maps to
  `SponsorError::Busy` (HTTP 503, "relay busy; retain the operation"): a second holder on the
  same sponsor root sees contention, a different root does not, and the lock releases on drop.

Coverage that remains in crate-private unit suites (run and recorded, not duplicated):
`cargo test --lib relay_` -> 22 passed, including the concurrent-callers one-signer/one-initial
-send journal behavior, sponsor write-fault fencing and copied-proof conflicts;
`cargo test --lib relay_http` -> quota persistence across restart, duplicate reservation,
corruption/deleted-quota fail-closed and bearer authentication.

Scope gap recorded: an end-to-end assertion of one broadcast, one sponsor nonce allocation and
the HTTP 503 busy state through the installed relayer binary exists only in the unmerged
relayer-fixture candidate (`sdk/shielded/tests/relayer_binary.rs`, chain-10143 fixture); it is
not on this branch. No queued/busy semantics were invented; the documented 503 mapping is used
as-is.

Commands:

```sh
cargo test --release --locked --manifest-path sdk/shielded/Cargo.toml --test rate_config_bounds
cargo test --release --locked --manifest-path sdk/shielded/Cargo.toml --test relayer_shared_account
cargo test --release --locked --manifest-path sdk/shielded/Cargo.toml --lib relay_ -- --test-threads=1
cargo clippy --release --locked --manifest-path sdk/shielded/Cargo.toml \
  --test rate_config_bounds --test relayer_shared_account -- -D warnings
```

Results: 3 passed (63 s), 1 passed, 22 passed, clippy clean.
