# Metropolis M8 Decisions

Updated: 2026-10-03. Branch: `metropolis`. M8 remains open.

## DM8-1. Buyer identity for access

The owner selected the existing agreement key for the first release.
The service requires `access_recipient == buyer_authorization_key`.
Separate access keys and delegated access are not supported by this profile.

An access request authorizes retrieval, not payment.
Its digest binds the deployment domain, service identity, deal commitment,
deal nullifier, resource digest, request nonce, and request expiry.
Suite 1 signs the Keccak digest.
Suite 2 signs Poseidon over tag `3002` and both 128-bit digest limbs.
Disclosure uses tag `3001`; payment authorization uses separate role tags.

Requests expire within five minutes. Remote access requires TLS.
A stolen request can be replayed during its validity period.
Retrieval of an immutable snapshot is idempotent, not a consumable usage allowance.
This profile does not select a prepaid or batched x402 billing model.

## DM8-2. Payment and delivery remain separate

The service verifies the agreed resource, seller, buyer, quantity, and payload digest.
It requires finalized payment evidence before issuance.
Public-bound settlement uses the durable EVM observer.
Shielded settlement uses paired finalized pool observation.
Distinct RPC URLs do not prove independent providers.

The service persists issuance before it returns the payload.
After a lost response, the same buyer can retrieve the same issuance.
The service has no transaction signing or payment submission path.
Pending observations do not issue access and do not prove nonpayment.

The delivery deadline does not cancel a paid entitlement.
Late issuance is reported explicitly.
The service response reports `delivery_verified: false`.
A response from the seller is not an independent audit of delivery.

Recovery requires the seller to retain the agreed resource and private agreement evidence.
The seller must restore issuance storage and continue serving the endpoint.
The service cannot force an uncooperative seller to deliver or refund.

## DM8-3. Shared artifacts, local witnesses

The installer authenticates the manifest with an independently trusted SHA-256 digest.
It verifies artifact lengths and hashes before publication in the local cache.
It locks concurrent installations, uses atomic writes, and rejects symlinks.
It repairs a corrupt regular cache entry by downloading it again.

The local prover requires public signals that match the manifest deployment domain.
The witness stays in an owner-only local file.
Only public artifacts are downloaded; the witness is not uploaded.
The prover verifies the generated proof locally before it returns public calldata.

Manifest authentication does not establish ceremony security or authenticate live verifier code.
Those remain separate release checks.
Known-entropy prototype keys require explicit development opt-in.
They must not become a secure testnet or mainnet release.

## DM8-4. Bounded disclosure observation

The public-bound disclosure CLI now persists observation progress across processes.
It accepts an explicit deployment start block and bounded log and ancestry budgets.
Exit code `2` means pending observation, not verified payment.
Exit code `0` requires matching finalized payment evidence.
Invalid grants fail before RPC access.

## DM8-8. Native payment uses durable consent and observes after uncertainty

The public-bound driver continues the native negotiation operation.
The operator fixes deployment pins, buyer, asset, gas account, fee caps, and a shared signer journal.
The agent request cannot override them.
The driver verifies the retained negotiation before the first transaction signature.
The existing coordinator persists the signing plan and signed bytes before broadcast.

Any recorded broadcast attempt disables automatic submission in this driver.
Later calls observe with two distinct RPC endpoints and separate durable checkpoints.
Only matching finalized evidence changes payment accounting.
Recovery works without the transaction key or negotiation transcript after submission.
This conservative default can stall a transaction fenced before network I/O.
Controlled rebroadcast and replacement remain M6 APIs, not native driver commands.

The runtime pin must come from an independently verified immutable deployment.
The first block must have that code, and the preceding block must have no code.
This prevents a late history start from silently omitting payments.
Runtime and paired RPC checks are not a contract audit or proof of provider honesty.

This driver currently supports public-bound settlement only.
It rejects shielded agreements without a fallback.
Public-bound calldata exposes the accepted canonical terms, blinding, and final authorizations.
Only the offchain negotiation history remains confidential in that mode.
The native shielded lifecycle remains required for M8 completion.
See the [payment runbook](metropolis-payment-runbook.md).

## Open release decisions

- Secure shielded artifacts, ceremony evidence, and authenticated verifier deployment.
- Versioned platform packages and public artifact distribution.
- Hosted-service resources, quotas, retention, and incident ownership.
- x402 scheme and measured per-request, prepaid, and batched comparison.
- Mainnet activation. The planning default remains a testnet developer release.

## DM8-5. Buyer content verification

