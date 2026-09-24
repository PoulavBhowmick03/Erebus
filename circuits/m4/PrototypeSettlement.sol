// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

interface ITransferVerifier {
    function verifyProof(
        uint256[2] calldata a,
        uint256[2][2] calldata b,
        uint256[2] calldata c,
        uint256[11] calldata input
    ) external view returns (bool);
}

/// @notice M4 proof-enforcement harness. It has no custody or deposit path.
contract PrototypeSettlement {
    ITransferVerifier public immutable verifier;
    uint256 public immutable verifierVersion;
    uint256 public immutable asset;
    uint256 public immutable acceptedRoot;
    mapping(uint256 => bool) public consumedDeals;
    mapping(uint256 => bool) public consumedNotes;
    uint256[] public outputCommitments;

    event TransferAccepted(
        uint256 indexed dealCommitment,
        uint256 indexed dealNullifier,
        uint256 indexed inputNullifier,
        uint256 paymentCommitment,
        uint256 changeCommitment
    );

    constructor(address verifier_, uint256 verifierVersion_, uint256 asset_, uint256 root_) {
        require(verifier_.code.length != 0, "verifier has no code");
        require(asset_ != 0 && asset_ < (1 << 160), "invalid asset");
        verifier = ITransferVerifier(verifier_);
        verifierVersion = verifierVersion_;
        asset = asset_;
        acceptedRoot = root_;
    }

    function settle(uint256[2] calldata a, uint256[2][2] calldata b, uint256[2] calldata c, uint256[11] calldata input)
        external
    {
        require(input[0] == block.chainid, "wrong chain");
        require(input[1] == uint160(address(this)), "wrong contract");
        require(input[2] == verifierVersion, "wrong verifier version");
        require(input[3] == asset, "wrong asset");
        require(input[6] == acceptedRoot, "unknown root");
        require(block.timestamp < input[10], "expired");
        require(!consumedDeals[input[5]], "deal consumed");
        require(!consumedNotes[input[7]], "note consumed");
        require(verifier.verifyProof(a, b, c, input), "invalid proof");

        consumedDeals[input[5]] = true;
        consumedNotes[input[7]] = true;
        outputCommitments.push(input[8]);
        outputCommitments.push(input[9]);
        emit TransferAccepted(input[4], input[5], input[7], input[8], input[9]);
    }
}
