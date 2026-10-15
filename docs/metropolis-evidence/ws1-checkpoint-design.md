# WS1: checkpointed history start for a fresh wallet (design note)

Status: local implementation note, 2026-10-14. Base: `origin/main` `cd1447d8d50e067e79c72a3e942c459be3e755b3`.
Storage: tested against the **current `IndexStore` on `main`** (single JSON file, 128 MiB cap). The
segmented-store candidate (`review/metropolis-cold-history-20261014`, F1–F3 open) is **not** mixed in.
A checkpointed index is small by construction, so WS1 does not depend on it; long-lived wallets still do.

## 1. Feasibility (pool contract `contracts/evm/src/ErebusShieldedPool.sol`)

- Incremental Merkle tree, `TREE_DEPTH = 20`, two-input Poseidon (`poseidonTwo`, circomlibjs `createCode(2)`,
  runtime pinned in the deployment record). `zeroHashes[0] = 0`, `zeroHashes[l+1] = H(z_l, z_l)`.
- Public state: `nextLeafIndex`, `currentRoot`, `zeroHashes[20]`, `filledSubtrees[20]`, `knownRoots`
  (never evicted: any root ever produced stays spendable-against), `insertedCommitments` (no duplicates),
  `consumedDeals`, `consumedNotes`.
- `_insert(leaf)` at index `i`: for level `l`, if bit `l` of `i` is 0 then `filledSubtrees[l] := node` and
  `node := H(node, z_l)`; else `node := H(filledSubtrees[l], node)`.

Claim (proved by the vectors below, not by provider agreement): for `0 < n < 2^20` leaves, with `i = n-1` and
`L*` the lowest zero bit of `i`, `currentRoot` is recomputed by starting at `filledSubtrees[L*]` and walking up,
using `filledSubtrees[l]` as the left sibling where bit `l` of `i` is 1 and `z_l` as the right sibling where it
is 0; at every zero-bit level above `L*` the stored `filledSubtrees[l]` must equal the recomputed node.
The entries used are exactly `{L*} ∪ {l > L*: bit_l(i) = 1}` = `{l : bit_l(n) = 1}`, the only entries the
next insertion reads (complete left subtrees). Entries below `L*` are overwritten before use and unverifiable.
Consequences:
- **root:** recomputable from `(filledSubtrees, n)`; `n = 0` is the constructor state (all zero hashes);
  `n = 2^20` (full) has no zero bit and is refused (nothing can be appended).
- **append:** the verified frontier is the complete state the contract itself uses to insert leaf `n`.
- **membership paths for leaves `j >= n`:** every node needed is either built from post-checkpoint leaves,
  zero, or a complete pre-checkpoint left block, and every such block reached is a frontier ommer
  `filledSubtrees[l]` at a level where bit `l` of `n` is 1 (block index `(n >> l) - 1`).
- **not available:** leaves, commitments, nullifiers and deal events before the checkpoint. A checkpointed index
  can never construct a path for a leaf `< n` or prove absence of an earlier event.
- **count binding (found by the vectors):** `(filledSubtrees, root)` does not uniquely fix `n`. A tree of `n` leaves
  has the same root and frontier as `n + 1` leaves whose last leaf is zero, because a zero leaf hashes like empty
  space. The contract never inserts zero (`invalid note`), so such a state cannot exist on chain. `n` is therefore
  bound by the paired `nextLeafIndex` read; an inflated count is refused at the next real insertion (its event
  index does not match) and by the post-sync `nextLeafIndex` check. One fewer leaf is always rejected.

## 2. The checkpoint contract

Retained (`TreeCheckpoint`): finalized block number `B` and hash; `leaf_count = nextLeafIndex`; `root =
currentRoot`; `filled_subtrees[20]`. The deployment anchor (`first_block`, `first_hash`, runtime and dependency
pins) is **kept unchanged** and still authenticated by every binary as today. `scan_start = B + 1`.

Acquisition at a common finalized block `B`:
1. both independently configured providers return the same chain, finalized `B` and hash (`shared_snapshot`);
2. each provider authenticates the pool runtime and dependency pins at `B` (existing pin rules);
3. each provider reads `nextLeafIndex`, `currentRoot`, `filledSubtrees[0..19]`, `knownRoots(currentRoot)` with
   `eth_call` pinned to `{blockHash: B, requireCanonical: true}`;
4. the two readings must be identical; the frontier must recompute to `currentRoot` (local Poseidon) and
   `knownRoots(currentRoot)` must be true;
5. both providers recheck that `B` is still canonical and finality has not regressed.

State read at `B` is the state after every transaction of `B`; logs are scanned from `B + 1`, so no insertion is
skipped or applied twice. The first scanned block must have `parent_hash = hash(B)`.

