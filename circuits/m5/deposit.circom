pragma circom 2.2.3;

include "circomlib/circuits/poseidon.circom";
include "circomlib/circuits/bitify.circom";

template DepositNote() {
    signal input chainId;
    signal input contractAddress;
    signal input verifierVersion;
    signal input asset;
    signal input amount;
    signal input noteCommitment;

    signal input ownerAx;
    signal input ownerAy;
    signal input spendTag;
    signal input salt;
    signal input privateChainId;
    signal input privateContractAddress;
    signal input privateVerifierVersion;

    component chainBits = Num2Bits(64);
    chainBits.in <== chainId;
    component contractBits = Num2Bits(160);
    contractBits.in <== contractAddress;
    component versionBits = Num2Bits(32);
    versionBits.in <== verifierVersion;
    component assetBits = Num2Bits(160);
    assetBits.in <== asset;
    component amountBits = Num2Bits(128);
    amountBits.in <== amount;
    component amountZero = IsZero();
    amountZero.in <== amount;
    amountZero.out === 0;
    component spendTagZero = IsZero();
    spendTagZero.in <== spendTag;
    spendTagZero.out === 0;

    component note = Poseidon(7);
    note.inputs[0] <== 2007;
    note.inputs[1] <== asset;
    note.inputs[2] <== amount;
    note.inputs[3] <== ownerAx;
    note.inputs[4] <== ownerAy;
    note.inputs[5] <== spendTag;
    note.inputs[6] <== salt;
    note.out === noteCommitment;

    // The pool checks these against its deployment. These constraints also ensure the
    // public values cannot be changed while reusing an existing proof.
    privateChainId === chainId;
    privateContractAddress === contractAddress;
    privateVerifierVersion === verifierVersion;
}

component main {public [chainId, contractAddress, verifierVersion, asset, amount, noteCommitment]} = DepositNote();
