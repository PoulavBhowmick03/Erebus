// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.24;

import {ErebusCodec} from "./ErebusCodec.sol";

/// @title ErebusSettlement
/// @notice Public-bound settlement for one authorized Erebus agreement (Metropolis M3).
/// @dev This contract is the on-chain half of the settlement verifier. It does not trust the
///      caller or the relayer: it opens the committed agreement from the supplied terms and
///      blinding, recomputes the commitment, verifies both role authorizations, enforces the
///      deployment domain, expiry, and deal replay, and only then moves tokens. Any mutation of
///      amount, recipient, asset, domain, expiry, or authorization changes the commitment or the
///      digest and the call reverts.
///
///      Payment details (amount, asset, payer, recipient, fee) are public in this mode. That is
///      the intended trade for M3; it is not shielded settlement and must not be presented as
///      such. The guarantees this contract actually enforces are returned in the receipt by the
///      Rust adapter.
contract ErebusSettlement {
    /// A supplied authorization did not verify.
    error BadSignature();
    /// The agreement did not match this deployment.
    error DomainMismatch();
    /// The agreement's expiry has passed.
    error Expired();
    /// The deal was already consumed.
    error DealAlreadySettled();
    /// The asset identifier did not resolve to the supplied token.
    error AssetMismatch();
    /// The token is unusable, the transfer failed, or the amount received was not exact.
    error TransferFailed();
    /// A required address was zero.
    error ZeroAddress();
    /// The deployment was configured for a different live EVM chain.
    error ChainIdMismatch();

    /// secp256k1 group order divided by two; signatures above this are rejected as malleable.
    uint256 private constant LOW_S_MAX = 0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0;

    /// Chain namespace this deployment serves, for example `eip155:10143`.
    string public chainNamespace;
    /// Numeric EVM chain id this deployment is bound to.
    uint256 public immutable chainId;
    /// Verifier version bound into every accepted agreement.
    uint32 public immutable verifierVersion;
    /// Consumed deal identities. The first valid revision to settle consumes the deal.
    mapping(bytes32 => bool) public consumedDeals;

    /// Emitted once per settled deal.
    event DealSettled(
        bytes32 indexed commitment,
        bytes32 indexed dealNullifier,
        address indexed buyer,
        address paymentRecipient,
        address token,
        uint256 amount,
        uint256 fee
    );

    constructor(uint256 expectedChainId, uint32 expectedVerifierVersion) {
        if (expectedChainId != block.chainid) {
            revert ChainIdMismatch();
        }
        chainId = expectedChainId;
        chainNamespace = string.concat("eip155:", decimal(expectedChainId));
        verifierVersion = expectedVerifierVersion;
    }

    /// Settles one authorized agreement and transfers the payment atomically.
    ///
    /// Callable by anyone: the payer is the decoded buyer key, which must have approved this
    /// contract. A relayer can submit, and cannot redirect value because every payment field is
    /// bound to the signed commitment.
    function settle(
        bytes calldata terms,
        bytes32 blinding,
        bytes calldata buyerSignature,
        bytes calldata sellerSignature,
        address token
    ) external returns (bytes32 commitment, bytes32 dealNullifier) {
        if (block.chainid != chainId) {
            revert ChainIdMismatch();
        }
        ErebusCodec.Terms memory decoded = ErebusCodec.decode(terms);

        // The agreement must authorize exactly this deployment.
        if (keccak256(bytes(decoded.namespace)) != keccak256(bytes(chainNamespace))) {
            revert DomainMismatch();
        }
        if (decoded.settlementContract != address(this)) {
            revert DomainMismatch();
        }
        if (decoded.verifierVersion != verifierVersion) {
            revert DomainMismatch();
        }
        if (block.timestamp >= decoded.expiry) {
            revert Expired();
        }

        bytes memory domain = ErebusCodec.domainSlice(terms, decoded);
        commitment = ErebusCodec.commitment(terms, blinding);
        verifyAuthorization(
            ErebusCodec.authorizationDigest(domain, ErebusCodec.ROLE_BUYER, commitment), buyerSignature, decoded.buyer
        );
        verifyAuthorization(
            ErebusCodec.authorizationDigest(domain, ErebusCodec.ROLE_SELLER, commitment),
            sellerSignature,
            decoded.seller
        );

        dealNullifier = ErebusCodec.dealNullifier(domain, decoded.buyer, decoded.settlementNonce);
        if (consumedDeals[dealNullifier]) {
            revert DealAlreadySettled();
        }

        if (token == address(0) || token.code.length == 0) {
            revert ZeroAddress();
        }
        address expectedToken = ErebusCodec.parseAssetToken(decoded.asset, chainNamespace);
        if (expectedToken != token) {
            revert AssetMismatch();
        }

        consumedDeals[dealNullifier] = true;
        exactTransferFrom(token, decoded.buyer, decoded.paymentRecipient, decoded.amount);
        if (decoded.fee != 0) {
            exactTransferFrom(token, decoded.buyer, decoded.feeRecipient, decoded.fee);
        }

        emit DealSettled(
            commitment, dealNullifier, decoded.buyer, decoded.paymentRecipient, token, decoded.amount, decoded.fee
        );
    }

    /// Recomputes the commitment a set of terms and a blinding produce.
    function computeCommitment(bytes calldata terms, bytes32 blinding) external pure returns (bytes32) {
        return ErebusCodec.commitment(terms, blinding);
    }

    /// Recomputes the consumed deal identity a set of terms produces.
    function computeDealNullifier(bytes calldata terms) external pure returns (bytes32) {
        ErebusCodec.Terms memory decoded = ErebusCodec.decode(terms);
        bytes memory domain = ErebusCodec.domainSlice(terms, decoded);
        return ErebusCodec.dealNullifier(domain, decoded.buyer, decoded.settlementNonce);
    }

    /// Verifies one 65-byte `r || s || v` authorization with a raw recovery id.
    function verifyAuthorization(bytes32 digest, bytes calldata signature, address expected) private pure {
        if (signature.length != 65) {
            revert BadSignature();
        }
        bytes32 r;
        bytes32 s;
        uint8 v;
        // Standard ecrecover split. `v` is the raw recovery id; the +27 adjustment happens below.
        assembly {
            r := calldataload(signature.offset)
            s := calldataload(add(signature.offset, 32))
            v := byte(0, calldataload(add(signature.offset, 64)))
        }
        if (v > 1) {
            revert BadSignature();
        }
        if (uint256(s) == 0 || uint256(s) > LOW_S_MAX) {
            revert BadSignature();
        }
        if (uint256(r) == 0) {
            revert BadSignature();
        }
        address recovered = ecrecover(digest, v + 27, r, s);
        if (recovered == address(0) || recovered != expected) {
            revert BadSignature();
        }
    }

    /// Transfers `value` and requires the recipient balance to increase by exactly `value`.
    ///
    /// Accepts the two standard ERC-20 return conventions (empty or `true`) and rejects
    /// fee-on-transfer and rebasing tokens: a payment that arrives short is not the authorized
    /// payment.
    function exactTransferFrom(address token, address from, address to, uint256 value) private {
        uint256 balanceBefore = IERC20(token).balanceOf(to);
        (bool ok, bytes memory returndata) =
            token.call(abi.encodeWithSelector(IERC20.transferFrom.selector, from, to, value));
        if (!ok) {
            revert TransferFailed();
        }
        if (returndata.length != 0) {
            if (returndata.length < 32 || !abi.decode(returndata, (bool))) {
                revert TransferFailed();
            }
        }
        if (IERC20(token).balanceOf(to) - balanceBefore != value) {
            revert TransferFailed();
        }
    }

    function decimal(uint256 value) private pure returns (string memory) {
        if (value == 0) {
            return "0";
        }
        uint256 digits;
        uint256 remaining = value;
        while (remaining != 0) {
            ++digits;
            remaining /= 10;
        }
        bytes memory encoded = new bytes(digits);
        while (value != 0) {
            --digits;
            // The modulo result is in the inclusive range 0..9.
            // forge-lint: disable-next-line(unsafe-typecast)
            encoded[digits] = bytes1(uint8(48 + value % 10));
            value /= 10;
        }
        return string(encoded);
    }
}

/// The subset of ERC-20 the settlement contract needs.
interface IERC20 {
    function balanceOf(address account) external view returns (uint256);

    function transferFrom(address from, address to, uint256 value) external returns (bool);
}
