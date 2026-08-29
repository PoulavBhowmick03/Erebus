# Mainnet two-account preflight — 2026-08-29

This run prepared two Erebus-owned accounts for a full mainnet flow and tested the final
deposit dependency. It completed a second standalone pool registration. It did not shield,
open a channel, negotiate, or settle.

## Accepted transactions

| Action | Transaction |
| --- | --- |
| Transfer 40 STRK from A to B | `0x0387b2c1ec88eb7fd88b0d1493d8670bf4f83d507be60b76ffa538c92735d7bb` |
| Deploy Account B | `0x075e44116b04e7f434bad492ff6059c7e4b798adb86004abf712d1587716a334` |
| Set A pool allowance to 25 STRK | `0x02a8f728fa98eb44e49556908f5070d648dc874cb657b11f6ae8b7e44b41e131` |
| Set B pool allowance to 18 STRK | `0x030a08b7dda11c8ec52324fcc25fb6293f1e7d26f66ffde8b47fe9497bba48cc` |
| Register B with the pool | `0x0572260b651525ea39ef717721bcc9fefc89a2087894654efb38111e09267189` |

Account B is `0x05cc7c436a454962c0b2833e1aeb5107fbefa8000cc1f94b6812580fc3d6ffac`.
Its registration succeeded in block `14031230` at `2026-08-29T05:36:13Z`. The block hash was
`0x0653965cee7c2cb8386d919dc6cd157492f7602981186442d2de18d438587f4f`.
The network fee was `2.841538430810898432 STRK`; the pool also collected its 6 STRK fee.

The signed registration transaction and receipt were persisted in mode-`0600` files before
and after submission. A fresh state read returned the pool public key derived from B's
protected pool-key file.

## Readiness after registration

Both `erebus-cli doctor` reports returned `ready: true`:

- A: registered, 25 STRK pool allowance, `144.522084358423568096 STRK` public balance.
- B: registered, 12 STRK remaining pool allowance, `31.022385990051624352 STRK` public balance.

A has allowance for its planned shield plus three later pool writes. B has allowance for its
planned reverse-channel open and counter offer. No allowance was consumed by the screening
probe because it never submitted a transaction.

## Screening probe

The probe built and proved a real 1 STRK shield for A against the canonical mainnet pool. It
compared the proof's server actions with the Alchemy `compile_actions` result and then checked
`additional_data.signature`. The local prover returned no screening signature.

The canonical pool has a non-zero screener key. Submitting this proof would therefore revert.
No fee estimate or transaction submission was attempted. The remaining external prerequisite
is access to the operator's elliptic-proxy `/screen` service through the published proof
interceptor, including `SCREENING_URL`, `SCREENING_PARTNER_NAME`, and
`SCREENING_PARTNER_SECRET`.

## Local prover limits observed

The 20 GiB ARM64 VM needs a fresh prover process for each large proof. A two-thread deposit
proof OOM-killed the container. A fresh one-thread process completed the deposit proof. The
SDK proof-request timeout is now 600 seconds because a healthy one-thread proof exceeded the
former 180-second limit.
