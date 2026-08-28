# What stands between the operator alpha and production

Updated 2026-08-28. Erebus is an operator-run Sepolia technical preview. It is not ready
for real value.

The Erebus-owned Protocol 4 release gates now pass. On 2026-08-27, a clean local wheel
installation completed both recovery paths on Sepolia. Spending reservations also became
fail-closed and Rust-authoritative.

The immediate release work is now:

1. Change all source package versions to `0.2.0`.
2. Publish the supported wheels, checksums, and SBOM.
3. Run the installed-artifact canary against the published files.

The production path then needs these controls:

- Operator-controlled Pathfinder, proving, and write-RPC infrastructure.
- A confirmed mainnet pool deployment and compatible service versions.
- Journal retention, encrypted backup and restore tooling, and a recovery drill.
- Secret-safe monitoring and an operational response process.
- A clean install and full run by an external operator.
- Independent security and cryptographic review.
- A bounded mainnet canary with current fee and proof-window evidence.

Protocol 4 now provides caller-supplied operation IDs, a durable Rust journal, read-only
reconciliation, and explicit resume. Local fault tests cover every write boundary. Wire v3
supports repeat deals, change-note settlement, and recipient-bound per-deal disclosure.
These results close several gaps in the original baseline below.

The relationship remains public. Channel-open calldata contains the counterparty address.
The chain also exposes the submitting account, timing, action shape, and note count. Erebus
hides the terms, not the relationship.

---

## Historical baseline from 2026-08-01

Written 2026-08-01, after the full loop ran on Sepolia and the MCP server reached the chain.
Each item says what breaks, what the mechanism is, and whose problem it is. None of it is a
plan; the sequencing is a product decision.

> The sections below preserve the original audit baseline. They do not describe the current
> source. Use the current summary above and [status.md](./status.md) for present behavior.

Ordered by what it blocks, not by effort.

---

## 1. Anyone can run it

### The prover and RPC see the pool key

`compile_actions(user_addr, user_private_key, actions)` is a mandatory preflight, and the
prove call carries the same calldata. Both the proving service and the Starknet RPC
therefore receive the pool private key in the clear (F14). Whoever runs them can decrypt
every note that identity will ever hold.

That makes the operator-run deployment a property of the protocol rather than a deployment
preference. A hosted Erebus that anyone points at would be a service holding every user's
viewing key, which is a worse trust position than the public chain it replaces.

Three ways out, and they are not equivalent:

- **Operator self-hosts.** Pathfinder plus `transaction-prover` on their own hardware. Works
  today, costs a synced node, and is the only option that makes poc.md's custody claim
  literally true.
- **StarkWare runs a public prover.** Removes the install cost and moves the exposure to
  StarkWare rather than removing it.
- **Upstream stops requiring the key.** The key is needed inside the virtual execution to
  derive channel keys and nullifiers. Whether the client could derive those locally and pass
  only the derived values is an upstream design question we have not asked. It is the single
  highest-leverage thing on this list, because every other mitigation is a workaround for it.

The third is worth raising with StarkWare explicitly. It is their design, we have a working
independent client, and the exposure is inherent rather than incidental.

### The binary has to travel

Python runs a compiled Rust binary. Distribution is either `cargo install` on the target, or
platform wheels carrying `erebus-cli` as package data, which is how ruff ships. The second
makes `uvx erebus-mcp` a single command and costs CI cross-compiling for macOS arm64 and
x86 plus Linux. Routine work with no design content.

---

## 2. Asynchronous communication at scale

### Reads are O(notes) RPC calls and restart from zero

`fetch_notes` walks `get_note` one index at a time from zero until it finds an empty slot.
Reading a six-round negotiation costs roughly thirty round trips per direction, and a poll
repeats all of it. Two agents polling every ten seconds generate a few hundred RPC calls a
minute between them for a conversation that produced twelve messages.

Two independent fixes. Cache the prefix, since notes are write-once and an index that
resolved once can never change. And read forward from the stored cursor rather than from
zero, which the client already tracks for writes.

### Nothing tells an agent that a message arrived

There is no push. `scripts/agent.sh wait` polls, which is honest about what it is doing and
does not scale past a demo. Upstream ships a Discovery Service for exactly this and our
client does not use it, preferring keyed contract reads (CLAUDE.md constraint 3 permits
either). Wiring it in is the difference between polling and subscribing.

An MCP-shaped alternative is a long-poll tool that blocks server-side until the transcript
grows, which keeps the agent's turn count down without changing the transport.

---

## 3. Two agents can trade more than once

### One channel per pair is the chain's rule

`compute_channel_key` takes no index (`hashes.cairo:119-124`) and its marker is written
`WriteOnce`, so a second `open_channel` between the same pair reverts (F29). Nothing we do
locally changes that.

### One deal per channel is ours

The guard is `lease.state().settled` in our own `StoredChannel`, checked in `propose_offer`
and `counter_offer`. The chain does not enforce it. Notes keep appending to a live channel,
so the constraint is a decision we made and can revisit.

What makes it non-trivial is framing. A message is five notes; a settlement's payment note
is one. After a settlement the note grid is no longer aligned to a multiple of five, so the
reader needs framing that tolerates variable-width entries rather than assuming a fixed
stride. That is a wire-format change, which means `sdk/ts` and the fixtures move with it.

Composed, these two mean two agents currently transact exactly once, ever. For a supplier
relationship that is not a limitation, it is a disqualification.

### Each message costs about 3 STRK

Measured, not estimated (F27). The pool's own fee is zero; the whole amount is Starknet gas
for verifying a STARK proof. A six-round negotiation cost roughly 18 STRK before any value
moved. Since one action set is one proof, the only lever is fewer, larger action sets:
batching several state transitions into one write. Compressing the payload buys nothing.

---

## 4. Safe to run against real money

### Traffic is still identifiable

Wire v2 encrypts the message, so contents are confidential (F30 closed, verified against a
live transaction). The envelope is 536 bits in 595 bits of capacity and the spare 59 bits are
zero filled, so the fifth salt of every message has a fixed shape and identifies Erebus
traffic to anyone reading calldata (F31). An observer learns which accounts negotiated, how
many rounds, and when. Random padding closes it.

### Registration is irreversible and hands the auditor everything

The first action set writes your pool private key encrypted to the pool's auditor key
(`privacy.cairo:329-334`). On StarkWare's Sepolia pool that auditor is theirs, it is
pool-wide, and there is no rotation and no revocation. Deploying our own pool instance puts
that key in our hands; the constructor is unpermissioned and the class is already declared.
Production would want it under threshold control rather than held by one party.

### No recovery, no rotation, no idempotency

Losing `pool.key` loses everything that identity holds, with no rotation path. Losing the
state directory loses the handles, though the channel key is derivable and on-chain recovery
is possible in principle and unimplemented. Protocol-2 calls carry no idempotency token, so a
crash after inclusion and before the response can orphan a handle or turn a retried proposal
into a second proposal.

### One token per client instance

`ClientConfig.token` is fixed at construction. The pool supports a subchannel per token, so
this is a client limitation rather than a protocol one.

---

## What this list is not

It is not ordered by effort and it is not a roadmap. Several items interact: fixing the note
grid and adding padding are both wire changes and should land together rather than twice;
self-hosting the prover and deploying our own pool are both operator-infrastructure work.

The one item that changes the shape of everything else is whether the pool key has to reach
the prover. Everything under §1 is a workaround for that constraint, and it belongs to
StarkWare rather than to us.
