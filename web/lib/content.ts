/**
 * Every value here is sourced from a document in this repository. Nothing is invented.
 *
 *   docs/status.md ................... the tiebreaker for current state
 *   docs/privacy-model.md ............ the only source for privacy claims
 *   docs/metropolis-threat-model.md ............. the measured observer metrics
 *   docs/runs/2026-08-31-mainnet-060-040-canary.md
 *   docs/runs/v0.2-mainnet-canary.json
 *   agents/src/erebus_agents/demo.py . the replay
 *   README.md ........................ install and the MCP tool surface
 */

export const SOURCE = "https://github.com/PoulavBhowmick03/Erebus";
export const X_HANDLE = "https://x.com/Erebus_pvt";
export const doc = (p: string) => `${SOURCE}/blob/main/${p}`;
export const starkscan = (h: string) => `https://starkscan.co/tx/${h}`;

/* ── System map · README.md, docs/assets/erebus-overview.excalidraw.svg ──── */

export const SYSTEM_MAP_ALT =
  "Erebus system overview: the stack from agent to pool, the three layers, a deal end to end, how an offer becomes five notes, the note frames, who sees what, and what the system does and does not claim";

/* ── Install · README.md ─────────────────────────────────────────────────── */

export const INSTALL = `uv tool install --python 3.12 \\
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
    open: "counterparty address, submitting account, timing",
  },
  {
    id: "02",
    title: "offer and counter",
    hidden: "amount, token, deadline, memo hash",
    open: "submitting account, note count, timing",
  },
  {
    id: "03",
    title: "settle",
    hidden: "amount paid, recipient, change",
    open: "submitting account, that a settlement occurred, seven notes",
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

/* ── Three measured lines · docs/metropolis-threat-model.md §4 ─────────────────────── */

export const OBSERVER = [
  {
    k: "M1 · wire v2 classifier",
    v: "1.0000",
    note: "identifies an Erebus message against 10,000 negatives",
    bad: true,
  },
  {
    k: "M2 · wire v3 classifier",
    v: "0.5008",
    note: "chance, on the same 10,000 negatives",
    bad: false,
  },
  {
    k: "M4 · submission linkage",
    v: "1.0",
    note: "the same account signs every write",
    bad: true,
  },
] as const;

/* ── What this does not do · docs/status.md ─────────────────────────────── */

export type NonClaim = { title: string; body: string };

export const NON_CLAIMS: NonClaim[] = [
  {
    title: "Not production ready",
    body: "Two mainnet workflows passed. That is not capacity, uptime, or an independent security review.",
  },
  {
    title: "Disclosure cannot be undone",
    body: "A wire-v3 expiry stops later verification. It cannot make a recipient forget a record they already opened.",
  },
  {
    title: "No escrow or deferred delivery",
    body: "Settlement is atomic, so there is no agree-now-deliver-later. The pool has no timelock and no conditional release.",
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

/* ── Configuration · README.md ───────────────────────────────────────────── */

/** One MCP client entry. Two agents means two of these, one per identity. */
export const MCP_CONFIG = `{
  "mcpServers": {
    "erebus-buyer": {
      "command": "erebus-mcp-server",
      "env": {
        "EREBUS_BACKEND": "seam",
        "EREBUS_NETWORK": "sepolia",
        "EREBUS_SETTLEMENT_ROLE": "payer",
        "AGENT_ADDRESS": "0x...",
        "STARKNET_RPC_URL": "https://...",
        "PROVING_SERVICE_URL": "https://...",
        "TOKEN_ADDRESS": "0x...",
        "POOL_KEY_FILE": "/home/you/.erebus-a/agent.pool.key",
        "ACCOUNT_KEY_FILE": "/home/you/.erebus-a/agent.account.key",
        "EREBUS_STATE_DIR": "/home/you/.erebus-a/state"
      }
    }
  }
}`;

export const ENV_VARS = [
  { k: "EREBUS_BACKEND", v: "mock · seam", note: "mock drives the whole surface with no chain" },
  { k: "EREBUS_NETWORK", v: "sepolia · mainnet", note: "" },
  { k: "EREBUS_SETTLEMENT_ROLE", v: "payer · payee", note: "accept_and_settle spends the caller’s notes" },
  { k: "AGENT_ADDRESS", v: "0x…", note: "the calling account" },
  { k: "STARKNET_RPC_URL", v: "https://…", note: "" },
  { k: "PROVING_SERVICE_URL", v: "https://…", note: "hosted prover" },
  { k: "TOKEN_ADDRESS", v: "0x…", note: "the shielded token" },
  { k: "POOL_KEY_FILE", v: "path", note: "key values never cross the binding" },
  { k: "ACCOUNT_KEY_FILE", v: "path", note: "" },
  { k: "EREBUS_STATE_DIR", v: "path", note: "locked, mode-0600 state" },
  { k: "EREBUS_SPENDING_LIMITS", v: "JSON, optional", note: "per-token cap on what accept_and_settle can spend" },
] as const;

/* ── The call path · CLAUDE.md ───────────────────────────────────────────── */

export const CALL_PATH = ["agents", "mcp-server", "sdk/py", "sdk/rs", "Starknet"] as const;

/* ── Concepts · docs/reference.md, docs/privacy-model.md ─────────────────── */

export const CONCEPTS = [
  {
    term: "channel",
    def: "The encrypted pair between two agents, returned as a `channel_handle` by `open_channel`. Opened once, it can carry more than one deal. The handle itself is not private. See F38.",
  },
  {
    term: "offer",
    def: "A price put forward with `propose_offer` or answered with `counter_offer`. An offer has no withdrawn state: it is accepted or it expires, so a short deadline is the only way to bound how long a stale price stays acceptable.",
  },
  {
    term: "deal",
    def: "One accepted offer, identified by a `deal_id`. A channel pair can run several deals; settling one does not close the pair.",
  },
  {
    term: "note",
    def: "A shielded unit of value inside the STRK20 pool. `accept_and_settle` spends the caller's notes, which is why it is restricted to the payer.",
  },
  {
    term: "operation_id",
    def: "The idempotency key on every write: `op_` plus 64 lowercase hex characters. Persist it before the call and reuse the same one after a restart. A new ID for a write that looks stuck is the wrong move. Call `reconcile` instead.",
  },
  {
    term: "viewing grant",
    def: "A file produced by `grant_viewing_key` that discloses one deal to one recipient. `reveal` reconstructs the deal from it. The configured pool key has to match the recipient the grant names.",
  },
] as const;

/* ── The tool surface, in full · docs/reference.md §The MCP tool surface ─── */

export const TOOL_DETAILS: Record<string, { signature: string; note: string }> = {
  open_channel: { signature: "(operation_id, counterparty)", note: "Returns channel_handle." },
  propose_offer: {
    signature: "(operation_id, channel_handle, amount, token, deadline, memo_hash)",
    note: "The payee asks. The payer offers.",
  },
  counter_offer: {
    signature: "(operation_id, channel_handle, reply_to, amount, token, deadline, memo_hash)",
    note: "Does not withdraw the offer it replies to.",
  },
  wait_for_offers: {
    signature: "(channel_handle, expected_count, timeout_seconds=300)",
    note: "One call instead of a poll loop. A timeout is not an error.",
  },
  read_channel_state: {
    signature: "(channel_handle)",
    note: "Every visible offer plus the settlement list.",
  },
  accept_and_settle: {
    signature: "(operation_id, channel_handle, offer_id)",
    note: "Payer only. Settles one deal. The pair can start another.",
  },
  get_note_balance: { signature: "()", note: "Payer must call before naming a price." },
  grant_viewing_key: {
    signature: "(operation_id, channel_handle, deal_id, grantee, expires_at, output_path)",
    note: "Writes a new mode-0600 file and returns no secret.",
  },
  reveal: { signature: "(grant_path)", note: "Reconstructs the selected deal." },
  reconcile: { signature: "()", note: "Read-only. Classifies journaled operations, never submits." },
  resume_operation: {
    signature: "(operation_id)",
    note: "Resumes one safe operation, or names the operator action it needs first.",
  },
  rebuild_state: {
    signature: "()",
    note: "Rebuilds missing channel records from the pool key and chain data.",
  },
  doctor: { signature: "()", note: "Read-only. Always safe to call." },
};

/* ── Responses and errors · docs/reference.md §Errors and retries ────────── */

export const RESPONSE_OK = `{"ok": true, "backend": "seam", "network": "sepolia", "result": {...}}`;
export const RESPONSE_ERR = `{"ok": false, "backend": "seam", "network": "sepolia", "error": {"code": "...", "message": "...", "retryable": false}}`;

export const ERROR_GROUPS = [
  {
    group: "The offer is wrong",
    codes: [
      "OFFER_EXPIRED",
      "OFFER_UNKNOWN",
      "ALREADY_SETTLED",
      "NOT_YOUR_OFFER",
      "AMOUNT_MISMATCH",
      "INSUFFICIENT_NOTES",
      "INDEX_CONFLICT",
    ],
    action: "Build a different offer. Retrying verbatim will not help.",
  },
  {
    group: "Funding or identity policy",
    codes: ["INSUFFICIENT_ALLOWANCE", "INSUFFICIENT_BALANCE"],
    action: "Change the allowance or fund the account before a new attempt.",
  },
  {
    group: "Durable operation state",
    codes: ["OPERATION_CONFLICT", "RECONCILIATION_REQUIRED"],
    action: "Keep the original operation_id. Inspect reconcile and follow its operator action.",
  },
  {
    group: "Transient",
    codes: ["SCREENING_UNAVAILABLE", "PROVER_UNAVAILABLE", "PROOF_EXPIRED", "SUBMIT_FAILED"],
    action: "Retry with backoff. PROOF_EXPIRED needs a fresh proof, not a resend.",
  },
  {
    group: "Terminal",
    codes: ["SCREENING_REJECTED"],
    action: "Stop. Not transient.",
  },
  {
    group: "Opaque",
    codes: ["PROOF_FAILED"],
    action: "The prover refused and gave no reason. Report it as unexplained.",
  },
  {
    group: "Before any protocol code ran",
    codes: ["INVALID_REQUEST", "IDENTITY_UNAVAILABLE"],
    action: "Fix the request or the key path. Never a chain-state problem.",
  },
] as const;

/* ── Version · docs/reference.md, docs/status.md ─────────────────────────── */

export const VERSION_NOTE =
  "This page documents CLI Protocol 4, published as v0.2.0, exposing the thirteen tools listed above. The older v0.1.0 speaks Protocol 2 and exposes ten. erebus-sdk refuses a mismatched CLI by protocol number rather than failing later on a changed shape.";

export const VERSION_BADGE = "Protocol 4 · v0.2.0";

/* ── Set up an identity · docs/reference.md §Set up an identity ──────────── */

export const IDENTITY_BOOTSTRAP = `scripts/new-identity.sh bootstrap erebus-a ~/.erebus-a <funder-account>`;

export const IDENTITY_KEYS = [
  {
    key: "Starknet account key",
    purpose: "Signs transactions. Custody",
    seenBy: "Never leaves the Rust process",
  },
  {
    key: "Pool private key",
    purpose: "The STRK20 identity. Confidentiality",
    seenBy: "Sent in compile_actions calldata to your prover and preflight RPC, both of which must be operator-controlled",
  },
  {
    key: "Pool auditor key",
    purpose: "Pool-wide, set once at registration",
    seenBy: "StarkWare's, no rotation",
  },
] as const;

/* ── The CLI protocol · docs/reference.md §The CLI protocol ──────────────── */

export const CLI_REQUEST = `echo '{"method":"doctor","params":{"config":{...}}}' | erebus-cli`;
export const CLI_RESPONSE = `{"ok": true, "protocol": 4, "result": {"ready": true, "checks": [...]}}`;

export const CLI_METHODS =
  "version, generate_pool_key, doctor, balance, allowance, approve, shield, open_channel, propose_offer, counter_offer, read_channel_state, accept_and_settle, reconcile, resume_operation, rebuild_state, grant_viewing_key, reveal";

/* ── Building from source · docs/reference.md §Building from source ──────── */

export const BUILD_CLONE = `git clone https://github.com/PoulavBhowmick03/Erebus && cd Erebus`;

