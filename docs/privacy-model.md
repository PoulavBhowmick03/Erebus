# Privacy model

The canonical statement of what Erebus hides and what it does not. Everything else in this
repository that describes privacy should point here rather than restate it, because a claim
maintained in five places drifts in five directions.

Scope: the wire-v2 path as implemented in `sdk/rs`, on Sepolia. Not a security review.

---

## The claim, and the non-claim

**Claim.** Negotiation contents and settlement amounts are confidential. An observer reading
public chain data cannot recover the amount, token, deadline, memo hash, message type, or
reply structure of a negotiation, and cannot read the amount or recipient of the settlement.

**Non-claim.** Erebus does not hide that a negotiation happened. It must not be described as
private in an absolute sense. An observer can identify Erebus pool interactions, count them,
time them, and attribute each to its submitting Starknet account.

Stated as one sentence, so it is quotable and hard to soften: **Erebus hides the terms, not
the relationship.**

---

## What leaks at each step

The seven steps are the ones in [runbook.md](./runbook.md) and the workflow walkthrough.

| Step | Hidden | Public |
|---|---|---|
| 0 · fund | nothing | depositor account, amount, token, timing — the whole ERC-20 leg |
| 1 · open channel | counterparty identity | submitting account, that a channel marker was written, timing |
| 2–4 · offer, counter, final offer | amount, token, deadline, memo hash, message type, `replyTo` | submitting account, five salt values per message, note count, timing |
| 5 · accept and settle | amount paid, recipient, change amount | submitting account, that a settlement occurred, note count (6 or 7) |
| 6 · grant | everything — local only, no transaction | nothing |
| 7 · reveal | everything — local only, no transaction | nothing |

Steps 6 and 7 produce no chain activity at all. Disclosure is a local read against data that
is already on chain, which is why a grant costs no gas and leaves no trace.

---

## What leaks by category

| Private from a public chain reader | Public, or visible to infrastructure |
|---|---|
| Wire-v2 offer content | The submitting Starknet account |
| Settlement amount and recipient | Pool interaction timing and frequency |
| Spent-note identity | Public shield and unshield amounts |
| Channel content without a grant | The fixed fifth-salt shape in wire v2 |
| Relationships outside a scoped grant | Note count per settlement, and so one bit about payer holdings |
| Change amount and change-note content | Proof-bearing transaction size |

---

## The four known leaks, in descending severity

### 1. The fifth-salt fingerprint

Wire v2 fills 536 of 595 payload bits: an 8-bit version marker, 400 bits of message, a
128-bit authentication tag. The remaining 59 bits are zero-filled. So the fifth salt of every
message has bit 119 pinned and bits 60–118 clear, whatever the message says.

A uniformly random 120-bit salt has that shape with probability 2⁻⁶⁰, or 2⁻⁵⁹ once you
condition on the pinned format flag that roughly half of all salts carry anyway. Either way
the fifth salt identifies an Erebus message essentially every time.

Verified in `sdk/rs/tests/wire_v2_fingerprint.rs`. Tracked as friction F31.

**Fix**: fill the spare bits with random padding rather than zeros. The salt then sits inside
the set of ordinary random salts and is distinguishable only by bit 119. Not done.

### 2. Submission linkability

Every write is an `apply_actions` transaction signed by a public Starknet account. The
account that opens a channel, the account that writes each offer, and the account that
settles are all visible and all the same identity across one deal.

Combined with leak 1, an observer who cannot read a single term can still build the
interaction graph: which accounts negotiate with which, how often, and when.

**Fix**: unlinkable submission — relayers, or account rotation per deal. Not designed.

### 3. The public funding leg

Shielding is a real ERC-20 transfer. Depositor, amount, token, and timing are public, and
they precede the first private action by a bounded interval.

**Fix**: none within this design. Funding correlation is an ecosystem-level problem.

### 4. Note count on settlement

A settlement creates six notes when the payer's selected inputs match the price exactly and
seven when they overshoot and a change note is minted. That leaks one bit about the payer's
holdings on every deal. Amounts stay private.

