# Metropolis M4 Feasibility: Shielded Payment Primitives on Monad

Date: 2026-09-20. Status: research draft, not an M4 completion. Findings feed the M4 decision; the owner will edit.
Scope: EVM-verifiable shielded payment and agreement binding for the first Monad testnet release.
All URLs accessed 2026-09-20 unless a date is stated. Claims that could not be verified from a primary source are marked "unverified". Where sources conflict, both are given.

## 1. Monad facts

Verified against official Monad sources unless noted.

| Fact | Value | Source |
|---|---|---|
| Mainnet chain ID | 143 (`0x8f`), native currency MON | https://docs.monad.xyz/ai/current-facts, https://docs.monad.xyz/developer-essentials/network-information |
| Testnet chain ID | 10143 (`0x279f`), native currency MON | https://docs.monad.xyz/ai/current-facts, https://docs.monad.xyz/developer-essentials/testnets |
| Testnet reset | Reset from genesis on 2025-12-16; canonical contracts redeployed | https://docs.monad.xyz/developer-essentials/testnets |
| Node versions | Mainnet v0.16.1, testnet v0.16.2 (page read 2026-09-20) | https://docs.monad.xyz/ai/current-facts |
| EVM level | Bytecode-compatible with Ethereum as of the Fusaka fork; all Fusaka opcodes; client simulated against historical Ethereum transactions producing identical merkle roots | https://docs.monad.xyz/introduction/monad-for-developers |
| Block time | 300 ms target; mean ~302 ms measured September 2026 | https://docs.monad.xyz/ai/current-facts |
| Finality | Speculative 300 ms (1 slot); finality 600 ms (2 slots), stated as a protocol guarantee | https://docs.monad.xyz/ai/current-facts |
| Block gas limit | 150M observed (MIP-12 reduced it from 200M when block time dropped to 300 ms) | https://docs.monad.xyz/ai/current-facts, https://docs.monad.xyz/developer-essentials/changelog/releases |
| Per-transaction gas limit | 30M | https://docs.monad.xyz/ai/current-facts |
| Memory | Linear memory expansion; 8 MB per transaction cap | https://docs.monad.xyz/developer-essentials/opcode-pricing |
| Gas charging | Charged on gas limit, not gas used; EIP-1559; minimum base fee 100 MON-gwei | https://docs.monad.xyz/developer-essentials/gas-pricing |
| Contract code size | 128 KB max (256 KB init code), versus 24 KB on Ethereum | https://docs.monad.xyz/developer-essentials/differences |
| Transaction types | 0, 1, 2, 4. Type 3 (EIP-4844 blobs) not supported. EIP-7702 supported with Monad-specific restrictions | https://docs.monad.xyz/developer-essentials/transactions, https://docs.monad.xyz/developer-essentials/wallet-developers |

### Precompiles