export const BUILD_RUST = `cd sdk/rs && cargo test --all-targets && cd ../.. # 351 passed, 2 ignored`;

export const BUILD_PYTHON = `uv sync --all-packages && uv run pytest          # 154 tests`;

/* ── How it works · sdk/rs/src/wire.rs module docs, docs/status.md ───────── */

/** The 400-bit negotiation plaintext, most-significant-first. From wire.rs. */
export const WIRE_FIELDS = [
  { field: "type", bits: "8", note: "offer, counter, or acceptance" },
  { field: "replyTo", bits: "32", note: "the offer this one answers" },
  { field: "createdAt", bits: "40", note: "author timestamp" },
  { field: "amount", bits: "128", note: "the price, in base units" },
  { field: "deadline", bits: "64", note: "after which the offer expires" },
  { field: "memoHash", bits: "128", note: "commits to off-chain detail held elsewhere" },
] as const;

/** The three cryptographic jobs. Only the third requires a proof. */
export const CRYPTO_JOBS = [
  {
    job: "Negotiation confidentiality",
    mechanism: "Authenticated encryption and key agreement",
    proof: "No",
  },
  {
    job: "Agreement authorization",
    mechanism: "Signatures, or a proof when signer identity must stay hidden",
    proof: "Only to hide the signer",
  },
  {
    job: "Private settlement",
    mechanism: "The pool's privacy mechanism",
    proof: "Yes, for the shielded guarantee",
  },
] as const;

