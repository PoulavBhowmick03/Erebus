import { describe, expect, it } from "vitest";

import {
  decodeMessageV3,
  decodeMessageV3WithDealKey,
  deriveDealIdV3,
  deriveDealKeyV3,
  encodeMessageV3,
  type WireContextV3,
  type WireMessageV3,
} from "../src/channel/wire-v3.js";

const CONTEXT: WireContextV3 = {
  chainId: 0x534e5f5345504f4c4941n,
  poolAddress: 0x9001n,
  channelKey: 0xc4a11en,
  token: 0x7042n,
  messageIndex: 7,
};

const MESSAGE: WireMessageV3 = {
  dealId: 0x123456789abcdef0n,
  type: "counter",
  replyTo: 3,
  createdAt: 1_800_000_060,
  terms: { amount: 900_000n, deadline: 1_800_003_600, memoHash: "0xdeadbeef" },
};

describe("wire v3", () => {
  it("round-trips and is deterministic", () => {
    const first = encodeMessageV3(CONTEXT, MESSAGE);
    expect(encodeMessageV3(CONTEXT, MESSAGE)).toEqual(first);
    expect(decodeMessageV3(CONTEXT, first)).toEqual(MESSAGE);
    expect(deriveDealIdV3(CONTEXT)).toBe(0x5034ace485cca8bfn);
  });

  it("binds ciphertext to the complete context", () => {
    const salts = encodeMessageV3(CONTEXT, MESSAGE);
    expect(() => decodeMessageV3({ ...CONTEXT, messageIndex: 8 }, salts)).toThrow();
    expect(() => decodeMessageV3({ ...CONTEXT, token: CONTEXT.token + 1n }, salts)).toThrow();
  });

  it("rejects a changed masked spare bit", () => {
    const salts = encodeMessageV3(CONTEXT, MESSAGE);
    salts[4] ^= 1n << 116n;
    expect(() => decodeMessageV3(CONTEXT, salts)).toThrow(/padding/);
  });

  it("opens one deal with its native key and no parent channel key", () => {
    const salts = encodeMessageV3(CONTEXT, MESSAGE);
    const key = deriveDealKeyV3(CONTEXT, MESSAGE.dealId);
    const grantContext = { ...CONTEXT, channelKey: 0n };
    expect(decodeMessageV3WithDealKey(grantContext, MESSAGE.dealId, key, salts)).toEqual(MESSAGE);

    const otherDeal = MESSAGE.dealId + 1n;
    const otherKey = deriveDealKeyV3(CONTEXT, otherDeal);
    expect(() => decodeMessageV3WithDealKey(grantContext, otherDeal, otherKey, salts)).toThrow();
  });
});
