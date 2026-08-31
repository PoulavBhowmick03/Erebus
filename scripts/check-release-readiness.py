#!/usr/bin/env python3
"""Fail closed when an Erebus release is internally inconsistent or unproven."""

from __future__ import annotations

import argparse
import json
import re
import sys
import tomllib
from pathlib import Path
from typing import Any


MANIFESTS = (
    ("pyproject.toml", "toml"),
    ("agents/pyproject.toml", "toml"),
    ("sdk/py/pyproject.toml", "toml"),
    ("mcp-server/pyproject.toml", "toml"),
    ("packaging/erebus-cli/pyproject.toml", "toml"),
    ("sdk/rs/Cargo.toml", "cargo"),
    ("sdk/ts/package.json", "json"),
)
LOCK_PACKAGES = {
    "uv.lock": {
        "erebus",
        "erebus-agents",
        "erebus-cli",
        "erebus-mcp-server",
        "erebus-sdk",
    },
    "sdk/rs/Cargo.lock": {"erebus-sdk"},
}
EVIDENCE_PATH = Path("docs/runs/v0.2-mainnet-canary.json")
PREAMBLE_PATH = Path("docs/release-preamble.md")
BASELINE_PHRASES = (
    "unaudited",
    "not the relationship",
    "Linux x86-64 and macOS arm64",
)
BLOCKED_PREAMBLE_PHRASES = (
    "remain Sepolia-only",
    "this is not a mainnet-verified install",
)
TRANSACTIONS = (
    "shield",
    "channel_a_to_b",
    "channel_b_to_a",
    "proposal",
    "counter",
    "settlement",
)
CHECKS = (
    "screening_signature_present",
    "payment_change_conserved",
    "observer_terms_hidden",
    "recovery_verified",
    "disclosure_verified",
)
FORBIDDEN_KEY_PARTS = (
    "private_key",
    "secret",
    "mnemonic",
    "seed_phrase",
    "rpc_url",
    "api_key",
    "viewing_key",
    "pool_key",
    "account_key",
)
SEMVER = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?$")
HEX_64 = re.compile(r"^0x[0-9a-fA-F]{1,64}$")
COMMIT = re.compile(r"^[0-9a-f]{40}$")


def _manifest_version(path: Path, kind: str) -> str:
    if kind == "json":
        data = json.loads(path.read_text())
    else:
        data = tomllib.loads(path.read_text())
    if kind == "json":
        version = data.get("version")
    elif kind == "cargo":
        version = data.get("package", {}).get("version")
    else:
        version = data.get("project", {}).get("version")
    if not isinstance(version, str):
        raise ValueError("does not define a string package version")
    return version


def _walk_keys(value: Any, prefix: str = "") -> list[str]:
    found: list[str] = []
    if isinstance(value, dict):
        for key, child in value.items():
            key_text = str(key).lower()
            path = f"{prefix}.{key}" if prefix else str(key)
            if any(part in key_text for part in FORBIDDEN_KEY_PARTS):
                found.append(path)
            found.extend(_walk_keys(child, path))
    elif isinstance(value, list):
        for index, child in enumerate(value):
            found.extend(_walk_keys(child, f"{prefix}[{index}]"))
    return found


def _check_lock_versions(root: Path, expected: str) -> list[str]:
    errors: list[str] = []
    for relative, names in LOCK_PACKAGES.items():
        path = root / relative
        try:
            packages = tomllib.loads(path.read_text()).get("package", [])
        except (OSError, tomllib.TOMLDecodeError) as exc:
            errors.append(f"{relative}: {exc}")
            continue
        found = {
            package.get("name"): package.get("version")
            for package in packages
            if isinstance(package, dict) and package.get("name") in names
        }
        for name in sorted(names):
            actual = found.get(name)
            if actual != expected:
                errors.append(f"{relative}: {name} version {actual!r}, expected {expected!r}")
    return errors


