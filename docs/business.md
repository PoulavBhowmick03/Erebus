# Erebus — Business Potential and Product Strategy

*Converted from the uploaded GPT research report. Source snapshot: Erebus commit `e4dbc346e704b82ad0de4482e298df42e801f7fb`; competitor documentation as available on 11 September 2026. Competitor descriptions are vendor or repository claims unless stated otherwise. Proposed prices, sales targets, costs, and customer segments are hypotheses to test — no external willingness to pay for Erebus has been verified.*

Erebus should pursue a focused business in confidential supplier purchasing for crypto-native companies. The proposed product connects a purchase request, approval, privately settled payment, and finance record. Agents can initiate the workflow, but a company with a recurring purchasing need must be the paying customer. Selling the broader idea of autonomous agents negotiating privately would leave both the buyer and the reason to pay unresolved.

The first commercial offer should be a paid implementation for one company and one established supplier, followed by a subscription for operation, recovery, and support. The initial workflow should cover a prepaid service allocation or an approved supplier invoice. A general marketplace, OTC venue, and multichain privacy network would add substantial problems before demand for the core product is established.

## Business proposition

The proposed value proposition: let a company authorize and pay established suppliers in stablecoins without publishing negotiated amounts and terms, while retaining records that its operators and finance team can inspect. The service must also explain the metadata and infrastructure parties that remain able to observe activity. Erebus cannot currently promise that every aspect of a business relationship is hidden.

The business depends on the intersection of four requirements: a customer already wants blockchain settlement; public commercial amounts create a meaningful problem; existing suppliers will participate; and the customer accepts the actual privacy and operational model. If any requirement fails, ordinary invoicing, a bank transfer, or an existing payment platform can be the better choice.

| Business element | Recommended initial definition |
|---|---|
| Product | Confidential purchase orders and supplier settlement |
| Paying customer | Crypto-native company with recurring digital-service purchases |
| Economic buyer | CTO or platform lead initially; finance approves operational adoption |
| Daily users | Finance operator, purchasing service, and optionally AI agents |
| First transaction | Prepaid service allocation or approved invoice with a known supplier |
| Revenue | Implementation fee plus recurring software and support fee |
| Initial distribution | Founder-led pilots through Starknet and existing infrastructure relationships |
| Expansion | More suppliers and business units, then embedded integrations and demanded chains |

## The problem and the existing alternatives

A company can negotiate over a private API or email and pay through a public wallet. This already keeps the conversation private, so encryption of messages alone is not a sufficient business proposition. The residual problem is that settlement can expose amounts and activity, while approvals, delivery records, and payment recovery may remain spread across several systems. Erebus needs to make that combined workflow materially better.

A research platform purchasing a proprietary data license may regard its negotiated discount as sensitive; a compute reseller may want suppliers and competitors to learn less about its purchasing economics. These are plausible problems, not evidence that either segment will adopt Erebus. Interviews must determine whether the public information actually changes bargaining power or operations, and whether cheaper measures already address the concern.

The strongest alternative may be a conventional invoice paid through a bank or card. Such payments do not expose their full contents on a public blockchain, although financial intermediaries retain information. A customer comfortable with those intermediaries has no automatic reason to adopt a shielded pool. Requiring blockchain settlement must come from the customer's workflow rather than the product's architecture.

Established products also cover much of the operational need. Request Finance offers approvals, reconciliation, and accounting integrations; its reviewed annual-billing prices include $250/month for Growth and $500/month for Pro, with stablecoin payouts advertised without processing fees.[^1] These prices create a demanding comparison for an Erebus subscription — the defensible premium must buy required confidentiality and supported integration.

Agent purchasing already has alternatives too. Nevermined offers delegated spending, metering, credits, and merchant integrations; Coinbase provides agent wallet tooling and security guardrails.[^2][^3] Neither a budget screen nor an MCP endpoint establishes differentiated value by itself.

## Buyers and purchase triggers

The first prospect should have a real supplier, an identified budget holder, and a recent purchase to discuss. A useful initial screening hypothesis is at least $25,000 in monthly relevant purchases, with individual settlement batches of $1,000 or more — qualification assumptions rather than measured market boundaries.

