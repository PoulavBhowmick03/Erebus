// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.24;

import {ErebusShieldedPool} from "../src/ErebusShieldedPool.sol";
import {MockERC20} from "../src/MockERC20.sol";
import {TestBase} from "./TestBase.sol";

/// @dev State-machine tests only. A permissive verifier cannot establish circuit soundness.
contract StateOnlyVerifier {
    bool public accepts = true;

    function setAccepts(bool value) external {
        accepts = value;
    }

    function verifyProof(uint256[2] calldata, uint256[2][2] calldata, uint256[2] calldata, uint256[6] calldata)
        external
        view
        returns (bool)
    {
        return accepts;
    }

    function verifyProof(uint256[2] calldata, uint256[2][2] calldata, uint256[2] calldata, uint256[11] calldata)
        external
        view
        returns (bool)
    {
        return accepts;
    }

    function verifyProof(uint256[2] calldata, uint256[2][2] calldata, uint256[2] calldata, uint256[8] calldata)
        external
        view
        returns (bool)
    {
        return accepts;
    }
}

/// @dev A deterministic field hash is enough to exercise the pool's tree bookkeeping.
contract StateOnlyPoseidon {
    uint256 private constant FIELD = 21888242871839275222246405745257275088548364400416034343698204186575808495617;

    function poseidon(uint256[2] calldata input) external pure returns (uint256) {
        return uint256(keccak256(abi.encode(input[0], input[1]))) % FIELD;
    }
}

