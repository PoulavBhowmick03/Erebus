# Driving Erebus from two autonomous agents

Two coding agents (Claude Code in one terminal, Codex in another) each hold one Erebus
identity and negotiate a price over a private channel, settling on-chain. Neither instance
sees the other's reasoning. They communicate only through notes in the privacy pool.

This is not DoD criterion 4. That one requires the MCP server, which is still scaffolding.
What this proves is that the negotiation loop runs under autonomous control with no human
in the path, which is the substance of the claim.

## Before you start

Each agent needs a deployed, funded Starknet account, a pool key, a state directory and an
env file. Section 1 of [runbook.md](./runbook.md) covers creating one. Budget about 15 STRK
per identity, since each write costs roughly 3 STRK in gas (F27).

Both identities must have shielded once, because registration only happens folded into an
action set. An unregistered counterparty fails `open` with `CounterpartyUnregistered`.

Pick a pair that has never traded. There is exactly one channel per (sender, recipient)
pair and our client treats a settled channel as terminal, so each pair negotiates once and
then needs fresh identities (F29).

## The seller's brief

Paste this into the first instance, substituting the env path and the counterparty address.

> You are a supplier agent negotiating the price of a data feed. Your identity lives in
> `~/.erebus-b/env`. Your counterparty is `<BUYER_ADDRESS>`.
>
> Drive everything through `scripts/agent.sh ~/.erebus-b/env <verb>`. Run it with no verb to
> see the list. Never read the files named by `POOL_KEY_FILE` or `ACCOUNT_KEY_FILE`, and
> never print their contents. The client reads them itself.
>
> Your walk-away price is 0.7 STRK. You want more. Open a channel to the buyer, propose 1.2
> STRK, then wait for their reply. Concede at most 0.15 STRK per round and never go below
> your walk-away price. If the buyer's standing offer is at or above 0.7, you may leave it
> for them to accept rather than countering again.
>
> Amounts are in wei: 1 STRK is 1000000000000000000. Each write takes about 20 seconds and
> costs about 3 STRK in gas, so do not poll in a tight loop. Use the `wait` verb, which
> blocks until the transcript grows.
>
> Report each round: what you offered, what they replied, and why you made the move you made.

## The buyer's brief

Paste this into the second instance.

> You are a procurement agent buying a data feed. Your identity lives in `~/.erebus-c/env`.
> Your counterparty is `<SELLER_ADDRESS>`.
>
> Drive everything through `scripts/agent.sh ~/.erebus-c/env <verb>`. Run it with no verb to
> see the list. Never read the files named by `POOL_KEY_FILE` or `ACCOUNT_KEY_FILE`, and
> never print their contents.
>
> Your maximum is 0.9 STRK. You want to pay less. The seller will open the channel and
> propose first, so begin by opening your own channel back to them (channels are
> directional, and you need yours to write into), then `wait` for their offer.
>
> Counter at 0.5 STRK and concede at most 0.1 per round. Accept as soon as their standing
> offer is at or below 0.9. Accepting settles: you pay, atomically, in the same proof.
>
> Amounts are in wei. Use the `wait` verb rather than polling.
>
> Report each round with your reasoning.

## What you should watch

Run `scripts/observer.py <settlement_tx>` afterwards. It should refuse to decode, which is
the wire-v2 confidentiality property holding under real agent traffic rather than under a
scripted demo.

It will still report five salts located by their format flag, because F31 is open. An
observer learns that these two accounts negotiated and how many rounds it took.

## Where this leaks

The agent process can read the key files. Passing paths rather than values keeps keys out of
model context and out of transcripts, which is the failure mode that actually happens. It is
not a sandbox. An agent that decides to `cat` the pool key can, and the honest description
of the current boundary is the local OS account.

`grant_viewing_key` returns a bearer secret. If an agent is told to grant one, that value
lands in its context and therefore in its transcript. Treat any grant issued during an agent
run as compromised.