**Primary buyer.** Target a small or midsized crypto-native software business purchasing data, inference, compute, or technical services from a limited set of repeat suppliers. It should already hold or receive stablecoins and have an engineer who can integrate the purchasing flow. Prioritize businesses for which supplier identities are already known or acceptable to reveal, because Erebus's current metadata exposure weakens a supplier-relationship secrecy pitch.

The CTO sponsors the pilot when payment integration and agent operation consume engineering time. A finance or operations lead co-approves the records, authorization model, and recurring expense. The supplier's technical or commercial lead is an adoption dependency even when the supplier pays nothing. An enthusiastic developer without either budget authority or a participating supplier is not a qualified opportunity.

**Secondary buyers**

| Segment | Purchase trigger | Budget owner | Assessment |
|---|---|---|---|
| Agent platforms buying services | Customers request confidential purchasing and controllable spending | Platform lead or CTO | Best fit when paid supplier traffic already exists |
| Crypto businesses with manual supplier payments | Confidential amounts and reconciliation become operational requirements | Finance lead with engineering sponsor | Broader near-term prospect pool; do not force agent adoption |
| Service marketplaces | Enterprise customers request private negotiated settlement | Product lead or marketplace operator | Attractive later; existing distribution helps, but integration is deeper |
| Wallet and payment platforms | Users request a supported confidential payment route | Infrastructure or payments lead | Possible embedded customer and simultaneous competitor |
| Protocol teams requiring Rust integration | A funded delivery deadline requires supported STRK20 integration | Technical lead | Fastest services revenue; weaker recurring-product evidence |
| DAOs and treasury teams | Confidential supplier spend is authorized by governance | Treasury signers and operations | Conditional fit — public accountability may conflict with secrecy |

Retail wallets, anonymous one-off traders, speculative agent swarms, and customers buying fractions of a cent of compute are poor first targets. Large regulated institutions may have genuine confidentiality needs, but their security review, deployment, and commercial expectations make them difficult initial customers — a sequencing judgment, not a claim these segments have no future value.

**Named organizations, interpreted carefully.** Nevermined describes Exa selling API access to agents on delegated cards — evidence of a real supplier-acceptance pattern and a counterexample to the idea that autonomous purchasing requires shielded settlement.[^4] Exa is a potential supplier-side interview candidate, not a verified Erebus lead. Request Finance publishes customer stories (RealT, Polemos, Whatever Digital) that support the existence of operational crypto-payment workflows, not unmet demand for Erebus privacy.[^5] Nevermined and Skyfire are more naturally integration partners or distribution competitors than direct end customers; Curvy is a direct confidentiality competitor; Tongo is an alternative infrastructure provider.[^6][^7][^8] Starknet ecosystem support can supply introductions and development funding, but should not be counted as recurring customer demand.

## Product design and delivery boundary

The first product should be a small purchasing application supported by the existing Rust implementation: a developer API, optional MCP integration, and a human operator console. A business must be able to approve a purchase without asking an LLM to perform every step.

An illustrative workflow: an agent or application requests a supplier's prepaid service allocation. The supplier returns a quote specifying asset, amount, service description, expiry, and delivery conditions. The company approves it under a supplier allowlist and spending policy. Erebus settles the payment, tracks confirmation, and produces a linked record. The supplier then activates credits or delivers the service under the parties' commercial arrangement.

If delivery fails, an operator follows an agreed refund or dispute process — the protocol does not automatically guarantee service delivery. Keeping that distinction explicit avoids selling an atomic exchange guarantee the implementation does not provide. Erebus's current acceptance and payment are combined, but agreement semantics are checked in the Rust client rather than independently understood by the underlying pool circuit.[^10]

**Minimum paid product**

