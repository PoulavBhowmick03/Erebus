// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.24;

/// @title TermsLib
/// @notice Test-side canonical encoder for agreement terms.
/// @dev This exists only so the settlement tests can build scenarios with known keys and the
///      deployed contract's address. It is not the trusted side: the decoder is pinned against
///      the Rust vectors in `ErebusVectors.t.sol`, and this encoder is exercised by the happy
///      path, so an encoder bug shows up as a decode revert rather than silently.
library TermsLib {
    struct Data {
        uint16 protocolVersion;
        uint16 suiteId;
        string namespace;
        bool hasSettlementContract;
        address settlementContract;
        bool hasPool;
        address pool;
        uint32 verifierVersion;
        bytes16 dealId;
        uint32 revision;
        bytes32 transcriptRoot;
        address buyer;
        address seller;
        address paymentRecipient;
        string asset;
        uint128 amount;
        uint64 expiry;
        uint128 fee;
        bool hasFeeRecipient;
        address feeRecipient;
        uint8 settlementMode;
        uint32 requiredGuarantees;
        bytes32 settlementNonce;
        string resource;
        uint128 quantity;
        string unit;
        address accessRecipient;
        uint64 deliveryDeadline;
        string fulfillmentMethod;
        bytes32 fulfillmentDigest;
    }

    function encode(Data memory data) internal pure returns (bytes memory out) {
        out = abi.encodePacked(data.protocolVersion, data.suiteId, text(data.namespace));
        out = abi.encodePacked(out, data.hasSettlementContract ? uint8(1) : uint8(0));
        if (data.hasSettlementContract) {
            out = abi.encodePacked(out, bytesField(abi.encodePacked(data.settlementContract)));
        }
        out = abi.encodePacked(out, data.hasPool ? uint8(1) : uint8(0));
        if (data.hasPool) {
            out = abi.encodePacked(out, bytesField(abi.encodePacked(data.pool)));
        }
        out = abi.encodePacked(
            out,
            data.verifierVersion,
            data.dealId,
            data.revision,
            data.transcriptRoot,
            bytesField(abi.encodePacked(data.buyer)),
            bytesField(abi.encodePacked(data.seller)),
            bytesField(abi.encodePacked(data.paymentRecipient)),
            text(data.asset),
            data.amount,
            data.expiry,
            data.fee
        );
        out = abi.encodePacked(out, data.hasFeeRecipient ? uint8(1) : uint8(0));
        if (data.hasFeeRecipient) {
            out = abi.encodePacked(out, bytesField(abi.encodePacked(data.feeRecipient)));
        }
        out = abi.encodePacked(
            out,
            data.settlementMode,
            data.requiredGuarantees,
            data.settlementNonce,
            text(data.resource),
            data.quantity,
            text(data.unit),
            bytesField(abi.encodePacked(data.accessRecipient)),
            data.deliveryDeadline,
            text(data.fulfillmentMethod),
            data.fulfillmentDigest
        );
    }

    function bytesField(bytes memory payload) internal pure returns (bytes memory) {
        return abi.encodePacked(uint16(payload.length), payload);
    }

    function text(string memory value) internal pure returns (bytes memory) {
        return bytesField(bytes(value));
    }
}
