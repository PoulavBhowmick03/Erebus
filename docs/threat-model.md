# Threat model

Who can observe Erebus, what each one sees, what we claim against that, and how the claim
gets measured rather than asserted.

Scope: the live wire-v2 path on Sepolia and the source-default wire-v3 path verified offline,
as of 2026-08-21. This is a threat model, not a security review.
[privacy-model.md](./privacy-model.md) stays canonical for what leaks;
this document adds the observers, the anonymity sets, and the metric.

**Why this exists before the wire changes.** Phase 8 reframes the wire — framed entries, deal
identifiers, randomized spare bits — and every one of those is a decision about *whom* we are
hiding from. Padding hides message shape from a chain reader and does nothing about the
prover. Deciding the wire without naming the observer first produces defences aimed at
nobody in particular.

---

## 1. Observers

Each row is an entity that sees something. The distinction that matters most is between what
reaches the *chain* and what reaches an *endpoint we chose*, because the second is invisible
to anyone auditing chain data.

| Observer | What they get | What they cannot get |
|---|---|---|
| **Chain reader** | Both addresses at channel-open, in the clear (F38). The submitting account on every write. Seven created notes per wire-v3 settlement. Timing and frequency. Historical wire-v2 records retain the fifth-salt shape (F31). | Message content, amounts, token, deadline, memo hash, reply structure, which notes were spent. |
| **Prover** | The pool private key, at calldata element 1 of `compile_actions` (`calldata.rs:32`). From it, that identity's entire channel structure and note amounts, past and future. | The account signing key. It cannot forge a different action set: the invoke is signed over exactly this calldata. |
| **Write RPC** | Identical to the prover. The `starknet_call(compile_actions)` preflight sends the same calldata (`execution.rs:156`, `rpc.rs:66`). | Same as the prover. Also sees the operator's IP and request timing. |
| **Counterparty** | Everything in the shared channel: every offer, every counter, the settlement, both directional keys. | The other side's other channels, other counterparties, note holdings outside this relationship, pool private key. |
| **Legacy v2 grant holder** | One relationship on one token, in full, permanently. Both directional channel keys. Wire v3 rejects this grant export. | The ability to spend — `compute_nullifier` needs the owner's pool private key, which no grant carries. Any other relationship. |
| **Pool auditor** | The pool private key of every registered identity, encrypted to a single pool-wide key at registration (`utils.cairo:220`, `privacy.cairo:329-334`). Set once, no rotation, granted automatically at registration rather than by choice. | Nothing, within this pool. This is the widest standing view in the system and it is not ours. |
| **Screener** | That a deposit was requested, by whom, for how much. Deposits only; note-to-note transfers are unaffected. | Channel contents. |
| **Paymaster / relayer** *(if adopted)* | The request, its timing, and the operator's IP. | The pool key. It is not in the submitted transaction, only in the preflight and proving calldata. |

**The single worst row is the prover, and no wire change touches it.** Erebus sends the pool
key to two operator-chosen endpoints because the pool is an account contract simulated
locally. The exposure aggregates across users on a shared prover and there is no rotation: the pool
key is the identity, and registration writes it once through `WriteOnce`
(`actions.cairo`, `SetViewingKey`), so it cannot be replaced. Self-hosting removes the assumption
entirely; OHTTP splits it (the relay learns who asks, the gateway learns what is asked).
Nothing else does.

This is where Erebus is structurally behind Stellar Private Payments, which proves in-process
with no mandatory remote prover — so SPP's write RPC sees a finished proof and never the
witness. Our prover and our write RPC see the same thing, and that thing is the key.

---

## 2. Anonymity sets

There are three, they behave differently, and conflating them is how the earlier
relationship-privacy claim survived as long as it did.

### 2.1 Content — not an anonymity set

Message confidentiality is encryption, not anonymity. AES-256-GCM-SIV under an HKDF-derived
directional key. Either the key holds or it does not; there is no set to be small. This is
the one claim currently supported by evidence (`scripts/observer.py`).

### 2.2 Value — the pool's note set

Which notes were spent hides among all notes in the pool, on the same argument SPP makes for
its Merkle tree: the proof shows the spent note exists somewhere without naming the leaf.

The **effective** set is smaller than the nominal one, and for the same reasons SPP lists:
public deposits, public withdrawals, timing, and counterparty knowledge each eliminate
candidates. STRK20 shipped June 2026, so the nominal set is small to begin with.

**Neither project currently measures its effective set.** Section 4 proposes how.

### 2.3 Relationship — the anonymity set is one

This is the finding that should stay unsoftened.

`recipient_channels` is `Map<ContractAddress, Vec<EncChannelInfo>>` (`privacy.cairo:88`), so
the recipient's plain address is a storage key, and the submitting account signs. Both
endpoints of the edge are written in the clear, twice per pair since channels are
directional.

