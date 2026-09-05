/**
 * Every value here is sourced from a document in this repository. Nothing is invented.
 *
 *   docs/status.md ................... the tiebreaker for current state
 *   docs/privacy-model.md ............ the only source for privacy claims
 *   docs/threat-model.md ............. the measured observer metrics
 *   docs/runs/2026-08-31-mainnet-060-040-canary.md
 *   docs/runs/v0.2-mainnet-canary.json
 *   README.md
 */

export const SOURCE = "https://github.com/PoulavBhowmick03/Erebus";
export const doc = (p: string) => `${SOURCE}/blob/main/${p}`;
export const starkscan = (h: string) => `https://starkscan.co/tx/${h}`;

/* ── Hero ledger: the second mainnet canary, 2026-08-31 ─────────────────── */

export type Field = {
  label: string;
  value: string;
  /** true when a public chain reader can read this. cinnabar. */
  leaks: boolean;
  note?: string;
};

export const SETTLEMENT: Field[] = [
  { label: "network", value: "SN_MAIN", leaks: true },
  { label: "counterparty", value: "0x0572…7189", leaks: true, note: "written in public calldata at channel open — F38" },
  { label: "submitting account", value: "0x6597…e54c", leaks: true, note: "the same identity signs every write" },
  { label: "block", value: "14147370", leaks: true },
  { label: "timestamp", value: "2026-08-31T11:51:10Z", leaks: true },
  { label: "notes created", value: "7", leaks: true, note: "wire v3 always creates seven" },
  { label: "amount paid", value: "0.6 STRK", leaks: false },
  { label: "change returned", value: "0.4 STRK", leaks: false },
  { label: "recipient", value: "account B", leaks: false },
  { label: "deal id", value: "10977364695535158093", leaks: false },
];

export const NEGOTIATION = [
  { step: "buyer opens", value: "0.48 STRK", who: "buyer-authored" },
  { step: "seller counters", value: "0.60 STRK", who: "seller-authored" },
  { step: "buyer accepts", value: "0.60 STRK", who: "settled atomically" },
];

/* ── What leaks at each step · docs/privacy-model.md ────────────────────── */

export type LeakRow = { step: string; hidden: string; open: string; severe?: boolean };

export const LEAKS: LeakRow[] = [
  {
    step: "0 · fund",
    hidden: "nothing",
    open: "depositor account, amount, token, timing — the whole ERC-20 leg",
  },
  {
    step: "1 · open channel",
    hidden: "the channel key",
    open: "the counterparty’s address, in the clear — plus the submitting account and timing",
    severe: true,
  },
  {
    step: "2–4 · offer, counter, final offer",
    hidden: "amount, token, deadline, memo hash, message type, replyTo",
    open: "submitting account, five salt values per message, note count, timing",
  },
  {
    step: "5 · accept and settle",
    hidden: "amount paid, recipient, change amount",
    open: "submitting account, that a settlement occurred, seven created notes on wire v3",
  },
  {
    step: "6 · grant",
    hidden: "everything — local only, no transaction",
    open: "nothing",
  },
  {
    step: "7 · reveal",
    hidden: "everything — local only, no transaction",
    open: "nothing",
  },
];

/* ── Measured observer results · docs/threat-model.md §4 ────────────────── */

export const METRICS = [
  {
    id: "M1",
    question: "Can an observer tell an Erebus transaction from other pool traffic?",
    result: "1.0000",
    detail:
      "wire v2, measured 2026-08-21 — 2 fixtures against 10,000 synthetic negatives, zero false positives",
    target: "0.5",
    bad: true,
  },
  {
    id: "M2",
    question: "Can an observer read the exact-vs-change bit from a settlement?",
    result: "0.5008",
    detail: "measured offline 2026-08-22 — wire v3 always creates seven notes",
    target: "0.5",
    bad: false,
  },
  {
    id: "M3",
    question: "How accurately can an observer count deals per account?",
    result: "exact",
    detail: "given M1. not separately measured",
    target: "bounded by M1",
    bad: true,
  },
  {
    id: "M4",
    question: "Can an observer link a submission to the pool identity acting?",
    result: "1.0",
    detail: "by construction — the same account signs every write. there is no relayer",
    target: "≈0",
    bad: true,
  },
] as const;

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
  pool_address: "0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a",
};

/* ── What this does not do · docs/status.md ─────────────────────────────── */

export type NonClaim = { title: string; body: string; ref?: string };

export const NON_CLAIMS: NonClaim[] = [
  {
    title: "Hide who you are dealing with.",
    body: "The counterparty’s address is written in public calldata at channel-open. This is upstream of our encryption and no wire change fixes it.",
    ref: "F38",
  },
  {
    title: "Hide that a negotiation happened.",
    body: "Wire v3 removes the fixed v2 salt classifier, but the submitting account, transaction timing, action shape, and note count remain public.",
  },
  {
    title: "Prove production readiness from two canaries.",
    body: "Two bounded mainnet workflows passed. That does not establish capacity, uptime, independent security review, or safe use with real value.",
  },
  {
    title: "Revoke facts already disclosed.",
    body: "A wire-v3 expiry stops a later verification. It cannot make a recipient forget a record opened before expiry.",
  },
  {
    title: "Escrow, or deferred delivery.",
    body: "Settlement is atomic, so there is no “agree now, deliver later”. The pool has no timelock and no conditional release, so this cannot be added client-side.",
  },
];

/* ── The tool surface · thirteen MCP tools, Protocol 4 ──────────────────── */

export const TOOLS = [
  "open_channel", "propose_offer", "counter_offer", "wait_for_offers",
  "read_channel_state", "accept_and_settle", "get_note_balance", "grant_viewing_key",
  "reveal", "reconcile", "resume_operation", "rebuild_state", "doctor",
] as const;

export const FACTS = [
  { k: "protocol", v: "Erebus 4" },
  { k: "wire", v: "v3 · AES-256-GCM-SIV" },
  { k: "release", v: "v0.2.0" },
  { k: "tests", v: "359 rs / 216 py / 43 ts" },
  { k: "friction entries", v: "40" },
  { k: "licence", v: "Apache-2.0" },
] as const;