The buyer access command reads its existing agreement key from a private local file.
It signs a retrieval request, not payment authorization or a transaction.
It sends only that short-lived request to the configured service.
The agreement opening, key, and resource contents are not model-visible tool arguments.

The buyer verifies the exact payload SHA-256 against the signed agreement.
It persists verified content before it reports a local resource path.
After interruption, it either restores a complete verified cache entry or retries retrieval.
The seller and buyer caches both use the existing durable journal implementation.

A completed local cache entry requires neither a signing key nor a network provider for later reading.
Filesystem ownership controls that local access.
No consumable usage allowance is decremented by this snapshot profile.

The receipt reports `resource_verified: true` and `seller_reported_payment_finalized: true`.
It keeps `payment_verified: false` and `delivery_verified: false`.
Content verification alone does not prove the seller's chain claim or an independent delivery audit.
Use the disclosure observer for independent payment verification.

The buyer CLI and MCP preserve `paid_but_undelivered` as a seller-reported outcome.
They report `seller_reported_payment_finalized: true` but keep `payment_verified: false`.
The CLI returns exit code `2`; recovery retries retrieval without another payment.

## DM8-6. Freeze negotiation before final authorizations

The owner selected one frozen negotiation transcript, not a separate authorization transcript.
Final signatures cannot enter the root they sign.
The M1 agreement encoding and M2 transcript hashes remain unchanged.

Negotiation profile 1 carries canonical draft terms with a zero transcript root and the private blinding.
The first proposal fixes the deployment, asset, keys, service, expiry, fees, guarantees, and settlement nonce.
Counters change only price and revision, alternate authors, and reference the previous proposal digest.
This is a price-only service profile, not arbitrary contract negotiation.

Both participants append acceptance of the same draft digest.
The first acceptance stops further proposals; the second completes the negotiation.
The store seals the resulting nonzero root under its existing append lock.
The final agreement replaces the draft's zero root with that sealed root.
Only then do participants authorize its commitment.

Final authorizations remain encrypted control messages, outside the negotiation log.
The peer verifies each signature and invokes a separate durable persistence callback before acknowledgement.
The buyer's coordinator reserves capacity and persists its signing fence before local signing.
Acceptance alone neither reserves funds nor proves payment.

Recovery uses a fresh Noise session and the retained transcript.
Exact-body re-delivery is idempotent in this typed layer; changed messages at an existing sequence fail.
A complete acceptance log repairs a missing freeze record after interruption.
A corrupt freeze record, changed prefix, or mismatched proposal fails closed.

## DM8-7. Bind discovery identity to shielded agreement identity

The owner selected a signed identity binding, not an implicit mapping or configuration-only identity pin.
The existing descriptor remains signed with its suite-1 Ethereum identity.
That signer attests the current descriptor digest, exact deployment domain, suite-2 key, and validity window.
The digest uses `EREBUS_AGREEMENT_KEY_BINDING_V1` and canonical encoding.

Peers exchange bindings inside the authenticated Noise channel before shielded negotiation.
The mapping must match the draft's buyer and seller agreement keys.
Bindings cannot outlive their descriptor, and descriptor rotation requires a new binding.
No shielded key is added to the public discovery document.
Neither bindings nor final authorizations enter the frozen negotiation root.

An identity binding is not payment consent or proof of possession of the shielded spending secret.
Both final agreement signatures and the settlement verifier remain mandatory.
Network endpoints, traffic timing, and public discovery identities remain observable.

The negotiation SDK and recovery tests are local evidence.
They do not complete package distribution, the live private workflow, or x402 integration.
See the [negotiation runbook](metropolis-negotiation-runbook.md).

### Cold-start and native integration

The cold-start API authenticates shielded keys before the buyer constructs its draft.
The seller receives the first offer through Noise and checks its own service policy before persistence or acknowledgement.
No shared private proposal file or configuration-pinned remote shielded key is required.
The seller's payment tag travels inside Noise and enters the final authorized agreement.
It is not a spending secret, identity binding, or proof of payment.

Reconnect uses retained drafts and transcript synchronization, not fresh deal randomness.
Both participants must recover the same root before the native command proceeds.
Count and root control messages remain outside the negotiation transcript.
The synchronous socket normalizes inherited nonblocking streams before deadline-bound I/O.

The native command supports both modes with separate participant processes and keys.
The buyer reserves policy capacity through the existing coordinator before signing.
The shielded seller persists its expected payment opening before releasing consent.
It exports verified selected-agreement evidence without submitting a payment.
This remains a component of M8, not completion of the installed private settlement workflow.
