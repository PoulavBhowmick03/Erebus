// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.24;

interface IERC20PoolAsset {
    function balanceOf(address account) external view returns (uint256);
    function transfer(address recipient, uint256 amount) external returns (bool);
    function transferFrom(address sender, address recipient, uint256 amount) external returns (bool);
}

interface IPoseidonTwo {
    function poseidon(uint256[2] calldata inputs) external pure returns (uint256);
}

interface IDepositVerifier {
    function verifyProof(
        uint256[2] calldata a,
        uint256[2][2] calldata b,
        uint256[2] calldata c,
        uint256[6] calldata input
    ) external view returns (bool);
}

interface ITransferVerifierM5 {
    function verifyProof(
        uint256[2] calldata a,
        uint256[2][2] calldata b,
        uint256[2] calldata c,
        uint256[11] calldata input
    ) external view returns (bool);
}

interface IWithdrawVerifier {
    function verifyProof(
        uint256[2] calldata a,
        uint256[2][2] calldata b,
        uint256[2] calldata c,
        uint256[8] calldata input
    ) external view returns (bool);
}

/// @notice Experimental one-asset shielded pool. Its verifiers and setup are deployment-specific.
/// @dev No upgrade authority. Never deploy with M5's test-only Groth16 setup or use with real value.
contract ErebusShieldedPool {
    uint256 public constant TREE_DEPTH = 20;
    uint256 public constant FIELD_MODULUS =
        21888242871839275222246405745257275088548364400416034343698204186575808495617;

    IERC20PoolAsset public immutable asset;
    IPoseidonTwo public immutable poseidonTwo;
    IDepositVerifier public immutable depositVerifier;
    ITransferVerifierM5 public immutable transferVerifier;
    IWithdrawVerifier public immutable withdrawVerifier;
    uint256 public immutable verifierVersion;
    uint256 public nextLeafIndex;
    uint256 public currentRoot;
    uint256[TREE_DEPTH] public zeroHashes;
    uint256[TREE_DEPTH] public filledSubtrees;
    mapping(uint256 => bool) public knownRoots;
    mapping(uint256 => bool) public insertedCommitments;
    mapping(uint256 => bool) public consumedDeals;
    mapping(uint256 => bool) public consumedNotes;
    bool private entered;

    event NoteInserted(uint256 indexed index, uint256 indexed commitment, uint256 root);
    event DealTransferred(
        uint256 indexed dealCommitment, uint256 indexed dealNullifier, uint256 indexed inputNullifier
    );
    event NoteWithdrawn(uint256 indexed nullifier, address indexed recipient, uint256 amount);

    modifier nonReentrant() {
        require(!entered, "reentrant");
        entered = true;
        _;
        entered = false;
    }

    constructor(
        address asset_,
        address poseidonTwo_,
        address depositVerifier_,
        address transferVerifier_,
        address withdrawVerifier_,
        uint256 verifierVersion_
    ) {
        require(asset_.code.length != 0 && poseidonTwo_.code.length != 0, "missing asset or hash code");
        require(
            depositVerifier_.code.length != 0 && transferVerifier_.code.length != 0
                && withdrawVerifier_.code.length != 0,
            "missing verifier code"
        );
        require(verifierVersion_ != 0, "zero version");
        asset = IERC20PoolAsset(asset_);
        poseidonTwo = IPoseidonTwo(poseidonTwo_);
        depositVerifier = IDepositVerifier(depositVerifier_);
        transferVerifier = ITransferVerifierM5(transferVerifier_);
        withdrawVerifier = IWithdrawVerifier(withdrawVerifier_);
        verifierVersion = verifierVersion_;

        uint256 zero;
        for (uint256 i; i < TREE_DEPTH; ++i) {
            zeroHashes[i] = zero;
            filledSubtrees[i] = zero;
            zero = _parent(zero, zero);
        }
        currentRoot = zero;
        knownRoots[zero] = true;
    }

    /// @notice Locks exactly `amount` tokens for a proved note opening.
    /// @dev Public inputs: chain, pool, version, asset, amount, note commitment.
    function deposit(uint256[2] calldata a, uint256[2][2] calldata b, uint256[2] calldata c, uint256[6] calldata input)
        external
        nonReentrant
    {
        _checkDomain(input[0], input[1], input[2], input[3]);
        uint256 amount = input[4];
        require(amount > 0 && amount <= type(uint128).max, "invalid amount");
        require(_isField(input[5]) && input[5] != 0, "invalid note");
        require(!insertedCommitments[input[5]], "duplicate note");
        require(nextLeafIndex < 2 ** TREE_DEPTH, "tree full");
        require(depositVerifier.verifyProof(a, b, c, input), "invalid deposit proof");

        uint256 beforeBalance = asset.balanceOf(address(this));
        require(asset.transferFrom(msg.sender, address(this), amount), "deposit transfer failed");
        require(asset.balanceOf(address(this)) == beforeBalance + amount, "inexact deposit");
        _insert(input[5]);
    }

    /// @notice Consumes one funded input note and one deal, inserting payment and change notes.
    /// @dev Public inputs match M4 transfer order, with a depth-20 membership path.
    function transferPrivate(
        uint256[2] calldata a,
        uint256[2][2] calldata b,
        uint256[2] calldata c,
        uint256[11] calldata input
    ) external nonReentrant {
        _checkDomain(input[0], input[1], input[2], input[3]);
        require(knownRoots[input[6]], "unknown root");
        require(block.timestamp < input[10], "expired");
        require(!consumedDeals[input[5]] && !consumedNotes[input[7]], "already consumed");
        require(nextLeafIndex <= (2 ** TREE_DEPTH) - 2, "tree full");
        require(_isField(input[4]) && _isField(input[5]) && _isField(input[7]), "invalid identity");
        require(_isField(input[8]) && _isField(input[9]), "invalid output");
        require(input[8] != 0 && input[9] != 0 && input[8] != input[9], "invalid output notes");
        require(!insertedCommitments[input[8]] && !insertedCommitments[input[9]], "duplicate note");
        require(transferVerifier.verifyProof(a, b, c, input), "invalid transfer proof");

        consumedDeals[input[5]] = true;
        consumedNotes[input[7]] = true;
        _insert(input[8]);
        _insert(input[9]);
        emit DealTransferred(input[4], input[5], input[7]);
    }

    /// @notice Unshields one note; the recipient and amount become public.
    /// @dev Public inputs: chain, pool, version, asset, root, note nullifier, recipient, amount.
    function withdraw(uint256[2] calldata a, uint256[2][2] calldata b, uint256[2] calldata c, uint256[8] calldata input)
        external
        nonReentrant
    {
        _checkDomain(input[0], input[1], input[2], input[3]);
        require(knownRoots[input[4]], "unknown root");
        require(!consumedNotes[input[5]], "note consumed");
        require(_isField(input[5]), "invalid nullifier");
        require(input[6] > 0 && input[6] < 1 << 160, "invalid recipient");
        require(input[7] > 0 && input[7] <= type(uint128).max, "invalid amount");
        require(withdrawVerifier.verifyProof(a, b, c, input), "invalid withdraw proof");

        consumedNotes[input[5]] = true;
        address recipient = address(uint160(input[6]));
        uint256 beforePool = asset.balanceOf(address(this));
        uint256 beforeRecipient = asset.balanceOf(recipient);
        require(asset.transfer(recipient, input[7]), "withdraw transfer failed");
        require(asset.balanceOf(address(this)) == beforePool - input[7], "inexact pool debit");
        require(asset.balanceOf(recipient) == beforeRecipient + input[7], "inexact withdrawal");
        emit NoteWithdrawn(input[5], recipient, input[7]);
    }

    function _checkDomain(uint256 chain, uint256 pool, uint256 version, uint256 token) private view {
        require(chain == block.chainid && pool == uint160(address(this)), "wrong deployment");
        require(version == verifierVersion && token == uint160(address(asset)), "wrong version or asset");
    }

    function _isField(uint256 value) private pure returns (bool) {
        return value < FIELD_MODULUS;
    }

    function _parent(uint256 left, uint256 right) private view returns (uint256) {
        uint256[2] memory pair = [left, right];
        return poseidonTwo.poseidon(pair);
    }

    function _insert(uint256 leaf) private {
        insertedCommitments[leaf] = true;
        uint256 index = nextLeafIndex;
        uint256 node = leaf;
        for (uint256 level; level < TREE_DEPTH; ++level) {
            if ((index & 1) == 0) {
                filledSubtrees[level] = node;
                node = _parent(node, zeroHashes[level]);
            } else {
                node = _parent(filledSubtrees[level], node);
            }
            index >>= 1;
        }
        uint256 insertedIndex = nextLeafIndex++;
        currentRoot = node;
        knownRoots[node] = true;
        emit NoteInserted(insertedIndex, leaf, node);
    }
}
