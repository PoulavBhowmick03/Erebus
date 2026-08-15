#!/usr/bin/env python3
"""Inspect public Starknet calldata for Erebus negotiation leakage.

The observer has no channel key, pool key, local state, or viewing grant. It accepts either
a Sepolia transaction hash or a JSON fixture containing ``calldata``:

    python3 scripts/observer.py 0x44289c4cacce0d07f45a6a788313ad341f44f40fd905c181a1e525050384bb7
    python3 scripts/observer.py scripts/fixtures/observer-wire-v2.json

Content recovery and traffic classification are separate findings. This script implements
the public wire-v1 codec as a positive control. It never receives the wire-v2 channel key.
The fifth-salt fingerprint uses only the public fixed-zero shape.

Unreviewed.
"""

from __future__ import annotations

import argparse
import dataclasses
import json
from pathlib import Path
import sys
import urllib.error
import urllib.request
from typing import Any, Sequence

DEFAULT_RPC = "https://starknet-sepolia-rpc.publicnode.com"

FLAG_BIT = 1 << 119
PAYLOAD_MASK = FLAG_BIT - 1
SALT_LIMIT = 1 << 120
LEGACY_NOTES_PER_MESSAGE = 4

FIELDS = (
    ("message_type", 8),
    ("reply_to", 32),
    ("created_at", 40),
    ("amount", 128),
    ("deadline", 64),
    ("memo_hash", 128),
)
MESSAGE_BITS = sum(width for _, width in FIELDS)
MESSAGE_TYPES = {1: "offer", 2: "counter", 3: "accept"}
NO_REPLY_TO = (1 << 32) - 1

# A stable validity range, not a claim about the current wall clock. It rejects random
# ciphertext while retaining all plausible protocol messages from 2020 through 2100.
MIN_PROTOCOL_TIME = 1_577_836_800
MAX_PROTOCOL_TIME = 4_102_444_800


class ObserverError(Exception):
    """Input, RPC, or fixture failure."""


@dataclasses.dataclass(frozen=True)
class Transcript:
    """A plausible wire-v1 transcript recovered without any key."""

    message_type: str
    reply_to: int | None
    created_at: int
    amount: int
    deadline: int
    memo_hash: int


@dataclasses.dataclass(frozen=True)
class Analysis:
    """Independent content-recovery and traffic-fingerprint verdicts."""

    calldata_felts: int
    public_salts: tuple[int, ...]
    recovered: tuple[Transcript, ...]
    fingerprint_salts: tuple[int, ...]

    @property
    def content_recovered(self) -> bool:
        """Whether the public legacy decoder recovered any plausible transcript."""
        return bool(self.recovered)

    @property
    def classified_as_erebus(self) -> bool:
        """Whether any salt has wire v2's 59-zero-bit fifth-slot shape."""
        return bool(self.fingerprint_salts)


def _felt(value: object) -> int:
    if isinstance(value, bool):
        raise ObserverError("boolean is not a calldata felt")
    if isinstance(value, int):
        parsed = value
    elif isinstance(value, str):
        try:
            parsed = int(value, 0)
        except ValueError as error:
            raise ObserverError(f"invalid calldata felt: {value!r}") from error
    else:
        raise ObserverError(f"invalid calldata felt: {value!r}")
    if parsed < 0:
        raise ObserverError(f"negative calldata felt: {parsed}")
    return parsed


def _calldata_from_json(value: object) -> list[object]:
    """Accept a raw array, transaction object, or JSON-RPC response fixture."""
    candidate = value
    if isinstance(candidate, dict) and "result" in candidate:
        candidate = candidate["result"]
    if isinstance(candidate, dict) and "transaction" in candidate:
        candidate = candidate["transaction"]
    if isinstance(candidate, dict):
        candidate = candidate.get("calldata")
    if not isinstance(candidate, list):
        raise ObserverError("fixture/RPC response does not contain a calldata array")
    return candidate


def load_fixture(path: Path) -> list[int]:
    """Read public calldata from a local JSON fixture."""
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except OSError as error:
        raise ObserverError(f"cannot read fixture {path}: {error}") from error
    except json.JSONDecodeError as error:
        raise ObserverError(f"fixture {path} is not valid JSON: {error}") from error
    return [_felt(item) for item in _calldata_from_json(value)]


def fetch_transaction(tx_hash: str, rpc_url: str) -> list[int]:
    """Fetch a public transaction by hash from Starknet JSON-RPC."""
    body = json.dumps(
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "starknet_getTransactionByHash",
            "params": [tx_hash],
        }
    ).encode()
    request = urllib.request.Request(
        rpc_url,
        data=body,
        headers={"content-type": "application/json", "user-agent": "erebus-observer/2"},
    )
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            value: Any = json.load(response)
    except (OSError, urllib.error.URLError, json.JSONDecodeError) as error:
        raise ObserverError(f"transaction RPC failed: {error}") from error
    if isinstance(value, dict) and value.get("error") is not None:
        raise ObserverError(f"transaction RPC returned an error: {value['error']}")
    return [_felt(item) for item in _calldata_from_json(value)]