| Component | What the customer receives | Delivery requirement |
|---|---|---|
| Supplier setup | Approved recipients, service identifiers, and delivery terms | One real supplier works end to end |
| Authorization | Limits, approval thresholds, and a reliable signing boundary | Include fees, retries, and negotiation spending in policy |
| Settlement | One supported stablecoin route with clear status | Validate deposit, payment, disclosure, and exit |
| Recovery | Durable operation IDs and reconciliation | Ambiguous status cannot silently trigger duplicate payment |
| Evidence | Linked authorization, terms, payment, and delivery records | Separate cryptographic facts from business assertions |
| Operator console | Pending payments, approvals, alerts, and exports | Usable by a finance operator without source-code inspection |
| Supplier integration | Quotes, receipts, and authenticated delivery callbacks | Handle duplicate and delayed callbacks safely |

Erebus already has useful foundations: agent tools, a Rust implementation, recovery mechanisms, scoped disclosure, and MCP spending reservations.[^11][^12] Its documented bounded mainnet runs are implementation evidence — they do not establish a production service level, a security audit, or a demonstrated stablecoin purchasing product.

The commercial privacy specification should identify separately what the public chain, counterparty, model provider, prover, RPC provider, operator, and upstream auditor can observe. The documented external compile/preflight route exposes the pool-private key to its infrastructure, with confidentiality consequences distinct from account signing-key custody; customers needing a different trust arrangement require a validated deployment alternative.[^13]

Scoped disclosure should initially be sold as selected-deal inspection. Current disclosure material can reveal the chosen transcript; recipient-side expiry checks cannot erase information already received. Business identities in an unsigned capsule should not be marketed as an independently signed certificate — product work should add explicit signatures and authority binding where customers require them.[^14]

**Architecture priorities.** Evaluate signed, encrypted negotiation off-chain followed by on-chain settlement — repeated paid on-chain quote rounds impose cost even on unsuccessful negotiations. A revised design should bind the final agreement and authorization to the execution path, define transcript availability, and preserve deterministic recovery. This requires security design, not just moving message transport to a server. Keep a clear internal boundary between purchase records and the settlement backend, preserving the option to support another privacy system when a paying customer requires it — but do not build several backends immediately, since each changes the guarantees, supported assets, recovery behavior, and audit surface.

## Starknet competition and product choices

The following projects were examined in the preceding technical assessment using their public repositories, reflecting reviewed evidence available on 11 September 2026. Reported demonstrations and repository completion labels are not equivalent to audited products or commercial adoption. The relevant project universe is discoverable in the STRK20 sprint index.[^15]

| Project | Overlap and evidence boundary | Implication for Erebus |
|---|---|---|
| VINSS | Deal rooms, offers, escrow, fulfillment and disputes; reports mainnet workflows | A generic private deal room is already contested |
| APP20 | Encrypted chat and RFQ/escrow design; confidential mainnet trading remains limited in its documentation | Encrypted negotiation alone is insufficient differentiation |
| Strkret | Cumulative vouchers and batched settlement; prototype enforcement limitations | Compare cost and credit guarantees before choosing per-message settlement |
| Kese | MCP wallet policies, approvals and audit workflows; production adoption unverified | Budget and approval features are expected functionality |
| OXA | Scoped payment credentials; full credential mainnet path incomplete in reviewed evidence | Authorization is a separate competitive product surface |
| Offbook | OTC and RFQ workflows | An OTC pivot requires trading-specific advantages |
| Tony Strk | Agent tools, Tor and payment-gated access | Private access and service payments are adjacent established ideas |
| ConditionalPay | Conditional payment and refund helper with public boundary information | Borrow workflow lessons without assuming identical privacy |
| Stealth Checkout | Merchant invoices, widgets and webhooks; route-dependent privacy | Supplier onboarding and payment status are commercial necessities |
| Tongo | Confidential balances and payments; advertises client-side proving and mainnet assets | Benchmark an alternative backend and its trust model |

The practical opening is execution depth for a selected buyer. Erebus could become the best-supported way for a specific class of company to purchase from private suppliers, reconcile payments, and recover safely — a proposed advantage to earn, not a demonstrated one. A feature inventory does not show that Erebus is ahead of every competitor or that competitors cannot add the same workflow.

## Competition across the wider ecosystem

Curvy is the clearest direct challenge to a broad private-stablecoin pitch: its product describes business and agent payments, developer integrations, named identities, and disclosure, with documentation on different visibility at entry, exit, and internal transfer.[^26][^27] Erebus should compare precise observer guarantees rather than adopt either project's broad marketing statements.

