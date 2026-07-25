/**
 * The baseline STRK20 flow, end to end, offline (P1.1).
 *
 * register → open channel → open subchannel → deposit → private transfer → discover.
 *
 * Runs entirely against upstream's in-memory `Mocknet`: mock pool contract, mock
 * proving, contract-backed discovery. No RPC, no devnet, no prover, no network. That
 * matters right now because there is no published Sepolia proving-service endpoint
 * (docs/friction.md F5), so this is how far we can get before StarkWare answers.
 *
 * What this does NOT prove: real proof generation, screening attestations on the
 * deposit leg (docs/friction.md F6), or anything about timing. The mock proof provider
 * skips all of it.
 */

import { describe, expect, it } from "vitest";
import { Mocknet } from "@starkware-libs/starknet-privacy-sdk/testing";
import {
  createEmptyRegistry,
  type ExecuteResult,
  type PrivateRegistry,
  type PrivateTransfersInterface,
} from "@starkware-libs/starknet-privacy-sdk";

const POOL_ADDRESS = 0x1n;

/**
 * Stands in for submitting the transaction. On a real network this is
 * `apply_actions(actions)` carrying proof facts; here the mock pool applies the server
 * actions directly. Same action set either way — only the proof is faked.
 */
function makeSubmit(pool: { apply_actions: (calldata: string[]) => unknown }) {
  return (result: ExecuteResult): PrivateRegistry => {
    pool.apply_actions(result.callAndProof.call.calldata as string[]);
    return result.registry;
  };
}

/** Registers `user` and opens a self-channel plus a token subchannel. */
async function openSelfChannel(
  user: PrivateTransfersInterface,
  address: bigint,
  token: bigint,
  submit: (result: ExecuteResult) => PrivateRegistry
): Promise<PrivateRegistry> {
  submit(await user.build().register().execute());
  submit(await user.build().setup(address).execute());

  const registry = createEmptyRegistry();
  registry.channels.set(address, (await user.discoverChannels([address])).channels.get(address)!);

  // A subchannel is per (channel, token) and a note cannot exist without one.
  submit(await user.build({ registry }).with(token).setup(address).execute());
  registry.channels.set(address, (await user.discoverChannels([address])).channels.get(address)!);

  return registry;
}

