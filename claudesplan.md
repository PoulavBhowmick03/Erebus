# Erebus: from infrastructure to a business

Written 11 September 2026. Plain-language strategy notes.

Everything marked **Fact** comes from the repo or from a source linked at the bottom.
Everything marked **My view** is my judgment and you should argue with it.

---

## The short version

Erebus was built as privacy infrastructure. While we were building it, **Starknet shipped
privacy to everybody**. Private transfers and private swaps are now native STRK20 features,
and StarkWare is releasing its own SDK. So "we do private payments on Starknet" is no longer
a thing we own.

What we still own is narrower and more interesting: **STRK20 lets you pay privately. It does
not let you *agree* privately.** Erebus does. Offer, counter-offer, accept, and payment all
bound together in one transaction, with a receipt you can hand to one auditor later. Nobody
else does that.

At the same time, the problem Erebus solves is being written about publicly as an open
problem in AI-agent payments — and the agents are mostly not on Starknet. They're on x402,
where there are around **69,000 active agents** and everything they pay is public.

So the real question is not "transfers or purchasing." It is: **do we go deep on Starknet
where our tech lives, or follow the agents to where the money already moves?**

---

## 1. What changed while we were building

This is the part that matters most, and it is new information.

| What happened | Why it matters to us |
|---|---|
| **Fact.** STRK20 went live with wallet shielding, **private transfers, and private swaps** built in. Multi-call lets you unshield, swap, borrow, repay and reshield in one private transaction. | The two things on our roadmap — transfers and swaps — are now free features of the platform we build on. Building them ourselves means competing with StarkWare's own product. |
| **Fact.** StarkWare is open-sourcing the wallet API and releasing an official SDK for builders. | Part of our value was "we wrote the Rust client because there wasn't one." That advantage shrinks when the official SDK lands. |
| **Fact.** Any ERC-20 can be shielded, and **private USDC is live on Starknet**. | Good news. The "can we settle in a stablecoin" question is largely answered at the platform level. It's a config and testing job, not an invention job. |
| **Fact.** x402 processed ~165 million transactions across ~69,000 active agents, roughly $50M cumulative volume, and AWS Bedrock adopted it in May 2026. | This is where AI agents actually pay each other today. It is not Starknet. |
| **Fact.** x402's public visibility is now openly discussed as a problem. One write-up describes exactly our pitch: *"a procurement agent buying compute on behalf of a company would have every one of its purchases logged on a public ledger with the amount, vendor and time fully visible."* | Our thesis is correct and other people have noticed. That is validation and competition at the same time. |
| **Fact.** Concordium shipped identity ZKPs on x402 in June 2026. IronWeave is doing range proofs for private agent payments. Nevermined markets itself as privacy-first. | We are not alone and we are not first to the x402 side of this. |
| **Fact.** Starknet Foundation has a **Proof of Privacy incubator** taking applications now for teams building on STRK20 — 8 weeks of mentorship and milestone funding. They name payroll, lending and identity as target use cases. | This is a real, open, fundable door with a deadline. |
| **Fact.** Starknet **Seed Grants** are up to $25,000 in STRK, non-dilutive, for teams with an MVP that haven't gone to market and are active in the Starknet community. | We fit this description almost exactly. |

**My view:** the roadmap item you asked me to write — "transfers and private swaps" — should
be *removed*, not scheduled. STRK20 already does both. Spending months rebuilding them is the
single most expensive mistake available to us right now.

---

## 2. What Erebus actually owns

Let me be blunt about what is and isn't ours.

**Not ours anymore:**
- Privacy. STRK20 gives it to everyone.
- Private transfers. Native feature.
- Private swaps. Native feature.
- "A client library for STRK20." StarkWare is shipping one.

**Still ours:**

1. **Private *agreements*, not just private payments.** This is the real invention. Two
   parties haggle — offer, counter, accept — and the moment they agree, the money moves, in
   one atomic transaction. There is no "we agreed but he didn't pay" gap. STRK20 has no
   concept of an agreement. It moves value. We add the deal.
2. **A receipt you can show to exactly one person.** Hand your accountant one deal. They
   cannot see your other deals with the same counterparty. This is built and proven on
   mainnet.
3. **Built for agents, not humans.** Thirteen MCP tools. StarkWare's SDK is aimed at wallets
   and dapps — humans clicking buttons. We're aimed at software that negotiates on its own.

**My view:** stop describing Erebus as a privacy project. Describe it as **the negotiation
and settlement layer for AI agents**, which happens to be private. Privacy is now a feature
of the chain, not our product.

---

## 3. The moat question, honestly

You asked about a moat. Here's the uncomfortable answer first.

**Things that are not a moat:**
- Our code. It's open source. Anyone can copy it.
- Being on Starknet. Anyone can deploy there.
- The mainnet runs. They prove it works. They don't stop anyone doing the same.
- Being early. Four months of lead time is not a moat.

**Things that could genuinely become a moat:**

