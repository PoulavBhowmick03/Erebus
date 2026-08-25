# Backup, restore, key loss, and rotation

What survives which failure, what an operator does about it, and which losses are permanent.

Phase 7 item 11. [custody-design.md](./custody-design.md) is the decision record for *where*
keys live; this is what happens when one of them is lost. Scope is the Rust SDK's own state
and keys, on Sepolia, as of 2026-08-25.

The short version, before the detail: **losing local state costs a rebuild, losing the pool
key costs the identity, and the pool key cannot be rotated.**

---

## 1. What an operator holds

| | What it is | Where | Rotatable |
|---|---|---|---|
| **Pool key** | The STRK20 identity. Derives channel keys, note locations, and nullifiers | `pool_key_file`, mode `0600` | **No.** See §5 |
| **Account key** | Signs the Starknet invoke that carries a proof | `account_key_file`, or never in this process at all — see `AccountSigner` | Yes, if the account contract supports it |
| **State directory** | Channel records: handles, both directional keys, cursors | `state_dir`, mode `0700` | n/a — rebuildable |
| **Operation journal** | What each write prepared, proved, signed, submitted, committed | `state_dir/operations` | n/a — **not** rebuildable |
| **Viewing grants** | Bearer or recipient-bound disclosure capsules | wherever the operator exported them | n/a |

The auditor key is StarkWare's, not the operator's. It is set once per pool, covers every
identity that registers, and no operator action affects it. See
[privacy-model.md](./privacy-model.md).

---

## 2. Losing the state directory

**Recoverable.** `erebus-cli rebuild_state` re-derives channel records from the pool key and
chain data by keyed discovery: outgoing channels are enumerated by an id only the pool-key
holder can compute, each recipient address is decrypted with that same key, and each channel
key is re-derived rather than stored.

What does not come back, and why:

- **The original handles.** A handle is random and local, so a rebuilt channel gets a new one.
  Anything holding an old handle string — an agent's notes, a journal entry, an MCP
  transcript — will not resolve against the rebuilt record.
- **`opened_transaction` and `last_write_block`.** Finding which transaction wrote a slot is
  event scanning, which this stack does not do. A zero `last_write_block` costs a wider
  proof-anchor wait on the next write and nothing else.
- **Channels on a token this client is not configured for.** Reported as `other_token` rather
  than silently skipped, so a partial rebuild is visible.

**Procedure.** Restore the pool key, run `doctor`, run `rebuild_state`, read the report. Treat
a non-zero `unrecoverable` or `other_token` count as a partial rebuild, not a complete one.

---

## 3. Losing the operation journal

**Not recoverable, and it is the one loss that can cost money.**

The journal is the only record that a transaction was signed. Without it, an operation that
died between signing and its receipt cannot be classified: reconciliation has no hash to look
up, so it cannot tell "never submitted" from "submitted and landed". Reissuing the request
then risks paying twice for one effect.

`rebuild_state` does not help. It reconstructs *channels*, and a channel's note cursor is read
from the chain, so a rebuilt channel writes at the right index — but an in-flight operation
leaves no chain trace until it lands.

**Procedure.** Back up `state_dir/operations` with the same discipline as the key files. If it
is already lost, do not reissue a write whose outcome is unknown; read the chain for the
channel's notes first and decide from what is actually there.

**Backup rule.** The journal and the state directory must be backed up *together*. A state
directory restored from a newer backup than its journal describes channels whose operations
the journal has never heard of, which reconciliation reports as records it cannot classify.

---

## 4. Losing the account key

**Recoverable if the account contract supports key rotation; otherwise terminal for writes.**

The pool does not know or care what the account key is. `assert_valid_signature`
(`utils.cairo:383`) delegates to the account contract at the user's address, trying custom
validation, then the transaction hash, then a SNIP-12 `CallSet` hash. Any key that account
accepts produces a valid signature.

So:

- **Argent or Braavos**, which support rotation: rotate at the wallet, point
  `account_key_file` at the new key, and continue. Channels, notes, and the pool identity are
  untouched.
- **A plain OpenZeppelin account** with no rotation: the account can no longer sign, so no
  write can be submitted. The pool identity is intact and the notes still exist, but nothing
  can move them. Recovering means moving to an account that can sign — which is a new address,
  and §5 applies.

**This is why `AccountSigner` exists.** A hardware or wallet signer means the SDK never holds
an account key to lose.

---

## 5. Losing or rotating the pool key

**Permanent. There is no rotation, and no recovery.**

Two facts, both upstream:

1. **Registration is write-once.** `set_viewing_key` writes the derived public key through
   `to_write_once_action` on `public_key[user_addr]` (`privacy.cairo:337-343`). An address's
   pool public key is bound the first time it registers and can never be changed. The SDK
   enforces the same rule locally: `verify_own_registration` refuses to act when the
   registered key and the loaded key disagree, rather than proceeding with a key the chain
   will not accept.
2. **Spending requires it.** A nullifier is derived from the owner's pool private key. Without
   it a note cannot be spent by anyone, including its owner. This is also what makes a viewing
   grant safe to hand out: it carries no pool key, so it can read and never spend.

Therefore:

- **A lost pool key means the identity is gone and its notes are unspendable forever.** Not
  frozen, not recoverable by StarkWare — unspendable.
- **"Rotating" the pool key means a new address**, because the old address's binding cannot be
  overwritten. And since a channel is permanent per address pair (F29), a new address means
  every existing channel is left behind and every counterparty must open a new one.

**Procedure.** Back up the pool key before the first registration, offline, in more than one
place. There is no second chance and no support path. StarkWare's *Enclave* project — trusted
operators storing offchain secrets such as STRK20 viewing keys — is the obvious candidate for
custody rather than building it here, and is unbuilt on our side.

---

## 6. Losing a viewing grant

**No loss of function.** A grant is derived from data the granter still holds; it can be
issued again. A wire-v3 grant is recipient-bound and carries an expiry, so an old one going
missing is not a disclosure either.

The reverse is the real risk: **a grant that leaks cannot be recalled.** Revocation cannot
un-tell a recipient what they already read, and for a bearer wire-v1 or wire-v2 grant, anyone
holding a copy can read that relationship. Wire-v3 per-deal grants narrow the blast radius to
one deal and expire; historical grants do neither.

---

## 7. What is not defined yet

Stated so nobody reads this page as more complete than it is:

- **No backup or restore tooling.** This describes what to copy and what it protects. There is
  no `erebus-cli backup`, no encryption of an exported bundle, and no restore verification.
- **No key-loss drill.** Nothing has rehearsed a pool-key loss or an account rotation against
  Sepolia; §4 and §5 are read from the contracts and the SDK, not from an exercise.
- **No custody recommendation.** Whether an operator should hold its own pool key at all is
  the open question in `custody-design.md`, and this page does not answer it.
