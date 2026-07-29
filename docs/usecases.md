# Where Erebus is used

A follow-up to the PoC. What the primitive actually gives you, and who on Starknet has a
problem shaped like it.

---

## What it is, without the agent framing

Strip the agent story off and four properties are left.

Two known parties get a confidential structured channel that lives on-chain. 400 bits per
transaction, four notes at 119 usable salt bits each. Only the counterparty can read it, and
nobody watching the chain can tell a negotiation from any other note write.

A payment can be bound to an agreement inside one proof, so the chain applies both or
neither. There is no window where one side holds an acceptance and no money.

Anyone holding a granted viewing key reconstructs the whole sequence from chain data alone.
Offers, counters, the acceptance, the settlement. No off-chain log to trust or lose.

And no third party holds keys. The library runs in the operator's process against the
operator's prover.

The constraints matter as much as the properties, because they decide the use cases. Two
parties, not more. About 29 seconds per round, since every write is a proof. Value moves one
way per settlement. Both sides have to be registered and funded in the pool, and the deposit
that funds them is public by construction.

That combination rules out order books, AMMs, HFT, and anything multi-party. What it fits is
bilateral, deliberate, and commercially sensitive.

---

## The cases

*Agent-to-agent service commerce.* One agent buys an inference run, an API call, a dataset,
a scraping job from another. Starknet already has the public version of this — Nethermind's
`x402-starknet`. The problem with the public version is not the payment, it is the dataset it
leaves behind. An agent transacting a few hundred times a day publishes its prices, its
counterparties, its volumes and its cadence permanently. That is a full commercial profile of
whoever operates it, reconstructable by anyone, forever.

The atomicity matters here more than in a human market. Two agents have no reputation
system between them and no legal recourse at machine speed. "I accepted and you did not pay"
is not a dispute a bot can resolve. Binding the acceptance to the payment in one proof means
the dispute cannot happen.

*Machine-to-machine procurement.* Same shape without the AI. A device buying bandwidth,
compute, storage or energy from another device. Price discovery between two parties,
settlement one way. DePIN-style deployments on Starknet have this shape and currently have
to pick between a public price feed and an off-chain agreement nobody can audit.

*RFQ where one leg is off-chain.* A taker asks a maker for a quote. On-chain RFQ leaks the
quote, so competing makers read your spread and the pending transaction is a front-running
target. Off-chain RFQ keeps the quote private but the taker can walk after the maker has
committed. Erebus gives a private quote with binding settlement. The caveat is the signer
limit below: this works when the counter-value is off-chain or already held, not when both
legs are on-chain assets.

*B2B invoicing and recurring settlement.* Two companies with a standing supply relationship.
Supplier pricing is close to the most sensitive number a business has and no finance team is
going to publish it to a public chain to save on settlement. But the same team does need an
audit trail, and their auditor and their regulator need to read it. That is viewing-key
disclosure exactly. This is the least agentic case on the list and possibly the most
commercially real one.

*Contractor and payroll flows.* Amounts private between the two parties, disclosable to a tax
authority or an auditor on demand without exposing them to everyone else.

*Any two-party protocol that needs a confidential channel.* The salt lane is not an Erebus
feature. It is a property of the pool that was already there. Any project on Starknet that
needs two known parties to exchange structured state privately on-chain can use it, whether
or not there is an agent anywhere near it.

---

## Why it would be used rather than the alternatives

There are two alternatives today and each fails on a different axis.

Public settlement is atomic and auditable, and it publishes your business. Every price you
accepted, everyone you dealt with, how often. For a party that transacts rarely this is
tolerable. For an agent that transacts constantly it is a behavioural feed.

Off-chain agreement with on-chain payment keeps the terms private and gives up the binding
between them. Now there are two systems that can disagree, and the gap between agreeing and
paying is a dispute surface.

The third axis is the one people skip. Full privacy with no disclosure path is unadoptable by
anyone with a counterparty, an auditor, or a regulator. Selective disclosure is what makes
the privacy usable rather than merely available, and it is why the viewing key is load-bearing
rather than a feature.

---

## The limit worth knowing about

One action set has exactly one signer. `__execute__` pulls a single
`(user_addr, user_private_key, client_actions)` out of the calldata and validates the
signature against that one user, so two parties cannot both spend inside one proof.

Erebus therefore settles a one-way payment bound to an agreement. The counter-value is
off-chain: a service delivered, a dataset handed over, compute run, fiat moved. It is not
delivery-versus-payment of two on-chain assets.

The invoke lane does not rescue this, because `ServerAction::Invoke` calldata is public. It
exists for AMM swaps where public swap parameters are acceptable, and using it for a private
swap would publish the thing being kept private.

If a multi-signer action set is ever on the roadmap, the surface here roughly doubles:
private DvP, private OTC in two on-chain assets, private collateral swaps. Worth knowing
whether that is being considered, because a fair amount of the interesting stuff sits behind
it.

Two smaller ones. The deposit that funds an agent is public, so the aggregate a party brings
into the pool is visible even though nothing after it is. And a round costs a proof, so this
is negotiation latency, not trading latency.

---

## What it brings out for STRK20

Every project on the privacy roadmap treats a note as a unit of value. Erebus is the first
one where a note has to say something rather than be worth something, and the primitive
turned out to support it without a contract change.

That extends what STRK20 addresses from private payments to private bilateral protocols. The
120-bit salt was sitting there as a nonce. It is a payload channel, and once notes can carry
structured data the pool is a private message bus that also happens to move money atomically.

The agent framing is where we think the near-term demand is. The mechanism underneath it is
not agent-specific at all.