def salts_from_calldata(calldata: Sequence[int]) -> tuple[int, ...]:
    """Extract public high-half salts, preserving first-seen calldata order.

    A compiled server action contains the same ``packed_value`` in its storage write and
    event. Deduplicating those public repetitions reconstructs the note order used by the
    old live observer. A repeated 119-bit chunk inside one message is theoretically
    possible; the harness reports first-seen candidates and does not claim to be a general
    transaction parser.
    """
    found: list[int] = []
    for felt in calldata:
        salt = felt >> 128
        if FLAG_BIT <= salt < SALT_LIMIT and salt not in found:
            found.append(salt)
    return tuple(found)


def _decode_legacy(salts: Sequence[int]) -> Transcript | None:
    message = 0
    for index, salt in enumerate(salts):
        message |= (salt & PAYLOAD_MASK) << (119 * index)

    decoded: dict[str, int] = {}
    offset = MESSAGE_BITS
    for name, width in FIELDS:
        offset -= width
        decoded[name] = (message >> offset) & ((1 << width) - 1)

    kind = MESSAGE_TYPES.get(decoded["message_type"])
    created_at = decoded["created_at"]
    deadline = decoded["deadline"]
    if (
        kind is None
        or not MIN_PROTOCOL_TIME <= created_at <= MAX_PROTOCOL_TIME
        or not created_at <= deadline <= MAX_PROTOCOL_TIME
    ):
        return None

    reply_raw = decoded["reply_to"]
    return Transcript(
        message_type=kind,
        reply_to=None if reply_raw == NO_REPLY_TO else reply_raw,
        created_at=created_at,
        amount=decoded["amount"],
        deadline=deadline,
        memo_hash=decoded["memo_hash"],
    )


def recover_transcripts(salts: Sequence[int]) -> tuple[Transcript, ...]:
    """Try the known-broken public wire-v1 decoder at every four-salt window."""
    recovered: list[Transcript] = []
    for start in range(len(salts) - LEGACY_NOTES_PER_MESSAGE + 1):
        transcript = _decode_legacy(salts[start : start + LEGACY_NOTES_PER_MESSAGE])
        if transcript is not None and transcript not in recovered:
            recovered.append(transcript)
    return tuple(recovered)


def has_v2_fifth_salt_shape(salt: int) -> bool:
    """Test bit 119 pinned, bits 60..118 zero, without reading any content bits."""
    return salt >> 60 == 1 << 59


def analyse_calldata(calldata: Sequence[int]) -> Analysis:
    """Run the no-key content and shape attacks over public calldata."""
    salts = salts_from_calldata(calldata)
    return Analysis(
        calldata_felts=len(calldata),
        public_salts=salts,
        recovered=recover_transcripts(salts),
        fingerprint_salts=tuple(salt for salt in salts if has_v2_fifth_salt_shape(salt)),
    )


def _print(analysis: Analysis, source: str) -> None:
    print(f"source: {source}")
    print(f"public calldata: {analysis.calldata_felts} felts")
    print(f"candidate note salts: {len(analysis.public_salts)}")

    print("\nCONTENT RECOVERY (no channel key)")
    if not analysis.recovered:
        print("not recovered: the public wire-v1 decoder found no plausible transcript")
    for index, transcript in enumerate(analysis.recovered, start=1):
        print(f"recovered transcript {index}:")
        print(f"  type: {transcript.message_type}")
        print(f"  reply_to: {transcript.reply_to}")
        print(f"  created_at: {transcript.created_at}")
        print(f"  amount: {transcript.amount}")
        print(f"  deadline: {transcript.deadline}")
        print(f"  memo_hash: {transcript.memo_hash:#x}")

    print("\nTRAFFIC CLASSIFICATION (shape only)")
    if analysis.classified_as_erebus:
        print(
            "likely Erebus wire-v2 traffic: found "
            f"{len(analysis.fingerprint_salts)} salt(s) with bit 119 set and bits 60..118 zero"
        )
        print(
            "an unrelated uniform 120-bit salt matches with probability 2^-60 "
            "(2^-59 after conditioning on bit 119 being set)"
        )
    else:
        print("not classified as wire-v2 by the fixed fifth-salt shape")


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", help="Sepolia transaction hash or local JSON fixture")
    parser.add_argument(
        "--rpc-url",
        default=DEFAULT_RPC,
        help=f"Starknet JSON-RPC URL for transaction hashes (default: {DEFAULT_RPC})",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    path = Path(args.source)
    try:
        if path.is_file():
            calldata = load_fixture(path)
        elif args.source.startswith("0x"):
            calldata = fetch_transaction(args.source, args.rpc_url)
        else:
            raise ObserverError("source must be an existing JSON file or a 0x transaction hash")
        _print(analyse_calldata(calldata), args.source)
    except ObserverError as error:
        print(f"observer error: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
