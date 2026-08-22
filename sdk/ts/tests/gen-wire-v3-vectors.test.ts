/** Generates normative wire-v3 vectors for the Rust/TypeScript differential tests. */

import { writeFileSync } from "node:fs";
import { expect, test } from "vitest";

import {
  decodeMessageV3,
  deriveDealIdV3,
  encodeMessageV3,
  WIRE_V3_CAPACITY_BITS,
  WIRE_V3_NOTES_PER_MESSAGE,
  WIRE_V3_PAYLOAD_BITS_PER_NOTE,
  type WireContextV3,
  type WireMessageV3,
} from "../src/channel/wire-v3.js";

const OUT = "../../rs/tests/fixtures/ts-wire-v3.json";

const CONTEXT: WireContextV3 = {
  chainId: 0x534e5f5345504f4c4941n,
  poolAddress: 0x9001n,
  channelKey: 0xc4a11en,
  token: 0x7042n,
  messageIndex: 0,
};

const DEAL_ID = deriveDealIdV3(CONTEXT);

const CASES: { name: string; messageIndex: number; message: WireMessageV3 }[] = [
  {
    name: "opening_offer_no_reply_to",
    messageIndex: 0,
    message: {
      dealId: DEAL_ID,
      type: "offer",
      createdAt: 1_753_699_200,
      terms: {
        amount: 1_000_000n,
        deadline: 1_753_702_800,
        memoHash: "0x1234567890abcdef1234567890abcdef",
      },
    },
  },
  {
    name: "counter_with_reply_to",
    messageIndex: 5,
    message: {
      dealId: DEAL_ID,
      type: "counter",
      replyTo: 0,
      createdAt: 1_753_699_260,
      terms: { amount: 950_000n, deadline: 1_753_702_800, memoHash: "0xdeadbeef" },
    },
  },
  {
    name: "accept",
    messageIndex: 10,
    message: {
      dealId: DEAL_ID,
      type: "accept",
      replyTo: 5,
      createdAt: 1_753_699_320,
      terms: { amount: 950_000n, deadline: 1_753_702_800, memoHash: "0x0" },
    },
  },
];

test("emit wire-v3 vectors for the Rust differential", () => {
  const vectors = CASES.map(({ name, messageIndex, message }) => {
    const context = { ...CONTEXT, messageIndex };
    const salts = encodeMessageV3(context, message);
    const decoded = decodeMessageV3(context, salts);
    expect(decoded.type).toBe(message.type);
    expect(decoded.replyTo).toBe(message.replyTo);
    expect(decoded.terms.amount).toBe(message.terms.amount);

    return {
      name,
      message_index: messageIndex,
      message: {
        deal_id: message.dealId.toString(),
        type: message.type,
        reply_to: message.replyTo ?? null,
        created_at: message.createdAt,
        amount: `0x${message.terms.amount.toString(16)}`,
        deadline: message.terms.deadline,
        memo_hash: message.terms.memoHash,
      },
      salts: salts.map((salt) => `0x${salt.toString(16)}`),
    };
  });

  writeFileSync(
    new URL(OUT, import.meta.url),
    `${JSON.stringify(
      {
        context: {
          chain_id: `0x${CONTEXT.chainId.toString(16)}`,
          pool_address: `0x${CONTEXT.poolAddress.toString(16)}`,
          channel_key: `0x${CONTEXT.channelKey.toString(16)}`,
          token: `0x${CONTEXT.token.toString(16)}`,
        },
        constants: {
          notes_per_message: WIRE_V3_NOTES_PER_MESSAGE,
          payload_bits_per_note: WIRE_V3_PAYLOAD_BITS_PER_NOTE,
          capacity_bits: WIRE_V3_CAPACITY_BITS,
        },
        vectors,
      },
      null,
      2
    )}\n`
  );
});
