/**
 * Independent TypeScript oracle for Erebus wire v3.
 *
 * Rust remains the production writer. This implementation exists to pin the cryptographic
 * and bit-level contract in a second language. It deliberately does not fall back to v2:
 * a channel's persisted wire version chooses exactly one decoder.
 */

import { gcmsiv } from "@noble/ciphers/aes.js";
import { hkdf } from "@noble/hashes/hkdf";
import { sha256 } from "@noble/hashes/sha256";

import { truncateMemoHash, type MessageType, type WireMessage } from "./wire.js";

/** Wire-v3 adds a deal id to the historical negotiation fields. */
export type WireMessageV3 = WireMessage & { dealId: bigint };

export const WIRE_V3_NOTES_PER_MESSAGE = 5;
export const WIRE_V3_PAYLOAD_BITS_PER_NOTE = 119;
export const WIRE_V3_CAPACITY_BITS =
  WIRE_V3_NOTES_PER_MESSAGE * WIRE_V3_PAYLOAD_BITS_PER_NOTE;

const HEADER_BYTES = 8;
const MESSAGE_BYTES = 50;
const TAG_BYTES = 16;
const PAYLOAD_BYTES = HEADER_BYTES + MESSAGE_BYTES + TAG_BYTES;
const PAYLOAD_BITS = PAYLOAD_BYTES * 8;
const MASK_LO = PAYLOAD_BITS;
const MASK_BITS = WIRE_V3_CAPACITY_BITS - MASK_LO;
const MASK_BYTES = Math.ceil(MASK_BITS / 8);
const ENVELOPE_BYTES = Math.ceil(WIRE_V3_CAPACITY_BITS / 8);
const FLAG_BIT = 1n << 119n;
const PAYLOAD_MASK = FLAG_BIT - 1n;
const MAX_SALT = 1n << 120n;

const encoder = new TextEncoder();
const NO_REPLY_TO = 0xffffffffn;
const TYPE_CODES: Record<MessageType, bigint> = {
  offer: 1n,
  counter: 2n,
  accept: 3n,
};
const TYPE_NAMES = new Map<bigint, MessageType>(
  Object.entries(TYPE_CODES).map(([name, code]) => [code, name as MessageType])
);

/** Context authenticated by wire v3. Felts are encoded as 32-byte big-endian values. */
export interface WireContextV3 {
  chainId: bigint;
  poolAddress: bigint;
  channelKey: bigint;
  token: bigint;
  messageIndex: number;
}

function concat(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let cursor = 0;
  for (const part of parts) {
    out.set(part, cursor);
    cursor += part.length;
  }
  return out;
}

function unsignedBytes(value: bigint, width: number, name: string): Uint8Array {
  if (value < 0n || value >= 1n << BigInt(width * 8)) {
    throw new Error(`${name} does not fit in ${width} bytes`);
  }
  const bytes = new Uint8Array(width);
  let rest = value;
  for (let index = width - 1; index >= 0; index--) {
    bytes[index] = Number(rest & 0xffn);
    rest >>= 8n;
  }
  return bytes;
}

function indexBytes(index: number): Uint8Array {
  if (!Number.isSafeInteger(index) || index < 0 || index > 0xffffffff) {
    throw new Error("messageIndex does not fit in u32");
  }
  return unsignedBytes(BigInt(index), 4, "messageIndex");
}

function scope(context: WireContextV3): Uint8Array {
  return concat(
    unsignedBytes(context.chainId, 32, "chainId"),
    unsignedBytes(context.poolAddress, 32, "poolAddress"),
    unsignedBytes(context.token, 32, "token")
  );
}

/** Native encryption key for one deal in one direction. */
export function deriveDealKeyV3(context: WireContextV3, dealId: bigint): Uint8Array {
  const material = unsignedBytes(context.channelKey, 32, "channelKey");
  return hkdf(
    sha256,
    material,
    encoder.encode("EREBUS_WIRE_V3_DEAL_KEY_HKDF_SHA256"),
    concat(
      encoder.encode("EREBUS_WIRE_V3_DEAL_KEY"),
      scope(context),
      unsignedBytes(dealId, HEADER_BYTES, "dealId")
    ),
    32
  );
}

