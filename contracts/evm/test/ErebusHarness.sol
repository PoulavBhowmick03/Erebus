// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.24;

import {ErebusCodec} from "../src/ErebusCodec.sol";

/// @title ErebusHarness
/// @notice External wrappers over the internal codec so tests can use `vm.expectRevert` and
///         `try/catch`, which need an external call boundary.
contract ErebusHarness {
    function decodeOk(bytes memory terms) external pure returns (bool) {
        ErebusCodec.decode(terms);
        return true;
    }

    function commitment(bytes memory terms, bytes32 blinding) external pure returns (bytes32) {
        return ErebusCodec.commitment(terms, blinding);
    }

    function dealNullifier(bytes memory terms) external pure returns (bytes32) {
        ErebusCodec.Terms memory decoded = ErebusCodec.decode(terms);
        bytes memory domain = ErebusCodec.domainSlice(terms, decoded);
        return ErebusCodec.dealNullifier(domain, decoded.buyer, decoded.settlementNonce);
    }

    function authorizationDigest(bytes memory terms, uint8 roleTag, bytes32 blinding) external pure returns (bytes32) {
        ErebusCodec.Terms memory decoded = ErebusCodec.decode(terms);
        bytes memory domain = ErebusCodec.domainSlice(terms, decoded);
        bytes32 dealCommitment = ErebusCodec.commitment(terms, blinding);
        return ErebusCodec.authorizationDigest(domain, roleTag, dealCommitment);
    }

    function assetToken(string memory asset, string memory expectedNamespace) external pure returns (address) {
        return ErebusCodec.parseAssetToken(asset, expectedNamespace);
    }
}
