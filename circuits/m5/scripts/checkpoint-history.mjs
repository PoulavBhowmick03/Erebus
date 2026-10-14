// Known-entropy Anvil fixture only: creates pre-existing pool history before a fresh wallet joins.
//
//   node checkpoint-history.mjs <anvil-url> <pool> <token> <deposits> <empty-blocks> [erc20|wmon]
//
// Each deposit is a real Groth16 proof for a random note owned by a throwaway key, submitted by Anvil
// account 1 (a published test account). Then <empty-blocks> blocks are mined. Prints a JSON summary.
import { randomBytes } from "node:crypto";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { buildEddsa, buildPoseidon } from "circomlibjs";
import { Contract, JsonRpcProvider } from "ethers";
import * as snarkjs from "snarkjs";
import { artifactBuild as build } from "./pool.mjs";

const [url, poolAddress, tokenAddress, countText, emptyText, mode = "erc20"] = process.argv.slice(2);
const provider = new JsonRpcProvider(url);
const FIELD = 21888242871839275222246405745257275088548364400416034343698204186575808495617n;
try {
  if (!/^http:\/\/127\.0\.0\.1:\d+$/.test(url) || !["erc20", "wmon"].includes(mode)) {
    throw new Error("history fixture requires local Anvil and an erc20 or wmon asset");
  }
  const count = Number(countText);
  const empty = Number(emptyText);
  const signer = await provider.getSigner(1);
  const pool = new Contract(poolAddress, [
    "function deposit(uint256[2],uint256[2][2],uint256[2],uint256[6])",
    "function nextLeafIndex() view returns (uint256)",
  ], signer);
  const token = new Contract(tokenAddress, [
    "function mint(address,uint256)", "function approve(address,uint256) returns (bool)",
    "function deposit() payable",
  ], signer);
  const poseidon = await buildPoseidon();
  const eddsa = await buildEddsa();
  const H = (items) => BigInt(poseidon.F.toObject(poseidon(items)));
  const chainId = (await provider.getNetwork()).chainId;
  const domain = H([chainId, BigInt(poolAddress), 2n]);
  const key = JSON.parse(readFileSync(resolve(build, "deposit-verification-key.json"), "utf8"));
  const field = () => BigInt(`0x${randomBytes(32).toString("hex")}`) % FIELD;
  const total = BigInt(count) * 11n;
  if (mode === "wmon") await (await token.deposit({ value: total })).wait();
  else await (await token.mint(await signer.getAddress(), total)).wait();
  await (await token.approve(poolAddress, total)).wait();
  for (let i = 0; i < count; i += 1) {
    const owner = eddsa.prv2pub(randomBytes(32)).map((value) => BigInt(eddsa.F.toObject(value)));
    const tag = H([2006n, field()]);
    const salt = field();
    const amount = 11n;
    const note = H([2010n, domain, BigInt(tokenAddress), amount, ...owner, tag, salt]);
    const input = {
      chainId: chainId.toString(), contractAddress: BigInt(poolAddress).toString(), verifierVersion: "2",
      asset: BigInt(tokenAddress).toString(), amount: amount.toString(), noteCommitment: note.toString(),
      ownerAx: owner[0].toString(), ownerAy: owner[1].toString(), spendTag: tag.toString(), salt: salt.toString(),
      privateChainId: chainId.toString(), privateContractAddress: BigInt(poolAddress).toString(),
      privateVerifierVersion: "2",
    };
    const proved = await snarkjs.groth16.fullProve(input, resolve(build, "deposit_js/deposit.wasm"), resolve(build, "deposit.zkey"));
    if (!await snarkjs.groth16.verify(key, proved.publicSignals, proved.proof)) throw new Error("invalid history proof");
    const tuple = JSON.parse(`[${await snarkjs.groth16.exportSolidityCallData(proved.proof, proved.publicSignals)}]`);
    await (await pool.deposit(...tuple)).wait();
  }
  if (empty > 0) await provider.send("anvil_mine", [`0x${empty.toString(16)}`]);
  console.log(JSON.stringify({
    history_deposits: count,
    empty_blocks: empty,
    leaf_count: Number(await pool.nextLeafIndex()),
    head_block: await provider.getBlockNumber(),
  }));
} finally {
  provider.destroy();
  await globalThis.curve_bn128?.terminate();
}