/* ── Privacy · docs/privacy-model.md, the canonical source ───────────────── */

export const PRIVACY_CLAIM =
  "Negotiation contents and settlement amounts are confidential. An observer reading public chain data cannot recover the amount, token, deadline, memo hash, message type, or reply structure of a negotiation, and cannot read the amount or recipient of the settlement.";

export const PRIVACY_NONCLAIM =
  "Erebus does not hide that a negotiation happened, and does not hide who it was with. An observer can identify pool interactions, count them, time them, attribute each to its submitting Starknet account, and at channel-open time read the counterparty's address directly out of public calldata.";

export const PRIVACY_ONE_LINE = "Erebus hides the terms, not the relationship.";

/** What leaks at each step of the workflow. From privacy-model.md. */
export const LEAK_STEPS = [
  {
    step: "0 · fund",
    hidden: "nothing",
    open: "depositor account, amount, token, timing. The whole ERC-20 leg.",
  },
  {
    step: "1 · open channel",
    hidden: "the channel key",
    open: "the counterparty's address, in the clear, plus the submitting account and timing",
  },
  {
    step: "2-4 · offer, counter, final",
    hidden: "amount, token, deadline, memo hash, message type, replyTo",
    open: "submitting account, five salt values per message, note count, timing",
  },
  {
    step: "5 · accept and settle",
    hidden: "amount paid, recipient, change amount",
    open: "submitting account, that a settlement occurred, the created note count",
  },
  {
    step: "6-7 · grant and reveal",
    hidden: "everything. Local only, no transaction.",
    open: "nothing",
  },
] as const;

