# Metropolis M4 Decisions: Shielded Transfer Proof

Date: 2026-09-24. Scope: a runnable agreement-bound private-transfer proof prototype.
Implementation: [`circuits/m4`](../circuits/m4). The [feasibility memo](metropolis-m4-feasibility.md)
records the alternatives considered. These decisions select the M5 pool direction; they do not
make the current prototype a custodial payment system.

## Decision

Build a small Erebus note pool on EVM. The evaluated rails do not expose a verified hook that
both constrains the accepted Erebus deal and creates a spendable private note for its recipient.
The prototype therefore owns one combined agreement-and-transfer relation. Reuse the audited
ideas and library primitives where possible, but do not import a third-party pool's accounting
claims without its complete transition model.

| Component | M4 selection |
|---|---|
| Proof | BN254 Groth16 |
| Circuit | Circom 2.2.3 with circomlib 2.0.5 |
| Field hash | circomlib Poseidon, with numeric domain tags 2001-2009 |
| Agreement authorization | BabyJubJub EdDSA over Poseidon, buyer and seller role tags distinct |
| Shielded agreement suite | Reserved suite ID 2; current Rust core still rejects it until M5 implements the mapping and authorization API |
| Tree | Poseidon binary tree, depth four in the measured prototype; M5 must select the live depth and root history |
| Prover | Local Node/snarkjs reference implementation; no witness sent to a relayer |
| Setup | Single-contributor local test setup, discarded for any public-value deployment |

Circom's compiler is GPL-3.0 and circomlib is LGPL-3.0. Their use and artifact distribution
need a license review before packaging the M5 release. The project does not infer a production
license conclusion from the npm package metadata alone.

## Suite 2 and the M1 boundary

Suite 1 hashes the whole canonical byte encoding with keccak256. Suite 2 uses a typed Poseidon
field map because proving two secp256k1 signatures and keccak over variable-length terms would
dominate the private transfer circuit. The canonical M1 terms remain the offchain agreement and
disclosure format. An M5 Rust encoder must validate them, map them to the fields below exactly,
and compute the same suite-2 commitment as the circuit before either party signs. Until that
encoder and its cross-language vectors exist, suite 2 is reserved, not selectable in the SDK.

The current proof shape fixes protocol version 1, suite 2, shielded mode, guarantees
`hidden-amount | hidden-recipient | agreement-bound-settlement` (`0x7`), and a zero fee. An M1
agreement with another mode, an unknown guarantee, or a nonzero fee is outside this proof shape.

| M1 field | Suite-2 field map |
|---|---|
| `domain` | `eip155:<chainId>`, with `pool` and `settlement_contract` both equal to the pool address; `verifier_version` is a public `u32` |
| `deal_id`, `revision` | 128-bit ID and 32-bit positive revision |
| `transcript_root` | Two 128-bit big-endian halves, kept private |
| buyer/seller authorization keys | Two canonical 32-byte BabyJubJub coordinates each |
| `payment_recipient` | Seller-chosen 32-byte spend tag `Poseidon(2006, spend_secret)` |
| `asset` | Canonical `eip155:<chainId>/erc20:0x<40 lowercase hex>`; the 160-bit address is public and fixed by the one-asset pool |
| `amount`, `expiry` | 128-bit positive base units and 64-bit Unix seconds; expiry is public for contract checking |
| `fee_policy` | Zero fee and absent recipient in this first circuit |
| `settlement_nonce` | Two 128-bit big-endian halves, fixed across revisions |
| `service` | SHA-256 of `ServiceRecord::encode()`, split into two 128-bit big-endian halves |
| commitment blinding | Fresh random nonzero canonical BN254 scalar (`0 < b < r`), encoded as 32 bytes |

Service bytes are checked outside the circuit. The circuit binds their 256-bit digest into both
role authorizations; it does not prove that a service was delivered. The M4 runner derives that
digest from the existing M1 fixture and checks its byte encoding against the pinned Rust vector.
It likewise binds the transcript root, while M2 remains responsible for its derivation.

Let `P(x...)` be circomlib Poseidon over exactly the listed field elements. The numeric constants
are fixed tags, not interchangeable labels:

```text
domain = P(chainId, poolAddress, verifierVersion)
paymentSalt = P(2009, domain, settlementNonceLo, settlementNonceHi)
A = P(2001, domain, dealId, revision, transcriptRootLo, transcriptRootHi)
B = P(serviceDigestLo, serviceDigestHi, nonceLo, nonceHi, asset, amount)
C = P(buyerAx, buyerAy, sellerAx, sellerAy, recipientSpendTag, paymentSalt)
Cdeal = P(2002, A, B, C, blinding, expiry)
Ndeal = P(2003, domain, buyerAx, buyerAy, nonceLo, nonceHi)
buyerMessage = P(2004, domain, Cdeal)
sellerMessage = P(2005, domain, Cdeal)
```

`Ndeal` omits revision and amount. Every signed revision under one suite-2 deployment and nonce
therefore competes for one consumed identity. Suite 1 and suite 2 use different contract domains
and signatures; switching suites is a new authorization, never a silent revision or fallback.

