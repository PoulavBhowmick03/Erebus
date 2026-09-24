// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.24;

import {TestBase} from "./TestBase.sol";
import {ErebusHarness} from "./ErebusHarness.sol";

/// @title ErebusVectorsTest
/// @notice Known-answer test: the Solidity decoder and digests must match the Rust-generated
///         agreement vectors byte-for-byte.
/// @dev This is the strongest correctness signal on the Solidity side. A one-byte decoding
///      difference produces a different commitment and every settlement fails; this test catches
///      that before any chain is involved.
contract ErebusVectorsTest is TestBase {
    ErebusHarness internal harness;
    string internal json;

    function setUp() public {
        harness = new ErebusHarness();
        json = vm.readFile("../../sdk/core/tests/fixtures/agreement-v1-vectors.json");
    }

    function field(uint256 index, string memory path) internal view returns (string memory) {
        return vm.parseJsonString(json, string.concat(".vectors[", toString(index), "].", path));
    }

    function test_vectors_match_rust() public view {
        for (uint256 index = 0; index < 4; ++index) {
            bytes memory terms = hexToBytes(field(index, "expected.canonicalHex"));
            bytes32 blinding = toBytes32(hexToBytes(field(index, "blindingHex")));

            if (index == 2) {
                // Suite 1 with shielded mode is not a public-bound agreement. The decoder must
                // reject the canonical bytes, not translate them silently.
                (bool ok,) = address(harness).staticcall(abi.encodeWithSelector(ErebusHarness.decodeOk.selector, terms));
                assertFalse(ok);
                continue;
            }

            assertTrue(harness.decodeOk(terms));
            assertEq(harness.commitment(terms, blinding), toBytes32(hexToBytes(field(index, "expected.commitmentHex"))));
            assertEq(harness.dealNullifier(terms), toBytes32(hexToBytes(field(index, "expected.dealNullifierHex"))));

            bytes32 buyerDigest = harness.authorizationDigest(terms, 1, blinding);
            bytes32 sellerDigest = harness.authorizationDigest(terms, 2, blinding);
            assertEq(buyerDigest, toBytes32(hexToBytes(field(index, "expected.buyerDigestHex"))));
            assertEq(sellerDigest, toBytes32(hexToBytes(field(index, "expected.sellerDigestHex"))));

            address buyerKey = toAddress(hexToBytes(field(index, "terms.buyerAuthorizationKeyHex")));
            address sellerKey = toAddress(hexToBytes(field(index, "terms.sellerAuthorizationKeyHex")));
            assertEq(recover(buyerDigest, hexToBytes(field(index, "expected.buyerSignatureHex"))), buyerKey);
            assertEq(recover(sellerDigest, hexToBytes(field(index, "expected.sellerSignatureHex"))), sellerKey);
        }
    }

    function test_asset_parsing_is_strict() public view {
        assertEq(
            harness.assetToken("eip155:31337/erc20:0x00000000000000000000000000000000000000aa", "eip155:31337"),
            address(0xaa)
        );
        // Wrong chain, wrong asset namespace, uppercase hex, and a short reference must all fail.
        (bool ok,) = address(harness)
            .staticcall(
                abi.encodeWithSelector(
                    ErebusHarness.assetToken.selector,
                    "eip155:1/erc20:0x00000000000000000000000000000000000000aa",
                    "eip155:31337"
                )
            );
        assertFalse(ok);
        (ok,) = address(harness)
            .staticcall(
                abi.encodeWithSelector(
                    ErebusHarness.assetToken.selector,
                    "eip155:31337/slip44:0x00000000000000000000000000000000000000aa",
                    "eip155:31337"
                )
            );
        assertFalse(ok);
        (ok,) = address(harness)
            .staticcall(
                abi.encodeWithSelector(
                    ErebusHarness.assetToken.selector,
                    "eip155:31337/erc20:0x00000000000000000000000000000000000000AA",
                    "eip155:31337"
                )
            );
        assertFalse(ok);
    }
}