function nonce(context: WireContextV3, dealKey: Uint8Array): Uint8Array {
  const salt = encoder.encode("EREBUS_WIRE_V3_HKDF_SHA256");
  const nonce = hkdf(
    sha256,
    dealKey,
    salt,
    concat(encoder.encode("EREBUS_WIRE_V3_NONCE"), scope(context), indexBytes(context.messageIndex)),
    12
  );
  return nonce;
}

function dealHeader(context: WireContextV3, dealId: bigint): Uint8Array {
  const mask = hkdf(
    sha256,
    unsignedBytes(context.channelKey, 32, "channelKey"),
    encoder.encode("EREBUS_WIRE_V3_HEADER_HKDF_SHA256"),
    concat(
      encoder.encode("EREBUS_WIRE_V3_DEAL_HEADER"),
      scope(context),
      indexBytes(context.messageIndex)
    ),
    HEADER_BYTES
  );
  const header = unsignedBytes(dealId, HEADER_BYTES, "dealId");
  return header.map((byte, index) => byte ^ (mask[index] ?? 0));
}

function recoverDealId(context: WireContextV3, header: Uint8Array): bigint {
  const mask = dealHeader(context, 0n);
  let value = 0n;
  for (let index = 0; index < HEADER_BYTES; index++) {
    value = (value << 8n) | BigInt((header[index] ?? 0) ^ (mask[index] ?? 0));
  }
  return value;
}

function associatedData(
  context: WireContextV3,
  dealId: bigint,
  header: Uint8Array
): Uint8Array {
  return concat(
    encoder.encode("EREBUS_WIRE_V3_AAD"),
    scope(context),
    indexBytes(context.messageIndex),
    unsignedBytes(dealId, HEADER_BYTES, "dealId"),
    header
  );
}

function mask(context: WireContextV3, dealKey: Uint8Array, header: Uint8Array): Uint8Array {
  return hkdf(
    sha256,
    dealKey,
    encoder.encode("EREBUS_WIRE_V3_MASK_HKDF_SHA256"),
    concat(
      encoder.encode("EREBUS_WIRE_V3_MASK"),
      scope(context),
      indexBytes(context.messageIndex),
      header
    ),
    MASK_BYTES
  );
}

function bitAtLsb(bytes: Uint8Array, position: number): boolean {
  const bitFromMsb = bytes.length * 8 - 1 - position;
  const byte = bytes[Math.floor(bitFromMsb / 8)];
  if (byte === undefined) throw new Error(`bit position ${position} is outside the envelope`);
  return (byte & (1 << (7 - (bitFromMsb % 8)))) !== 0;
}

function setBitLsb(bytes: Uint8Array, position: number): void {
  const bitFromMsb = bytes.length * 8 - 1 - position;
  const index = Math.floor(bitFromMsb / 8);
  const byte = bytes[index];
  if (byte === undefined) throw new Error(`bit position ${position} is outside the envelope`);
  bytes[index] = byte | (1 << (7 - (bitFromMsb % 8)));
}

function maskBit(bytes: Uint8Array, offset: number): boolean {
  const byte = bytes[Math.floor(offset / 8)];
  if (byte === undefined) throw new Error(`mask bit ${offset} is outside the keystream`);
  return (byte & (1 << (offset % 8))) !== 0;
}

function assertFits(value: bigint, bits: bigint, name: string): void {
  if (value < 0n || value >= 1n << bits) {
    throw new Error(`${name} does not fit in ${bits} bits`);
  }
}