Monad supports all Ethereum precompiles as of Fusaka (`0x01`-`0x11`) plus `0x0100` P256VERIFY (EIP-7951) and Monad-only staking/reserve precompiles at `0x1000`/`0x1001` (https://docs.monad.xyz/developer-essentials/precompiles).

Relevant to a privacy pool:

- `0x01` ecRecover: 6,000 gas (2x Ethereum). Usable for public-bound secp256k1 signature recovery.
- `0x02` is SHA-256, not keccak. Keccak-256 is the SHA3 opcode, unchanged in price. The frequently assumed "keccak precompile at 0x02" does not exist on Monad or Ethereum.
- `0x05` modexp: available, priced per evm.codes.
- `0x06`/`0x07`/`0x08` BN254 ecAdd/ecMul/ecPairing: available, repriced 2x/5x/5x. Published Monad schedule: ecAdd 300, ecMul 30,000, ecPairing 225,000 + 170,000 per point.
- `0x0a` KZG point evaluation: 200,000 (4x).
- `0x0b`-`0x11` BLS12-381 per EIP-2537, including `0x0f` `bls12_pairing_check`. The precompiles page lists no gas value for `0x0c`, `0x0e`, `0x0f` and defers to EIP-2537, so the Monad price of a BLS pairing check is unverified.

Derived, not measured: a four-pair BN254 pairing check costs 225,000 + 4x170,000 = 905,000 gas on Monad by the published schedule, versus 45,000 + 4x34,000 = 181,000 on Ethereum. A Groth16 verifier uses one multi-pairing of four pairs plus MSM/EC operations. An EVM Groth16 verifier that costs ~220,000 gas on Ethereum (measured, snarkjs: https://github.com/wizicer/zkp-solidity-gas) will therefore cost materially more on Monad under the 5x ecMul/ecPairing repricing. The exact number must be measured in the prototype; the roadmap's M4 evidence list already requires verifier gas.

### Monad-specific zk and privacy notes

- No Monad-specific verifier precompile or zk gas discount was found in the official docs. "zk verifier deployment costs" are not addressed as a topic by any primary source located; treat as unverified.
- The 128 KB code-size limit is relevant because third-party reports put an UltraHonk (Noir) Solidity verifier at ~33 KB, above Ethereum's 24,576-byte EIP-170 limit but below Monad's (third-party: https://www.luk3.tech/blog/hello-noir-part-2; unverified against a first-party Monad test).
- Monad's privacy page lists one supported provider, Unlink (https://docs.monad.xyz/tooling-and-infra/privacy). Unlink announced Monad testnet deployment on 2026-06-25 (https://www.unlink.xyz/blog/monad-native-privacy, https://blog.monad.xyz/blog/privacy-comes-to-monad-with-unlink). AnomaPay is live on Monad (https://anoma.net/blog/anomapay-is-now-live-on-monad); PriFi and Clean-Privacy were found but are unverified third-party projects.

## 2. Reusable privacy-pool candidates

No candidate found provides a first-class hook that constrains note spending by an external signed agreement commitment. The closest documented surface is Privacy Pools' public `context` input. Details below.

### Privacy Pools / 0xbow

- Source: https://github.com/0xbow-io/privacy-pools-core, Apache-2.0, created 2025-02-17, last push 2026-09-14 (GitHub API), 132 stars. Contracts, Circom circuits, relayer, TypeScript SDK in one monorepo. Contract `VERSION` string is `'0.1.0'` (https://docs.privacypools.com/layers/contracts/privacy-pools).
- Circuits: Circom; Poseidon commitment; label = keccak256(scope, nonce) as a public input; Groth16 with a completed trusted-setup ceremony (514 contributors per https://0xbow.io/; V2 ceremony at https://ceremony.privacypools.com shows "0 contributions" as of 2026-09-20, meaning not established).
- Operations exposed by the core docs and interfaces: deposit, withdrawal, ragequit. A partial withdrawal creates a change commitment; no internal transfer that creates a spendable note owned by a different user is documented. 0xbow marketing claims "arbitrary peer to peer private transactions", which the core docs do not substantiate. Treat the internal-transfer capability as unverified.
- Agreement binding: the withdrawal circuit takes `context` as a public input, where `context = uint256(keccak256(abi.encode(withdrawal, pool.SCOPE()))) % SNARK_SCALAR_FIELD` (https://docs.privacypools.com/protocol/withdrawal). This binds a proof to withdrawal parameters. It is not an external authorization hook. A custom Entrypoint/circuit could extend context construction, but that is a fork, not a configuration.
- Compliance gate: the Association Set Provider can exclude or revoke deposit labels after the fact; the original depositor has a ragequit path (https://docs.privacypools.com/layers/asp, https://docs.privacypools.com/layers/contracts/privacy-pools). This is deposit provenance policy, not per-deal spending policy.
- Monad: no 0xbow deployment on Monad was found. EVM contracts plus a BN254 Groth16 verifier are portable in principle; deployment would be the team's own work.
- Output decryptability: the withdrawal recipient receives a public transfer; the private artifact is a change commitment. No encrypted output note or memo channel to an arbitrary recipient note key is documented, so seller note recovery in the M4 sense is not provided.
- Note discovery: deposit label derivation plus chain events as described in the docs; no encrypted memo channel.

### Tornado Cash Nova class

- Source: https://github.com/tornadocash/tornado-nova, archived (read-only), last push 2022-07-21, no license metadata (GitHub API). Experimental arbitrary-amount pool on Gnosis Chain with shielded internal transfers; contracts were governance-upgradable by design.
- Reusable: the commitment/nullifier transfer pattern. Not reusable: no maintained code, no license, no agreement hook, no Monad deployment. Docs remain at https://docs.tornado-cash.com/.

### Railgun

- Sources: https://github.com/Railgun-Privacy/contract (no LICENSE file, GitHub license metadata null, last push 2026-08-15) and https://github.com/Railgun-Privacy/circuits-v2 (pushed 2026-07-27, GitHub license "Other/NOASSERTION"; raw `LICENSE` on `main` returns 404). Licensing is unresolved and is a blocker for a public release until clarified. Both repos are active.
- Design: Groth16/Circom, UTXO notes with commitments/nullifiers, encrypted note outputs, wallet-side note scanning with viewing keys, "Adapt modules" for private calls into DeFi (vendor profile: https://iptf.ethereum.org/vendors/railgun).
- Agreement binding: no external spend-policy or signed-agreement hook is documented. Adapt modules change what a private note can call, not who may spend it under what terms. Integration would require contract/circuit modification or a wrapper with its own audit.
- Output decryptability and discovery: this part matches the M4 requirement; recipient wallets scan events and decrypt memos with viewing keys.
- Monad: no Railgun Monad deployment found; unverified. EVM-portable in principle.

### Aztec: Connect legacy and the current network

- Aztec Connect is deprecated: deposits stopped 2023-03-21 and the sequencer stopped 2024-03-31 (https://docs.aztec.network/aztec_connect_sunset). A legacy Aztec Connect contract was exploited in June 2026 (news report, unverified beyond the report). The Aztec Network today is a separate L2 with client-side Noir proofs; its contracts and rollup are not deployable on an EVM L1. Reusable pieces are the toolchain only: Noir (Apache-2.0, https://github.com/noir-lang/noir) and Barretenberg/UltraHonk.
- zk.money is dead with Connect.

### zkBob

- Docs state withdrawal-only mode because of a vulnerability discovered 2026-06-05 (https://docs.zkbob.com/). Deployed on Polygon/Optimism/Tron, not Monad. Not a viable integration; relevant only as evidence of pool contract risk.

### Unlink

- The only privacy provider listed by Monad docs, deployed on Monad testnet, mainnet "coming soon" as of 2026-06-25 (https://docs.unlink.xyz/, https://www.unlink.xyz/blog/monad-native-privacy).
- Design: Groth16, encrypted UTXO notes, deposit/transfer/withdraw/execute, TypeScript SDK, gasless relaying.
- Trust model (primary source, https://docs.unlink.xyz/trust-model): spending key is EdDSA on BabyJubJub and stays client-side; viewing and nullifying keys are sent to the hosted Unlink Engine once at registration; the Engine builds the Groth16 proof from public state plus the client signature, then broadcasts. This is a remote prover and a trusted indexer. It conflicts with D01's local-proving default for the shielded EVM backend.
- Source availability: the GitHub org publishes only a `.github` repository; the contract source was not found publicly. The npm package `@unlink-xyz/sdk` latest is `0.0.2-canary.0` with no license field (npm registry metadata, fetched 2026-09-20).
- Agreement binding: no documented hook; the contract is closed. Output decryptability and discovery exist inside Unlink accounts but are Engine-assisted.

### Semaphore and RLN

Identity and anonymous-signaling systems: Semaphore proves group membership for signaling, RLN adds rate limiting; neither is a value-transfer pool. Listed on https://ethereum.org/en/privacy. Not candidates for settlement, and no further evaluation is needed.

### 2025-2026 adjacent work

- Kohaku (EF wallet privacy SDK) wraps Railgun, Tornado, and Privacy Pools clients for wallets; packages are labelled unaudited, the repository has no license metadata (https://github.com/ethereum/kohaku, GitHub API; WIP items in https://ethereum.github.io/kohaku/). Useful as integration clients, not as pools.
- PSE's Halo2 fork is archived (https://github.com/privacy-ethereum/halo2, archived, last push 2026-07-01); zcash/halo2 remains active. PSE's "Private Transfers" research report is listed as WIP (https://pse.dev/ecosystem). eERC is cited as amount-only in vendor material (https://build.avax.network/integrations/unlink); not independently evaluated here.

### Integration surface for agreement binding

Across candidates, agreement binding requires either (a) modifying the pool circuit and verifier to constrain a shared deal commitment, or (b) a separate verifier whose proof is atomically checked in the same transaction. None of the pools offers a production hook for it today. Privacy Pools' `context` field is the nearest documented public-input binding and is the least invasive starting point for a fork.

## 3. Proof systems

Gas figures are Ethereum measurements unless stated. Monad's 5x ecPairing/ecMul repricing will increase all pairing-based verifier costs; see section 1.

| System | Setup | Proof size | EVM verifier gas | Prover cost (attribution) | License / maturity |
|---|---|---|---|---|---|
| Groth16 | Per-circuit trusted setup | ~192-256 B | ~207k + 7.16k/public input (snarkjs formula, https://github.com/wizicer/zkp-solidity-gas); 210,565 gas measured (https://github.com/recmo/evm-groth16, 2023); ~221k snarkjs (same source) | Fastest SNARK tier; circom-ecdsa 1.5M-constraint circuit: 45 s prove, 934 MB proving key on AWS c5.4xlarge (0xPARC benchmark, 2021) | Toolchains mature; Circom/snarkjs GPL-3.0 |
| PLONK / UltraPlonk | Universal SRS | ~800 B | snarkjs ~291k measured; ~298k in arXiv 2409.01976; Aztec 2019 reported ~223k for TurboPLONK | Slower than Groth16 on non-native hashes; comparable on field-native gates | Mature; used by Aztec |
| Halo2 | Universal SRS (KZG) or none (IPA) | 3-10 KB IPA; smaller with KZG | PSE KZG verifier ~305-321k function gas depending on config (https://hints.plonk.pro/gas) | secp256k1 ECDSA: 2.0-3.5 s prove on M2 Max (axiom-crypto/halo2-lib benchmarks) | PSE fork archived; zcash/halo2 active; license metadata NOASSERTION |
| Plonky2 / Plonky3 | Transparent | ~100 KB+ class | Not practical directly; requires wrapping | IACR 2026 framework: Plonky3 among smallest proving time and RAM at 8 CPU cores (https://iacr.org/news/item/29329) | Apache-2.0 (Plonky3 GitHub API); active |
| Noir + Barretenberg (UltraHonk) | Universal SRS, no per-circuit ceremony | ~7.5 KB (tiny) to ~14-16 KB (https://www.luk3.tech/blog/hello-noir-part-2; https://satsbridge.com/bitcoin-clock.pdf) | ~1.6M gas per shielded transaction (third-party project docs, https://docs.sanect.com/guide/privacy; unverified) | 98,578 gates: 1.77 s mean prove, 419 ms witness, 40 ms verify on AMD Ryzen 7 4700U (satsbridge paper); EVM verifier contract ~33 KB (luk3; unverified) | Noir Apache-2.0, active |
| Circom + snarkjs | Both of the above | per system | per system | Widely benchmarked (IACR 2023/681 covers Poseidon, Pedersen, MiMC, SHA-256, ECDSA, EdDSA, Keccak) | GPL-3.0 for compiler and snarkjs (GitHub API) |
| SP1 / RISC Zero (zkVM) | Wrap STARK in Groth16 | ~256 B wrapped | SP1 Groth16: 327,621 gas for one real transaction (~286k verifier, remainder calldata/gateway) (third-party, https://github.com/blokzdev/blokz, June 2026; unverified) | zkVM proving costs far above equivalent hand-written circuits; "proof-wrapping tax" (same source) | Apache-2.0; active; RISC Zero verifier router docs at https://dev.risczero.com/api/blockchain-integration/contracts/verifier |

Monad support: BN254 pairing (`0x08`) and BLS12-381 pairing (`0x0f`) both exist, so Groth16/PLONK over either curve is executable on Monad in principle. The practical constraint is gas, not opcode availability. The 8 MB per-transaction memory cap is not expected to bind a verifier; the 30M per-transaction gas limit is the relevant budget, and its adequacy for a Groth16 verifier with agreement constraints is a prototype measurement.

Assessment for this project: Groth16 + Circom is the lowest-verifier-cost and best-precedented route (Privacy Pools, Railgun, Tornado), at the cost of a per-circuit ceremony. UltraHonk/Noir removes the per-circuit ceremony and has good local-proving ergonomics, at roughly the gas cost reported above and a larger verifier contract that Monad's 128 KB limit accommodates.

## 4. Hash and signature choices inside the circuit

Constraint benchmarks are not comparable across papers; attribute and measure.

- Poseidon (BN254): ~240 constraints per hash from a circomlib/ethresear.ch benchmark (https://ethresear.ch/t/gas-and-circuit-constraint-benchmarks-of-binary-and-quinary-incremental-merkle-trees-using-the-poseidon-hash-function/7446); the Poseidon paper reports 7,290 constraints for a depth-30 Merkle tree, i.e. ~243 per hash (https://eprint.iacr.org/2019/458). Poseidon2 reduces linear-layer constraints further (https://eprint.iacr.org/2023/323).
- SHA-256: ~25,000-27,500 constraints per compression, derived from 826,020 constraints for a depth-30 tree in the Poseidon paper table; a third-party doc states ~25,000 (https://docs.orbinum.network/architecture/zk-proofs).
- Keccak-256: sources conflict by roughly 6x. vocdoni's Circom implementation reports 150,848 constraints for a 32-byte input (https://github.com/vocdoni/keccak256-circom, GPL-3.0, experimental); a project spec claims ~24,000 with lookups (https://github.com/SolanaAEP/saep spec; unverified). Both are order-of-magnitude above Poseidon. Treat the exact number as a prototype measurement.
- Pedersen: 41,400 constraints for a depth-30 tree, i.e. ~1,380 per hash (Poseidon paper table); superseded for new work but present in older circuits.
- secp256k1 ECDSA in-circuit: circom-ecdsa full verification is 1,506,136 R1CS constraints, 45 s proving, 934 MB proving key on a 16-core server (https://github.com/0xPARC/circom-ecdsa, GPL-3.0, 2021 benchmark). Halo2 implementations prove in 2.0-3.5 s on an Apple M2 Max (axiom-crypto/halo2-lib benchmarks). One ECDSA verification dominates any circuit that is otherwise a few hundred thousand constraints.
- EdDSA over Baby Jubjub is field-native to BN254 (ERC-2494, https://eips.ethereum.org/EIPS/eip-2494), the scheme used by Semaphore/MACI identities and by Unlink's spending key. No precise constraint count from a primary benchmark was located; qualitatively it is far cheaper than secp256k1 ECDSA. Measure in the prototype.
- Pasta/BN254 alternatives: ECDSA on a circuit-native curve exists in research toolchains but was not evaluated here.

Implication for the M1 suite registry. Suite 1 (keccak256 commitment + secp256k1 ECDSA) is provable in-circuit but sits at roughly 1.5M constraints per ECDSA verification plus ~24k-151k per keccak, before any pool logic. That is feasible for a server, marginal for the M4/M5 consumer-laptop local-proving target. A second authorization suite bound to the same canonical terms appears to be the realistic shielded-path design: for example a Poseidon (or Poseidon2) commitment with EdDSA/BabyJubjub authorization for deals that require hidden parties and amounts, while suite 1 remains the public-bound V1 path. Any second suite must:

- commit to `suite_id` in both the commitment preimage and the signed payload so a signature cannot move between suites;
- define the same deal-consumption identity `Ndeal` derivation independent of suite, or explicitly bind the identity to the suite, with replay tests across suites and revisions (M0 D02);
- define the same canonical `D` encoding mapped into each suite's commitment function (byte encoding versus field-element encoding), with cross-language vectors.

The Privacy Pools pattern is precedent: keccak is computed outside the circuit as a public input (`context`, `label`), while the field-native Poseidon commitment is verified inside. Where the agreement terms must stay hidden, however, the commitment preimage is private and must be hashed in-circuit, which forces a circuit-friendly hash or an explicit "commitment computed outside the circuit" redesign.

## 5. Proving-key lifecycle and artifact reproducibility

Established practice found:

- Groth16 artifact chain: universal or circuit-set phase-1 powers-of-tau (`.ptau`), per-circuit phase-2 `.zkey`, exported verification key; `snarkjs` is the common CLI (https://github.com/iden3/snarkjs; walkthrough: https://github.com/apeoverflow/groth16-setup). Groth16 requires a per-circuit setup; PLONK/Halo2-KZG/UltraHonk reuse a universal SRS.
- Ceremony precedents: Ethereum's KZG ceremony for the KZG SRS (https://ceremony.ethereum.org/, resources: https://github.com/ethereum/kzg-ceremony); the Perpetual Powers of Tau lineage is commonly reused for Circom setups (cited as the canonical universal path in a 2026 survey: https://shattered.io/groth16-vs-plonk-zk-snark-2026); 0xbow reports 514 contributors for its Privacy Pools ceremony and runs a V2 ceremony (https://0xbow.io/, https://ceremony.privacypools.com). Academic cautions: SRS verification by users is not optional (https://eprint.iacr.org/2025/2000), and setup protocols versus ceremonies are systematized in https://eprint.iacr.org/2025/064.
- Versioned on-chain verification keys: RISC Zero ships a verifier router with named verifier versions (https://dev.risczero.com/api/blockchain-integration/contracts/verifier); SP1 uses a verifier gateway (third-party description, https://github.com/blokzdev/blokz). Privacy Pools pins immutable verifier addresses per pool and provides `windDown` (https://docs.privacypools.com/layers/contracts/privacy-pools). Railgun deploys behind an EIP-1967 transparent proxy with a `proxyAdmin` (deployment output in https://docs.railgun.org/developer-guide/engine-1/getting-started-with-the-contracts), an upgradeable pattern with explicit admin trust.
- A testnet release would need to document: circuit source commit, compiler and proving-tool versions, phase-1 file source and hash, phase-2 zkey hash, verification-key hash, verifier contract address and deployed bytecode hash, the ceremony transcript verification procedure, the upgrade authority and its procedure for changing verifier versions, and an offline, pinned artifact-install procedure. The roadmap already requires versioned proving artifacts and reproducible installation for M4/M5; this memo does not add requirements beyond recording sources for how they are normally done.

## 6. Integration route and open prototype questions

Most credible first route. A custom (or Privacy Pools-derived, Apache-2.0) BN254 Groth16 pool with a transfer/withdrawal circuit extended to constrain the agreement predicate and a `context`-style public input carrying the deal commitment. Rationale: it is the only route found that can put agreement binding inside the verified transition; the contracts and circuits are open source under a permissive license; the verifier is small enough for the chain budget (pending measurement under Monad's repricing); and it can be proven locally. Its costs are the trusted setup, a circuit fork, and the fact that Privacy Pools v0.1.0 has no recipient output-note flow, so output decryptability and note discovery must be designed as part of the fork or reproduced from a Railgun-class design.

Routes not recommended as-is:

- Unlink: the only deployed Monad rail, but closed contract, hosted prover, no agreement hook; conflicts with D01's local-proving default and cannot satisfy M4's agreement requirement without vendor cooperation.
- Railgun: strongest existing shielded-transfer and note-discovery design, but licensing is unresolved (no license on contracts, NOASSERTION on circuits) and there is no agreement hook.
- 0xbow as-is: no recipient note creation; withdrawal reveals amount and recipient publicly; would require a fork.
- zkBob: withdrawal-only after a 2026-06 vulnerability.
- Tornado Nova: unmaintained, unlicensed, 2022-era.

Open questions for the prototype (each answerable by measurement or yes/no):

1. Can the agreement predicate (commitment opening, both authorizations, amount/asset/recipient equality, deal nullifier) be added to the pool circuit such that end-to-end local proving completes under a stated budget (target: one proof per private payment on an Apple Silicon or x86 laptop, no GPU)? Measure time, peak RAM, proof size.
2. What is the exact verifier gas on Monad testnet for the chosen circuit with its public-input count, given the 5x ecPairing/ecMul repricing, and does it fit the 30M per-transaction limit with the rest of the settlement transaction? Measure, do not derive.
3. Does a Groth16 verifier plus pool contract deploy within Monad's 128 KB code-size limit without library splitting, and what is the deployment gas cost at the 100 MON-gwei minimum base fee? Yes/no plus measurement.
4. Is the Monad BLS12-381 pairing precompile (`0x0f`) actually usable for a production verifier, and is its gas schedule published anywhere beyond the EIP-2537 deferral on the precompiles page? Yes/no, and cite.
5. Does a second authorization suite (field-native commitment plus EdDSA/BabyJubjub authorization) over the same canonical terms produce deterministic cross-language vectors, and do cross-suite and cross-revision replay tests reject as specified in M0 D02? Yes/no with test vectors.
6. Can the deal commitment be computed outside the circuit and bound as a public input (Privacy Pools `context` pattern) without exposing amount or recipient, or is in-circuit hashing of private terms required? Measure both variants' constraint counts and proving time before fixing the M1 suite registry.
7. For the chosen fork, can a payment create a spendable recipient note whose owner decrypts it and spends it after the buyer goes offline, using only chain events and local keys, with no hosted service? End-to-end test.
8. What is the exact license for Railgun contracts and circuits (no license file, NOASSERTION metadata), and does any spend-authorization hook exist for an external agreement commitment? Legal yes/no plus code review; this memo treats both as unresolved.
9. Will Unlink expose the contract source or an agreement-binding extension, and does Monad mainnet deployment still hold to the 2026-06 testnet announcement? Vendor yes/no; re-check before any dependency.
10. What is the measured proving time, memory, proof size, and Monad verifier gas for the exact M4 prototype circuit on the declared target hardware? This is the M4 acceptance measurement.

## What this does not establish

- No Monad deployment, testnet transaction, or verifier deployment was performed or measured. All Monad gas implications are derived from published schedules.
- No Monad-specific zk verifier support or cost guidance was found; the absence is an absence of evidence, not evidence of absence.
- No circuit was compiled. Constraint counts and proving figures come from third-party benchmarks and are not comparable across tools.
- No license conclusion is reached for Railgun, Kohaku, Tornado Nova, or Unlink; licensing statements here are observations of repository metadata and must be reviewed by the owner.
- No claim is made that Privacy Pools v0.1.0 can or cannot perform internal peer-to-peer transfers; the core docs do not document it.
- No agreement-binding design is selected. Section 6 states a provisional route and the experiments that would confirm or reject it.
- Unlink's Monad mainnet status and contract source availability were not re-verified beyond the sources cited.

## Decisions this memo feeds (for owner review)

- M4 route: forked/custom pool versus vendor rail, with the agreement-binding requirement kept explicit.
- Proof system and circuit language: Groth16/Circom versus UltraHonk/Noir, with the ceremony burden and verifier gas trade-off recorded as a judgment by the owner.
- M1 suite registry: whether a second shielded-path suite is authorized, and how `Ndeal` and cross-suite replay rules are stated.
- Hash and signature suite: keccak/secp256k1 for the public path, and the shielded-path counterpart.
- Proving-key lifecycle: ceremony scope, artifact pinning, verifier versioning, and upgrade authority before any testnet contract is treated as reviewable.
- M8 deployment assumptions: Monad versions and parameters re-verified at deployment time, not taken from this memo.