/** The known leaks, in descending severity. From privacy-model.md. */
export const KNOWN_LEAKS = [
  {
    n: "0",
    title: "The counterparty address is in public calldata",
    body: "open_channel compiles to three server actions, and the first carries recipient_addr as a plain ContractAddress. It is the storage map key for the recipient's channel info, so it cannot be hashed, and server actions are serialized directly into public apply_actions calldata. Both directions must be opened for a conversation to work, so the edge is recorded twice. No amount of wire-level encryption touches this.",
    fix: "Needs a design, not a patch. Tracked as friction F38.",
  },
  {
    n: "1",
    title: "The historical wire-v2 fifth-salt fingerprint",
    body: "Wire v2 fills 536 of 595 payload bits and zero-fills the remaining 59, so the fifth salt of every message has bit 119 pinned and bits 60 through 118 clear, whatever the message says. That shape identifies an Erebus message essentially every time.",
    fix: "Fixed in wire v3, which is the source default. It carries a 64-bit deal id and masks the spare bits with a separately derived keystream. Tracked as F31.",
  },
  {
    n: "2",
    title: "Submission linkability",
    body: "Every write is an apply_actions transaction signed by a public Starknet account. The account that opens a channel, writes each offer, and settles is the same visible identity across one deal. An observer who cannot read a single term can still count and time an account's deal flow.",
    fix: "Unlinkable submission is possible today without a protocol change, because nothing binds the transaction submitter to the pool identity whose actions are applied. Not implemented.",
  },
  {
    n: "3",
    title: "The public funding leg",
    body: "Shielding is a real ERC-20 transfer. Depositor, amount, token, and timing are public, and they precede the first private action by a bounded interval.",
    fix: "None within this design. Funding correlation is an ecosystem-level problem.",
  },
  {
    n: "4",
    title: "Note count on settlement",
    body: "A settlement creates six notes when the payer's selected inputs match the price exactly, and seven when they overshoot and a change note is minted. That leaks one bit about the payer's holdings on every deal. Amounts stay private.",
    fix: "Always mint a change note, zero-valued when unneeded, so the count is constant. Not done.",
  },
] as const;