There is nobody to hide among. The relationship anonymity set is **1**, not "small". Padding,
fixed shapes, and cover traffic all operate on a set that does not exist for this property.

The bootstrap makes it structural rather than an oversight: `EncChannelInfo` carries the
channel key the recipient does not yet have, so its location must be derivable from something
the recipient knows without any shared secret. The protocol encrypts the *outgoing* side
(`enc_recipient_addr`, `utils.cairo:150-164`), which shows the asymmetry is deliberate — only
the incoming side has the chicken-and-egg problem.

#### Sub-accounts do not fix this. Checked 2026-08-21.

Upstream's sub-account work looked like the answer and is not. Recorded here because a closed
question is worth as much as an open one.

The primitive is real and has exactly the right shape. `compute_identity_key` returns
`h(IDENTITY_KEY_TAG, user_addr, user_private_key, contract_address)` and is documented as "a
pseudonymous proof of ownership the target can derive sub-accounts from **without learning who
the user is**" (`hashes.cairo:54-60`). A sub-account address derives deterministically from
`h(h(identity_key, dapp_name), nonce)`, so it is unlinkable to its owner without that owner's
private key.

It cannot serve as a channel recipient, for three reasons that compound:

1. **A recipient must be a registered pool identity.** Registration is a `SetViewingKey`
   client action, and every action set is gated by `assert_valid_signature(user_addr, ...)`
   (`privacy.cairo:207`, `utils.cairo:383-408`). All three accepted routes — custom
   validation, standard `is_valid_signature`, legacy SNIP-12 — require the address itself to
   validate a signature.
2. **A sub-account does not sign for its owner.** It is driven only by the anonymizer, which
   asserts `get_caller_address() == privacy_contract` before calling `sub_account.execute`
   (`sub_account_anonymizer.cairo:304-313`). The user cannot make it act directly.
3. **A sub-account holds no pool keypair.** Recovering a channel key means decrypting
   `EncChannelInfo.enc_channel_key` by ephemeral-static ECDH against the recipient's pool
   private key. A sub-account's identity is a commitment, not a keypair, so there is no
   private key to decrypt with.

The model is also aimed elsewhere: a sub-account is a transient vehicle that executes dapp
calls and has its balance collected straight back into the user's open notes. It is not a
persistent identity that receives and later spends notes.

**What this leaves.** The pool already contains a working pseudonymous-identity primitive; it
is simply not wired to the channel bootstrap. That converts a vague ask — "make channels
private" — into a specific one worth putting to StarkWare: *let a channel be addressed to an
identity commitment rather than a `ContractAddress`, with the recipient's pool key bound to
the commitment.* Whether that is even constructible given the bootstrap problem in §2.3 is
their question to answer, not ours to guess.

Compare SPP: its public leak is the withdrawal recipient in `ExtData`, at the *exit* of the
pool. Ours is at the *formation of the relationship*, which is the thing Erebus exists to
protect. Different severity for the same category of leak.

---

## 3. Auxiliary data

What an observer plausibly holds beyond chain data. A model that omits this measures the
wrong adversary.

- **Public funding legs.** Shielding is a real ERC-20 transfer: depositor, amount, token,
  timing, and it precedes the first private action by a bounded interval.
- **Agent identity from outside the chain.** Erebus agents are reachable over MCP or A2A. An
  observer who knows an agent's Starknet address from a directory, an invoice, or an A2A
  card links every one of that agent's channel-opens without touching the wire.
- **Timing correlation with off-chain events.** A deal that follows an API call by seconds is
  attributable to that call.
- **Repeat structure.** Once D3 lands and a pair deals repeatedly, cadence over the same
  public edge becomes a business signal even with every term encrypted.
- **The counterparty themselves.** They know the edge already, and half the transcript.

---

## 4. Success metric

Phase 13 requires a measurable target. Since the relationship edge is public, a
"link the counterparty" metric is trivially perfect and therefore useless. The measurable
questions are the ones padding, constant shapes, and relaying can actually move.

All four extend `scripts/observer.py` and run against a mixed corpus of Erebus transactions
and unrelated STRK20 pool traffic.

| # | Question | Metric | Baseline now | Target |
|---|---|---|---|---|
| M1 | Can an observer tell an Erebus transaction from other pool traffic? | Precision, recall, balanced accuracy | **1.0000, measured 2026-08-21** (`scripts/linkage.py`, 2 fixtures against 10,000 synthetic negatives, zero false positives) | 0.5, indistinguishable from random salts except bit 119 |
| M2 | Can an observer read the exact-vs-change bit from a settlement? | Accuracy on a binary guess | **0.5008, measured offline 2026-08-22** — wire v3 always creates seven notes | 0.5 |
| M3 | Given a corpus, how accurately can an observer count deals per account? | Error in deals-per-account over a window | Exact, given M1. Not separately measured | Bounded by M1; no separate mechanism |
| M4 | After relaying, can an observer link a submission to the pool identity acting? | Precision of submitter→identity linkage | 1.0 by construction — the same account signs every write. Not measured; there is no relayer to measure against | ≈0, since nothing in the contract binds submitter to identity outside `collect_fee` |