def check(
    root: Path,
    *,
    expected_version: str | None = None,
    tag: str | None = None,
    require_mainnet: bool = False,
) -> list[str]:
    errors: list[str] = []
    versions: dict[str, str] = {}
    for relative, kind in MANIFESTS:
        path = root / relative
        try:
            versions[relative] = _manifest_version(path, kind)
        except (OSError, ValueError, json.JSONDecodeError, tomllib.TOMLDecodeError) as exc:
            errors.append(f"{relative}: {exc}")

    if versions:
        expected = expected_version or versions.get("pyproject.toml")
        if expected is None or not SEMVER.fullmatch(expected):
            errors.append(f"invalid expected version: {expected!r}")
        else:
            for relative, actual in versions.items():
                if actual != expected:
                    errors.append(f"{relative}: version {actual!r}, expected {expected!r}")
            errors.extend(_check_lock_versions(root, expected))
            if tag is not None and tag != f"v{expected}":
                errors.append(f"tag {tag!r}, expected 'v{expected}'")
    else:
        expected = expected_version

    preamble_path = root / PREAMBLE_PATH
    try:
        preamble = preamble_path.read_text()
    except OSError as exc:
        errors.append(f"{PREAMBLE_PATH}: {exc}")
        preamble = ""
    for phrase in BASELINE_PHRASES:
        if phrase not in preamble:
            errors.append(f"{PREAMBLE_PATH}: missing required phrase {phrase!r}")

    if not require_mainnet:
        return errors

    if str(EVIDENCE_PATH) not in preamble:
        errors.append(f"{PREAMBLE_PATH}: must link {EVIDENCE_PATH}")
    for phrase in BLOCKED_PREAMBLE_PHRASES:
        if phrase in preamble:
            errors.append(f"{PREAMBLE_PATH}: still contains pre-mainnet text {phrase!r}")

    evidence_path = root / EVIDENCE_PATH
    try:
        evidence = json.loads(evidence_path.read_text())
    except (OSError, json.JSONDecodeError) as exc:
        errors.append(f"{EVIDENCE_PATH}: {exc}")
        return errors
    if not isinstance(evidence, dict):
        errors.append(f"{EVIDENCE_PATH}: root must be an object")
        return errors

    forbidden = _walk_keys(evidence)
    if forbidden:
        errors.append(f"{EVIDENCE_PATH}: forbidden secret-bearing key(s): {', '.join(forbidden)}")

    required_values = {
        "release": expected,
        "package_version": expected,
        "status": "passed",
        "network": "SN_MAIN",
    }
    for key, wanted in required_values.items():
        if evidence.get(key) != wanted:
            errors.append(f"{EVIDENCE_PATH}: {key} must be {wanted!r}")
    source_commit = str(evidence.get("source_commit", ""))
    if not COMMIT.fullmatch(source_commit) or set(source_commit) == {"0"}:
        errors.append(f"{EVIDENCE_PATH}: source_commit must be a 40-character lowercase commit")
    for key in ("prover_version", "rpc_spec_version", "completed_at"):
        if not isinstance(evidence.get(key), str) or not evidence[key].strip():
            errors.append(f"{EVIDENCE_PATH}: {key} must be a non-empty string")

    transactions = evidence.get("transactions")
    if not isinstance(transactions, dict):
        errors.append(f"{EVIDENCE_PATH}: transactions must be an object")
    else:
        for name in TRANSACTIONS:
            item = transactions.get(name)
            tx_hash = item.get("transaction_hash") if isinstance(item, dict) else None
            if (
                not isinstance(tx_hash, str)
                or not HEX_64.fullmatch(tx_hash)
                or int(tx_hash, 16) == 0
            ):
                errors.append(f"{EVIDENCE_PATH}: transactions.{name}.transaction_hash is invalid")

    checks = evidence.get("checks")
    if not isinstance(checks, dict):
        errors.append(f"{EVIDENCE_PATH}: checks must be an object")
    else:
        for name in CHECKS:
            if checks.get(name) is not True:
                errors.append(f"{EVIDENCE_PATH}: checks.{name} must be true")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--version", help="expected package version; defaults to root manifest")
    parser.add_argument("--tag", help="tag that must equal v<version>")
    parser.add_argument("--require-mainnet", action="store_true")
    args = parser.parse_args()
    errors = check(
        args.repo_root.resolve(),
        expected_version=args.version,
        tag=args.tag,
        require_mainnet=args.require_mainnet,
    )
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print("release readiness checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
