// Generates checkpoint vectors from the real ErebusShieldedPool Solidity on local Anvil.
//
//   forge build --root contracts/evm
//   node circuits/m5/scripts/generate-checkpoint-vectors.mjs \
//     > sdk/shielded/tests/fixtures/checkpoint-vectors.json
//
// The pool is the repository source with the production two-input Poseidon bytecode (circomlibjs
// createCode(2), checked against the deployment record's runtime pin). Verifiers are a permissive
// test-only stub: they do not affect tree state. Commitments are deterministic field elements.
// Every snapshot is read with eth_call at the exact block of its last insertion.
// The `near_full` case plants a frontier for 2^20-3 identical leaves with anvil_setStorageAt
// (the only way to reach the capacity boundary locally); its later insertions are real Solidity.
import { spawn } from "node:child_process";
import { readFileSync } from "node:fs";
import { createServer } from "node:net";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { ContractFactory, JsonRpcProvider, NonceManager, Wallet, keccak256, AbiCoder, toBeHex, zeroPadValue } from "ethers";
import { buildPoseidon, poseidonContract } from "circomlibjs";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "../../..");
const OUT = join(ROOT, "contracts/evm/out");
const FIELD = 21888242871839275222246405745257275088548364400416034343698204186575808495617n;
const DEPTH = 20;
const PIN = JSON.parse(readFileSync(join(ROOT, "contracts/evm/deployments/monad-testnet-shielded-v2.json"), "utf8"));
const COUNTS = [0, 1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129, 255, 256, 257,
  511, 512, 513, 1023, 1024, 1025, 1500];
const TOTAL = COUNTS.at(-1);
// Anvil's default first account; a published test key, never a funded real-network key.
const ANVIL_KEY = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

const artifact = (file, name) => JSON.parse(readFileSync(join(OUT, file, `${name}.json`), "utf8"));
const hex = (value) => "0x" + BigInt(value).toString(16).padStart(64, "0");
const commitment = (i) => {
  const value = BigInt(keccak256(AbiCoder.defaultAbiCoder().encode(["string", "uint256"], ["erebus-ws1-vector", i]))) % FIELD;
  return value === 0n ? 1n : value;
};

async function freePort() {
  return new Promise((resolve) => {
    const server = createServer();
    server.listen(0, "127.0.0.1", () => { const { port } = server.address(); server.close(() => resolve(port)); });
  });
}

function findPin(object) {
  if (object && typeof object === "object") {
    for (const [key, value] of Object.entries(object)) {
      if (key === "poseidon" && typeof value === "string" && /^[0-9a-f]{64}$/.test(value)) return value;
      const found = findPin(value);
      if (found) return found;
    }
  }
  return null;
}