Coinbase Agentic Wallet, Nevermined, and Skyfire compete for developer adoption and existing merchant connectivity. A developer can select one of these for access, authorization, payments, or metering before considering any privacy component — Erebus's expansion route may therefore be an integration into such workflows rather than replacing their whole stack. Partnership feasibility and commercial terms are unverified.[^28][^29][^30]

| Category | Examples | Commercial significance |
|---|---|---|
| Confidential settlement | Curvy, RAILGUN, Privacy Cash, Payy | Alternative rails and integrations can reduce demand for a standalone Erebus SDK |
| Private trading | Renegade | A trading product must compete on execution and liquidity, not just encrypted messages |
| Agent payment distribution | Coinbase, Nevermined, Skyfire | Existing adoption surfaces can own the customer relationship |
| Payment standards | x402, AP2, ERC-8183 | Reusable interfaces can make isolated proprietary functionality less valuable |
| Confidential execution | Aztec, Zama, Arcium and Fhenix | Other builders can construct similar application workflows |
| Future chain-native privacy | Circle Arc APS | Potential future substitute; the reviewed privacy page explicitly says unavailable today |

x402 already documents signed offers and receipts, variable authorization, and batching; AP2 addresses authorization mandates; draft ERC-8183 describes agent-job escrow and evaluation. They solve different parts of commerce and do not automatically supply Erebus's privacy properties — nonetheless, a strategy based on claiming that agent commerce lacks offers, receipts, budgets, or settlement machinery is inaccurate.

## Revenue model and pricing tests

Charge the purchasing company first. Supplier participation should be free during initial pilots, because requiring both sides to buy software increases friction. Keep the protocol and core SDK available to developers, subject to the repository's actual licensing terms, while charging for supported deployment, operations, and business functionality.

Begin with a fixed-scope paid pilot: a proposed $3,000 fee covers one organization, one stablecoin route, one supplier, deployment assistance, and an agreed acceptance exercise. It excludes a bespoke marketplace, new contracts for delivery guarantees, or unlimited integrations. A written scope should specify what happens if the technical acceptance criteria cannot be met.

The next offer is a $500 monthly supported workspace, with a proposed $1,500 monthly dedicated-deployment tier only when service scope and support costs justify it. These are asking-price experiments, not market-clearing prices. If most customers only need a simple stablecoin payment, Erebus will struggle to justify them against existing platforms.

| Revenue stream | Proposed starting offer | What must be true |
|---|---|---|
| Implementation | $3,000 fixed pilot | Delivery cost stays within scope and buyer has a funded workflow |
| Shared operation | $500 per organization per month | Integration and confidentiality save measurable effort or satisfy a real requirement |
| Dedicated deployment | From $1,500 per month | Buyer needs deployment control and pays the associated support premium |
| Embedded platform contract | Negotiated minimum plus usage after validation | Partner supplies qualified distribution and clear support boundaries |
| Bespoke engineering | Separately scoped project | Recognize it as services revenue, not recurring software revenue |

Network, pool, and external proving costs should be itemized and passed through during validation — avoid an uncapped promise to absorb them. Do not create a token or depend on reserve yield for the initial model; neither is necessary to sell the proposed purchasing software, and both would distract from testing customer value.

Published competitor pricing helps bound the experiment: Nevermined lists 1% on stablecoin merchant settlement and 2% on card rails, with processing passed through, plus optional $250 and $500 monthly organization plans; its external-service purchasing route lists a 2% fee.[^47] These are prices for a broader commerce service, not evidence that customers would accept the same percentage for Erebus confidentiality alone.

## Unit economics and revenue potential

The historical August 31 Erebus run records approximately 35.21 STRK in network and pool fees, excluding deposited principal; proposal, counteroffer, and settlement components sum to approximately 26.38 STRK for a funded channel. This is dated evidence, not a live quote — the economics must be remeasured on the intended stablecoin workflow.[^48]

At that historical round cost, keeping transaction infrastructure below 1% of payment value requires a payment above approximately 2,638 STRK; the comparable 5% threshold is approximately 528 STRK. A dollar conversion would depend on the applicable exchange rate and is intentionally omitted. Neither calculation includes subscription fees, funding and exit costs, support, or margin.

