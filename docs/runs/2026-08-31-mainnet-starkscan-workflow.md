# Mainnet Starkscan workflow — 2026-08-31

This is the first complete bounded Erebus workflow on Starknet mainnet. It used the
operator-approved Starkscan hosted STRK20 prover. No local transaction-prover or screening
interceptor ran during this canary.

## Frozen inputs

| Item | Value |
| --- | --- |
| Network | `SN_MAIN` |
| Source | `00554201451e4a7659d4fda02694e34509ad4018` |
| Erebus package candidate | `0.2.0`, Protocol 4, wire v3 |
| Prover | Starkscan asynchronous mainnet relay; upstream prover version not disclosed by the API |
| Prover API | `https://api.starkscan.co/v1/SN_MAIN/prove` |
| RPC specification | `0.10.3-rc.0` |
| Pool | `0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a` |
| Token | STRK |
| Pool fee | 6 STRK per `apply_actions` |
| Shield | 1 STRK from Account A |
| Agreement | 0.8 STRK from A to B |
| Change | 0.2 STRK to A |
| Deal ID | `11534264160786983021` |

The Starkscan key authenticated with `prove` scope. It stayed in a mode-0600 ignored env
file and is not present in this record. The relay received the proof invocation, including
the pool private key. The Alchemy RPC received the same key during `compile_actions`.

## Readiness

Both identities passed `doctor`: RPC, Starkscan capability check, chain, pool version 2.0,
registration, key files, and public funding were live. Reconciliation found no ambiguous or
unresolved operation. A had 19 STRK allowance, enough for the 1 STRK deposit plus three
6 STRK pool fees. B had 6 STRK allowance for its counter.

The existing directional channels were reused:

- A to B: `ch_b7afee5fd1f75ddc8425e8ca8b7879b4780588f81163a581f258289c238d9af8`
- B to A: `ch_c94f5afb8ad73af9f78baf4be45099ca7f57f28c230fa3a1104bef70b0d04fb2`

## Mainnet transactions

All fees below are actual receipt amounts in FRI. Every receipt reported `SUCCEEDED` and
`ACCEPTED_ON_L2`.

| Action | Transaction | Block | UTC timestamp | Actual fee |
| --- | --- | ---: | --- | ---: |
| Screened shield | `0x1a30c0b6e8db645df67795f2356cb6a251b9680d0b61cea54d28d04edd70d1` | 14144027 | 2026-08-31T10:17:26Z | 2906846580843097760 |
| Buyer proposal | `0x5a287657f829a82ecc83a0543674f1397aa75bdade8365b28f72e3925d759b5` | 14144192 | 2026-08-31T10:22:02Z | 2782002980501155600 |
| Seller counter | `0x4126a3ee0971728d7181296fe4d128f12fc6a828a172b7bd12d8f5128364923` | 14144213 | 2026-08-31T10:22:37Z | 2782001530873223600 |
| Atomic settlement | `0x72adebfcffdfd45bba66d2152c7e11b50107aa0fe2b4c9f1ea851da16ab6c6d` | 14144242 | 2026-08-31T10:23:25Z | 2848863099822651008 |

The shield's durable Starkscan result contained nine proof facts, one L2-to-L1 message, and
`additional_data.signature`. The other three results contained no screening signature, as
required for non-deposit actions. Each one-time result is stored in a mode-0600 file under
the matching identity's mode-0700 `prover-jobs` directory. Proof blobs, signatures, job IDs,
request bodies, and key material are deliberately absent here.

## MCP negotiation and conservation

Two packaged MCP servers started against the seam backend and passed startup `doctor`. The
buyer policy proposed 0.8 STRK. The seller read it through its reverse channel and countered
at 0.8 STRK. The buyer accepted the seller-authored offer and settled atomically.

Post-settlement note discovery returned:

- Account A: one spendable 0.2 STRK change note.
- Account B: one spendable 0.8 STRK payment note.
- Selected input: 1 STRK.
- Conservation: `1.0 = 0.8 + 0.2` STRK.
- Spent nullifier: `0x172317f8d0625ad7fb0b662aab1e08662746079b37e984f525fa612a7d5e046`.

Both channel views reconstructed three messages for the same deal. Their settlement records
agreed on the accepted offer, acceptance, agreed amount, and paid amount.

## Recovery, observer, and disclosure

The MCP terminal closed after the buyer selected settlement, before its final output was
collected. No write was retried. `reconcile` classified shield, proposal, counter, and
settlement as committed effects with `next_action: none`; every chain effect, local state,
and result agreed.

The public observer inspected the settlement's 61 calldata felts and nine candidate note
salts. It recovered no plausible transcript and did not match the historical wire-v2 fixed
fifth-salt shape. This does not hide the submitting accounts, channel relationship, timing,
action shape, or note count.

Account A then created a recipient-bound, one-hour grant for this deal. Account B opened it
from a mode-0600 file and reconstructed exactly three offers plus the settlement. The
disclosed agreed and paid amounts both equalled 0.8 STRK. Grant creation and reveal are local
operations; they do not have transaction hashes and do not grant spending authority.

## Result

The bounded mainnet workflow passed: screened shield, note discovery, MCP negotiation,
atomic payment and change, crash-safe reconciliation, public-observer negative control, and
recipient-bound selective disclosure. This is one successful canary, not evidence of audit,
capacity, uptime, or safe handling of material value.
