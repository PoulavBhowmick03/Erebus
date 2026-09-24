// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.24;

import {Vm} from "./Vm.sol";

/// @title TestBase
/// @notice Cheatcode handle, assertions, and encoding helpers for the settlement tests.
abstract contract TestBase {
    Vm internal constant vm = Vm(0x7109709ECfa91a80626fF3989D68f67F5b1DD12D);

    function assertTrue(bool value) internal pure {
        require(value, "assertTrue failed");
    }

    function assertFalse(bool value) internal pure {
        require(!value, "assertFalse failed");
    }

    function assertEq(uint256 left, uint256 right) internal pure {
        require(left == right, "assertEq uint256 failed");
    }

    function assertEq(address left, address right) internal pure {
        require(left == right, "assertEq address failed");
    }

    function assertEq(bytes32 left, bytes32 right) internal pure {
        require(left == right, "assertEq bytes32 failed");
    }

    function assertEq(string memory left, string memory right) internal pure {
        require(keccak256(bytes(left)) == keccak256(bytes(right)), "assertEq string failed");
    }

    /// Hex-decodes a string, accepting an optional `0x` prefix.
    function hexToBytes(string memory input) internal pure returns (bytes memory out) {
        bytes memory characters = bytes(input);
        uint256 start = 0;
        if (characters.length >= 2 && characters[0] == bytes1(0x30) && characters[1] == bytes1(0x78)) {
            start = 2;
        }
        uint256 digits = characters.length - start;
        require(digits % 2 == 0, "odd hex length");
        out = new bytes(digits / 2);
        for (uint256 i = 0; i < out.length; ++i) {
            out[i] = bytes1((nibble(characters[start + 2 * i]) << 4) | nibble(characters[start + 2 * i + 1]));
        }
    }

    function nibble(bytes1 character) internal pure returns (uint8) {
        uint8 value = uint8(character);
        if (value >= 0x30 && value <= 0x39) {
            return value - 0x30;
        }
        if (value >= 0x61 && value <= 0x66) {
            return value - 0x57;
        }
        if (value >= 0x41 && value <= 0x46) {
            return value - 0x37;
        }
        revert("invalid hex digit");
    }

    function toBytes32(bytes memory value) internal pure returns (bytes32 out) {
        require(value.length == 32, "expected 32 bytes");
        for (uint256 i = 0; i < 32; ++i) {
            out = (out << 8) | bytes32(uint256(uint8(value[i])));
        }
    }

    function toAddress(bytes memory value) internal pure returns (address out) {
        require(value.length == 20, "expected 20 bytes");
        uint160 parsed;
        for (uint256 i = 0; i < 20; ++i) {
            parsed = (parsed << 8) | uint160(uint8(value[i]));
        }
        return address(parsed);
    }

    /// Recovers the signer of a 65-byte `r || s || v` signature with a raw recovery id.
    function recover(bytes32 digest, bytes memory signature) internal pure returns (address) {
        require(signature.length == 65, "signature length");
        bytes32 r;
        bytes32 s;
        uint8 v;
        assembly {
            r := mload(add(signature, 32))
            s := mload(add(signature, 64))
            v := byte(0, mload(add(signature, 96)))
        }
        return ecrecover(digest, v + 27, r, s);
    }

    function toString(uint256 value) internal pure returns (string memory) {
        if (value == 0) {
            return "0";
        }
        uint256 digits;
        uint256 remaining = value;
        while (remaining != 0) {
            digits++;
            remaining /= 10;
        }
        bytes memory buffer = new bytes(digits);
        while (value != 0) {
            digits -= 1;
            buffer[digits] = bytes1(uint8(0x30 + (value % 10)));
            value /= 10;
        }
        return string(buffer);
    }

    function lowercaseHexString(address account) internal pure returns (string memory) {
        bytes memory alphabet = "0123456789abcdef";
        bytes20 data = bytes20(account);
        bytes memory out = new bytes(40);
        for (uint256 i = 0; i < 20; ++i) {
            out[2 * i] = alphabet[uint8(data[i] >> 4)];
            out[2 * i + 1] = alphabet[uint8(data[i] & 0x0f)];
        }
        return string(out);
    }
}