The customer's effective cost is the subscription allocated across payments plus all transaction and operational costs. At $500 monthly, ten payments allocate $50 of subscription cost to each payment before infrastructure; one hundred payments allocate $5 each. Larger batches may improve that ratio, but could change delivery timing, credit exposure, and reconciliation requirements.

**Illustrative operating scenarios** — planning scenarios, not forecasts or estimates of total market size. They assume network and proving costs are passed through and excluded from software revenue. Direct monthly service cost assumptions include infrastructure and support valued at $50/hour. Sales, core engineering, security review, and administration are excluded from contribution and must still be funded.

| Scenario | Paying customers | Monthly fee | MRR | Direct cost per customer | Monthly contribution |
|---|---|---|---|---|---|
| Early service | 5 | $500 | $2,500 | $200 | $1,500 |
| Repeatable niche | 25 | $800 average | $20,000 | $240 | $14,000 |
| Expanded product | 100 | $1,000 average | $100,000 | $200 | $80,000 |

The implied annual recurring revenue is $30,000, $240,000, and $1.2 million respectively. The final scenario assumes substantial improvement in support efficiency despite a higher average contract — that improvement must be demonstrated; custom integrations can instead make support cost increase with customer count.

A $3,000 implementation delivered in 30 hours at $50/hour leaves $1,500 before other expenses. At 70 hours, labor alone costs $3,500 and the engagement loses $500 before overhead — fixed-scope pilots are therefore a test of delivery repeatability as much as a revenue source.

With a $500 subscription and $200 direct monthly cost, contribution is $300 per customer. An assumed $15,000 monthly fixed expense base would require 50 such customers to cover it. A $1,000 sales acquisition cost would take about 3.3 months of contribution to recover, excluding onboarding losses and churn. These assumptions should be replaced with actual founder time, support tickets, infrastructure bills, and renewal data.

A pure 10-basis-point fee produces only $100 from $100,000 of monthly payment volume. Generating $10,000 in monthly gross revenue would require $10 million of volume before costs. Subscriptions provide a more credible early revenue experiment than relying on a small fraction of an unproven payment network.

## Market scope and expansion

There is insufficient evidence to publish a credible dollar TAM for confidential agent purchasing. Stablecoin transfer volume, agent counts, and hackathon registrations include activities unrelated to this buyer and cannot be multiplied by an arbitrary fee and presented as Erebus's addressable revenue.

Build the market estimate from qualified organizations instead. An initial discovery exercise might list 50 organizations, identify 20 with relevant active spend, find 8 that meet the privacy and integration criteria, secure 3 paid pilots, and retain 2 subscriptions — an illustrative funnel for planning, not an observed pipeline. Record the actual conversion at each step and the reasons for rejection.

At $1,000 monthly per customer, a $10 million ARR business requires approximately 834 paying organizations. At $12,000 monthly per platform contract, it requires approximately 70 platforms. Those counts show how different a substantial platform business would be from a handful of Starknet integrations; neither population has been established by the available evidence.

Starknet should be the first distribution and implementation base because Erebus already works with its infrastructure. The next market is the set of customers with the same purchasing problem, regardless of chain. Expand only when a paying buyer needs another chain and the expected contract contribution can fund the integration and continued maintenance — existing chain support is a practical advantage, not the customer's reason to buy.

## Defensibility and partnership strategy

The strongest potential advantage is a library of working supplier integrations, well-understood failure cases, repeatable deployments, and records that customers can reconcile without specialist assistance. A customer's reliance on a reliable purchasing workflow can be more durable than preference for one cryptographic implementation. Open-source code and standard interfaces also allow competitors to reproduce visible features, so this advantage requires execution and service quality.

Preserve data portability — a customer should be able to export purchase records and operate an agreed recovery path if Erebus becomes unavailable. Hidden lock-in would undermine trust in a product selling control over sensitive financial workflows. Dedicated operation and independent verification may be premium requirements, but their cost must be measured.

