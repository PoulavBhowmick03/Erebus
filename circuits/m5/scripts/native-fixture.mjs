// Known-entropy Anvil fixture only. Funds a note owned by the native suite-2 buyer.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { buildEddsa, buildPoseidon, poseidonContract } from "circomlibjs";
import { ContractFactory, JsonRpcProvider, keccak256 } from "ethers";
import * as snarkjs from "snarkjs";
import { compileContracts } from "./pool.mjs";

const build = fileURLToPath(new URL("../build/", import.meta.url));
const provider = new JsonRpcProvider(process.argv[2]);
try {
  if (!/^http:\/\/127\.0\.0\.1:\d+$/.test(process.argv[2]) || (await provider.getNetwork()).chainId !== 31337n) {
    throw new Error("native fixture requires a local Anvil chain 31337");
  }
  const signer = await provider.getSigner(0);
  const compiled = compileContracts();
  const deploy = async (source, name, args = []) => {
    const artifact = compiled[source][name];
    const contract = await new ContractFactory(artifact.abi, artifact.evm.bytecode.object, signer).deploy(...args);
    await contract.waitForDeployment();
    return contract;
  };
  const token = await deploy("MockERC20.sol", "MockERC20", ["Test", "TEST"]);
  const hash = await new ContractFactory(poseidonContract.generateABI(2), poseidonContract.createCode(2), signer).deploy();
  await hash.waitForDeployment();
  const deposit = await deploy("deposit-verifier.sol", "Groth16Verifier");
  const transfer = await deploy("transfer-verifier.sol", "Groth16Verifier");
  const withdraw = await deploy("withdraw-verifier.sol", "Groth16Verifier");
  const pool = await deploy("ErebusShieldedPool.sol", "ErebusShieldedPool", [
    await token.getAddress(), await hash.getAddress(), await deposit.getAddress(),
    await transfer.getAddress(), await withdraw.getAddress(), 2,
  ]);
  const deployment = await pool.deploymentTransaction().wait();
  const poseidon = await buildPoseidon();
  const eddsa = await buildEddsa();
  const owner = eddsa.prv2pub(Buffer.alloc(32, 61)).map((value) => BigInt(eddsa.F.toObject(value)));
  const spendSecret = BigInt(`0x${"06".repeat(32)}`);
  const salt = BigInt(`0x${"07".repeat(32)}`);
  const tag = BigInt(poseidon.F.toObject(poseidon([2006n, spendSecret])));
  const asset = BigInt(await token.getAddress());
  const note = BigInt(poseidon.F.toObject(poseidon([2007n, asset, 150n, ...owner, tag, salt])));
  const input = {
    chainId: "31337", contractAddress: BigInt(await pool.getAddress()).toString(), verifierVersion: "2",
    asset: asset.toString(), amount: "150", noteCommitment: note.toString(), ownerAx: owner[0].toString(), ownerAy: owner[1].toString(),
    spendTag: tag.toString(), salt: salt.toString(), privateChainId: "31337",
    privateContractAddress: BigInt(await pool.getAddress()).toString(), privateVerifierVersion: "2",
  };
  const proved = await snarkjs.groth16.fullProve(input, resolve(build, "deposit_js/deposit.wasm"), resolve(build, "deposit.zkey"));
  const key = JSON.parse(readFileSync(resolve(build, "deposit-verification-key.json"), "utf8"));
  if (!await snarkjs.groth16.verify(key, proved.publicSignals, proved.proof)) throw new Error("invalid fixture deposit proof");
  const tuple = JSON.parse(`[${await snarkjs.groth16.exportSolidityCallData(proved.proof, proved.publicSignals)}]`);
  await (await token.mint(await signer.getAddress(), 150)).wait();
  await (await token.approve(await pool.getAddress(), 150)).wait();
  await (await pool.deposit(...tuple)).wait();
  const runtime = async (contract) => keccak256(await provider.getCode(await contract.getAddress()));
  console.log(JSON.stringify({
    pool: (await pool.getAddress()).toLowerCase(), token: (await token.getAddress()).toLowerCase(),
    first_block: deployment.blockNumber, first_hash: deployment.blockHash,
    runtime_keccak256: await runtime(pool), poseidon_keccak256: await runtime(hash),
    deposit_verifier_keccak256: await runtime(deposit), transfer_verifier_keccak256: await runtime(transfer),
    withdraw_verifier_keccak256: await runtime(withdraw),
  }));
} finally {
  provider.destroy();
  await globalThis.curve_bn128?.terminate();
}
