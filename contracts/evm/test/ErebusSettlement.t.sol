// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.24;

import {TestBase} from "./TestBase.sol";
import {TermsLib} from "./TermsLib.sol";
import {ErebusHarness} from "./ErebusHarness.sol";
import {ErebusSettlement} from "../src/ErebusSettlement.sol";
import {MockERC20} from "../src/MockERC20.sol";
import {MockReentrantERC20} from "../src/MockReentrantERC20.sol";

/// @title ErebusSettlementTest
/// @notice Behavioural and adversarial tests for the public-bound settlement contract.
contract ErebusSettlementTest is TestBase {
    MockERC20 internal token;
    ErebusSettlement internal settlement;
    ErebusHarness internal harness;

    uint256 internal constant BUYER_PK = 0xA11CE;
    uint256 internal constant SELLER_PK = 0x5E11E2;
    address internal buyer;
    address internal seller;
    address internal paymentRecipient = address(0xBEEF);
    address internal feeRecipient = address(0xFEE);
    bytes32 internal blinding = keccak256("erebus-test-blinding");

    uint128 internal constant AMOUNT = 1_000_000;
    uint128 internal constant FEE = 10_000;

    function setUp() public {
        token = new MockERC20("Test Token", "TST");
        settlement = new ErebusSettlement(block.chainid, 1);
        harness = new ErebusHarness();
        buyer = vm.addr(BUYER_PK);
        seller = vm.addr(SELLER_PK);
    }

    // ---------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------

    function defaultData(uint128 amount, uint128 fee, address recipient, bool forceFeeRecipient)
        internal
        view
        returns (TermsLib.Data memory data)
    {
        return defaultDataForToken(amount, fee, recipient, address(token), forceFeeRecipient);
    }

    function defaultDataWithToken(uint128 amount, uint128 fee, address recipient, address tokenAddress)
        internal
        view
        returns (TermsLib.Data memory data)
    {
        return defaultDataForToken(amount, fee, recipient, tokenAddress, false);
    }

    function defaultDataForToken(
        uint128 amount,
        uint128 fee,
        address recipient,
        address tokenAddress,
        bool forceFeeRecipient
    ) internal view returns (TermsLib.Data memory data) {
        data.protocolVersion = 1;
        data.suiteId = 1;
        data.namespace = "eip155:31337";
        data.hasSettlementContract = true;
        data.settlementContract = address(settlement);
        data.hasPool = false;
        data.verifierVersion = 1;
        data.dealId = bytes16(uint128(0x1234));
        data.revision = 1;
        data.transcriptRoot = bytes32(uint256(0x5678));
        data.buyer = buyer;
        data.seller = seller;
        data.paymentRecipient = recipient;
        data.asset = string.concat("eip155:31337/erc20:0x", lowercaseHexString(tokenAddress));
        data.amount = amount;
        data.expiry = uint64(block.timestamp + 1 days);
        data.fee = fee;
        data.hasFeeRecipient = forceFeeRecipient || fee != 0;
        data.feeRecipient = feeRecipient;
        data.settlementMode = 1;
        data.requiredGuarantees = 0x4;
        data.settlementNonce = bytes32(uint256(0x9999));
        data.resource = "gpu.h100.hour";
        data.quantity = 500;
        data.unit = "gpu-hour";
        data.accessRecipient = buyer;
        data.deliveryDeadline = uint64(block.timestamp + 2 days);
        data.fulfillmentMethod = "http-access";
        data.fulfillmentDigest = bytes32(0);
    }

    function baseTerms(uint128 amount, uint128 fee) internal view returns (bytes memory) {
        return TermsLib.encode(defaultData(amount, fee, paymentRecipient, false));
    }

    function sign(uint256 privateKey, bytes32 digest) internal pure returns (bytes memory) {
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(privateKey, digest);
        return abi.encodePacked(r, s, v - 27);
    }

    function authorize(bytes memory terms, uint256 privateKey, uint8 role) internal view returns (bytes memory) {
        return sign(privateKey, harness.authorizationDigest(terms, role, blinding));
    }

    function fund(uint256 total) internal {
        token.mint(buyer, total);
        vm.prank(buyer);
        token.approve(address(settlement), total);
    }

    /// A well-formed but meaningless signature, for cases that must fail before verification.
    function dummySignature() internal pure returns (bytes memory) {
        return new bytes(65);
    }

    function truncate(bytes memory data) internal pure returns (bytes memory out) {
        require(data.length > 0, "empty");
        out = new bytes(data.length - 1);
        for (uint256 i = 0; i < out.length; ++i) {
            out[i] = data[i];
        }
    }

    function uncheckedText(bytes memory encoded) internal pure returns (string memory text) {
        assembly {
            text := encoded
        }
    }

    // ---------------------------------------------------------------------
    // Happy path
    // ---------------------------------------------------------------------

    function test_settles_once_and_pays_exactly() public {
        bytes memory terms = baseTerms(AMOUNT, FEE);
        fund(uint256(AMOUNT) + FEE);
        bytes memory buyerSignature = authorize(terms, BUYER_PK, 1);
        bytes memory sellerSignature = authorize(terms, SELLER_PK, 2);
        bytes32 expectedCommitment = harness.commitment(terms, blinding);
        bytes32 expectedNullifier = harness.dealNullifier(terms);

        (bytes32 commitment, bytes32 nullifier) =
            settlement.settle(terms, blinding, buyerSignature, sellerSignature, address(token));

        assertEq(commitment, expectedCommitment);
        assertEq(nullifier, expectedNullifier);
        assertTrue(settlement.consumedDeals(nullifier));
        assertEq(token.balanceOf(paymentRecipient), AMOUNT);
        assertEq(token.balanceOf(feeRecipient), FEE);
        assertEq(token.balanceOf(buyer), 0);
        assertEq(settlement.computeCommitment(terms, blinding), expectedCommitment);
        assertEq(settlement.computeDealNullifier(terms), expectedNullifier);
    }

    function test_a_relayer_can_submit_for_the_buyer() public {
        bytes memory terms = baseTerms(AMOUNT, 0);
        fund(AMOUNT);
        vm.prank(address(0xDEAD));
        settlement.settle(
            terms, blinding, authorize(terms, BUYER_PK, 1), authorize(terms, SELLER_PK, 2), address(token)
        );
        assertEq(token.balanceOf(paymentRecipient), AMOUNT);
    }

    function test_constructor_rejects_a_different_live_chain() public {
        vm.expectRevert(ErebusSettlement.ChainIdMismatch.selector);
        new ErebusSettlement(block.chainid + 1, 1);
    }

    // ---------------------------------------------------------------------
    // Adversarial
    // ---------------------------------------------------------------------

    function test_replay_is_rejected() public {
        bytes memory terms = baseTerms(AMOUNT, 0);
        fund(AMOUNT);
        bytes memory buyerSignature = authorize(terms, BUYER_PK, 1);
        bytes memory sellerSignature = authorize(terms, SELLER_PK, 2);
        settlement.settle(terms, blinding, buyerSignature, sellerSignature, address(token));

        vm.expectRevert(ErebusSettlement.DealAlreadySettled.selector);
        settlement.settle(terms, blinding, buyerSignature, sellerSignature, address(token));
    }

    function test_changed_amount_fails() public {
        bytes memory signedTerms = baseTerms(AMOUNT, 0);
        bytes memory buyerSignature = authorize(signedTerms, BUYER_PK, 1);
        bytes memory sellerSignature = authorize(signedTerms, SELLER_PK, 2);
        fund(uint256(AMOUNT) + 1);

        bytes memory mutated = baseTerms(AMOUNT + 1, 0);
        vm.expectRevert(ErebusSettlement.BadSignature.selector);
        settlement.settle(mutated, blinding, buyerSignature, sellerSignature, address(token));
    }

    function test_changed_payment_recipient_fails() public {
        bytes memory signedTerms = baseTerms(AMOUNT, 0);
        bytes memory buyerSignature = authorize(signedTerms, BUYER_PK, 1);
        bytes memory sellerSignature = authorize(signedTerms, SELLER_PK, 2);
        fund(AMOUNT);

        bytes memory mutated = TermsLib.encode(defaultData(AMOUNT, 0, address(0xBAD), false));
        vm.expectRevert(ErebusSettlement.BadSignature.selector);
        settlement.settle(mutated, blinding, buyerSignature, sellerSignature, address(token));
    }

    function test_swapped_authorizations_fail() public {
        bytes memory terms = baseTerms(AMOUNT, 0);
        fund(AMOUNT);
        bytes memory sellerSignature = authorize(terms, SELLER_PK, 2);
        bytes memory buyerSignature = authorize(terms, BUYER_PK, 1);
        vm.expectRevert(ErebusSettlement.BadSignature.selector);
        settlement.settle(terms, blinding, sellerSignature, buyerSignature, address(token));
    }

    function test_wrong_token_fails() public {
        MockERC20 other = new MockERC20("Other", "OTH");
        bytes memory terms = baseTerms(AMOUNT, 0);
        fund(AMOUNT);
        bytes memory buyerSignature = authorize(terms, BUYER_PK, 1);
        bytes memory sellerSignature = authorize(terms, SELLER_PK, 2);
        vm.expectRevert(ErebusSettlement.AssetMismatch.selector);
        settlement.settle(terms, blinding, buyerSignature, sellerSignature, address(other));
    }

    function test_wrong_namespace_fails() public {
        TermsLib.Data memory data = defaultData(AMOUNT, 0, paymentRecipient, false);
        data.namespace = "eip155:1";
        data.asset = string.concat("eip155:1/erc20:0x", lowercaseHexString(address(token)));
        bytes memory terms = TermsLib.encode(data);
        fund(AMOUNT);
        bytes memory buyerSignature = authorize(terms, BUYER_PK, 1);
        bytes memory sellerSignature = authorize(terms, SELLER_PK, 2);
        vm.expectRevert(ErebusSettlement.DomainMismatch.selector);
        settlement.settle(terms, blinding, buyerSignature, sellerSignature, address(token));
    }

    function test_runtime_chain_change_fails() public {
        bytes memory terms = baseTerms(AMOUNT, 0);
        fund(AMOUNT);
        vm.chainId(block.chainid + 1);
        vm.expectRevert(ErebusSettlement.ChainIdMismatch.selector);
        settlement.settle(terms, blinding, dummySignature(), dummySignature(), address(token));
    }

    function test_wrong_settlement_contract_fails() public {
        ErebusSettlement other = new ErebusSettlement(block.chainid, 1);
        bytes memory terms = baseTerms(AMOUNT, 0);
        fund(AMOUNT);
        bytes memory buyerSignature = authorize(terms, BUYER_PK, 1);
        bytes memory sellerSignature = authorize(terms, SELLER_PK, 2);
        vm.expectRevert(ErebusSettlement.DomainMismatch.selector);
        other.settle(terms, blinding, buyerSignature, sellerSignature, address(token));
    }

    function test_wrong_verifier_version_fails() public {
        ErebusSettlement other = new ErebusSettlement(block.chainid, 2);
        bytes memory terms = baseTerms(AMOUNT, 0);
        fund(AMOUNT);
        bytes memory buyerSignature = authorize(terms, BUYER_PK, 1);
        bytes memory sellerSignature = authorize(terms, SELLER_PK, 2);
        vm.expectRevert(ErebusSettlement.DomainMismatch.selector);
        other.settle(terms, blinding, buyerSignature, sellerSignature, address(token));
    }

    function test_expired_agreement_fails() public {
        bytes memory terms = baseTerms(AMOUNT, 0);
        fund(AMOUNT);
        bytes memory buyerSignature = authorize(terms, BUYER_PK, 1);
        bytes memory sellerSignature = authorize(terms, SELLER_PK, 2);
        vm.warp(block.timestamp + 1 days + 1);
        vm.expectRevert(ErebusSettlement.Expired.selector);
        settlement.settle(terms, blinding, buyerSignature, sellerSignature, address(token));
    }

    function test_fee_recipient_without_fee_fails() public {
        bytes memory terms = TermsLib.encode(defaultData(AMOUNT, 0, paymentRecipient, true));
        fund(AMOUNT);
        vm.expectRevert();
        settlement.settle(terms, blinding, dummySignature(), dummySignature(), address(token));
    }

    function test_unknown_guarantee_bits_fail() public {
        TermsLib.Data memory data = defaultData(AMOUNT, 0, paymentRecipient, false);
        data.requiredGuarantees = 0x100;
        bytes memory terms = TermsLib.encode(data);
        fund(AMOUNT);
        vm.expectRevert();
        settlement.settle(terms, blinding, dummySignature(), dummySignature(), address(token));
    }

    function test_unsupported_scoped_disclosure_guarantee_fails() public {
        TermsLib.Data memory data = defaultData(AMOUNT, 0, paymentRecipient, false);
        data.requiredGuarantees = 0xC;
        bytes memory terms = TermsLib.encode(data);
        fund(AMOUNT);
        vm.expectRevert();
        settlement.settle(terms, blinding, dummySignature(), dummySignature(), address(token));
    }

    function test_invalid_utf8_text_fails() public view {
        bytes[] memory invalid = new bytes[](7);
        invalid[0] = hex"ff"; // invalid leading byte
        invalid[1] = hex"80"; // stray continuation byte
        invalid[2] = hex"c0af"; // overlong two-byte form
        invalid[3] = hex"e08080"; // overlong three-byte form
        invalid[4] = hex"eda080"; // UTF-16 surrogate
        invalid[5] = hex"f4908080"; // above U+10FFFF
        invalid[6] = hex"f09f92"; // truncated four-byte form
        for (uint256 i = 0; i < invalid.length; ++i) {
            TermsLib.Data memory data = defaultData(AMOUNT, 0, paymentRecipient, false);
            data.resource = uncheckedText(invalid[i]);
            (bool ok,) = address(harness)
                .staticcall(abi.encodeWithSelector(ErebusHarness.decodeOk.selector, TermsLib.encode(data)));
            require(!ok, string.concat("accepted invalid utf-8 case ", toString(i)));
        }
    }

    function test_truncated_terms_fail() public {
        bytes memory terms = truncate(baseTerms(AMOUNT, 0));
        fund(AMOUNT);
        vm.expectRevert();
        settlement.settle(terms, blinding, dummySignature(), dummySignature(), address(token));
    }

    function test_trailing_bytes_fail() public {
        bytes memory terms = abi.encodePacked(baseTerms(AMOUNT, 0), uint8(0));
        fund(AMOUNT);
        vm.expectRevert();
        settlement.settle(terms, blinding, dummySignature(), dummySignature(), address(token));
    }

    function test_fee_on_transfer_token_is_rejected() public {
        token.setFeeOnTransfer(true);
        bytes memory terms = baseTerms(AMOUNT, 0);
        fund(AMOUNT);
        bytes memory buyerSignature = authorize(terms, BUYER_PK, 1);
        bytes memory sellerSignature = authorize(terms, SELLER_PK, 2);
        vm.expectRevert(ErebusSettlement.TransferFailed.selector);
        settlement.settle(terms, blinding, buyerSignature, sellerSignature, address(token));
    }

    function test_insufficient_allowance_fails() public {
        token.mint(buyer, AMOUNT);
        bytes memory terms = baseTerms(AMOUNT, 0);
        bytes memory buyerSignature = authorize(terms, BUYER_PK, 1);
        bytes memory sellerSignature = authorize(terms, SELLER_PK, 2);
        vm.expectRevert(ErebusSettlement.TransferFailed.selector);
        settlement.settle(terms, blinding, buyerSignature, sellerSignature, address(token));
    }

    function test_a_reentrant_token_cannot_settle_twice() public {
        MockReentrantERC20 reentrant = new MockReentrantERC20();
        reentrant.mint(buyer, AMOUNT);
        vm.prank(buyer);
        reentrant.approve(address(settlement), AMOUNT);

        bytes memory terms = TermsLib.encode(defaultDataWithToken(AMOUNT, 0, paymentRecipient, address(reentrant)));
        bytes memory buyerSignature = authorize(terms, BUYER_PK, 1);
        bytes memory sellerSignature = authorize(terms, SELLER_PK, 2);
        bytes32 nullifier = harness.dealNullifier(terms);

        // The token re-enters `settle` with the same authorized arguments during its own
        // transfer. The replay state is written before the external call, so the nested attempt
        // reverts, the token bubbles it, and the whole transaction reverts: no payment, no
        // consumed deal.
        reentrant.arm(
            address(settlement),
            abi.encodeWithSelector(
                ErebusSettlement.settle.selector, terms, blinding, buyerSignature, sellerSignature, address(reentrant)
            )
        );
        vm.expectRevert();
        settlement.settle(terms, blinding, buyerSignature, sellerSignature, address(reentrant));
        assertEq(reentrant.balanceOf(paymentRecipient), 0);
        assertFalse(settlement.consumedDeals(nullifier));
    }

    function test_a_reverted_settlement_does_not_consume_the_deal() public {
        token.mint(buyer, AMOUNT);
        bytes memory terms = baseTerms(AMOUNT, 0);
        bytes32 nullifier = harness.dealNullifier(terms);
        bytes memory buyerSignature = authorize(terms, BUYER_PK, 1);
        bytes memory sellerSignature = authorize(terms, SELLER_PK, 2);
        vm.expectRevert(ErebusSettlement.TransferFailed.selector);
        settlement.settle(terms, blinding, buyerSignature, sellerSignature, address(token));
        assertFalse(settlement.consumedDeals(nullifier));
    }
}