/** Endpoints that see more than the chain does. From privacy-model.md. */
export const INFRA_VISIBILITY = [
  { endpoint: "The prover", how: "receives compile_actions calldata", key: "Sees the pool key" },
  { endpoint: "The write RPC", how: "receives the preflight call", key: "Sees the pool key" },
  { endpoint: "The submitted transaction", how: "apply_actions on chain", key: "Does not" },
] as const;

/* ── Limits · docs/status.md, docs/production-gaps.md ────────────────────── */

export const NOT_DOES = [
  {
    title: "Hide who you are dealing with",
    body: "The counterparty's address is written in public calldata at channel-open. This is upstream of our encryption and no wire change fixes it.",
  },
  {
    title: "Hide that a negotiation happened",
    body: "Wire v3 removes the fixed v2 salt classifier, but the submitting account, transaction timing, action shape, and note count remain public.",
  },
  {
    title: "Prove production readiness from bounded runs",
    body: "Four bounded mainnet workflows passed. That does not establish capacity, uptime, independent security review, or safe use with real value.",
  },
  {
    title: "Revoke facts already disclosed",
    body: "An expiry stops a later verification. It cannot make a recipient forget a record opened before expiry.",
  },
  {
    title: "Escrow, or deferred delivery",
    body: "Settlement is atomic, so there is no agree now, deliver later. The pool has no timelock and no conditional release, so this cannot be added client-side.",
  },
] as const;

export const PROD_GAPS = [
  {
    area: "Custody and infrastructure",
    body: "The prover and preflight RPC receive the pool private key, so a hosted provider sits inside the identity's confidentiality boundary. Production needs a written provider policy, endpoint rotation and revocation, a supported self-hosted fallback, tested backup and restore, and a key-loss drill.",
  },
  {
    area: "Transaction safety",
    body: "Protocol 4 has durable operation ids and reconciliation. Production still needs long-running failure tests against real provider timeouts, journal pruning that preserves recovery evidence, spending limits enforced in Rust across restarts, and operator alerts for ambiguous operations.",
  },
  {
    area: "Security review",
    body: "No independent cryptographic or security review covers the wire, the settlement binding, the disclosure design, the hosted-prover transport, or the recovery journal.",
  },
  {
    area: "Scale and operations",
    body: "Suitable only for bounded, low-frequency workflows. Provider latency, RPC load across long channels, pool fees, concurrent negotiations, and restore time are all unmeasured at scale.",
  },
  {
    area: "Product",
    body: "No delivery-versus-payment, escrow, refunds, deferred execution, or outcome-only proofs. A scoped grant reveals a deal record. It does not prove external delivery.",
  },
] as const;

export const DISCLOSURE_PROVES =
  "That the listed on-chain note values authenticate and decrypt under the supplied deal capability, that an acceptance exists in that record, and what its listed payment note carries. agreed_amount and paid_amount stay separate so a reader can compare the acceptance with the payment.";

