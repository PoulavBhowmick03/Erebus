import { createHash } from "node:crypto";
import { execFileSync, spawn } from "node:child_process";
import { once } from "node:events";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { buildEddsa, poseidonContract } from "circomlibjs";
import { ContractFactory, JsonRpcProvider, getCreateAddress } from "ethers";
import * as snarkjs from "snarkjs";
import solc from "solc";

const dir = resolve(fileURLToPath(new URL("..", import.meta.url)));
const root = resolve(dir, "../..");
const build = resolve(dir, "build");
const snarkjsBin = resolve(dir, "node_modules/.bin/snarkjs");
const prime = BigInt("21888242871839275222246405745257275088548364400416034343698204186575808495617");

function run(binary, args) {
  execFileSync(binary, args, { cwd: dir, stdio: "inherit" });
}

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function testField(name) {
  return BigInt(`0x${sha256(`EREBUS_M5_TEST_ONLY_${name}`)}`) % prime;
}

function limbs(name) {
  const digest = sha256(`EREBUS_M5_TEST_ONLY_${name}`);
  return [BigInt(`0x${digest.slice(0, 32)}`), BigInt(`0x${digest.slice(32)}`)];
}

function serviceDigestFromM1Vector() {
  const fixture = JSON.parse(readFileSync(resolve(root, "sdk/core/tests/fixtures/agreement-v1-vectors.json"), "utf8"));
  const { service } = fixture.vectors[0].terms;
  const sized = (bytes) => {
    const length = Buffer.alloc(2);
    length.writeUInt16BE(bytes.length);
    return Buffer.concat([length, bytes]);
  };
  const quantity = Buffer.alloc(16);
  quantity.writeBigUInt64BE(BigInt(service.quantity), 8);
  const deadline = Buffer.alloc(8);
  deadline.writeBigUInt64BE(BigInt(service.deliveryDeadline));
  const encoded = Buffer.concat([
    sized(Buffer.from(service.resource, "utf8")), quantity,
    sized(Buffer.from(service.unit, "utf8")),
    sized(Buffer.from(service.accessRecipientHex, "hex")), deadline,
    sized(Buffer.from(service.fulfillmentMethod, "utf8")),
    Buffer.from(service.fulfillmentDigestHex, "hex"),
  ]);
  if (!fixture.vectors[0].expected.canonicalHex.endsWith(encoded.toString("hex"))) {
    throw new Error("M5 service encoding differs from the pinned Rust M1 vector");
  }
  const digest = sha256(encoded);
  return [BigInt(`0x${digest.slice(0, 32)}`), BigInt(`0x${digest.slice(32)}`)];
}

function stringify(value) {
  return JSON.parse(JSON.stringify(value, (_key, item) => typeof item === "bigint" ? item.toString() : item));
}

async function setup() {
  mkdirSync(build, { recursive: true });
  const tau = resolve(build, "pot16_final.ptau");
  if (!existsSync(tau)) {
    const initial = resolve(build, "pot16_0000.ptau");
    const contributed = resolve(build, "pot16_0001.ptau");
    run(snarkjsBin, ["powersoftau", "new", "bn128", "16", initial]);
    run(snarkjsBin, ["powersoftau", "contribute", initial, contributed,
      "--name=m5-local-only", "-e=erebus-m5-known-test-entropy"]);
    run(snarkjsBin, ["powersoftau", "prepare", "phase2", contributed, tau]);
  }
  const manifestPath = resolve(build, "artifact-manifest.json");
  const oldManifest = existsSync(manifestPath) ? JSON.parse(readFileSync(manifestPath, "utf8")) : {};
  const manifest = { version: "m5-local-prototype-v1", circuits: {} };
  for (const name of ["deposit", "transfer", "withdraw"]) {
    run("circom", [`${name}.circom`, "--r1cs", "--wasm", "-l", "node_modules", "-o", "build"]);
    const r1cs = resolve(build, `${name}.r1cs`);
    const zkey = resolve(build, `${name}.zkey`);
    const vkey = resolve(build, `${name}-verification-key.json`);
    const verifierSource = resolve(build, `${name}-verifier.sol`);
    const r1csHash = sha256(readFileSync(r1cs));
    const previous = oldManifest.circuits?.[name];
    const reuse = previous?.r1csSha256 === r1csHash
      && [zkey, vkey, verifierSource].every((file) => existsSync(file))
      && previous.zkeySha256 === sha256(readFileSync(zkey))
      && previous.verificationKeySha256 === sha256(readFileSync(vkey))
      && previous.verifierSourceSha256 === sha256(readFileSync(verifierSource));
    if (!reuse) {
      const initial = resolve(build, `${name}-initial.zkey`);
      run(snarkjsBin, ["groth16", "setup", r1cs, tau, initial]);
      run(snarkjsBin, ["zkey", "contribute", initial, zkey,
        "--name=m5-local-only", "-e=erebus-m5-known-test-phase2"]);
      run(snarkjsBin, ["zkey", "verify", r1cs, tau, zkey]);
      run(snarkjsBin, ["zkey", "export", "verificationkey", zkey, vkey]);
      run(snarkjsBin, ["zkey", "export", "solidityverifier", zkey, verifierSource]);
    }
    manifest.circuits[name] = {
      r1csSha256: r1csHash,
      zkeySha256: sha256(readFileSync(zkey)),
      verificationKeySha256: sha256(readFileSync(vkey)),
      verifierSourceSha256: sha256(readFileSync(verifierSource)),
    };
  }
  writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
  return manifest;
}