**Balanced accuracy, not AUC.** The M1 classifier is a deterministic predicate with no
score to threshold, so it has exactly one operating point, and an "AUC" over a single point is
that point's balanced accuracy under another name. Reported honestly as what it is.

**The baseline must be timing-only.** Measure what an observer achieves from timing and
ordering alone, then add the fingerprint, then the note count, then public funding. Otherwise
an improvement in one channel gets credit for a leak that was never closed. **Not yet
measured** — it needs a corpus of real pool traffic, which `linkage.py` deliberately does not
fetch.

**Limits of the current measurement**, stated so the numbers are not read as more than they
are. The positive set is two committed fixtures, so M1's recall rests on n=2. The negatives
are uniformly random salts, which is the correct null model for F31's own claim but is not a
sample of live STRK20 traffic. M3 and M4 are argued, not measured.

`scripts/tests/test_linkage.py` pins both directions: the current wire scores 1.0, and
synthetic padded salts and a constant note count score at chance. A metric that could only
ever report 1.0 would record the leak and the fix identically, so the second half of each
pair is what makes the first half worth recording.

**M1 and M2 are what the Phase 8 wire work is for.** M4 is a relayer, not a wire change. No
metric here addresses section 2.3, because nothing at this layer can.

---

## 5. What the wire can and cannot fix

Decided here so Phase 8 has a scope rather than a wish list.

**In reach of the wire:**

- Randomize the 59 spare bits instead of zero-filling them (M1). Note that the decoder
  currently *validates* those bits as zero, so this is reader-breaking: every party needs the
  new code, and it is a wire-version change rather than a patch.
- Constant note count via an always-minted, zero-valued-when-unneeded change note (M2).
- Framed entries and deal identifiers, so repeat deals do not need fresh channels (D3).

**Not in reach of the wire, and must not be implied to be:**

- The counterparty address (§2.3). Upstream, structural.
- The prover and write-RPC key exposure (§1). Self-hosting or OHTTP.
- The public funding leg. Ecosystem-level.
- Submission linkability. A relayer, which the pool already permits.
- The pool auditor's standing view. Not ours to change.

---

## 6. Where SPP is ahead, and what to take

Read against `stellar-private-payments` on 2026-08-21.

| | SPP | Erebus |
|---|---|---|
| Prover trust | In-process, no mandatory remote prover | Remote prover **and** write RPC both see the pool key |
| Transaction shape | Fixed 2-in/2-out circuit, so note count leaks nothing | 6 or 7 notes, leaking one bit per deal |
| Disclosure scope | One receipt, specific notes, bound to pool, authority, purpose, identity payload and nonce via `extContextHash` | Bearer blob, whole channel pair, permanent, no binding |
| Disclosure verification | Offline Groth16 check against a published verifying-key hash | Requires our software and chain reads |
| Public edge | Withdrawal recipient at pool exit | Both addresses at relationship formation |

Three things worth taking directly:

1. **Fixed shape as a design rule, not a fix.** SPP got constant note count by construction.
   Our M2 fix is the same idea applied late.
2. **Context binding on disclosure.** `extContextHash` over network, pool, authority,
   purpose and nonce is exactly the binding Phase 11 needs, and it demonstrates the design is
   buildable rather than theoretical.
3. **The receipt answers "what makes it true" with a proof, not a signature.** SPP's
   disclosure receipt is a Groth16 proof verifiable offline by anyone holding the file and
   the verifying-key hash. That is the honest form of the platform receipt in Phase 12, and
   it is the option that needs a circuit.

One thing SPP does *not* solve and we should not claim from the comparison: it also has no
stated or measured effective anonymity set, and its disclosure receipt is persistently
linkable — the holder can watch for the disclosed nullifiers later. Scoped disclosure narrows
who learns what; it does not make the disclosure forgettable.

---

## 7. The claim this model supports

Unchanged from `privacy-model.md`, and now with observers attached:

> Erebus hides the terms, not the relationship.

Precisely: negotiation content and settlement amounts are confidential against a chain
reader. They are **not** confidential against the prover or the write RPC, which see the pool
key. The relationship is not hidden from anyone.

**Rule for this document.** If M1 through M4 are measured and miss their targets, the claim
narrows to match the measurement. The measurement is not permitted to be reinterpreted to
match the claim.

---

## 8. Open

- What is the effective, not nominal, size of the STRK20 note set on Sepolia?
- Does relaying via a paymaster carry ordinary settlements, or only AVNU swaps? (Phase 10.3.)
- What does an agent-directory observer achieve? Section 3 asserts the risk; nobody has
  measured it.
