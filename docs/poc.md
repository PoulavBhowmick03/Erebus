# Erebus PoC


Two AI agents that need to transact have no private way to do it. They can negotiate over an API and settle with a public transfer, which puts their prices, counterparties and volumes on-chain for anyone to look at. Or they settle off-chain and give up atomicity, so one side can agree and not pay.

Erebus is attempting a third option. Two agents open a channel on STRK20, negotiate as
structured state transitions, and settle atomically inside the pool. Atomic settlement and
scoped reconstruction now work live. The current salt wire is publicly decodable, so the
private-channel claim remains unfulfilled until wire v2.

---

## The first problem

We had assumed a note could carry an `Offer` struct. It can't, and we checked at diff levels.

A note is `(packed_value: felt252, token: ContractAddress)`. For encrypted notes only
`packed_value` is written. `ClientAction` is a closed enum and none of the note-creating
variants take a payload field. We confirmed the same against the deployed Sepolia pool's
ABI. There is no data field, and literal on-chain messaging is not something the current
stack supports.

## The salt lane

Every encrypted note carries a client-chosen salt. The contract constrains it to
`2 <= salt < 2^120` and stores it in the high 120 bits of `packed_value`. It is
written by us, stored by the pool, and recoverable by the counterparty. It is a payload
channel, but not a confidential one: `packed_value` exposes the salt publicly.

We use 119 of them. A chunk that happened to come out as 0 or 1 would be rejected with
`ZERO_SALT` or `SALT_TOO_SMALL`, which is rare enough to survive testing and horrible to
debug in production. Pinning bit 119 to 1 puts every salt in `[2^119, 2^120)`, always valid,
no special cases.

An `OfferTerms` is 5 felts as declared, about 760 bits. Two fields are redundant on the
wire. `token` is implied because a subchannel is a token, so both sides already know it.
`nonce` is unnecessary because the note index already orders messages and makes each one unique. Truncating `memoHash` from 252 to 128 bits leaves 2^64 collision resistance, which is fine for a commitment of this usecase.

That gets an offer down to 400 bits, and four notes at 119 bits gives 476. So one negotiation message is four zero-amount notes, written in a single transaction.

Fixed stride, so message `k` sits at indices `4k` through `4k+3` and the reader needs no
framing search. One transaction, one proof, one atomic write.

### The rule to keep it secure

Structured salts go on zero-amount notes only. Value-bearing notes keep a random salt,
because the salt is the one-time-pad nonce for the encrypted amount. Reuse a mask across two notes with different amounts and an observer can subtract the ciphertexts and recover the difference. Zero-amount notes have no variance to leak, so they are immune.

---

## Workflow

*Setup, once per agent*: The operator has a Starknet account and generates a
separate pool key. Registering publishes the public half so other agents can send to them. Getting money in is two transactions: an ERC-20 approve, then a deposit that turns public funds into a private note. The deposit leg is public by construction. Wire-v1 negotiation terms are also public because their salts appear in `packed_value`.

*Opening a channel*: Agent A derives a shared location from its own pool key plus B's
address and public key, then writes an encrypted note telling B where that is. From then on both sides go straight to the right storage slots. An observer sees writes to storage that looks unrelated.

*Negotiating, wire v1*: A's policy engine produces terms, Erebus packs them into four salts,
and one transaction writes them into B's subchannel. B reassembles the offer and decides:
accept, counter, or walk. A public observer can currently reassemble the same salts from the
transaction. Confidential encoding is the remaining protocol blocker.

Each round costs one proof, ~29 seconds on published figure. Three rounds is
about 90 seconds before settlement starts.

*Settling*: Acceptance and payment go into one set. Spend A's notes, create a note
for B with the agreed amount, record the acceptance. One proof covers all of it, so either the deal is struck and paid or neither happened. There is no state where B accepted and wasn't paid.

*Disclosure*: An agent grants a viewing key and the holder reads the full sequence:
offers, counters, acceptance, settlement, reconstructed from chain data alone. Nobody else learns anything.

## The same pipeline for every write action

Simulate locally, prove, submit through `apply_actions`. We run our own prover. This is deliberate. The invocation sent to `starknet_proveTransaction` carries `user_private_key` in the clear at `calldata[5]`. A prover therefore sits inside the trust boundary of whoever owns that key. For agent infrastructure that means the prover belongs to the agent operator, and Erebus operates nothing.

That means, Erebus never holds agent keys. The library
runs inside the operator's own process, against the operator's own prover.

---

## Where we are (AI generated)

Working and tested offline:

| | |
|---|---|
| Full pool flow: register, channel, subchannel, shield, private transfer | TypeScript, 37 tests |
| Negotiation encoding, four notes per message | TypeScript, round-trips |
| Rust client: domain hashes, `ClientAction` encoding, `INVOKE_TXN_V3` hashing, Stark ECDSA | 36 tests |
| Rust reproduces an SDK-built proof invocation signature byte for byte | pinned |
| Proving endpoint reachable, spec `0.10.3-rc.2` | verified |

The Rust client exists because there is no Rust write side. `discovery-core`
covers reads. We need to build `ClientAction`s, serialises calldata, signing the invoke or calling the prover.

Not done: anything on-chain, settlement, the MCP server, the agent loop.


## Where this goes

The MVP is two agents, one channel, offer-counter-accept, one atomic settlement, one
viewing-key reveal, driven end to end through an MCP server so any agent framework can use it without knowing Erebus exists.

The thing about the salt lane, it is a general mechanism the privacy stack already had and nobody had used. the internal projects on the privacy roadmap were content with the wallet actions covering all of value moving functionalities, as per what I imagine reading their descriptions. Erebus is the first case where a note needs to say something rather than be worth something, and the primitive turned out to support it.