function compileContracts() {
  const sources = {};
  for (const name of ["deposit", "transfer", "withdraw"]) {
    sources[`${name}-verifier.sol`] = { content: readFileSync(resolve(build, `${name}-verifier.sol`), "utf8") };
  }
  for (const name of ["ErebusShieldedPool", "MockERC20"]) {
    sources[`${name}.sol`] = { content: readFileSync(resolve(root, `contracts/evm/src/${name}.sol`), "utf8") };
  }
  const output = JSON.parse(solc.compile(JSON.stringify({
    language: "Solidity", sources,
    settings: { optimizer: { enabled: true, runs: 200 }, outputSelection: { "*": { "*": ["abi", "evm.bytecode.object"] } } },
  })));
  if (output.errors?.some((entry) => entry.severity === "error")) {
    throw new Error(output.errors.map((entry) => entry.formattedMessage).join("\n"));
  }
  return output.contracts;
}

async function freePort() {
  const server = createServer();
  await new Promise((resolveReady, reject) => server.once("error", reject).listen(0, "127.0.0.1", resolveReady));
  const { port } = server.address();
  await new Promise((done) => server.close(done));
  return port;
}

async function prove(name, input, publicSignals) {
  const wasm = resolve(build, `${name}_js/${name}.wasm`);
  const zkey = resolve(build, `${name}.zkey`);
  const { proof, publicSignals: generated } = await snarkjs.groth16.fullProve(stringify(input), wasm, zkey);
  if (JSON.stringify(generated) !== JSON.stringify(publicSignals.map(String))) {
    throw new Error(`${name} generated different public inputs`);
  }
  const key = JSON.parse(readFileSync(resolve(build, `${name}-verification-key.json`), "utf8"));
  if (!await snarkjs.groth16.verify(key, generated, proof)) throw new Error(`${name} proof failed offchain`);
  return JSON.parse(`[${await snarkjs.groth16.exportSolidityCallData(proof, generated)}]`);
}

function tree(poseidon, leaves, selectedIndex) {
  const zero = [0n];
  for (let level = 0; level < 20; level += 1) zero.push(poseidon([zero[level], zero[level]]));
  let nodes = new Map(leaves.map((leaf, index) => [index, leaf]));
  let index = selectedIndex;
  const pathElements = [];
  const pathIndices = [];
  for (let level = 0; level < 20; level += 1) {
    pathElements.push(nodes.get(index ^ 1) ?? zero[level]);
    pathIndices.push(BigInt(index & 1));
    const parents = new Map();
    for (const position of nodes.keys()) {
      const parent = Math.floor(position / 2);
      if (parents.has(parent)) continue;
      const left = nodes.get(parent * 2) ?? zero[level];
      const right = nodes.get(parent * 2 + 1) ?? zero[level];
      parents.set(parent, poseidon([left, right]));
    }
    nodes = parents;
    index = Math.floor(index / 2);
  }
  return { root: nodes.get(0) ?? zero[20], pathElements, pathIndices };
}

