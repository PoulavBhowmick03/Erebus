import { createHash } from "node:crypto";
import { execFileSync, spawn } from "node:child_process";
import { once } from "node:events";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
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

function serviceDigestFromM1Vector(override) {
  const fixture = JSON.parse(readFileSync(resolve(root, "sdk/core/tests/fixtures/agreement-v1-vectors.json"), "utf8"));
  const service = override ?? fixture.vectors[0].terms.service;
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
  if (!override && !fixture.vectors[0].expected.canonicalHex.endsWith(encoded.toString("hex"))) {
    throw new Error("M5 service encoding differs from the pinned Rust M1 vector");
  }
  const digest = sha256(encoded);
  return [BigInt(`0x${digest.slice(0, 32)}`), BigInt(`0x${digest.slice(32)}`)];
}

function stringify(value) {
  return JSON.parse(JSON.stringify(value, (_key, item) => typeof item === "bigint" ? item.toString() : item));
}

function fieldHex(value, bytes = 32) {
  return BigInt(value).toString(16).padStart(bytes * 2, "0");
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
    const wasm = resolve(build, `${name}_js/${name}.wasm`);
    const zkey = resolve(build, `${name}.zkey`);
    const vkey = resolve(build, `${name}-verification-key.json`);
    const verifierSource = resolve(build, `${name}-verifier.sol`);
    const r1csHash = sha256(readFileSync(r1cs));
    const previous = oldManifest.circuits?.[name];
    const reuse = previous?.r1csSha256 === r1csHash
      && [zkey, vkey, verifierSource].every((file) => existsSync(file))
      && (previous.wasmSha256 === undefined || previous.wasmSha256 === sha256(readFileSync(wasm)))
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
      wasmSha256: sha256(readFileSync(wasm)),
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
  let forkAnvil = null;
  let forkProvider = null;
  let indexerProcess = null;
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
    writeFileSync(resolve(build, "deposit-input.json"), `${JSON.stringify(stringify(depositInput), null, 2)}\n`);
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
    const depositReceipt = await (await pool.deposit(...depositProof)).wait();
    await reject("duplicate funded note", async () => pool.deposit.staticCall(...depositProof));
    if (!await pool.insertedCommitments(inputNote)) throw new Error("funded note was not registered");
    const firstTree = tree(poseidon, [inputNote], 0);
    if (await pool.currentRoot() !== firstTree.root) throw new Error("deposit tree root differs from Poseidon path");

    const domain = poseidon([chainId, contractAddress, version]);
    const dealId = BigInt(`0x${sha256("EREBUS_M5_DEAL_TEST_ONLY").slice(0, 32)}`);
    const transcript = JSON.parse(execFileSync("cargo", [
      "run", "--locked", "--quiet", "--manifest-path", resolve(root, "sdk/transport/Cargo.toml"),
      "--example", "m5_transcript", "--", fieldHex(dealId, 16),
    ], { cwd: dir, encoding: "utf8" }));
    if (transcript.dealIdHex !== fieldHex(dealId, 16)
      || !/^[0-9a-f]{64}$/.test(transcript.transcriptRootHex)
      || !Array.isArray(transcript.messagesHex) || transcript.messagesHex.length === 0) {
      throw new Error("Rust transcript fixture is invalid");
    }
    writeFileSync(resolve(build, "transcript.json"), `${JSON.stringify(transcript, null, 2)}\n`);
    const transcriptRootLo = BigInt(`0x${transcript.transcriptRootHex.slice(0, 32)}`);
    const transcriptRootHi = BigInt(`0x${transcript.transcriptRootHex.slice(32)}`);
    const service = process.env.EREBUS_M8_ACCESS === "1" ? {
      resource: "dataset.snapshot.v1", quantity: "1", unit: "snapshot",
      accessRecipientHex: fieldHex(buyer[0]) + fieldHex(buyer[1]),
      deliveryDeadline: 4102444800, fulfillmentMethod: "http-access-v1",
      fulfillmentDigestHex: sha256("EREBUS_M8_TEST_ONLY_DATASET"),
    } : JSON.parse(readFileSync(resolve(root, "sdk/core/tests/fixtures/agreement-v1-vectors.json"), "utf8")).vectors[0].terms.service;
    const [serviceDigestLo, serviceDigestHi] = serviceDigestFromM1Vector(process.env.EREBUS_M8_ACCESS === "1" ? service : undefined);
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
    const signatureHex = (signature) => [F.toObject(signature.R8[0]), F.toObject(signature.R8[1]), signature.S]
      .map((part) => fieldHex(part)).join("");
    const agreementInput = {
      blindingHex: fieldHex(blinding),
      buyerSignatureHex: signatureHex(buyerSignature),
      sellerSignatureHex: signatureHex(sellerSignature),
      terms: {
        protocolVersion: 1, suiteId: 2,
        domain: {
          namespace: `eip155:${chainId}`,
          settlementContractHex: fieldHex(contractAddress, 20),
          poolHex: fieldHex(contractAddress, 20),
          verifierVersion: Number(version),
        },
        dealIdHex: fieldHex(dealId, 16), revision: 1,
        transcriptRootHex: fieldHex(transcriptRootLo, 16) + fieldHex(transcriptRootHi, 16),
        buyerAuthorizationKeyHex: fieldHex(buyer[0]) + fieldHex(buyer[1]),
        sellerAuthorizationKeyHex: fieldHex(seller[0]) + fieldHex(seller[1]),
        paymentRecipientHex: fieldHex(recipientSpendTag),
        asset: `eip155:${chainId}/erc20:0x${fieldHex(asset, 20)}`,
        amount: String(amount), expiry: Number(expiry), fee: "0", feeRecipientHex: null,
        settlementMode: "shielded",
        requiredGuarantees: ["hidden-amount", "hidden-recipient", "agreement-bound-settlement"],
        settlementNonceHex: fieldHex(settlementNonceLo, 16) + fieldHex(settlementNonceHi, 16),
        service,
      },
    };
    writeFileSync(resolve(build, "agreement-input.json"), `${JSON.stringify(agreementInput, null, 2)}\n`);
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
    writeFileSync(resolve(build, "transfer-input.json"), `${JSON.stringify(stringify(transferInput), null, 2)}\n`);
    const transferPublic = [
      chainId, contractAddress, version, asset, dealCommitment, dealNullifier, firstTree.root,
      inputNullifier, paymentNote, changeNote, expiry,
    ];
    const transferProof = await prove("transfer", transferInput, transferPublic);
    let settlementProof = transferProof;
    if (process.env.EREBUS_M5_NATIVE_PROOF === "1" && process.env.EREBUS_M6_COORDINATED !== "1") {
      const prepared = JSON.parse(readFileSync(resolve(build, "rust-prepared-transfer-calldata.json"), "utf8"));
      const native = stringify(pool.interface.decodeFunctionData("transferPrivate", prepared));
      if (!Array.isArray(native[3]) || native[3].length !== transferProof[3].length
        || native[3].some((value, index) => BigInt(value) !== BigInt(transferProof[3][index]))) {
        throw new Error("native proof public inputs differ from the JS agreement transition");
      }
      if (!await verifier.transfer.verifyProof(...native)) {
        throw new Error("native Rust proof failed the deployed Solidity verifier");
      }
      await pool.transferPrivate.staticCall(...native);
      settlementProof = native;
    }
    await reject("service mutation in witness", async () => prove("transfer", {
      ...transferInput, serviceDigestLo: serviceDigestLo + 1n,
    }, transferPublic));
    await reject("seller authorization mutation in witness", async () => prove("transfer", {
      ...transferInput, sellerS: sellerSignature.S + 1n,
    }, transferPublic));
    for (const [label, mutation] of [
      ["buyer authorization", { buyerS: buyerSignature.S + 1n }],
      ["agreed amount", { amount: amount + 1n }],
      ["recipient spend tag", { recipientSpendTag: recipientSpendTag + 1n }],
      ["input spend secret", { inputSpendSecret: inputSpendSecret + 1n }],
      ["change conservation", { changeAmount: changeAmount + 1n }],
      ["membership sibling", { pathElements: [firstTree.pathElements[0] + 1n, ...firstTree.pathElements.slice(1)] }],
    ]) {
      await reject(`${label} mutation in witness`, async () => prove("transfer", {
        ...transferInput, ...mutation,
      }, transferPublic));
    }
    for (const [label, index] of [
      ["chain", 0], ["pool", 1], ["verifier version", 2], ["asset", 3],
      ["deal commitment", 4], ["deal nullifier", 5], ["root", 6],
      ["input nullifier", 7], ["payment output", 8], ["change output", 9], ["expiry", 10],
    ]) {
      const changed = structuredClone(transferProof);
      changed[3][index] = String(BigInt(changed[3][index]) + 1n);
      await reject(`${label} public-input mutation`, async () => pool.transferPrivate.staticCall(...changed));
      if (label === "asset") {
        await reject("asset-changing transaction", async () => pool.transferPrivate(...changed));
      }
    }
    let fundedMatrix = null;
    let fundedDisclosure = null;
    if (process.env.EREBUS_M6_COORDINATED === "1") {
      if (process.env.EREBUS_M5_NATIVE_PROOF !== "1") {
        throw new Error("coordinated transfer requires the native Rust proof");
      }
      const deploymentReceipt = await pool.deploymentTransaction().wait();
      const stateRoot = mkdtempSync(resolve(build, "coordinated-"));
      run("cargo", ["build", ...(process.env.EREBUS_M6_MATRIX === "1" ? ["--release"] : []),
        "--locked", "--quiet", "--manifest-path", resolve(root, "sdk/shielded/Cargo.toml"), "--bin", "erebus-shielded-disclosure"]);
      process.env.EREBUS_M7_DISCLOSURE_BIN ??= resolve(root, "sdk/shielded/target",
        process.env.EREBUS_M6_MATRIX === "1" ? "release" : "debug", "erebus-shielded-disclosure");
      if (process.env.EREBUS_M8_ACCESS === "1") {
        run("cargo", ["build", ...(process.env.EREBUS_M6_MATRIX === "1" ? ["--release"] : []),
          "--locked", "--quiet", "--manifest-path", resolve(root, "sdk/shielded/Cargo.toml"), "--bin", "erebus-access-service", "--bin", "erebus-access"]);
        process.env.EREBUS_M8_ACCESS_BIN ??= resolve(root, "sdk/shielded/target",
          process.env.EREBUS_M6_MATRIX === "1" ? "release" : "debug", "erebus-access-service");
        process.env.EREBUS_M8_ACCESS_CLIENT_BIN ??= resolve(root, "sdk/shielded/target",
          process.env.EREBUS_M6_MATRIX === "1" ? "release" : "debug", "erebus-access");
      }
      run("cargo", [
        "run", ...(process.env.EREBUS_M6_MATRIX === "1" ? ["--release"] : []),
        "--locked", "--quiet", "--manifest-path", resolve(root, "sdk/shielded/Cargo.toml"),
        "--example", "coordinated_transfer", "--", `http://127.0.0.1:${port}`, poolAddress,
        String(deploymentReceipt.blockNumber), deploymentReceipt.blockHash,
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80", stateRoot,
      ]);
      fundedDisclosure = JSON.parse(readFileSync(resolve(stateRoot, "disclosure-report.json"), "utf8"));
      if (fundedDisclosure.mode !== "shielded" || fundedDisclosure.agreement_verified !== true
        || fundedDisclosure.payment_verified !== true || fundedDisclosure.delivery_verified !== false) {
        throw new Error("independent shielded disclosure did not verify agreement and payment");
      }
      if (process.env.EREBUS_M6_MATRIX === "1") {
        fundedMatrix = JSON.parse(readFileSync(resolve(stateRoot, "funded-matrix/report.json"), "utf8"));
        if (fundedMatrix.boundaries < 1 || fundedMatrix.completedTrials !== fundedMatrix.boundaries + 1) {
          throw new Error("funded crash matrix did not complete every discovered boundary");
        }
      }
    } else {
      await (await pool.transferPrivate(...settlementProof)).wait();
    }
    if (!await pool.insertedCommitments(paymentNote) || !await pool.insertedCommitments(changeNote)) {
      throw new Error("transfer outputs were not registered");
    }
    if (!await pool.consumedDeals(dealNullifier) || !await pool.consumedNotes(inputNullifier)) {
      throw new Error("transfer did not consume deal and input note");
    }
    await reject("deal replay", async () => pool.transferPrivate.staticCall(...transferProof));
    const finalTree = tree(poseidon, [inputNote, paymentNote, changeNote], 1);
    if (await pool.currentRoot() !== finalTree.root) throw new Error("transfer tree root differs");
    const noteVector = {
      assetHex: fieldHex(asset, 20),
      input: {
        amount: String(inputAmount), ownerHex: fieldHex(buyer[0]) + fieldHex(buyer[1]),
        spendSecretHex: fieldHex(inputSpendSecret), spendTagHex: fieldHex(inputSpendTag),
        saltHex: fieldHex(inputSalt), commitmentHex: fieldHex(inputNote),
        nullifierHex: fieldHex(inputNullifier), index: 0,
        rootHex: fieldHex(firstTree.root),
      },
      payment: {
        amount: String(amount), ownerHex: fieldHex(seller[0]) + fieldHex(seller[1]),
        spendSecretHex: fieldHex(recipientSpendSecret), spendTagHex: fieldHex(recipientSpendTag),
        saltHex: fieldHex(paymentSalt), commitmentHex: fieldHex(paymentNote),
        nullifierHex: fieldHex(poseidon([2008n, recipientSpendSecret, paymentNote])), index: 1,
        rootHex: fieldHex(finalTree.root),
      },
      changeCommitmentHex: fieldHex(changeNote),
    };
    writeFileSync(resolve(build, "note-vector.json"), `${JSON.stringify(noteVector, null, 2)}\n`);
    const pinnedNote = JSON.parse(readFileSync(resolve(root, "sdk/core/tests/fixtures/m5-note-vector.json"), "utf8"));
    if (JSON.stringify(noteVector) !== JSON.stringify(pinnedNote)) {
      throw new Error("computed M5 note vector differs from pinned Rust fixture");
    }
    let recoverRecipient = () => {};
    if (process.env.EREBUS_M5_RUST_SCAN === "1") {
      const deploymentReceipt = await pool.deploymentTransaction().wait();
      const recoveryDir = mkdtempSync(resolve(build, "recovery-"));
      const args = [
        "run", "--locked", "--quiet", "--manifest-path", resolve(root, "sdk/shielded/Cargo.toml"),
        "--example", "recover_test_note", "--", `http://127.0.0.1:${port}`, String(chainId), poolAddress,
        String(deploymentReceipt.blockNumber), deploymentReceipt.blockHash,
        resolve(recoveryDir, "public/index.json"), resolve(recoveryDir, "private/wallet.enc"),
      ];
      recoverRecipient = (expected, rpcUrl = `http://127.0.0.1:${port}`) => {
        const command = [...args, expected];
        command[8] = rpcUrl;
        run("cargo", command);
      };
      recoverRecipient("1");
    }

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
    writeFileSync(resolve(build, "withdraw-input.json"), `${JSON.stringify(stringify(withdrawInput), null, 2)}\n`);
    const withdrawalProof = await prove("withdraw", withdrawInput, [
      chainId, contractAddress, version, asset, finalTree.root, paymentNullifier, recipient, amount,
    ]);
    const changed = structuredClone(withdrawalProof);
    changed[3][6] = "1";
    await reject("withdraw redirect", async () => pool.withdraw.staticCall(...changed));
    const changedAmount = structuredClone(withdrawalProof);
    changedAmount[3][7] = "71";
    await reject("withdraw amount mutation", async () => pool.withdraw.staticCall(...changedAmount));
    await (await token.setFeeOnTransfer(true)).wait();
    await reject("fee-on-transfer withdrawal", async () => pool.withdraw.staticCall(...withdrawalProof));
    await (await token.setFeeOnTransfer(false)).wait();
    const beforeWithdrawalSnapshot = process.env.EREBUS_M5_RUST_SCAN === "1"
      ? await provider.send("evm_snapshot", []) : null;
    const before = await token.balanceOf(await signer.getAddress());
    const withdrawalReceipt = await (await pool.withdraw(...withdrawalProof)).wait();
    if (await token.balanceOf(await signer.getAddress()) !== before + amount) throw new Error("seller payout mismatch");
    if (await token.balanceOf(poolAddress) !== changeAmount) throw new Error("pool liability mismatch");
    await reject("withdraw replay", async () => pool.withdraw.staticCall(...withdrawalProof));
    recoverRecipient("0");
    if (process.env.EREBUS_M5_RUST_SCAN === "1") {
      const deploymentReceipt = await pool.deploymentTransaction().wait();
      const cache = resolve(mkdtempSync(resolve(build, "index-")), "pool.json");
      const scanner = [
        "run", "--locked", "--quiet", "--manifest-path", resolve(root, "sdk/shielded/Cargo.toml"),
        "--example", "scan_pool", "--", `http://127.0.0.1:${port}`, String(chainId), poolAddress,
        String(deploymentReceipt.blockNumber),
      ];
      run("cargo", [
        ...scanner, String(depositReceipt.blockNumber), `0x${fieldHex(firstTree.root)}`, "1",
        cache, deploymentReceipt.blockHash,
      ]);
      run("cargo", [
        ...scanner, String(withdrawalReceipt.blockNumber), `0x${fieldHex(finalTree.root)}`, "3",
        cache, deploymentReceipt.blockHash,
      ]);
      if (!await provider.send("evm_revert", [beforeWithdrawalSnapshot])) {
        throw new Error("Anvil did not revert the withdrawal block");
      }
      recoverRecipient("1");
      await provider.send("evm_mine", []);
      recoverRecipient("1");
      await (await pool.withdraw(...withdrawalProof)).wait();
      recoverRecipient("0");
      if (await token.balanceOf(poolAddress) !== changeAmount) {
        throw new Error("reorg rehearsal left the wrong pool liability");
      }
      const forkPort = await freePort();
      const forkUrl = `http://127.0.0.1:${forkPort}`;
      forkAnvil = spawn("anvil", [
        "--port", String(forkPort), "--chain-id", String(chainId),
        "--fork-url", `http://127.0.0.1:${port}`,
        "--fork-block-number", String(await provider.getBlockNumber()), "--silent",
      ], { stdio: "ignore" });
      forkProvider = new JsonRpcProvider(forkUrl, 10143, { staticNetwork: true });
      let forkReady = false;
      for (let attempt = 0; attempt < 40; attempt += 1) {
        try { await forkProvider.getBlockNumber(); forkReady = true; break; } catch { await delay(100); }
      }
      if (!forkReady) throw new Error("second Anvil RPC did not start");
      recoverRecipient("0", forkUrl);

      run("cargo", [
        "build", "--locked", "--quiet", "--manifest-path", resolve(root, "sdk/shielded/Cargo.toml"),
        "--bin", "erebus_pool_indexer",
      ]);
      const indexerPort = await freePort();
      const indexerUrl = `http://127.0.0.1:${indexerPort}`;
      const indexerToken = "m5-local-indexer-test-only";
      indexerProcess = spawn(resolve(root, "sdk/shielded/target/debug/erebus_pool_indexer"), [], {
        stdio: "ignore",
        env: {
          ...process.env,
          EREBUS_INDEXER_RPC: forkUrl,
          EREBUS_INDEXER_CHAIN_ID: String(chainId),
          EREBUS_INDEXER_POOL: poolAddress,
          EREBUS_INDEXER_DEPLOYMENT_BLOCK: String(deploymentReceipt.blockNumber),
          EREBUS_INDEXER_DEPLOYMENT_HASH: deploymentReceipt.blockHash,
          EREBUS_INDEXER_ROOT: mkdtempSync(resolve(build, "service-")),
          EREBUS_INDEXER_CONFIRMATIONS: "0",
          EREBUS_INDEXER_PORT: String(indexerPort),
          EREBUS_INDEXER_TOKEN: indexerToken,
        },
      });
      const headers = { Authorization: `Bearer ${indexerToken}` };
      let indexerReady = false;
      for (let attempt = 0; attempt < 600; attempt += 1) {
        if (indexerProcess.exitCode !== null) throw new Error("Rust public indexer exited before readiness");
        try {
          const response = await fetch(`${indexerUrl}/healthz`, { headers });
          if (response.ok && (await response.json()).status === "ok") {
            indexerReady = true;
            break;
          }
        } catch { /* wait for the local server */ }
        await delay(100);
      }
      if (!indexerReady) throw new Error("Rust public indexer did not reach healthy state");
      if ((await fetch(`${indexerUrl}/healthz`)).status !== 401) {
        throw new Error("public indexer accepted an unauthenticated request");
      }
      const blocksResponse = await fetch(
        `${indexerUrl}/v1/blocks?after=${deploymentReceipt.blockNumber - 1}&limit=128`,
        { headers },
      );
      if (!blocksResponse.ok) throw new Error("public indexer block read failed");
      const indexed = await blocksResponse.json();
      if (indexed.root !== `0x${fieldHex(finalTree.root)}`
        || indexed.blocks?.flatMap((block) => block.events).filter((event) => event.Inserted).length !== 3) {
        throw new Error("hosted public index differs from verified pool history");
      }
    }
    writeFileSync(resolve(build, "run.json"), `${JSON.stringify({
      chainId: Number(chainId), pool: poolAddress, asset: await token.getAddress(),
      deposit: inputAmount.toString(), privatePayment: amount.toString(),
      finalVaultBalance: (await token.balanceOf(poolAddress)).toString(), artifacts,
      testOnly: true,
      fundedMatrix,
      fundedDisclosure,
    }, null, 2)}\n`);
    console.log("M5 local prototype: funded deposit, private transfer, output recovery, withdrawal, replay checks passed");
  } finally {
    if (indexerProcess?.exitCode === null) {
      indexerProcess.kill("SIGTERM");
      await Promise.race([once(indexerProcess, "exit"), delay(1000)]);
    }
    if (forkProvider) await forkProvider.destroy();
    if (forkAnvil?.exitCode === null) {
      forkAnvil.kill("SIGTERM");
      await Promise.race([once(forkAnvil, "exit"), delay(1000)]);
    }
    await provider.destroy();
    if (anvil.exitCode === null) {
      anvil.kill("SIGTERM");
      await Promise.race([once(anvil, "exit"), delay(1000)]);
    }
    await globalThis.curve_bn128?.terminate();
  }
}

main().catch((error) => { console.error(error); process.exitCode = 1; });
