#!/usr/bin/env python3
"""Qualify two downloaded registry-run artifacts and publish them to the Metropolis registry.

    publish-metropolis-release.py --artifacts DIR [--publish]

DIR holds `metropolis-registry-macos-arm64/` and `metropolis-registry-linux-x86_64/` exactly as
`gh run download <registry run>` writes them. Without --publish nothing leaves this machine.
With it, a new prerelease is created on PoulavBhowmick03/erebus-metropolis (never overwritten),
the wheels and release-manifest.json are uploaded, and the registry's Pages index is dispatched.
The stable Starknet index is never touched.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

REPOSITORY = "PoulavBhowmick03/erebus-metropolis"
PLATFORMS = {"macos-arm64": ("macosx_11_0_arm64", "macho-arm64"), "linux-x86_64": ("linux_x86_64", "elf-x86_64")}


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def native(blob: bytes, kind: str) -> bool:
    if kind == "macho-arm64":
        return blob[:4] == b"\xcf\xfa\xed\xfe" and int.from_bytes(blob[4:8], "little") == 0x0100000C
    return blob[:6] == b"\x7fELF\x02\x01" and int.from_bytes(blob[18:20], "little") == 62


def qualify(artifacts: Path) -> tuple[dict, dict[str, Path], list[str]]:
    manifests, assets, problems = {}, {}, []
    for platform, (tag, kind) in PLATFORMS.items():
        root = artifacts / f"metropolis-registry-{platform}"
        manifest = json.loads((root / "release.json").read_text())
        manifests[platform] = manifest
        if manifest["dirty_source"] or manifest["published"] or manifest.get("test_only_artifacts_included"):
            problems.append(f"{platform}: dirty, already published, or includes test-only artifacts")
        if manifest["platform_qualification"]["wheel_tag"] != tag:
            problems.append(f"{platform}: wheel tag {manifest['platform_qualification']['wheel_tag']} is not {tag}")
        if manifest.get("licenses", {}).get("project") != "Apache-2.0":
            problems.append(f"{platform}: manifest records no project license")
        for name, meta in manifest["wheels"].items():
            path = root / "wheels" / name
            data = path.read_bytes()
            if sha256(data) != meta["sha256"]:
                problems.append(f"{platform} {name}: hash does not match the manifest")
            with zipfile.ZipFile(path) as archive:
                names = archive.namelist()
                licenses = {n.rsplit("/", 1)[-1] for n in names if ".dist-info/licenses/" in n}
                record = [n for n in names if n.endswith(".dist-info/WHEEL")]
                tags = [line for line in archive.read(record[0]).decode().splitlines() if line.startswith("Tag:")]
                if name.endswith("-py3-none-any.whl"):
                    if "LICENSE" not in licenses:
                        problems.append(f"{name}: no LICENSE")
                else:
                    if tags != [f"Tag: py3-none-{tag}"] or not {"LICENSE", "THIRD_PARTY_NOTICES"} <= licenses:
                        problems.append(f"{name}: tag {tags} or licenses {sorted(licenses)}")
                    for binary, binary_meta in {**manifest["binaries"], **manifest.get("launchers", {})}.items():
                        hits = [n for n in names if n.endswith(f"/scripts/{binary}")]
                        blob = archive.read(hits[0]) if len(hits) == 1 else b""
                        if sha256(blob) != binary_meta["sha256"]:
                            problems.append(f"{platform} {binary}: missing or hash mismatch")
                        elif binary in manifest["binaries"] and not native(blob, kind):
                            problems.append(f"{platform} {binary}: not a {kind} executable")
            if name in assets and assets[name].read_bytes() != data:
                problems.append(f"{name}: differs between platforms")
            assets.setdefault(name, path)
        if len(manifest["binaries"]) != 14:
            problems.append(f"{platform}: expected 14 native binaries, found {len(manifest['binaries'])}")
    commits = {m["source_commit"] for m in manifests.values()}
    versions = {m["version"] for m in manifests.values()}
    if len(commits) != 1 or len(versions) != 1:
        problems.append(f"platforms disagree: commits {commits}, versions {versions}")
    version = versions.pop()
    combined = {"version": version, "tag": "v" + version, "channel": "metropolis-testnet",
                "source_commit": commits.pop(), "build_profile": manifests["macos-arm64"]["build_profile"],
                "platforms": {m["platform"]: m for m in manifests.values()}}
    return combined, assets, problems


def gh(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(["gh", *args], capture_output=True, text=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--publish", action="store_true")
    args = parser.parse_args()
    combined, assets, problems = qualify(args.artifacts.resolve())
    summary = {"version": combined["version"], "source_commit": combined["source_commit"],
               "assets": sorted(assets), "problems": problems}
    if problems:
        print(json.dumps({"status": "not_qualified", **summary}, indent=2))
        raise SystemExit(1)
    tag = combined["tag"]
    if gh("release", "view", tag, "--repo", REPOSITORY).returncode == 0:
        print(json.dumps({"status": "refused", "reason": f"{tag} already exists; releases are never overwritten"}))
        raise SystemExit(1)
    if not args.publish:
        print(json.dumps({"status": "qualified", "published": False, **summary}, indent=2))
        return
    with tempfile.TemporaryDirectory() as staging:
        manifest = Path(staging) / "release-manifest.json"
        manifest.write_text(json.dumps(combined, indent=2) + "\n")
        created = gh("release", "create", tag, "--repo", REPOSITORY, "--prerelease", "--title", f"Metropolis {combined['version']}",
                     "--notes", f"Built from {combined['source_commit']}. Experimental Metropolis testnet packages; "
                     "see release-manifest.json for wheel, native, launcher, and license hashes and platform qualifications.")
        if created.returncode:
            sys.exit(f"release creation failed: {created.stderr.strip()}")
        uploaded = gh("release", "upload", tag, "--repo", REPOSITORY, *[str(path) for path in assets.values()], str(manifest))
        if uploaded.returncode:
            sys.exit(f"upload failed after creating {tag}; fix the assets of that release, do not recreate it: {uploaded.stderr.strip()}")
    dispatched = gh("api", f"repos/{REPOSITORY}/dispatches", "-f", "event_type=metropolis-registry", "-f", f"client_payload[tag]={tag}")
    print(json.dumps({"status": "published", "tag": tag, "pages_dispatched": dispatched.returncode == 0, **summary}, indent=2))


if __name__ == "__main__":
    main()
