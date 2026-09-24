pragma circom 2.2.3;

include "circomlib/circuits/poseidon.circom";
include "circomlib/circuits/eddsaposeidon.circom";
include "circomlib/circuits/bitify.circom";

// Domain tags are distinct field elements; all hashes use circomlib Poseidon on BN254.
template PrivateTransfer(depth) {
    signal input chainId;
    signal input contractAddress;
    signal input verifierVersion;
    signal input asset;
    signal input dealCommitment;
    signal input dealNullifier;
    signal input root;
    signal input inputNullifier;
    signal input paymentCommitment;
    signal input changeCommitment;
    signal input expiry;

    signal input dealId;
    signal input revision;
    signal input transcriptRootLo;
    signal input transcriptRootHi;
    signal input serviceDigestLo;
    signal input serviceDigestHi;
    signal input settlementNonceLo;
    signal input settlementNonceHi;
    signal input amount;
    signal input blinding;
    signal input buyerAx;
    signal input buyerAy;
    signal input sellerAx;
    signal input sellerAy;
    signal input recipientSpendTag;
    signal input buyerR8x;
    signal input buyerR8y;
    signal input buyerS;
    signal input sellerR8x;
    signal input sellerR8y;
    signal input sellerS;

    signal input inputAmount;
    signal input inputSalt;
    signal input inputSpendSecret;
    signal input pathElements[depth];
    signal input pathIndices[depth];
    signal input changeAmount;
    signal input changeSpendTag;
    signal input changeSalt;

    // Fixed shape for the prototype: one ERC-20 asset, one input, one recipient,
    // one change output, no protocol fee. The agreement fixes every payment field.
    component chainBits = Num2Bits(64);
    chainBits.in <== chainId;
    component addressBits = Num2Bits(160);
    addressBits.in <== contractAddress;
    component versionBits = Num2Bits(32);
    versionBits.in <== verifierVersion;
    component idBits = Num2Bits(128);
    idBits.in <== dealId;
    component revisionBits = Num2Bits(32);
    revisionBits.in <== revision;
    component expiryBits = Num2Bits(64);
    expiryBits.in <== expiry;
    component assetBits = Num2Bits(160);
    assetBits.in <== asset;
    component amountBits = Num2Bits(128);
    amountBits.in <== amount;
    component inputAmountBits = Num2Bits(128);
    inputAmountBits.in <== inputAmount;
    component changeAmountBits = Num2Bits(128);
    changeAmountBits.in <== changeAmount;
    component transcriptLoBits = Num2Bits(128);
    transcriptLoBits.in <== transcriptRootLo;
    component transcriptHiBits = Num2Bits(128);
    transcriptHiBits.in <== transcriptRootHi;
    component serviceLoBits = Num2Bits(128);
    serviceLoBits.in <== serviceDigestLo;
    component serviceHiBits = Num2Bits(128);
    serviceHiBits.in <== serviceDigestHi;
    component nonceLoBits = Num2Bits(128);
    nonceLoBits.in <== settlementNonceLo;
    component nonceHiBits = Num2Bits(128);
    nonceHiBits.in <== settlementNonceHi;

    // Prevent field wraparound in conservation: three 128-bit quantities fit in BN254.
    inputAmount === amount + changeAmount;
    // A zero-value agreement would let a relayer consume a deal without payment.
    component amountZero = IsZero();
    amountZero.in <== amount;
    amountZero.out === 0;
    component assetZero = IsZero();
    assetZero.in <== asset;
    assetZero.out === 0;
    component revisionZero = IsZero();
    revisionZero.in <== revision;
    revisionZero.out === 0;
    component blindingZero = IsZero();
    blindingZero.in <== blinding;
    blindingZero.out === 0;
    component spendSecretZero = IsZero();
    spendSecretZero.in <== inputSpendSecret;
    spendSecretZero.out === 0;
    component recipientTagZero = IsZero();
    recipientTagZero.in <== recipientSpendTag;
    recipientTagZero.out === 0;

    component domain = Poseidon(3);
    domain.inputs[0] <== chainId;
    domain.inputs[1] <== contractAddress;
    domain.inputs[2] <== verifierVersion;
    component paymentSalt = Poseidon(4);
    paymentSalt.inputs[0] <== 2009;
    paymentSalt.inputs[1] <== domain.out;
    paymentSalt.inputs[2] <== settlementNonceLo;
    paymentSalt.inputs[3] <== settlementNonceHi;

    // The suite-2 commitment maps M1 agreement fields to fixed field elements.
    // Service/transcript digests are two 128-bit limbs, preserving all 256 bits.
    component termsA = Poseidon(6);
    termsA.inputs[0] <== 2001;
    termsA.inputs[1] <== domain.out;
    termsA.inputs[2] <== dealId;
    termsA.inputs[3] <== revision;
    termsA.inputs[4] <== transcriptRootLo;
    termsA.inputs[5] <== transcriptRootHi;
    component termsB = Poseidon(6);
    termsB.inputs[0] <== serviceDigestLo;
    termsB.inputs[1] <== serviceDigestHi;
    termsB.inputs[2] <== settlementNonceLo;
    termsB.inputs[3] <== settlementNonceHi;
    termsB.inputs[4] <== asset;
    termsB.inputs[5] <== amount;
    component termsC = Poseidon(6);
    termsC.inputs[0] <== buyerAx;
    termsC.inputs[1] <== buyerAy;
    termsC.inputs[2] <== sellerAx;
    termsC.inputs[3] <== sellerAy;
    termsC.inputs[4] <== recipientSpendTag;
    termsC.inputs[5] <== paymentSalt.out;
    component commitment = Poseidon(6);
    commitment.inputs[0] <== 2002;
    commitment.inputs[1] <== termsA.out;
    commitment.inputs[2] <== termsB.out;
    commitment.inputs[3] <== termsC.out;
    commitment.inputs[4] <== blinding;
    commitment.inputs[5] <== expiry;
    commitment.out === dealCommitment;

    component nullifier = Poseidon(6);
    nullifier.inputs[0] <== 2003;
    nullifier.inputs[1] <== domain.out;
    nullifier.inputs[2] <== buyerAx;
    nullifier.inputs[3] <== buyerAy;
    nullifier.inputs[4] <== settlementNonceLo;
    nullifier.inputs[5] <== settlementNonceHi;
    nullifier.out === dealNullifier;

    component buyerMessage = Poseidon(3);
    buyerMessage.inputs[0] <== 2004;
    buyerMessage.inputs[1] <== domain.out;
    buyerMessage.inputs[2] <== dealCommitment;
    component sellerMessage = Poseidon(3);
    sellerMessage.inputs[0] <== 2005;
    sellerMessage.inputs[1] <== domain.out;
    sellerMessage.inputs[2] <== dealCommitment;
    component buyerAuth = EdDSAPoseidonVerifier();
    buyerAuth.enabled <== 1;
    buyerAuth.Ax <== buyerAx;
    buyerAuth.Ay <== buyerAy;
    buyerAuth.R8x <== buyerR8x;
    buyerAuth.R8y <== buyerR8y;
    buyerAuth.S <== buyerS;
    buyerAuth.M <== buyerMessage.out;
    component sellerAuth = EdDSAPoseidonVerifier();
    sellerAuth.enabled <== 1;
    sellerAuth.Ax <== sellerAx;
    sellerAuth.Ay <== sellerAy;
    sellerAuth.R8x <== sellerR8x;
    sellerAuth.R8y <== sellerR8y;
    sellerAuth.S <== sellerS;
    sellerAuth.M <== sellerMessage.out;

    component spendTag = Poseidon(2);
    spendTag.inputs[0] <== 2006;
    spendTag.inputs[1] <== inputSpendSecret;
    component inputNote = Poseidon(7);
    inputNote.inputs[0] <== 2007;
    inputNote.inputs[1] <== asset;
    inputNote.inputs[2] <== inputAmount;
    inputNote.inputs[3] <== buyerAx;
    inputNote.inputs[4] <== buyerAy;
    inputNote.inputs[5] <== spendTag.out;
    inputNote.inputs[6] <== inputSalt;
    component inputNull = Poseidon(3);
    inputNull.inputs[0] <== 2008;
    inputNull.inputs[1] <== inputSpendSecret;
    inputNull.inputs[2] <== inputNote.out;
    inputNull.out === inputNullifier;

    signal pathNode[depth + 1];
    component branch[depth];
    pathNode[0] <== inputNote.out;
    for (var i = 0; i < depth; i++) {
        pathIndices[i] * (pathIndices[i] - 1) === 0;
        branch[i] = Poseidon(2);
        branch[i].inputs[0] <== pathNode[i] + pathIndices[i] * (pathElements[i] - pathNode[i]);
        branch[i].inputs[1] <== pathElements[i] + pathIndices[i] * (pathNode[i] - pathElements[i]);
        pathNode[i + 1] <== branch[i].out;
    }
    pathNode[depth] === root;

    component paymentNote = Poseidon(7);
    paymentNote.inputs[0] <== 2007;
    paymentNote.inputs[1] <== asset;
    paymentNote.inputs[2] <== amount;
    paymentNote.inputs[3] <== sellerAx;
    paymentNote.inputs[4] <== sellerAy;
    paymentNote.inputs[5] <== recipientSpendTag;
    paymentNote.inputs[6] <== paymentSalt.out;
    paymentNote.out === paymentCommitment;

    component changeNote = Poseidon(7);
    changeNote.inputs[0] <== 2007;
    changeNote.inputs[1] <== asset;
    changeNote.inputs[2] <== changeAmount;
    changeNote.inputs[3] <== buyerAx;
    changeNote.inputs[4] <== buyerAy;
    changeNote.inputs[5] <== changeSpendTag;
    changeNote.inputs[6] <== changeSalt;
    changeNote.out === changeCommitment;
}

component main {public [chainId, contractAddress, verifierVersion, asset, dealCommitment,
    dealNullifier, root, inputNullifier, paymentCommitment, changeCommitment, expiry]} = PrivateTransfer(4);
