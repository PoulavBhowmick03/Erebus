#!/usr/bin/env python3
"""Build the cumulative Metropolis index from hash-checked release assets."""

import argparse
import hashlib
import html
import json
import re
from pathlib import Path

PROJECTS = {"erebus-cli", "erebus-sdk", "erebus-mcp-server"}
RELEASE_URL = "https://github.com/PoulavBhowmick03/erebus-metropolis/releases/download"
TAG = re.compile(r"v\d+\.\d+\.\d+\.dev\d+")


def build_index(releases: Path, output: Path, required_tag: str) -> None:
    if not TAG.fullmatch(required_tag):
        raise ValueError("invalid dispatched prerelease tag")
    projects = {}
    found = set()
    for release in sorted(releases.iterdir()):
        tag = release.name
        if release.is_symlink() or not release.is_dir() or not TAG.fullmatch(tag):
            raise ValueError("invalid release directory")
        found.add(tag)
        manifest_path = release / "release-manifest.json"
        if manifest_path.is_symlink() or not manifest_path.is_file():
            raise ValueError(f"{tag}: missing regular manifest")
        manifest = json.loads(manifest_path.read_text())
        if manifest.get("tag") != tag:
            raise ValueError(f"{tag}: release manifest names a different tag")
        platforms = manifest.get("platforms", {})
        if len(platforms) != 2:
            raise ValueError(f"{tag}: expected two platform manifests")
        tags = [data.get("platform_qualification", {}).get("wheel_tag") for data in platforms.values()]
        if set(tags) != {"linux_x86_64", "macosx_11_0_arm64"}:
            raise ValueError(f"{tag}: platform qualifications are not the supported pair")
        release_projects = set()
        for data in platforms.values():
            platform_projects = set()
            for name, meta in data["wheels"].items():
                if Path(name).name != name or "\\" in name:
                    raise ValueError(f"{tag}: invalid wheel filename")
                parts = name.split("-")
                if len(parts) != 5 or parts[1] != tag[1:] or not name.endswith(".whl"):
                    raise ValueError(f"{tag}: wheel version or filename does not match the release")
                project = parts[0].replace("_", "-").lower()
                if project not in PROJECTS:
                    raise ValueError(f"{tag}: unexpected project")
                expected_platform = data["platform_qualification"]["wheel_tag"] if project == "erebus-cli" else "any"
                if parts[2:] != ["py3", "none", f"{expected_platform}.whl"]:
                    raise ValueError(f"{tag}: wheel tag does not match its platform qualification")
                local = release / name
                if local.is_symlink() or not local.is_file():
                    raise ValueError(f"{tag}/{name}: missing regular release asset")
                digest = meta["sha256"]
                if not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest):
                    raise ValueError(f"{tag}/{name}: invalid manifest hash")
                with local.open("rb") as stream:
                    actual = hashlib.file_digest(stream, "sha256").hexdigest()
                if actual != digest:
                    raise ValueError(f"{tag}/{name}: manifest hash does not match the asset")
                files = projects.setdefault(project, {})
                if name in files and files[name][1] != digest:
                    raise ValueError(f"{name}: published twice with different contents")
                files[name] = (tag, digest)
                platform_projects.add(project)
                release_projects.add(project)
            if platform_projects != PROJECTS or len(data["wheels"]) != 3:
                raise ValueError(f"{tag}: each platform must contain all three projects")
        if release_projects != PROJECTS:
            raise ValueError(f"{tag}: missing projects")
    if required_tag not in found:
        raise ValueError("dispatched tag is not a downloaded published release")

    # No output is created until every release passes the same checks.
    output.mkdir(parents=True, exist_ok=False)
    (output / "index.html").write_text(
        "<!doctype html>\n" + "\n".join(f'<a href="{project}/">{project}</a><br>' for project in sorted(projects))
    )
    for project, files in sorted(projects.items()):
        directory = output / project
        directory.mkdir()
        links = [
            f'<a href="{RELEASE_URL}/{tag}/{html.escape(name, quote=True)}#sha256={digest}">'
            f"{html.escape(name)}</a><br>"
            for name, (tag, digest) in sorted(files.items())
        ]
        (directory / "index.html").write_text("<!doctype html>\n" + "\n".join(links))
    print(f"indexed {len(projects)} projects from {len(found)} releases")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--releases", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--tag", required=True)
    args = parser.parse_args()
    build_index(args.releases, args.output, args.tag)