| Possible moat | How strong | What it takes |
|---|---|---|
| **Owning the receipt format** — if the way agents record "what was bought, for how much, agreed by whom" becomes *our* format, everyone who wants to read those receipts has to speak our language | Strongest available to us. Standards are very sticky | Publish it as a spec with test vectors, get two other projects to use it. Cheap to start, hard to win |
| **Being inside somebody else's agent platform** — once you are the payment rail, ripping you out is a project nobody wants to do | Strong, but you only get one or two shots | One real integration partner who has paying traffic already |
| **The failure library** — we have 42 documented ways this stack bites you. Anyone rebuilding this hits all 42 | Real but temporary. Buys maybe 2–3 months | It already exists in `docs/friction.md`. Publish it louder |
| **Relationships with StarkWare** — incubator, grants, being the reference agent-commerce example | Real, and it compounds | Apply to Proof of Privacy. Be the example they point at |

**My view:** for a small team, the honest moat is not technology. It is **being the thing
people already built against.** That argues for making Erebus extremely easy to adopt and
getting the receipt format into other people's code, rather than building more features.

---

## 4. Who our users are

There are three different groups and it matters not to confuse them.

**Group 1 — Starknet developers (now, free, small)**
They saw the launch. Maybe 5,000 views, which realistically means a few dozen curious people
and a handful who would actually run it. They are not customers. They are how you learn what
breaks. Worth having, not worth building a company on.

**Group 2 — Agent developers who need payments (the real user)**
Someone building an AI agent that has to pay for something — an API, compute, a dataset,
another agent's work. They already reach for MCP tools. There are **over 10,000 MCP servers**
and a documented complaint that developers waste hours finding the right one. This is a large
group and we are already shaped correctly for them.

**Group 3 — Agent platforms and marketplaces (the payer)**
A company running a marketplace where agents buy from each other. They have the traffic, they
have the budget, and they have a genuine problem: their customers' prices are public. **This
is who writes a cheque.**

**The key point:** Group 2 *uses* Erebus. Group 3 *pays* for Erebus. They're different
people. Most of our effort should make Group 2 successful, because that's the evidence we
show Group 3.

---

## 5. How we reach them

Concrete, cheapest first.

**This week, free:**
- **List the MCP server everywhere.** mcp.so (20,000+ servers listed), smithery.ai,
  glama.ai, the official registry, and the `awesome-mcp-servers` GitHub list. We have a
  published MCP server and, as far as I can tell, we're in none of these. This is free
  distribution to exactly Group 2 and it's a few hours of work.
- **Apply to Proof of Privacy.** It's open now, it's aimed precisely at teams building on
  STRK20, and it comes with mentorship and money.
- **Apply for a Seed Grant.** Up to $25k STRK, non-dilutive, and we match the criteria.

**This month:**
- **Change what the front page asks people to do.** Right now it offers a demo, an
  explanation, and source code. None of those is "start using it." One clear action:
  *get two agents paying each other in 15 minutes.*
- **Go where the argument is happening.** The privacy-on-x402 discussion is live and public.
  We have working code and mainnet receipts. Most people in that conversation have opinions.
- **Write up the 42 friction items as a public post.** It's the most credible thing we have
  and it costs nothing — it's already written.

**My view:** do not build a waitlist for a product that doesn't exist. Get the existing thing
in front of Group 2 through the MCP directories, and watch what they do.

---

## 6. The options after launch

Five real options. Not all are good.

### Option A — Go deep on Starknet
Become the agent-commerce layer of STRK20. Proof of Privacy incubator, Seed Grant, be the
example StarkWare points at.
- **Cost:** low. Mostly applications and polish.
- **Pays:** grant money soon, credibility, possible StarkWare relationship.
- **Ceiling:** limited by how many agents end up on Starknet.
- **Kills it:** Starknet's agent ecosystem stays small.

### Option B — Follow the agents to x402
Be the confidentiality layer for agent payments where the agents already are. 69,000 active
agents, and public amounts are a known complaint.
- **Cost:** high. Different chain, different stack, and our Starknet work is not directly
  portable. Note our own `CLAUDE.md`: ERC-8004 is EVM-only and x402-on-Starknet exists only
  in TypeScript.
- **Pays:** the largest market by far.
- **Ceiling:** high.
- **Kills it:** Concordium, IronWeave and Nevermined get there first, or the privacy problem
  turns out not to be painful enough to switch for.

### Option C — Own the agreement format
Publish how agents record a deal — what was bought, price, who agreed — as an open spec with
test vectors. Chain-agnostic. Get others to adopt it.
- **Cost:** low to build, very high to get adopted.
- **Pays:** the only durable moat on this list.
- **Ceiling:** highest, and slowest.
- **Kills it:** nobody adopts it, and a big player publishes their own.

### Option D — Services and grants
Paid integrations, supported deployments, ecosystem funding.
- **Cost:** low.
- **Pays:** real money, soon.
- **Ceiling:** it's a consultancy, not a product company.
- **Kills it:** nothing. It just doesn't grow.

### Option E — Get absorbed
StarkWare or an agent platform hires the team and takes the tech.
- **Worth naming** because a strong Option A makes this more likely, and it is a genuine
  outcome, not a failure.

