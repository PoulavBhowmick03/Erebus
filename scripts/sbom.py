#!/usr/bin/env python3
"""Generates a CycloneDX 1.5 software bill of materials from the lockfiles.

Reads `sdk/rs/Cargo.lock` and `uv.lock` and emits one document covering both dependency
trees. No network access and no third-party tool: the lockfiles already pin every name,
version, and hash, and a generator that resolved anything itself could disagree with what
actually ships.

Deterministic. Components are sorted, so two runs on the same lockfiles produce identical
bytes and a diff means a dependency really moved.

    python3 scripts/sbom.py > sbom.json
    python3 scripts/sbom.py --check      # non-zero if a dependency lacks a hash

The `--check` mode exists because an unhashed dependency is one nobody can verify was not
swapped in transit, and a release should not be the moment that gets noticed.
"""

from __future__ import annotations

import argparse
import json
import sys
import tomllib
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[1]
CARGO_LOCK = REPO_ROOT / "sdk" / "rs" / "Cargo.lock"
UV_LOCK = REPO_ROOT / "uv.lock"

SPEC_VERSION = "1.5"


def _cargo_components() -> list[dict[str, Any]]:
    data = tomllib.loads(CARGO_LOCK.read_text())
    out: list[dict[str, Any]] = []
    for pkg in data.get("package", []):
        source = pkg.get("source")
        # No source means a path dependency: this repository's own crate, not something
        # pulled from a registry. It belongs in metadata, not in the dependency list, and
        # leaving it here would make --check report a permanently missing hash.
        if not source:
            continue

        name = pkg["name"]
        version = pkg["version"]
        component: dict[str, Any] = {
            "type": "library",
            "name": name,
            "version": version,
            "purl": f"pkg:cargo/{name}@{version}",
            "scope": "required",
            "externalReferences": [{"type": "distribution", "url": source}],
        }
        checksum = pkg.get("checksum")
        if checksum:
            component["hashes"] = [{"alg": "SHA-256", "content": checksum}]
        out.append(component)
    return out


def _uv_components() -> list[dict[str, Any]]:
    data = tomllib.loads(UV_LOCK.read_text())
    out: list[dict[str, Any]] = []
    for pkg in data.get("package", []):
        name = pkg["name"]
        version = pkg.get("version")
        source = pkg.get("source", {})
        # Workspace members are this repository, not third-party supply chain. They appear
        # in the metadata section instead.
        if "editable" in source or "virtual" in source:
            continue
        if version is None:
            continue

        component: dict[str, Any] = {
            "type": "library",
            "name": name,
            "version": version,
            "purl": f"pkg:pypi/{name}@{version}",
            "scope": "required",
        }
        # Prefer the sdist hash, then the first wheel. uv records one hash per artifact and
        # a package may ship many wheels; the first is enough to identify the release.
        artifact = pkg.get("sdist") or (pkg.get("wheels") or [None])[0]
        if artifact and artifact.get("hash", "").startswith("sha256:"):
            component["hashes"] = [
                {"alg": "SHA-256", "content": artifact["hash"].removeprefix("sha256:")}
            ]
        if artifact and artifact.get("url"):
            component["externalReferences"] = [
                {"type": "distribution", "url": artifact["url"]}
            ]
        out.append(component)
    return out


def build() -> dict[str, Any]:
    components = _cargo_components() + _uv_components()
    components.sort(key=lambda c: (c["purl"]))

    root_version = tomllib.loads((REPO_ROOT / "pyproject.toml").read_text())["project"][
        "version"
    ]

    return {
        "bomFormat": "CycloneDX",
        "specVersion": SPEC_VERSION,
        "version": 1,
        "metadata": {
            # No timestamp on purpose. It would change every run and turn a byte-identical
            # rebuild into a diff, which defeats checking that the SBOM matches the release.
            "component": {
                "type": "application",
                "name": "erebus",
                "version": root_version,
                "description": (
                    "Private coordination and settlement infrastructure for AI agents "
                    "on Starknet"
                ),
            },
            "tools": [{"name": "scripts/sbom.py", "vendor": "erebus"}],
        },
        "components": components,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="exit non-zero if any dependency has no hash",
    )
    args = parser.parse_args()

    bom = build()

    if args.check:
        unhashed = [c["purl"] for c in bom["components"] if "hashes" not in c]
        cargo = sum(1 for c in bom["components"] if c["purl"].startswith("pkg:cargo/"))
        pypi = sum(1 for c in bom["components"] if c["purl"].startswith("pkg:pypi/"))
        print(f"{len(bom['components'])} components: {cargo} cargo, {pypi} pypi")
        if unhashed:
            print(f"{len(unhashed)} without a hash:", file=sys.stderr)
            for purl in unhashed:
                print(f"  {purl}", file=sys.stderr)
            return 1
        print("every component carries a SHA-256")
        return 0

    json.dump(bom, sys.stdout, indent=2, sort_keys=True)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
