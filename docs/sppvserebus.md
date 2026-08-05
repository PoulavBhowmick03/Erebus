# STRK20, Erebus, and Stellar Private Payments

This document compares the note and pool models used by STRK20 and Stellar
Private Payments (SPP), and separates upstream STRK20 capabilities from the
current Erebus MVP.

The source snapshots reviewed were:

- Erebus `0ad5850`
- Stellar Private Payments `9e53ecf`

This is a source review. It does not contain live proving benchmarks, fee
measurements, or a fresh end-to-end execution of either protocol.

## Bottom line

Both systems use the same high-level economic model:

```text
public tokens
    ↓ deposit/shield
pool contract holds the tokens
    ↓
private ownership is represented by notes
    ↓ spend
old notes become spent via nullifiers
new notes represent the recipient's value and optional change
```

The pool and the notes are therefore not competing designs:

- The **pool** is the contract that holds the real tokens and enforces value
  conservation.
- A **note** is a private claim on some of those pooled tokens.
- A **nullifier** is the public, one-time marker that prevents a note from being
  spent twice.

The decisive architectural difference is how note state is represented:

- **SPP** publishes a commitment for every note in an append-only Merkle tree.
  A fixed Groth16 JoinSplit circuit proves membership and spends notes from that
  tree.
- **STRK20** does not publish a global note-commitment tree. Each note is stored
  at a storage key derived from channel secrets, and a general Cairo execution
  proves an ordered sequence of privacy actions against historical Starknet
  state.

The distinction drives the differences in discovery, synchronization, proof
generation, transaction shape, composability, and metadata leakage.

Useful starting points:

- STRK20 note identity and nullifier: `sdk/rs/src/hashes.rs:142`
- STRK20 stored-note decoding: `sdk/rs/src/decrypt.rs:103`
- STRK20 action ordering: `sdk/rs/src/actions.rs:36`
- STRK20 proof pipeline: `sdk/rs/src/execution.rs:1`
- SPP transaction circuit:
  `/Users/odinson/Developer/stellar-private-payments/circuits/src/policyTransaction.circom:19`
- SPP pool transition:
  `/Users/odinson/Developer/stellar-private-payments/contracts/pool/src/pool.rs:561`
