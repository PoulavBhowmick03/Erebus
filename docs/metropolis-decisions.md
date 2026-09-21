# Metropolis M0 Decisions

Date: 2026-09-20. Scope: the new EVM developer product on `metropolis`.
These are implementation defaults for subsequent milestones, not claims of implemented behavior.
The [roadmap](metropolis-roadmap.md) owns completion status.

## D01. Product, modes, and custody

Target a Monad testnet developer release with SDK, CLI, MCP integration, and self-hostable services.
Keep public-bound and shielded settlement as distinct backend capabilities.
Never downgrade a signed privacy requirement. Public-bound success does not satisfy the private-payment release gate.
Select the chain, deployment, asset, and mode before negotiation. Bind them into the authorized agreement.
Keep the coordinator offchain. Keep proof generation internal to the backend that needs it.
Clients hold keys and authorize locally. The EVM shielded backend proves locally by default.
Mainnet activation, real-value limits, and production contract administration remain separate release decisions.

## D02. Agreement and replay identity

Use explicit bilateral authorization of a blinded commitment to canonical, versioned terms.
Include the service record, recipient, amount in base units, expiry, fee policy, and required capabilities.
M1 must specify exact byte encoding and vectors before any signature or circuit integration.
The service record identifies the resource, quantity/unit, intended access recipient, and fulfillment conditions.
Payment and delivery remain distinct states. Neither a terms digest nor a receipt proves service quality.

Use one settlement identity per deal, shared across all its signed revisions.
Include a fresh random 256-bit settlement nonce in every revision of that deal.
Derive the consumed identifier deterministically from a domain tag, deployment domain, buyer identity, and that fixed nonce.
Authorization covers all those fields. Private verification must constrain the same derivation.
Pool note nullifiers are separate from the deal-consumption identifier.

The first valid signed revision to settle consumes the deal. A later counteroffer does not revoke earlier signed permission.
Signed revisions remain executable until their authorized expiry or deal consumption.
Local cancellation stops local work only. Onchain revocation is not part of the initial contract.
Display this behavior before authorization and test competing signed revisions with different funding notes.
Changing deployment creates a different authorization domain. This does not promise global cross-chain deal uniqueness.

## D03. Authorization roles and policy

Separate transport authentication, agreement authorization, note spending, and transaction submission keys by role.
Do not infer business consent from the gas payer's transaction signature.
Bind buyer and seller roles to authorization to reject role substitution.
Persist the accepted commitment and authorized context before submission.

Enforce per-deal limits, per-asset aggregate limits, permitted assets, and counterparty permissions below the LLM.
Reserve capacity across concurrent operations and reconcile uncertain payments before releasing reservations.
M1 specifies policy time windows and configuration semantics before implementation.
Policy enforcement protects a configured operator. It is not a claim that a hostile host cannot change its own policy.

The signature suite and commitment hash remain M1/M4 gates.
Select them jointly with the private proof prototype to avoid freezing an impractical circuit interface.
Do not implement a generic signature verifier that treats every key type as interchangeable.

## D04. Compatibility and state

Keep existing Starknet wire codecs, state records, and CLI behavior under regression coverage.
Add new versioned EVM records rather than silently rewriting historical channels or journals.
Namespace new state by backend, chain, deployment, and local identity.
Reject incompatible versions explicitly. Require explicit migration and backup for future incompatible state changes.
Keep the existing protocol usable while new APIs are introduced additively.
Report legacy STRK20 agreement enforcement as client-side. A wrapper does not upgrade its proof guarantees.

## D05. Hosted and self-hosted services

Support native clients on Linux x86-64 and macOS arm64 initially.
Target Linux containers for service deployment, with documented persistent volumes and health endpoints.
Browser proving, mobile clients, and Intel macOS packaging are not initial release acceptance environments.
Use identical service implementations for shared testnet and self-hosted deployments.