## Note and transition

```text
spendTag = P(2006, spendSecret)
note = P(2007, asset, amount, ownerAx, ownerAy, spendTag, salt)
noteNullifier = P(2008, spendSecret, note)
merkleParent = P(left, right)
```

The payer proves possession of the input note's spend secret and a membership path to a public
accepted root. The input note owner is the buyer authorization key. The input nullifier prevents
spending the same note again; the separate deal nullifier prevents paying a signed deal again
with another note. Amounts are range constrained to `u128`, and `inputAmount = dealAmount +
changeAmount` is enforced in the field without overflow. The asset is the same for input,
payment, and change. The current circuit has one input and one payment/change pair.

The seller chooses `recipientSpendTag` using a secret held locally and verifies that the tag in
the private deal matches that secret before signing. The payment note is constrained to the
seller's authorization key, the deal amount and asset, that tag, and the deterministic
`paymentSalt`. The seller already has every opening field and can recompute the payment
commitment to scan chain events after restart, without relying on a ciphertext or the buyer
remaining online. The buyer knows the tag but cannot spend the output without its preimage.
M5 must implement persistent note scanning, backup, and an actual recipient spend/withdrawal.

For a future nonzero fee, extend the relation with a fee output fixed by the signed fee policy
and enforce `input = payment + change + fee`, all in the same asset. For multiple inputs and
assets, perform conservation separately for each asset and range constrain every input and
output. The current zero-fee circuit must reject any attempt to use it for a fee-bearing deal.

## Public and private schemas

The generated verifier reads these eleven public field elements in this exact order:

| Index | Field | Contract check |
|---:|---|---|
| 0 | `chainId` | equals `block.chainid` |
| 1 | `contractAddress` | equals `address(this)` |
| 2 | `verifierVersion` | equals deployed version |
| 3 | `asset` | equals the pool's immutable ERC-20 address |
| 4 | `dealCommitment` | event and audit handle |
| 5 | `dealNullifier` | unused, then consumed |
| 6 | `root` | accepted tree root |
| 7 | `inputNullifier` | unused, then consumed |
| 8 | `paymentCommitment` | appended output |
| 9 | `changeCommitment` | appended output |
| 10 | `expiry` | `block.timestamp < expiry` |

Private witness: deal ID and revision; transcript and service digest limbs; settlement nonce
limbs; amount and blinding; buyer and seller keys and EdDSA signatures; recipient spend
tag; input amount, salt, spend secret, and four Merkle siblings/directions; change amount, spend
tag, and salt. `paymentSalt` is derived in the circuit. Neither accepted amount nor recipient
key is a public input. The asset is visible through the pool. A valid proof binds both
authorizations and the payment output to the same private amount, public asset, and recipient.

The `PrototypeSettlement` harness checks public domain, expiry, root, and both nullifiers,
verifies the proof, then records consumed identities and output commitments in one EVM
transaction. It has no ERC-20 custody or live tree update; a valid harness transaction is proof
evidence, not a transfer of value. M5 must replace the immutable test root with a pool-managed
tree, bind deposits to notes and withdrawals to vault balances, and make output insertion
spendable. The verifier and state transition must remain atomic.

## Negative vectors and artifact lifecycle

The pinned positive public vector is [`circuits/m4/fixtures/public.json`](../circuits/m4/fixtures/public.json).
The runner also checks these mutations against the actual circuit or Solidity verifier:

| Mutation | Expected failure |
|---|---|
| Amount 70 to 71, same output | Conservation or committed payment mismatch |
| Seller key changed | Commitment and authorization mismatch |
| Accepted root changed | Membership mismatch |
| Buyer signature changed | Role authorization mismatch |
| Change 80 to 79 | Conservation mismatch |
| Expiry changed | Commitment mismatch |
| Zero blinding or spend secret | Circuit rejects before accepting a proof |
| Zero recipient spend tag | Circuit rejects an unusable note destination |
| Public payment commitment changed | Solidity verifier returns false |
| Same proof submitted twice | Deal or note nullifier already consumed |

`npm run prototype` pins Circom/npm/solc versions, rebuilds circuit artifacts, and regenerates a
local setup when the R1CS hash changes. Generated files and test secrets stay under ignored
`circuits/m4/build/`. The test setup has one known contributor and supplies **no security
against toxic-waste compromise**. A public testnet build needs a reviewed setup ceremony or
a different proving system, pinned setup input and zkey hashes, a verification-key hash,
verifier bytecode hash, source/toolchain commit IDs, and an artifact verification command.
Changing the circuit or suite requires a new verifier version and deployment. M5 owns those
release artifacts and the local Rust witness builder.

Sources: [Circom setup and verifier workflow](https://docs.circom.io/getting-started/proving-circuits/),
[circomlib](https://github.com/iden3/circomlib), and
[Monad precompile pricing](https://docs.monad.xyz/developer-essentials/precompiles).
