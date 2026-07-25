import { describe, expect, it } from "vitest";
import { ec, hash, shortString } from "starknet";
import {
  deriveChannelTransportKey,
  deriveSharedSecret,
  deriveTransportKey,
  deriveViewingPublicKey,
} from "../src/crypto/channel-secret.js";

// Two arbitrary canonical viewing keys.
const ALICE_PRIV = "0x1234567890abcdef1234567890abcdef1234567890abcdef12345678";
const BOB_PRIV = "0x0fedcba9876543210fedcba9876543210fedcba9876543210fedcba9";

describe("known-answer tests against Cairo", () => {
  // Vectors printed by upstream
  // packages/privacy/src/tests/generate_reference_data.cairo::generate_reference_hashes
  // (run with `snforge test generate_reference_hashes --include-ignored`).
  // If any of these break, our curve arithmetic has diverged from the pool's.

  it("deriveViewingPublicKey matches derive_public_key", () => {
    expect(BigInt(deriveViewingPublicKey("0x12345"))).toBe(
      0x2f8ffcb446d2a062ef18561eb507b08ea01d52d4c594e90cfca47f075cb952n
    );
    expect(BigInt(deriveViewingPublicKey("0x54321"))).toBe(
      0x61d0f6b01e5696a475786dbbd6de15f984b23a553be50018489781de416140n
    );
    expect(BigInt(deriveViewingPublicKey("0xabcd"))).toBe(
      0x43299cc92ce884c67fa0353094e10e3d80b77f10607f266c79972941a42518an
    );
  });

  it("shared secret reproduces encrypt_private_key end to end", () => {
    // Cairo: encrypt_private_key(EPHEMERAL_SECRET=0xabcd, auditor_public_key,
    //        USER_PRIVATE_KEY=0x888)
    //   enc_private_key = h(ENC_PRIVATE_KEY_TAG, shared_x) + private_key
    // Exercises the whole path: point recovery from x, scalar multiply, x-only
    // shared secret, Poseidon with a domain tag, felt addition.
    const auditorPublicKey = deriveViewingPublicKey("0x54321");
    const sharedX = deriveSharedSecret("0xabcd", auditorPublicKey);

    const tag = BigInt(shortString.encodeShortString("ENC_PRIVATE_KEY_TAG:V1"));
    const mask = BigInt(hash.computePoseidonHashOnElements([tag, sharedX]));
    const encrypted = (mask + 0x888n) % ec.starkCurve.CURVE.Fp.ORDER;

    expect(encrypted).toBe(
      0x4c449c5af81633274b7af381a4c56717265c601ce9cc46cd360800077dcb092n
    );
  });
});

describe("deriveSharedSecret", () => {
  it("is symmetric — both agents land on the same value", () => {
    const alicePub = deriveViewingPublicKey(ALICE_PRIV);
    const bobPub = deriveViewingPublicKey(BOB_PRIV);

    expect(deriveSharedSecret(ALICE_PRIV, bobPub)).toBe(
      deriveSharedSecret(BOB_PRIV, alicePub)
    );
  });

  it("is sign-invariant across the recovered root", () => {
    // An on-chain public key is x-only, so point recovery may yield P or -P.
    // x(k·-P) === x(k·P), which is why the pool uses the x-coordinate alone.
    const bobPub = deriveViewingPublicKey(BOB_PRIV);
    const padded = BigInt(bobPub).toString(16).padStart(64, "0");

    const evenRoot = ec.starkCurve.ProjectivePoint.fromHex(`02${padded}`)
      .multiply(BigInt(ALICE_PRIV))
      .toAffine().x;
    const oddRoot = ec.starkCurve.ProjectivePoint.fromHex(`03${padded}`)
      .multiply(BigInt(ALICE_PRIV))
      .toAffine().x;

    expect(evenRoot).toBe(oddRoot);
    expect(deriveSharedSecret(ALICE_PRIV, bobPub)).toBe(evenRoot);
  });

  it("is deterministic", () => {
    const bobPub = deriveViewingPublicKey(BOB_PRIV);
    expect(deriveSharedSecret(ALICE_PRIV, bobPub)).toBe(
      deriveSharedSecret(ALICE_PRIV, bobPub)
    );
  });

  it("differs per counterparty", () => {
    const bobPub = deriveViewingPublicKey(BOB_PRIV);
    const carolPub = deriveViewingPublicKey("0x5555");
    expect(deriveSharedSecret(ALICE_PRIV, bobPub)).not.toBe(
      deriveSharedSecret(ALICE_PRIV, carolPub)
    );
  });

  it("rejects self-derivation", () => {
    const alicePub = deriveViewingPublicKey(ALICE_PRIV);
    expect(() => deriveSharedSecret(ALICE_PRIV, alicePub)).toThrow(/yourself/);
  });

  it("rejects a zero or non-canonical private key", () => {
    const bobPub = deriveViewingPublicKey(BOB_PRIV);
    expect(() => deriveSharedSecret(0n, bobPub)).toThrow(/non-zero/);
    // The pool requires key < ORDER/2 (is_canonical_key).
    const nonCanonical = ec.starkCurve.CURVE.n / 2n + 1n;
    expect(() => deriveSharedSecret(nonCanonical, bobPub)).toThrow(/canonical/);
  });

  it("rejects a public key not on the curve", () => {
    expect(() => deriveSharedSecret(ALICE_PRIV, 0n)).toThrow(/non-zero/);
    // x = 5 has no square root for y on this curve. Note x = 7 does — most small
    // integers are valid x-coordinates, so "looks like a small number" is not a
    // validity check.
    expect(() => deriveSharedSecret(ALICE_PRIV, 0x5n)).toThrow();
  });
});

describe("deriveTransportKey", () => {
  it("is domain-separated from the raw shared secret", () => {
    const bobPub = deriveViewingPublicKey(BOB_PRIV);
    const shared = deriveSharedSecret(ALICE_PRIV, bobPub);
    expect(BigInt(deriveTransportKey(shared))).not.toBe(shared);
  });

  it("gives independent keys per channel nonce", () => {
    const bobPub = deriveViewingPublicKey(BOB_PRIV);
    const shared = deriveSharedSecret(ALICE_PRIV, bobPub);
    expect(deriveTransportKey(shared, 0)).not.toBe(deriveTransportKey(shared, 1));
  });

  it("rejects a negative nonce", () => {
    const bobPub = deriveViewingPublicKey(BOB_PRIV);
    const shared = deriveSharedSecret(ALICE_PRIV, bobPub);
    expect(() => deriveTransportKey(shared, -1)).toThrow(/non-negative/);
  });
});

describe("deriveChannelTransportKey", () => {
  it("both agents derive the same transport key with no exchange", () => {
    const alicePub = deriveViewingPublicKey(ALICE_PRIV);
    const bobPub = deriveViewingPublicKey(BOB_PRIV);

    expect(deriveChannelTransportKey(ALICE_PRIV, bobPub, 7)).toBe(
      deriveChannelTransportKey(BOB_PRIV, alicePub, 7)
    );
  });
});