async function main() {
  const port = await freePort();
  const anvil = spawn("anvil", ["--host", "127.0.0.1", "--port", String(port), "--silent"], { stdio: "ignore" });
  try {
    const provider = new JsonRpcProvider(`http://127.0.0.1:${port}`, 31337, { staticNetwork: true, pollingInterval: 50 });
    for (let attempt = 0; ; attempt += 1) {
      try { await provider.getBlockNumber(); break; } catch (error) {
        if (attempt > 100) throw error;
        await new Promise((r) => setTimeout(r, 100));
      }
    }
    const signer = new NonceManager(new Wallet(ANVIL_KEY, provider));
    const signerAddress = await signer.getAddress();
    const deploy = async (abi, bytecode, args = []) => {
      const contract = await new ContractFactory(abi, bytecode, signer).deploy(...args);
      await contract.waitForDeployment();
      return contract;
    };
    const poseidonHash = await deploy(poseidonContract.generateABI(2), poseidonContract.createCode(2));
    const runtime = await provider.getCode(await poseidonHash.getAddress());
    const runtimeKeccak = keccak256(runtime).slice(2);
    const pinned = findPin(PIN);
    if (runtimeKeccak !== pinned) throw new Error(`poseidon runtime ${runtimeKeccak} differs from pin ${pinned}`);

    const token = artifact("MockERC20.sol", "MockERC20");
    const verifier = artifact("ErebusShieldedPool.t.sol", "StateOnlyVerifier");
    const poolArtifact = artifact("ErebusShieldedPool.sol", "ErebusShieldedPool");
    const asset = await deploy(token.abi, token.bytecode.object, ["Vector", "VEC"]);
    const stub = await deploy(verifier.abi, verifier.bytecode.object);
    const deployPool = () => deploy(poolArtifact.abi, poolArtifact.bytecode.object, [
      asset.target, poseidonHash.target, stub.target, stub.target, stub.target, 2]);
    const pool = await deployPool();
    await (await asset.mint(signerAddress, 10n ** 30n)).wait();
    await (await asset.approve(pool.target, 2n ** 255n)).wait();

    const chainId = (await provider.getNetwork()).chainId;
    const depositInput = (target, leaf) => [chainId, BigInt(target), 2n, BigInt(asset.target), 1n, leaf];
    const proof = [[0n, 0n], [[0n, 0n], [0n, 0n]], [0n, 0n]];
    const state = async (contract, blockTag) => {
      const read = (name, ...args) => contract[name](...args, { blockTag });
      const frontier = [];
      for (let level = 0; level < DEPTH; level += 1) frontier.push(hex(await read("filledSubtrees", level)));
      const root = await read("currentRoot");
      return {
        leaf_count: Number(await read("nextLeafIndex")),
        root: hex(root),
        filled_subtrees: frontier,
        root_known: await read("knownRoots", root),
      };
    };

    const zeroHashes = [];
    for (let level = 0; level < DEPTH; level += 1) zeroHashes.push(hex(await pool.zeroHashes(level)));

    const leaves = [];
    const snapshots = [];
    let lastBlock = await provider.getBlockNumber();
    snapshots.push({ ...(await state(pool, lastBlock)) });
    for (let i = 0; i < TOTAL; i += 1) {
      const leaf = commitment(i);
      const receipt = await (await pool.deposit(...proof, depositInput(pool.target, leaf))).wait();
      const log = receipt.logs.map((entry) => { try { return pool.interface.parseLog(entry); } catch { return null; } })
        .find((parsed) => parsed && parsed.name === "NoteInserted");
      if (!log || Number(log.args.index) !== i || log.args.commitment !== leaf) throw new Error(`insertion ${i} mismatch`);
      leaves.push({ index: i, commitment: hex(leaf), root: hex(log.args.root) });
      lastBlock = receipt.blockNumber;
      if (COUNTS.includes(i + 1)) snapshots.push(await state(pool, lastBlock));
    }
    for (const snapshot of snapshots) {
      if (snapshot.leaf_count !== 0 && snapshot.root !== leaves[snapshot.leaf_count - 1].root) {
        throw new Error(`snapshot ${snapshot.leaf_count} root differs from its last event`);
      }
    }

    // Near-full boundary: plant a consistent frontier for N identical leaves, then insert for real.
    const poseidon = await buildPoseidon();
    const H = (left, right) => BigInt(poseidon.F.toString(poseidon([left, right])));
    const near = await deployPool();
    await (await asset.approve(near.target, 2n ** 255n)).wait();
    const N = (1 << DEPTH) - 3;
    const fill = commitment(10_000_000);
    const zero = zeroHashes.map(BigInt);
    const full = [fill];
    for (let level = 0; level < DEPTH; level += 1) full.push(H(full[level], full[level]));
    const memo = new Map();
    const partial = (level, count) => {
      if (count === 0) return zero[level] ?? H(zero[level - 1], zero[level - 1]);
      if (count === 2 ** level) return full[level];
      const key = `${level}:${count}`;
      if (memo.has(key)) return memo.get(key);
      const half = 2 ** (level - 1);
      const left = Math.min(count, half);
      const value = H(partial(level - 1, left), partial(level - 1, count - left));
      memo.set(key, value);
      return value;
    };
    const last = N - 1;
    const frontier = [];
    for (let level = 0; level < DEPTH; level += 1) {
      if (((last >> level) & 1) === 1) frontier.push(full[level]);
      else frontier.push(partial(level, (last & ((1 << level) - 1)) + 1));
    }
    const plantedRoot = partial(DEPTH, N);
    const setSlot = (slot, value) => provider.send("anvil_setStorageAt", [near.target, toBeHex(slot, 32), hex(value)]);
    await setSlot(0n, BigInt(N));
    await setSlot(1n, plantedRoot);
    for (let level = 0; level < DEPTH; level += 1) await setSlot(22n + BigInt(level), frontier[level]);
    const knownSlot = BigInt(keccak256(AbiCoder.defaultAbiCoder().encode(["uint256", "uint256"], [plantedRoot, 42n])));
    await setSlot(knownSlot, 1n);
    await provider.send("evm_mine", []);
    const plantedBlock = await provider.getBlockNumber();
    const nearBase = await state(near, plantedBlock);
    if (nearBase.root !== hex(plantedRoot) || nearBase.leaf_count !== N) throw new Error("planted state differs");
    const nearLeaves = [];
    for (let i = 0; i < 3; i += 1) {
      const leaf = commitment(20_000_000 + i);
      const receipt = await (await near.deposit(...proof, depositInput(near.target, leaf))).wait();
      const log = receipt.logs.map((entry) => { try { return near.interface.parseLog(entry); } catch { return null; } })
        .find((parsed) => parsed && parsed.name === "NoteInserted");
      nearLeaves.push({ index: Number(log.args.index), commitment: hex(leaf), root: hex(log.args.root) });
    }
    let fullRejected = false;
    try {
      await near.deposit.staticCall(...proof, depositInput(near.target, commitment(30_000_000)));
    } catch (error) { fullRejected = String(error.message).includes("tree full"); }
    if (!fullRejected) throw new Error("full tree accepted a deposit");

    process.stdout.write(JSON.stringify({
      description: "ErebusShieldedPool frontier vectors generated from the repository Solidity on local Anvil",
      generator: "node circuits/m5/scripts/generate-checkpoint-vectors.mjs",
      poseidon_runtime_keccak256: runtimeKeccak,
      tree_depth: DEPTH,
      zero_hashes: zeroHashes,
      leaves,
      snapshots,
      near_full: {
        note: "frontier for 2^20-3 identical leaves planted via anvil_setStorageAt; the three insertions are real Solidity",
        fill_leaf: hex(fill),
        base: nearBase,
        insertions: nearLeaves,
        full_tree_rejects_deposit: fullRejected,
      },
    }, null, 1) + "\n");
  } finally {
    anvil.kill();
  }
}

main().catch((error) => { console.error(error); process.exit(1); });
