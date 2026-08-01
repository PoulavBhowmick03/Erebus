# Driving Erebus from two autonomous agents

Two coding agents (Claude Code in one terminal, Codex in another) each hold one Erebus
identity and negotiate a price over a private channel, settling on-chain. Neither instance
sees the other's reasoning. They communicate only through notes in the privacy pool.

This is DoD criterion 4 as of 2026-08-01: the MCP server drives the real Rust client, so an
external agent framework runs the whole loop without touching Erebus internals. The shell
wrapper below still works and is kept for debugging by hand.

## Before you start

Each agent needs a deployed, funded Starknet account, a pool key, a state directory and an
env file. Section 1 of [runbook.md](./runbook.md) covers creating one. Budget about 15 STRK
per identity, since each write costs roughly 3 STRK in gas (F27).

Both identities must have shielded once, because registration only happens folded into an
action set. An unregistered counterparty fails `open` with `CounterpartyUnregistered`.

Pick a pair that has never traded. There is exactly one channel per (sender, recipient)
pair and our client treats a settled channel as terminal, so each pair negotiates once and
then needs fresh identities (F29).

## Registering the two servers

One server per identity, each spawned by its own MCP client configuration.

```shell
claude mcp add erebus -- ~/Developer/erebus/scripts/erebus-mcp.sh ~/.erebus-d/env   # seller
claude mcp add erebus -- ~/Developer/erebus/scripts/erebus-mcp.sh ~/.erebus-e/env   # buyer
```

Both agents then see eight tools: the seven §4 methods plus `wait_for_offers`, which blocks
server-side until the transcript grows. Polling `read_channel_state` in a loop works and
costs the agent one turn per attempt, which is why the extra tool exists.

## The seller's brief (MCP)

> You are a supplier agent negotiating the price of a data feed. Use the `erebus` MCP tools;
> do not run shell commands for any of this.
>
> Your counterparty is `0x059dd5d765dac43a034c74ddc1c887f32a128113bbdc92fb52b495f3aa3b3362`.
> The token is `0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d`.
> Amounts are integers in wei, so 1 STRK is 1000000000000000000. Set every `deadline` to the
> current unix time plus 86400, and use `4660` for `memo_hash`.
>
> Call `open_channel` with the counterparty address to get a `channel_handle`. Then
> `propose_offer` at 1200000000000000000. Then `wait_for_offers` with `expected_count` 2 to
> block until the buyer replies.
>
> Your walk-away price is 700000000000000000. Concede at most 150000000000000000 per round
> using `counter_offer`, replying to their most recent `offer_id`. Never go below the
> walk-away. When their standing offer is at or above it, stop countering and leave it for
> them to accept. Do not call `accept_and_settle` yourself; the buyer pays.
>
> Every write takes about 20 seconds and costs about 3 STRK in gas. Report each round: what
> you offered, what they replied, and why you moved as you did.

## The buyer's brief (MCP)

> You are a procurement agent buying a data feed. Use the `erebus` MCP tools; do not run
> shell commands for any of this.
>
> Your counterparty is `0x06af6b7a6364ded70674dda9f14f896b65d56fdd3399b92aafd9cf5d72477ee9`.
> The token is `0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d`.
> Amounts are integers in wei. Set every `deadline` to the current unix time plus 86400, and
> use `22136` for `memo_hash`.
>
> Channels are directional, so start with `open_channel` on the seller's address to get your
> own `channel_handle`. Then `wait_for_offers` with `expected_count` 1 to pick up their
> opening price.
>
> Your maximum is 900000000000000000. Counter at 500000000000000000 with `counter_offer`,
> replying to their `offer_id`, and concede at most 100000000000000000 per round. As soon as
> their standing offer is at or below your maximum, call `accept_and_settle` on it. That
> settles and pays atomically in one proof.
>
> After settling, call `grant_viewing_key` with grantee `0xa0d17` and report the returned
> `channel_id` so a third party can audit the deal. Do not print the `viewing_key` itself.
>
> Report each round with your reasoning.

## The shell fallback

These drive `scripts/agent.sh` directly and are kept for debugging without an MCP client.
Addresses here are identities B and C, whose pair has already settled, so these are a
reference rather than a runnable demo.

### Seller

> You are a supplier agent negotiating the price of a data feed. Your identity lives in
> `~/.erebus-b/env`. Your counterparty is `0x73dde9582bb68e9a917b01792aaf1daa62e26cc1bd7bfacb0c16ab504445b8b`.
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

### Buyer

> You are a procurement agent buying a data feed. Your identity lives in `~/.erebus-c/env`.
> Your counterparty is `0xe67a39573b40297a5cbedab4cc2a80eb7689e1e0e03f410f63e2ebb2bfdda7`.
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
