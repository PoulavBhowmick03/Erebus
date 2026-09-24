import { createHash } from "node:crypto";
import { spawn, execFileSync } from "node:child_process";
import { once } from "node:events";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { buildEddsa } from "circomlibjs";
import { ContractFactory, JsonRpcProvider, getCreateAddress } from "ethers";
import * as snarkjs from "snarkjs";
import solc from "solc";

const workDir = resolve(fileURLToPath(new URL("..", import.meta.url)));
const buildDir = resolve(workDir, "build");
const wasmPath = resolve(buildDir, "transfer_js/transfer.wasm");
const r1csPath = resolve(buildDir, "transfer.r1cs");
const zkeyPath = resolve(buildDir, "transfer_final.zkey");
const snarkjsBin = resolve(workDir, "node_modules/.bin/snarkjs");
const prime = BigInt("21888242871839275222246405745257275088548364400416034343698204186575808495617");

function run(binary, args) {
  const started = performance.now();
  execFileSync(binary, args, { cwd: workDir, stdio: "inherit" });
  return Math.round(performance.now() - started);
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function testField(label) {
  return BigInt(`0x${sha256(`EREBUS_M4_TEST_ONLY_${label}`)}`) % prime;
}

function limbPair(label) {
  const digest = sha256(`EREBUS_M4_TEST_ONLY_${label}`);
  return [BigInt(`0x${digest.slice(0, 32)}`), BigInt(`0x${digest.slice(32)}`)];
}

function serviceDigestFromM1Vector() {
  const fixture = JSON.parse(readFileSync(resolve(workDir, "../../sdk/core/tests/fixtures/agreement-v1-vectors.json"), "utf8"));
  const { service } = fixture.vectors[0].terms;
  const textField = (value) => {
    const bytes = Buffer.from(value, "utf8");
    const length = Buffer.alloc(2);
    length.writeUInt16BE(bytes.length);
    return Buffer.concat([length, bytes]);
  };
  const bytesField = (value) => {
    const bytes = Buffer.from(value, "hex");
    const length = Buffer.alloc(2);
    length.writeUInt16BE(bytes.length);
    return Buffer.concat([length, bytes]);
  };
  const quantity = Buffer.alloc(16);
  quantity.writeBigUInt64BE(BigInt(service.quantity), 8);
  const deadline = Buffer.alloc(8);
  deadline.writeBigUInt64BE(BigInt(service.deliveryDeadline));
  const encoded = Buffer.concat([
    textField(service.resource), quantity, textField(service.unit),
    bytesField(service.accessRecipientHex), deadline,
    textField(service.fulfillmentMethod), Buffer.from(service.fulfillmentDigestHex, "hex"),
  ]);
  if (!fixture.vectors[0].expected.canonicalHex.endsWith(encoded.toString("hex"))) {
    throw new Error("M4 service encoding differs from the pinned Rust M1 vector");
  }
  const digest = sha256(encoded);
  return [BigInt(`0x${digest.slice(0, 32)}`), BigInt(`0x${digest.slice(32)}`)];
}

function asDecimalStrings(value) {
  return JSON.parse(JSON.stringify(value, (_key, item) => typeof item === "bigint" ? item.toString() : item));
}

async function setupIfNeeded() {
  mkdirSync(buildDir, { recursive: true });
  run("circom", ["transfer.circom", "--r1cs", "--wasm", "--sym", "-l", "node_modules", "-o", "build"]);
  const digest = sha256(readFileSync(r1csPath));
  const manifestPath = resolve(buildDir, "artifact-manifest.json");
  const verificationKeyPath = resolve(buildDir, "verification_key.json");
  const verifierSourcePath = resolve(buildDir, "Verifier.sol");
  if (existsSync(manifestPath)) {
    const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
    if (manifest.r1csSha256 === digest && existsSync(zkeyPath)
        && existsSync(verificationKeyPath) && existsSync(verifierSourcePath)) {
      for (const [path, expected] of [
        [zkeyPath, manifest.zkeySha256],
        [verificationKeyPath, manifest.verificationKeySha256],
        [verifierSourcePath, manifest.verifierSourceSha256],
      ]) {
        if (sha256(readFileSync(path)) !== expected) throw new Error(`cached proof artifact changed: ${path}`);
      }
      return manifest;
    }
  }
  const tau0 = resolve(buildDir, "pot15_0000.ptau");
  const tau1 = resolve(buildDir, "pot15_0001.ptau");
  const tauFinal = resolve(buildDir, "pot15_final.ptau");
  if (!existsSync(tauFinal)) {
    run(snarkjsBin, ["powersoftau", "new", "bn128", "15", tau0]);
    run(snarkjsBin, ["powersoftau", "contribute", tau0, tau1,
      "--name=erebus-m4-local-only", "-e=erebus-m4-local-only-entropy"]);
    run(snarkjsBin, ["powersoftau", "prepare", "phase2", tau1, tauFinal]);
  }
  const initialZkey = resolve(buildDir, "transfer_0000.zkey");
  run(snarkjsBin, ["groth16", "setup", r1csPath, tauFinal, initialZkey]);
  run(snarkjsBin, ["zkey", "contribute", initialZkey, zkeyPath,
    "--name=erebus-m4-local-only", "-e=erebus-m4-local-only-phase2"]);
  run(snarkjsBin, ["zkey", "verify", r1csPath, tauFinal, zkeyPath]);
  run(snarkjsBin, ["zkey", "export", "verificationkey", zkeyPath, verificationKeyPath]);
  run(snarkjsBin, ["zkey", "export", "solidityverifier", zkeyPath, verifierSourcePath]);
  const manifest = {
    version: "m4-prototype-v1",
    circomCommit: "ad44e915a12bb047b05745c2884aad9cc8326bc6",
    r1csSha256: digest,
    zkeySha256: sha256(readFileSync(zkeyPath)),
    verificationKeySha256: sha256(readFileSync(verificationKeyPath)),
    verifierSourceSha256: sha256(readFileSync(verifierSourcePath)),
  };
  writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
  return manifest;
}

function makeInput(eddsa, contractAddress, expiry) {
  const F = eddsa.babyJub.F;
  const poseidon = (items) => F.toObject(eddsa.poseidon(items));
  const buyerPrivate = Buffer.from(sha256("EREBUS_M4_BUYER_TEST_ONLY"), "hex");
  const sellerPrivate = Buffer.from(sha256("EREBUS_M4_SELLER_TEST_ONLY"), "hex");
  const buyer = eddsa.prv2pub(buyerPrivate).map((coord) => F.toObject(coord));
  const seller = eddsa.prv2pub(sellerPrivate).map((coord) => F.toObject(coord));
  const chainId = 10143n;
  const verifierVersion = 2n;
  const domain = poseidon([chainId, contractAddress, verifierVersion]);
  const dealId = BigInt(`0x${sha256("EREBUS_M4_DEAL_TEST_ONLY").slice(0, 32)}`);
  const [transcriptRootLo, transcriptRootHi] = limbPair("transcript");
  const [serviceDigestLo, serviceDigestHi] = serviceDigestFromM1Vector();
  const [settlementNonceLo, settlementNonceHi] = limbPair("nonce");
  const asset = BigInt("0x1234567890123456789012345678901234567890");
  const amount = 70n;
  const inputAmount = 150n;
  const changeAmount = inputAmount - amount;
  const inputSpendSecret = testField("input-spend");
  const inputSpendTag = poseidon([2006n, inputSpendSecret]);
  const recipientSpendSecret = testField("seller-spend");
  const recipientSpendTag = poseidon([2006n, recipientSpendSecret]);
  const changeSpendTag = poseidon([2006n, testField("change-spend")]);
  const inputSalt = testField("input-salt");
  const paymentSalt = poseidon([2009n, domain, settlementNonceLo, settlementNonceHi]);
  const changeSalt = testField("change-salt");
  const blinding = testField("blinding");
  const inputNote = poseidon([2007n, asset, inputAmount, ...buyer, inputSpendTag, inputSalt]);
  const pathElements = [0, 1, 2, 3].map((index) => testField(`sibling-${index}`));
  const pathIndices = [0n, 1n, 0n, 1n];
  let root = inputNote;
  for (let index = 0; index < 4; index += 1) {
    root = pathIndices[index] === 0n
      ? poseidon([root, pathElements[index]])
      : poseidon([pathElements[index], root]);
  }

  const termsA = poseidon([2001n, domain, dealId, 1n, transcriptRootLo, transcriptRootHi]);
  const termsB = poseidon([serviceDigestLo, serviceDigestHi, settlementNonceLo, settlementNonceHi, asset, amount]);
  const termsC = poseidon([...buyer, ...seller, recipientSpendTag, paymentSalt]);
  const dealCommitment = poseidon([2002n, termsA, termsB, termsC, blinding, expiry]);
  const dealNullifier = poseidon([2003n, domain, ...buyer, settlementNonceLo, settlementNonceHi]);
  const inputNullifier = poseidon([2008n, inputSpendSecret, inputNote]);
  const paymentCommitment = poseidon([2007n, asset, amount, ...seller, recipientSpendTag, paymentSalt]);
  const changeCommitment = poseidon([2007n, asset, changeAmount, ...buyer, changeSpendTag, changeSalt]);

  const buyerMessage = eddsa.poseidon([2004n, domain, dealCommitment]);
  const sellerMessage = eddsa.poseidon([2005n, domain, dealCommitment]);
  const buyerSignature = eddsa.signPoseidon(buyerPrivate, buyerMessage);
  const sellerSignature = eddsa.signPoseidon(sellerPrivate, sellerMessage);
  if (!eddsa.verifyPoseidon(buyerMessage, buyerSignature, eddsa.prv2pub(buyerPrivate))
      || !eddsa.verifyPoseidon(sellerMessage, sellerSignature, eddsa.prv2pub(sellerPrivate))) {
    throw new Error("test authorizations did not verify");
  }

  const input = {
    chainId, contractAddress, verifierVersion, asset, dealCommitment, dealNullifier, root,
    inputNullifier, paymentCommitment, changeCommitment, expiry,
    dealId, revision: 1n, transcriptRootLo, transcriptRootHi,
    serviceDigestLo, serviceDigestHi, settlementNonceLo, settlementNonceHi,
    asset, amount, blinding, buyerAx: buyer[0], buyerAy: buyer[1],
    sellerAx: seller[0], sellerAy: seller[1], recipientSpendTag,
    buyerR8x: F.toObject(buyerSignature.R8[0]),
    buyerR8y: F.toObject(buyerSignature.R8[1]), buyerS: buyerSignature.S,
    sellerR8x: F.toObject(sellerSignature.R8[0]),
    sellerR8y: F.toObject(sellerSignature.R8[1]), sellerS: sellerSignature.S,
    inputAmount, inputSalt, inputSpendSecret, pathElements, pathIndices,
    changeAmount, changeSpendTag, changeSalt,
  };
  return {
    input: asDecimalStrings(input),
    publicSignals: [chainId, contractAddress, verifierVersion, asset, dealCommitment,
      dealNullifier, root, inputNullifier, paymentCommitment, changeCommitment, expiry].map(String),
    sellerRecovery: asDecimalStrings({
      asset, amount, sellerAx: seller[0], sellerAy: seller[1],
      spendSecret: recipientSpendSecret, domain, settlementNonceLo, settlementNonceHi,
    }),
  };
}

function compileContracts() {
  const sources = {
    "Verifier.sol": { content: readFileSync(resolve(buildDir, "Verifier.sol"), "utf8") },
    "PrototypeSettlement.sol": { content: readFileSync(resolve(workDir, "PrototypeSettlement.sol"), "utf8") },
  };
  const output = JSON.parse(solc.compile(JSON.stringify({
    language: "Solidity",
    sources,
    settings: { optimizer: { enabled: true, runs: 200 }, outputSelection: { "*": { "*": ["abi", "evm.bytecode.object"] } } },
  })));
  if (output.errors?.some((item) => item.severity === "error")) {
    throw new Error(output.errors.map((item) => item.formattedMessage).join("\n"));
  }
  return {
    verifier: output.contracts["Verifier.sol"].Groth16Verifier,
    settlement: output.contracts["PrototypeSettlement.sol"].PrototypeSettlement,
  };
}

async function freePort() {
  const server = createServer();
  await new Promise((res, rej) => server.once("error", rej).listen(0, "127.0.0.1", res));
  const { port } = server.address();
  await new Promise((res) => server.close(res));
  return port;
}

async function expectRejected(label, operation) {
  try {
    await operation();
  } catch {
    return;
  }
  throw new Error(`${label} unexpectedly succeeded`);
}

async function main() {
  const artifactManifest = await setupIfNeeded();
  const r1csInfo = execFileSync(snarkjsBin, ["r1cs", "info", r1csPath], { cwd: workDir, encoding: "utf8" });
  const constraints = Number(r1csInfo.match(/# of Constraints:\s*(\d+)/)?.[1]);
  if (!Number.isSafeInteger(constraints)) throw new Error("could not read constraint count");
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
    const verifierFactory = new ContractFactory(contracts.verifier.abi, contracts.verifier.evm.bytecode.object, signer);
    const verifier = await verifierFactory.deploy();
    await verifier.waitForDeployment();
    const predictedSettlementAddress = getCreateAddress({ from: await signer.getAddress(), nonce: await signer.getNonce() });
    const expiry = 4102444800n;
    const eddsa = await buildEddsa();
    const { input, publicSignals: expectedSignals, sellerRecovery } = makeInput(
      eddsa, BigInt(predictedSettlementAddress), expiry,
    );
    const pinnedSignals = JSON.parse(readFileSync(resolve(workDir, "fixtures/public.json"), "utf8"));
    if (JSON.stringify(expectedSignals) !== JSON.stringify(pinnedSignals)) {
      throw new Error(`computed public inputs differ from the pinned M4 vector: ${JSON.stringify(expectedSignals)}`);
    }
    const settlementFactory = new ContractFactory(contracts.settlement.abi, contracts.settlement.evm.bytecode.object, signer);
    const settlement = await settlementFactory.deploy(await verifier.getAddress(), 2n, input.asset, input.root);
    await settlement.waitForDeployment();
    if ((await settlement.getAddress()).toLowerCase() !== predictedSettlementAddress.toLowerCase()) {
      throw new Error("predicted settlement address did not match deployment");
    }

    const peak = { bytes: process.memoryUsage().rss };
    const sampler = setInterval(() => { peak.bytes = Math.max(peak.bytes, process.memoryUsage().rss); }, 10);
    let proof, publicSignals;
    const provingStart = performance.now();
    try {
      ({ proof, publicSignals } = await snarkjs.groth16.fullProve(input, wasmPath, zkeyPath));
    } finally {
      clearInterval(sampler);
    }
    const provingMs = Math.round(performance.now() - provingStart);
    if (JSON.stringify(publicSignals) !== JSON.stringify(expectedSignals)) {
      throw new Error(`prover public inputs differ: actual=${JSON.stringify(publicSignals)} expected=${JSON.stringify(expectedSignals)}`);
    }
    const verificationKey = JSON.parse(readFileSync(resolve(buildDir, "verification_key.json"), "utf8"));
    if (!await snarkjs.groth16.verify(verificationKey, publicSignals, proof)) {
      throw new Error("offchain proof verification failed");
    }

    const calldata = JSON.parse(`[${await snarkjs.groth16.exportSolidityCallData(proof, publicSignals)}]`);
    if (!await verifier.verifyProof(...calldata)) throw new Error("Solidity verifier rejected the proof");
    const naiveVerifierGasEstimate = await verifier.verifyProof.estimateGas(...calldata);
    const verifierCallReceipt = await (await signer.sendTransaction({
      to: await verifier.getAddress(),
      data: verifier.interface.encodeFunctionData("verifyProof", calldata),
      gasLimit: 5_000_000n,
    })).wait();
    const verifierTrace = await provider.send("debug_traceTransaction", [
      verifierCallReceipt.hash, { tracer: "callTracer" },
    ]);
    const verifierPrecompiles = [];
    function collectCalls(call) {
      for (const child of call.calls ?? []) {
        if (child.to) verifierPrecompiles.push(BigInt(child.to).toString());
        collectCalls(child);
      }
    }
    collectCalls(verifierTrace);
    if (!verifierPrecompiles.includes("7") || !verifierPrecompiles.includes("8")) {
      throw new Error(`verifier trace lacked BN254 calls: ${JSON.stringify(verifierPrecompiles)}`);
    }
    const settlementGasEstimate = await settlement.settle.estimateGas(...calldata);
    const receipt = await (await settlement.settle(...calldata)).wait();
    if (!await settlement.consumedDeals(input.dealNullifier)
        || !await settlement.consumedNotes(input.inputNullifier)
        || await settlement.outputCommitments(0) !== BigInt(input.paymentCommitment)
        || await settlement.outputCommitments(1) !== BigInt(input.changeCommitment)) {
      throw new Error("atomic state transition did not record expected outputs");
    }
    const restartedSeller = JSON.parse(JSON.stringify(sellerRecovery));
    const poseidon = (items) => eddsa.babyJub.F.toObject(eddsa.poseidon(items));
    const recoveredTag = poseidon([2006n, BigInt(restartedSeller.spendSecret)]);
    const recoveredSalt = poseidon([
      2009n, BigInt(restartedSeller.domain),
      BigInt(restartedSeller.settlementNonceLo), BigInt(restartedSeller.settlementNonceHi),
    ]);
    const recoveredPayment = poseidon([
      2007n, BigInt(restartedSeller.asset), BigInt(restartedSeller.amount),
      BigInt(restartedSeller.sellerAx), BigInt(restartedSeller.sellerAy),
      recoveredTag, recoveredSalt,
    ]);
    if (recoveredPayment !== await settlement.outputCommitments(0)) {
      throw new Error("seller could not reconstruct its payment note after settlement");
    }
    await expectRejected("deal replay", async () => settlement.settle.staticCall(...calldata));
    for (const [label, index, value] of [
      ["wrong chain", 0, "1"],
      ["wrong contract", 1, "1"],
      ["wrong verifier version", 2, "1"],
      ["wrong asset", 3, "1"],
      ["unknown root", 6, "1"],
      ["expired authorization", 10, "1"],
    ]) {
      const changed = structuredClone(calldata);
      changed[3][index] = value;
      await expectRejected(label, async () => settlement.settle.staticCall(...changed));
    }

    const mutatedPublic = structuredClone(calldata);
    mutatedPublic[3][8] = `0x${(BigInt(mutatedPublic[3][8]) + 1n).toString(16)}`;
    if (await verifier.verifyProof(...mutatedPublic)) {
      throw new Error("verifier accepted a changed payment output");
    }
    for (const [label, change] of [
      ["changed amount", { amount: "71" }],
      ["changed recipient", { sellerAx: (BigInt(input.sellerAx) + 1n).toString() }],
      ["changed root", { root: (BigInt(input.root) + 1n).toString() }],
      ["changed signature", { buyerS: (BigInt(input.buyerS) + 1n).toString() }],
      ["changed conservation", { changeAmount: "79" }],
      ["changed expiry", { expiry: (expiry - 1n).toString() }],
      ["zero blinding", { blinding: "0" }],
      ["zero input spend secret", { inputSpendSecret: "0" }],
      ["zero recipient spend tag", { recipientSpendTag: "0" }],
    ]) {
      await expectRejected(label, async () => snarkjs.groth16.fullProve({ ...input, ...change }, wasmPath, zkeyPath));
    }

    const metrics = {
      toolchain: { circom: "2.2.3", snarkjs: "0.7.6", solc: "0.8.24" },
      machine: {
        platform: process.platform, arch: process.arch,
        cpus: (await import("node:os")).cpus().length,
        cpuModel: (await import("node:os")).cpus()[0].model,
      },
      circuit: { constraints, depth: 4, publicInputs: 11 },
      provingMs, peakNodeRssMiB: Math.round(peak.bytes / 1024 / 1024),
      proofBytes: 256, naiveVerifierGasEstimate: naiveVerifierGasEstimate.toString(),
      verifierCallGasUsed: verifierCallReceipt.gasUsed.toString(),
      verifierPrecompileCalls: {
        ecMul: verifierPrecompiles.filter((address) => address === "7").length,
        ecPairing: verifierPrecompiles.filter((address) => address === "8").length,
      },
      settlementGasEstimate: settlementGasEstimate.toString(),
      settlementGasUsed: receipt.gasUsed.toString(),
      verifierAddress: await verifier.getAddress(), settlementAddress: await settlement.getAddress(),
      transactionHash: receipt.hash, testChainId: 10143,
      setup: "single local contributor; test-only and unsuitable for value",
      artifactManifest,
    };
    writeFileSync(resolve(buildDir, "metrics.json"), `${JSON.stringify(metrics, null, 2)}\n`);
    writeFileSync(resolve(buildDir, "public.json"), `${JSON.stringify(publicSignals, null, 2)}\n`);
    writeFileSync(resolve(buildDir, "proof.json"), `${JSON.stringify(proof, null, 2)}\n`);
    console.log(JSON.stringify(metrics, null, 2));
    console.log("M4 prototype: proof, local EVM transition, replay and mutation checks passed");
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