| Service | Responsibility | Trust and availability boundary |
|---|---|---|
| Message relay | Authenticated mailbox access and ciphertext delivery | Sees routing, timing, and sizes; can delay or discard messages |
| Transaction relayer | Submit authorized envelopes with bound fees | Sees public inputs and network metadata; cannot change authorized outputs |
| Indexer | Serve public events with block identity and cursor | Can omit or stale data; clients verify chain evidence and recover by rescan |

The developer owns keys, witness generation, local policy, and encrypted backups.
The hosted operator owns availability, access limits, maintenance, diagnostics, and incident communication.
Self-hosting transfers those duties to the deploying team. No availability SLA is promised for the testnet service.
Discovery uses published service descriptors with authenticated identity and explicit settlement capabilities.
Start with a configured directory endpoint and portable descriptors, not an onchain discovery registry.
Public descriptors contain no negotiation transcripts or reservation prices.

Default relay retention is seven days, with an explicit expiry returned on message acceptance.
Acknowledgment confirms transport delivery, not settlement or permanent archival.
Clients persist authenticated transcripts before acknowledgment and retain their own recoverable copy.
Keep operational logs free of payloads, keys, authorization material, and agreement openings.
Use a seven-day default for access/security logs, disclose their metadata contents, and support earlier deletion where operationally possible.
Indexers retain public events from the configured deployment block and support rebuild from RPC.
Document actual backup retention and deletion behavior before shared service launch.
Resource limits and rate-limit values are M2/M6 configuration gates, verified with overload tests before hosting.

## D06. External acceptance scenario

Use synthetic services and test tokens. Record network, deployment, package, and verifier versions.
An external developer starts on a supported machine without this checkout, existing keys, or operator state.

1. Install documented packages and create local identities.
2. Discover a compatible seller through its published descriptor.
3. Configure spending limits and verify a disallowed counterparty or amount is rejected.
4. Obtain test funds and follow the displayed allowance, gas, and deposit instructions.
5. Negotiate and authorize a service agreement through their agent integration.
6. Complete a shielded Monad payment and verify the recipient's recovered note.
7. Interrupt a submitted operation and recover its receipt without a second payment.
8. Interrupt service access issuance and recover access without another charge.
9. Disclose one deal to a separate recipient and reject access to a neighboring deal.
10. Restore a wallet backup and withdraw or spend the remaining test funds.

The team provides only published instructions. Record any private assistance as a failed acceptance step until documentation fixes pass a fresh attempt.
A separate fresh Linux environment must run the self-hosting package and repeat the flow after endpoint switching.
Record installation errors, supported hardware, timings, policy denials, recovery results, and public observer inputs.
No external participant means this product gate remains open, regardless of internal test success.

## D07. Decisions deliberately gated after M0

M0 defines boundaries and acceptance. It does not certify the feasibility of an EVM privacy implementation.

| Gate | Decision evidence required before implementation depends on it |
|---|---|
| M1 agreement suite | Canonical encoding, hash, signature verification, field bounds, and cross-language vectors |
| M2 transport | Maintained crypto implementation, session/key lifecycle, message limits, and crash-safe nonce behavior |
| M3 EVM contracts | Supported token behavior, signing scheme, fee handling, and administration model |
| M4 proof and pool | Atomic agreement/payment binding, decryptable outputs, local proving measurements, and verifier compatibility |
| M8 shared service launch | Measured capacity, access quotas, retention controls, incident owner, and verified network configuration |

These gates must produce decision records and evidence. They are not permission to silently weaken the product guarantees.

## M1 decision record

The M1 agreement decisions (canonical encoding, suite registry, core crate boundary, expiry semantics, policy windows, and payment/delivery states) are recorded in [metropolis-agreement.md](metropolis-agreement.md) section 13, with M4 gate notes in [metropolis-m4-feasibility.md](metropolis-m4-feasibility.md). This document remains the M0 record.