function messageBytes(message: WireMessageV3): Uint8Array {
  const type = TYPE_CODES[message.type];
  if (type === undefined) throw new Error(`unknown message type: ${String(message.type)}`);
  const replyTo = message.replyTo === undefined ? NO_REPLY_TO : BigInt(message.replyTo);
  if (message.replyTo !== undefined && replyTo === NO_REPLY_TO) {
    throw new Error("replyTo 2^32-1 is reserved as the 'no reply' sentinel");
  }
  const fields: [bigint, bigint, string][] = [
    [type, 8n, "type"],
    [replyTo, 32n, "replyTo"],
    [BigInt(message.createdAt), 40n, "createdAt"],
    [message.terms.amount, 128n, "amount"],
    [BigInt(message.terms.deadline), 64n, "deadline"],
    [truncateMemoHash(message.terms.memoHash), 128n, "memoHash"],
  ];
  let packed = 0n;
  for (const [value, bits, name] of fields) {
    assertFits(value, bits, name);
    packed = (packed << bits) | value;
  }
  return unsignedBytes(packed, MESSAGE_BYTES, "wire-v3 message");
}

function unpackMessage(dealId: bigint, bytes: Uint8Array): WireMessageV3 {
  let packed = 0n;
  for (const byte of bytes) packed = (packed << 8n) | BigInt(byte);
  const take = (bits: bigint): bigint => {
    const value = packed & ((1n << bits) - 1n);
    packed >>= bits;
    return value;
  };
  const memoHash = take(128n);
  const deadline = take(64n);
  const amount = take(128n);
  const createdAt = take(40n);
  const replyTo = take(32n);
  const typeCode = take(8n);
  const type = TYPE_NAMES.get(typeCode);
  if (type === undefined) throw new Error(`unknown message type code: ${typeCode}`);
  return {
    dealId,
    type,
    ...(replyTo === NO_REPLY_TO ? {} : { replyTo: Number(replyTo) }),
    createdAt: Number(createdAt),
    terms: {
      amount,
      deadline: Number(deadline),
      memoHash: `0x${memoHash.toString(16)}`,
    },
  };
}

/** Derives an opening offer's 64-bit deal id from its physical frame start. */
export function deriveDealIdV3(context: WireContextV3): bigint {
  const bytes = hkdf(
    sha256,
    unsignedBytes(context.channelKey, 32, "channelKey"),
    encoder.encode("EREBUS_WIRE_V3_DEAL_HKDF_SHA256"),
    concat(encoder.encode("EREBUS_WIRE_V3_DEAL_ID"), scope(context), indexBytes(context.messageIndex)),
    8
  );
  let value = 0n;
  for (const byte of bytes) value = (value << 8n) | BigInt(byte);
  return value;
}

/** Encrypts a message into the exact five salts emitted by Rust wire v3. */
export function encodeMessageV3(context: WireContextV3, message: WireMessageV3): bigint[] {
  const header = dealHeader(context, message.dealId);
  const dealKey = deriveDealKeyV3(context, message.dealId);
  const sealed = gcmsiv(
    dealKey,
    nonce(context, dealKey),
    associatedData(context, message.dealId, header)
  ).encrypt(messageBytes(message));
  if (sealed.length !== MESSAGE_BYTES + TAG_BYTES) {
    throw new Error(`AES-GCM-SIV returned ${sealed.length} bytes, expected 66`);
  }
  const payload = concat(header, sealed);
  const keystream = mask(context, dealKey, header);

  const salts: bigint[] = [];
  for (let slot = 0; slot < WIRE_V3_NOTES_PER_MESSAGE; slot++) {
    let chunk = 0n;
    for (let bit = 0; bit < WIRE_V3_PAYLOAD_BITS_PER_NOTE; bit++) {
      const position = slot * WIRE_V3_PAYLOAD_BITS_PER_NOTE + bit;
      const carried = position < PAYLOAD_BITS && bitAtLsb(payload, position);
      const masked = position >= MASK_LO && maskBit(keystream, position - MASK_LO);
      if (carried !== masked) chunk |= 1n << BigInt(bit);
    }
    salts.push(chunk | FLAG_BIT);
  }
  return salts;
}