async function reject(label, action) {
  try { await action(); } catch { return; }
  throw new Error(`${label} unexpectedly succeeded`);
}

async function main() {
  const artifacts = await setup();
  const contracts = compileContracts();
  const port = await freePort();
  const anvil = spawn("anvil", ["--port", String(port), "--chain-id", "10143", "--silent"], { stdio: "ignore" });
  const provider = new JsonRpcProvider(`http://127.0.0.1:${port}`, 10143, { staticNetwork: true });
  try {
    let ready = false;
    for (let attempt = 0; attempt < 40; attempt += 1) {
      try { await provider.getBlockNumber(); ready = true; break; } catch { await delay(100); }
    }
    if (!ready) throw new Error("Anvil did not start");
    const signer = await provider.getSigner(0);
    const deploy = async (artifact, ...args) => {
      const contract = await new ContractFactory(artifact.abi, artifact.evm.bytecode.object, signer).deploy(...args);
      await contract.waitForDeployment();
      return contract;
    };
    const token = await deploy(contracts["MockERC20.sol"].MockERC20, "Test Token", "TEST");
    const poseidonHash = await new ContractFactory(
      poseidonContract.generateABI(2), poseidonContract.createCode(2), signer,
    ).deploy();
    await poseidonHash.waitForDeployment();
    const verifier = {};
    for (const name of ["deposit", "transfer", "withdraw"]) {
      verifier[name] = await deploy(contracts[`${name}-verifier.sol`].Groth16Verifier);
    }
    const poolAddress = getCreateAddress({ from: await signer.getAddress(), nonce: await signer.getNonce() });
    const pool = await deploy(
      contracts["ErebusShieldedPool.sol"].ErebusShieldedPool,
      await token.getAddress(), await poseidonHash.getAddress(),
      await verifier.deposit.getAddress(), await verifier.transfer.getAddress(),
      await verifier.withdraw.getAddress(), 2n,
    );
    if ((await pool.getAddress()).toLowerCase() !== poolAddress.toLowerCase()) throw new Error("pool address prediction failed");

    const eddsa = await buildEddsa();
    const F = eddsa.babyJub.F;
    const poseidon = (items) => F.toObject(eddsa.poseidon(items));
    const buyerPrivate = Buffer.from(sha256("EREBUS_M5_BUYER_TEST_ONLY"), "hex");
    const sellerPrivate = Buffer.from(sha256("EREBUS_M5_SELLER_TEST_ONLY"), "hex");
    const buyer = eddsa.prv2pub(buyerPrivate).map((value) => F.toObject(value));
    const seller = eddsa.prv2pub(sellerPrivate).map((value) => F.toObject(value));
    const chainId = 10143n;
    const version = 2n;
    const contractAddress = BigInt(poolAddress);
    const asset = BigInt(await token.getAddress());
    const inputAmount = 150n;
    const amount = 70n;
    const changeAmount = inputAmount - amount;
    const inputSpendSecret = testField("buyer-spend");
    const inputSpendTag = poseidon([2006n, inputSpendSecret]);
    const inputSalt = testField("input-salt");
    const inputNote = poseidon([2007n, asset, inputAmount, ...buyer, inputSpendTag, inputSalt]);
    const depositInput = {
      chainId, contractAddress, verifierVersion: version, asset, amount: inputAmount,
      noteCommitment: inputNote, ownerAx: buyer[0], ownerAy: buyer[1],
      spendTag: inputSpendTag, salt: inputSalt,
      privateChainId: chainId, privateContractAddress: contractAddress, privateVerifierVersion: version,
    };
    const depositProof = await prove("deposit", depositInput,
      [chainId, contractAddress, version, asset, inputAmount, inputNote]);
    await (await token.mint(await signer.getAddress(), inputAmount)).wait();
    await (await token.approve(poolAddress, inputAmount)).wait();
    const wrongDeposit = structuredClone(depositProof);
    wrongDeposit[3][4] = "149";
    await reject("deposit amount mutation", async () => pool.deposit.staticCall(...wrongDeposit));
    await (await token.setFeeOnTransfer(true)).wait();
    await reject("fee-on-transfer deposit", async () => pool.deposit.staticCall(...depositProof));
    await (await token.setFeeOnTransfer(false)).wait();
    await (await pool.deposit(...depositProof)).wait();
    await reject("duplicate funded note", async () => pool.deposit.staticCall(...depositProof));
    if (!await pool.insertedCommitments(inputNote)) throw new Error("funded note was not registered");
    const firstTree = tree(poseidon, [inputNote], 0);
    if (await pool.currentRoot() !== firstTree.root) throw new Error("deposit tree root differs from Poseidon path");

    const domain = poseidon([chainId, contractAddress, version]);
    const dealId = BigInt(`0x${sha256("EREBUS_M5_DEAL_TEST_ONLY").slice(0, 32)}`);
    const [transcriptRootLo, transcriptRootHi] = limbs("transcript");
    const [serviceDigestLo, serviceDigestHi] = serviceDigestFromM1Vector();
    const [settlementNonceLo, settlementNonceHi] = limbs("nonce");
    const recipientSpendSecret = testField("seller-spend");
    const recipientSpendTag = poseidon([2006n, recipientSpendSecret]);
    const changeSpendTag = poseidon([2006n, testField("change-spend")]);
    const changeSalt = testField("change-salt");
    const blinding = testField("blinding");
    const expiry = 4102444800n;
    const paymentSalt = poseidon([2009n, domain, settlementNonceLo, settlementNonceHi]);
    const termsA = poseidon([2001n, domain, dealId, 1n, transcriptRootLo, transcriptRootHi]);
    const termsB = poseidon([serviceDigestLo, serviceDigestHi, settlementNonceLo, settlementNonceHi, asset, amount]);
    const termsC = poseidon([...buyer, ...seller, recipientSpendTag, paymentSalt]);
    const dealCommitment = poseidon([2002n, termsA, termsB, termsC, blinding, expiry]);
    const dealNullifier = poseidon([2003n, domain, ...buyer, settlementNonceLo, settlementNonceHi]);
    const inputNullifier = poseidon([2008n, inputSpendSecret, inputNote]);
    const paymentNote = poseidon([2007n, asset, amount, ...seller, recipientSpendTag, paymentSalt]);
    const changeNote = poseidon([2007n, asset, changeAmount, ...buyer, changeSpendTag, changeSalt]);
    const buyerMessage = eddsa.poseidon([2004n, domain, dealCommitment]);
    const sellerMessage = eddsa.poseidon([2005n, domain, dealCommitment]);
    const buyerSignature = eddsa.signPoseidon(buyerPrivate, buyerMessage);
    const sellerSignature = eddsa.signPoseidon(sellerPrivate, sellerMessage);
    const transferInput = {
      chainId, contractAddress, verifierVersion: version, asset, dealCommitment, dealNullifier,
      root: firstTree.root, inputNullifier, paymentCommitment: paymentNote,
      changeCommitment: changeNote, expiry,
      dealId, revision: 1n, transcriptRootLo, transcriptRootHi, serviceDigestLo, serviceDigestHi,
      settlementNonceLo, settlementNonceHi, amount, blinding,
      buyerAx: buyer[0], buyerAy: buyer[1], sellerAx: seller[0], sellerAy: seller[1], recipientSpendTag,
      buyerR8x: F.toObject(buyerSignature.R8[0]), buyerR8y: F.toObject(buyerSignature.R8[1]), buyerS: buyerSignature.S,
      sellerR8x: F.toObject(sellerSignature.R8[0]), sellerR8y: F.toObject(sellerSignature.R8[1]), sellerS: sellerSignature.S,
      inputAmount, inputSalt, inputSpendSecret,
      pathElements: firstTree.pathElements, pathIndices: firstTree.pathIndices,
      changeAmount, changeSpendTag, changeSalt,
    };
    const transferProof = await prove("transfer", transferInput, [
      chainId, contractAddress, version, asset, dealCommitment, dealNullifier, firstTree.root,
      inputNullifier, paymentNote, changeNote, expiry,
    ]);
    await reject("service mutation in witness", async () => prove("transfer", {
      ...transferInput, serviceDigestLo: serviceDigestLo + 1n,
    }, [
      chainId, contractAddress, version, asset, dealCommitment, dealNullifier, firstTree.root,
      inputNullifier, paymentNote, changeNote, expiry,
    ]));
    await reject("seller authorization mutation in witness", async () => prove("transfer", {
      ...transferInput, sellerS: sellerSignature.S + 1n,
    }, [
      chainId, contractAddress, version, asset, dealCommitment, dealNullifier, firstTree.root,
      inputNullifier, paymentNote, changeNote, expiry,
    ]));
    const redirectedTransfer = structuredClone(transferProof);
    redirectedTransfer[3][8] = "1";
    await reject("payment output mutation", async () => pool.transferPrivate.staticCall(...redirectedTransfer));
    const wrongTransferRoot = structuredClone(transferProof);
    wrongTransferRoot[3][6] = "1";
    await reject("transfer root mutation", async () => pool.transferPrivate.staticCall(...wrongTransferRoot));
    await (await pool.transferPrivate(...transferProof)).wait();
    if (!await pool.insertedCommitments(paymentNote) || !await pool.insertedCommitments(changeNote)) {
      throw new Error("transfer outputs were not registered");
    }
    if (!await pool.consumedDeals(dealNullifier) || !await pool.consumedNotes(inputNullifier)) {
      throw new Error("transfer did not consume deal and input note");
    }
    await reject("deal replay", async () => pool.transferPrivate.staticCall(...transferProof));
    const finalTree = tree(poseidon, [inputNote, paymentNote, changeNote], 1);
    if (await pool.currentRoot() !== finalTree.root) throw new Error("transfer tree root differs");

    // A recipient restarting with its own secret and the agreement can find this output.
    const recoveredTag = poseidon([2006n, recipientSpendSecret]);
    const recoveredSalt = poseidon([2009n, domain, settlementNonceLo, settlementNonceHi]);
    const recoveredNote = poseidon([2007n, asset, amount, ...seller, recoveredTag, recoveredSalt]);
    if (recoveredNote !== paymentNote) throw new Error("recipient could not rediscover its output");
    const paymentNullifier = poseidon([2008n, recipientSpendSecret, paymentNote]);
    const recipient = BigInt(await signer.getAddress());
    const withdrawInput = {
      chainId, contractAddress, verifierVersion: version, asset, root: finalTree.root,
      noteNullifier: paymentNullifier, recipient, amount,
      ownerAx: seller[0], ownerAy: seller[1], spendSecret: recipientSpendSecret, salt: paymentSalt,
      pathElements: finalTree.pathElements, pathIndices: finalTree.pathIndices,
      privateChainId: chainId, privateContractAddress: contractAddress,
      privateVerifierVersion: version, privateRecipient: recipient,
    };
    const withdrawalProof = await prove("withdraw", withdrawInput, [
      chainId, contractAddress, version, asset, finalTree.root, paymentNullifier, recipient, amount,
    ]);
    const changed = structuredClone(withdrawalProof);
    changed[3][6] = "1";
    await reject("withdraw redirect", async () => pool.withdraw.staticCall(...changed));
    const changedAmount = structuredClone(withdrawalProof);
    changedAmount[3][7] = "71";
    await reject("withdraw amount mutation", async () => pool.withdraw.staticCall(...changedAmount));
    const before = await token.balanceOf(await signer.getAddress());
    await (await pool.withdraw(...withdrawalProof)).wait();
    if (await token.balanceOf(await signer.getAddress()) !== before + amount) throw new Error("seller payout mismatch");
    if (await token.balanceOf(poolAddress) !== changeAmount) throw new Error("pool liability mismatch");
    await reject("withdraw replay", async () => pool.withdraw.staticCall(...withdrawalProof));
    writeFileSync(resolve(build, "run.json"), `${JSON.stringify({
      chainId: Number(chainId), pool: poolAddress, asset: await token.getAddress(),
      deposit: inputAmount.toString(), privatePayment: amount.toString(),
      finalVaultBalance: (await token.balanceOf(poolAddress)).toString(), artifacts,
      testOnly: true,
    }, null, 2)}\n`);
    console.log("M5 local prototype: funded deposit, private transfer, output recovery, withdrawal, replay checks passed");
  } finally {
    await provider.destroy();
    if (anvil.exitCode === null) {
      anvil.kill("SIGTERM");
      await Promise.race([once(anvil, "exit"), delay(1000)]);
    }
    await globalThis.curve_bn128?.terminate();
  }
}

main().catch((error) => { console.error(error); process.exitCode = 1; });
