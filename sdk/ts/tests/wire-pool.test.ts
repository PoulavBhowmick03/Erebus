/**
 * The wire format through an actual pool.
 *
 * Proves the thing that matters for P1.3: an offer encoded into note salts survives a
 * real write/read cycle, and the counterparty recovers it by keyed read rather than
 * scanning. Offline — mock pool, no prover, no network.
 *
 * This bypasses the SDK builder deliberately. The builder hardcodes
 * `salt: generateRandom120()` (compiler.ts:424) and discards the salt on read
 * (contract-discovery.ts:193), so we construct `ClientAction[]` directly — which is the
 * shape we will hand to the prover on Sepolia.
 */

import { describe, expect, it } from "vitest";
import {
  Mocknet,
  compute_channel_key,
  compute_note_id,
} from "@starkware-libs/starknet-privacy-sdk/testing";
import type { ExecuteResult, PrivateRegistry } from "@starkware-libs/starknet-privacy-sdk";
import {
  decodeMessage,
  encodeMessage,
  NOTES_PER_MESSAGE,
  noteIndexForMessage,
  truncateMemoHash,
  type WireMessage,
} from "../src/channel/wire.js";

const POOL_ADDRESS = 0x1n;

const OFFER: WireMessage = {
  type: "offer",
  createdAt: 1_800_000_000,
  terms: {
    amount: 1_000_000n,
    deadline: 1_800_003_600,
    memoHash: "0x5f2a91c3b7e40d68a1f95c2e8b34d70f6a29e51c8d3b06f4a7e29c15b8d34e06",
  },
};

const COUNTER: WireMessage = {
  type: "counter",
  replyTo: 0,
  createdAt: 1_800_000_060,
  terms: { amount: 850_000n, deadline: 1_800_003_600, memoHash: "0xc0ffee" },
};

/** Sets up sender→recipient with a channel and a token subchannel. */
async function openChannel(
  mocknet: Mocknet,
  sender: ReturnType<Mocknet["createPrivateTransfers"]>,
  recipient: ReturnType<Mocknet["createPrivateTransfers"]>,
  recipientAddress: bigint,
  token: bigint
): Promise<void> {
  const submit = (result: ExecuteResult): PrivateRegistry => {
    mocknet.pool.apply_actions(result.callAndProof.call.calldata as string[]);
    return result.registry;
  };
  submit(await recipient.build().register().execute());
  submit(await sender.build().register().execute());
  submit(await sender.build().setup(recipientAddress).execute());

  const registry = (await sender.discoverChannels([recipientAddress])).channels;
  const reg = { channels: registry } as unknown as PrivateRegistry;
  submit(await sender.build({ registry: reg }).with(token).setup(recipientAddress).execute());
}

/**
 * Writes one message as `NOTES_PER_MESSAGE` zero-amount notes at consecutive indices.
 * Returns the note indices used.
 */
function writeMessage(
  mocknet: Mocknet,
  sender: { address: bigint; privateKey: bigint },
  recipientAddress: bigint,
  recipientPublicKey: bigint,
  token: bigint,
  messageIndex: number,
  message: WireMessage
): number[] {
  const salts = encodeMessage(message);
  const base = noteIndexForMessage(messageIndex);

  const actions = salts.map((salt, slot) => ({
    type: "CreateEncNote" as const,
    input: {
      recipient_addr: recipientAddress,
      recipient_public_key: recipientPublicKey,
      token,
      // Zero amount: a pure data note. Moves no value, needs no deposit.
      amount: 0n,
      index: base + slot,
      salt,
    },
  }));

  // Mirrors the real pipeline: `execute` compiles and simulates but restores its
  // snapshot (compile_actions is a view), so nothing lands until apply_actions.
  // On Sepolia the gap between these two lines is where proof generation happens.
  const serverActionTypes = mocknet.pool.execute(
    sender.address,
    sender.privateKey,
    ...actions
  );
  // Trailing "0x1" is Serde for Option::None — no screening attestation, which is
  // correct here because data notes contain no TransferFrom (friction.md F6).
  mocknet.pool.apply_actions([...serverActionTypes, "0x1"]);

  return actions.map((action) => action.input.index);
}

/**
 * Reads a message back the way the counterparty would: derive each note_id from the
 * shared channel key, read the note, take the top 120 bits of packed_value.
 */
function readMessage(
  mocknet: Mocknet,
  channelKey: bigint,
  token: bigint,
  messageIndex: number
): WireMessage {
  const base = noteIndexForMessage(messageIndex);
  const salts: bigint[] = [];

  for (let slot = 0; slot < NOTES_PER_MESSAGE; slot++) {
    const noteId = compute_note_id(channelKey, token, base + slot);
    const note = mocknet.pool.get_note(noteId);
    const packedValue = BigInt(note.packed_value);
    if (packedValue === 0n) {
      throw new Error(`note ${base + slot} does not exist`);
    }
    salts.push(packedValue >> 128n);
  }

  return decodeMessage(salts);
}

