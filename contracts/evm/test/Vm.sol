// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.24;

/// @title Vm
/// @notice The Foundry cheatcode interface, restricted to the cheatcodes these tests use.
/// @dev Declared locally so the test suite needs no forge-std dependency and no git submodule.
interface Vm {
    function readFile(string calldata path) external view returns (string memory);

    function parseJsonString(string calldata json, string calldata key) external pure returns (string memory);

    function addr(uint256 privateKey) external pure returns (address);

    function sign(uint256 privateKey, bytes32 digest) external pure returns (uint8 v, bytes32 r, bytes32 s);

    function prank(address sender) external;

    function warp(uint256 timestamp) external;

    function chainId(uint256 newChainId) external;

    function expectRevert() external;

    function expectRevert(bytes4 selector) external;

    function label(address account, string calldata newLabel) external;
}
