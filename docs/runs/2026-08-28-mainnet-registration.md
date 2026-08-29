# Mainnet registration canary — 2026-08-28

This run registered one Erebus identity with the canonical STRK20 mainnet pool. It is the
first Erebus transaction on mainnet, but it is not a shield, channel, negotiation, or
settlement canary.

## Scope

| Item | Value |
| --- | --- |
| Network | Starknet mainnet, `SN_MAIN` |
| Account A | `0x022764371563f7e6660f816605ed62edfe9b4bb3411036e61777336790e877f` |
| Pool | `0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a` |
| Pool class | `0x067dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d` |
| Prover | `PRIVACY-0.14.3-RC.2`, locally built ARM64 image |
| Prover RPC | localhost only, `127.0.0.1:3000` |
| State and submission RPC | Alchemy Starknet mainnet `v0_10` |
| Action | Standalone `SetViewingKey` |

The account and pool keys remained in mode-`0600` files outside the repository. The account
key was used only by the local Rust process. The pool key was sent to the local prover and to
Alchemy's `compile_actions` preflight, so both endpoints are inside this run's trust boundary.

## Compatibility checks

Before the write, an expired real proof bundle was sent through Alchemy's
`starknet_estimateFee`. The pool returned `PROOF_EXPIRED`, which proved that Alchemy preserved
and interpreted the current `PROOF1` facts. A fresh proof then passed the same fee-estimation
path.

The first six-thread submission attempt never reached signing or broadcast. The 20 GiB
Colima VM OOM-killed the prover with exit 137. Recreating the container with
`RAYON_NUM_THREADS=2` completed the next proof without changing the action or transaction
path. These are observations from this machine, not general proving-performance claims.

## Receipt

| Field | Value |
| --- | --- |
| Transaction | `0x06597adb6581bb1910d30b31139fe871665db4cc61fefef8120b89773528e54c` |
| Block | `14004848` |
| Block hash | `0x0322e38fe013cb997673626f72b7b4d0e9d6dc48be74acbeb786e4bd445bb16b` |
| Block time | `2026-08-28T17:17:52Z` |
| Execution | `SUCCEEDED` |
| Finality at verification | `ACCEPTED_ON_L2` |
| Pool fee | `6 STRK` |
| Network fee | `2.840485686189731776 STRK` |

The signed transaction was persisted before `starknet_addInvokeTransaction`. The receipt was
persisted separately. Both files remain mode `0600` outside the repository.

## Independent state checks

After acceptance:

- `get_public_key(Account A)` returned the public key derived from the protected pool-key
  file.
- The STRK allowance from Account A to the pool was `0`, proving that the exact 6 STRK
  approval was consumed.
- Account A's public STRK balance was `184.691083159243495520 STRK`, down from
  `193.531568845433227296 STRK` by exactly the pool fee plus the receipt's network fee.

## Post-run custody remediation

A later local diagnostic exposed the original Account A transaction signer. No pool key was
exposed. The signer was replaced on-chain before further funding or workflow transactions:

| Field | Value |
| --- | --- |
| Rotation transaction | `0x01aaf5909b7f18573514c5af9a030935f6f8760ef62b55106e3178f0701e4db` |
| Block | `14005902` |
| Execution | `SUCCEEDED` |
| New public signer | `0x0446eca69dea47377e4a844e922ed7f074873ad435286ed76ec4544aa19d6af9` |

An independent `get_public_key` call returned the new signer. The old signer is no longer
accepted by the account contract. The replacement account key and unchanged pool key remain
in separate mode-`0600` files outside the repository.

Fresh, undeployed B and C account credentials were generated after the same diagnostic, so
the exposed counterfactual credentials were never used on-chain. Dedicated pool-key files
and isolated runtime state directories now exist for A, B, and C; only A is funded,
deployed, and registered at this point.

## What this proves, and what it does not

This proves that the current Rust transaction construction, local RC.2 prover, Alchemy v0.10
proof-fact transport, account signing, and canonical mainnet pool can complete a registration
write together.

It does not prove shielding, screening access, note discovery, channel operations,
negotiation, settlement, recovery, or disclosure on mainnet. Registration is public and
write-once; it also encrypts the identity's pool key to the pool-wide auditor key.