Introduced by the change-note work on `change_output_payback`. Record against F31 rather than
as a separate finding — it compounds the same weakness, letting a reader classify and count
Erebus transactions without reading one.

**Fix**: always mint a change note, zero-valued when unneeded, so the count is constant. Not
done.

---

## What the observer harness establishes

`scripts/observer.py` runs a no-key recovery attack against public calldata.

- **Positive control, wire v1**: reconstructs the full acceptance from four public salt
  halves — amount, memo, timestamp, all of it.
- **Wire v2**: finds no plausible transcript. No message type, reply target, timestamp,
  amount, deadline, or memo hash.
- **Both**: detects the fifth-salt shape without decrypting anything and classifies the
  transaction as likely Erebus traffic.

Same script, same access, one leaks and one does not. That is the strongest evidence in the
repository for the content claim.

Its boundaries: it tests the supplied fixtures, it relies on AES-256-GCM-SIV being sound and
the channel key staying secret, and it is not an external cryptographic review. It also
currently mislabels wire-v1 traffic as wire v2 — a defect in the version classifier, not in
the recovery result. See [privacy-observer-finding.md](./privacy-observer-finding.md).

---

## Infrastructure that sees more than the chain does

The chain is not the only observer, and two endpoints see materially more.

| Endpoint | How it sees | Sees the pool key |
|---|---|---|
| The prover | `starknet_proveTransaction` → `PROVING_SERVICE_URL` (`prover.rs:186`) | **Yes** |
| The write RPC | `starknet_call` preflight → `STARKNET_RPC_URL` (`execution.rs:147`, `rpc.rs:65`) | **Yes** |
| The submitted transaction | `apply_actions` (`calldata.rs:73`) | No |

`compile_actions` embeds the pool private key at calldata element 1 (`calldata.rs:34`),
because the pool is an account contract simulated locally. Both endpoints above receive that
calldata and can therefore derive the identity's full history. The exposure aggregates across
users on a shared prover, and it is permanent — there is no rotation or revocation.

The submitted transaction does not carry the key. That part of the design holds.

Separately, registration writes your pool private key encrypted to a single pool-wide auditor
key (`channel.cairo:329-334`, `channel.rs:123-129`). It is set once, covers everything that
identity ever does, and is not something you grant — it happens the moment you register.

---

## Trust boundaries in our own stack

The account signing key stays in the Rust process; Python passes only a file path. Python
does handle bearer viewing grants, so MCP transcripts sit inside the disclosure trust
boundary.

A viewing grant is a **bearer** secret. Possession is what permits reading; the `grantee`
field is metadata at the outer API and binds nothing. Its checksum detects edited or
incompatible grant data but is not a signature and does not authenticate who issued it.

A grant carries both directional channel keys for one pair on one token, and no pool private
key. So it reads exactly one relationship and cannot spend: nullifiers need the owner's pool
private key (`compute_nullifier`), which no grant contains.

---

## What a disclosed record proves, and what it only asserts

**Proves.** That these messages were written to these channels, that an acceptance exists,
and what the payment note actually carries. `agreed_amount` and `paid_amount` are kept as
separate fields precisely so a reader can compare what the acceptance said against what was
paid.

**Asserts.** Everything about business meaning. `memo_hash` commits to off-chain detail whose
preimage and semantics live outside this wire. Atomicity is narrower than semantic proof: the
acceptance and payment share one action set, and the amount-equality check is Rust-side
validation, not a statement that the STRK20 circuit understands the negotiation. There is no
separate ZK receipt proving the business meaning to an external verifier.

---

## Related

- [friction.md](./friction.md) F30, F31 — how the salt lane turned out to be public, and what
  wire v2 fixed and did not
- [privacy-observer-finding.md](./privacy-observer-finding.md) — the harness result in full
- [roadmap.md](./roadmap.md) §4 — the same boundary, in planning terms
- [runbook.md](./runbook.md) — the seven steps, reproducible
