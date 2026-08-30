# Mainnet Account C registration — 2026-08-30

Erebus deployed and registered a third identity with the canonical STRK20 mainnet pool.
The registration receipt emits an event from the pool address, so it is the third hash in
the sprint manifest that satisfies the hub's mechanical transaction check. This run did not
shield funds, open a channel, negotiate, settle, or disclose a deal.

## Environment

- Source commit: `a0dcba61a31092e937fb6b3ba07b1b440d646c95`
- Account: `0x06b293619b447480677ee2da22dbb7c442c4ae0251de9889f9cbb375ef51bad6`
- Pool: `0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a`
- Prover image: `strk20-prover:privacy-0.14.3-rc.2-arm64`
- Prover service version: `0.19.0-rc.2`
- Prover JSON-RPC version: `0.10.3-rc.2`
- Chain: `SN_MAIN`
- State and submission RPC: Alchemy Starknet mainnet v0.10
- Proving lag: 10 blocks

## Setup transactions

| Action | Transaction | Block | UTC timestamp | Network fee |
| --- | --- | ---: | --- | ---: |
| Fund C with 12 STRK | `0x013d191d71427b5c2711981c6dadf623751cd3ade4a561e4c8000e48f0e1ebc8` | `14111223` | `2026-08-30T18:45:21Z` | `0.060037180918668608 STRK` |
| Deploy C | `0x02f5fc32b770d74ab794f21321a78537ce1ef08fd828cd16c9272e6c54b516ca` | `14111321` | `2026-08-30T18:48:05Z` | `0.074410790805097696 STRK` |
| Approve 6 STRK | `0x00b8e3676d2fb247e85508e7f4748021c7b1b2f1f173d4a47e66c7073de4794e` | `14111571` | `2026-08-30T18:55:03Z` | `0.052047310203273456 STRK` |
| Add 1 STRK fee buffer | `0x0198336facc88ed9e77904e3fefa38b1b33cdf2af80353b7c2628e66f44d3c09` | `14111765` | `2026-08-30T19:00:29Z` | `0.046518787420720768 STRK` |

The 6 STRK allowance was confirmed on-chain and waited to 28 blocks of depth before proving.
The extra 1 STRK covered the difference between C's balance and the proof-aware maximum fee
bound. The two funding-transfer fees were paid by Account A, not C.

## Registration

| Transaction | Block | UTC timestamp | Execution | Finality | Network fee | Pool fee |
| --- | ---: | --- | --- | --- | ---: | ---: |
| `0x0159748de29d9dbe7016c9eb37459cbd7dd72290c3bc22412a034fb1c4c38a99` | `14111797` | `2026-08-30T19:01:22Z` | `SUCCEEDED` | `ACCEPTED_ON_L2` | `2.638313416045782912 STRK` | `6 STRK` |

The local prover prepared one `SetViewingKey` action against block `14111609`. The proof
contained nine proof facts and one pool message. A proof-aware dry run returned a maximum
network-fee bound of `5.933420523516909841 STRK`. The signed transaction was persisted before
submission and accepted without a retry.

Independent post-write reads confirmed that the pool public key is non-zero and matches the
protected Account C pool key. The receipt contains an event whose normalized emitter is the
canonical pool. C retained `4.235228482945845936 STRK` after the network and pool fees.

## Evidence boundary

Registration is public and irreversible. It writes the identity's pool key encrypted for
the pool auditor. It proves that the local prover, proof-fact transport, account signer, RPC,
and canonical pool completed a registration. It does not prove a mainnet shield, private
offer, settlement, recovery, or selective disclosure. Those full-workflow results remain on
Sepolia until operator screening access unblocks mainnet shielding.