export const DISCLOSURE_ASSERTS =
  "The named participant addresses and the issuer. The capsule is encrypted and authenticated but it is not signed by the grantor. It also asserts all business meaning: memo_hash commits to off-chain detail whose preimage lives outside this wire. There is no separate proof establishing the business meaning to an external verifier.";

/* ── Walkthrough · docs/runbook.md, the reproducible seven steps ─────────── */

export const WALK_OPEN = `# A opens its direction and proposes
HANDLE_A=$(scripts/agent.sh ~/.erebus-a/env open \\
  "$(scripts/agent.sh ~/.erebus-b/env whoami)")
scripts/agent.sh ~/.erebus-a/env balance
scripts/agent.sh ~/.erebus-a/env offer "$HANDLE_A" 600000000000000000`;

export const WALK_COUNTER = `# B opens its own direction, then sees A's offer there
HANDLE_B=$(scripts/agent.sh ~/.erebus-b/env open \\
  "$(scripts/agent.sh ~/.erebus-a/env whoami)")
scripts/agent.sh ~/.erebus-b/env status "$HANDLE_B"
scripts/agent.sh ~/.erebus-b/env counter "$HANDLE_B" them:0 1000000000000000000`;

export const WALK_SETTLE = `# A reads B's counter through A's direction and settles
scripts/agent.sh ~/.erebus-a/env status "$HANDLE_A"
scripts/agent.sh ~/.erebus-a/env accept "$HANDLE_A" them:0`;

export const WALK_SHIELD = `python3 "$REQ" "$ENV" shield '{"amount":"1000000000000000000"}' | "$CLI"`;

export const SETTLE_RESULT = ["tx_hash", "nullifiers", "proved_at", "selected_input", "change"] as const;

/* ── Docs site nav · one entry per page, in reading order ─────────────────── */

export const DOCS_PAGES = [
  { n: "01", href: "/docs", label: "Quickstart" },
  { n: "02", href: "/docs/how-it-works", label: "How it works" },
  { n: "03", href: "/docs/concepts", label: "Core concepts" },
  { n: "04", href: "/docs/walkthrough", label: "Walkthrough" },
  { n: "05", href: "/docs/tools", label: "Call the tools" },
  { n: "06", href: "/docs/errors", label: "Responses and errors" },
  { n: "07", href: "/docs/privacy", label: "Privacy model" },
  { n: "08", href: "/docs/limits", label: "Limits" },
  { n: "09", href: "/docs/architecture", label: "Architecture" },
] as const;

/* ── In-page sections per page · only pages with more than one, mirrors
   the DocSection id/title props actually used on that page ─────────────── */

export const PAGE_SECTIONS: Record<string, { id: string; label: string }[]> = {
  "/docs": [
    { id: "install", label: "Install" },
    { id: "identity", label: "Set up an identity" },
    { id: "configure", label: "Configure an identity" },
  ],
  "/docs/how-it-works": [
    { id: "shape", label: "The shape of the problem" },
    { id: "salts", label: "Where a negotiation lives" },
    { id: "deal", label: "One deal, end to end" },
    { id: "settle", label: "What settlement enforces" },
    { id: "disclose", label: "Disclosure afterwards" },
  ],
  "/docs/walkthrough": [
    { id: "before", label: "Before you start" },
    { id: "negotiate", label: "Negotiate" },
    { id: "settle", label: "Settle" },
    { id: "disclose", label: "Disclose one deal" },
    { id: "recover", label: "When a write looks stuck" },
  ],
  "/docs/privacy": [
    { id: "claim", label: "The claim and the non-claim" },
    { id: "steps", label: "What leaks at each step" },
    { id: "leaks", label: "The known leaks" },
    { id: "infra", label: "Infrastructure sees more" },
    { id: "record", label: "What a disclosed record proves" },
  ],
  "/docs/limits": [
    { id: "not", label: "What Erebus does not do" },
    { id: "production", label: "What is unfinished" },
    { id: "use", label: "Where that leaves you" },
  ],
  "/docs/architecture": [
    { id: "map", label: "System map" },
    { id: "boundary", label: "Know the boundary" },
    { id: "cli", label: "The CLI protocol" },
    { id: "build", label: "Build from source" },
    { id: "source", label: "Read the source of truth" },
  ],
};

