import { describe, expect, it } from "vitest";
import {
  decodeMessage,
  encodeMessage,
  NOTES_PER_MESSAGE,
  noteIndexForMessage,
  PAYLOAD_BITS_PER_NOTE,
  truncateMemoHash,
  WIRE_CAPACITY_BITS,
  WIRE_MESSAGE_BITS,
  type WireMessage,
} from "../src/channel/wire.js";

const MIN_SALT = 2n;
const MAX_SALT = 1n << 120n;

const OFFER: WireMessage = {
  type: "offer",
  createdAt: 1_800_000_000,
  terms: {
    amount: 1_000_000n,
    deadline: 1_800_003_600,
    memoHash: "0x5f2a91c3b7e40d68a1f95c2e8b34d70f6a29e51c8d3b06f4a7e29c15b8d34e06",
  },
};

describe("wire layout", () => {
  it("fits in the notes it allocates", () => {
    expect(WIRE_MESSAGE_BITS).toBeLessThanOrEqual(WIRE_CAPACITY_BITS);
  });

  it("uses 119 bits per note, leaving bit 119 for the format flag", () => {
    expect(PAYLOAD_BITS_PER_NOTE).toBe(119n);
  });
});

describe("encodeMessage", () => {
  it("produces exactly one salt per note", () => {
    expect(encodeMessage(OFFER)).toHaveLength(NOTES_PER_MESSAGE);
  });

  it("produces salts the contract will accept", () => {
    // The pool asserts 2 <= salt < 2^120 (ZERO_SALT / SALT_TOO_SMALL /
    // SALT_EXCEEDS_120_BITS). Verified against the live contract in
    // contracts/probes/p0_2_subchannel_payload.cairo.
    for (const salt of encodeMessage(OFFER)) {
      expect(salt).toBeGreaterThanOrEqual(MIN_SALT);
      expect(salt).toBeLessThan(MAX_SALT);
    }
  });

  it("never emits 0 or 1 even when the payload chunk is zero", () => {
    // The whole reason bit 119 is pinned. An all-zero message would otherwise
    // produce salt = 0, which the contract rejects.
    const empty: WireMessage = {
      type: "offer",
      createdAt: 0,
      terms: { amount: 0n, deadline: 0, memoHash: "0x0" },
    };
    for (const salt of encodeMessage(empty)) {
      expect(salt).toBeGreaterThanOrEqual(MIN_SALT);
    }
  });
});

describe("round trip", () => {
  it("preserves an opening offer", () => {
    const decoded = decodeMessage(encodeMessage(OFFER));
    expect(decoded.type).toBe("offer");
    expect(decoded.replyTo).toBeUndefined();
    expect(decoded.createdAt).toBe(OFFER.createdAt);
    expect(decoded.terms.amount).toBe(OFFER.terms.amount);
    expect(decoded.terms.deadline).toBe(OFFER.terms.deadline);
    // memoHash is truncated to 128 bits on the wire, by design.
    expect(BigInt(decoded.terms.memoHash)).toBe(truncateMemoHash(OFFER.terms.memoHash));
  });

  it("preserves a counter with replyTo", () => {
    const counter: WireMessage = {
      type: "counter",
      replyTo: 0,
      createdAt: 1_800_000_060,
      terms: { amount: 900_000n, deadline: 1_800_003_600, memoHash: "0xdeadbeef" },
    };
    const decoded = decodeMessage(encodeMessage(counter));
    expect(decoded.type).toBe("counter");
    expect(decoded.replyTo).toBe(0);
    expect(decoded.terms.amount).toBe(900_000n);
  });

  it("preserves an accept", () => {
    const accept: WireMessage = {
      type: "accept",
      replyTo: 1,
      createdAt: 1_800_000_120,
      terms: { amount: 900_000n, deadline: 1_800_003_600, memoHash: "0xdeadbeef" },
    };
    expect(decodeMessage(encodeMessage(accept)).type).toBe("accept");
  });

  it("preserves boundary values", () => {
    const max: WireMessage = {
      type: "accept",
      replyTo: 0xfffffffe, // 2^32-1 is the reserved sentinel
      createdAt: 0xffffffffff, // 40 bits
      terms: {
        amount: (1n << 128n) - 1n,
        deadline: Number.MAX_SAFE_INTEGER,
        memoHash: `0x${"f".repeat(32)}`,
      },
    };
    const decoded = decodeMessage(encodeMessage(max));
    expect(decoded.replyTo).toBe(0xfffffffe);
    expect(decoded.createdAt).toBe(0xffffffffff);
    expect(decoded.terms.amount).toBe((1n << 128n) - 1n);
    expect(decoded.terms.deadline).toBe(Number.MAX_SAFE_INTEGER);
  });

  it("distinguishes an absent replyTo from replyTo 0", () => {
    const withZero = decodeMessage(encodeMessage({ ...OFFER, replyTo: 0 }));
    const without = decodeMessage(encodeMessage(OFFER));
    expect(withZero.replyTo).toBe(0);
    expect(without.replyTo).toBeUndefined();
  });
});

describe("rejections", () => {
  it("rejects an amount wider than u128", () => {
    expect(() =>
      encodeMessage({ ...OFFER, terms: { ...OFFER.terms, amount: 1n << 128n } })
    ).toThrow(/amount does not fit/);
  });

  it("rejects the reserved replyTo sentinel", () => {
    expect(() => encodeMessage({ ...OFFER, replyTo: 0xffffffff })).toThrow(/reserved/);
  });

  it("rejects a negative deadline", () => {
    expect(() =>
      encodeMessage({ ...OFFER, terms: { ...OFFER.terms, deadline: -1 } })
    ).toThrow(/non-negative/);
  });

  it("rejects the wrong number of salts", () => {
    expect(() => decodeMessage(encodeMessage(OFFER).slice(0, 3))).toThrow(/expected 4/);
  });

  it("rejects a salt without the format flag", () => {
    // A random salt from a normal value note has no flag bit set with any
    // reliability — this is how we avoid decoding someone's payment as an offer.
    const salts = encodeMessage(OFFER);
    salts[2] = 0x1234n;
    expect(() => decodeMessage(salts)).toThrow(/format flag/);
  });
});

describe("noteIndexForMessage", () => {
  it("lays messages out at fixed stride", () => {
    expect(noteIndexForMessage(0)).toBe(0);
    expect(noteIndexForMessage(1)).toBe(4);
    expect(noteIndexForMessage(7)).toBe(28);
  });

  it("rejects a negative index", () => {
    expect(() => noteIndexForMessage(-1)).toThrow(/non-negative/);
  });
});