Pursue one application partner that already controls paid purchasing traffic. An integration should identify the account owner, data controller, support responsibilities, fee allocation, and which party sees private information. A logo exchange or jointly announced demo is not distribution unless it delivers qualified customers or sustained usage.

## Go to market and decision gates

The first offer should be specific enough for a buyer to reject: a supported confidential purchasing pilot with one established supplier, one settlement asset, and an explicit monthly operating price. Founder-led conversations should start with the buyer's last real purchase and existing payment process. Demonstrate Erebus only after confirming a costly or mandatory problem.

Ask what was bought, who approved it, how it was paid, which information needs protection from whom, and whether the supplier will cooperate. Ask why email plus a conventional payment, or an existing crypto finance platform, is insufficient. Request a sample sanitized workflow and an introduction to the budget owner. Interest without a workflow or budget should remain a research contact.

| Period | Deliverable | Decision evidence |
|---|---|---|
| Days 1–14 | 15 qualified interviews and three scoped opportunities | At least one buyer funds a pilot; otherwise revise segment before broad feature work |
| Days 15–30 | One supplier integration and supported stablecoin run | Buyer accepts privacy boundaries and measured cost; independent operator completes recovery |
| Days 31–60 | Repeated external purchases with finance records | Usage survives beyond a demo; support cost and failed-payment handling are measured |
| Days 61–90 | Three paying organizations sought | Repeat use, renewal intent, and contribution justify continued product investment |

The 90-day targets are management gates, not success guarantees. Track paying organizations, repeat purchases, renewal decisions, time to integrate, intervention rate, cost per completed purchase, and support hours. Founder-funded transfers, agent-generated activity, grants, and repository stars should be reported separately.

## Risks and conditions for changing direction

The largest risk is insufficient urgency. If qualified buyers accept ordinary settlement exposure or prefer bank-based confidentiality, a larger feature set will not fix the problem — seek a different workflow or keep Erebus as a supported integration business rather than inventing demand.

If buyers require hidden relationships and the current metadata model fails that requirement, change the architecture before selling the promise. If they reject external proving access, validate customer-controlled infrastructure or a different backend. If fees exceed accepted budgets, simplify settlement frequency or select a different transaction class; repeated microtransactions should not be the default experiment.

If each integration requires extensive custom work, price the work as services and narrow the supported workflow. If incumbent payment platforms satisfy requirements with a cheaper privacy route, Erebus may be more useful as an integration component than a separate customer application. If customers require escrow or service guarantees, treat that as a distinct security and product program rather than relabeling current acceptance and payment.

Before supporting material customer funds, obtain appropriate independent security review and agree operational responsibilities. The commercial model and target jurisdictions also need a specific legal assessment before custody, payment intermediation, or compliance assurances are offered. This strategy does not determine a licensing classification, and selected-deal disclosure alone does not establish regulatory compliance.

## Recommended commitment

Commit to one confidential supplier-purchasing pilot and a bounded validation period. Keep the Rust and STRK20 work as the implementation foundation, add the minimum operator workflow, and retain agents as an integration advantage. The first milestone is a company paying for a repeated purchase process with an existing supplier.

The supported-integration route can generate earlier revenue but may remain a small services business. A repeatable purchasing product could support a durable niche software company. A much larger outcome requires substantially more qualified buyers or embedded platform distribution than the evidence currently demonstrates. Additional protocol features should follow those customer signals.

## Sources

