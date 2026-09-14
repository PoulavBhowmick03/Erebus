/**
 * Every value here is sourced from a document in this repository. Nothing is invented.
 *
 *   docs/status.md ................... the tiebreaker for current state
 *   docs/privacy-model.md ............ the only source for privacy claims
 *   docs/threat-model.md ............. the measured observer metrics
 *   docs/runs/2026-08-31-mainnet-060-040-canary.md
 *   docs/runs/v0.2-mainnet-canary.json
 *   agents/src/erebus_agents/demo.py . the replay
 *   README.md ........................ install and the MCP tool surface
 */

export const SOURCE = "https://github.com/PoulavBhowmick03/Erebus";
export const doc = (p: string) => `${SOURCE}/blob/main/${p}`;
export const starkscan = (h: string) => `https://starkscan.co/tx/${h}`;

/* ── Install · README.md ─────────────────────────────────────────────────── */

export const INSTALL = `uv tool install \\
  --extra-index-url https://poulavbhowmick03.github.io/Erebus/simple \\
  erebus-mcp-server`;

/* ── Evidence manifest · the 0.6/0.4 canary, all fees are receipt amounts ── */

export const MANIFEST = [
  { action: "Allowance, A", hash: "0x2a3eef681ef7f602fad690161868479bfd186e9e179a05fb71cb5e7afd469cc", block: "14146609", utc: "11:29:54Z", fee: "0.053831" },
  { action: "Allowance, B", hash: "0x6d9c7764b1583eae8ff39b8e716c6062d3d62e4d6943110ce86b8629f8a1f3a", block: "14146616", utc: "11:30:09Z", fee: "0.054907" },
  { action: "Screened shield", hash: "0x273b0f97f1c0707a259bbe5cacc337df6876509adba09b7376a0501a0f028b7", block: "14146663", utc: "11:31:27Z", fee: "2.727291" },
  { action: "Buyer proposal, 0.48", hash: "0x51fa13c6d11c529208785af163f2d5bc1cc95451192e3265150e0067aadeda4", block: "14147302", utc: "11:49:15Z", fee: "2.769592" },
  { action: "Seller counter, 0.6", hash: "0x6e55194809fec58c1426a8178dcdb10270d5e8aecfcfd6db2b28f6284ce5467", block: "14147331", utc: "11:50:03Z", fee: "2.769592" },
  { action: "Atomic settlement", hash: "0x79167f213952fb33a57eec6457963fa7dd7ba3a38160d5ef04540e91bd4f97a", block: "14147370", utc: "11:51:10Z", fee: "2.836143" },
] as const;

export const MANIFEST_TOTALS = {
  network: "11.211356 STRK",
  pool: "24 STRK",
  poolFee: "6 STRK per apply_actions",
};

/* ── The replay · agents/src/erebus_agents/demo.py ───────────────────────── */

export type ReplayStage = { id: string; title: string; hidden: string; open: string };

export const REPLAY: ReplayStage[] = [
  {
    id: "01",
    title: "open channel",
    hidden: "the channel key",
    open: "the counterparty’s address, the submitting account, timing",
  },
  {
    id: "02",
    title: "offer, counter, accept",
    hidden: "amount, token, deadline, memo hash",
    open: "the submitting account, note count, timing",
  },
  {
    id: "03",
    title: "settle atomically",
    hidden: "amount paid, recipient, change",
    open: "the submitting account, that a settlement occurred, seven created notes",
  },
];

export type TranscriptLine = {
  stage: number;
  text: string;
  secret?: string;
  tail?: string;
};

export const TRANSCRIPT: TranscriptLine[] = [
  { stage: 1, text: "opened encrypted channel", tail: "ch_b7afee5f…8d9af8" },
  { stage: 2, text: "buyer proposed", secret: "0.48 STRK" },
  { stage: 2, text: "seller countered", secret: "0.60 STRK" },
  { stage: 2, text: "buyer accepted the counteroffer" },
  { stage: 3, text: "accepted offer and shielded payment committed atomically" },
  { stage: 3, text: "deal-scoped viewing grant created for", tail: "0xauditor" },
  { stage: 3, text: "auditor reconstructed two offers and the settlement record" },
];

export const DEAL_SUMMARY = [
  { k: "channel", v: "ch_b7afee5f…8d9af8", hidden: false },
  { k: "participants", v: "buyer ↔ seller", hidden: false },
  { k: "agreed", v: "0.60 STRK", hidden: true },
  { k: "paid", v: "0.60 STRK", hidden: true },
] as const;

/* ── Who sees what · docs/privacy-model.md ───────────────────────────────── */

export const DISCLOSURE = [
  { who: "Public chain reader", terms: "Hidden" },
  { who: "Channel party", terms: "Readable" },
  { who: "Viewing-grant holder", terms: "Readable for one deal" },
] as const;

/* ── Three measured lines · docs/threat-model.md §4 ─────────────────────── */

export const OBSERVER = [
  {
    k: "M1 · wire v2 classifier",
    v: "1.0000",
    note: "an Erebus message, identified — the failure wire v3 set out to fix",
    bad: true,
  },
  {
    k: "M2 · wire v3 classifier",
    v: "0.5008",
    note: "chance, against the v3 fixture and 10,000 synthetic negatives",
    bad: false,
  },
  {
    k: "M4 · submission linkage",
    v: "1.0",
    note: "the same account signs every write; there is no relayer",
    bad: true,
  },
] as const;

/* ── What this does not do · docs/status.md ─────────────────────────────── */

export type NonClaim = { title: string; body: string };

export const NON_CLAIMS: NonClaim[] = [
  {
    title: "Prove production readiness from two canaries.",
    body: "Two bounded mainnet workflows passed — not capacity, uptime, or independent security review. Do not put value you care about through it.",
  },
  {
    title: "Revoke facts already disclosed.",
    body: "A wire-v3 expiry stops later verification. It cannot make a recipient forget a record already opened.",
  },
  {
    title: "Escrow, or deferred delivery.",
    body: "Settlement is atomic, so there is no “agree now, deliver later”. The pool has no timelock or conditional release, and neither can be bolted on client-side.",
  },
];

/* ── The tool surface · thirteen MCP tools, Protocol 4 ──────────────────── */

export const TOOL_GROUPS = [
  {
    label: "Negotiate, settle, disclose",
    tools: [
      "open_channel", "propose_offer", "counter_offer", "wait_for_offers",
      "read_channel_state", "accept_and_settle", "get_note_balance", "grant_viewing_key",
      "reveal",
    ],
  },
  {
    label: "Recovery & ops",
    tools: ["reconcile", "resume_operation", "rebuild_state", "doctor"],
  },
] as const;