describe("wire format through the pool", () => {
  it("an offer written into note salts is recovered by the counterparty", async () => {
    const mocknet = new Mocknet({ poolAddress: POOL_ADDRESS });
    const env = mocknet.initialize();
    const alice = mocknet.createPrivateTransfers(env.alice.address, env.alice.privateKey);
    const bob = mocknet.createPrivateTransfers(env.bob.address, env.bob.privateKey);
    const token = BigInt(env.ace);

    await openChannel(mocknet, alice, bob, env.bob.address, token);

    const bobPublicKey = mocknet.pool.get_public_key(env.bob.address);
    writeMessage(mocknet, env.alice, env.bob.address, bobPublicKey, token, 0, OFFER);

    // Bob's view: he can derive the channel key, so he knows exactly where to look.
    const channelKey = compute_channel_key(
      env.alice.address,
      env.alice.privateKey,
      env.bob.address,
      bobPublicKey
    );
    const decoded = readMessage(mocknet, channelKey, token, 0);

    expect(decoded.type).toBe("offer");
    expect(decoded.terms.amount).toBe(OFFER.terms.amount);
    expect(decoded.terms.deadline).toBe(OFFER.terms.deadline);
    expect(decoded.createdAt).toBe(OFFER.createdAt);
    expect(BigInt(decoded.terms.memoHash)).toBe(truncateMemoHash(OFFER.terms.memoHash));
  });

  it("carries a multi-message negotiation at fixed stride", async () => {
    const mocknet = new Mocknet({ poolAddress: POOL_ADDRESS });
    const env = mocknet.initialize();
    const alice = mocknet.createPrivateTransfers(env.alice.address, env.alice.privateKey);
    const bob = mocknet.createPrivateTransfers(env.bob.address, env.bob.privateKey);
    const token = BigInt(env.ace);

    await openChannel(mocknet, alice, bob, env.bob.address, token);
    const bobPublicKey = mocknet.pool.get_public_key(env.bob.address);

    // Both messages go in Alice's channel to Bob. A real counter from Bob would need
    // his own channel back to Alice — channels are directional.
    writeMessage(mocknet, env.alice, env.bob.address, bobPublicKey, token, 0, OFFER);
    writeMessage(mocknet, env.alice, env.bob.address, bobPublicKey, token, 1, COUNTER);

    const channelKey = compute_channel_key(
      env.alice.address,
      env.alice.privateKey,
      env.bob.address,
      bobPublicKey
    );

    expect(readMessage(mocknet, channelKey, token, 0).type).toBe("offer");

    const second = readMessage(mocknet, channelKey, token, 1);
    expect(second.type).toBe("counter");
    expect(second.replyTo).toBe(0);
    expect(second.terms.amount).toBe(850_000n);
  });

  it("data notes move no value — nobody's balance changes", async () => {
    const mocknet = new Mocknet({ poolAddress: POOL_ADDRESS });
    const env = mocknet.initialize();
    const alice = mocknet.createPrivateTransfers(env.alice.address, env.alice.privateKey);
    const bob = mocknet.createPrivateTransfers(env.bob.address, env.bob.privateKey);
    const token = BigInt(env.ace);

    await openChannel(mocknet, alice, bob, env.bob.address, token);
    const bobPublicKey = mocknet.pool.get_public_key(env.bob.address);

    writeMessage(mocknet, env.alice, env.bob.address, bobPublicKey, token, 0, OFFER);

    // Zero-amount notes carry no value, so discovery reports nothing spendable.
    const notes = await bob.discoverNotes([token]);
    const spendable = (notes.notes.get(token) ?? []).reduce(
      (sum, note) => sum + BigInt(note.amount),
      0n
    );
    expect(spendable).toBe(0n);
  });

  it("indices stay contiguous — the pool enforces it", async () => {
    const mocknet = new Mocknet({ poolAddress: POOL_ADDRESS });
    const env = mocknet.initialize();
    const alice = mocknet.createPrivateTransfers(env.alice.address, env.alice.privateKey);
    const bob = mocknet.createPrivateTransfers(env.bob.address, env.bob.privateKey);
    const token = BigInt(env.ace);

    await openChannel(mocknet, alice, bob, env.bob.address, token);
    const bobPublicKey = mocknet.pool.get_public_key(env.bob.address);

    // Skipping message 0 and writing message 1 first leaves a gap at indices 0-3.
    // INDEX_NOT_SEQUENTIAL on-chain; the encoder must never do this.
    expect(() =>
      writeMessage(mocknet, env.alice, env.bob.address, bobPublicKey, token, 1, OFFER)
    ).toThrow();
  });
});
