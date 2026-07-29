/**
 * Wire format v1 — negotiation messages carried in note salts.
 *
 * A pool note has no payload field. Its only client-writable space is the salt, which
 * the contract constrains to `2 <= salt < 2^120` and stores verbatim in the high 120
 * bits of `packed_value` (docs/friction.md F1). We carry negotiation state there, in
 * zero-amount notes that move no value.
 *
 * ## Why 119 bits, not 120
 *
 * A chunk that happened to be 0 or 1 would be rejected by the contract
 * (`ZERO_SALT` / `SALT_TOO_SMALL`) — rare, and a horrible bug to find. So bit 119 is
 * pinned to 1 and payload lives in bits 0-118. Every salt is then in
 * `[2^119, 2^120)`, always valid, no special cases.
 *
 * ## Layout
 *
 * Fixed width: 4 notes per message, at consecutive indices. Message `k` occupies
 * indices `4k .. 4k+3`, so a reader needs no framing search.
 *
 * ```
 *   note 0  header   [ type:8 | replyTo:32 | createdAt:40 ]        = 80 bits
 *   note 1  payload  [ amount high 119 bits of 128 ]
 *   note 2  payload  [ amount low 9 bits | deadline:64 | pad ]
 *   note 3  payload  [ memoHash truncated to 128 bits ]  (upper 119)
 * ```
 *
 * In practice we do not hand-place fields — {@link encodeMessage} serialises to a
 * bit string and chunks it. The table above is what that works out to.
 *
 * ## Compression
 *
 * `OfferTerms` is 5 felts (~760 bits) as declared. Two fields are redundant on the wire:
 * - `token` — a subchannel *is* a token, so both parties already know it;
 * - `nonce` — the note index orders messages and makes each unique.
 *
 * `memoHash` is truncated from 252 to 128 bits, leaving 2^64 collision resistance.
 * That leaves 320 bits of payload, which fits in 3 notes at 119 bits.
 *
 * ## Rule that governs which notes may use this
 *
 * Structured salts on **data** notes only. A value-bearing note must keep a random salt:
 * the salt is the one-time-pad nonce for the encrypted amount, so reusing a mask across
 * two differing amounts lets an observer subtract the ciphertexts and recover the
 * difference. Zero-amount notes have no variance and are immune.
 */

import type { OfferTerms } from "../interface.js";

/** Payload bits per note. Bit 119 is a constant 1; see module docs. */
export const PAYLOAD_BITS_PER_NOTE = 119n;

/** Notes per message. Fixed so `noteIndex = 4 * messageIndex + slot`. */
export const NOTES_PER_MESSAGE = 4;

const FLAG_BIT = 1n << 119n;
const PAYLOAD_MASK = FLAG_BIT - 1n;

/** Lowest salt the contract accepts (`salt > OPEN_NOTE_SALT`). */
const MIN_SALT = 2n;
const MAX_SALT = 1n << 120n;

const TYPE_BITS = 8n;
const REPLY_TO_BITS = 32n;
const CREATED_AT_BITS = 40n;
const AMOUNT_BITS = 128n;
const DEADLINE_BITS = 64n;
const MEMO_HASH_BITS = 128n;

/** Total payload width. Must fit in `NOTES_PER_MESSAGE * PAYLOAD_BITS_PER_NOTE`. */
const MESSAGE_BITS =
  TYPE_BITS +
  REPLY_TO_BITS +
  CREATED_AT_BITS +
  AMOUNT_BITS +
  DEADLINE_BITS +
  MEMO_HASH_BITS;

export type MessageType = "offer" | "counter" | "accept";

const TYPE_CODES: Record<MessageType, bigint> = {
  offer: 1n,
  counter: 2n,
  accept: 3n,
};

const TYPE_NAMES = new Map<bigint, MessageType>(
  Object.entries(TYPE_CODES).map(([name, code]) => [code, name as MessageType])
);

/**
 * A negotiation message as it goes on the wire.
 *
 * `replyTo` is a message index within this channel, not an opaque `OfferId` — 32 bits
 * of it. The SDK maps between the two; agents never see this type.
 */
export interface WireMessage {
  type: MessageType;
  /** Index of the message being replied to, or `undefined` for an opening offer. */
  replyTo?: number;
  /** Unix seconds. 40 bits — good past the year 36000. */
  createdAt: number;
  /** `token` and `nonce` are omitted deliberately; see module docs. */
  terms: Omit<OfferTerms, "token" | "nonce">;
}

/** No `replyTo`. 2^32-1 is reserved as the sentinel, so it is not a valid index. */
const NO_REPLY_TO = (1n << REPLY_TO_BITS) - 1n;

function assertFits(value: bigint, bits: bigint, name: string): void {
  if (value < 0n) {
    throw new Error(`${name} must be non-negative`);
  }
  if (value >= 1n << bits) {
    throw new Error(`${name} does not fit in ${bits} bits`);
  }
}

/** Truncates a felt252 memo hash to the low 128 bits carried on the wire. */
export function truncateMemoHash(memoHash: string): bigint {
  return BigInt(memoHash) & ((1n << MEMO_HASH_BITS) - 1n);
}