describe("baseline STRK20 flow (offline)", () => {
  it("registers, shields, transfers privately, and the recipient finds the note", async () => {
    const mocknet = new Mocknet({ poolAddress: POOL_ADDRESS });
    const env = mocknet.initialize();
    const submit = makeSubmit(mocknet.pool);

    const alice = mocknet.createPrivateTransfers(env.alice.address, env.alice.privateKey);
    const bob = mocknet.createPrivateTransfers(env.bob.address, env.bob.privateKey);
    const token = BigInt(env.ace);

    // --- Alice: register + self-channel, so she has somewhere to hold notes ---
    const aliceRegistry = await openSelfChannel(alice, env.alice.address, token, submit);

    // --- Shield: public ERC-20 into an encrypted note owned by Alice ---
    submit(
      await alice
        .build({ registry: aliceRegistry })
        .with(token)
        .deposit({ recipient: env.alice.address, amount: 500n })
        .execute()
    );

    const aliceNotes = await alice.discoverNotes([token]);
    const shielded = aliceNotes.notes.get(token) ?? [];
    expect(shielded.reduce((sum, note) => sum + BigInt(note.amount), 0n)).toBe(500n);

    // --- Bob registers, Alice opens a channel to him ---
    // Channels are directional: this one only lets Alice pay Bob.
    submit(await bob.build().register().execute());
    submit(await alice.build().setup(env.bob.address).execute());

    const registry = createEmptyRegistry();
    registry.channels.set(
      env.bob.address,
      (await alice.discoverChannels([env.bob.address])).channels.get(env.bob.address)!
    );
    submit(await alice.build({ registry }).with(token).setup(env.bob.address).execute());
    registry.channels.set(
      env.bob.address,
      (await alice.discoverChannels([env.bob.address])).channels.get(env.bob.address)!
    );

    // --- Private transfer: 300 to Bob, 200 change back to Alice ---
    submit(
      await alice
        .build()
        .with(token)
        .transfer({ recipient: env.bob.address, amount: 300n })
        .surplusTo(env.alice.address)
        .execute({
          autoDiscover: { channels: "refresh", notes: "refresh" },
          autoSetup: true,
          autoSelectNotes: "naive",
        })
    );

    // --- Bob finds his note without scanning: note_id derives from channel_key ---
    const bobNotes = await bob.discoverNotes([token]);
    const received = bobNotes.notes.get(token) ?? [];
    expect(received.reduce((sum, note) => sum + BigInt(note.amount), 0n)).toBe(300n);

    // --- Alice keeps the change; the spent note is nullified ---
    const aliceAfter = await alice.discoverNotes([token]);
    const remaining = aliceAfter.notes.get(token) ?? [];
    expect(remaining.reduce((sum, note) => sum + BigInt(note.amount), 0n)).toBe(200n);
  });

  it("collapses cold setup into one action set per operation", async () => {
    // Same outcome as the test above, but letting the builder do the setup. Each
    // `execute` here is one action set and would be one proof on-chain — so cold
    // shield-and-settle is 2 proofs (~58s at F7's ~29s), not five transactions.
    const mocknet = new Mocknet({ poolAddress: POOL_ADDRESS });
    const env = mocknet.initialize();
    const submit = makeSubmit(mocknet.pool);

    const alice = mocknet.createPrivateTransfers(env.alice.address, env.alice.privateKey);
    const bob = mocknet.createPrivateTransfers(env.bob.address, env.bob.privateKey);
    const token = BigInt(env.ace);

    const auto = {
      autoRegister: true,
      autoDiscover: { channels: "refresh", notes: "refresh" },
      autoSetup: true,
      autoSelectNotes: "naive",
    } as const;

    submit(await bob.build().register().execute());

    // Alice is cold here: unregistered, no channel, no subchannel, no notes.
    // SetViewingKey + OpenChannel + OpenSubchannel + Deposit + CreateEncNote, one set.
    submit(
      await alice
        .build()
        .with(token)
        .deposit({ recipient: env.alice.address, amount: 500n })
        .execute(auto)
    );

    // Channel and subchannel to Bob do not exist yet; still one set.
    submit(
      await alice
        .build()
        .with(token)
        .transfer({ recipient: env.bob.address, amount: 300n })
        .surplusTo(env.alice.address)
        .execute(auto)
    );

    const received = (await bob.discoverNotes([token])).notes.get(token) ?? [];
    expect(received.reduce((sum, note) => sum + BigInt(note.amount), 0n)).toBe(300n);
  });

  it("a note cannot be created without a subchannel", async () => {
    const mocknet = new Mocknet({ poolAddress: POOL_ADDRESS });
    const env = mocknet.initialize();
    const submit = makeSubmit(mocknet.pool);

    const alice = mocknet.createPrivateTransfers(env.alice.address, env.alice.privateKey);
    const bob = mocknet.createPrivateTransfers(env.bob.address, env.bob.privateKey);
    const token = BigInt(env.ace);

    await openSelfChannel(alice, env.alice.address, token, submit);
    submit(await bob.build().register().execute());

    // Channel to Bob is never opened, and autoSetup is off, so there is no
    // subchannel to write into. Mirrors SUBCHANNEL_NOT_FOUND on-chain.
    await expect(
      alice.build().with(token).transfer({ recipient: env.bob.address, amount: 10n }).execute({
        autoDiscover: { channels: "refresh", notes: "refresh" },
        autoSetup: false,
        autoSelectNotes: "naive",
      })
    ).rejects.toThrow();
  });
});
