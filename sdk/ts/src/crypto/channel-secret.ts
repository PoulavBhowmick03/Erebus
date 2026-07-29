/**
 * Static-static ECDH over the Stark curve, for keying the off-chain negotiation
 * transport.
 *
 * Why this exists: the privacy pool cannot store a structured offer payload
 * (docs/friction.md F1), so negotiation moves to an off-chain transport. Its
 * confidentiality has to come from a secret only the two agents share. Both agents
 * already publish a Stark-curve viewing public key on-chain (`public_key[user_addr]`
 * in the pool), so they can derive that secret with no extra registration and no
 * extra round trip.
 *
 * Relationship to the pool's own crypto: the pool uses *ephemeral*-static ECDH
 * (`_compute_shared_x` in utils.cairo) — a fresh scalar per message. We use
 * static-static: each side's long-term viewing key against the other's published
 * public key. Same curve, same "x-coordinate only" convention, different scalar.
 *
 * The x-only convention matters. An on-chain public key is only the x-coordinate, so
 * recovering a curve point from it is sign-ambiguous — you may get P or -P. Since
 * `x(k·(-P)) == x(-(k·P)) == x(k·P)`, taking only the x-coordinate of the shared point
 * makes both sides agree regardless of which root they recovered. `derivedKeyAgrees`
 * in the tests pins this.
 *
 * Verified against Cairo reference vectors from
 * `packages/privacy/src/tests/generate_reference_data.cairo` — see the KAT block in
 * tests/channel-secret.test.ts.
 *
 * SDK BOUNDARY. Private keys enter here and never leave (CLAUDE.md constraint 6).
 * Nothing in this module logs, serialises, or returns a private key.
 */

import { ec, hash, shortString } from "starknet";

const starkCurve = ec.starkCurve;

/** Stark field prime. Sums are reduced mod this, matching felt252 arithmetic. */
const FIELD_PRIME = starkCurve.CURVE.Fp.ORDER;

/** Stark curve group order. */
const CURVE_ORDER = starkCurve.CURVE.n;

/**
 * Domain-separation tag for the transport key. Deliberately distinct from every tag in
 * the pool's `hashes.cairo` so a transport key can never collide with a pool
 * ciphertext mask.
 */
const TRANSPORT_KEY_TAG = "EREBUS_TRANSPORT_KEY:V1";

/** Hex-encoded felt252. */
export type Felt = string;

function toBigInt(value: Felt | bigint): bigint {
  return typeof value === "bigint" ? value : BigInt(value);
}

function toFelt(value: bigint): Felt {
  return `0x${value.toString(16)}`;
}

/** SEC1-compressed encoding of an x-only key, using the even-y root (see module docs). */
function pointFromX(publicKeyX: bigint): ReturnType<typeof starkCurve.ProjectivePoint.fromHex> {
  const padded = publicKeyX.toString(16).padStart(64, "0");
  return starkCurve.ProjectivePoint.fromHex(`02${padded}`);
}

/**
 * Rejects a private key the pool would reject.
 *
 * The pool requires non-zero and *canonical* — strictly below half the group order
 * (`is_canonical_key`, utils.cairo). A non-canonical key registers a public key the
 * pool will refuse to authenticate against, so catching it here turns a confusing
 * on-chain revert into a local error.
 */
function assertUsablePrivateKey(privateKey: bigint): void {
  if (privateKey <= 0n) {
    throw new Error("private key must be non-zero");
  }
  if (privateKey >= CURVE_ORDER) {
    throw new Error("private key must be below the curve order");
  }
  if (privateKey >= CURVE_ORDER / 2n) {
    throw new Error(
      "private key is not canonical (must be < ORDER/2); the pool will reject it"
    );
  }
}

function assertUsablePublicKey(publicKeyX: bigint): void {
  if (publicKeyX <= 0n) {
    throw new Error("public key must be non-zero");
  }
  if (publicKeyX >= FIELD_PRIME) {
    throw new Error("public key must be a valid felt252");
  }
  // Throws if x is not on the curve.
  pointFromX(publicKeyX);
}

/**
 * Derive the viewing public key from a viewing private key.
 *
 * Matches the pool's `derive_public_key`: the x-coordinate of `privateKey · G`. This is
 * the value the pool stores in `public_key[user_addr]`.
 */
export function deriveViewingPublicKey(privateKey: Felt | bigint): Felt {
  const key = toBigInt(privateKey);
  assertUsablePrivateKey(key);
  return starkCurve.getStarkKey(toFelt(key));
}

/**
 * Derive the shared secret between two agents.
 *
 * `sharedSecret(a, pub(b)) === sharedSecret(b, pub(a))`, so both sides compute the same
 * value independently with nothing transmitted. Returns the x-coordinate of the shared
 * point as a felt.
 *
 * Do not use the raw output as an encryption key — feed it to
 * {@link deriveTransportKey}, which adds domain separation.
 */
export function deriveSharedSecret(
  myPrivateKey: Felt | bigint,
  theirPublicKey: Felt | bigint
): bigint {
  const priv = toBigInt(myPrivateKey);
  const pub = toBigInt(theirPublicKey);
  assertUsablePrivateKey(priv);
  assertUsablePublicKey(pub);

  if (deriveViewingPublicKey(priv) === toFelt(pub)) {
    throw new Error("cannot derive a shared secret with yourself");
  }

  return pointFromX(pub).multiply(priv).toAffine().x;
}

/**
 * Domain-separated transport key for one channel.
 *
 * `h(TRANSPORT_KEY_TAG, sharedSecret, channelNonce)`. The nonce lets one agent pair run
 * several independent channels without reusing a key — pass the channel index, or any
 * value both sides agree on.
 *
 * This keys the off-chain transport only. It is not, and must not be confused with, the
 * pool's `channel_key`, which is a hash of the sender's *private* key and is
 * transmitted to the recipient encrypted (`EncChannelInfo.enc_channel_key`).
 */
export function deriveTransportKey(
  sharedSecret: bigint,
  channelNonce: number | bigint = 0n
): Felt {
  const nonce = typeof channelNonce === "bigint" ? channelNonce : BigInt(channelNonce);
  if (nonce < 0n) {
    throw new Error("channel nonce must be non-negative");
  }
  const tag = BigInt(shortString.encodeShortString(TRANSPORT_KEY_TAG));
  return hash.computePoseidonHashOnElements([tag, sharedSecret, nonce]);
}

/**
 * Convenience: private key + counterparty public key straight to a transport key.
 *
 * Both agents call this with their own private key and the other's published viewing
 * key, and land on the same value.
 */
export function deriveChannelTransportKey(
  myPrivateKey: Felt | bigint,
  theirPublicKey: Felt | bigint,
  channelNonce: number | bigint = 0n
): Felt {
  return deriveTransportKey(
    deriveSharedSecret(myPrivateKey, theirPublicKey),
    channelNonce
  );
}
