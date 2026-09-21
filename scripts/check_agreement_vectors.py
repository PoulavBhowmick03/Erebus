"""Independent canonical-byte encoder for M1 fixtures, not a production SDK.

Uses only Python's standard library. Hashes and signatures remain covered by the
Rust suite's external primitive vectors; this check independently pins field order,
widths, optional tags, and length prefixes against the agreement specification.
"""

import json
from pathlib import Path


def uint(value: int | str, width: int) -> bytes:
    return int(value).to_bytes(width, "big")


def blob(value: bytes) -> bytes:
    return uint(len(value), 2) + value


def text(value: str) -> bytes:
    return blob(value.encode("utf-8"))


def key(value: str) -> bytes:
    return blob(bytes.fromhex(value))


def optional_key(value: str | None) -> bytes:
    return b"\x00" if value is None else b"\x01" + key(value)


def encode(terms: dict) -> bytes:
    domain = terms["domain"]
    service = terms["service"]
    bits = {"hidden-amount": 1, "hidden-recipient": 2,
            "agreement-bound-settlement": 4, "scoped-disclosure": 8}
    guarantees = 0
    for name in terms["requiredGuarantees"]:
        guarantees |= bits[name]
    return b"".join([
        uint(terms["protocolVersion"], 2), uint(terms["suiteId"], 2),
        text(domain["namespace"]), optional_key(domain["settlementContractHex"]),
        optional_key(domain["poolHex"]), uint(domain["verifierVersion"], 4),
        bytes.fromhex(terms["dealIdHex"]), uint(terms["revision"], 4),
        bytes.fromhex(terms["transcriptRootHex"]),
        key(terms["buyerAuthorizationKeyHex"]), key(terms["sellerAuthorizationKeyHex"]),
        key(terms["paymentRecipientHex"]), text(terms["asset"]),
        uint(terms["amount"], 16), uint(terms["expiry"], 8), uint(terms["fee"], 16),
        optional_key(terms["feeRecipientHex"]),
        uint({"public_bound": 1, "shielded": 2}[terms["settlementMode"]], 1),
        uint(guarantees, 4), bytes.fromhex(terms["settlementNonceHex"]),
        text(service["resource"]), uint(service["quantity"], 16), text(service["unit"]),
        key(service["accessRecipientHex"]), uint(service["deliveryDeadline"], 8),
        text(service["fulfillmentMethod"]), bytes.fromhex(service["fulfillmentDigestHex"]),
    ])


def main() -> None:
    path = Path(__file__).resolve().parents[1] / "sdk/core/tests/fixtures/agreement-v1-vectors.json"
    vectors = json.loads(path.read_text())["vectors"]
    if len(vectors) != 4:
        raise ValueError("expected three public-bound vectors and one shielded rejection vector")
    for vector in vectors:
        if encode(vector["terms"]).hex() != vector["expected"]["canonicalHex"]:
            raise ValueError(f"canonical encoding mismatch: {vector['name']}")
    print("4 independent canonical encodings match (3 valid, 1 rejected by Rust mode validation)")


if __name__ == "__main__":
    main()