/** Authenticates and decodes exactly one wire-v3 message. */
export function decodeMessageV3(context: WireContextV3, salts: bigint[]): WireMessageV3 {
  if (salts.length !== WIRE_V3_NOTES_PER_MESSAGE) {
    throw new Error(`expected ${WIRE_V3_NOTES_PER_MESSAGE} salts, got ${salts.length}`);
  }

  const envelope = new Uint8Array(ENVELOPE_BYTES);
  for (const [slot, salt] of salts.entries()) {
    if (salt < 2n || salt >= MAX_SALT) throw new Error(`salt ${slot} is outside the pool range`);
    if ((salt & FLAG_BIT) === 0n) throw new Error(`salt ${slot} is missing the format flag`);
    const chunk = salt & PAYLOAD_MASK;
    for (let bit = 0; bit < WIRE_V3_PAYLOAD_BITS_PER_NOTE; bit++) {
      const position = slot * WIRE_V3_PAYLOAD_BITS_PER_NOTE + bit;
      if (position < PAYLOAD_BITS && ((chunk >> BigInt(bit)) & 1n) === 1n) {
        setBitLsb(envelope, position);
      }
    }
  }

  const payload = envelope.slice(ENVELOPE_BYTES - PAYLOAD_BYTES);
  const header = payload.slice(0, HEADER_BYTES);
  const dealId = recoverDealId(context, header);
  const dealKey = deriveDealKeyV3(context, dealId);
  return decodeMessageV3WithDealKey(context, dealId, dealKey, salts);
}

/** Authenticates one frame without receiving the parent channel key. */
export function decodeMessageV3WithDealKey(
  context: WireContextV3,
  dealId: bigint,
  dealKey: Uint8Array,
  salts: bigint[]
): WireMessageV3 {
  if (salts.length !== WIRE_V3_NOTES_PER_MESSAGE) {
    throw new Error(`expected ${WIRE_V3_NOTES_PER_MESSAGE} salts, got ${salts.length}`);
  }
  const envelope = new Uint8Array(ENVELOPE_BYTES);
  for (const [slot, salt] of salts.entries()) {
    if (salt < 2n || salt >= MAX_SALT) throw new Error(`salt ${slot} is outside the pool range`);
    if ((salt & FLAG_BIT) === 0n) throw new Error(`salt ${slot} is missing the format flag`);
    const chunk = salt & PAYLOAD_MASK;
    for (let bit = 0; bit < WIRE_V3_PAYLOAD_BITS_PER_NOTE; bit++) {
      const position = slot * WIRE_V3_PAYLOAD_BITS_PER_NOTE + bit;
      if (position < PAYLOAD_BITS && ((chunk >> BigInt(bit)) & 1n) === 1n) {
        setBitLsb(envelope, position);
      }
    }
  }
  const payload = envelope.slice(ENVELOPE_BYTES - PAYLOAD_BYTES);
  const header = payload.slice(0, HEADER_BYTES);
  const keystream = mask(context, dealKey, header);
  for (let position = PAYLOAD_BITS; position < WIRE_V3_CAPACITY_BITS; position++) {
    const slot = Math.floor(position / WIRE_V3_PAYLOAD_BITS_PER_NOTE);
    const bit = position % WIRE_V3_PAYLOAD_BITS_PER_NOTE;
    const carried = (((salts[slot] ?? 0n) >> BigInt(bit)) & 1n) === 1n;
    if (carried !== maskBit(keystream, position - MASK_LO)) {
      throw new Error("invalid wire-v3 padding");
    }
  }
  const sealed = payload.slice(HEADER_BYTES);
  const plaintext = gcmsiv(
    dealKey,
    nonce(context, dealKey),
    associatedData(context, dealId, header)
  ).decrypt(sealed);
  return unpackMessage(dealId, plaintext);
}