1. Request Finance. [Pricing](https://www.requestfinance.com/pricing). Accessed 11 September 2026.
2. Nevermined. [Agentic payments infrastructure](https://nevermined.ai/). Accessed 11 September 2026.
3. Coinbase. [Agentic Wallet overview](https://docs.cdp.coinbase.com/agentic-wallet/welcome). Accessed 2026-09-11.
4. Nevermined. [API Providers](https://nevermined.ai/use-cases/api-providers/). Accessed 11 September 2026.
5. Request Finance. [Customer stories](https://www.requestfinance.com/customers). Accessed 11 September 2026.
6. Skyfire. [Agent Trust Stack](https://skyfire.xyz/). Accessed 11 September 2026.
7. Curvy. [Stablecoin confidentiality product](https://curvy.box/). Accessed 2026-09-11.
8. Fat Solutions. [Tongo mainnet product and architecture](https://www.tongo.cash/). Accessed 2026-09-11.
9. Erebus. [Channel implementation](https://github.com/PoulavBhowmick03/Erebus/blob/e4dbc346e704b82ad0de4482e298df42e801f7fb/sdk/rs/src/channel.rs). Pinned source.
10. Erebus. [Status](https://github.com/PoulavBhowmick03/Erebus/blob/e4dbc346e704b82ad0de4482e298df42e801f7fb/docs/status.md). 2026-09-07.
11. Erebus. [MCP spending reservations](https://github.com/PoulavBhowmick03/Erebus/blob/e4dbc346e704b82ad0de4482e298df42e801f7fb/mcp-server/src/erebus_mcp/spending.py). Pinned source.
12. Erebus. [Privacy model](https://github.com/PoulavBhowmick03/Erebus/blob/e4dbc346e704b82ad0de4482e298df42e801f7fb/docs/privacy-model.md). Snapshot reviewed 2026-09-11.
13. Erebus. [Disclosure implementation](https://github.com/PoulavBhowmick03/Erebus/blob/e4dbc346e704b82ad0de4482e298df42e801f7fb/sdk/rs/src/disclosure.rs). Pinned source.
14. Starkience. [Private Sprint project index](https://github.com/starkience/strk20-hackathon/blob/main/projects.json). Snapshot accessed 2026-09-11.
15–24. STRK20 hackathon project READMEs (VINSS, APP20, Strkret, Kese, OXA, Offbook, Tony-Strk, ConditionalPay, Stealth Checkout) — individual GitHub repositories, accessed 2026-09-11.
25–27. Curvy and Tongo product/privacy documentation, accessed 2026-09-11.
28–43. Wider ecosystem sources — Nevermined, Coinbase, Skyfire, RAILGUN, Privacy Cash, Payy, Renegade, x402, AP2, ERC-8183, Aztec, Fhenix, Zama, Arcium, Circle Arc — accessed 2026-09-11.
47. Nevermined. [Pricing](https://nevermined.ai/pricing/). Accessed 11 September 2026.
48. Erebus. [Second mainnet canary: 0.6/0.4](https://github.com/PoulavBhowmick03/Erebus/blob/e4dbc346e704b82ad0de4482e298df42e801f7fb/docs/runs/2026-08-31-mainnet-060-040-canary.md). 2026-08-31.

[^1]: Request Finance, Pricing. Accessed 11 September 2026.
[^2]: Nevermined, Agentic payments infrastructure. Accessed 11 September 2026.
[^3]: Coinbase, Agentic Wallet overview. Accessed 2026-09-11.
[^4]: Nevermined, API Providers. Accessed 11 September 2026.
[^5]: Request Finance, Customer stories. Accessed 11 September 2026.
[^6]: Nevermined, Agentic payments infrastructure. Accessed 11 September 2026.
[^7]: Skyfire, Agent Trust Stack. Accessed 11 September 2026.
[^8]: Curvy, Stablecoin confidentiality product. Accessed 2026-09-11.
[^10]: Erebus, Channel implementation. Pinned source.
[^11]: Erebus, Status. 2026-09-07.
[^12]: Erebus, MCP spending reservations. Pinned source.
[^13]: Erebus, Privacy model. Snapshot reviewed 2026-09-11.
[^14]: Erebus, Disclosure implementation. Pinned source.
[^15]: Starkience, Private Sprint project index. Snapshot accessed 2026-09-11.
[^26]: Curvy, Stablecoin confidentiality product. Accessed 2026-09-11.
[^27]: Curvy, Privacy model. Accessed 2026-09-11.
[^28]: Nevermined, Agentic payments infrastructure. Accessed 11 September 2026.
[^29]: Coinbase, Agentic Wallet overview. Accessed 2026-09-11.
[^30]: Skyfire, Agent Trust Stack. Accessed 11 September 2026.
[^47]: Nevermined, Pricing. Accessed 11 September 2026.
[^48]: Erebus, Second mainnet canary: 0.6/0.4. 2026-08-31.