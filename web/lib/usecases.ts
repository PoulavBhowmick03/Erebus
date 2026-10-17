/**
 * The use-case guide and its two browser tools. Every statement is sourced from a file in this
 * repository, named next to it. Nothing is invented.
 *
 *   agents/README.md ................................. what each use case does
 *   agents/src/erebus_agents/metropolis_loop.py ...... the shared negotiate/settle/recover/deliver loop
 *   mcp-server/src/erebus_mcp/metropolis.py .......... the participant's MCP tools and their rules
 *   mcp-server/src/erebus_mcp/access.py .............. retrieval and metered access
 *   sdk/shielded/src/bin/erebus_negotiate.rs ......... operator-fixed price, asset and peer
 *   docs/metropolis-threat-model.md .................. what each surface makes visible
 *
 * These use cases live on the `metropolis` branch, so their source links point there, not at
 * `main` like the rest of the site.
 */

import { SOURCE } from "@/lib/content";

export const metropolisDoc = (p: string) => `${SOURCE}/blob/metropolis/${p}`;

/* ── The shared loop · metropolis_loop.py, metropolis.py, erebus_negotiate.rs ── */

export type LoopStep = { id: string; tool: string; who: string; body: string; source: string };

export const LOOP: LoopStep[] = [
  {
    id: "00",
    tool: "erebus-init",
    who: "each operator, once",
    body:
      "Writes the participant's keys, a signed service descriptor, a negotiation file and a payment file. The negotiation file fixes the minimum and maximum price, the asset template and exactly one peer descriptor. Keys stay in those files.",
    source: "sdk/shielded/src/bin/erebus_negotiate.rs",
  },
  {
    id: "01",
    tool: "negotiate_deal",
    who: "buyer and seller",
    body:
      "Both sides negotiate the fixed-policy deal over an encrypted channel and authorize it. The agent passes only an operation ID; on main it may omit it, and the tool creates, saves and returns one, and the seller can wait for a late buyer. The result is a deal commitment, never an amount, and it cannot submit a payment.",
    source: "mcp-server/src/erebus_mcp/metropolis.py",
  },
  {
    id: "02",
    tool: "settle_deal",
    who: "buyer only",
    body:
      "After a funding check, the buyer prepares, proves if shielded, and submits at most one broadcast. On main it first waits for the wallet's own pending transactions, and fund_wallet funds the note in one call. After a settlement attempt the agent never calls it again.",
    source: "agents/src/erebus_agents/metropolis_loop.py",
  },
  {
    id: "03",
    tool: "recover_deal",
    who: "buyer only",
    body:
      "Observes and reconciles until the payment is finalized. It cannot sign, prove or submit, so a pending result is never a reason to pay twice.",
    source: "mcp-server/src/erebus_mcp/metropolis.py",
  },
  {
    id: "04",
    tool: "retrieve_service_access",
    who: "buyer, when access is configured",
    body:
      "Fetches the paid resource and checks its hash against the signed agreement. On main the agent names the deal by its operation ID. A failed retrieval is retried by retrieving, never by paying. A download is not a delivery audit.",
    source: "mcp-server/src/erebus_mcp/access.py",
  },
  {
    id: "05",
    tool: "metropolis_audit.py",
    who: "an independent auditor",
    body:
      "The issuer exports one deal to the auditor's key. The auditor verifies the agreement, and the payment against chain data. Disclosure never proves delivery.",
    source: "agents/src/erebus_agents/metropolis_audit.py",
  },
];

/* ── Visibility · docs/metropolis-threat-model.md,
   mcp-server/src/erebus_mcp/metropolis.py server instructions ─────────────── */

export const VISIBILITY = {
  shielded:
    "Shielded settlement calldata carries the proof, chain, pool, asset, deal commitment and nullifier, root and note commitments. It does not carry the plain amount or participant keys.",
  publicBound: "Public-bound settlement exposes the accepted terms and parties.",
  transaction: "Every transaction shows its gas sender, target, timing and fees.",
  x402: "Standard x402 is public, not private: accepted terms and identities are visible.",
};

/* ── The use cases · agents/README.md ────────────────────────────────────── */

export type UseCase = {
  slug: string;
  name: string;
  line: string;
  how: string[];
  /** Facts a public chain reader can see for this use case. Rendered as leaks. */
  public: string[];
  command: string;
  sources: string[];
  tool?: { href: string; label: string };
};

