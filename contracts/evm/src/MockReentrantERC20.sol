// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.24;

/// @title MockReentrantERC20
/// @notice A token that attempts a reentrant call during `transferFrom`.
/// @dev Used to prove the settlement contract's replay state is written before any external
///      call, so a token cannot re-enter `settle` and pay the same deal twice.
contract MockReentrantERC20 {
    string public name = "Reentrant";
    string public symbol = "RE";
    uint8 public decimals = 18;
    uint256 public totalSupply;

    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    address public reentryTarget;
    bytes public reentryCalldata;
    bool private reentered;

    event Transfer(address indexed from, address indexed to, uint256 value);

    function arm(address target, bytes calldata data) external {
        reentryTarget = target;
        reentryCalldata = data;
    }

    function mint(address to, uint256 value) external {
        totalSupply += value;
        balanceOf[to] += value;
        emit Transfer(address(0), to, value);
    }

    function approve(address spender, uint256 value) external returns (bool) {
        allowance[msg.sender][spender] = value;
        return true;
    }

    function transfer(address to, uint256 value) external returns (bool) {
        _move(msg.sender, to, value);
        return true;
    }

    function transferFrom(address from, address to, uint256 value) external returns (bool) {
        uint256 allowed = allowance[from][msg.sender];
        require(allowed >= value, "allowance");
        if (allowed != type(uint256).max) {
            allowance[from][msg.sender] = allowed - value;
        }
        _move(from, to, value);

        // Attempt the armed reentrant call once. Its revert is bubbled, so a reentrant attempt
        // that fails also reverts the outer settlement.
        if (reentryTarget != address(0) && !reentered) {
            reentered = true;
            (bool ok,) = reentryTarget.call(reentryCalldata);
            require(ok, "reentry reverted");
        }
        return true;
    }

    function _move(address from, address to, uint256 value) private {
        require(balanceOf[from] >= value, "balance");
        balanceOf[from] -= value;
        balanceOf[to] += value;
        emit Transfer(from, to, value);
    }
}
