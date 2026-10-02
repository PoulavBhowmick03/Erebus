#!/usr/bin/env python3
"""Verify a deployed EVM contract against a local artifact, read-only.

It pins the chain's `finalized` block and reads `eth_getCode` by block hash. Immutable
storage slots are masked using the artifact's `deployedBytecode.immutableReferences`, so
the check verifies code identity without rejecting the constructor-configured immutable.
`--verifier-version` additionally reads `verifierVersion()` at the pinned block, which is
the value the immutable carries on chain.

A mismatch means the address does not hold the artifact that was reviewed, whatever the
deployment record claims. It never signs or submits.

Usage:
  scripts/check-evm-deployment.py --rpc-url URL --address 0x... \
      [--artifact contracts/evm/out/ErebusSettlement.sol/ErebusSettlement.json] \
      [--chain-id 10143] [--verifier-version 1]
"""

from __future__ import annotations

import argparse
import json
import sys
import urllib.request

VERIFIER_VERSION_SELECTOR = "0xa3f966a9"


def rpc(url: str, method: str, params: list) -> object:
    request = urllib.request.Request(
        url,
        data=json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=20) as response:
        body = json.loads(response.read())
    if "error" in body:
        raise RuntimeError(body["error"].get("message", "RPC error"))
    return body["result"]


def immutable_ranges(artifact: dict) -> list[tuple[int, int]]:
    references = artifact["deployedBytecode"].get("immutableReferences") or {}
    ranges = []
    for entries in references.values():
        for entry in entries:
            ranges.append((int(entry["start"]), int(entry["length"])))
    return ranges


def mask(code: bytes, ranges: list[tuple[int, int]]) -> bytes:
    masked = bytearray(code)
    for start, length in ranges:
        for index in range(start, min(start + length, len(masked))):
            masked[index] = 0
    return bytes(masked)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--rpc-url", required=True)
    parser.add_argument("--address", required=True)
    parser.add_argument(
        "--artifact",
        default="contracts/evm/out/ErebusSettlement.sol/ErebusSettlement.json",
    )
    parser.add_argument("--chain-id", type=int)
    parser.add_argument("--verifier-version", type=int)
    args = parser.parse_args()

    address = args.address.lower()
    if not address.startswith("0x") or len(address) != 42:
        print(json.dumps({"ok": False, "error": "invalid address"}))
        return 1

    if args.chain_id is not None:
        found = int(rpc(args.rpc_url, "eth_chainId", []), 16)
        if found != args.chain_id:
            print(json.dumps({"ok": False, "error": "chain id mismatch", "found": found}))
            return 1

    finalized = rpc(args.rpc_url, "eth_getBlockByNumber", ["finalized", False])
    if not finalized or not finalized.get("hash"):
        print(json.dumps({"ok": False, "error": "finalized block unavailable"}))
        return 1
    block_hash = finalized["hash"]
    pin = {"blockHash": block_hash, "requireCanonical": True}

    deployed = rpc(args.rpc_url, "eth_getCode", [address, pin])
    if not isinstance(deployed, str) or deployed in ("0x", "0x0"):
        print(json.dumps({"ok": False, "error": "no code at address", "block": block_hash}))
        return 1

    with open(args.artifact, "r", encoding="utf-8") as handle:
        artifact = json.load(handle)
    expected = artifact["deployedBytecode"]["object"]
    if expected.startswith("0x"):
        expected = expected[2:]

    ranges = immutable_ranges(artifact)
    deployed_masked = mask(bytes.fromhex(deployed[2:]), ranges)
    expected_masked = mask(bytes.fromhex(expected), ranges)
    code_ok = deployed_masked == expected_masked

    result: dict[str, object] = {
        "ok": code_ok,
        "address": address,
        "block": block_hash,
        "codeLength": len(deployed_masked),
        "artifactLength": len(expected_masked),
        "immutableRanges": len(ranges),
    }

    if args.verifier_version is not None:
        returned = rpc(
            args.rpc_url,
            "eth_call",
            [{"to": address, "data": VERIFIER_VERSION_SELECTOR}, pin],
        )
        if not isinstance(returned, str) or len(returned) < 66:
            result["ok"] = False
            result["verifierVersionError"] = "unexpected call result"
        else:
            on_chain = int(returned[2:], 16) & 0xFFFFFFFF
            result["verifierVersion"] = on_chain
            if on_chain != args.verifier_version:
                result["ok"] = False
                result["verifierVersionError"] = "verifier version mismatch"

    print(json.dumps(result, sort_keys=True))
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