Trust: the same two-provider, finalized-state assumption every sync already relies on for headers and logs.
Provider agreement is **not** a cryptographic state proof; two colluding or identically compromised providers
could return a false frontier. The local root recomputation proves only that the frontier is internally
consistent with the returned root. A full-history scan remains available and unchanged for anyone who needs to
remove the checkpoint's reliance on a single finalized state read.

## 3. Eligibility (fresh participant only, explicit opt-in)

`erebus-wallet checkpoint` requires `"confirm_fresh_wallet": true`. It refuses unless every history-bearing store
under the participant `state_root` is absent or holds only lock files: `wallet-index/`, `wallet-operations/`,
deposit requests, `coordinator/`, `payment-observation/`, access `pool-history*.json` / `public-history/`, and the
signer journal for this chain and signer. The encrypted wallet file must be absent or decrypt to a snapshot with
no notes, reservations or retained transfer choices. Unreadable or corrupt state refuses. Intended order: init,
checkpoint, then wrap/approve/deposit.
Limitation: an operator who deletes every local file is indistinguishable from a new participant; notes received
before the checkpoint are then invisible to that state root and recoverable only with a separate full-history
state root. Repeating the command with an existing matching record is idempotent and acquires nothing.

## 4. Durable form

`<state_root>/history-checkpoint.json` (version 1, owner-only, created with create-new + fsync + rename, bound to
chain, pool and deployment anchor). Every participant binary that builds an `IndexDomain` under that state root
loads the record. `IndexDomain` carries the checkpoint, so primary and peer caches must share it and a cache
built for another checkpoint (or for full history) fails the existing domain-equality check. Checkpointed caches
are written as index version 3; full-history caches are byte-identical to version 2 and older binaries reject
version 3 instead of misreading it.

## 5. Consumers reviewed

| Consumer | Behaviour with a checkpoint |
|---|---|
| `PoolIndex::path` (transfer prep `lib.rs:391/513`, withdraw) | Sparse construction from frontier ommers + post-checkpoint leaves; a leaf `< n` is refused. |
| `PoolIndex::is_consumed` (`lib.rs:502`) | Only queried for wallet notes, all inserted after `B`; their consumption cannot precede insertion. |
| Payment observation (`observation.rs` `indexed_evidence`) | Already cross-checks the index against `consumedDeals` state at head and final; a deal consumed before `B` yields an explicit `HistoryIncomplete`, never "unpaid". |
| Wallet sync / recovery replay (`recovery.rs`) | Scans from `B + 1`; wallet rewinds only at or after `B + 1`. |
| Finalized operation reconciliation | Uses receipts of the participant's own operations, all after `B`. |
| Status | Reports `history_from_block = B + 1` so callers cannot read it as a deployment-era scan. |
| Auditor disclosure (`erebus-shielded-disclosure`) | Separate cache root, no record: full history, unchanged. |
| Seller access service (`access-state`) | Honours a record in its own state root. Generated configs place the access service in a separate state root, so today the record must be placed there (the WS1 harness rehearsal copies it). Packaging must create or share it: integration gate. Without it the service scans full history (safe, slow). |
| Relayer admission, public pool indexer | Unchanged (no participant record). |
| Backup/restore, endpoint change | The record and caches move together with the state root; a restored cache from another checkpoint fails the domain check. |

## 6. Final-source results (2026-10-14)

Source identity `33d0fa5f6c84b33563fe13e217868ebc328a499cb327f2ba9cfee845b8fe4de3` (candidate-input tree; base
`cd1447d`, uncommitted). Hardware: Apple M4 Pro, 14 cores, 24 GiB. Local Anvil unless stated.

| Gate | Command | Result |
|---|---|---|
| Default suite | `cargo test --locked --release` (sdk/shielded) | 251 passed, 0 failed, 83 ignored (funded/artifact tests) |
| Contract vectors | `cargo test --locked --release --test checkpoint_vectors` | 7/7: every Solidity frontier recomputes its root; appends match every emitted root; paths equal full-history paths; near-full boundary through leaf 1,048,575; unused levels never read |
| Record/eligibility | `cargo test --locked --release --lib checkpoint::` | 5/5: binding, tamper/corrupt/version refusal, lock, interruption at FileSynced/Renamed/DirectorySynced, freshness |
| Paired acquisition | `cargo test --locked --release --test wallet_ops -- --ignored paired_checkpoint` | honest = full replay; every single-provider fault and identical frontier/root/membership lie refused; anchor change, finality regression, wrong pins/deployment refused; inflated count refused at next insertion; racing deposits applied once |
| Checkpoint command | `... --ignored wallet_checkpoint_is_explicit` | opt-in; each history store refuses with bytes unchanged; concurrent refused; no signing, nonce read or log scan; idempotent with zero RPC; full-history wallet cannot convert; copied record fails closed |
| WMON end-to-end | `... --ignored fresh_wallet_from_checkpoint` | 10 real-proof deposits + 3,000 blocks before join; checkpoint, wrap, approve, 2 deposits, 2 real withdraw proofs, unwrap; 7 broadcasts; root, count and every path equal a full-history wallet |
| Harness end-to-end | `scripts/rehearse-metropolis-workflow.py ... --pools wmon --demo-main-only --checkpoint-fresh-participants 2000` against a registry built from this tree (`0.3.0.dev2026101499`, rehearsal pins, not publishable) | `verified`: lost settlement response, buyer restart, recovery with 1 settlement broadcast, seller restart, resource hash, auditor agreement+payment, wrong issuer refused; all 8 participant caches version 3 starting at checkpoint + 1 |
| Read-only Monad | `erebus-wallet checkpoint` from a throwaway state root, public `testnet-rpc.monad.xyz` + `rpc.ankr.com`, published 1307 pins | paired checkpoint at block 70,908,176, 3 leaves, root verified, 24.9 s; nothing signed or sent |
| Lint | `cargo fmt --check`; `cargo clippy --release --lib --bins --test checkpoint_vectors --test wallet_ops --test index_store --test indexer --test rpc_finality -D warnings` | clean. Unscoped `--tests` clippy fails on the unchanged base too (`relay_client.rs:225`, `relay_adversarial.rs:343`). |
| Python | `uv run pytest scripts/tests` | 396 passed, 9 skipped |

