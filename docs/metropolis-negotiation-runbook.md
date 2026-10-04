# Metropolis Negotiation SDK

Scope: local Rust developer components on `metropolis`, not a published agent release.
Owner decisions: [DM8-6 and DM8-7](metropolis-m8-decisions.md).

## Code entry points

| File | Responsibility |
|---|---|
| `sdk/transport/src/socket.rs` | Bounded TCP framing and descriptor-bound Noise sessions |
| `sdk/transport/src/negotiation.rs` | Canonical proposals, alternating counters, acceptance, frozen final terms |
| `sdk/transport/src/store.rs` | Persist-before-ack transcript and atomic freeze record |
| `sdk/transport/src/binding.rs` | Descriptor-signed attestation of a shielded agreement key |
| `sdk/transport/src/negotiation/peer.rs` | Persist/send/receive helpers and separate final authorization callbacks |
| `sdk/transport/src/negotiation/bootstrap.rs` | Authenticated identity exchange and first offers without a shared private draft |
| `sdk/coordinator/src/lib.rs` | Buyer policy reservation, signing fence, and durable authorizations |
| `sdk/shielded/src/bin/erebus_negotiate.rs` | Native deterministic price negotiation and durable consent for both settlement modes |

`NegotiationPeer::public_bound` requires agreement keys equal to the authenticated descriptor addresses.
`NegotiationPeer::shielded` exchanges private identity bindings before typed negotiation.
Both constructors verify capabilities against the operator-selected draft.
These synchronous APIs require a blocking worker when called from an async application.

## Cold-start SDK

`NegotiationBootstrap` requires signed descriptors and the operator-selected settlement context, not a private draft on both machines.
For suite 2, it authenticates the remote agreement key through the private signed binding.
The seller also sends its payment tag through Noise. It never sends its spending secret.
The final seller signature authorizes that payment tag; the identity binding alone does not.

The buyer uses `create_proposal` to insert authenticated identities and generate fresh local randomness.
This replaces the deal ID, settlement nonce, blinding, revision, transcript root, and access recipient.
It preserves the selected deployment, asset, guarantees, service promise, fees, and deadline.
The buyer calls `offer` to persist the draft before delivery.
The seller calls `receive_offer` with its own service-policy check before storage or acknowledgement.
It receives the canonical terms and blinding only through Noise.

After a fresh handshake, `NegotiationPeer::synchronize` re-delivers retained events in role order.
Both participants must recover the same root before further negotiation or authorization.
Invalid counts, changed events, and unequal roots close the connection without discarding durable state.
Synchronization control messages never enter the negotiation root.
Session limits still apply; this operation does not bypass re-handshake requirements.

## Native command

Build and check the local command from the repository root:

```sh
cargo build --manifest-path sdk/shielded/Cargo.toml --locked --bin erebus-negotiate
printf '%s' '{"method":"version"}' | sdk/shielded/target/debug/erebus-negotiate
cargo test --manifest-path sdk/shielded/Cargo.toml --locked --test negotiation_cli
```

The version response states `settlement_submission: false`.
This command negotiates and authorizes an agreement. It does not prove, fund, sign a transaction, or submit payment.
It has no RPC configuration or submission path.
The complete settlement command and combined MCP workflow remain open M8 work.

Each participant has a separate owner-only JSON configuration and its own keys.
All key files contain exactly 32 raw bytes. The terms template contains canonical `AgreementTerms` bytes.
Signed descriptor files contain public JSON. The configured TCP endpoint must occur in the signed seller descriptor.
This local profile uses literal socket addresses, not automatic hostname discovery or a hosted relay.

Example operator configuration, with paths replaced by that operator's existing local files:

```json
{
  "version": 1,
  "role": "buyer",
  "state_root": "/operator/erebus/state",
  "transport_key_file": "/operator/erebus/transport.key",
  "agreement_key_file": "/operator/erebus/agreement.key",
  "discovery_key_file": "/operator/erebus/discovery.key",
  "local_descriptor_file": "/operator/erebus/buyer.json",
  "peer_descriptor_file": "/operator/erebus/seller.json",
  "terms_template_file": "/operator/erebus/service.terms",
  "endpoint": "127.0.0.1:9020",
  "maximum_price": "75",
  "minimum_price": "0",
  "max_deal_lifetime_seconds": 3600,
  "timeout_seconds": 30,
  "seller_spend_secret_file": null,
  "seller_wallet_file": null,
  "seller_wallet_key_file": null
}
```

For suite 1, the agreement key also signs the discovery descriptor.
For suite 2, the separate discovery key signs the identity binding.
A shielded seller must also configure its spending secret, encrypted wallet, and wallet encryption key.
Before sending seller consent, the command persists the expected payment note in that wallet.
An expected note is not an included or spendable payment.

Start one seller process, then one buyer process, with their separate configuration paths:

```json
{"method":"negotiate","config_file":"/operator/erebus/config.json","operation_ref":"1111111111111111111111111111111111111111111111111111111111111111"}
```

