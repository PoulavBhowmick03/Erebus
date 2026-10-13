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