Measurements (WMON end-to-end, 3,094-block history, 11 leaves at checkpoint, local Anvil, 1 s blocks):
checkpoint acquisition 3.3 s; first sync until wrap final 2.9 s; first post-checkpoint withdraw proof 2.3 s;
restart sync 1.4 s; equivalent full-history first sync 169.9 s with 6,604 header reads on the primary provider.
Acquisition alone: 68 requests per provider (31 `eth_call`, 8 headers, 10 `eth_getCode`, 16 `eth_chainId`,
3 `eth_blockNumber`), zero `eth_getLogs`. The checkpointed wallet read no log range at or below the checkpoint
and no header at or below it except the deployment block and its parent (runtime authentication, DM8-8).
Local numbers do not predict public-RPC catch-up time; no onboarding duration is claimed from them.

## 6a. Live Monad testnet run (2026-10-15)

Freshly onboarded buyer and seller (installed `erebus-init --metropolis` from published `0.3.0.dev2026101307`;
configs pointed at the WS1 Rust CLIs built from this branch), public Monad RPC as primary and a keyed provider as
peer, both paced at 25 requests per second. Driven through the two MCP servers.

| Step | Result |
|---|---|
| Checkpoints | buyer block 71,206,939 and seller block 71,207,005, about 20 s each; a wallet with history refused with no bytes changed |
| Funding | wrap, approve, real-proof deposit of 0.003 WMON; spendable after 395 s |
| Deal 1 | negotiated; funding `ready` in 49 s; one settlement broadcast; buyer restarted before recovery; payment verified by observation; pool token balance unchanged; seller access restarted; resource SHA-256 verified |
| Deal 2 | spent the post-checkpoint 0.002 WMON change note; one broadcast; payment verified; resource verified |
| Caches | all six buyer and seller-access caches are version 3 and start at their own checkpoint + 1; no separate payment wallet cache |

Total 1,728 s. Earlier live attempts on the first commit found three defects, fixed in the follow-up commit:
the payment command replayed pool history twice more in its own caches (it now uses the wallet's finalized
caches and seeds observation from them); a lost save race on a shared cache was a hard error (now
`HistoryPending`); and a public provider's 429 throttling exhausted three 100-400 ms retries (now four retries
with 0.5-4 s backoff or `Retry-After`). Monad charges the gas limit: about 0.31 MON per buyer transaction (3.375 MON over 11) at the
generated 3,000,000 gas limit.

## 7. Remaining gates

- **Integration/review:** Rust protocol review (repository rule) of `indexer.rs`, `index_store.rs`, `rpc.rs`,
  `checkpoint.rs`, `recovery.rs`, `observation.rs`; decision that the paired finalized-state trust is acceptable.
- **Packaging:** generated seller configs keep the access service in a separate `access-state`; the record must be
  created or shared there (the harness placed it). `erebus-init --metropolis` should offer the checkpoint step.
- **Not exercised end to end:** the USDC asset in the end-to-end and harness gates
  (acquisition and command gates used the mock ERC-20); MCP exposure (deliberately none: operator command).
- **Long-lived wallets:** still bounded by the 128 MiB single-file store; segmented storage (separate lane, F1–F3)
  is not part of this change.
- **Installed/live:** native CI on the candidate and publication remain open. The live run (§6a) used published
  `erebus-init` with unpublished WS1 CLIs, so it qualifies the code path, not a release.
- **Auditor:** disclosure verification still replays from the deployment block; it has no checkpoint.
- **Errors:** the MCP layer reports every native wallet failure as `WALLET_UNAVAILABLE`, and the wallet reports
  provider throttling as `paired wallet recovery failed`; both hide the cause from operators.
