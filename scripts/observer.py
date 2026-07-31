#!/usr/bin/env python3
"""Decodes Erebus negotiation terms from a public Starknet transaction.

This is the adversary. It is given a transaction hash and a public RPC URL and nothing
else — no channel key, no pool key, no handle, no state directory, no viewing grant. If it
prints your offer terms, the salt lane is not confidential.

Run it against the settlement transaction from the first live run:

    python3 scripts/observer.py 0x44289c4cacce0d07f45a6a788313ad341f44f40fd905c181a1e525050384bb7

**It deliberately re-implements the wire decode** rather than importing the SDK's. A checker
that shares code with the thing it checks cannot detect a shared misunderstanding, and the
whole question here is whether an outsider — who would write exactly this, from the public
Cairo — can read the payload.

When the wire is fixed to encrypt the message before fragmentation (F30), this script must
stop producing sensible terms. That makes it the regression test for the fix, not just the
demonstration of the bug.
"""

from __future__ import annotations

import json
import sys
import urllib.request

RPC = "https://starknet-sepolia-rpc.publicnode.com"

# Layout, read off the public contract and sdk/rs/src/wire.rs:20-30. A salt is the high 128
# bits of packed_value; the pool stores it verbatim (privacy.cairo, utils.cairo:288-301).
FLAG_BIT = 1 << 119
PAYLOAD_MASK = FLAG_BIT - 1
NOTES_PER_MESSAGE = 4

# Fields are packed most-significant-first into 400 bits.
FIELDS = [
    ("type", 8),
    ("reply_to", 32),
    ("created_at", 40),
    ("amount", 128),
    ("deadline", 64),
    ("memo_hash", 128),
]
MESSAGE_BITS = sum(bits for _, bits in FIELDS)

MESSAGE_TYPES = {1: "offer", 2: "counter", 3: "accept"}


def rpc(method: str, params: list, url: str) -> dict:
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
    req = urllib.request.Request(
        url,
        data=body.encode(),
        # Some public nodes 403 urllib's default agent. Nothing here is privileged access —
        # that is the point.
        headers={"content-type": "application/json", "user-agent": "curl/8"},
    )
    with urllib.request.urlopen(req, timeout=20) as response:
        return json.load(response)["result"]


def salts_from_calldata(calldata: list[str]) -> list[int]:
    """Every felt whose high half looks like an Erebus salt, in calldata order.

    The filter is the format flag the wire pins at bit 119 — which is what makes Erebus
    traffic self-identifying to an observer, independently of whether the payload is
    readable.
    """
    found = []
    for felt in calldata:
        salt = int(felt, 16) >> 128
        if salt & FLAG_BIT and salt < (1 << 120) and salt not in found:
            found.append(salt)
    return found


def decode(salts: list[int]) -> dict:
    """Chunk 0 holds the least significant payload bits."""
    message = 0
    for index, salt in enumerate(salts[:NOTES_PER_MESSAGE]):
        message |= (salt & PAYLOAD_MASK) << (119 * index)

    out, offset = {}, MESSAGE_BITS
    for name, bits in FIELDS:
        offset -= bits
        out[name] = (message >> offset) & ((1 << bits) - 1)
    return out


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    tx_hash, url = sys.argv[1], (sys.argv[2] if len(sys.argv) > 2 else RPC)

    tx = rpc("starknet_getTransactionByHash", [tx_hash], url)
    calldata = tx.get("calldata", [])
    salts = salts_from_calldata(calldata)

    print(f"transaction : {tx_hash}")
    print(f"calldata    : {len(calldata)} felts, entirely public")
    print(f"erebus salts: {len(salts)} carrying the bit-119 format flag\n")

    if len(salts) < NOTES_PER_MESSAGE:
        print("no complete message in this transaction")
        return 1

    terms = decode(salts)
    kind = MESSAGE_TYPES.get(terms["type"])

    # Plausibility, so the script reports a verdict instead of leaving you to eyeball a
    # number. Wire v2 encrypts the payload, so these bits are ciphertext and the decode
    # produces noise that fails every one of these.
    readable = (
        kind is not None
        and 1_600_000_000 < terms["created_at"] < 2_600_000_000
        and 1_600_000_000 < terms["deadline"] < 2_600_000_000
    )

    if not readable:
        print("  payload does not decode to a valid message")
        print(f"    type {terms['type']}, created_at {terms['created_at']}, "
              f"deadline {terms['deadline']}")
        print("\nContent is confidential (F30 closed).")
        print(f"Traffic is not: {len(salts)} salts were located by their bit-119 format "
              "flag without any key, which is F31.")
        return 1

    print(f"  message type : {kind}")
    print(f"  amount       : {terms['amount']}  ({terms['amount'] / 1e18:g} tokens)")
    print(f"  deadline     : {terms['deadline']}")
    print(f"  memo_hash    : {terms['memo_hash']:#x}")
    print(f"  reply_to     : {terms['reply_to']}")
    print(f"  created_at   : {terms['created_at']}")
    print("\nRecovered with no key material of any kind.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