/** Packs a message into one big integer, most significant field first. */
function packMessage(message: WireMessage): bigint {
  const type = TYPE_CODES[message.type];
  if (type === undefined) {
    throw new Error(`unknown message type: ${String(message.type)}`);
  }

  const replyTo =
    message.replyTo === undefined ? NO_REPLY_TO : BigInt(message.replyTo);
  if (replyTo === NO_REPLY_TO && message.replyTo !== undefined) {
    throw new Error("replyTo 2^32-1 is reserved as the 'no reply' sentinel");
  }

  const createdAt = BigInt(message.createdAt);
  const amount = message.terms.amount;
  const deadline = BigInt(message.terms.deadline);
  const memoHash = truncateMemoHash(message.terms.memoHash);

  assertFits(type, TYPE_BITS, "type");
  assertFits(replyTo, REPLY_TO_BITS, "replyTo");
  assertFits(createdAt, CREATED_AT_BITS, "createdAt");
  assertFits(amount, AMOUNT_BITS, "amount");
  assertFits(deadline, DEADLINE_BITS, "deadline");

  let packed = 0n;
  packed = (packed << TYPE_BITS) | type;
  packed = (packed << REPLY_TO_BITS) | replyTo;
  packed = (packed << CREATED_AT_BITS) | createdAt;
  packed = (packed << AMOUNT_BITS) | amount;
  packed = (packed << DEADLINE_BITS) | deadline;
  packed = (packed << MEMO_HASH_BITS) | memoHash;
  return packed;
}

function unpackMessage(packed: bigint): WireMessage {
  const take = (value: bigint, bits: bigint): [bigint, bigint] => [
    value >> bits,
    value & ((1n << bits) - 1n),
  ];

  let rest = packed;
  let memoHash: bigint;
  let deadline: bigint;
  let amount: bigint;
  let createdAt: bigint;
  let replyTo: bigint;
  let type: bigint;

  [rest, memoHash] = take(rest, MEMO_HASH_BITS);
  [rest, deadline] = take(rest, DEADLINE_BITS);
  [rest, amount] = take(rest, AMOUNT_BITS);
  [rest, createdAt] = take(rest, CREATED_AT_BITS);
  [rest, replyTo] = take(rest, REPLY_TO_BITS);
  [rest, type] = take(rest, TYPE_BITS);

  const name = TYPE_NAMES.get(type);
  if (name === undefined) {
    throw new Error(`unknown message type code: ${type}`);
  }

  return {
    type: name,
    ...(replyTo === NO_REPLY_TO ? {} : { replyTo: Number(replyTo) }),
    createdAt: Number(createdAt),
    terms: {
      amount,
      deadline: Number(deadline),
      memoHash: `0x${memoHash.toString(16)}`,
    },
  };
}

/**
 * Encodes a message into exactly {@link NOTES_PER_MESSAGE} salts, in note-index order.
 *
 * Every returned salt satisfies the contract's `2 <= salt < 2^120`.
 */
export function encodeMessage(message: WireMessage): bigint[] {
  const packed = packMessage(message);

  const salts: bigint[] = [];
  for (let slot = NOTES_PER_MESSAGE - 1; slot >= 0; slot--) {
    const shift = BigInt(slot) * PAYLOAD_BITS_PER_NOTE;
    const chunk = (packed >> shift) & PAYLOAD_MASK;
    // Bit 119 pinned high keeps every salt inside the contract's valid range.
    salts.push(chunk | FLAG_BIT);
  }
  salts.reverse();

  for (const salt of salts) {
    if (salt < MIN_SALT || salt >= MAX_SALT) {
      throw new Error(`encoder produced an invalid salt: 0x${salt.toString(16)}`);
    }
  }
  return salts;
}

/** Inverse of {@link encodeMessage}. Salts must be in note-index order. */
export function decodeMessage(salts: readonly bigint[]): WireMessage {
  if (salts.length !== NOTES_PER_MESSAGE) {
    throw new Error(
      `expected ${NOTES_PER_MESSAGE} salts, got ${salts.length}`
    );
  }

  let packed = 0n;
  for (let slot = NOTES_PER_MESSAGE - 1; slot >= 0; slot--) {
    const salt = salts[slot]!;
    if ((salt & FLAG_BIT) === 0n) {
      throw new Error(
        `salt at slot ${slot} is missing the format flag — not an Erebus data note`
      );
    }
    packed = (packed << PAYLOAD_BITS_PER_NOTE) | (salt & PAYLOAD_MASK);
  }

  return unpackMessage(packed);
}

/** Note index of the first note of message `messageIndex` within a subchannel. */
export function noteIndexForMessage(messageIndex: number): number {
  if (!Number.isInteger(messageIndex) || messageIndex < 0) {
    throw new Error("message index must be a non-negative integer");
  }
  return messageIndex * NOTES_PER_MESSAGE;
}

/** Compile-time-ish guard: the layout must fit the notes we allocate for it. */
export const WIRE_CAPACITY_BITS = BigInt(NOTES_PER_MESSAGE) * PAYLOAD_BITS_PER_NOTE;
if (MESSAGE_BITS > WIRE_CAPACITY_BITS) {
  throw new Error(
    `wire layout is ${MESSAGE_BITS} bits but only ${WIRE_CAPACITY_BITS} are available`
  );
}
export const WIRE_MESSAGE_BITS = MESSAGE_BITS;