**My view:** A + C together. Option A funds us and gives us a home while we do Option C,
which is the only thing on this list that becomes defensible. Option B is the big prize but
we should not attempt it until we know whether anyone actually pays for the private version.
Option D should be taken opportunistically, never as the plan.

---

## 7. The money question

From our own measured mainnet run, so these are hard numbers, not estimates.

- A full negotiated deal (offer + counter + settle) costs **26.38 STRK**.
- **18 of that is the pool fee, and it goes to StarkWare, not us.**
- For infrastructure cost to stay under 1% of the payment, the deal has to be above
  **~2,638 STRK**.

**What this rules out:** taking a percentage of each payment. The fee we don't collect
already eats the margin. A 10-basis-point fee on $100,000 of monthly volume is $100.

**What's left:**
1. **Sell to the platform, not the agent.** A marketplace pays to embed us. One contract,
   real money, and it's the only shape with a big number at the end.
2. **Paid integration and support.** Honest revenue, small, services-shaped.
3. **Grants.** Not revenue, but it's runway, and it's available right now.

**My view:** we have no evidence anyone will pay for any of this yet, and neither did the
ChatGPT strategy doc. The subscription prices in it ($3,000 pilot, $500/month) are guesses.
The useful thing the numbers do is rule out the take-rate model with certainty.

---

## 8. What I'd do in the next 30 days

1. **Delete transfers and swaps from the roadmap.** STRK20 ships them. Say so publicly —
   it's a credibility win, not an admission.
2. **Re-describe Erebus** as the negotiation and settlement layer for agents. Update the
   README and the site.
3. **List on every MCP directory.** Free, immediate, reaches exactly the right people.
4. **Apply to Proof of Privacy and the Seed Grant.**
5. **Publish the receipt format.** Right now a settlement records "2 STRK agreed, 2 STRK
   paid" and nothing about *what was bought*. The field to fix this (`memo_hash`) already
   exists and is unused. Defining it is cheap and it is the first brick of Option C.
6. **Fix the first-run failure.** Our own friction log says a wrong allowance costs a failed
   transaction and 6 STRK. That's someone's first experience of Erebus. A read-only
   "what do I need to fund" tool fixes it.
7. **Talk to ten people** building agents that pay for things, and ask what their payments
   look like today. Not a demo. Questions.

---

## 9. What I could not find out

Honest gaps. These need humans, not research.

- Whether anyone will actually pay for private agent settlement. No evidence either way.
- How big Starknet's agent developer population really is. I found no number.
- Whether StarkWare would rather partner with us or build this themselves.
- Whether the 5,000 views contained any real buyers, or was mostly other builders.
- What Tongo, Curvy and KAGE charge, if anything.
- Whether our atomic negotiate-and-settle is genuinely unique, or just unusual. I found
  nobody else doing it, but absence of evidence isn't proof.

---

## Sources

- [Privacy Is Now Live on Starknet — STRK20 Launch](https://www.starknet.io/blog/privacy-live-on-starknet/)
- [Push to Private: Starknet's Privacy Stack Is Open for Builders](https://www.starknet.io/blog/push-to-private/)
- [Starknet v0.14.2: Native Privacy for STRK20 and strkBTC](https://www.starknet.io/blog/starknet-v0-14-2-the-privacy-engine-arrives/)
- [Private USDC Features now on Starknet](https://www.starknet.io/blog/privacy-features-for-usdc-on-starknet/)
- [Starknet Foundation launches Proof of Privacy incubator](https://cryptobriefing.com/starknet-proof-of-privacy-incubator-strk20/)
- [Starknet Grants](https://www.starknet.io/grants/)
- [Starknet Seed Grant Program](https://grantedai.com/grants/starknet-seed-grant-program-starknet-foundation-2064a55e)
- [Solving the agentic payments privacy problem](https://www.financederivative.com/solving-the-agentic-payments-privacy-problem/)
- [Agent Payments Showdown: x402 vs AP2 vs MPP vs ACP in 2026](https://agentlux.ai/blog/the-agent-payments-showdown-x402-vs-ap2-vs-mpp-vs-acp-in-2026)
- [x402 Payments in 2026: Coinbase, Stripe, Cloudflare, AWS and Alternatives](https://wavect.io/blog/x402-payments-comparison-2026/)
- [Private Agent Payments With Range Proofs — IronWeave](https://archive-blog.ironweave.io/x402-is-better-with-privacy/)
- [Best MCP Registries in 2026](https://www.truefoundry.com/blog/best-mcp-registries)
- [MCP Registries: Where to List Your Server](https://roxyapi.com/blogs/mcp-registries-where-to-list-your-server)
- [Tongo docs](https://docs.tongo.cash/)
- [KAGE — confidential payments wallet POC](https://github.com/keep-starknet-strange/kage)

Repo sources: `docs/runs/2026-08-31-mainnet-060-040-canary.md` (the fee numbers),
`docs/friction.md` (42 entries, F27 and F42 in particular), `docs/usecases.md`
(what the protocol can and cannot do), `docs/privacy-model.md` (what actually leaks).