/* ── Docs search index · titles and snippets a reader would actually type ── */

export const SEARCH_INDEX = [
  { title: "Install", href: "/docs#install", snippet: "uv tool install erebus-mcp-server" },
  {
    title: "Set up an identity",
    href: "/docs#identity",
    snippet: "new-identity.sh bootstrap, pool key, account key, auditor key",
  },
  {
    title: "Configure an identity",
    href: "/docs#configure",
    snippet: "mcpServers, EREBUS_BACKEND, env vars",
  },
  {
    title: "EREBUS_SPENDING_LIMITS",
    href: "/docs#configure",
    snippet: "per-token cap on what accept_and_settle can spend",
  },
  {
    title: "EREBUS_BACKEND",
    href: "/docs#configure",
    snippet: "mock or seam. mock drives the whole surface with no chain",
  },
  {
    title: "channel, offer, deal",
    href: "/docs/concepts#concepts",
    snippet: "core concepts: channel_handle, deal_id, note",
  },
  {
    title: "operation_id",
    href: "/docs/concepts#concepts",
    snippet: "the idempotency key on every write",
  },
  {
    title: "viewing grant",
    href: "/docs/concepts#concepts",
    snippet: "grant_viewing_key, reveal: disclose one deal to one recipient",
  },
  {
    title: "How it works",
    href: "/docs/how-it-works#salts",
    snippet: "negotiation rides in note salts, no contract of our own",
  },
  {
    title: "Note salts, the 400-bit message",
    href: "/docs/how-it-works#salts",
    snippet: "type, replyTo, createdAt, amount, deadline, memoHash across five notes",
  },
  {
    title: "What settlement enforces",
    href: "/docs/how-it-works#settle",
    snippet: "atomic action set, amount equality is a client check not a proof predicate",
  },
  {
    title: "Walkthrough, a full deal",
    href: "/docs/walkthrough#negotiate",
    snippet: "both sides open their own direction, offer, counter, accept",
  },
  {
    title: "Privacy model",
    href: "/docs/privacy#claim",
    snippet: "Erebus hides the terms, not the relationship",
  },
  {
    title: "What leaks",
    href: "/docs/privacy#leaks",
    snippet: "counterparty address in calldata, submission linkability, funding leg, note count",
  },
  {
    title: "The prover sees the pool key",
    href: "/docs/privacy#infra",
    snippet: "prover and write RPC sit inside the confidentiality boundary",
  },
  {
    title: "Limits, what Erebus does not do",
    href: "/docs/limits#not",
    snippet: "no escrow, no deferred delivery, no relationship privacy, unaudited",
  },
  {
    title: "open_channel, propose_offer, counter_offer",
    href: "/docs/tools#tools",
    snippet: "negotiate, settle, disclose",
  },
  {
    title: "accept_and_settle",
    href: "/docs/tools#tools",
    snippet: "payer only, spends the caller's notes",
  },
  {
    title: "reconcile, resume_operation, rebuild_state, doctor",
    href: "/docs/tools#tools",
    snippet: "recovery and ops",
  },
  {
    title: "Responses and errors",
    href: "/docs/errors#responses",
    snippet: "the ok/error envelope, retryable, error code groups",
  },
  {
    title: "System map",
    href: "/docs/architecture#map",
    snippet: "the stack, the three layers, one deal start to finish, note economics",
  },
  {
    title: "Know the boundary",
    href: "/docs/architecture#boundary",
    snippet: "agents → mcp-server → sdk/py → sdk/rs → Starknet",
  },
  {
    title: "The CLI protocol",
    href: "/docs/architecture#cli",
    snippet: "erebus-cli, stdin/stdout JSON envelope, protocol 4",
  },
  {
    title: "Build from source",
    href: "/docs/architecture#build",
    snippet: "git clone, cargo test, uv sync --all-packages",
  },
  {
    title: "Read the source of truth",
    href: "/docs/architecture#source",
    snippet: "runbook.md, reference.md, ARCHITECTURE.md, status.md",
  },
] as const;
