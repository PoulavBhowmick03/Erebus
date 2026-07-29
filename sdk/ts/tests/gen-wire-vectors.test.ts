/**
 * Not a test — a vector generator, run through vitest because that is the only
 * TypeScript runner this package has.
 *
 * Emits the salt-lane encoding of a spread of messages so the Rust port can be pinned
 * against it byte-for-byte. Cairo emits no vector for this: the wire format is ours, so
 * the TypeScript implementation is the only oracle that exists.
 *
 * Regenerate with: pnpm vitest run tests/gen-wire-vectors.test.ts
 */
import { writeFileSync } from "node:fs";
import { test } from "vitest";

import {
  encodeMessage,
  decodeMessage,
  noteIndexForMessage,
  NOTES_PER_MESSAGE,
  PAYLOAD_BITS_PER_NOTE,
  WIRE_MESSAGE_BITS,
  WIRE_CAPACITY_BITS,
  type WireMessage,
} from "../src/channel/wire.js";

const OUT = "../../rs/tests/fixtures/ts-wire-salts.json";

const CASES: { name: string; message: WireMessage }[] = [
  {
    name: "opening_offer_no_reply_to",
    message: {
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
    message: {
      type: "counter",
      replyTo: 0,
      createdAt: 1_753_699_260,
      terms: {
        amount: 950_000n,
        deadline: 1_753_702_800,
        memoHash: "0xdeadbeef",
      },
    },
  },
  {
    name: "accept",
    message: {
      type: "accept",
      replyTo: 1,
      createdAt: 1_753_699_320,
      terms: {
        amount: 950_000n,
        deadline: 1_753_702_800,
        memoHash: "0x0",
      },
    },
  },
  {
    name: "all_fields_at_max",
    message: {
      type: "accept",
      replyTo: 0xfffffffe, // 2^32-1 is the no-reply sentinel, so this is the real max
      createdAt: 0xffffffffff, // 40 bits
      terms: {
        amount: (1n << 128n) - 1n,
        deadline: Number.MAX_SAFE_INTEGER,
        // 63 hex digits with a leading 7 keeps this under the STARK prime; a full
        // 64-digit value is not a valid felt252 (see friction.md F19).
        memoHash: "0x7" + "f".repeat(62),
      },
    },
  },
  {
    name: "all_fields_zero",
    message: {
      type: "offer",
      replyTo: 0,
      createdAt: 0,
      terms: { amount: 0n, deadline: 0, memoHash: "0x0" },
    },
  },
  {
    name: "memo_hash_needing_truncation",
    message: {
      type: "offer",
      createdAt: 1,
      terms: {
        // 252-bit value; only the low 128 bits survive onto the wire.
        amount: 1n,
        deadline: 1,
        memoHash: "0x7" + "a".repeat(62),
      },
    },
  },
];

test("emit wire vectors for the Rust port", () => {
  const vectors = CASES.map(({ name, message }) => {
    const salts = encodeMessage(message);
    // Round-trip here so a broken fixture fails at generation, not in Rust.
    decodeMessage(salts);
    return {
      name,
      message: {
        type: message.type,
        reply_to: message.replyTo ?? null,
        created_at: message.createdAt,
        amount: "0x" + message.terms.amount.toString(16),
        deadline: message.terms.deadline,
        memo_hash: message.terms.memoHash,
      },
      salts: salts.map((s) => "0x" + s.toString(16)),
    };
  });

  const payload = {
    constants: {
      notes_per_message: NOTES_PER_MESSAGE,
      payload_bits_per_note: Number(PAYLOAD_BITS_PER_NOTE),
      message_bits: Number(WIRE_MESSAGE_BITS),
      capacity_bits: Number(WIRE_CAPACITY_BITS),
    },
    note_indices: [0, 1, 2, 7].map((m) => ({
      message_index: m,
      first_note_index: noteIndexForMessage(m),
    })),
    vectors,
  };

  writeFileSync(
    new URL(OUT, import.meta.url),
    JSON.stringify(payload, null, 2) + "\n"
  );
});