- [STRK20 pool model](https://strk20-by-example.org/what-is-strk20)
- [STRK20 notes and nullifiers](https://strk20-by-example.org/notes-and-nullifiers)

## SPP notes

An SPP note has private logical data:

```text
amount
note private key
note public key = derive(note private key)
blinding
leaf index / Merkle path, once inserted
```

The public note commitment is:

```text
commitment = Poseidon2(amount, note_public_key, blinding)
```

The private key is **not** an input to the commitment hash. The circuit derives
the note public key from the private key and proves that the spender knows the
corresponding private key:

```text
private note key
    ↓ derive inside circuit
note public key
    ↓
Poseidon2(amount, note public key, blinding)
    ↓
must equal the public commitment in the Merkle tree
```

See `policyTransaction.circom:51-80` in the SPP repository.

SPP publishes two related but separate objects:

1. The commitment, which is inserted into the public Merkle tree.
2. An encrypted output payload containing `amount || blinding`, which lets the
   recipient discover that the commitment belongs to them.

The ciphertext is not the commitment. The recipient trial-decrypts ciphertexts,
recovers the amount and blinding, and recomputes the commitment before accepting
the note. See `sdk/prover/src/notes.rs:24` and
`sdk/prover/src/encryption.rs:233` in the SPP repository.

## STRK20 notes

STRK20 does not store a public commitment of the SPP form. Its logical note data
includes:

```text
channel key
token
sequential note index
amount
salt
owner's pool key material
```

Not all of those fields are stored together. They play three distinct roles.

### 1. The storage location

The note's storage key is:

```text
note_id = H(
    NOTE_ID_TAG,
    channel_key,
    token,
    index,
    0,
)
```

See `sdk/rs/src/hashes.rs:142`.

The channel key is secret. A wallet that knows the channel key, token, and next
sequential index can derive the exact storage slot. An unrelated observer cannot
efficiently enumerate a global list of note commitments because no such list is
maintained by the protocol.

### 2. The stored value

At that location, an encrypted note stores one packed field element:

```text
packed_value = salt * 2^128 + encrypted_amount
```

For ordinary encrypted notes:

```text
mask = low128(H(
    ENC_AMOUNT_TAG,
    channel_key,
    token,
    index,
    0,
    salt,
))

encrypted_amount = amount + mask mod 2^128
```

The recipient recovers the amount with:

```text
amount = encrypted_amount - mask mod 2^128
```

See `sdk/rs/src/decrypt.rs:103` and `sdk/rs/src/hashes.rs:186`.

Therefore, “encrypted value at a secret-derived location” means specifically:

- The **amount** is additively masked.
- The **salt** occupies the high bits of the stored field and is public once the
  storage slot or server action is observed.
- The **token, channel key, and index** participate in locating the note and
  deriving its amount mask; they are not all concatenated into the stored value.
- Ownership is not represented by placing a private key inside the note. It is
  established when the proof successfully uses the owner secret and the note's
  channel/location information.

Open notes are an exception. A note with the reserved salt `1` stores its amount
in plaintext. STRK20 uses open notes for flows whose output amount is determined
by an external contract during execution.

### 3. The spend marker

The nullifier is:

```text
nullifier = H(
    NULLIFIER_TAG,
    channel_key,
    token,
    index,
    0,
    owner_private_key,
)
```

See `sdk/rs/src/hashes.rs:153`.

This separates three concepts that SPP combines around a commitment tree:

```text
STRK20 note location  = secret-derived note_id
STRK20 note value     = packed salt + masked amount
STRK20 spend marker   = owner-secret-derived nullifier
```

## Architectural comparison

| Dimension                | STRK20                                                                 | Stellar Private Payments                                                  |
| ------------------------ | ---------------------------------------------------------------------- | ------------------------------------------------------------------------- |
| Custody                  | One shared privacy pool can handle multiple token contracts            | Each pool constructor fixes one token contract                            |
| Public note state        | Secret-derived storage slot containing a packed salt and masked amount | Poseidon2 commitment in a public append-only Merkle tree                  |
| Ownership secret         | Pool/viewing-spending key plus channel information                     | Note private key corresponding to the committed note public key           |
| Note discovery           | Derive exact locations from channel key, token, and sequential index   | Scan commitment events and trial-decrypt every output ciphertext          |
| Membership proof         | Prove the Cairo execution's reads against historical Starknet state    | Prove a Merkle authentication path inside the Groth16 circuit             |
| Spend marker             | Nullifier derived from channel/location data and owner private key     | Nullifier derived from commitment, leaf position/path data, and owner key |
| Proof system             | Cairo execution proof using Stwo/STARKs                                | Circom circuit using Groth16 over BN254                                   |
| Verification             | Proof facts are checked through Starknet's protocol/sequencer path     | Pool invokes a separately deployed Groth16 verifier contract              |
| Transaction shape        | Variable-length, phase-ordered action program                          | Circuit and keys are fixed to two inputs and two outputs                  |
| Change                   | Add a new note back to the sender                                      | Use one of the two output slots for change                                |
| Multi-asset              | Per-token accounting within one pool/action program                    | Pool and commitment tree are specific to one token                        |
| Private DeFi             | One external invocation can be included through an anonymizer          | Current circuit expresses deposit, transfer, and withdrawal               |
| Current proving boundary | Erebus sends its pool private key to its prover and preflight RPC      | SPP generates the Groth16 witness and proof locally                       |
| Trusted setup            | No circuit-specific trusted ceremony                                   | Groth16 keys are circuit-specific and require a production ceremony       |
| Current maturity         | STRK20 is deployed; Erebus remains pre-MVP                             | WIP testnet reference implementation, unaudited                           |

## What “variable ordered action sequence” means

STRK20 does not prove only one hard-coded equation such as “two notes become two
notes.” It proves the execution of a list of typed client actions.

The action families are:

```text
phase 0: SetViewingKey
phase 1: OpenChannel
phase 2: OpenSubchannel
phase 3: Deposit
phase 4: UseNote
phase 5: CreateEncNote / CreateOpenNote
phase 6: Withdraw
phase 7: InvokeExternal / ComputeAndInvoke
```

See `sdk/rs/src/actions.rs:36`.

“Variable” means that the list can contain different numbers of actions. For
example, a private payment could be:

```text
UseNote(Alice note 100)
CreateEncNote(Bob 60)
CreateEncNote(Alice change 40)
```

A fragmented wallet could consume three notes in the same proved execution:

```text
UseNote(Alice note 2)
UseNote(Alice note 3)
UseNote(Alice note 5)
CreateEncNote(Bob 10)
```

A shield-and-transfer operation could include:

```text
Deposit(token, 100)
CreateEncNote(Bob 60)
CreateEncNote(Alice surplus 40)
```

The list is not arbitrarily ordered. Phases must be non-decreasing. In
particular, consumed notes must appear before newly created notes. An action set
that creates a note and then tries to use an old note is rejected as out of
order.

The current rules also require:

- At least one storage `WriteOnce`, which supplies replay protection.
- Per-token temporary balances that never create value and finish balanced.
- At most one invoke-phase action, and it must be last.
- Sequential note indexes within each channel/subchannel.

See `sdk/rs/src/action_set.rs:1`.

Erebus demonstrates why this is more than theoretical generality. Its atomic
accept-and-settle operation can consume several payment notes and create:

- One value-bearing payment note.
- Five zero-value encrypted notes carrying the authenticated acceptance record.

All of those writes share one action set and one proof. See
`sdk/rs/src/channel.rs:515`.

## What “fixed 2-input/2-output JoinSplit” means

SPP's base Circom template is parameterized by `nIns` and `nOuts`, but the
transaction entry points, proving keys, and deployed verifier currently pin
those parameters to `2` and `2`.

Every accepted pool transaction therefore proves this shape:

```text
up to 2 input notes
    ↓ consume and nullify
exactly 2 output commitment slots
```

The circuit enforces:

```text
input_0.amount
+ input_1.amount
+ public_amount
= output_0.amount
+ output_1.amount
```

`public_amount` represents the public leg:

- Positive for a deposit.
- Zero for a fully private transfer.
- Negative for a withdrawal.

Unused input or output slots are filled with zero-value dummy notes. This keeps
the public circuit shape constant.

Examples:

```text
Deposit 100

inputs:   [dummy 0, dummy 0]
public:   +100
outputs:  [Alice 100, dummy 0]
```

```text
Private transfer 60 from a 100 note

inputs:   [Alice 100, dummy 0]
public:   0
outputs:  [Bob 60, Alice change 40]
```

```text
Withdraw 60 from a 100 note

inputs:   [Alice 100, dummy 0]
public:   -60
outputs:  [Alice change 40, dummy 0]
```

The contract API mirrors the fixed output arity with
`output_commitment0` and `output_commitment1`. See
`contracts/pool/src/pool.rs:83` in the SPP repository.

If a wallet must spend more than two real notes, the current SPP planner creates
multiple JoinSplits. For notes `[2, 3, 5]` paying `10`:

```text
step 1: [2, 3] → [consolidated 5, dummy]
step 2: [5, 5] → [Bob 10, dummy]
```

See `sdk/tx-planner/src/plan/mod.rs:158` in the SPP repository.

That is the practical difference:

```text
STRK20: the transaction program's note arity is chosen by the action list
SPP:    the circuit's note arity is fixed by its constraint system and keys
```

## A concrete STRK20 spend

Suppose Alice owns a private note worth 100 and wants to send Bob 60.

### 1. Find the note

Alice knows the channel key, token, and expected sequential index. Her wallet
derives `note_id` and reads that exact storage slot. It does not scan a global
commitment tree.

The Erebus reader stops at the first missing sequential note because gaps are
not valid in a channel. See `sdk/rs/src/read.rs:7`. Upstream STRK20 calls this
[location-indexed discovery](https://docs.starknet.io/build/starknet-privacy/discovery).

### 2. Build the action program

```text
UseNote(Alice 100)
CreateEncNote(Bob 60)
CreateEncNote(Alice 40)
```

The temporary balance for the token evolves as:

```text
0
+ 100 from UseNote
- 60  from Bob's output
- 40  from Alice's change
= 0
```

The action program is valid because use-note actions precede create-note
actions and the per-token balance ends at zero. See the
[STRK20 actions and proofs guide](https://strk20-by-example.org/actions-and-proofs).

### 3. Prove and apply it

The client virtually executes the pool against a recent Starknet state:

```text
client actions
    → compile/preflight
    → virtual Cairo execution
    → Stwo proof
    → proved server actions
    → apply_actions
```

The proof establishes that:

- Alice's note existed in the selected historical state.
- Alice knew the secret required to spend it.
- Its nullifier had not already been written.
- Each token's value was conserved.
- The resulting storage writes and external operations match the proved
  execution.

There is no application-specific Merkle membership circuit for each input note.
The Cairo execution reads the relevant Starknet state during virtual execution.
See the [official architecture](https://docs.starknet.io/build/starknet-privacy/architecture)
and [proof-generation model](https://docs.starknet.io/build/starknet-privacy/proving).

The proof is anchored to recent historical state. Newly created notes may need
to mature until that state is provable, and a proof can expire if submission is
delayed too long.

## The Erebus exact-payment restriction

The upstream STRK20 protocol can construct the 100 → 60 + 40 action program.
The current Erebus client intentionally does not.

`ErebusClient::shield` creates exact-denomination notes because general change
construction is not yet part of its MVP. Settlement selects an exact subset of
the payer's notes and creates no change note. See `sdk/rs/src/client.rs:84` and
`sdk/rs/src/client.rs:1088`.

Consequently:

```text
Erebus wallet [100]         can pay 100, but not 60
Erebus wallet [50, 20, 10] can pay exact subset sums such as 70 or 80
```

This is an Erebus application restriction, not an STRK20 protocol restriction:

```text
STRK20 capability: consume notes and create recipient + change notes
Erebus MVP policy: select an exact subset and create no change
```

## A concrete SPP spend

Suppose Alice owns an SPP note worth 100 and wants to send Bob 60.

### 1. Find the note

The pool emitted the note commitment, leaf index, and encrypted output when the
note was created. Alice's wallet consumes the global event stream and attempts
to decrypt each output.

After recovering `amount` and `blinding`, it recomputes:

```text
Poseidon2(amount, Alice's note public key, blinding)
```

It accepts the note only if that value equals the published commitment. It also
derives the expected nullifier from the commitment, leaf position, and private
key-derived signature.

SPP therefore needs the global event history to reconstruct the commitment tree
and the Merkle paths needed for proving.

### 2. Construct and prove the JoinSplit

```text
inputs:  [Alice 100, dummy 0]
outputs: [Bob 60, Alice change 40]
```

The private witness contains input amounts, note private keys, blindings, Merkle
paths, output values and recipient keys. The public inputs contain the recent
Merkle root, nullifiers, two output commitments, the public deposit/withdrawal
amount, the external-data hash, and applicable ASP roots.

The current Rust and browser SDKs generate this Groth16 proof locally. See
`sdk/client/src/prover/local.rs:14` in the SPP repository.

### 3. Apply it

The Soroban pool:

1. Checks that the Merkle root is still in its recent-root history.
2. Rejects previously spent nullifiers.
3. Checks the external data and public amount.
4. Checks applicable ASP roots.
5. Calls the Groth16 verifier contract.
6. Marks both input nullifiers as spent.
7. Moves public tokens for a deposit or withdrawal.
8. Inserts exactly two new commitments.
9. Emits both encrypted output payloads.

See `contracts/pool/src/pool.rs:561` in the SPP repository.

The pool retains 90 recent root rotations, not 90 ledgers. A proof remains usable
only while its root stays in that ring. See
`contracts/pool/src/merkle_with_history.rs:17`.

## Consequences of the two state models

### Discovery and synchronization

STRK20 wallets derive exact locations from channel secrets. Discovery work is
primarily related to that wallet's channels and notes.

SPP wallets consume the pool's global commitment history, trial-decrypt output
ciphertexts, and maintain the Merkle tree required for future witnesses.

Stellar RPC's event-retention window means a restored SPP client may need a
bootnode or archive provider. That provider does not possess spending keys, but
it can omit or corrupt history, censor access, or observe request timing and IP
metadata. See `docs/src/bootnode.md:37` in the SPP repository.

### Token visibility

Each SPP pool fixes one token address in its constructor. Calling the pool
therefore identifies the asset even when the transfer amount and ownership edge
remain private. Different assets use different pool contracts and commitment
trees.

STRK20 supports per-token subchannels inside one shared pool. A purely private
transfer can keep the token type inside the private state transition. Public
deposits and withdrawals necessarily reveal their token and amount.

### Public metadata

SPP publishes a global stream of nullifiers, output commitments, ciphertexts,
and leaf indexes. It hides the cryptographic input-to-output edge, while the
fixed two-output shape avoids directly publishing the transaction's real output
arity.

STRK20 avoids a global public commitment tree. Its relationship model is instead
structured around directional channels, token subchannels, and dense sequential
indexes. Disclosure of a channel key reveals that channel, not unrelated
channels.

Both systems expose pool-interaction timing. Public deposits and withdrawals
also expose their public addresses, tokens, and amounts. Neither design by
itself defeats timing analysis around unusual or low-volume entry and exit
flows.

SPP additionally takes a public `sender` argument and calls
`sender.require_auth()` for every `transact`. The circuit does not bind that
address to the note owner, so a relayer design is possible, but the current
direct client flow exposes the submitting Stellar account's interaction with
the pool. See `contracts/pool/src/pool.rs:513`.

### Effective anonymity set

The following is an inference rather than a protocol guarantee.

SPP has an easily identified anonymity domain: the commitments in one token's
pool. STRK20 puts multiple token flows behind one pool address and hides more of
the private-transfer state.

In neither case is total pool TVL automatically the effective anonymity set.
Observers can still use timing, asset-specific entry and exit legs, unusual
amounts, submitting accounts, consolidation patterns, and low activity. The
useful anonymity set is closer to contemporaneous, behaviorally
indistinguishable traffic.

## Proof-system and trust boundaries

### STRK20

Stwo/STARK proving does not require a circuit-specific trusted ceremony. The
proved Cairo program can express multiple note operations, per-token accounting,
and a protocol-specific external invocation.

The current Erebus execution route sends the pool private key in plaintext to
both the proving service and the `compile_actions` preflight RPC. Those endpoints
are therefore inside Erebus's confidentiality boundary. See
`sdk/rs/src/prover.rs:3`.

Self-hosted proving changes that operational boundary, not the protocol's proof
semantics. Deposit screening remains enforced by the protocol; self-hosting is
not a screening bypass.

### SPP

SPP generates a specialized Groth16 proof locally and verifies it through
Stellar's native BN254 host operations and a verifier contract.

Its constraint system is coupled to its key lifecycle:

- Changing the transaction circuit changes the R1CS.
- New proving and verifying keys are required.
- The matching on-chain verifier must be updated or redeployed.
- A production deployment needs an appropriate trusted ceremony.

The committed testnet keys were locally generated and were not produced by a
trusted ceremony on the current R1CS. See
`deployments/testnet/circuit_keys/README.md:21` in the SPP repository.

## Composability

STRK20 actions include `InvokeExternal` and `ComputeAndInvoke`. This permits one
protocol-specific anonymizer call in the same private state transition. The
anonymizer must still be designed and audited for the target protocol. Open
notes used at external boundaries can reveal amounts; this is not automatic
private DeFi.

SPP's current circuit expresses deposit, private transfer, and withdrawal.
Adding arbitrary private DeFi would require new circuit constraints and public
inputs, contract integration, keys, verifier configuration, and SDK proving
artifacts.

Mechanically, STRK20 proves a comparatively general private state-transition
program, while SPP proves a narrow payment relation.

## Compliance and governance

STRK20 combines protocol-enforced deposit screening with encrypted viewing-key
material for designated auditors. This does not mean that every private transfer
is automatically screened or that applications receive automatic regulatory
approval.

SPP embeds optional Association Set Provider statements in its transaction
circuits. Pools can be open, allowlist-based, blocklist-based, or require both
proofs. The pool checks the current ASP roots before accepting a proof.

SPP's documented global-view-key circuits are not yet an end-to-end deployed
audit system. Contract storage for the auditor key, event emission, and admin
decryption tooling are explicitly follow-up work. See
`docs/src/global_view_key.md:14` in the SPP repository.

## Maturity boundary

These are not currently equivalent production systems:

- The official STRK20 pool is deployed on Starknet mainnet. A May 2026
  OpenZeppelin audit covers a particular privacy-contract snapshot, not every
  current SDK, wallet, prover, anonymizer, or downstream integration.
- Erebus is pre-MVP and not production-ready. Its encrypted wire-v2 negotiation
  path has not yet received a fresh live run or independent review.
- Erebus's README statement that STRK20 has no mainnet deployment is stale.
- SPP describes itself as a WIP reference implementation. It is unaudited, its
  web SDK is alpha, and its current committed key material is for testnet use.

Current upstream references:

- [STRK20 privacy repository](https://github.com/starkware-libs/starknet-privacy)
- [STRK20 audit directory](https://github.com/starkware-libs/starknet-privacy/tree/main/docs/audit)
- [Starknet privacy architecture](https://docs.starknet.io/build/starknet-privacy/architecture)
- [Stellar ZK host primitives](https://developers.stellar.org/docs/build/apps/zk)

## Shortest accurate summary

> SPP publishes note commitments in a conventional Merkle tree and proves a
> fixed two-input/two-output payment circuit. STRK20 stores a masked amount at a
> secret-derived note location and proves a variable, phase-ordered Cairo state
> transition. Erebus intentionally narrows the upstream STRK20 model to
> exact-denomination agent settlement and should not be used as the boundary of
> STRK20's capabilities.