Use a new nonzero operation reference for each new deal. Reuse the same reference after interruption.
The buyer starts at its template price and accepts no counter above `maximum_price`.
The seller accepts an offer at or above `minimum_price`; otherwise it counters with its template price.
This command supports that deterministic single-counter policy, not general-purpose bargaining strategies.
The buyer generates the payment expiry from its configured lifetime; the seller checks its own lifetime bound.
The remaining service fields and fee policy must match the seller's template.

`freeze_only: true` stops after bilateral acceptance, before any signature or spending reservation.
Repeat without that flag to reconnect, synchronize, and authorize the retained agreement.
After uncertain delivery, repeat the same request on both participants. Never generate a new deal to hide an error.
The command serializes calls for one operator state directory. It is not a concurrent marketplace daemon.

An authorized response reports the commitment and a private `evidence_file` path.
That journal blob contains `SelectedAgreement` bytes, despite the storage engine's `.tx` filename suffix.
It can feed the existing disclosure and access components.
The buyer also retains coordinator intent, consent, and one policy reservation for later settlement.
Both `payment_verified` and `delivery_verified` remain `false`.

The copied-command test runs without checkout files or environment variables in the child processes.
It exercises both modes, direct acceptance, a counteroffer, frozen restart, repeated authorization, and private seller-note retention.
This is local component evidence, not a published package or live Monad payment.

## Negotiation boundary

The caller selects an initial canonical draft and fresh random blinding per deal.
The draft has a zero transcript root and revision `1`.
It fixes the deployment, asset, identities, service, expiry, fee policy, guarantees, and settlement nonce.
This profile negotiates only price; a different service or deployment requires a new deal.

The buyer sends the initial offer.
Participants alternate counters, each bound to the previous proposal digest.
The nonproposer accepts the latest proposal; the proposer confirms it.
The first acceptance prevents further counters.
The second seals the transcript root before final signatures.

For buyer authorization, call `Coordinator::record_intent` and `Coordinator::authorize_buyer` before encrypted signature delivery.
For the received seller authorization, use `Coordinator::accept_seller` as the persistence callback.
The seller must persist both final authorizations in private durable state before acknowledgement.
The callback boundary does not implement seller storage automatically.
Neither a frozen root nor stored signatures prove payment.

## Local verification

From the repository root, run the transport tests:

```sh
cargo test --manifest-path sdk/transport/Cargo.toml --locked --all-targets
```

The tests cover both suites, exact-body retries, changed context, expiry, corrupt freeze records,
concurrent append/freeze, key substitution, signature rejection, and failed persistence callbacks.
They also verify that existing M1 commitment and M2 root functions reproduce the new agreement.

Run the independent-process integration:

```sh
cargo test --manifest-path sdk/evm/Cargo.toml --locked --test negotiation_flow
```

One parent test starts separate buyer and seller workers twice, then an independent auditor.
The worker entry point is ignored by default and invoked by that parent test.
Expected: the parent test passes; both peers retain one root across restart and authorize one commitment.
The buyer retains one reservation, and the auditor reports a verified agreement.
No RPC provider, prover, or chain funding is required.
The test does not settle a payment or prove resource delivery.

## Recovery and privacy limits

After uncertain delivery, close the connection and establish a fresh Noise handshake.
Use `NegotiationPeer::synchronize` to reconcile retained events before resuming.
Use `NegotiationPeer::redeliver` for an explicit already-persisted local event.
Re-delivery changes only the session envelope, not the transcript-relevant body.
It does not generate new signatures, cancel earlier authorizations, or release policy capacity.

If a freeze record is missing after complete acceptance, replay repairs it.
If a freeze record is corrupt, stop; do not delete it to force progress.
Restore trusted private state and inspect the retained log before resuming.
Final agreement signatures and chain observation remain separate recovery steps.

Transcript stores contain plaintext commercial terms and the blinding; keep them private.
Key bindings and final authorizations travel encrypted but do not hide endpoints or traffic timing.
No per-message public signature proves individual authorship to an auditor.
Both final signatures authorize the disclosed transcript root, not the truth of business statements.

Remaining: combined installed CLI/Python/MCP workflow, funded live private negotiation,
package distribution, hosted/self-hosted rehearsal, x402, and stage measurements.
# Seller Access Handoff

For a seller serving paid snapshots, set `access_evidence_root` in its private
negotiation configuration to the access service's absolute `evidence_root` directory.
Create that directory with mode `0700` before starting either process.
The native seller verifies the frozen transcript and both authorizations before
publishing `<deal_commitment>.evidence` with mode `0600`.
Publication syncs the file and directory, permits identical retries, and rejects
conflicting existing evidence. No watcher or Python cryptography is required.
The access service must still validate its configured seller, service, and payment.
Buyers cannot configure this publication path. A handoff error is not permission
to delete state or authorize a new payment; retry the same negotiation operation.
