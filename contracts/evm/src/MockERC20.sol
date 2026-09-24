// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.24;

/// @title MockERC20
/// @notice Minimal standard ERC-20 for local settlement tests, with an optional transfer fee.
/// @dev The fee mode exists only so a test can prove the settlement contract rejects
///      fee-on-transfer tokens: the recipient must receive exactly the authorized amount.
contract MockERC20 {
    string public name;
    string public symbol;
    uint8 public decimals = 18;
    uint256 public totalSupply;
    bool public feeOnTransfer;

    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    event Transfer(address indexed from, address indexed to, uint256 value);
    event Approval(address indexed owner, address indexed spender, uint256 value);

    constructor(string memory tokenName, string memory tokenSymbol) {
        name = tokenName;
        symbol = tokenSymbol;
    }

    function setFeeOnTransfer(bool enabled) external {
        feeOnTransfer = enabled;
    }

    function mint(address to, uint256 value) external {
        totalSupply += value;
        balanceOf[to] += value;
        emit Transfer(address(0), to, value);
    }

    function approve(address spender, uint256 value) external returns (bool) {
        allowance[msg.sender][spender] = value;
        emit Approval(msg.sender, spender, value);
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
        return true;
    }

    function _move(address from, address to, uint256 value) private {
        require(balanceOf[from] >= value, "balance");
        balanceOf[from] -= value;
        if (feeOnTransfer && value > 0) {
            balanceOf[to] += value - 1;
            totalSupply -= 1;
            emit Transfer(from, to, value - 1);
        } else {
            balanceOf[to] += value;
            emit Transfer(from, to, value);
        }
    }
}