export const USE_CASES: UseCase[] = [
  {
    slug: "discovery",
    name: "Find a counterparty",
    line: "Search sellers by what they do, what they accept, and their advertised price.",
    how: [
      "A seller's operator runs erebus-init, which signs a service descriptor: address, endpoints, chain, assets, settlement modes, expiry.",
      "The descriptor is indexed under a capability and an advertised unit-price band. Every other field is read from the file, and the file's bytes are pinned by SHA-256.",
      "A buyer searches by capability, asset, ceiling and mode. Resolve refuses an expired or swapped descriptor.",
      "The buyer's operator configures that file as its peer descriptor. Rust verifies the signature when it loads it; the index never claims to.",
    ],
    public: [],
    command: `uv run python agents/src/erebus_agents/discovery_demo.py --directory discovery.json \\
  search --capability summarize-documents --mode shielded`,
    sources: ["agents/src/erebus_agents/discovery.py", "sdk/transport/src/descriptor.rs"],
    tool: { href: "/tools/discovery/", label: "Browse an index" },
  },
  {
    slug: "marketplace",
    name: "Buy a service",
    line: "Pick the cheapest seller you can reach, pay once, and receive the result.",
    how: [
      "Sellers publish listings. The buyer's operator keeps a route file mapping each listing to the configured servers for that seller, because each negotiation file pins exactly one peer.",
      "The buyer picks the cheapest routed listing inside its ceiling. The advertised band only chooses the seller; the price is fixed by operator files and capped by the buyer's maximum price.",
      "The loop runs: negotiate on both sides, settle once, recover by observation until finalized.",
      "With access configured, the resource is retrieved and recorded as delivered with its hash. Delivery is an off-chain observation, never settlement evidence.",
    ],
    public: [VISIBILITY.shielded, VISIBILITY.publicBound, VISIBILITY.transaction],
    command: `uv run python agents/src/erebus_agents/marketplace_demo.py --registry registry.json \\
  --routes buyer-routes.json --capability summarize-documents \\
  --ceiling-unit-price 8 --operation <64-hex operation ID>`,
    sources: ["agents/src/erebus_agents/marketplace.py", "agents/src/erebus_agents/marketplace_demo.py"],
  },
  {
    slug: "offchain",
    name: "Buy compute, data or an API response",
    line: "The same purchase, named by what is being bought.",
    how: [
      "Compute, a dataset and an API response are the same shape: one buyer, one seller, one settlement, one off-chain deliverable.",
      "So this is the marketplace purchase under a capability for that kind. It needs no new mechanism.",
    ],
    public: [VISIBILITY.shielded, VISIBILITY.publicBound, VISIBILITY.transaction],
    command: `uv run python agents/src/erebus_agents/offchain_purchase_demo.py --kind compute \\
  --registry registry.json --routes buyer-routes.json \\
  --ceiling-unit-price 8 --operation <64-hex operation ID>`,
    sources: ["agents/src/erebus_agents/offchain_purchase_demo.py"],
  },
  {
    slug: "gateway",
    name: "Pay per API call",
    line: "Each call is one deal on the operator-selected x402-exact rail.",
    how: [
      "The buyer's x402 server exposes only negotiate_deal and retrieve_service_access.",
      "The first retrieval signs one local Permit2 authorization and the seller submits it. Every retry is another retrieval that reuses the retained permit.",
      "Operation IDs are written to an owner-only ledger before the first call, so an interrupted run resumes without fresh IDs and never pays twice.",
      "Nothing on this path verifies payment independently. An auditor does that from the disclosure grant.",
    ],
    public: [VISIBILITY.x402, VISIBILITY.transaction],
    command: `uv run python agents/src/erebus_agents/api_gateway_demo.py --buyer-negotiation buyer.json \\
  --buyer-access buyer-access.json --seller-negotiation seller.json \\
  --calls 3 --ledger calls.json`,
    sources: ["agents/src/erebus_agents/api_gateway_demo.py", "mcp-server/src/erebus_mcp/access.py"],
  },
  {
    slug: "metering",
    name: "Prepay for a bundle of calls",
    line: "One settlement buys N calls; the agent spends them and never overdraws.",
    how: [
      "The buyer's operator selects the metered access profile, which adds metered_service_status and call_metered_service.",
      "One run of the loop settles the bundle. Its size and price come from the operators' files.",
      "Calls are spent against the seller's reported allowance. A pending call is retried by calling again, never by paying, and nothing is called after exhaustion.",
      "Rust keeps the call cursor and the seller's issuer is the authority on how many calls were bought.",
    ],
    public: [VISIBILITY.shielded, VISIBILITY.publicBound, VISIBILITY.transaction],
    command: `uv run python agents/src/erebus_agents/saas_metering_demo.py --buyer-negotiation buyer.json \\
  --buyer-payment payment.json --buyer-access buyer-metered-access.json \\
  --seller-negotiation seller.json --operation <64-hex operation ID> --calls 5`,
    sources: ["agents/src/erebus_agents/metering.py", "agents/src/erebus_agents/saas_metering_demo.py"],
  },
  {
    slug: "receipt",
    name: "Show a deal receipt",
    line: "Turn an independent audit into a receipt a person can read.",
    how: [
      "An auditor runs metropolis_audit.py against the issuer's disclosure grant.",
      "The receipt keeps three facts apart: the agreement matched, the payment matched, and delivery was never proven by the protocol.",
      "A pending payment is shown as pending, not failure. A receipt that claims delivery verification is refused.",
    ],
    public: ["The deal commitment is in the settlement calldata, so a chain reader already has it."],
    command: `uv run python agents/src/erebus_agents/receipt_explorer.py --report audit.json \\
  --issuer <participant> --format html --out receipt.html`,
    sources: ["agents/src/erebus_agents/receipt_explorer.py", "agents/src/erebus_agents/metropolis_audit.py"],
    tool: { href: "/tools/receipt/", label: "Open a report" },
  },
];

/* ── Not yet on the real path · agents/README.md, PR 39 notes ─────────────── */

export const NOT_YET = [
  { name: "Request for quote", why: "There is no quote exchange, and descriptors carry no price by design." },
  { name: "Vendor selection", why: "It compares agreed amounts, and the real path never returns an amount to the agent." },
  { name: "Spending firewall and grants", why: "They check amounts the agent cannot see. Rust already enforces the per-deal maximum, the asset and the pinned peer." },
];
