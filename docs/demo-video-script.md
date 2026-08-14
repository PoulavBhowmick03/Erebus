# Demo video script

Target length: 3 minutes. Record the public demo and the linked explorer pages. Do not show
wallet keys, environment files, RPC credentials, or terminal history.

## 0:00 to 0:25: problem

Open the demo homepage.

> Erebus gives AI agents a private place to negotiate and a shielded way to settle. The
> relationship stays inside one STRK20 channel. An accepted offer and its payment become one
> state transition.

## 0:25 to 0:55: architecture

Open the architecture section in the README.

> The call path starts with two reference agents. They use role-bound MCP servers, which call
> a thin Python binding and the Rust SDK. The SDK derives the channel, encrypts the wire
> payload, builds STRK20 actions, obtains a proof, and submits the pool call.

> The pool does not have a payload field. Erebus fragments each encrypted message across five
> 119-bit note salts. That constraint shaped the protocol.

## 0:55 to 1:40: agent flow

Return to the demo. Leave the buyer budget at 1,000 and the seller reserve at 800. Select
**Run negotiation**.

> This browser run mirrors the checked-in Python reference agents. It is a simulation, so it
> does not ask for a wallet. The buyer opens a channel and proposes 1,000 units. The seller
> counters. The buyer accepts. Erebus commits the accepted offer and shielded payment
> atomically.

> After settlement, either party can grant a viewing key. The auditor uses that key to recover
> the two offers, participants, agreed amount, and paid amount for this channel.

## 1:40 to 2:10: on-chain evidence

Scroll to **Verifiable evidence**. Open the mainnet transaction links from the completed
submission manifest, then show each successful receipt and its STRK20 pool event.

> These are the mainnet calls listed in strk20.json. Each transaction succeeded and emitted a
> pool event. The manifest lets the sprint hub verify the same evidence without trusting this
> recording.

Do not record this section until the three mainnet transactions exist.

## 2:10 to 2:40: privacy check

Open `scripts/observer.py` and the F30/F31 entries in `docs/friction.md`.

> We test the privacy claim from an observer's position. Against wire v1, the observer recovers
> the acceptance terms from public calldata. Against the wire v2 Sepolia settlement, the same
> tool has no channel key and recovers no message content.

> This does not hide all metadata. Transaction timing and pool usage remain public. The fifth
> salt also has a fixed 59-bit shape, so Erebus traffic can still be fingerprinted. We track
> that as F31 instead of claiming traffic privacy.

## 2:40 to 3:00: close

Return to the homepage.

> Erebus is a Rust SDK, MCP server, and reference agent flow for private coordination on
> STRK20. The source, tests, friction log, and transaction evidence are public. The next work
> is a second wire-v2 implementation and an independent security review.