contract ErebusShieldedPoolTest is TestBase {
    MockERC20 private token;
    ErebusShieldedPool private pool;
    StateOnlyVerifier private verifier;

    function setUp() public {
        token = new MockERC20("Test", "TEST");
        verifier = new StateOnlyVerifier();
        StateOnlyPoseidon hash = new StateOnlyPoseidon();
        pool = new ErebusShieldedPool(
            address(token), address(hash), address(verifier), address(verifier), address(verifier), 2
        );
        token.mint(address(this), 1_000_000);
        token.approve(address(pool), type(uint256).max);
    }

    function _deposit(uint256 amount, uint256 commitment) private {
        uint256[2] memory a;
        uint256[2][2] memory b;
        uint256[2] memory c;
        uint256[6] memory input;
        input[0] = block.chainid;
        input[1] = uint160(address(pool));
        input[2] = 2;
        input[3] = uint160(address(token));
        input[4] = amount;
        input[5] = commitment;
        pool.deposit(a, b, c, input);
    }

    function _transferAtRoot(uint256 deal, uint256 noteNullifier, uint256 payment, uint256 change, uint256 root)
        private
    {
        uint256[2] memory a;
        uint256[2][2] memory b;
        uint256[2] memory c;
        uint256[11] memory input;
        input[0] = block.chainid;
        input[1] = uint160(address(pool));
        input[2] = 2;
        input[3] = uint160(address(token));
        input[4] = deal;
        input[5] = deal + 100;
        input[6] = root;
        input[7] = noteNullifier;
        input[8] = payment;
        input[9] = change;
        input[10] = block.timestamp + 100;
        pool.transferPrivate(a, b, c, input);
    }

    function _transfer(uint256 deal, uint256 noteNullifier, uint256 payment, uint256 change) private {
        _transferAtRoot(deal, noteNullifier, payment, change, pool.currentRoot());
    }

    function _withdraw(uint256 nullifier, address recipient, uint256 amount) private {
        uint256[2] memory a;
        uint256[2][2] memory b;
        uint256[2] memory c;
        uint256[8] memory input;
        input[0] = block.chainid;
        input[1] = uint160(address(pool));
        input[2] = 2;
        input[3] = uint160(address(token));
        input[4] = pool.currentRoot();
        input[5] = nullifier;
        input[6] = uint160(recipient);
        input[7] = amount;
        pool.withdraw(a, b, c, input);
    }

    function attemptDeposit(uint256 amount, uint256 commitment) external {
        _deposit(amount, commitment);
    }

    function attemptTransfer(uint256 deal, uint256 noteNullifier, uint256 payment, uint256 change) external {
        _transfer(deal, noteNullifier, payment, change);
    }

    function attemptWithdraw(uint256 nullifier, address recipient, uint256 amount) external {
        _withdraw(nullifier, recipient, amount);
    }

    function test_deposit_transfer_and_withdraw_update_pool_state_once() public {
        _deposit(150, 11);
        uint256 depositedRoot = pool.currentRoot();
        assertTrue(pool.knownRoots(depositedRoot));
        assertEq(pool.nextLeafIndex(), 1);
        assertEq(token.balanceOf(address(pool)), 150);

        _transfer(21, 31, 41, 51);
        assertEq(pool.nextLeafIndex(), 3);
        assertTrue(pool.consumedDeals(121));
        assertTrue(pool.consumedNotes(31));
        assertTrue(pool.insertedCommitments(41));
        assertTrue(pool.insertedCommitments(51));
        assertTrue(pool.knownRoots(pool.currentRoot()));
        assertEq(token.balanceOf(address(pool)), 150);
        vm.expectRevert();
        this.attemptTransfer(21, 32, 61, 71);
        vm.expectRevert();
        this.attemptTransfer(22, 31, 61, 71);
        vm.expectRevert();
        this.attemptDeposit(1, 41);

        address recipient = address(0xBEEF);
        _withdraw(42, recipient, 70);
        assertEq(token.balanceOf(recipient), 70);
        assertEq(token.balanceOf(address(pool)), 80);
        assertTrue(pool.consumedNotes(42));
        vm.expectRevert();
        this.attemptWithdraw(42, recipient, 1);
    }

    function test_rejected_proof_or_inexact_token_delta_never_consumes_state() public {
        verifier.setAccepts(false);
        vm.expectRevert();
        this.attemptDeposit(150, 11);
        assertEq(pool.nextLeafIndex(), 0);
        verifier.setAccepts(true);
        token.setFeeOnTransfer(true);
        vm.expectRevert();
        this.attemptDeposit(150, 11);
        assertEq(pool.nextLeafIndex(), 0);
        token.setFeeOnTransfer(false);
        _deposit(150, 11);

        verifier.setAccepts(false);
        vm.expectRevert();
        this.attemptTransfer(21, 31, 41, 51);
        assertFalse(pool.consumedDeals(121));
        assertEq(pool.nextLeafIndex(), 1);
        verifier.setAccepts(true);
        _transfer(21, 31, 41, 51);
        token.setFeeOnTransfer(true);
        vm.expectRevert();
        this.attemptWithdraw(42, address(0xBEEF), 70);
        assertFalse(pool.consumedNotes(42));
        assertEq(token.balanceOf(address(pool)), 150);
    }

    function test_multiple_deposits_keep_earlier_root_available_for_one_spend() public {
        _deposit(100, 11);
        uint256 firstRoot = pool.currentRoot();
        _deposit(80, 12);
        uint256 secondRoot = pool.currentRoot();
        assertTrue(firstRoot != secondRoot);
        assertTrue(pool.knownRoots(firstRoot));
        assertEq(pool.nextLeafIndex(), 2);
        assertEq(token.balanceOf(address(pool)), 180);

        _transferAtRoot(21, 31, 41, 51, firstRoot);
        assertEq(pool.nextLeafIndex(), 4);
        assertTrue(pool.consumedDeals(121));
        assertTrue(pool.consumedNotes(31));
        assertEq(token.balanceOf(address(pool)), 180);
        vm.expectRevert();
        this.attemptTransfer(21, 32, 61, 71);
    }

    function testFuzz_exact_vault_delta_for_supported_amounts(uint128 rawDeposit, uint128 rawWithdrawal) public {
        uint256 depositAmount = uint256(rawDeposit % 10_000) + 1;
        uint256 withdrawalAmount = uint256(rawWithdrawal % uint128(depositAmount)) + 1;
        _deposit(depositAmount, 11);
        _withdraw(42, address(0xBEEF), withdrawalAmount);
        assertEq(token.balanceOf(address(pool)), depositAmount - withdrawalAmount);
        assertEq(token.balanceOf(address(0xBEEF)), withdrawalAmount);
        assertEq(pool.nextLeafIndex(), 1);
    }
}
