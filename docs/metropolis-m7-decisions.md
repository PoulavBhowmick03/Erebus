# Metropolis M7 Decisions

Date: 2026-10-02. Branch: `metropolis`.

## DM7-1: Direct Suite-2 Grant Signing

The owner selected direct participant signing, not a suite-1 grant with a participant attestation.
A shielded grant uses the participant's existing suite-2 seed for local signing.
Either the buyer or seller can issue a grant. A separate disclosure signer cannot issue one without participant authorization.
This format does not implement delegation.

The recipient uses a separate X25519 disclosure key, not a spending key.
Python and MCP pass local file paths to Rust. They never read the participant seed.
No circuit, proof generation, contract change, or hosted prover is needed for grant signing.

## DM7-2: Versioned Grant Format

Version 1 retains its existing suite-1 encoding and Keccak/secp256k1 signature.
It cannot authenticate a suite-2 agreement. Version 2 is exclusively a suite-2 shielded grant.
Unknown versions fail. Neither version falls back to the other.

Version 2 encodes these fields in order:

| Field | Encoding |
|---|---|
| Version | Big-endian `u16`, value `2` |
| Issuer | 64-byte suite-2 public key, `Ax || Ay` |
| Recipient | 32-byte X25519 public key |
| Deal ID | 16 bytes |
| Expiry | Big-endian `u64`, Unix seconds |
| Noise handshake | Length-prefixed bytes |
| Frame count | Big-endian `u32` |
| Ciphertext frames | Length-prefixed bytes per frame |
| Signature | 96 bytes, `R8x || R8y || S` |

Existing evidence encoding, grant bounds, encryption, and backup rules stay unchanged.
The decoder rejects trailing bytes. The verifier requires an independently supplied expected issuer.
After decryption, it verifies both agreement authorizations and requires the issuer to match one participant key.

## DM7-3: Disclosure Signature Domain

The version-2 Noise prologue is the Keccak hash of the domain, issuer, recipient, deal ID, and expiry.
The domain is `EREBUS_DEAL_DISCLOSURE_V2_SUITE2`.
The grant digest also includes the handshake, frame count, and each ciphertext frame with its length.
The signature message is `Poseidon([3001, high128(digest), low128(digest)])`.
Both digest limbs remain intact. This does not reduce a 256-bit digest modulo the scalar field.

Tag `3001` is separate from payment authorization tags `2004` and `2005`.
Tests pin independent circomlibjs vectors and reject disclosure signatures as payment authorizations.
The encrypted evidence binds the commitment opening, deployment, revision, transcript, and both participant authorizations.

## DM7-4: Observation And Metadata Boundary

The public header exposes the issuer public key, recipient public key, deal ID, expiry, and ciphertext length.
Observers with multiple grants can link a reused issuer key. This format does not hide those relationships between grant holders.
Only the recipient can decrypt the selected transcript and opening.
The grant contains no participant seed, note spend secret, or parent session key.

The auditor verifies payment through its own configured deployment and RPCs.
The encrypted grant carries signed settlement lookup fields through the agreement domain, commitment opening, and nullifier derivation.
The verified recipient package combines the opened agreement with independently observed `DealReads` settlement evidence.
An issuer-supplied receipt flag is never accepted as payment evidence.
Shielded observation requires two matching sources and separate public caches.
Those sources never receive the private agreement opening. Provider agreement does not prove provider independence or honest contract code.
Expiry is a verifier policy. It cannot erase plaintext or revoke a recipient's decryption key.
Payment verification does not verify service delivery.

## Local Evidence

The funded Anvil harness uses a replayable transcript and the existing test-only proving artifacts.
Separate `select`, `export`, agreement verification, and payment verification commands exercise the installed command protocol.
The harness renames participant storage before auditor verification.
The fresh auditor receives only its key, encrypted grant, and public deployment configuration.
The same workflow verifies payment through the official MCP stdio client.

This is local M7 evidence, not a Monad deployment, published package, secure ceremony, or audited release.
See [M7 progress](metropolis-m7-progress.md) and [M7 runbook](metropolis-m7-runbook.md).
